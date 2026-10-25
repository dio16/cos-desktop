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
import time
import uuid
from pathlib import Path

import contracts
import driver
import r0c_contracts as r0c
import s2b_contracts as s2b

MAX_PROBE_BYTES = 2 * 1024 * 1024
PROBE_TIMEOUT_S = 120
DRAIN_S = 5.0


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


def _require_identity_agreement(manifest: dict, s2b_manifest: dict) -> None:
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


def _pids() -> set[int]:
    import csv
    import io

    completed = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq cos-desktop.exe", "/FO", "CSV", "/NH"],
        capture_output=True, text=True, encoding="utf-8-sig", errors="replace", timeout=15,
    )
    found = set()
    for row in csv.reader(io.StringIO(completed.stdout)):
        if len(row) >= 2 and row[0].lower() == "cos-desktop.exe":
            try:
                found.add(int(row[1]))
            except ValueError:
                pass
    return found


def _run_bounded(command: list[str], *, timeout_s: float, env: dict | None = None) -> tuple[int, str, str]:
    process = subprocess.Popen(
        command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, encoding="utf-8", errors="replace", env=env,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout_s)
    except subprocess.TimeoutExpired:
        process.kill()
        try:
            stdout, stderr = process.communicate(timeout=DRAIN_S)
        except subprocess.TimeoutExpired:
            stdout, stderr = "", ""
        return 124, stdout[-MAX_PROBE_BYTES:], "TIMEOUT"
    return process.returncode, stdout[-MAX_PROBE_BYTES:], stderr[-MAX_PROBE_BYTES:]


def run_probe(plan_path: Path, out_path: Path, manifest: dict, excluded: set[int],
              probe: str, timeout_s: float) -> tuple[int, str, str]:
    command = [
        sys.executable, "-B", str(Path(probe).resolve()),
        "--plan", str(plan_path),
        "--out", str(out_path),
        "--title", manifest["window"]["title"],
        "--generation", str(manifest["window"]["generation"]),
        "--excluded-pids", ",".join(str(pid) for pid in sorted(excluded)),
        "--max-lag-ms", str(r0c.MAX_LAG_MS),
        "--close-timeout-ms", str(r0c.CLOSE_TIMEOUT_MS),
    ]
    return _run_bounded(command, timeout_s=timeout_s)


def _load_json(path: Path, limit: int) -> object:
    return contracts.load_document(str(path), limit)


def _journal(capture_path: Path, observation: dict, executable: str) -> dict:
    raw = capture_path.read_bytes()
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
        "counts": kinds,
        "summary": {key: summary[key] for key in (
            "journal_seen", "journal_lost", "eligible", "retained", "retention_dropped",
            "excluded", "input_unassociated", "unmatched_submission", "undrained_journal",
        )},
        "scope_ns": capture["scope_end_ns"] - capture["scope_start_ns"],
    }


def _achieved(actions: list[dict]) -> tuple[dict[str, float], dict[str, int]]:
    per_kind: dict[str, list[int]] = {}
    delivered: dict[str, int] = {}
    for action in actions:
        if action["actual_ns"] is not None:
            per_kind.setdefault(action["kind"], []).append(action["actual_ns"])
        if action["classification"] in ("on-time", "early", "late"):
            delivered[action["kind"]] = delivered.get(action["kind"], 0) + 1
    achieved: dict[str, float] = {}
    for kind, times in per_kind.items():
        span = (max(times) - min(times)) / 1_000_000_000 if len(times) > 1 else 0.0
        count = delivered.get(kind, 0)
        achieved[kind] = round(count / span, 3) if span > 0 else 0.0
    return achieved, delivered


def verify_probe(probe: dict, manifest: dict, trace: dict, pre_run_pids: set[int]) -> None:
    """Reject a probe result that is not from this generation or this trace."""
    if probe.get("generation") != manifest["window"]["generation"]:
        raise DriverError("WINDOW_GENERATION_MISMATCH")
    window = probe.get("window") or {}
    if window.get("title") != manifest["window"]["title"]:
        raise DriverError("WINDOW_IDENTITY_MISMATCH")
    pid = window.get("pid")
    if type(pid) is not int or pid <= 0 or pid in pre_run_pids:
        raise DriverError("WINDOW_GENERATION_MISMATCH")
    planned = {action["sequence"]: action for action in trace["actions"]}
    seen: set[int] = set()
    for action in probe.get("actions", []):
        sequence = action.get("sequence")
        if sequence not in planned or sequence in seen:
            raise DriverError("PROBE_TRACE_MISMATCH")
        if action.get("kind") != planned[sequence]["kind"]:
            raise DriverError("PROBE_TRACE_MISMATCH")
        seen.add(sequence)
    if seen != set(planned):
        raise DriverError("PROBE_TRACE_INCOMPLETE")


def assemble_result(manifest: dict, trace: dict, probe: dict, journal: dict | None,
                    wrapper_exit: int, cleanup: dict, status: str, reason: str) -> dict:
    actions = probe["actions"]
    requested: dict[str, float] = {}
    for action in manifest["actions"]:
        kind = action["kind"]
        requested[kind] = max(requested.get(kind, 0.0), float(action["requested_hz"]))
    achieved, delivered = _achieved(actions)
    for kind in requested:
        achieved.setdefault(kind, 0.0)
    endpoints = r0c.endpoint_matrix()
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
        "journal": {
            "source": "s2b-journal" if journal is not None else "none",
            "counts": (journal or {}).get("counts", {}),
            "note": "journal event counts are not presented-frame counts",
            "capture_sha256": (journal or {}).get("capture_sha256", "sha256:" + "0" * 64),
        },
        "endpoints": endpoints,
        "cleanup": cleanup,
        "limits": {
            "max_actions": r0c.MAX_ACTIONS,
            "max_lag_ms": r0c.MAX_LAG_MS,
            "probe_timeout_ms": int(PROBE_TIMEOUT_S * 1000),
            "capture_timeout_ms": int(manifest["lifecycle"]["timeout_ms"]),
            "output_bytes": MAX_PROBE_BYTES,
        },
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
    manifest = r0c.load_manifest(args.manifest)
    s2b_manifest = s2b.validate_manifest(
        contracts.load_document(args.s2b_manifest, contracts.MAX_MANIFEST_BYTES)
    )
    _require_identity_agreement(manifest, s2b_manifest)
    binding = _approved_binding(manifest)
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
        text=True, encoding="utf-8", errors="replace",
    )
    probe_exit, probe_stdout, probe_stderr = run_probe(
        plan_path, probe_path, manifest, pre_run_pids, probe_command, probe_timeout,
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

    probe = json.loads(probe_path.read_text(encoding="utf-8")) if probe_path.exists() else None
    if probe is None:
        probe = {
            "generation": manifest["window"]["generation"],
            "window": {"pid": 0, "title": manifest["window"]["title"]},
            "actions": [
                {
                    "sequence": action["sequence"],
                    "kind": action["kind"],
                    "planned_ns": action["planned_ns"],
                    "actual_ns": None,
                    "classification": "failed",
                }
                for action in trace["actions"]
            ],
            "close": {"posted": False},
        }
    probe_error = ""
    try:
        verify_probe(probe, manifest, trace, pre_run_pids)
    except DriverError as error:
        probe_error = error.code
    close_posted = bool(probe.get("close", {}).get("posted"))
    journal = None
    journal_error = ""
    observation_path = output / "s2b-observation.json"
    capture_path = output / "s2b-capture.json"
    if observation_path.exists() and capture_path.exists():
        observation = _load_json(observation_path, s2b.MAX_BYTES)
        try:
            journal = _journal(capture_path, observation, binding["executable_sha256"])
        except (s2b.ObservationError, contracts.ContractError, OSError):
            journal_error = "JOURNAL_INVALID"

    cleanup = {
        "wrapper_exit": wrapper_exit,
        "child_reaped": capture_process.poll() is not None,
        "close_posted": close_posted,
        "timed_out": timed_out,
        "cancelled": cancelled,
    }
    status = "COMPLETE"
    if probe is None or probe_exit != 0:
        status = "FAILED"
    if timed_out or wrapper_exit != 0 or journal is None or not close_posted:
        status = "FAILED"
    reason = _result_reason(status, wrapper_exit, timed_out, close_posted)
    if probe is None:
        reason = "PROBE_FAILED"
    elif probe_exit != 0:
        reason = "PROBE_" + (probe_stderr.strip().splitlines()[-1] if probe_stderr.strip() else "FAILED")
    if journal_error:
        status, reason = "FAILED", journal_error
    if probe_error:
        status, reason = "FAILED", probe_error
    result = assemble_result(manifest, trace, probe, journal, wrapper_exit, cleanup, status, reason)
    r0c.validate_result(result)
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


def verify(args: argparse.Namespace) -> int:
    manifest = r0c.load_manifest(args.manifest)
    result = r0c.validate_result(contracts.load_document(args.result, r0c.MAX_RESULT_BYTES))
    r0c.check_identity(manifest, result)
    trace = r0c.plan_trace(manifest)
    if result["trace_sha256"] != trace["trace_sha256"]:
        raise r0c.ContractError("TRACE_MISMATCH", "$.trace_sha256")
    requested: dict[str, float] = {}
    for action in manifest["actions"]:
        requested[action["kind"]] = max(requested.get(action["kind"], 0.0), float(action["requested_hz"]))
    if result["requested_cadence"] != requested:
        raise r0c.ContractError("CADENCE_MISMATCH", "$.requested_cadence")
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
