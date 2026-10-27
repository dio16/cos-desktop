"""Bounded WIC process-memory scaling; no product scheduler or allocator claims."""
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
from w6_memory_fixtures import generate, compare
from w6_memory_sample import SAMPLE_SCRIPT, capture_window
from w6_memory_verify import (ALLOCATION_SCOPE, BOUNDS, LIMITS, SOURCE_FILES, groups,
                              comparison_rows, matched_differences, requirements,
                              verify, window_metrics)

ROOT = Path(__file__).resolve().parents[4]


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
        try:
            info = path.stat()
        except FileNotFoundError:
            # The probe atomically renames this exact pending file on publication.
            # Charge its published bytes rather than dropping it from the bound.
            if path.name != "pending.png" or path.parent.name != "observe":
                raise
            info = (path.parent.parent / "output.png").stat()
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


def run_group(spec, work, pwsh, inventory, started, executable_sha):
    group = {"id": spec["id"], "boundary": spec["boundary"], "action": spec["action"],
             "start_ns": time.monotonic_ns(), "workers": [], "windows": [], "failure": None}
    workers = []
    try:
        for fid in spec["fixtures"]:
            kind = spec["action"] if spec["action"] != "release" else (
                "cohort" if len(spec["fixtures"]) > 1 else "success")
            workers.append(Worker((spec["id"] + "_" + fid, fid, kind, spec["boundary"]),
                                  work, pwsh, inventory, LIMITS))
        while not all(w.at(spec["boundary"]) for w in workers):
            guard(work, started, workers)
            if any(w.proc.poll() is not None for w in workers):
                raise ValueError("worker_exited_before_boundary")
            time.sleep(.005)
        previous_end = None
        for index in range(spec["windows"]):
            guard(work, started, workers)
            window = capture_window(workers, pwsh, index, previous_end)
            group["windows"].append(window)
            previous_end = window["end_ns"]
            if window["complete"]:
                for worker in workers:
                    worker.identity_verified = True
                if window["totals"]["private_bytes"] > BOUNDS["max_observed_private_bytes"]:
                    raise ValueError("observed_private_bound_exceeded")
            guard(work, started, workers)
        if spec["action"] == "timeout":
            while time.monotonic_ns() - min(w.started for w in workers) < 5_000_000_000:
                guard(work, started, workers)
                if any(w.proc.poll() is not None for w in workers):
                    raise ValueError("worker_exited_before_operator_timeout")
                time.sleep(.005)
        for worker in workers:
            if spec["action"] == "release":
                worker.release()
            else:
                worker.stop(spec["action"])
        while any(w.proc.poll() is None for w in workers):
            guard(work, started, workers)
            time.sleep(.005)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        group["failure"] = type(exc).__name__ + ":" + str(exc)[:160]
    finally:
        for worker in workers:
            try:
                if worker.proc.poll() is None:
                    worker.stop("cleanup")
                row = worker.finish(executable_sha)
                row["limits"] = dict(LIMITS)
                row["trace_utf8"] = (worker.observe / "trace.jsonl").read_bytes().decode("utf-8")
                row["stdout_utf8"] = worker.stdout_path.read_bytes().decode("utf-8")
                row["output_hex"] = worker.output.read_bytes().hex() if worker.output.is_file() else None
                row["oracle"] = (compare(inventory[worker.fid],
                                          (work / "cases" / inventory[worker.fid]["path"]).read_bytes(),
                                          worker.output.read_bytes())
                                 if row["outcome"] == "decoded" and worker.output.is_file() else None)
                group["workers"].append(row)
            finally:
                if worker.proc.poll() is None:
                    worker.proc.kill()
                    worker.proc.wait(timeout=5)
                worker.stdout.close()
                worker.stderr.close()
    group["end_ns"] = time.monotonic_ns()
    group["metrics"] = window_metrics(group["windows"])
    group["requirements"] = requirements([inventory[f] for f in spec["fixtures"]])
    group["target_shortfall_reason"] = (
        "Single-worker two-window series preserves the global32-window cap; three windows remain the multiworker target."
        if len(spec["fixtures"]) == 1 else None)
    return group


def run(work, out, pwsh, harness):
    work, out, pwsh, harness = map(_safe_path, (work, out, pwsh, harness))
    if work.exists() or out.exists() or out.is_relative_to(work):
        raise ValueError("fresh_owned_work_and_external_report_required")
    if not pwsh.is_file() or not harness.is_file():
        raise ValueError("real_executables_required")
    identity = source_identity(pwsh, harness)
    started = time.monotonic_ns()
    work.mkdir(parents=True, exist_ok=False)
    fixtures = generate(work / "cases")
    inventory = {f["id"]: dict(f, case_id=f["id"], sha256="sha256:" + f["sha256"]) for f in fixtures}
    # Worker accepts prefixed digests; the fixture/oracle contract uses bare hex.
    oracle_inventory = {f["id"]: f for f in fixtures}
    experiments = []
    for spec in groups():
        # Keep the original bare digest available to the independent oracle.
        group = run_group(spec, work, pwsh, inventory, started, identity["pwsh_sha256"])
        experiments.append(group)
        guard(work, started)
        if group["failure"]:
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
                              capture_output=True, text=True, timeout=3, check=True)
    if len(platform.stdout.encode("utf-8")) > BOUNDS["max_capture_bytes"]:
        raise ValueError("platform_capture_cap")
    record = {"schema_version": 1, "complete": len(experiments) == len(groups())
              and all(g["failure"] is None for g in experiments) and unchanged,
              "bounds": BOUNDS, "decode_limits": dict(LIMITS), "run_id": work.name,
              "source_identity": identity, "platform": json.loads(platform.stdout),
              "fixtures": list(oracle_inventory.values()),
              "sampler_script_sha256": sha(SAMPLE_SCRIPT.encode("utf-8")),
              "experiments": experiments, "allocation_scope": dict(ALLOCATION_SCOPE),
              "duration_ns": time.monotonic_ns() - started, "work_bytes": _work_bytes(work),
              "cleanup": {"complete": True, "live_direct_children": []}, "work_root_disposable": True,
              "measurement_complete": all(g["metrics"]["incomplete_window_count"] == 0 for g in experiments),
              "comparison": comparison_rows(experiments, fixtures),
              "matched_run_differences": matched_differences(experiments) if len(experiments) == len(groups()) else [],
              "limitations": ["Sequential process-counter reads are not an atomic OS snapshot.",
                              "PowerShell/.NET/runtime/library costs are included; not WIC-only allocations.",
                              "Sample gaps can hide memory changes; observed maxima are not true peaks.",
                              "Before/after groups use different workers, so differences are matched-run observations.",
                              "Boundary holds are artificial instrumentation, not decoder latency.",
                              "No product concurrency policy or numeric memory SLO is selected."]}
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
