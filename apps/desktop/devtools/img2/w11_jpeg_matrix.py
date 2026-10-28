"""Serial, bounded real-WIC observations for the W11 JPEG admission experiment."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import time

from w2_resource_matrix import Worker, file_sha, sha
from w6_memory_sample import SAMPLE_SCRIPT, capture_window
from w11_jpeg_fixtures import FIXTURES, write_fixture
from w11_jpeg_oracle import compare_output, source_geometry, verify_fixture
from w11_jpeg_verify import (
    ALLOCATION_SCOPE,
    BOUNDS,
    LIMITS,
    SCENARIOS,
    SOURCE_FILES,
    UNMEASURED,
    verify,
)

ROOT = Path(__file__).resolve().parents[4]
_FIXTURE_METHOD = (
    "streamed DC-only grayscale baseline JPEG; 64 KiB entropy chunks; Q=8; "
    "block level=48+20*((3*bx+5*by+(bx^by)) mod 9); no full-frame raster"
)
_NORMALIZATION = "Raw Native hashes plus explicitly declared LF-normalized Git correspondence"


def _is_reparse(path: Path) -> bool:
    return path.is_symlink() or (hasattr(path, "is_junction") and path.is_junction())


def _safe_path(path) -> Path:
    candidate = Path(path)
    if (not candidate.is_absolute() or len(candidate.drive) != 2 or candidate.drive[1] != ":"
            or ".." in candidate.parts):
        raise ValueError("absolute_local_disk_path_required")
    if any(_is_reparse(part) for part in (candidate, *candidate.parents)):
        raise ValueError("reparse_path_refused")
    resolved = candidate.resolve(strict=False)
    if any(_is_reparse(part) for part in (resolved, *resolved.parents)):
        raise ValueError("reparse_path_refused")
    return resolved


def _git(*args) -> bytes:
    executable = os.environ.get("GIT_EXECUTABLE") or shutil.which("git")
    if not executable:
        raise ValueError("git_executable_unavailable")
    completed = subprocess.run(
        [executable, *args], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        check=True, timeout=15,
    )
    if len(completed.stdout) > BOUNDS["max_report_bytes"] or len(completed.stderr) > BOUNDS["max_capture_bytes"]:
        raise ValueError("source_identity_capture_cap")
    return completed.stdout


def _source_identity(pwsh: Path) -> dict:
    commit = _git("rev-parse", "HEAD").decode("ascii").strip()
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("source_commit_invalid")
    files = {}
    for relative in SOURCE_FILES:
        rel = Path(relative)
        if rel.is_absolute() or ".." in rel.parts:
            raise ValueError("source_path_invalid")
        path = _safe_path(ROOT / rel)
        data = path.read_bytes()
        normalized = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        blob = _git("show", f"{commit}:{relative}")
        if normalized != blob.replace(b"\r\n", b"\n").replace(b"\r", b"\n"):
            raise ValueError("experimental_source_not_pinned")
        files[relative] = {
            "bytes": len(data),
            "raw_sha256": sha(data),
            "lf_sha256": sha(normalized),
            "git_blob_sha256": sha(blob),
            "unchanged_after_run": False,
        }
    return {
        "source_commit": commit,
        "files": files,
        "pwsh_sha256": file_sha(pwsh),
        "normalization": _NORMALIZATION,
    }


def _same_source(before: dict, after: dict) -> bool:
    if (before["source_commit"] != after["source_commit"]
            or before["pwsh_sha256"] != after["pwsh_sha256"]
            or before["normalization"] != after["normalization"]
            or before["files"].keys() != after["files"].keys()):
        return False
    return all(
        {k: v for k, v in before["files"][name].items() if k != "unchanged_after_run"}
        == {k: v for k, v in after["files"][name].items() if k != "unchanged_after_run"}
        for name in before["files"]
    )


def _work_bytes(work: Path) -> int:
    total = 0
    entries = 0
    for path in work.rglob("*"):
        entries += 1
        if entries > 256 or _is_reparse(path):
            raise ValueError("work_entry_bound_or_reparse")
        info = path.stat()
        if stat.S_ISREG(info.st_mode):
            total += info.st_size
        elif not stat.S_ISDIR(info.st_mode):
            raise ValueError("work_nonregular_entry")
    return total


def _guard(work: Path, started: int, workers=()) -> int:
    now = time.monotonic_ns()
    if now - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    if len(workers) > BOUNDS["max_workers"]:
        raise ValueError("worker_cap_exceeded")
    work_bytes = _work_bytes(work)
    if work_bytes > BOUNDS["max_work_bytes"]:
        raise ValueError("work_cap_exceeded")
    for worker in workers:
        if now - worker.started > BOUNDS["max_worker_seconds"] * 1_000_000_000:
            raise ValueError("worker_deadline_exceeded")
        trace = worker.observe / "trace.jsonl"
        if trace.exists() and (_is_reparse(trace) or not trace.is_file()
                               or trace.stat().st_size > BOUNDS["max_trace_bytes"]):
            raise ValueError("trace_cap_or_path_invalid")
        worker.read_trace()
        if trace.exists() and len(worker.rows) > BOUNDS["max_trace_records"]:
            raise ValueError("trace_record_cap_exceeded")
        for path in (worker.stdout_path, worker.stderr_path):
            if _is_reparse(path) or not path.is_file() or path.stat().st_size > BOUNDS["max_capture_bytes"]:
                raise ValueError("capture_cap_or_path_invalid")
        for path in (worker.output, worker.observe / "pending.png", worker.observe / "pending.bmp"):
            if path.exists():
                if _is_reparse(path) or not path.is_file():
                    raise ValueError("output_path_invalid")
                if path.stat().st_size > BOUNDS["max_output_bytes"]:
                    raise ValueError("output_cap_exceeded")
    return work_bytes


def _platform(pwsh: Path) -> dict:
    script = (
        "[ordered]@{powershell=$PSVersionTable.PSVersion.ToString();"
        "dotnet=[Runtime.InteropServices.RuntimeInformation]::FrameworkDescription;"
        "os=[Environment]::OSVersion.VersionString;"
        "windows=[OperatingSystem]::IsWindows()}|ConvertTo-Json -Compress"
    )
    result = subprocess.run(
        [str(pwsh), "-NoProfile", "-Command", script], stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, timeout=5, check=True,
    )
    if (len(result.stdout) > BOUNDS["max_capture_bytes"]
            or len(result.stderr) > BOUNDS["max_capture_bytes"]):
        raise ValueError("platform_capture_cap")
    try:
        platform = json.loads(result.stdout.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError("platform_record_invalid") from exc
    if (not isinstance(platform, dict) or platform.get("windows") is not True
            or any(not isinstance(platform.get(key), str) or not platform[key]
                   for key in ("powershell", "dotnet", "os"))):
        raise ValueError("windows_platform_required")
    return platform


def _fixture_records(work: Path, started: int):
    cases_dir = work / "cases"
    cases_dir.mkdir(exist_ok=False)
    fixture_inventory = {}
    fixture_data = {}
    records = []
    for spec in FIXTURES:
        fixture_id = spec["id"]
        filename = fixture_id + ".jpg"
        path = cases_dir / filename
        generated = write_fixture(fixture_id, path)
        if (generated.get("case_id") != fixture_id or generated.get("bytes") != path.stat().st_size
                or _is_reparse(path) or not path.is_file()):
            raise ValueError("fixture_write_identity_invalid")
        if generated["bytes"] <= 0 or generated["bytes"] > BOUNDS["max_encoded_input_bytes"]:
            raise ValueError("encoded_input_over_bound")
        data = path.read_bytes()
        if len(data) != generated["bytes"] or sha(data) != "sha256:" + generated["sha256"]:
            raise ValueError("fixture_digest_mismatch")
        geometry = source_geometry(data)
        independent = verify_fixture(fixture_id, data)
        if (independent.get("fixture_id") != fixture_id or independent.get("bytes") != len(data)
                or independent.get("sha256") != sha(data) or independent.get("geometry") != geometry
                or (geometry.get("width"), geometry.get("height")) != (spec["width"], spec["height"])):
            raise ValueError("fixture_oracle_mismatch")
        record = {
            "case_id": fixture_id,
            "path": filename,
            "codec": "jpeg",
            "width": spec["width"],
            "height": spec["height"],
            "bytes": len(data),
            "sha256": sha(data),
            "method": _FIXTURE_METHOD,
            "geometry": geometry,
        }
        records.append(record)
        fixture_inventory[fixture_id] = {"path": filename, "sha256": record["sha256"]}
        fixture_data[fixture_id] = data
        _guard(work, started)
    return records, fixture_inventory, fixture_data


def _hold_name(case: dict) -> tuple[str, ...]:
    return ("before_onload", "after_onload", "before_publish") if case["role"] == "decode" else ("before_admission",)


def _strict_trace(worker: Worker) -> tuple[bytes, str]:
    path = worker.observe / "trace.jsonl"
    if _is_reparse(path) or not path.is_file():
        raise ValueError("trace_missing")
    if path.stat().st_size > BOUNDS["max_trace_bytes"]:
        raise ValueError("trace_cap_exceeded")
    data = path.read_bytes()
    if not data or not data.endswith(b"\n"):
        raise ValueError("trace_incomplete")
    lines = data.splitlines()
    if len(lines) > BOUNDS["max_trace_records"]:
        raise ValueError("trace_record_cap_exceeded")
    try:
        rows = [json.loads(line.decode("utf-8")) for line in lines]
        text = data.decode("utf-8")
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError("trace_invalid") from exc
    if (rows != worker.rows or not rows or rows[-1].get("stage") != "probe_exit"
            or rows[-1].get("complete") is not True):
        raise ValueError("trace_terminal_invalid")
    return data, text


def _run_worker(case: dict, hold: str, work: Path, pwsh: Path, fixture_inventory: dict,
                fixture_data: dict, started: int, executable_sha: str) -> dict:
    worker_id = case["id"] + "_" + hold
    kind = "decode" if case["role"] == "decode" else "oversize"
    limits = dict(LIMITS, max_dimension=case["max_dimension"])
    roi = ",".join(str(value) for value in case["roi"]) if case["roi"] is not None else None
    _guard(work, started)
    worker = None
    try:
        worker = Worker(
            (worker_id, case["fixture_id"], kind, hold), work, pwsh,
            fixture_inventory, limits, roi=roi,
            max_roi_output=BOUNDS["max_roi_output_bytes"],
        )
        while not worker.at(hold):
            _guard(work, started, [worker])
            if worker.proc.poll() is not None:
                raise ValueError("worker_exited_before_required_boundary")
            time.sleep(0.002)
        _guard(work, started, [worker])
        sample = capture_window([worker], pwsh, 0)
        _guard(work, started, [worker])
        if sample.get("complete") is not True or sample.get("status") != "complete":
            raise ValueError("sampler_incomplete")
        totals = sample.get("totals")
        if not isinstance(totals, dict) or type(totals.get("private_bytes")) is not int:
            raise ValueError("observed_private_bytes_invalid")
        if totals["private_bytes"] > BOUNDS["max_observed_private_bytes"]:
            raise ValueError("observed_private_bound_exceeded")
        worker.identity_verified = True
        worker.release()
        while worker.proc.poll() is None:
            _guard(work, started, [worker])
            time.sleep(0.002)
        worker.proc.wait(timeout=5)
        _guard(work, started, [worker])
        result = worker.finish(executable_sha)
        trace_bytes, trace_utf8 = _strict_trace(worker)
        stdout_bytes = worker.stdout_path.read_bytes()
        stderr_bytes = worker.stderr_path.read_bytes()
        if (len(stdout_bytes) > BOUNDS["max_capture_bytes"]
                or len(stderr_bytes) > BOUNDS["max_capture_bytes"]):
            raise ValueError("capture_cap_exceeded")
        stdout_utf8 = stdout_bytes.decode("utf-8")
        stderr_utf8 = stderr_bytes.decode("utf-8")
        if (not result["reaped"] or result["alive_after_wait"] or result["exit_code"] is None
                or result["cancellation"]["state"] != "not_requested"
                or result["trace_complete"] is not True or result["trace_partial_bytes"] != 0
                or result["trace_bytes"] != len(trace_bytes)
                or result["trace_sha256"] != sha(trace_bytes)
                or result["stdout_bytes"] != len(stdout_bytes)
                or result["stderr_bytes"] != len(stderr_bytes)):
            raise ValueError("worker_finish_incomplete")
        if result["probe_result"] != json.loads(stdout_utf8):
            raise ValueError("probe_capture_mismatch")
        if case["role"] == "decode":
            if (result["exit_code"] != 0 or result["outcome"] != "decoded"
                    or not result["output_present"] or not result["published_success"]):
                raise ValueError("decode_worker_failed")
            if any((worker.observe / name).exists() for name in ("pending.png", "pending.bmp")):
                raise ValueError("decode_pending_output_remaining")
            output_path = worker.output
            if (not output_path.is_file() or _is_reparse(output_path)
                    or output_path.stat().st_size > BOUNDS["max_output_bytes"]):
                raise ValueError("output_cap_or_path_invalid")
            output_bytes = output_path.read_bytes()
            if len(output_bytes) != result["output_bytes"]:
                raise ValueError("output_size_mismatch")
            roi_coordinates = case["roi"]
            if roi_coordinates is None:
                raise ValueError("decode_roi_missing")
            oracle = compare_output(fixture_data[case["fixture_id"]], output_bytes, roi_coordinates)
            if oracle.get("ok") is not True:
                raise ValueError("roi_oracle_mismatch")
            output_hex = output_bytes.hex()
        else:
            if (result["exit_code"] != 4 or result["outcome"] != "oversize"
                    or result["probe_result"].get("admission_status") != "refused"
                    or result["probe_result"].get("materialization") != "not_requested"
                    or result["output_present"] or result["published_success"]
                    or any((worker.observe / name).exists() for name in ("pending.png", "pending.bmp"))):
                raise ValueError("refusal_worker_failed")
            output_hex = None
            oracle = None
        result.update({
            "limits": limits,
            "hold_stage": hold,
            "trace_utf8": trace_utf8,
            "stdout_utf8": stdout_utf8,
            "stderr_utf8": stderr_utf8,
            "output_hex": output_hex,
            "oracle": oracle,
            "sample": sample,
        })
        return result
    finally:
        if worker is not None:
            proc = worker.proc
            cleanup_error = None
            try:
                if proc.poll() is None:
                    proc.kill()
                proc.wait(timeout=5)
            except BaseException as exc:
                cleanup_error = exc
                try:
                    if proc.poll() is None:
                        proc.kill()
                    proc.wait(timeout=5)
                except BaseException:
                    pass
            for stream in (worker.stdout, worker.stderr):
                try:
                    stream.close()
                except BaseException as exc:
                    if cleanup_error is None:
                        cleanup_error = exc
            if cleanup_error is not None:
                raise ValueError("worker_cleanup_failed") from cleanup_error


def run(work_root, out, pwsh) -> dict:
    work, report_path, pwsh = map(_safe_path, (work_root, out, pwsh))
    if (work.exists() or report_path.exists() or report_path.is_relative_to(work)
            or work.is_relative_to(report_path)):
        raise ValueError("fresh_work_and_external_output_required")
    if not pwsh.is_file():
        raise ValueError("real_powershell_executable_required")
    if any(character in str(work) for character in ("'", "\r", "\n")):
        raise ValueError("unsafe_work_path")
    if len(SCENARIOS) != BOUNDS["max_cases"] or len(FIXTURES) != 2:
        raise ValueError("fixed_matrix_shape_invalid")
    before = _source_identity(pwsh)
    started = time.monotonic_ns()
    work.mkdir(parents=True, exist_ok=False)
    if _is_reparse(work):
        raise ValueError("work_root_reparse")
    (work / "workers").mkdir(exist_ok=False)
    fixture_records, fixture_inventory, fixture_data = _fixture_records(work, started)
    platform = _platform(pwsh)
    _guard(work, started)
    rows = []
    all_workers = []
    seen_lifetimes = set()
    for case in SCENARIOS:
        limits = dict(LIMITS, max_dimension=case["max_dimension"])
        worker_records = []
        for hold in _hold_name(case):
            worker_record = _run_worker(
                case, hold, work, pwsh, fixture_inventory, fixture_data,
                started, before["pwsh_sha256"],
            )
            lifetime = (worker_record["pid"], worker_record["start_utc_ticks"])
            if lifetime in seen_lifetimes:
                raise ValueError("worker_lifetime_reused")
            seen_lifetimes.add(lifetime)
            all_workers.append(worker_record)
            worker_records.append(worker_record)
        rows.append({
            "case_id": case["id"],
            "fixture_id": case["fixture_id"],
            "role": case["role"],
            "roi": case["roi"],
            "limits": limits,
            "failure": None,
            "classification": "ROI_OUTPUT_VERIFIED" if case["role"] == "decode" else "REFUSED_FULL_FRAME",
            "workers": worker_records,
        })
        _guard(work, started)
    if len(all_workers) != BOUNDS["max_worker_runs"]:
        raise ValueError("worker_run_count_invalid")
    after = _source_identity(pwsh)
    unchanged = _same_source(before, after)
    for relative, item in before["files"].items():
        item["unchanged_after_run"] = unchanged and (
            {k: v for k, v in item.items() if k != "unchanged_after_run"}
            == {k: v for k, v in after["files"][relative].items() if k != "unchanged_after_run"}
        )
    if not unchanged or not all(item["unchanged_after_run"] for item in before["files"].values()):
        raise ValueError("source_or_executable_changed_during_run")
    if any(worker["reaped"] is not True or worker["alive_after_wait"] is not False for worker in all_workers):
        raise ValueError("worker_cleanup_incomplete")
    _guard(work, started)
    work_bytes = _work_bytes(work)
    if work_bytes > BOUNDS["max_work_bytes"]:
        raise ValueError("work_cap_exceeded")
    record = {
        "schema_version": 1,
        "complete": True,
        "run_id": work.name,
        "bounds": BOUNDS,
        "decode_limits": LIMITS,
        "source_identity": before,
        "platform": platform,
        "fixtures": fixture_records,
        "cases": rows,
        "sampler_script_sha256": sha(SAMPLE_SCRIPT.encode("utf-8")),
        "allocation_scope": ALLOCATION_SCOPE,
        "unmeasured": list(UNMEASURED),
        "duration_ns": time.monotonic_ns() - started,
        "work_bytes": work_bytes,
        "cleanup": {"complete": True, "live_direct_children": []},
        "work_root_disposable": True,
        "limitations": [
            "Each hold is a separate worker lifetime; process counters are not same-process allocation deltas.",
            "Process counters include the worker runtime and observer effects; WIC-internal scratch and true allocation peaks are not measured.",
            "Boundary holds are artificial instrumentation and are not decoder latency.",
            "The existing OnLoad WIC path materializes the full frame before CopyPixels; verified ROI output does not verify partial decode.",
            "16K inputs are refused before OnLoad; 16K full-decode memory is NOT_MEASURED and GDI+ 16K is NOT_RUN.",
            "The Rust decode_compare harness process was not invoked; only the existing PowerShell WIC probe was run.",
        ],
    }
    if record["duration_ns"] > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    from w2_resource_verify import canonical
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    report_path.parent.mkdir(parents=True, exist_ok=True)
    if any(_is_reparse(part) for part in (report_path.parent, *report_path.parent.parents)):
        raise ValueError("report_parent_reparse")
    with report_path.open("xb") as stream:
        stream.write(data)
    if report_path.stat().st_size != len(data) or report_path.stat().st_size > BOUNDS["max_report_bytes"]:
        raise ValueError("report_write_invalid")
    return record


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    execute = commands.add_parser("run")
    for name in ("work-root", "out", "pwsh"):
        execute.add_argument("--" + name, required=True)
    check = commands.add_parser("verify")
    check.add_argument("--report", required=True)
    check.add_argument("--source-root")
    args = parser.parse_args(argv)
    try:
        if args.operation == "run":
            record = run(args.work_root, args.out, args.pwsh)
            result = verify(record, ROOT)
            success = result.get("ok") is True and result.get("closure_eligible") is True
        else:
            report = Path(args.report)
            if not report.is_file() or report.stat().st_size > BOUNDS["max_report_bytes"]:
                raise ValueError("report_missing_or_over_bound")
            record = json.loads(report.read_bytes().decode("utf-8"))
            result = verify(record, args.source_root)
            success = result.get("ok") is True
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0 if success else 1
    except Exception as exc:
        token = str(exc)
        reason = token if re.fullmatch(r"[a-z_]{1,80}", token) else type(exc).__name__
        print(json.dumps({"ok": False, "reason": reason}, sort_keys=True, separators=(",", ":")))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
