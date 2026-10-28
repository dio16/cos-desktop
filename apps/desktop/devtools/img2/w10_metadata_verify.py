"""Fail-closed verification for the W10 JPEG metadata differential matrix."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess

from w2_resource_verify import sha
from w10_metadata_fixtures import CASES, fixture_bytes
from w10_metadata_reference import parse

ROOT = Path(__file__).resolve().parents[4]
BOUNDS = {"max_dimension": 2048, "max_pixels": 4194304, "max_bit_depth": 8,
          "max_encoded_input": 4 * 1024 * 1024, "max_metadata_bytes": 64 * 1024,
          "max_output_bytes": 32 * 1024 * 1024, "max_work_bytes": 128 * 1024 * 1024,
          "max_report_bytes": 4 * 1024 * 1024, "max_capture_bytes": 64 * 1024,
          "max_worker_seconds": 45, "max_total_seconds": 300, "max_native_legs": 24}
LIMITS = {"max_dimension": 2048, "max_pixels": 4194304, "max_bit_depth": 8,
          "max_decoded_bytes": 16 * 1024 * 1024, "max_scratch_bytes": 16 * 1024 * 1024}
SOURCE_FILES = (
    "apps/desktop/devtools/img2/w10_metadata_fixtures.py",
    "apps/desktop/devtools/img2/w10_metadata_reference.py",
    "apps/desktop/devtools/img2/w10_metadata_matrix.py",
    "apps/desktop/devtools/img2/w10_metadata_verify.py",
    "apps/desktop/devtools/img2/w5_metadata_fixtures.py",
    "apps/desktop/devtools/img2/w8_jpeg_fixtures.py",
    "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "apps/desktop/devtools/img2/w2_resource_verify.py",
    "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "apps/desktop/examples/decode_compare/gdiplus_probe.ps1",
    "crates/media/src/decode.rs",
    "Cargo.lock",
)
UNMEASURED = (
    "ICC colour conversion", "absolute colour fidelity", "all JPEG metadata variants",
    "EXIF orientations outside 1/3/6/8", "GDI+ process lifetime and memory",
    "WIC internal scratch peak", "product orientation policy", "product ICC policy",
    "product decoder adoption", "full MP-IMG2 Decode Exit",
)
PROBES = {"wic": "wic-gdiplus-v1", "gdiplus": "system-drawing-gdiplus-v1"}
LEG_ORDER = {"wic_metadata": 0, "wic_transform": 1, "gdiplus_metadata": 2}
MUTATION_CLASSES = 24


def need(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum=None):
    if type(value) is not int:
        return False
    return True if minimum is None else value >= minimum


def _source_identity(record, source_root) -> bool:
    identity = record.get("source_identity")
    need(isinstance(identity, dict), "source_identity_missing")
    need(isinstance(identity.get("files"), dict) and identity["files"], "source_files_missing")
    need(all(path in identity["files"] for path in SOURCE_FILES), "source_files_incomplete")
    if source_root is None:
        return False
    root = Path(source_root)
    commit = identity.get("source_commit")
    need(isinstance(commit, str) and len(commit) == 40, "source_commit_invalid")

    def git(*args):
        executable = os.environ.get("GIT_EXECUTABLE") or "git"
        return subprocess.run([executable, *args], cwd=root, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, check=True, timeout=15).stdout

    for path, item in identity["files"].items():
        blob = git("show", f"{commit}:{path}")
        normalized = blob.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        need(item["git_blob_sha256"] == sha(blob), "source_blob_digest_mismatch:" + path)
        need(item["lf_sha256"] == sha(normalized), "source_lf_digest_mismatch:" + path)
        need(item.get("unchanged_after_run") is True, "source_changed_during_run:" + path)
    return True


def _expected_legs(case, fixture):
    names = ["wic_metadata"]
    if fixture["expected_transform"] != "none":
        names.append("wic_transform")
    names.append("gdiplus_metadata")
    return names


def _check_structure(structure, kind):
    need(structure["classification"] in ("METADATA_PARSED", "METADATA_ABSENT", "METADATA_TAMPERED"),
         "structure_classification_invalid")
    need(not structure["unknown_markers"], "structure_unknown_markers")
    need(structure["markers"][0] == "SOI" and structure["markers"][-1] in ("EOI", "SOS"),
         "structure_marker_order_invalid")
    if kind == "malformed":
        need(structure["classification"] == "METADATA_TAMPERED", "malformed_must_be_tampered")
    else:
        need(structure["classification"] != "METADATA_TAMPERED", "wellformed_marked_tampered")


def _check_result(leg, case, fixture, structure):
    result = leg.get("probe_result")
    need(isinstance(result, dict), "probe_result_missing:" + leg["leg"])
    need(result.get("probe") == PROBES[leg["candidate"]], "probe_identity_invalid:" + leg["leg"])
    need(result.get("input_bytes") == fixture["bytes"], "input_bytes_mismatch:" + leg["leg"])
    outcome = result.get("outcome")
    need(outcome in ("decoded", "corrupt", "error", "unsupported", "oversize", "not_run"),
         "outcome_invalid:" + leg["leg"])
    if leg["classification"] == "NOT_RUN":
        need(result.get("outcome") in (None, "not_run"), "not_run_with_outcome")
        return
    if outcome != "decoded":
        need(leg.get("output", {}).get("present") is False, "refused_leg_published_output:" + leg["leg"])
        need(leg["classification"] in ("REFUSED", "TAMPER_NOT_VERIFIED", "OVERSIZE"),
             "refusal_classification_invalid:" + leg["leg"])
        return
    output = leg.get("output") or {}
    need(output.get("present") is True and integer(output.get("bytes"), 1), "decoded_leg_output_missing:" + leg["leg"])
    need(result.get("output_bytes") == output["bytes"], "declared_output_bytes_mismatch:" + leg["leg"])
    need(result.get("output_width") == output.get("width", result.get("output_width")),
         "output_width_echo_invalid:" + leg["leg"])
    expected = "DECODED_TAMPER_NOT_VERIFIED" if structure["classification"] == "METADATA_TAMPERED" else "DECODED_METADATA_OBSERVED"
    need(leg["classification"] == expected, "decoded_classification_invalid:" + leg["leg"])
    if leg["candidate"] == "wic":
        need(result.get("metadata_requested") is True, "wic_metadata_not_requested")
        _check_wic_metadata(leg["leg"], result, case, fixture, structure)
    else:
        need(result.get("metadata_requested") is True, "gdiplus_metadata_not_requested")
        _check_gdiplus_metadata(leg["leg"], result, case, fixture, structure)


def _check_wic_metadata(name, result, case, fixture, structure):
    orientation = None if structure.get("exif") is None else structure["exif"]["orientation"]
    need(result.get("orientation_present") is (orientation is not None),
         "wic_orientation_presence_mismatch")
    if orientation is not None:
        need(result.get("orientation_raw") == orientation, "wic_orientation_value_mismatch")
    icc = structure.get("icc")
    need(result.get("icc_present") is (icc is not None), "wic_icc_presence_mismatch")
    if icc is not None:
        need(result.get("icc_readable") is True, "wic_icc_unreadable")
        need(result.get("icc_length") == icc["bytes"], "wic_icc_length_mismatch")
        need(result.get("icc_sha256") == icc["sha256"], "wic_icc_digest_mismatch")
        need(result.get("colour_contexts") == 1, "wic_colour_context_count_invalid")
    if name == "wic_transform":
        request = {1: "explicit_identity", 3: "explicit_rotate_180",
                   6: "explicit_rotate_90", 8: "explicit_rotate_270"}.get(orientation)
        applied = "by_probe_no_rotation" if orientation == 1 else "by_probe"
        need(request is not None, "transform_without_orientation")
        need(result.get("transform_requested") == request, "transform_request_mismatch")
        need(result.get("transform_applied") == applied, "transform_applied_mismatch")
        need(result.get("output_width") == fixture["expected_output_width"]
             and result.get("output_height") == fixture["expected_output_height"],
             "transform_dims_mismatch")
    if structure["classification"] == "METADATA_TAMPERED":
        need(result.get("transform_applied") in (None, "not_requested", "unavailable", "refused"),
             "tamper_transform_promoted")


def _check_gdiplus_metadata(name, result, case, fixture, structure):
    orientation = None if structure.get("exif") is None else structure["exif"]["orientation"]
    need(result.get("orientation_present") is (orientation is not None),
         "gdiplus_orientation_presence_mismatch")
    if orientation is not None:
        need(result.get("orientation_raw") == orientation, "gdiplus_orientation_value_mismatch")
    icc = structure.get("icc")
    need(result.get("icc_present") is (icc is not None), "gdiplus_icc_presence_mismatch")
    if icc is not None:
        need(result.get("icc_readable") is True, "gdiplus_icc_unreadable")
        need(result.get("icc_length") == icc["bytes"], "gdiplus_icc_length_mismatch")
        need(result.get("icc_sha256") == icc["sha256"], "gdiplus_icc_digest_mismatch")


def _check_cross_leg(row):
    results = {leg["leg"]: leg.get("probe_result") or {} for leg in row["legs"]}
    wic = results.get("wic_metadata") or {}
    gdi = results.get("gdiplus_metadata") or {}
    if wic.get("outcome") == "decoded" and gdi.get("outcome") == "decoded":
        need(wic.get("orientation_present") == gdi.get("orientation_present"),
             "leg_orientation_disagreement:" + row["case_id"])
        need(wic.get("icc_present") == gdi.get("icc_present"), "leg_icc_disagreement:" + row["case_id"])
        if wic.get("icc_present"):
            need(wic.get("icc_sha256") == gdi.get("icc_sha256"), "leg_icc_digest_disagreement:" + row["case_id"])
            need(wic.get("icc_length") == gdi.get("icc_length"), "leg_icc_length_disagreement:" + row["case_id"])


def _check_row(row, fixtures):
    case = next(item for item in CASES if item["id"] == row["case_id"])
    fixture = fixtures[row["case_id"]]
    need(row["kind"] == case["kind"], "row_kind_mismatch:" + row["case_id"])
    need(row["reference_class"] == case["reference"], "row_reference_mismatch:" + row["case_id"])
    expected = parse(fixture_bytes(row["case_id"]))
    need(row["structure"] == expected, "row_structure_not_reproducible:" + row["case_id"])
    _check_structure(expected, case["kind"])
    need(json.dumps(row["expected"], sort_keys=True) == json.dumps({
        "orientation": None if "exif" not in case["segments"] else case["orientation"],
        "transform": fixture["expected_transform"],
        "output_width": fixture["expected_output_width"],
        "output_height": fixture["expected_output_height"],
        "icc_present": "icc" in case["segments"],
        "icc_sha256": fixture["expected_icc_sha256"]}, sort_keys=True),
        "row_expected_not_declared:" + row["case_id"])
    names = [leg["leg"] for leg in row["legs"]]
    need(names == _expected_legs(case, fixture), "leg_set_invalid:" + row["case_id"])
    for leg in row["legs"]:
        need(leg["case_id"] == row["case_id"], "leg_case_binding_invalid:" + leg["leg"])
        need(leg["candidate"] == ("gdiplus" if leg["leg"].startswith("gdiplus") else "wic"),
             "leg_candidate_binding_invalid:" + leg["leg"])
        need(leg.get("failure") is None, "leg_failure:" + leg["leg"] + ":" + str(leg.get("failure"))[:80])
        _check_result(leg, case, fixture, expected)
    _check_cross_leg(row)
    expected_row = {"malformed": "TAMPER_CLASSIFIED", "plain": "CONTROL_OBSERVED"}.get(case["kind"], "DIFFERENTIAL_OBSERVED")
    need(row["classification"] == expected_row, "row_classification_invalid:" + row["case_id"])


def verify(record, source_root=None):
    if not isinstance(record, dict):
        return {"ok": False, "reason": "report_not_object", "closure_eligible": False, "source_identity_verified": False}
    try:
        need(record.get("schema_version") == 1, "schema_version_invalid")
        need(record.get("complete") is True, "run_incomplete")
        bounds = record.get("bounds") or {}
        need(bounds.get("max_report_bytes") == BOUNDS["max_report_bytes"], "bounds_mismatch")
        need(record.get("decode_limits") == LIMITS, "decode_limits_mismatch")
        need(record.get("work_root_disposable") is True, "work_root_not_disposable")
        need((record.get("cleanup") or {}).get("complete") is True, "cleanup_incomplete")
        need(record.get("work_bytes", 0) <= BOUNDS["max_work_bytes"], "work_bytes_over_cap")
        need(record.get("duration_ns", 0) <= BOUNDS["max_total_seconds"] * 1_000_000_000,
             "duration_over_cap")
        unmeasured = record.get("unmeasured")
        need(isinstance(unmeasured, list) and list(unmeasured) == list(UNMEASURED),
             "unmeasured_boundary_changed")
        cases = record.get("cases")
        need(isinstance(cases, list) and cases, "cases_missing")
        fixtures = {item["case_id"]: item for item in record.get("fixtures") or []}
        need(list(fixtures) == [case["id"] for case in CASES], "fixture_set_invalid")
        for case in CASES:
            fixture = fixtures[case["id"]]
            data = fixture_bytes(case["id"])
            need(fixture["bytes"] == len(data), "fixture_bytes_mismatch:" + case["id"])
            need(fixture["sha256"] == sha(data), "fixture_digest_mismatch:" + case["id"])
            need(fixture["width"] == 256 and fixture["height"] == 192, "fixture_geometry_invalid:" + case["id"])
        need(len(cases) == len(CASES), "case_count_invalid")
        need(sum(len(row["legs"]) for row in cases) <= BOUNDS["max_native_legs"], "leg_count_over_cap")
        for row in cases:
            _check_row(row, fixtures)
        source_verified = _source_identity(record, source_root)
        return {"ok": True, "cases": len(cases), "legs": sum(len(row["legs"]) for row in cases),
                "metadata_parsed": sum(1 for row in cases if row["structure"]["classification"] == "METADATA_PARSED"),
                "metadata_absent": sum(1 for row in cases if row["structure"]["classification"] == "METADATA_ABSENT"),
                "metadata_tampered": sum(1 for row in cases if row["structure"]["classification"] == "METADATA_TAMPERED"),
                "differential": sum(1 for row in cases if row["classification"] == "DIFFERENTIAL_OBSERVED"),
                "control": sum(1 for row in cases if row["classification"] == "CONTROL_OBSERVED"),
                "tamper": sum(1 for row in cases if row["classification"] == "TAMPER_CLASSIFIED"),
                "source_identity_verified": source_verified,
                "closure_eligible": bool(source_verified)}
    except (ValueError, KeyError, TypeError, IndexError, subprocess.SubprocessError) as exc:
        return {"ok": False, "reason": str(exc)[:200], "closure_eligible": False,
                "source_identity_verified": False}
