"""Bounded W7 large-dimension WIC decode and admission matrix.

Admission is the safety boundary: oversized declarations are refused before
OnLoad. Materialized 2K/4K outputs are compared with an independent streaming
oracle. Process counters are observations, never decoder-internal measurements.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
from stat import S_ISREG
import time

from w2_resource_matrix import Worker, _safe_path
from w2_resource_verify import canonical, file_sha, sha
from w6_memory_sample import SAMPLE_SCRIPT, capture_window
from w7_large_fixtures import CASES, MAX_ENCODED_BYTES, expected_sha256, write_fixture
from w7_large_oracle import compare, png_header, source_geometry
from w7_large_verify import (ALLOCATION_SCOPE, BOUNDS, LIMITS, SOURCE_FILES, UNMEASURED,
                             verify)

ROOT = Path(__file__).resolve().parents[4]
OUTPUT_BYTES_RETAINED = 4 * 1024 * 1024


def _git(*args):
    executable = os.environ.get("GIT_EXECUTABLE") or shutil.which("git")
    if not executable:
        raise ValueError("git_executable_unavailable")
    return subprocess.run([executable, *args], cwd=ROOT, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, check=True, timeout=15).stdout


def source_identity(pwsh, harness):
    commit = _git("rev-parse", "HEAD").decode("ascii").strip()
    files = {}
    for path in SOURCE_FILES:
        data = (ROOT / path).read_bytes()
        normalized = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        blob = _git("show", f"{commit}:{path}")
        if normalized != blob.replace(b"\r\n", b"\n").replace(b"\r", b"\n"):
            raise ValueError("experimental_source_not_pinned:" + path)
        files[path] = {"bytes": len(data), "raw_sha256": sha(data),
                       "lf_sha256": sha(normalized), "git_blob_sha256": sha(blob),
                       "unchanged_after_run": True}
    return {"source_commit": commit, "files": files, "pwsh_sha256": file_sha(pwsh),
            "harness_sha256": file_sha(harness),
            "normalization": "Raw Native hashes plus explicitly declared LF-normalized Git correspondence"}


def _work_bytes(work):
    total = 0
    for path in work.rglob("*"):
        info = path.stat()
        if S_ISREG(info.st_mode):
            total += info.st_size
    return total


def guard(work, started, workers=()):
    now = time.monotonic_ns()
    if now - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("global_deadline_exceeded")
    if _work_bytes(work) > BOUNDS["max_work_bytes"]:
        raise ValueError("work_cap_exceeded")
    if len(workers) > BOUNDS["max_workers"]:
        raise ValueError("worker_cap_exceeded")
    for worker in workers:
        if now - worker.started > BOUNDS["max_worker_seconds"] * 1_000_000_000:
            raise ValueError("worker_deadline_exceeded")
        worker.read_trace()
        for path in (worker.stdout_path, worker.stderr_path):
            if path.stat().st_size > BOUNDS["max_capture_bytes"]:
                raise ValueError("worker_capture_cap_exceeded")
        if worker.output.exists() and worker.output.stat().st_size > BOUNDS["max_output_bytes"]:
            raise ValueError("output_cap_exceeded")


def _hold_stage(case):
    return "after_onload" if case["role"] == "decode" else "before_admission"


def run_case(case, work, pwsh, fixtures, started, executable_sha):
    spec = (case["id"], case["id"], case["role"], _hold_stage(case))
    roi = case.get("roi")
    worker = Worker(spec, work, pwsh, fixtures, LIMITS, roi=roi,
                    max_roi_output=BOUNDS["max_roi_output_bytes"])
    row = {"case_id": case["id"], "role": case["role"], "hold_stage": spec[3], "windows": [],
           "failure": None, "classification": "NOT_RUN", "sample": None}
    try:
        stage = _hold_stage(case)
        while not worker.at(stage):
            guard(work, started, [worker])
            if worker.proc.poll() is not None:
                raise ValueError("worker_exited_before_required_boundary")
            time.sleep(.005)
        window = capture_window([worker], pwsh, 0)
        row["windows"].append(window)
        row["sample"] = window
        if window["complete"]:
            worker.identity_verified = True
            if window["totals"]["private_bytes"] > BOUNDS["max_observed_private_bytes"]:
                raise ValueError("observed_private_bound_exceeded")
        guard(work, started, [worker])
        worker.release()
        while worker.proc.poll() is None:
            guard(work, started, [worker])
            time.sleep(.005)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        row["failure"] = type(exc).__name__ + ":" + str(exc)[:160]
    finally:
        try:
            if worker.proc.poll() is None:
                worker.stop("cleanup")
            record = worker.finish(executable_sha)
            record["limits"] = dict(LIMITS)
            record["trace_utf8"] = (worker.observe / "trace.jsonl").read_bytes().decode("utf-8")
            record["stdout_utf8"] = worker.stdout_path.read_bytes().decode("utf-8")
            output = worker.output if worker.output.is_file() else None
            record["output_hex"] = (output.read_bytes().hex()
                                    if output is not None and output.stat().st_size <= OUTPUT_BYTES_RETAINED
                                    else None)
            record["oracle"] = None
            record["oracle_error"] = None
            if record["outcome"] == "decoded" and output is not None:
                try:
                    record["oracle"] = compare(case, work / "cases" / (case["id"] + ".png"), output, "png",
                                               BOUNDS["max_report_bytes"])
                except (OSError, ValueError, KeyError, TypeError) as exc:
                    record["oracle_error"] = type(exc).__name__ + ":" + str(exc)[:120]
            row["worker"] = record
            row["classification"] = classify(case, record)
        finally:
            if worker.proc.poll() is None:
                worker.proc.kill()
                worker.proc.wait(timeout=5)
            worker.stdout.close()
            worker.stderr.close()
    return row


def classify(case, record):
    outcome = record["outcome"]
    if outcome == "decoded":
        if record["oracle"] is not None and record["oracle"]["ok"]:
            return "DECODED_AND_VERIFIED"
        return "TAMPER_NOT_VERIFIED"
    if outcome in ("cancelled", "timeout", "not_verified"):
        return "NOT_VERIFIED"
    if record["probe_result"] and record["probe_result"].get("admission_status") == "refused":
        return "REFUSED_FULL_FRAME"
    return "NOT_VERIFIED"


def run(work, out, pwsh, harness):
    work, out, pwsh, harness = map(_safe_path, (work, out, pwsh, harness))
    if work.exists() or out.exists() or out.is_relative_to(work):
        raise ValueError("fresh_owned_work_and_external_report_required")
    if not pwsh.is_file() or not harness.is_file():
        raise ValueError("real_executables_required")
    identity = source_identity(pwsh, harness)
    started = time.monotonic_ns()
    work.mkdir(parents=True, exist_ok=False)
    cases_dir = work / "cases"
    cases_dir.mkdir()
    fixtures = {}
    for case in CASES:
        record = write_fixture(case["id"], cases_dir / (case["id"] + ".png"))
        fixtures[case["id"]] = {"case_id": case["id"], "path": case["id"] + ".png",
                                "sha256": "sha256:" + record["sha256"], "bytes": record["bytes"],
                                "codec": case["codec"], "width": case["width"], "height": case["height"]}
        if record["sha256"] != expected_sha256(case["id"])[7:]:
            raise ValueError("fixture_regeneration_mismatch:" + case["id"])
        if record["bytes"] > MAX_ENCODED_BYTES:
            raise ValueError("encoded_input_over_bound")
    rows = []
    for case in CASES:
        rows.append(run_case(case, work, pwsh, fixtures, started, identity["pwsh_sha256"]))
        if rows[-1]["failure"]:
            break
    after = source_identity(pwsh, harness)
    unchanged = identity == after
    for path, item in identity["files"].items():
        item["unchanged_after_run"] = unchanged and item == after["files"][path]
    platform_script = ('[ordered]@{powershell=$PSVersionTable.PSVersion.ToString();'
                       'dotnet=[Runtime.InteropServices.RuntimeInformation]::FrameworkDescription;'
                       'os=[Environment]::OSVersion.VersionString;'
                       'windows=[OperatingSystem]::IsWindows()}|ConvertTo-Json -Compress')
    platform = subprocess.run([str(pwsh), "-NoProfile", "-Command", platform_script],
                              capture_output=True, text=True, timeout=5, check=True)
    if len(platform.stdout.encode("utf-8")) > BOUNDS["max_capture_bytes"]:
        raise ValueError("platform_capture_cap")
    geometries = {}
    for case in CASES:
        path = cases_dir / (case["id"] + ".png")
        try:
            if case["codec"] == "png":
                width, height, _ = png_header(path, BOUNDS["max_encoded_input_bytes"])
            else:
                parsed = source_geometry(case["codec"], path, BOUNDS["max_encoded_input_bytes"])
                width, height = parsed["width"], parsed["height"]
            geometries[case["id"]] = {"width": width, "height": height}
        except (OSError, ValueError) as exc:
            geometries[case["id"]] = {"error": type(exc).__name__ + ":" + str(exc)[:80]}
    record = {"schema_version": 1,
              "complete": len(rows) == len(CASES) and all(r["failure"] is None for r in rows) and unchanged,
              "run_id": work.name, "bounds": BOUNDS, "decode_limits": dict(LIMITS),
              "source_identity": identity, "platform": json.loads(platform.stdout),
              "fixtures": list(fixtures.values()), "geometry": geometries,
              "sampler_script_sha256": sha(SAMPLE_SCRIPT.encode("utf-8")),
              "cases": rows, "allocation_scope": dict(ALLOCATION_SCOPE),
              "unmeasured": list(UNMEASURED),
              "duration_ns": time.monotonic_ns() - started, "work_bytes": _work_bytes(work),
              "cleanup": {"complete": True, "live_direct_children": []}, "work_root_disposable": True,
              "limitations": [
                  "Process counters are whole-process observations, not WIC-internal allocations.",
                  "Sequential counter reads are not an atomic snapshot.",
                  "Boundary holds are artificial instrumentation, not decoder latency.",
                  "A refused large source states only that admission refused it; no 16K decode capability is shown.",
                  "Decoded outputs are re-encoded PNGs, not the decoder's internal residency."]}
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("xb") as stream:
        stream.write(data)
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    execute = sub.add_parser("run")
    for name in ("work-root", "out", "pwsh", "harness"):
        execute.add_argument("--" + name, required=True, type=Path)
    check = sub.add_parser("verify")
    check.add_argument("--report", required=True, type=Path)
    check.add_argument("--source-root", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.operation == "run":
            result = verify(run(args.work_root, args.out, args.pwsh, args.harness), ROOT)
        else:
            if args.report.stat().st_size > BOUNDS["max_report_bytes"]:
                raise ValueError("report_cap_exceeded")
            result = verify(json.loads(args.report.read_bytes()), args.source_root)
        print(json.dumps(result, sort_keys=True))
        return 0 if result["ok"] and result["closure_eligible"] else 1
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as exc:
        print(json.dumps({"ok": False, "error": type(exc).__name__, "reason": str(exc)[:200]}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
