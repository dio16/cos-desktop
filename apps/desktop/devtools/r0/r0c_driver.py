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
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path

import contracts
import driver
import r0c_contracts as r0c
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
    return {
        "capture_sha256": "sha256:" + hashlib.sha256(raw).hexdigest(),
        "observation_sha256": observation_sha256,
        "clock_mapping": str(capture.get("clock", {}).get("mapping", "NONE")),
        "counts": kinds,
        "summary": {key: summary[key] for key in (
            "journal_seen", "journal_lost", "eligible", "retained", "retention_dropped",
            "excluded", "input_unassociated", "unmatched_submission", "undrained_journal",
        )},
        "scope_ns": capture["scope_end_ns"] - capture["scope_start_ns"],
    }


def _achieved(actions: list[dict]) -> tuple[dict[str, float | None], dict[str, int]]:
    per_kind: dict[str, list[int]] = {}
    delivered: dict[str, int] = {}
    for action in actions:
        if action["actual_ns"] is not None:
            per_kind.setdefault(action["kind"], []).append(action["actual_ns"])
        if action["classification"] in ("on-time", "early", "late"):
            delivered[action["kind"]] = delivered.get(action["kind"], 0) + 1
    achieved: dict[str, float | None] = {}
    for kind, times in per_kind.items():
        count = delivered.get(kind, 0)
        if len(times) < 2:
            # A single sample has no interval; a cadence would be fabricated.
            achieved[kind] = None
            continue
        span = (max(times) - min(times)) / 1_000_000_000
        achieved[kind] = round(count / span, 3) if span > 0 else None
    return achieved, delivered


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
        if type(delivered_flag) is not bool or type(suppressed) is not bool:
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
    return {
        "method": "wall-clock-capture-lifetime",
        "inside": True,
        "input_start_wall_ns": input_start,
        "input_end_wall_ns": input_end,
        "capture_start_wall_ns": capture_start_wall_ns,
        "capture_end_wall_ns": capture_end_wall_ns,
        "scope_ns": scope_ns,
        "scope_source": "s2b-manifest-duration",
        "journal_clock_mapping": journal_clock_mapping,
    }


def assemble_result(manifest: dict, trace: dict, probe: dict, journal: dict | None,
                    wrapper_exit: int, cleanup: dict, status: str, reason: str,
                    limits: dict, delivered_actions: int, correspondence: dict) -> dict:
    actions = probe["actions"]
    requested: dict[str, float] = {}
    for action in manifest["actions"]:
        kind = action["kind"]
        requested[kind] = max(requested.get(kind, 0.0), float(action["requested_hz"]))
    achieved, _ = _achieved(actions)
    for kind in requested:
        achieved.setdefault(kind, None)
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
        "requested_cadence": {kind: requested[kind] for kind in requested},
        "achieved_cadence": {kind: achieved[kind] for kind in requested},
        "actions": actions,
        "journal": journal_document,
        "endpoints": endpoints,
        "cleanup": cleanup,
        "limits": limits,
        "delivered_actions": delivered_actions,
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
    probe_timeout = manifest["scenario"]["active_end_ms"] / 1000 + r0c.CLOSE_TIMEOUT_MS / 1000 + 30
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

    if probe_path.exists():
        if probe_path.stat().st_size > MAX_PROBE_BYTES:
            raise DriverError("PROBE_OUTPUT_TOO_LARGE")
        probe = json.loads(probe_path.read_text(encoding="utf-8"))
    else:
        probe = None
    if probe is None:
        probe = {
            "generation": generation,
            "binding": {"bound": False},
            "window": {"pid": 0, "title": manifest["window"]["title"]},
            "actions": [
                {
                    "sequence": action["sequence"],
                    "kind": action["kind"],
                    "planned_ns": action["planned_ns"],
                    "actual_ns": None,
                    "delivered": False,
                    "suppressed": False,
                    "classification": "failed",
                }
                for action in trace["actions"]
            ],
            "close": {"posted": False},
        }
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
            "inside": False,
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
    reason = _result_reason(status, wrapper_exit, timed_out, close_posted)
    if probe_exit != 0:
        reason = "PROBE_" + (probe_stderr.strip().splitlines()[-1] if probe_stderr.strip() else "FAILED")
    if journal_error:
        status, reason = "FAILED", journal_error
    if probe_error:
        status, reason = "FAILED", probe_error
    if correspondence_error:
        status, reason = "FAILED", correspondence_error
    if delivered_actions == 0:
        status, reason = "FAILED", "WORKLOAD_NOT_DELIVERED"
    elif delivered_actions != len(trace["actions"]):
        status, reason = "FAILED", "WORKLOAD_INCOMPLETE"
    result = assemble_result(
        manifest, trace, probe, journal, wrapper_exit, cleanup, status, reason,
        limits, delivered_actions, correspondence,
    )
    r0c.validate_result(result)
    if r0c.result_bytes(result) > MAX_RESULT_BYTES:
        raise DriverError("RESULT_TOO_LARGE")
    driver.exclusive_write(output / "r0c-plan.json", trace)
    driver.exclusive_write(output / "r0c-probe.json", probe)
    driver.exclusive_write(output / "r0c-result.json", result)
    print(json.dumps({
        "ok": status == "COMPLETE",
        "execution_id": execution,
        "status": status,
        "reason": reason,
        "requested_cadence": result["requested_cadence"],
        "achieved_cadence": result["achieved_cadence"],
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
    achieved, _ = _achieved(result["actions"])
    for kind, requested in result["requested_cadence"].items():
        expected_value = achieved.get(kind)
        if expected_value != result["achieved_cadence"][kind]:
            raise r0c.ContractError("CADENCE_MISMATCH", f"$.achieved_cadence.{kind}")
        if requested <= 0:
            raise r0c.ContractError("CADENCE_MISMATCH", f"$.requested_cadence.{kind}")
    counts = {int(key): value for key, value in result["journal"]["counts"].items()}
    if result["endpoints"] != r0c.endpoint_matrix(counts):
        raise r0c.ContractError("ENDPOINT_MISMATCH", "$.endpoints")
    correspondence = result["time_correspondence"]
    inside = (
        correspondence["inside"] is True
        and correspondence["input_start_wall_ns"] >= correspondence["capture_start_wall_ns"]
        and correspondence["input_end_wall_ns"] <= correspondence["capture_end_wall_ns"]
        and correspondence["input_end_wall_ns"] <= correspondence["capture_start_wall_ns"] + correspondence["scope_ns"]
    )
    if not inside:
        raise r0c.ContractError("CAPTURE_RANGE_MISMATCH", "$.time_correspondence")
    if result["status"] == "COMPLETE" and delivered != len(result["actions"]):
        raise r0c.ContractError("WORKLOAD_INCOMPLETE", "$.delivered_actions")
    return {"delivered_actions": delivered, "actions": len(result["actions"])}


def verify(args: argparse.Namespace) -> int:
    manifest = r0c.load_manifest(args.manifest)
    result = r0c.validate_result(contracts.load_document(args.result, r0c.MAX_RESULT_BYTES))
    if result["status"] != "COMPLETE":
        raise r0c.ContractError("NOT_COMPLETE", "$.status")
    # Recompute from the per-action record before trusting any aggregate.
    recompute(result, manifest)
    output = Path(args.output_dir)
    capture_path = output / "s2b-capture.json"
    observation_path = output / "s2b-observation.json"
    if not capture_path.is_file() or not observation_path.is_file():
        raise r0c.ContractError("EVIDENCE_MISSING", "$.journal")
    if result["journal"].get("source") != "s2b-journal":
        raise r0c.ContractError("EVIDENCE_MISSING", "$.journal.source")
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
