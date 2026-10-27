"""Bounded real-WIC W5 metadata/orientation/ICC matrix."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import time

from w2_resource_matrix import Worker, _work_bytes
from w2_resource_verify import canonical, file_sha, sha
from w5_metadata_fixtures import generate
from w5_metadata_oracle import compare
from w5_metadata_verify import BOUNDS, CASE_IDS, CASES, LIMITS, SOURCE_FILES, verify

ROOT = Path(__file__).resolve().parents[4]
MAX_METADATA = BOUNDS["max_metadata_bytes"]


def guard(work, started, worker=None):
    if time.monotonic_ns() - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    if _work_bytes(work) > BOUNDS["max_work_bytes"]:
        raise ValueError("work_byte_cap")
    if worker:
        for path in (worker.stdout_path, worker.stderr_path):
            if path.stat().st_size > BOUNDS["max_capture_bytes"]:
                raise ValueError("capture_cap")
        worker.read_trace()
        if worker.output.exists() and worker.output.stat().st_size > BOUNDS["max_output_bytes"]:
            raise ValueError("output_cap")
        if time.monotonic_ns() - worker.started > BOUNDS["max_worker_seconds"] * 1_000_000_000:
            raise ValueError("worker_deadline_exceeded")


def source_identity(pwsh):
    files = {}
    for name in SOURCE_FILES:
        data = (ROOT / name).read_bytes()
        files[name] = {"bytes": len(data), "raw_sha256": sha(data),
                       "lf_sha256": sha(data.replace(b"\r\n", b"\n").replace(b"\r", b"\n"))}
    return {"files": files, "pwsh_sha256": file_sha(pwsh)}


def run_worker(case, work, pwsh, inventory, started):
    cid, fid, _kind, _value, apply_orientation = case
    stop = cid in ("cancel_metadata", "timeout_metadata")
    hold = "before_publish" if stop else ""
    adapted = {k: dict(v, case_id=k, sha256="sha256:" + v["sha256"]) for k, v in inventory.items()}
    worker = Worker((cid, fid, "metadata", hold), work, pwsh, adapted, LIMITS,
                    metadata=True, apply_orientation=apply_orientation, max_metadata=MAX_METADATA)
    try:
        while worker.proc.poll() is None and not worker.at(hold):
            guard(work, started, worker)
            time.sleep(.002)
        if worker.proc.poll() is None:
            if stop:
                if cid == "timeout_metadata":
                    while time.monotonic_ns() - worker.started < 5_000_000_000:
                        guard(work, started, worker)
                        if worker.proc.poll() is not None:
                            raise ValueError("worker_exited_before_operator_timeout")
                        time.sleep(.002)
                worker.stop("timeout" if cid == "timeout_metadata" else "cancel")
            else:
                worker.release()
        while worker.proc.poll() is None:
            guard(work, started, worker)
            time.sleep(.002)
        row = worker.finish(file_sha(pwsh))
        row["metadata_requested"] = True
        row["apply_orientation"] = apply_orientation
        row["trace_utf8"] = (worker.observe / "trace.jsonl").read_text(encoding="utf-8")
        row["stdout_utf8"] = worker.stdout_path.read_text(encoding="utf-8")
        row["output_hex"] = worker.output.read_bytes().hex() if worker.output.exists() else None
        fixture = inventory[fid]
        if not stop and row["outcome"] == "decoded":
            row["oracle"] = compare(fixture, (work / "cases" / fixture["path"]).read_bytes(),
                                    worker.output.read_bytes(), apply_orientation)
        else:
            row["oracle"] = None
        return row
    finally:
        if worker.proc.poll() is None:
            worker.stop("cleanup")
        worker.stdout.close()
        worker.stderr.close()


def run(work, out, pwsh):
    work, out, pwsh = map(lambda p: Path(p).resolve(), (work, out, pwsh))
    if work.exists() or out.exists() or out.is_relative_to(work):
        raise ValueError("fresh_external_work_and_report_required")
    if not pwsh.is_file():
        raise ValueError("real_powershell_required")
    started = time.monotonic_ns()
    identity = source_identity(pwsh)
    work.mkdir(parents=True, exist_ok=False)
    fixtures = generate(work / "cases")
    inventory = {f["id"]: f for f in fixtures}
    for f in fixtures:
        f["input_hex"] = (work / "cases" / f["path"]).read_bytes().hex()
    workers = [run_worker(case, work, pwsh, inventory, started) for case in CASES]
    guard(work, started)
    if identity != source_identity(pwsh):
        raise ValueError("source_or_executable_changed")
    record = {
        "schema_version": 1, "complete": True, "run_id": work.name, "bounds": BOUNDS,
        "source_identity": identity, "fixtures": fixtures,
        "limits_basis": "existing decode_compare DecodeBudget narrow overrides; W5 adds no budget",
        "workers": workers, "duration_ns": time.monotonic_ns() - started,
        "work_bytes": _work_bytes(work), "cleanup": {"complete": True, "live_direct_children": []},
        "work_root_disposable": True,
        "unmeasured": ["WIC internal colour pipeline and scratch", "allocator reservations",
                       "true concurrent allocation peak", "cooperative in-decoder cancellation",
                       "16K full decode", "true region/sequential/tiled decode",
                       "ICC colour transform fidelity", "JPEG absolute raster",
                       "product orientation/colour policy"],
    }
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap")
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("xb") as stream:
        stream.write(data)
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="op", required=True)
    ex = sub.add_parser("run")
    for flag in ("work-root", "out", "pwsh"):
        ex.add_argument("--" + flag, required=True, type=Path)
    chk = sub.add_parser("verify")
    chk.add_argument("--report", required=True, type=Path)
    chk.add_argument("--source-root", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.op == "run":
            result = verify(run(args.work_root, args.out, args.pwsh), ROOT)
        else:
            result = verify(json.loads(args.report.read_bytes()), args.source_root)
        print(json.dumps(result, sort_keys=True))
        return 0 if result["ok"] else 1
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as exc:
        print(json.dumps({"ok": False, "error": type(exc).__name__, "reason": str(exc)[:160]}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
