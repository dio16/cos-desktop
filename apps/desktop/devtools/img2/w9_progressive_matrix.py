"""Bounded W9 progressive JPEG matrix: WIC and GDI+ legs over fixed fixtures.

Reuses the W8 leg runners (unchanged probes, W2 worker trace and bounded
capture) and adds the progressive multi-scan structure and differential
classification this slice needs.
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
from w6_memory_sample import SAMPLE_SCRIPT
from w8_jpeg_matrix import run_gdi_leg, run_wic_leg
from w8_jpeg_verify import canonical_compare
from w9_progressive_fixtures import CASES, write_fixture
from w9_progressive_reference import classify, parse_scans
from w9_progressive_verify import BOUNDS, LIMITS, SOURCE_FILES, UNMEASURED, verify

ROOT = Path(__file__).resolve().parents[4]
CASES_DIR = {"progressive_gray_256": "progressive", "progressive_gray_1024": "progressive",
             "malformed_scan_truncated": "malformed", "malformed_spectral_invalid": "malformed",
             "baseline_control_420_256": "control"}


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
        files[path] = {"bytes": len(data), "raw_sha256": sha(data), "lf_sha256": sha(normalized),
                       "git_blob_sha256": sha(blob), "unchanged_after_run": True}
    probes = {"wic": file_sha(ROOT / "apps/desktop/examples/decode_compare/wic_probe.ps1"),
              "gdiplus": file_sha(ROOT / "apps/desktop/examples/decode_compare/gdiplus_probe.ps1")}
    probes_lf = {"wic": sha((ROOT / "apps/desktop/examples/decode_compare/wic_probe.ps1").read_bytes().replace(b"\r\n", b"\n")),
                 "gdiplus": sha((ROOT / "apps/desktop/examples/decode_compare/gdiplus_probe.ps1").read_bytes().replace(b"\r\n", b"\n"))}
    return {"source_commit": commit, "files": files, "pwsh_sha256": file_sha(pwsh),
            "harness_sha256": file_sha(harness), "probes": probes, "probes_lf": probes_lf,
            "normalization": "Raw Native hashes plus explicitly declared LF-normalized Git correspondence"}


def _work_bytes(work):
    total = 0
    for path in work.rglob("*"):
        info = path.stat()
        if S_ISREG(info.st_mode):
            total += info.size
    return total


def _classify(case, leg_row):
    record = leg_row.get("leg") or {}
    result = record.get("probe_result") or {}
    outcome = result.get("outcome")
    if outcome == "decoded":
        return "DECODED_DIFFERENTIAL_ONLY" if case["reference"] == "DIFFERENTIAL" else (
            "CONTROL_DECODED" if case["reference"] == "CONTROL" else "DECODED_TAMPER_NOT_VERIFIED")
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
        record = write_fixture(case["id"], cases_dir / (case["id"] + ".jpg"))
        inventory[case["id"]] = {"case_id": case["id"], "path": case["id"] + ".jpg",
                                 "sha256": "sha256:" + record["sha256"], "bytes": record["bytes"],
                                 "width": case["width"], "height": case["height"],
                                 "reference": case["reference"], "kind": CASES_DIR[case["id"]]}
    rows = []
    for case in CASES:
        data = (cases_dir / (case["id"] + ".jpg")).read_bytes()
        row = {"case_id": case["id"], "reference_class": case["reference"],
               "kind": CASES_DIR[case["id"]], "structure": None, "structure_error": None,
               "legs": [], "comparison": None, "classification": "NOT_RUN"}
        try:
            structure = parse_scans(data)
            row["structure"] = {"frame": structure["frame"], "scans": structure["scans"],
                                "quant": structure["quant"],
                                "huffman": [{"table_class": table_class, "table_id": table_id, **body}
                                            for (table_class, table_id), body in sorted(structure["huffman"].items())],
                                "restart_interval": structure["restart_interval"],
                                "bytes": structure["bytes"], "markers": structure["markers"],
                                "classification": classify(structure)}
        except (ValueError, KeyError, IndexError) as exc:
            row["structure_error"] = type(exc).__name__ + ":" + str(exc)[:120]
        fixtures = {case["id"]: {"case_id": case["id"], "path": case["id"] + ".jpg",
                                 "sha256": inventory[case["id"]]["sha256"],
                                 "width": case["width"], "height": case["height"]}}
        wic = run_wic_leg(case, work, pwsh, fixtures, started, identity["pwsh_sha256"])
        gdi = run_gdi_leg(case, work, pwsh, started)
        for leg in (wic, gdi):
            leg["reference_class"] = case["reference"]
            leg["classification"] = _classify(case, leg)
        row["legs"] = [wic, gdi]
        wic_hex = (wic.get("leg") or {}).get("output", {}).get("hex")
        gdi_hex = (gdi.get("leg") or {}).get("output", {}).get("hex")
        if wic_hex and gdi_hex:
            row["comparison"] = canonical_compare(bytes.fromhex(wic_hex), bytes.fromhex(gdi_hex),
                                                  BOUNDS["max_report_bytes"])
        row["classification"] = _row_class(row)
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
              "complete": len(rows) == len(CASES) and unchanged,
              "run_id": work.name, "bounds": BOUNDS, "decode_limits": dict(LIMITS),
              "source_identity": identity, "platform": json.loads(platform.stdout),
              "fixtures": list(inventory.values()),
              "sampler_script_sha256": sha(SAMPLE_SCRIPT.encode("utf-8")),
              "cases": rows, "unmeasured": list(UNMEASURED),
              "duration_ns": time.monotonic_ns() - started, "work_bytes": _work_bytes(work),
              "cleanup": {"complete": True, "live_direct_children": []}, "work_root_disposable": True,
              "limitations": [
                  "Progressive entropy content is parsed for structure, not decoded; entropy values stay NOT_VERIFIED.",
                  "WIC and GDI+ share the installed Windows JPEG codec, so agreement is differential, not absolute.",
                  "GDI+ has no pre-materialization admission, trace or lifetime observation.",
                  "Refinement scans are not exercised; only an initial DC scan and an initial AC scan are emitted.",
                  "Malformed decoder success is never promoted to content verification."]}
    data = canonical(record)
    if len(data) > BOUNDS["max_report_bytes"]:
        raise ValueError("report_cap_exceeded")
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("xb") as stream:
        stream.write(data)
    return record


def _row_class(row):
    classes = [leg["classification"] for leg in row["legs"]]
    if row["kind"] == "progressive":
        if all(c == "DECODED_DIFFERENTIAL_ONLY" for c in classes):
            return "DIFFERENTIAL_VERIFIED"
        return "NOT_VERIFIED"
    if row["kind"] == "control":
        return "CONTROL_VERIFIED" if all(c == "CONTROL_DECODED" for c in classes) else "NOT_VERIFIED"
    if any(c == "DECODED_TAMPER_NOT_VERIFIED" for c in classes):
        return "DECODED_TAMPER_NOT_VERIFIED"
    if any(c in ("CORRUPT_REFUSED", "UNSUPPORTED") for c in classes):
        return "REFUSED"
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
