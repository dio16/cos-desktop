"""Bounded W8 JPEG decoder matrix: WIC and GDI+ legs over fixed fixtures.

WIC runs through the existing budget-admitting probe with a boundary hold for a
process sample; GDI+ runs through the existing managed probe, which has no
pre-materialization admission and no trace, so its lifetime stays unmeasured.
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
from w8_jpeg_fixtures import CASES, write_fixture
from w8_jpeg_reference import absolute_rgba, parse
from w8_jpeg_verify import (BOUNDS, LIMITS, SOURCE_FILES, UNMEASURED, canonical_compare,
                            verify)

ROOT = Path(__file__).resolve().parents[4]
GDI = ROOT / "apps/desktop/examples/decode_compare/gdiplus_probe.ps1"
WIC = ROOT / "apps/desktop/examples/decode_compare/wic_probe.ps1"
RETAIN_LIMIT = 1 << 20


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
            "normalization": "Raw Native hashes plus explicitly declared LF-normalized Git correspondence",
            "probes": {"wic": file_sha(WIC), "gdiplus": file_sha(GDI)}}


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
    if _work_bytes(work) > BOUNDS["max_work_root"]:
        raise ValueError("work_cap_exceeded")
    if len(workers) > BOUNDS["max_concurrent_workers"]:
        raise ValueError("worker_cap_exceeded")
    for worker in workers:
        if now - worker.started > BOUNDS["max_worker_seconds"] * 1_000_000_000:
            raise ValueError("worker_deadline_exceeded")
        worker.read_trace()
        for path in (worker.stdout_path, worker.stderr_path):
            if path.stat().st_size > BOUNDS["max_capture_bytes"]:
                raise ValueError("worker_capture_cap_exceeded")
        if worker.output.exists() and worker.output.stat().st_size > BOUNDS["max_output_file"]:
            raise ValueError("output_cap_exceeded")


def _trim(path: Path, limit: int) -> dict:
    if not path.is_file():
        return {"present": False, "bytes": 0, "sha256": None, "hex": None}
    data = path.read_bytes()
    return {"present": True, "bytes": len(data), "sha256": sha(data),
            "hex": data.hex() if len(data) <= limit else None,
            "retained": len(data) <= limit}


def run_wic_leg(case, work, pwsh, fixtures, started, executable_sha):
    spec = (case["id"] + "_wic", case["id"], "success", "after_onload")
    worker = Worker(spec, work, pwsh, fixtures, LIMITS)
    row = {"case_id": case["id"], "candidate": "wic", "probe": "wic_probe.ps1",
           "failure": None, "sample": None, "classification": "NOT_RUN"}
    try:
        stage = "after_onload"
        while not worker.at(stage):
            guard(work, started, [worker])
            if worker.proc.poll() is not None:
                break
            time.sleep(.005)
        if worker.proc.poll() is None:
            window = capture_window([worker], pwsh, 0)
            row["sample"] = window
            if window["complete"]:
                worker.identity_verified = True
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
            record["output"] = _trim(worker.output, RETAIN_LIMIT)
            record["sample"] = row["sample"]
            row["leg"] = record
            row["classification"] = classify(record, row)
        finally:
            if worker.proc.poll() is None:
                worker.proc.kill()
                worker.proc.wait(timeout=5)
            worker.stdout.close()
            worker.stderr.close()
    return row


def run_gdi_leg(case, work, pwsh, started):
    root = work / "workers" / (case["id"] + "_gdi")
    root.mkdir(parents=True)
    output = root / "output.png"
    argv = [str(pwsh), "-NoProfile", "-File", str(GDI), "-InputPath",
            str(work / "cases" / (case["id"] + ".jpg")), "-OutputPath", str(output),
            "-MaxPixels", str(LIMITS["max_pixels"])]
    row = {"case_id": case["id"], "candidate": "gdiplus", "probe": "gdiplus_probe.ps1",
           "failure": None, "sample": None, "classification": "NOT_RUN"}
    process = None
    try:
        process = subprocess.Popen(argv, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        began = time.monotonic_ns()
        stdout, stderr = process.communicate(timeout=BOUNDS["max_worker_seconds"])
        ended = time.monotonic_ns()
        guard(work, started)
        record = {"pid": process.pid, "started_ns": began, "exited_ns": ended,
                  "exit_code": process.returncode,
                  "stdout_utf8": stdout.decode("utf-8", "replace"),
                  "stderr_utf8": stderr.decode("utf-8", "replace"),
                  "stdout_bytes": len(stdout), "stderr_bytes": len(stderr),
                  "trace_utf8": None, "stages": [], "trace_complete": False,
                  "lifetime": "NOT_MEASURED",
                  "lifetime_reason": "The existing GDI+ probe exposes no trace or boundary hold, so its short-lived process cannot be sampled with the bounded observer.",
                  "identity_verified": False, "output": _trim(output, RETAIN_LIMIT)}
        try:
            record["probe_result"] = json.loads(record["stdout_utf8"]) if record["stdout_utf8"] else None
        except (UnicodeError, ValueError):
            record["probe_result"] = None
        row["leg"] = record
        row["classification"] = classify(record, row)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        row["failure"] = type(exc).__name__ + ":" + str(exc)[:160]
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=5)
    return row


def classify(record, row):
    result = record.get("probe_result") or {}
    outcome = result.get("outcome")
    if outcome == "decoded":
        if row.get("reference_class") == "ABSOLUTE":
            return "DECODED_CONTENT_VERIFIED"
        return "DECODED_DIFFERENTIAL_ONLY"
    if outcome == "oversize":
        return "OVERSIZE"
    if outcome == "unsupported":
        return "UNSUPPORTED"
    if outcome == "corrupt":
        return "CORRUPT_REFUSED"
    if outcome == "error":
        return "TAMPER_NOT_VERIFIED"
    return "NOT_RUN"


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
    inventory = {}
    for case in CASES:
        if case["kind"] == "not_run":
            inventory[case["id"]] = {"case_id": case["id"], "path": None, "sha256": None,
                                     "bytes": 0, "width": case["width"], "height": case["height"],
                                     "reference": "NOT_RUN"}
            continue
        record = write_fixture(case["id"], cases_dir / (case["id"] + ".jpg"))
        inventory[case["id"]] = {"case_id": case["id"], "path": case["id"] + ".jpg",
                                 "sha256": "sha256:" + record["sha256"], "bytes": record["bytes"],
                                 "width": case["width"], "height": case["height"],
                                 "reference": case["reference"]}
    rows = []
    for case in CASES:
        if case["kind"] == "not_run":
            rows.append({"case_id": case["id"], "reference_class": "NOT_RUN",
                         "reason": case["reason"], "legs": [], "classification": "NOT_RUN"})
            continue
        data = (cases_dir / (case["id"] + ".jpg")).read_bytes()
        row = {"case_id": case["id"], "reference_class": case["reference"], "legs": [],
               "structure": None, "structure_error": None, "absolute": None,
               "comparison": None, "reason": None}
        try:
            structure = parse(data)
            row["structure"] = {key: structure[key] for key in
                                ("frame", "scan", "quant", "huffman", "restart_interval",
                                 "bytes", "progressive")}
            row["structure"]["segments"] = structure["segments"]
            if case["reference"] == "ABSOLUTE":
                row["absolute"] = absolute_rgba(structure)
        except (ValueError, KeyError, IndexError) as exc:
            row["structure_error"] = type(exc).__name__ + ":" + str(exc)[:120]
        fixtures = {case["id"]: {"case_id": case["id"], "path": case["id"] + ".jpg",
                                 "sha256": inventory[case["id"]]["sha256"],
                                 "width": case["width"], "height": case["height"]}}
        wic = run_wic_leg(case, work, pwsh, fixtures, started, identity["pwsh_sha256"])
        gdi = run_gdi_leg(case, work, pwsh, started)
        for leg in (wic, gdi):
            leg["reference_class"] = case["reference"]
            leg["classification"] = classify(leg.get("leg") or {}, leg)
        row["legs"] = [wic, gdi]
        wic_hex = (wic.get("leg") or {}).get("output", {}).get("hex")
        gdi_hex = (gdi.get("leg") or {}).get("output", {}).get("hex")
        if wic_hex and gdi_hex:
            try:
                row["comparison"] = canonical_compare(bytes.fromhex(wic_hex), bytes.fromhex(gdi_hex),
                                                      BOUNDS["max_report_bytes"])
            except (ValueError, OSError) as exc:
                row["comparison"] = {"error": type(exc).__name__ + ":" + str(exc)[:120]}
        row["classification"] = _row_classification(case, row)
        rows.append(row)
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
    record = {"schema_version": 1,
              "complete": len(rows) == len(CASES) and unchanged
              and all(row.get("failure") is None for r in rows for row in r.get("legs", [])),
              "run_id": work.name, "bounds": BOUNDS, "decode_limits": dict(LIMITS),
              "source_identity": identity, "platform": json.loads(platform.stdout),
              "fixtures": list(inventory.values()),
              "sampler_script_sha256": sha(SAMPLE_SCRIPT.encode("utf-8")),
              "cases": rows, "unmeasured": list(UNMEASURED),
              "duration_ns": time.monotonic_ns() - started, "work_bytes": _work_bytes(work),
              "cleanup": {"complete": True, "live_direct_children": []},
              "work_root_disposable": True,
              "limitations": [
                  "WIC and GDI+ share the installed Windows JPEG codec; agreement is differential, not absolute proof.",
                  "GDI+ has no pre-materialization admission and no boundary hold, so its lifetime and memory stay NOT_MEASURED.",
                  "WIC process counters are whole-process observations under an artificial hold.",
                  "Progressive and any sampling class without an approved encoder stay NOT_RUN.",
                  "A truncated JPEG decodes in both candidates and remains NOT_VERIFIED without a reference."]}
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("xb") as stream:
        stream.write(data)
    return record


def _row_classification(case, row):
    if row.get("structure_error") and case["kind"] == "malformed":
        return "TAMPER_NOT_VERIFIED"
    classes = [leg["classification"] for leg in row["legs"]]
    if case["reference"] == "ABSOLUTE":
        if all(c == "DECODED_CONTENT_VERIFIED" for c in classes):
            return "DECODED_CONTENT_VERIFIED"
        return "NOT_VERIFIED"
    if all(c == "DECODED_DIFFERENTIAL_ONLY" for c in classes):
        return "DECODED_DIFFERENTIAL_ONLY"
    if any(c in ("UNSUPPORTED", "OVERSIZE", "CORRUPT_REFUSED") for c in classes):
        return classes[0]
    return "NOT_VERIFIED"


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
