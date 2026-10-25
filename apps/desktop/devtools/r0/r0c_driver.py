"""MP-R0C-S1 workload driver: deterministic planned trace, bounded execution.

Development-only, Python standard library only. The driver validates a versioned
workload manifest, verifies the approved executable/helper/fixture binding, runs
the existing approved S2B capture wrapper, replays the planned trace through the
Windows synthetic input probe, and records requested versus achieved cadence.
It never promotes a missing endpoint, never treats synthetic input as a human
session, and never converts journal event counts into presented frames.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path

import contracts
import driver
import r0c_contracts as r0c
import s2b_capture
import s2b_contracts as s2b

MAX_PROBE_BYTES = 2 * 1024 * 1024
MAX_RESULT_BYTES = r0c.MAX_RESULT_BYTES
DRAIN_S = 5.0
MAX_TIMEOUT_MS = r0c.MAX_DURATION_MS


class DriverError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def _approved_binding(manifest: dict) -> dict:
    identity = manifest["identity"]
    matches = [
        entry for entry in driver.approved_digests()
        if entry["product_commit"] == identity["repository_sha"]
        and entry["executable_sha256"] == identity["executable_sha256"]
    ]
    if len(matches) != 1:
        raise DriverError("SOURCE_NOT_APPROVED")
    return matches[0]


def _require_identity_agreement(manifest: dict, s2b_manifest: dict, binding: dict) -> None:
    identity = manifest["identity"]
    recorded = s2b_manifest["identity"]
    if recorded["repository"]["sha"] != identity["repository_sha"]:
        raise DriverError("IDENTITY_MISMATCH")
    if recorded["executable_sha256"] != identity["executable_sha256"]:
        raise DriverError("IDENTITY_MISMATCH")
    if recorded["gpui_revision"] != identity["gpui_revision"]:
        raise DriverError("IDENTITY_MISMATCH")
    if s2b_manifest["workload"]["source"]["sha256"] != manifest["scenario"]["fixture_sha256"]:
        raise DriverError("IDENTITY_MISMATCH")
    if set(recorded["build"]["features"]) != set(identity["features"]):
        raise DriverError("IDENTITY_MISMATCH")
    if recorded["build"]["profile"] != identity["build_profile"]:
        raise DriverError("IDENTITY_MISMATCH")
    if binding["helper_sha256"] != identity["helper_sha256"]:
        raise DriverError("HELPER_NOT_APPROVED")
    if binding["product_commit"] != identity["repository_sha"]:
        raise DriverError("SOURCE_NOT_APPROVED")


def _tasklist() -> Path:
    root = os.environ.get("SystemRoot")
    if not root:
        raise DriverError("SYSTEM_ROOT_UNAVAILABLE")
    path = Path(root) / "System32" / "tasklist.exe"
    if not path.is_file():
        raise DriverError("TASKLIST_UNAVAILABLE")
    return path


def _pids() -> set[int]:
    import csv
    import io

    completed = subprocess.run(
        [str(_tasklist()), "/FI", "IMAGENAME eq cos-desktop.exe", "/FO", "CSV", "/NH"],
        capture_output=True, text=True, encoding="utf-8-sig", errors="replace", timeout=15,
    )
    if completed.returncode != 0:
        raise DriverError("TASKLIST_FAILED")
    found = set()
    for row in csv.reader(io.StringIO(completed.stdout)):
        if len(row) >= 2 and row[0].lower() == "cos-desktop.exe":
            try:
                found.add(int(row[1]))
            except ValueError:
                pass
    return found


def _run_bounded(command: list[str], *, timeout_s: float, env: dict | None = None) -> tuple[int, str, str]:
    """Run a child with a wall-clock bound and a bounded in-memory capture.

    Readers drain both pipes continuously and stop appending after the cap, so a
    noisy child cannot force unbounded memory before the post-run truncation.
    """
    process = subprocess.Popen(
        command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env=env,
    )
    buffers = {"out": bytearray(), "err": bytearray()}

    def drain(pipe, key: str) -> None:
        while True:
            chunk = pipe.read(65536)
            if not chunk:
                return
            room = MAX_PROBE_BYTES - len(buffers[key])
            if room > 0:
                buffers[key].extend(chunk[:room])

    readers = [
        threading.Thread(target=drain, args=(process.stdout, "out"), daemon=True),
        threading.Thread(target=drain, args=(process.stderr, "err"), daemon=True),
    ]
    for reader in readers:
        reader.start()
    try:
        process.wait(timeout=timeout_s)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=DRAIN_S)
        for reader in readers:
            reader.join(timeout=DRAIN_S)
        for pipe in (process.stdout, process.stderr):
            if pipe is not None:
                pipe.close()
        return 124, "", "TIMEOUT"
    for reader in readers:
        reader.join(timeout=DRAIN_S)
    for pipe in (process.stdout, process.stderr):
        if pipe is not None:
            pipe.close()
    return (
        process.returncode,
        buffers["out"].decode("utf-8", "replace"),
        buffers["err"].decode("utf-8", "replace"),
    )


def _terminate(process: subprocess.Popen | None) -> None:
    if process is None:
        return
    if process.poll() is None:
        process.kill()
    try:
        process.communicate(timeout=DRAIN_S)
    except subprocess.TimeoutExpired:
        pass


def run_probe(plan_path: Path, out_path: Path, manifest: dict, excluded: set[int],
              probe: str, timeout_s: float, launcher_pid: int, generation: int) -> tuple[int, str, str]:
    command = [
        sys.executable, "-B", str(Path(probe).resolve()),
        "--plan", str(plan_path),
        "--out", str(out_path),
        "--title", manifest["window"]["title"],
        "--generation", str(generation),
        "--launcher-pid", str(launcher_pid),
        "--excluded-pids", ",".join(str(pid) for pid in sorted(excluded)),
        "--max-lag-ms", str(r0c.MAX_LAG_MS),
        "--close-timeout-ms", str(r0c.CLOSE_TIMEOUT_MS),
        "--close-after-ms", str(manifest["lifecycle"]["close_after_ms"]),
    ]
    return _run_bounded(command, timeout_s=timeout_s)


def _load_json(path: Path, limit: int) -> object:
    return contracts.load_document(str(path), limit)


def _journal(capture_path: Path, observation_path: Path, observation: dict,
             executable: str) -> dict:
    raw = capture_path.read_bytes()
    observation_sha256 = "sha256:" + hashlib.sha256(observation_path.read_bytes()).hexdigest()
    capture = s2b.validate_capture(
        s2b.parse_bytes(raw, s2b.MAX_BYTES),
        execution=observation["identity"]["execution_id"],
        pid=observation["identity"]["pid"],
        executable=executable,
    )
    summary = capture["summary"]
    records = capture["records"]
    kinds: dict[str, int] = {}
    for record in records:
        kinds[str(record["kind"])] = kinds.get(str(record["kind"]), 0) + 1
    input_dispatch_ns = sorted(
        int(record["start_ns"]) for record in records if record["kind"] == 0
    )
    return {
        "capture_sha256": "sha256:" + hashlib.sha256(raw).hexdigest(),
        "observation_sha256": observation_sha256,
        "clock_mapping": str(capture.get("clock", {}).get("mapping", "NONE")),
        "input_dispatch_ns": input_dispatch_ns,
        "counts": kinds,
        "summary": {key: summary[key] for key in (
            "journal_seen", "journal_lost", "eligible", "retained", "retention_dropped",
            "excluded", "input_unassociated", "unmatched_submission", "undrained_journal",
        )},
        "scope_ns": capture["scope_end_ns"] - capture["scope_start_ns"],
    }


def _cadence(times: list[int]) -> float | None:
    """Rate from actual timestamps; a single sample has no interval and is null."""
    if len(times) < 2:
        return None
    span = (max(times) - min(times)) / 1_000_000_000
    return round((len(times) - 1) / span, 3) if span > 0 else None


def _observed_cadence(actions: list[dict], journal: dict | None,
                      kinds: list[str]) -> dict:
    attempts: dict[str, list[int]] = {kind: [] for kind in kinds}
    successes: dict[str, list[int]] = {kind: [] for kind in kinds}
    for action in actions:
        if action.get("attempted") and action["actual_ns"] is not None:
            attempts.setdefault(action["kind"], []).append(action["actual_ns"])
        if action.get("delivered") and action["actual_ns"] is not None:
            successes.setdefault(action["kind"], []).append(action["actual_ns"])
    application = None
    if journal is not None:
        application = _cadence(list(journal.get("input_dispatch_ns", [])))
    return {
        "posting_attempt_hz": {kind: _cadence(attempts.get(kind, [])) for kind in kinds},
        "postmessage_success_hz": {kind: _cadence(successes.get(kind, [])) for kind in kinds},
        "application_processing_hz": application,
    }


CADENCE_NOTE = (
    "postmessage_success_hz is PostMessage success, not application processing; "
    "application_processing_hz counts retained input-dispatch journal records only. "
    "A larger posted count must never be read as a larger processed count."
)


def verify_probe(probe: dict, manifest: dict, trace: dict, pre_run_pids: set[int],
                 launcher_pid: int, observation_pid: int, generation: int) -> int:
    """Bind the probe to the approved child and re-derive every classification.

    Returns the number of delivered actions. A probe that echoes a different
    generation, window, trace or classification is rejected rather than trusted.
    """
    binding = probe.get("binding") or {}
    if binding.get("bound") is not True:
        raise DriverError("WINDOW_NOT_BOUND")
    if binding.get("launcher_pid") != launcher_pid:
        raise DriverError("WINDOW_LAUNCHER_MISMATCH")
    if probe.get("generation") != generation:
        raise DriverError("WINDOW_GENERATION_MISMATCH")
    window = probe.get("window") or {}
    if window.get("title") != manifest["window"]["title"]:
        raise DriverError("WINDOW_IDENTITY_MISMATCH")
    pid = window.get("pid")
    if type(pid) is not int or pid <= 0 or pid in pre_run_pids:
        raise DriverError("WINDOW_GENERATION_MISMATCH")
    if pid != observation_pid:
        raise DriverError("WINDOW_PID_MISMATCH")
    if binding.get("window_pid") != pid:
        raise DriverError("WINDOW_PID_MISMATCH")
    planned = {action["sequence"]: action for action in trace["actions"]}
    seen: set[int] = set()
    delivered = 0
    for action in probe.get("actions", []):
        sequence = action.get("sequence")
        if sequence not in planned or sequence in seen:
            raise DriverError("PROBE_TRACE_MISMATCH")
        expected = planned[sequence]
        if action.get("kind") != expected["kind"]:
            raise DriverError("PROBE_TRACE_MISMATCH")
        if action.get("planned_ns") != expected["planned_ns"]:
            raise DriverError("PROBE_TRACE_MISMATCH")
        delivered_flag = action.get("delivered")
        suppressed = action.get("suppressed")
        attempted = action.get("attempted")
        if type(delivered_flag) is not bool or type(suppressed) is not bool or type(attempted) is not bool:
            raise DriverError("PROBE_TRACE_MISMATCH")
        interval_ns = max(1, int(round(1_000_000_000 / expected["requested_hz"])))
        expected_class = r0c.classify(
            expected["planned_ns"], action.get("actual_ns"), delivered_flag,
            interval_ns=interval_ns, suppressed=suppressed,
        )
        if action.get("classification") != expected_class:
            raise DriverError("PROBE_CLASSIFICATION_MISMATCH")
        if delivered_flag:
            delivered += 1
        seen.add(sequence)
    if seen != set(planned):
        raise DriverError("PROBE_TRACE_INCOMPLETE")
    return delivered


def check_time_correspondence(probe: dict, capture_start_wall_ns: int,
                              capture_end_wall_ns: int, scope_ns: int,
                              journal_clock_mapping: str = "NONE") -> dict:
    """Prove the input range is inside the capture's wall-clock lifetime and scope.

    The journal clock has no wall anchor in this build, so the conservative
    boundary is the capture process lifetime plus the declared scope duration:
    input ending before capture_start + scope_ns cannot be after the journal scope
    end, because the journal scope starts after the capture process starts.
    """
    values = probe.get("time_correspondence") or {}
    try:
        input_start = int(values["input_start_wall_ns"])
        input_end = int(values["input_end_wall_ns"])
    except (KeyError, TypeError, ValueError):
        raise DriverError("CAPTURE_RANGE_UNKNOWN")
    if input_start <= 0 or input_end < input_start:
        raise DriverError("CAPTURE_RANGE_UNKNOWN")
    if capture_start_wall_ns <= 0 or capture_end_wall_ns <= capture_start_wall_ns or scope_ns <= 0:
        raise DriverError("CAPTURE_RANGE_UNKNOWN")
    if input_start < capture_start_wall_ns or input_end > capture_end_wall_ns:
        raise DriverError("CAPTURE_RANGE_MISMATCH")
    if input_end > capture_start_wall_ns + scope_ns:
        raise DriverError("CAPTURE_RANGE_MISMATCH")
    reason = (
        "NO_COMMON_CLOCK: journal clock mapping is NONE"
        if journal_clock_mapping == "NONE"
        else "NO_VERIFIABLE_SYNC_POINT: journal clock mapping has no input anchor"
    )
    return {
        "method": "wall-clock-capture-lifetime",
        # This is proven from the same wall clock. It is not the journal's range.
        "capture_lifetime_inside": True,
        # Without a common clock or a verified sync point, containment in the
        # journal's observed range is UNKNOWN and is never claimed.
        "journal_range": "UNKNOWN",
        "journal_range_reason": reason,
        "input_start_wall_ns": input_start,
        "input_end_wall_ns": input_end,
        "capture_start_wall_ns": capture_start_wall_ns,
        "capture_end_wall_ns": capture_end_wall_ns,
        "scope_ns": scope_ns,
        "scope_source": "s2b-manifest-duration",
        "journal_clock_mapping": journal_clock_mapping,
    }


def journal_containment(input_start_journal_ns: int, input_end_journal_ns: int,
                        scope_start_ns: int, scope_end_ns: int) -> str:
    """Containment decision for a run with a verified journal-clock mapping."""
    if input_start_journal_ns < scope_start_ns or input_end_journal_ns > scope_end_ns:
        return "MISMATCH"
    return "PROVEN"


def assemble_result(manifest: dict, trace: dict, probe: dict, journal: dict | None,
                    wrapper_exit: int, cleanup: dict, status: str, reason: str,
                    limits: dict, delivered_actions: int, correspondence: dict,
                    execution_id: str) -> dict:
    actions = probe["actions"]
    requested: dict[str, float] = {}
    for action in manifest["actions"]:
        kind = action["kind"]
        requested[kind] = max(requested.get(kind, 0.0), float(action["requested_hz"]))
    observed_cadence = _observed_cadence(actions, journal, list(requested))
    counts = {}
    if journal is not None:
        counts = {int(key): value for key, value in journal.get("counts", {}).items()}
    endpoints = r0c.endpoint_matrix(counts)
    journal_document = {
        "source": "s2b-journal" if journal is not None else "none",
        "counts": (journal or {}).get("counts", {}),
        "note": "journal event counts are not presented-frame counts",
    }
    if journal is not None and "capture_sha256" in journal:
        journal_document["capture_sha256"] = journal["capture_sha256"]
    if journal is not None and "observation_sha256" in journal:
        journal_document["observation_sha256"] = journal["observation_sha256"]
    return {
        "result_version": r0c.RESULT_VERSION,
        "scenario": {
            "id": manifest["scenario"]["id"],
            "version": manifest["scenario"]["version"],
            "family": manifest["scenario"]["family"],
            "seed": manifest["scenario"]["seed"],
        },
        "trace_sha256": trace["trace_sha256"],
        "execution_id": execution_id,
        "requested_cadence": {kind: requested[kind] for kind in requested},
        "observed_cadence": observed_cadence,
        "cadence_note": CADENCE_NOTE,
        "actions": actions,
        "journal": journal_document,
        "endpoints": endpoints,
        "cleanup": cleanup,
        "limits": limits,
        "delivered_actions": delivered_actions,
        "probe_identity": {
            "generation": probe.get("generation"),
            "window_pid": (probe.get("window") or {}).get("pid"),
            "launcher_pid": (probe.get("binding") or {}).get("launcher_pid"),
            "helper_pid": (probe.get("binding") or {}).get("helper_pid"),
        },
        "time_correspondence": correspondence,
        "status": status,
        "reason": reason,
    }


def _result_reason(status: str, wrapper_exit: int, timed_out: bool, close_posted: bool) -> str:
    if timed_out:
        return "CAPTURE_TIMEOUT"
    if wrapper_exit != 0:
        return "CAPTURE_REFUSED"
    if not close_posted:
        return "CLOSE_NOT_POSTED"
    if status != "COMPLETE":
        return "NOT_COMPLETE"
    return ""


REQUIRED_S2B_EVIDENCE = (
    "s2b-capture.json",
    "s2b-observation.json",
    "ledger.json",
    "build-record.json",
    "source-identity.json",
    "completion.json",
    "result.json",
    "endpoint-availability.json",
)


def probe_shape_error(probe: object) -> str:
    """Structural validation before a probe is trusted; any failure is corrupt."""
    if not isinstance(probe, dict):
        return "NOT_OBJECT"
    for key in ("binding", "window", "close", "time_correspondence"):
        if not isinstance(probe.get(key), dict):
            return key.upper()
    if not isinstance(probe.get("actions"), list) or not probe["actions"]:
        return "ACTIONS"
    if type(probe.get("binding_lost")) is not bool or type(probe.get("cancelled")) is not bool:
        return "STATE"
    close = probe["close"]
    if type(close.get("posted")) is not bool or type(close.get("window_gone")) is not bool:
        return "CLOSE"
    if probe.get("generation") is not None and type(probe.get("generation")) is not int:
        return "GENERATION"
    for action in probe["actions"]:
        if not isinstance(action, dict):
            return "ACTION"
        if type(action.get("sequence")) is not int:
            return "SEQUENCE"
        if not isinstance(action.get("kind"), str):
            return "KIND"
        if type(action.get("planned_ns")) is not int:
            return "PLANNED"
        if action.get("actual_ns") is not None and type(action.get("actual_ns")) is not int:
            return "ACTUAL"
        for key in ("attempted", "delivered", "suppressed"):
            if type(action.get(key)) is not bool:
                return key.upper()
        if not isinstance(action.get("classification"), str):
            return "CLASSIFICATION"
    return ""


def load_probe(path: Path) -> tuple[dict | None, str]:
    """Return (probe, state); a missing and a corrupt probe are different states."""
    if not path.exists():
        return None, "MISSING"
    if path.stat().st_size > MAX_PROBE_BYTES:
        return None, "CORRUPT"
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None, "CORRUPT"
    if probe_shape_error(document):
        return None, "CORRUPT"
    return document, "OK"


PROBE_CLASSIFICATIONS = ("on-time", "early", "late", "suppressed", "failed")


def probe_satisfies_contract(probe: dict, trace: dict) -> bool:
    """Every probe field the result contract needs must already be valid."""
    if probe_shape_error(probe):
        return False
    planned = {action["sequence"]: action for action in trace["actions"]}
    if [action["sequence"] for action in probe["actions"]] != [
        action["sequence"] for action in trace["actions"]
    ]:
        return False
    for action in probe["actions"]:
        expected = planned.get(action["sequence"])
        if expected is None:
            return False
        if action["kind"] != expected["kind"] or action["planned_ns"] != expected["planned_ns"]:
            return False
        if action["classification"] not in PROBE_CLASSIFICATIONS:
            return False
    return True


def resolve_probe(path: Path, trace: dict, title: str) -> tuple[dict, str]:
    """Return a contract-satisfiable probe, or the bounded fallback with CORRUPT."""
    probe, state = load_probe(path)
    if state != "OK":
        return fallback_probe(trace, title), state
    if not probe_satisfies_contract(probe, trace):
        return fallback_probe(trace, title), "CORRUPT"
    return probe, "OK"


def fallback_probe(trace: dict, title: str) -> dict:
    return {
        "generation": None,
        "binding": {"bound": False},
        "window": {"pid": None, "title": title},
        "actions": [
            {
                "sequence": action["sequence"],
                "kind": action["kind"],
                "planned_ns": action["planned_ns"],
                "actual_ns": None,
                "attempted": False,
                "delivered": False,
                "suppressed": False,
                "classification": "failed",
            }
            for action in trace["actions"]
        ],
        "close": {"posted": False, "window_gone": False},
        "binding_lost": False,
        "cancelled": False,
    }


def write_stderr_log(path: Path, data: str) -> bool:
    """Best-effort bounded sidecar; a log failure never blocks the result."""
    try:
        with path.open("xb") as handle:
            handle.write(data.encode("utf-8")[:8_192])
        return True
    except OSError:
        return False


def probe_exit_code(probe_exit: int) -> str:
    return "PROBE_TIMEOUT" if probe_exit == 124 else "PROBE_EXITED"


def failure_reason(probe_state: str, probe: dict | None, probe_exit: int,
                   probe_error: str, journal_error: str,
                   correspondence_error: str, cleanup: dict, delivered: int,
                   planned: int) -> str:
    """One deterministic priority order for the FAILED record's reason."""
    if probe_state == "MISSING":
        return "PROBE_MISSING"
    if probe_state == "CORRUPT":
        return "PROBE_CORRUPT"
    if probe is not None and (probe.get("binding_lost") is True or probe.get("cancelled") is True):
        return "PROBE_BINDING_LOST"
    if probe_error:
        return probe_error
    if probe_exit != 0:
        return probe_exit_code(probe_exit)
    if journal_error:
        return journal_error
    if correspondence_error:
        return correspondence_error
    if not cleanup.get("window_gone"):
        return "CLEANUP_UNCONFIRMED"
    if delivered == 0:
        return "WORKLOAD_NOT_DELIVERED"
    if delivered != planned:
        return "WORKLOAD_INCOMPLETE"
    return ""


def require_observed_disposition(bundle: dict) -> None:
    disposition = bundle.get("disposition")
    if disposition == "OBSERVED":
        return
    codes = {
        "CANCELLED": "S2B_DISPOSITION_CANCELLED",
        "FAILED": "S2B_DISPOSITION_FAILED",
        "NOT_MEASURED": "S2B_DISPOSITION_NOT_MEASURED",
    }
    raise r0c.ContractError(codes.get(disposition, "S2B_DISPOSITION_INVALID"),
                            "$.journal.disposition")


def build_bundle_view(output: Path, view: Path, link=os.link) -> str:
    """Materialise the S2B view: hardlinks when possible, validated copy otherwise."""
    method = "hardlink"
    for entry in output.iterdir():
        name = entry.name
        if name in {"r0c-plan.json", "r0c-probe.json", "r0c-result.json", "r0c-probe.stderr.txt"}:
            continue
        if s2b_capture.MARKER.fullmatch(name):
            if entry.is_symlink() or not entry.is_dir():
                raise r0c.ContractError("EVIDENCE_TYPE", "$.journal")
            s2b_capture.require_marker(output, name)
            (view / name).mkdir()
            continue
        if name not in s2b_capture.ARTIFACT_FILES:
            raise r0c.ContractError("EVIDENCE_UNEXPECTED_ENTRY", "$.journal")
        if entry.is_symlink() or not entry.is_file():
            raise r0c.ContractError("EVIDENCE_TYPE", "$.journal")
        data = s2b_capture.read_pinned(entry, s2b_capture.ARTIFACT_FILES[name])
        try:
            link(entry, view / name)
        except OSError:
            # Cross-volume or link-hostile filesystem: same Pin-validated bytes,
            # written into the view instead of linked.
            method = "copy"
            with (view / name).open("xb") as handle:
                handle.write(data)
    return method


def check_s2b_bundle(output: Path, manifest_path: str, timeout_ms: int) -> dict:
    """Validate the existing S2B publication without modifying it.

    The R0C driver adds r0c-* evidence after the S2B publication; the S2B replay
    rejects unknown entries. A temporary hardlink view omits only those R0C files,
    then the existing verify_bundle/materialized_copy/require_marker safety path
    validates types, links, marker names, per-file limits and totals. The original
    is never modified.
    """
    known_r0c = {
        "r0c-plan.json", "r0c-probe.json", "r0c-result.json", "r0c-probe.stderr.txt",
    }
    missing = [name for name in REQUIRED_S2B_EVIDENCE if not (output / name).is_file()]
    if missing:
        raise r0c.ContractError("EVIDENCE_MISSING", "$.journal")
    try:
        with tempfile.TemporaryDirectory() as directory:
            view = Path(directory) / "output"
            view.mkdir()
            build_bundle_view(output, view)
            return s2b_capture.verify_bundle(
                Path(manifest_path).resolve(), view, timeout_ms
            )
    except (RuntimeError, contracts.ContractError, s2b.ObservationError, OSError):
        raise r0c.ContractError("S2B_VERIFICATION_FAILED", "$.journal")


def launch(args: argparse.Namespace) -> int:
    if not (1_000 <= args.timeout_ms <= MAX_TIMEOUT_MS):
        raise DriverError("TIMEOUT_OUT_OF_RANGE")
    manifest = r0c.load_manifest(args.manifest)
    s2b_manifest = s2b.validate_manifest(
        contracts.load_document(args.s2b_manifest, contracts.MAX_MANIFEST_BYTES)
    )
    binding = _approved_binding(manifest)
    _require_identity_agreement(manifest, s2b_manifest, binding)
    exe, helper, fixture = Path(args.executable), Path(args.helper), Path(args.fixture)
    if _sha256_file(exe) != binding["executable_sha256"]:
        raise DriverError("EXECUTABLE_DIGEST_MISMATCH")
    if _sha256_file(helper) != binding["helper_sha256"]:
        raise DriverError("HELPER_DIGEST_MISMATCH")
    if _sha256_file(fixture) != manifest["scenario"]["fixture_sha256"]:
        raise DriverError("FIXTURE_DIGEST_MISMATCH")
    expected_helper = driver.approved_helper_path(binding)
    if os.path.normcase(str(helper.resolve())) != os.path.normcase(str(expected_helper)):
        raise DriverError("HELPER_NOT_APPROVED")

    output, profile = Path(args.output_dir), Path(args.profile)
    driver.require_empty_directory(output)
    execution = uuid.uuid4().hex
    if not re.fullmatch(r"[0-9a-f]{32}", execution):
        raise DriverError("EXECUTION_IDENTITY_INVALID")
    generation = int(execution[:8], 16) & 0x7FFFFFFF
    (profile / execution / "roaming").mkdir(parents=True)
    (profile / execution / "temp").mkdir()
    scratch = Path(args.scratch) if args.scratch else Path(os.environ.get("TEMP", ".")) / ("r0c-s1-" + execution)
    if scratch.exists() and any(scratch.iterdir()):
        raise DriverError("SCRATCH_NOT_EMPTY")
    scratch.mkdir(parents=True, exist_ok=True)
    plan_path, probe_path = scratch / "plan.json", scratch / "probe.json"

    trace = r0c.plan_trace(manifest)
    r0c.write_trace(trace, str(plan_path))

    pre_run_pids = _pids() if sys.platform == "win32" else set()
    probe_command = args.probe or str(Path(__file__).resolve().with_name("r0c_input.py"))
    probe_timeout = (
        max(manifest["scenario"]["active_end_ms"], manifest["lifecycle"]["close_after_ms"])
        / 1000 + r0c.CLOSE_TIMEOUT_MS / 1000 + 30
    )
    capture_process: subprocess.Popen | None = None
    capture_start_wall_ns = time.time_ns()
    try:
        capture_process = subprocess.Popen(
            [
                sys.executable, "-B", str(Path(args.s2b_capture).resolve()),
                "--manifest", str(Path(args.s2b_manifest).resolve()),
                "--executable", str(exe.resolve()),
                "--fixture", str(fixture.resolve()),
                "--output-dir", str(output.resolve()),
                "--helper", str(helper.resolve()),
                "--profile", str((profile / execution).resolve()),
                "--timeout-ms", str(args.timeout_ms),
            ],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        probe_exit, probe_stdout, probe_stderr = run_probe(
            plan_path, probe_path, manifest, pre_run_pids, probe_command, probe_timeout,
            capture_process.pid, generation,
        )
        timed_out = probe_exit == 124
        cancelled = probe_exit != 0
        if cancelled and capture_process.poll() is None:
            capture_process.kill()
        try:
            capture_stdout, capture_stderr = capture_process.communicate(
                timeout=args.timeout_ms / 1000 + 30
            )
        except subprocess.TimeoutExpired:
            capture_process.kill()
            capture_stdout, capture_stderr = capture_process.communicate(timeout=DRAIN_S)
            timed_out = True
        wrapper_exit = capture_process.returncode
        capture_end_wall_ns = time.time_ns()
    except BaseException:
        _terminate(capture_process)
        raise

    probe, probe_state = resolve_probe(
        probe_path, trace, manifest["window"]["title"]
    )
    close_posted = bool(probe.get("close", {}).get("posted"))
    journal = None
    journal_error = ""
    observation_path = output / "s2b-observation.json"
    capture_path = output / "s2b-capture.json"
    observation_pid = 0
    if observation_path.exists() and capture_path.exists():
        observation = _load_json(observation_path, s2b.MAX_BYTES)
        if observation["identity"]["execution_id"] != execution:
            raise DriverError("EXECUTION_IDENTITY_MISMATCH")
        observation_pid = int(observation["identity"]["pid"])
        try:
            journal = _journal(capture_path, observation_path, observation,
                               binding["executable_sha256"])
        except (s2b.ObservationError, contracts.ContractError, OSError):
            journal_error = "JOURNAL_INVALID"
    probe_error = ""
    delivered_actions = 0
    try:
        delivered_actions = verify_probe(
            probe, manifest, trace, pre_run_pids, capture_process.pid, observation_pid, generation
        )
    except DriverError as error:
        probe_error = error.code

    # The journal clock has no wall mapping, so the conservative capture bound is
    # the S2B manifest's declared duration from the capture process start.
    scope_ns = int(s2b_manifest["workload"]["duration_ms"]) * 1_000_000
    journal_clock_mapping = str(journal.get("clock_mapping", "NONE")) if journal is not None else "NONE"
    correspondence_error = ""
    try:
        correspondence = check_time_correspondence(
            probe, capture_start_wall_ns, capture_end_wall_ns, scope_ns,
            journal_clock_mapping,
        )
    except DriverError as error:
        correspondence_error = error.code
        correspondence = {
            "method": "wall-clock-capture-lifetime",
            "capture_lifetime_inside": False,
            "journal_range": "UNKNOWN",
            "journal_range_reason": correspondence_error,
            "input_start_wall_ns": 0,
            "input_end_wall_ns": 0,
            "capture_start_wall_ns": max(capture_start_wall_ns, 0),
            "capture_end_wall_ns": max(capture_end_wall_ns, 0),
            "scope_ns": max(scope_ns, 0),
            "scope_source": "s2b-manifest-duration",
            "journal_clock_mapping": journal_clock_mapping,
        }

    cleanup = {
        "wrapper_exit": wrapper_exit,
        "child_reaped": capture_process.poll() is not None,
        "close_posted": close_posted,
        "window_gone": bool(probe.get("close", {}).get("window_gone")),
        "timed_out": timed_out,
        "cancelled": cancelled,
    }
    limits = {
        "max_actions": r0c.MAX_ACTIONS,
        "max_lag_ms": r0c.MAX_LAG_MS,
        "probe_timeout_ms": int(probe_timeout * 1000),
        "capture_timeout_ms": int(args.timeout_ms),
        "output_bytes": MAX_PROBE_BYTES,
    }
    status = "COMPLETE"
    if probe_exit != 0 or timed_out or wrapper_exit != 0 or journal is None or not close_posted:
        status = "FAILED"
    reason = failure_reason(
        probe_state, probe, probe_exit, probe_error, journal_error,
        correspondence_error, cleanup, delivered_actions, len(trace["actions"]),
    )
    if reason:
        status = "FAILED"
    else:
        reason = _result_reason(status, wrapper_exit, timed_out, close_posted)
    result = assemble_result(
        manifest, trace, probe, journal, wrapper_exit, cleanup, status, reason,
        limits, delivered_actions, correspondence, execution,
    )
    r0c.validate_result(result)
    if r0c.result_bytes(result) > MAX_RESULT_BYTES:
        raise DriverError("RESULT_TOO_LARGE")
    if probe_stderr:
        write_stderr_log(output / "r0c-probe.stderr.txt", probe_stderr)
    try:
        driver.exclusive_write(output / "r0c-plan.json", trace)
        driver.exclusive_write(output / "r0c-probe.json", probe)
        driver.exclusive_write(output / "r0c-result.json", result)
    except OSError:
        raise DriverError("RESULT_WRITE_FAILED")
    print(json.dumps({
        "ok": status == "COMPLETE",
        "execution_id": execution,
        "status": status,
        "reason": reason,
        "requested_cadence": result["requested_cadence"],
        "observed_cadence": result["observed_cadence"],
        "delivered_actions": result["delivered_actions"],
        "journal_input_dispatch": result["journal"]["counts"].get("0", 0),
        "journal": result["journal"]["counts"],
        "endpoints_not_measured": [
            item["name"] for item in result["endpoints"] if item["status"] == "NOT_MEASURED"
        ],
    }, sort_keys=True))
    return 0 if status == "COMPLETE" else 1


def recompute(result: dict, manifest: dict) -> dict:
    """Recompute every derived number from the per-action record.

    Anything that cannot be recomputed is not reported as verified.
    """
    r0c.validate_result(result)
    r0c.check_identity(manifest, result)
    trace = r0c.plan_trace(manifest)
    if result["trace_sha256"] != trace["trace_sha256"]:
        raise r0c.ContractError("TRACE_MISMATCH", "$.trace_sha256")
    expected_sequences = [action["sequence"] for action in trace["actions"]]
    actual_sequences = [action["sequence"] for action in result["actions"]]
    if actual_sequences != expected_sequences:
        # Missing, duplicated or reordered entries are not a reproduction.
        raise r0c.ContractError("TRACE_SEQUENCE_MISMATCH", "$.actions")
    if len(set(actual_sequences)) != len(actual_sequences):
        raise r0c.ContractError("TRACE_SEQUENCE_MISMATCH", "$.actions")
    requested_expected: dict[str, float] = {}
    for action in manifest["actions"]:
        kind = action["kind"]
        requested_expected[kind] = max(
            requested_expected.get(kind, 0.0), float(action["requested_hz"])
        )
    if result["requested_cadence"] != requested_expected:
        raise r0c.ContractError("REQUESTED_CADENCE_MISMATCH", "$.requested_cadence")
    planned = {action["sequence"]: action for action in trace["actions"]}
    delivered = 0
    for action in result["actions"]:
        expected = planned.get(action["sequence"])
        if expected is None or expected["kind"] != action["kind"] or expected["planned_ns"] != action["planned_ns"]:
            raise r0c.ContractError("TRACE_MISMATCH", "$.actions")
        interval_ns = max(1, int(round(1_000_000_000 / expected["requested_hz"])))
        expected_class = r0c.classify(
            action["planned_ns"], action["actual_ns"], action["delivered"],
            interval_ns=interval_ns, suppressed=action["suppressed"],
        )
        if action["classification"] != expected_class:
            raise r0c.ContractError("CLASSIFICATION_MISMATCH", "$.actions")
        if action["delivered"]:
            delivered += 1
    if delivered != result["delivered_actions"]:
        raise r0c.ContractError("DELIVERED_MISMATCH", "$.delivered_actions")
    observed = _observed_cadence(
        result["actions"], None, list(result["requested_cadence"])
    )
    for name in ("posting_attempt_hz", "postmessage_success_hz"):
        if observed[name] != result["observed_cadence"][name]:
            raise r0c.ContractError("CADENCE_MISMATCH", f"$.observed_cadence.{name}")
    for kind, requested in result["requested_cadence"].items():
        if requested <= 0:
            raise r0c.ContractError("CADENCE_MISMATCH", f"$.requested_cadence.{kind}")
    counts = {int(key): value for key, value in result["journal"]["counts"].items()}
    if result["endpoints"] != r0c.endpoint_matrix(counts):
        raise r0c.ContractError("ENDPOINT_MISMATCH", "$.endpoints")
    correspondence = result["time_correspondence"]
    inside = (
        correspondence["capture_lifetime_inside"] is True
        and correspondence["input_start_wall_ns"] >= correspondence["capture_start_wall_ns"]
        and correspondence["input_end_wall_ns"] <= correspondence["capture_end_wall_ns"]
        and correspondence["input_end_wall_ns"] <= correspondence["capture_start_wall_ns"] + correspondence["scope_ns"]
    )
    if not inside:
        raise r0c.ContractError("CAPTURE_RANGE_MISMATCH", "$.time_correspondence")
    if correspondence["journal_range"] == "MISMATCH":
        raise r0c.ContractError("JOURNAL_RANGE_MISMATCH", "$.time_correspondence.journal_range")
    if result["status"] == "COMPLETE" and delivered != len(result["actions"]):
        raise r0c.ContractError("WORKLOAD_INCOMPLETE", "$.delivered_actions")
    return {"delivered_actions": delivered, "actions": len(result["actions"])}


def check_probe_identity(identity: dict, observation: dict) -> None:
    """Bind the result's probe identity to the capture's stored identity."""
    execution = str(observation["identity"]["execution_id"])
    if len(execution) != 32:
        raise r0c.ContractError("EXECUTION_ID_MISMATCH", "$.execution_id")
    if identity.get("generation") != int(execution[:8], 16) & 0x7FFFFFFF:
        raise r0c.ContractError("PROBE_GENERATION_MISMATCH", "$.probe_identity.generation")
    if identity.get("window_pid") != observation["identity"]["pid"]:
        raise r0c.ContractError("PROBE_PID_MISMATCH", "$.probe_identity.window_pid")


def check_close(probe: dict, cleanup: dict) -> None:
    """The probe's close record and the cleanup record must agree."""
    close = probe.get("close") or {}
    if close.get("posted") != cleanup.get("close_posted"):
        raise r0c.ContractError("CLOSE_MISMATCH", "$.cleanup.close_posted")
    if close.get("window_gone") != cleanup.get("window_gone"):
        raise r0c.ContractError("CLOSE_MISMATCH", "$.cleanup.window_gone")


def check_termination(ledger: dict, cleanup: dict) -> None:
    """Cross-check the launcher's termination evidence; abnormal is never success."""
    if (
        ledger.get("timed_out") is not False
        or ledger.get("exit_code") != 0
        or ledger.get("child_reaped") is not True
        or ledger.get("cleanup") != "complete"
    ):
        raise r0c.ContractError("TERMINATION_ABNORMAL", "$.cleanup")
    if cleanup.get("timed_out") is not False or cleanup.get("wrapper_exit") != 0:
        raise r0c.ContractError("TERMINATION_ABNORMAL", "$.cleanup")


def verify(args: argparse.Namespace) -> int:
    manifest = r0c.load_manifest(args.manifest)
    s2b_manifest = s2b.validate_manifest(
        contracts.load_document(args.s2b_manifest, contracts.MAX_MANIFEST_BYTES)
    )
    _require_identity_agreement(manifest, s2b_manifest, _approved_binding(manifest))
    result = r0c.validate_result(contracts.load_document(args.result, r0c.MAX_RESULT_BYTES))
    if result["status"] != "COMPLETE":
        raise r0c.ContractError("NOT_COMPLETE", "$.status")
    # Recompute from the per-action record before trusting any aggregate.
    recompute(result, manifest)
    output = Path(args.output_dir)
    probe_path = output / "r0c-probe.json"
    capture_path = output / "s2b-capture.json"
    observation_path = output / "s2b-observation.json"
    if not probe_path.is_file():
        raise r0c.ContractError("EVIDENCE_MISSING", "$.probe")
    probe = contracts.load_document(str(probe_path), MAX_PROBE_BYTES)
    if probe_shape_error(probe):
        raise r0c.ContractError("PROBE_CORRUPT", "$.probe")
    binding = probe.get("binding") or {}
    if binding.get("bound") is not True:
        raise r0c.ContractError("PROBE_BINDING_UNCONFIRMED", "$.probe.binding")
    if probe.get("binding_lost") is not False or probe.get("cancelled") is not False:
        raise r0c.ContractError("PROBE_BINDING_LOST", "$.probe.binding_lost")
    identity = result["probe_identity"]
    if any(identity[key] is None for key in
           ("generation", "window_pid", "launcher_pid", "helper_pid")):
        raise r0c.ContractError("PROBE_IDENTITY_INCOMPLETE", "$.probe_identity")
    if probe.get("actions") != result["actions"]:
        raise r0c.ContractError("PROBE_MISMATCH", "$.actions")
    sequences = [action.get("sequence") for action in probe.get("actions", [])]
    if len(set(sequences)) != len(sequences):
        raise r0c.ContractError("PROBE_SEQUENCE_DUPLICATE", "$.actions")
    identity = result["probe_identity"]
    if probe.get("generation") != identity["generation"]:
        raise r0c.ContractError("PROBE_IDENTITY_MISMATCH", "$.probe_identity.generation")
    window = probe.get("window") or {}
    if window.get("pid") != identity["window_pid"]:
        raise r0c.ContractError("PROBE_IDENTITY_MISMATCH", "$.probe_identity.window_pid")
    binding = probe.get("binding") or {}
    if binding.get("window_pid") != (probe.get("window") or {}).get("pid"):
        raise r0c.ContractError("PROBE_WINDOW_MISMATCH", "$.probe.binding.window_pid")
    if (probe.get("window") or {}).get("title") != manifest["window"]["title"]:
        raise r0c.ContractError("PROBE_TITLE_MISMATCH", "$.probe.window.title")
    if binding.get("launcher_pid") != identity["launcher_pid"]:
        raise r0c.ContractError("PROBE_IDENTITY_MISMATCH", "$.probe_identity.launcher_pid")
    if binding.get("helper_pid") != identity["helper_pid"]:
        raise r0c.ContractError("PROBE_IDENTITY_MISMATCH", "$.probe_identity.helper_pid")
    probe_time = probe.get("time_correspondence") or {}
    correspondence = result["time_correspondence"]
    if probe_time.get("input_start_wall_ns") != correspondence["input_start_wall_ns"]:
        raise r0c.ContractError("PROBE_TIME_MISMATCH", "$.time_correspondence.input_start_wall_ns")
    if probe_time.get("input_end_wall_ns") != correspondence["input_end_wall_ns"]:
        raise r0c.ContractError("PROBE_TIME_MISMATCH", "$.time_correspondence.input_end_wall_ns")
    if not capture_path.is_file() or not observation_path.is_file():
        raise r0c.ContractError("EVIDENCE_MISSING", "$.journal")
    if result["journal"].get("source") != "s2b-journal":
        raise r0c.ContractError("EVIDENCE_MISSING", "$.journal.source")
    observation = contracts.loads_document(observation_path.read_bytes().decode("utf-8"))
    execution = str(observation["identity"]["execution_id"])
    if result["execution_id"] != execution:
        raise r0c.ContractError("EXECUTION_ID_MISMATCH", "$.execution_id")
    check_probe_identity(result["probe_identity"], observation)
    ledger_path = output / "ledger.json"
    if not ledger_path.is_file():
        raise r0c.ContractError("EVIDENCE_MISSING", "$.cleanup")
    ledger = contracts.load_document(str(ledger_path), contracts.MAX_MANIFEST_BYTES)
    check_termination(ledger, result["cleanup"])
    bundle = check_s2b_bundle(
        output, args.s2b_manifest, int(result["limits"]["capture_timeout_ms"])
    )
    require_observed_disposition(bundle)
    check_close(probe, result["cleanup"])
    capture_bytes = capture_path.read_bytes()
    observation_bytes = observation_path.read_bytes()
    capture_sha = "sha256:" + hashlib.sha256(capture_bytes).hexdigest()
    observation_sha = "sha256:" + hashlib.sha256(observation_bytes).hexdigest()
    if result["journal"].get("capture_sha256") != capture_sha:
        raise r0c.ContractError("CAPTURE_DIGEST_MISMATCH", "$.journal.capture_sha256")
    if result["journal"].get("observation_sha256") != observation_sha:
        raise r0c.ContractError("OBSERVATION_DIGEST_MISMATCH", "$.journal.observation_sha256")
    observation = contracts.loads_document(observation_bytes.decode("utf-8"))
    capture = s2b.validate_capture(
        s2b.parse_bytes(capture_bytes, s2b.MAX_BYTES),
        execution=observation["identity"]["execution_id"],
        pid=observation["identity"]["pid"],
        executable=manifest["identity"]["executable_sha256"],
    )
    counts: dict[str, int] = {}
    for record in capture["records"]:
        counts[str(record["kind"])] = counts.get(str(record["kind"]), 0) + 1
    if counts != result["journal"]["counts"]:
        raise r0c.ContractError("JOURNAL_MISMATCH", "$.journal.counts")
    dispatch_ns = sorted(
        int(record["start_ns"]) for record in capture["records"] if record["kind"] == 0
    )
    if _cadence(dispatch_ns) != result["observed_cadence"]["application_processing_hz"]:
        raise r0c.ContractError(
            "APPLICATION_CADENCE_MISMATCH", "$.observed_cadence.application_processing_hz"
        )
    print(json.dumps({"ok": True, "status": result["status"]}, sort_keys=True))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="MP-R0C-S1 workload driver")
    sub = parser.add_subparsers(dest="command", required=True)
    plan = sub.add_parser("plan")
    plan.add_argument("--manifest", required=True)
    plan.add_argument("--out", required=True)
    run = sub.add_parser("run")
    for option in ("manifest", "output-dir", "profile", "executable", "fixture", "helper",
                   "s2b-manifest", "s2b-capture"):
        run.add_argument("--" + option, required=True)
    run.add_argument("--timeout-ms", type=int, default=120_000)
    run.add_argument("--scratch", default="")
    run.add_argument("--probe", default="")
    verify_parser = sub.add_parser("verify")
    verify_parser.add_argument("--manifest", required=True)
    verify_parser.add_argument("--result", required=True)
    verify_parser.add_argument("--output-dir", required=True)
    verify_parser.add_argument("--s2b-manifest", required=True)
    args = parser.parse_args(argv)
    if args.command == "plan":
        manifest = r0c.load_manifest(args.manifest)
        trace = r0c.plan_trace(manifest)
        r0c.write_trace(trace, args.out)
        print(json.dumps({"ok": True, "trace_sha256": trace["trace_sha256"],
                          "actions": len(trace["actions"])}, sort_keys=True))
        return 0
    if args.command == "run":
        return launch(args)
    return verify(args)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (r0c.ContractError, DriverError, contracts.ContractError, s2b.ObservationError) as error:
        code = getattr(error, "code", "DRIVER_FAILED")
        print(json.dumps({"ok": False, "error": str(code)}, sort_keys=True))
        raise SystemExit(1)
