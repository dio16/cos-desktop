"""Bounded, pinned Windows WIC/GDI+ JPEG metadata and raster experiment."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from w2_resource_matrix import _safe_path
from w2_resource_verify import canonical, file_sha, sha
from w10_metadata_fixtures import CASES, write_fixture
from w10_metadata_reference import parse
from w10_metadata_pixels import output_record, comparisons
from w10_metadata_verify import BOUNDS, LIMITS, SOURCE_FILES, UNMEASURED, verify, row_class

ROOT = Path(__file__).resolve().parents[4]
WIC = ROOT / "apps/desktop/examples/decode_compare/wic_probe.ps1"
GDI = ROOT / "apps/desktop/examples/decode_compare/gdiplus_metadata_probe.ps1"


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
            "wic_probe_sha256": file_sha(WIC), "gdiplus_probe_sha256": file_sha(GDI)}


def _work_bytes(work):
    return sum(path.stat().st_size for path in work.rglob("*") if path.is_file())


def _run(case, name, source, folder, pwsh, work, started):
    folder.mkdir()
    output = folder / "output.png"
    stdout_path, stderr_path = folder / "stdout", folder / "stderr"
    candidate = "gdiplus" if name == "gdiplus_metadata" else "wic"
    probe = GDI if candidate == "gdiplus" else WIC
    argv = [str(pwsh), "-NoProfile", "-File", str(probe), "-InputPath", str(source),
            "-OutputPath", str(output), "-MaxPixels", str(LIMITS["max_pixels"]),
            "-Metadata", "-MaxMetadataBytes", str(BOUNDS["max_metadata_bytes"])]
    if candidate == "wic":
        argv += ["-MaxDimension", str(LIMITS["max_dimension"]), "-MaxBitDepth", str(LIMITS["max_bit_depth"]),
                 "-MaxDecodedBytes", str(LIMITS["max_decoded_bytes"]),
                 "-MaxScratchBytes", str(LIMITS["max_scratch_bytes"])]
        if name == "wic_transform":
            argv.append("-ApplyOrientation")
    began = time.monotonic_ns()
    process = None
    failure = None
    # File spools bound both memory use and pipe backpressure. Observe every cap
    # during execution, not after unbounded communicate() has accumulated bytes.
    with stdout_path.open("xb") as stdout, stderr_path.open("xb") as stderr:
        try:
            process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr)
            while process.poll() is None:
                now = time.monotonic_ns()
                if now - began > BOUNDS["max_worker_seconds"] * 1_000_000_000:
                    raise ValueError("worker_deadline_exceeded")
                if now - started > BOUNDS["max_total_seconds"] * 1_000_000_000:
                    raise ValueError("total_deadline_exceeded")
                if any(p.stat().st_size > BOUNDS["max_capture_bytes"] for p in (stdout_path, stderr_path)):
                    raise ValueError("capture_cap")
                if output.exists() and output.stat().st_size > BOUNDS["max_output_bytes"]:
                    raise ValueError("output_cap")
                if _work_bytes(work) > BOUNDS["max_work_bytes"]:
                    raise ValueError("work_byte_cap")
                time.sleep(.01)
        except (OSError, ValueError) as exc:
            failure = type(exc).__name__ + ":" + str(exc)
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=5)
    exited = time.monotonic_ns()
    captures = []
    for path in (stdout_path, stderr_path):
        with path.open("rb") as stream:
            data = stream.read(BOUNDS["max_capture_bytes"] + 1)
        if len(data) > BOUNDS["max_capture_bytes"]:
            failure = "capture_cap"
            data = data[:BOUNDS["max_capture_bytes"]]
        captures.append(data.decode("utf-8", "strict"))
    try:
        result = json.loads(captures[0])
    except ValueError:
        result = None
        failure = failure or "probe_result_missing"
    data = None
    if output.exists():
        with output.open("rb") as stream:
            data = stream.read(BOUNDS["max_output_bytes"] + 1)
        if len(data) > BOUNDS["max_output_bytes"]:
            raise ValueError("output_cap")
    return {"leg": name, "case_id": case["id"], "candidate": candidate,
            "input_sha256": file_sha(source), "probe_sha256": file_sha(probe),
            "pid": process.pid if process is not None else None,
            "started_ns": began, "exited_ns": exited,
            "exit_code": process.returncode if process is not None else None,
            "failure": failure, "reaped": process is not None and process.poll() is not None,
            "stdout_utf8": captures[0], "stderr_utf8": captures[1],
            "stdout_bytes": stdout_path.stat().st_size, "stderr_bytes": stderr_path.stat().st_size,
            "probe_result": result, "output": output_record(data),
            "lifetime": "NOT_MEASURED"}


def run(work, out, pwsh):
    work, out, pwsh = map(_safe_path, (work, out, pwsh))
    if work.exists() or out.exists() or out.is_relative_to(work):
        raise ValueError("fresh_owned_work_and_external_report_required")
    if not pwsh.is_file():
        raise ValueError("real_executable_required")
    identity = source_identity(pwsh)
    started = time.monotonic_ns()
    work.mkdir(parents=True)
    cases_dir = work / "cases"
    cases_dir.mkdir()
    fixtures, rows = [], []
    for case in CASES:
        path = cases_dir / (case["id"] + ".jpg")
        fixture = write_fixture(case["id"], path)
        fixtures.append(fixture)
        structure = parse(path.read_bytes())
        names = ["wic_metadata"]
        if fixture["expected_transform"] != "none":
            names.append("wic_transform")
        names.append("gdiplus_metadata")
        legs = [_run(case, name, path, work / (case["id"] + "_" + name), pwsh, work, started)
                for name in names]
        for leg in legs:
            outcome = (leg["probe_result"] or {}).get("outcome")
            leg["classification"] = ("NOT_RUN" if leg["failure"] else
                "DECODED_TAMPER_NOT_VERIFIED" if outcome == "decoded" and case["kind"] == "malformed" else
                "DECODED_METADATA_OBSERVED" if outcome == "decoded" else "REFUSED")
        row = {"case_id": case["id"], "kind": case["kind"], "reference_class": case["reference"],
               "structure": structure, "legs": legs,
               "expected": {"orientation": fixture["expected_orientation"],
                            "transform": fixture["expected_transform"],
                            "output_width": fixture["expected_output_width"],
                            "output_height": fixture["expected_output_height"],
                            "icc_present": fixture["expected_icc"],
                            "icc_sha256": fixture["expected_icc_sha256"]}}
        row["classification"] = row_class(row)
        rows.append(row)
    after = source_identity(pwsh)
    unchanged = identity == after
    record = {"schema_version": 2, "run_id": work.name, "complete": unchanged,
              "bounds": BOUNDS, "decode_limits": LIMITS, "source_identity": identity,
              "platform": {"os": os.name, "windows": os.name == "nt"},
              "fixtures": fixtures, "cases": rows, "comparisons": comparisons(rows),
              "unmeasured": list(UNMEASURED), "duration_ns": time.monotonic_ns() - started,
              "work_bytes": _work_bytes(work),
              "cleanup": {"complete": all(leg["reaped"] for row in rows for leg in row["legs"]),
                          "live_direct_children": []}, "work_root_disposable": True}
    if len(canonical(record)) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("xb") as stream:
        stream.write(canonical(record))
    return record


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
            with args.report.open("rb") as stream:
                data = stream.read(BOUNDS["max_report_bytes"] + 1)
            if len(data) > BOUNDS["max_report_bytes"]:
                raise ValueError("report_cap_exceeded")
            result = verify(json.loads(data), args.source_root)
        print(json.dumps(result, sort_keys=True))
        return 0 if result["ok"] and result["closure_eligible"] else 1
    except (OSError, ValueError, KeyError, TypeError, AssertionError, subprocess.SubprocessError) as exc:
        print(json.dumps({"ok": False, "error": type(exc).__name__, "reason": str(exc)[:200]}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
