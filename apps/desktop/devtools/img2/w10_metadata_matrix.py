"""Bounded W10 JPEG metadata differential matrix over real WIC and GDI+ decoders.

Every leg runs the existing probe scripts as a separate process with an explicit
deadline, capture and output cap, and a fresh output path. Nothing here decides
colour management: an ICC profile that both decoders merely report is recorded
as recognition, never as a colour conversion.
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

from w2_resource_matrix import _safe_path
from w2_resource_verify import canonical, file_sha, sha
from w10_metadata_fixtures import CASES, write_fixture
from w10_metadata_reference import parse
from w10_metadata_verify import BOUNDS, LIMITS, SOURCE_FILES, UNMEASURED, verify

ROOT = Path(__file__).resolve().parents[4]
WIC = ROOT / "apps/desktop/examples/decode_compare/wic_probe.ps1"
GDI = ROOT / "apps/desktop/examples/decode_compare/gdiplus_probe.ps1"


def _git(*args):
    executable = os.environ.get("GIT_EXECUTABLE") or shutil.which("git")
    if not executable:
        raise ValueError("git_executable_unavailable")
    return subprocess.run([executable, *args], cwd=ROOT, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, check=True, timeout=15).stdout


def source_identity(pwsh):
    commit = _git("rev-parse", "HEAD").decode("ascii").strip()
    files = {}
    for path in SOURCE_FILES:
        data = (ROOT / path).read_bytes()
        normalized = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        blob = _git("show", f"{commit}:{path}")
        if normalized != blob.replace(b"\r\n", b"\n").replace(b"\r", b"\n"):
            raise ValueError("experimental_source_not_pinned:" + path)
        files[path] = {"bytes": len(data), "raw_sha256": sha(data), "lf_sha256": sha(normalized),
                       "git_blob_sha256": sha(blob), "unchanged_after_run": True}
    return {"source_commit": commit, "files": files, "pwsh_sha256": file_sha(pwsh),
            "wic_probe_sha256": file_sha(WIC), "gdiplus_probe_sha256": file_sha(GDI),
            "normalization": "Raw Native hashes plus explicitly declared LF-normalized Git correspondence"}


def _work_bytes(work):
    total = 0
    for path in work.rglob("*"):
        info = path.stat()
        if S_ISREG(info.st_mode):
            total += info.st_size
    return total


def _guard(work, started):
    if time.monotonic_ns() - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
        raise ValueError("total_deadline_exceeded")
    if _work_bytes(work) > BOUNDS["max_work_bytes"]:
        raise ValueError("work_byte_cap")


def _run(pwsh, argv, output, work, started):
    _guard(work, started)
    began = time.monotonic_ns()
    process = subprocess.Popen(argv, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        stdout, stderr = process.communicate(timeout=BOUNDS["max_worker_seconds"])
    except subprocess.TimeoutExpired:
        process.kill()
        stdout, stderr = process.communicate()
        raise ValueError("worker_deadline_exceeded")
    ended = time.monotonic_ns()
    _guard(work, started)
    if len(stdout) > BOUNDS["max_capture_bytes"] or len(stderr) > BOUNDS["max_capture_bytes"]:
        raise ValueError("capture_cap")
    record = {"pid": process.pid, "started_ns": began, "exited_ns": ended,
              "exit_code": process.returncode, "stdout_bytes": len(stdout),
              "stderr_bytes": len(stderr),
              "stdout_utf8": stdout.decode("utf-8", "replace")[:BOUNDS["max_capture_bytes"]],
              "stderr_utf8": stderr.decode("utf-8", "replace")[:BOUNDS["max_capture_bytes"]],
              "lifetime": "NOT_MEASURED",
              "lifetime_reason": "W10 observes metadata interpretation, not process lifetime."}
    try:
        record["probe_result"] = json.loads(record["stdout_utf8"]) if record["stdout_utf8"] else None
    except (UnicodeError, ValueError):
        record["probe_result"] = None
    if output.exists():
        info = output.stat()
        if info.st_size > BOUNDS["max_output_bytes"]:
            raise ValueError("output_cap")
        record["output"] = {"present": True, "bytes": info.st_size,
                            "sha256": "sha256:" + sha(output.read_bytes())}
    else:
        record["output"] = {"present": False, "bytes": 0, "sha256": None}
    return record


def _wic(pwsh, source, output, limits, transform, work, started):
    argv = [str(pwsh), "-NoProfile", "-File", str(WIC), "-InputPath", str(source),
            "-OutputPath", str(output), "-Metadata", "-MaxMetadataBytes",
            str(BOUNDS["max_metadata_bytes"]),
            "-MaxPixels", str(limits["max_pixels"]), "-MaxDimension", str(limits["max_dimension"]),
            "-MaxBitDepth", str(limits["max_bit_depth"]),
            "-MaxDecodedBytes", str(limits["max_decoded_bytes"]),
            "-MaxScratchBytes", str(limits["max_scratch_bytes"])]
    if transform:
        argv.append("-ApplyOrientation")
    return _run(pwsh, argv, output, work, started)


def _gdi(pwsh, source, output, limits, work, started):
    argv = [str(pwsh), "-NoProfile", "-File", str(GDI), "-InputPath", str(source),
            "-OutputPath", str(output), "-MaxPixels", str(limits["max_pixels"]),
            "-Metadata", "-MaxMetadataBytes", str(BOUNDS["max_metadata_bytes"])]
    return _run(pwsh, argv, output, work, started)


def _classify(record: dict, structure: dict) -> str:
    result = record.get("probe_result") or {}
    outcome = result.get("outcome")
    if outcome in (None, "not_run"):
        return "NOT_RUN"
    if outcome == "oversize":
        return "OVERSIZE"
    if outcome in ("unsupported", "corrupt"):
        return "REFUSED"
    if outcome == "error":
        return "TAMPER_NOT_VERIFIED"
    if structure["classification"] == "METADATA_TAMPERED":
        return "DECODED_TAMPER_NOT_VERIFIED"
    return "DECODED_METADATA_OBSERVED"


def run(work, out, pwsh):
    work, out, pwsh = map(_safe_path, (work, out, pwsh))
    if work.exists() or out.exists() or out.is_relative_to(work):
        raise ValueError("fresh_owned_work_and_external_report_required")
    if not pwsh.is_file():
        raise ValueError("real_executable_required")
    identity = source_identity(pwsh)
    started = time.monotonic_ns()
    work.mkdir(parents=True, exist_ok=False)
    cases_dir = work / "cases"
    cases_dir.mkdir()
    workers = work / "workers"
    workers.mkdir()
    fixtures = []
    structures = {}
    for case in CASES:
        record = write_fixture(case["id"], cases_dir / (case["id"] + ".jpg"))
        fixtures.append(record)
        structures[case["id"]] = parse((cases_dir / (case["id"] + ".jpg")).read_bytes())
    rows = []
    for case in CASES:
        structure = structures[case["id"]]
        row = {"case_id": case["id"], "kind": case["kind"], "reference_class": case["reference"],
               "structure": structure, "expected": {
                   "orientation": None if "exif" not in case["segments"] else case["orientation"],
                   "transform": fixtures[CASES.index(case)]["expected_transform"],
                   "output_width": fixtures[CASES.index(case)]["expected_output_width"],
                   "output_height": fixtures[CASES.index(case)]["expected_output_height"],
                   "icc_present": bool("icc" in case["segments"]),
                   "icc_sha256": fixtures[CASES.index(case)]["expected_icc_sha256"]},
               "legs": [], "classification": "NOT_RUN"}
        source = cases_dir / (case["id"] + ".jpg")
        meta_output = workers / (case["id"] + "_wic.png")
        meta_output.parent.mkdir(exist_ok=True)
        row["legs"].append(_leg("wic_metadata", case, source, meta_output, "wic",
                                lambda: _wic(pwsh, source, meta_output, LIMITS, False, work, started),
                                structure))
        if row["expected"]["transform"] not in ("none",):
            transform_output = workers / (case["id"] + "_wic_transform.png")
            row["legs"].append(_leg("wic_transform", case, source, transform_output, "wic",
                                    lambda: _wic(pwsh, source, transform_output, LIMITS, True, work, started),
                                    structure))
        gdi_output = workers / (case["id"] + "_gdiplus.png")
        row["legs"].append(_leg("gdiplus_metadata", case, source, gdi_output, "gdiplus",
                                lambda: _gdi(pwsh, source, gdi_output, LIMITS, work, started),
                                structure))
        row["classification"] = _row_class(row)
        rows.append(row)
    after = source_identity(pwsh)
    unchanged = identity == after
    for path, item in identity["files"].items():
        item["unchanged_after_run"] = unchanged and item == after["files"][path]
    platform_script = ('[ordered]@{powershell=$PSVersionTable.PSVersion.ToString();'
                       'dotnet=[Runtime.InteropServices.RuntimeInformation]::FrameworkDescription;'
                       'os=[Environment]::OSVersion.VersionString;'
                       'windows=[OperatingSystem]::IsWindows()}|ConvertTo-Json -Compress')
    platform = subprocess.run([str(pwsh), "-NoProfile", "-Command", platform_script],
                              capture_output=True, text=True, timeout=5, check=True)
    record = {"schema_version": 1, "run_id": work.name, "complete": len(rows) == len(CASES) and unchanged,
              "bounds": BOUNDS, "decode_limits": dict(LIMITS), "source_identity": identity,
              "platform": json.loads(platform.stdout), "fixtures": fixtures, "cases": rows,
              "unmeasured": list(UNMEASURED), "duration_ns": time.monotonic_ns() - started,
              "work_bytes": _work_bytes(work), "cleanup": {"complete": True, "live_direct_children": []},
              "work_root_disposable": True,
              "limitations": [
                  "ICC values are reported, not applied; colour conversion stays NOT_MEASURED.",
                  "WIC and GDI+ share the installed Windows JPEG codec, so agreement is differential.",
                  "GDI+ exposes no transform, trace or admission control; its leg is observation only.",
                  "A decoder accepting truncated metadata is never promoted to content verification.",
                  "Only the four ASCII EXIF orientations 1, 3, 6 and 8 are exercised."]}
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("xb") as stream:
        stream.write(data)
    return record


def _leg(name, case, source, output, candidate, runner, structure):
    record = {"leg": name, "case_id": case["id"], "candidate": candidate, "probe": None,
              "failure": None}
    try:
        body = runner()
        record.update(body)
        result = record.get("probe_result") or {}
        record["probe"] = result.get("probe")
        record["classification"] = _classify(record, structure)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        record["failure"] = type(exc).__name__ + ":" + str(exc)[:160]
        record["probe_result"] = None
        record["output"] = {"present": False, "bytes": 0, "sha256": None}
        record["classification"] = "NOT_RUN"
    if output.exists():
        output.unlink()
    return record


def _row_class(row):
    classes = [leg["classification"] for leg in row["legs"]]
    if any(value == "NOT_RUN" for value in classes):
        return "NOT_RUN"
    if row["kind"] == "malformed":
        if all(value in ("DECODED_TAMPER_NOT_VERIFIED", "REFUSED", "TAMPER_NOT_VERIFIED")
               for value in classes):
            return "TAMPER_CLASSIFIED"
        return "NOT_VERIFIED"
    if row["kind"] == "plain":
        return "CONTROL_OBSERVED" if all(value == "DECODED_METADATA_OBSERVED" for value in classes) else "NOT_VERIFIED"
    return "DIFFERENTIAL_OBSERVED" if all(value == "DECODED_METADATA_OBSERVED" for value in classes) else "NOT_VERIFIED"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    execute = sub.add_parser("run")
    for name in ("work-root", "out", "pwsh"):
        execute.add_argument("--" + name, required=True, type=Path)
    check = sub.add_parser("verify")
    check.add_argument("--report", required=True, type=Path)
    check.add_argument("--source-root", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.operation == "run":
            result = verify(run(args.work_root, args.out, args.pwsh), ROOT)
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
