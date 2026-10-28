"""Fail-closed W10 verification of pinned sources, encoded metadata and real pixels."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess

from w2_resource_verify import sha, canonical
from w10_metadata_fixtures import CASES, fixture_bytes, expected_dims, expected_transform, expected_orientation, ICC_PROFILE, WIDTH, HEIGHT
from w10_metadata_reference import parse
from w10_metadata_pixels import output_record, comparisons

BOUNDS = {"max_dimension": 2048, "max_pixels": 4194304, "max_bit_depth": 8,
          "max_encoded_input": 4 * 1024 * 1024, "max_metadata_bytes": 64 * 1024,
          "max_output_bytes": 1024 * 1024, "max_work_bytes": 128 * 1024 * 1024,
          "max_report_bytes": 4 * 1024 * 1024, "max_capture_bytes": 64 * 1024,
          "max_worker_seconds": 45, "max_total_seconds": 300, "max_native_legs": 32}
LIMITS = {"max_dimension": 2048, "max_pixels": 4194304, "max_bit_depth": 8,
          "max_decoded_bytes": 16 * 1024 * 1024, "max_scratch_bytes": 16 * 1024 * 1024}
SOURCE_FILES = (
    "apps/desktop/devtools/img2/w10_metadata_fixtures.py",
    "apps/desktop/devtools/img2/w10_metadata_reference.py",
    "apps/desktop/devtools/img2/w10_metadata_matrix.py",
    "apps/desktop/devtools/img2/w10_metadata_verify.py",
    "apps/desktop/devtools/img2/w10_metadata_pixels.py",
    "apps/desktop/devtools/img2/tests/test_w10_metadata_fixtures.py",
    "apps/desktop/devtools/img2/tests/test_w10_metadata_negative.py",
    "apps/desktop/devtools/img2/w5_metadata_fixtures.py",
    "apps/desktop/devtools/img2/w5_metadata_oracle.py",
    "apps/desktop/devtools/img2/w4_region_oracle.py",
    "apps/desktop/devtools/img2/w3_fixtures.py",
    "apps/desktop/devtools/img2/w8_jpeg_fixtures.py",
    "apps/desktop/devtools/img2/w2_independent_oracle.py",
    "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "apps/desktop/devtools/img2/w2_resource_verify.py",
    "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "apps/desktop/examples/decode_compare/wic_admission.ps1",
    "apps/desktop/examples/decode_compare/gdiplus_metadata_probe.ps1",
    "Cargo.lock",
)
UNMEASURED = (
    "ICC colour conversion", "absolute colour fidelity", "all JPEG metadata variants",
    "EXIF orientations outside 1/3/6/8", "GDI+ process lifetime and memory",
    "WIC internal scratch peak", "product orientation policy", "product ICC policy",
    "product decoder adoption", "full MP-IMG2 Decode Exit",
    "in-decoder cooperative cancellation", "absolute JPEG raster correctness",
)
PROBES = {"wic": "wic-gdiplus-v1", "gdiplus": "w10-system-drawing-metadata-v1"}
EXITS = {"decoded": 0, "error": 1, "unsupported": 2, "corrupt": 3, "oversize": 4}


def need(condition, message):
    if not condition:
        raise ValueError(message)


def row_class(row):
    if any(leg.get("failure") or leg["classification"] == "NOT_RUN" for leg in row["legs"]):
        return "NOT_RUN"
    if row["kind"] == "malformed":
        return "TAMPER_CLASSIFIED"
    if row["kind"] == "plain":
        return "CONTROL_OBSERVED"
    return "DIFFERENTIAL_OBSERVED"


def _source_identity(record, root):
    identity = record["source_identity"]
    need(set(identity["files"]) == set(SOURCE_FILES), "source_file_set_invalid")
    commit = identity["source_commit"]
    need(isinstance(commit, str) and len(commit) == 40 and all(c in "0123456789abcdef" for c in commit),
         "source_commit_invalid")
    for item in identity["files"].values():
        need(item["unchanged_after_run"] is True, "source_changed_during_run")
    if root is None:
        return False
    executable = os.environ.get("GIT_EXECUTABLE") or "git"
    for path in SOURCE_FILES:
        item = identity["files"][path]
        blob = subprocess.run([executable, "show", f"{commit}:{path}"], cwd=root,
                              capture_output=True, check=True, timeout=15).stdout
        normalized = blob.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        live = (Path(root) / path).read_bytes().replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        need(sha(blob) == item["git_blob_sha256"] and sha(normalized) == item["lf_sha256"],
             "source_blob_digest_mismatch:" + path)
        need(live == normalized, "candidate_source_differs_from_native:" + path)
        forms = (blob, normalized, normalized.replace(b"\n", b"\r\n"))
        need(any(item["raw_sha256"] == sha(body) and item["bytes"] == len(body) for body in forms),
             "source_raw_digest_mismatch:" + path)
    return True


def _metadata(result, fixture, structure, candidate):
    need(result.get("metadata_requested") is True, "metadata_not_requested")
    for key in ("orientation_present", "icc_present", "icc_readable"):
        need(key in result and (type(result[key]) is bool or result[key] is None), "metadata_flag_missing:" + key)
    # Invalid metadata may be exposed, ignored or refused. Record these differences,
    # never force agreement between decoders or turn acceptance into validity.
    if structure["classification"] == "METADATA_TAMPERED":
        if result["icc_readable"] is True:
            partial = structure.get("icc_partial")
            need(isinstance(partial, dict), "malformed_icc_without_payload")
            need(result.get("icc_length") == partial["bytes"] and
                 result.get("icc_sha256") == partial["payload_sha256"], "malformed_icc_payload_mismatch")
        return
    need(result.get("metadata_error") is None, "metadata_observation_error")
    orientation = fixture["expected_orientation"]
    need(result["orientation_present"] is (orientation is not None), "orientation_presence_mismatch")
    need(result.get("orientation_raw") == orientation, "orientation_value_mismatch")
    need(result["icc_present"] is fixture["expected_icc"], "icc_presence_mismatch")
    if fixture["expected_icc"]:
        need(result["icc_readable"] is True and result.get("icc_length") == len(ICC_PROFILE), "icc_length_mismatch")
        need(result.get("icc_sha256") == sha(ICC_PROFILE), "icc_digest_mismatch")
        if candidate == "wic":
            need(result.get("colour_contexts") == 1, "wic_colour_context_count_invalid")
    else:
        need(result["icc_readable"] is False and result.get("icc_length") is None and
             result.get("icc_sha256") is None, "absent_icc_claimed_readable")


def _leg(leg, case, fixture, structure, identity):
    need(leg["case_id"] == case["id"], "leg_case_binding_invalid")
    name = leg["leg"]
    candidate = "gdiplus" if name == "gdiplus_metadata" else "wic"
    need(leg["candidate"] == candidate, "leg_candidate_binding_invalid")
    need(leg["input_sha256"] == fixture["sha256"], "leg_input_digest_mismatch")
    need(leg["probe_sha256"] == identity[candidate + "_probe_sha256"], "leg_probe_digest_mismatch")
    need(leg["failure"] is None and leg["reaped"] is True, "worker_not_complete")
    need(type(leg["pid"]) is int and leg["pid"] > 0, "worker_identity_missing")
    need(type(leg["started_ns"]) is int and type(leg["exited_ns"]) is int and
         0 < leg["exited_ns"] - leg["started_ns"] <= BOUNDS["max_worker_seconds"] * 1_000_000_000 + 5_000_000_000,
         "worker_duration_invalid")
    for stream in ("stdout", "stderr"):
        data = leg[stream + "_utf8"].encode("utf-8")
        need(len(data) == leg[stream + "_bytes"] <= BOUNDS["max_capture_bytes"], "capture_invalid")
    result = leg["probe_result"]
    need(json.loads(leg["stdout_utf8"]) == result, "probe_stdout_mismatch")
    need(result["probe"] == PROBES[candidate], "probe_identity_invalid")
    need(result["input_bytes"] == fixture["bytes"], "input_bytes_mismatch")
    outcome = result["outcome"]
    need(outcome in EXITS and leg["exit_code"] == EXITS[outcome], "probe_exit_outcome_mismatch")
    output = leg["output"]
    if outcome == "decoded":
        need(output["present"] is True and isinstance(output["hex"], str) and
             len(output["hex"]) <= BOUNDS["max_output_bytes"] * 2, "decoded_output_missing")
        actual = output_record(bytes.fromhex(output["hex"]))
        need(actual == output, "output_bytes_or_pixels_mismatch")
        need(result["output_bytes"] == output["bytes"], "declared_output_bytes_mismatch")
        expected_dims = ((fixture["expected_output_width"], fixture["expected_output_height"])
                         if name == "wic_transform" else (WIDTH, HEIGHT))
        need((output["width"], output["height"]) == expected_dims, "output_geometry_mismatch")
        need((result["width"], result["height"]) == (WIDTH, HEIGHT), "probe_geometry_mismatch")
        if candidate == "wic":
            need((result["output_width"], result["output_height"]) == expected_dims, "probe_output_geometry_mismatch")
            need(result["admission_status"] == "admitted" and result["materialization"] == "completed", "wic_admission_invalid")
            need(result["decoded_bytes_required"] == WIDTH * HEIGHT * 3 and
                 result["bgra32_bytes_required"] == WIDTH * HEIGHT * 4 and
                 result["scratch_bytes_required"] == WIDTH * HEIGHT * 4, "wic_requirement_mismatch")
        _metadata(result, fixture, structure, candidate)
        expected_class = "DECODED_TAMPER_NOT_VERIFIED" if case["kind"] == "malformed" else "DECODED_METADATA_OBSERVED"
        need(leg["classification"] == expected_class, "decoded_classification_invalid")
        if name == "wic_transform":
            orientation = fixture["expected_orientation"]
            request = {1: "explicit_identity", 3: "explicit_rotate_180", 6: "explicit_rotate_90", 8: "explicit_rotate_270"}[orientation]
            applied = "by_probe_no_rotation" if orientation == 1 else "by_probe"
            need(result["transform_requested"] == request and result["transform_applied"] == applied,
                 "transform_request_or_application_mismatch")
        elif candidate == "wic":
            need(result["transform_requested"] == "none" and result["transform_applied"] == "not_requested", "unrequested_transform")
    else:
        need(output == output_record(None) and result.get("output_bytes") in (None, 0), "refused_leg_publishes")
        need(case["kind"] == "malformed" and leg["classification"] == "REFUSED", "required_case_not_decoded")


def verify(record, source_root=None):
    try:
        need(isinstance(record, dict) and record["schema_version"] == 2, "schema_version_invalid")
        need(record["complete"] is True and record["bounds"] == BOUNDS and record["decode_limits"] == LIMITS, "run_or_bounds_invalid")
        need(len(canonical(record)) <= BOUNDS["max_report_bytes"], "report_cap_exceeded")
        need(record["cleanup"] == {"complete": True, "live_direct_children": []}, "cleanup_incomplete")
        need(record["platform"]["windows"] is True and record["work_root_disposable"] is True, "native_environment_invalid")
        need(type(record["work_bytes"]) is int and 0 < record["work_bytes"] <= BOUNDS["max_work_bytes"], "work_bytes_invalid")
        need(type(record["duration_ns"]) is int and 0 < record["duration_ns"] <= BOUNDS["max_total_seconds"] * 1_000_000_000, "duration_invalid")
        need(record["unmeasured"] == list(UNMEASURED), "unmeasured_boundary_changed")
        ids = [case["id"] for case in CASES]
        need([f["case_id"] for f in record["fixtures"]] == ids and
             [row["case_id"] for row in record["cases"]] == ids, "case_or_fixture_set_invalid")
        legs = 0
        for case, fixture, row in zip(CASES, record["fixtures"], record["cases"]):
            data = fixture_bytes(case["id"])
            dims = expected_dims(case)
            expected = {"orientation": expected_orientation(case), "transform": expected_transform(case),
                        "output_width": dims[0], "output_height": dims[1],
                        "icc_present": any(s.startswith("icc") for s in case["segments"]),
                        "icc_sha256": sha(ICC_PROFILE) if any(s.startswith("icc") for s in case["segments"]) else None}
            need(fixture["sha256"] == sha(data) and fixture["bytes"] == len(data) <= BOUNDS["max_encoded_input"], "fixture_bytes_or_digest_mismatch")
            need((fixture["width"], fixture["height"]) == (WIDTH, HEIGHT), "fixture_geometry_invalid")
            for key, value in expected.items():
                field = {"orientation": "expected_orientation", "transform": "expected_transform",
                         "output_width": "expected_output_width", "output_height": "expected_output_height",
                         "icc_present": "expected_icc", "icc_sha256": "expected_icc_sha256"}[key]
                need(fixture[field] == value, "fixture_expectation_forged")
            structure = parse(data)
            need(row["structure"] == structure and row["expected"] == expected, "structure_or_expectation_forged")
            need(row["kind"] == case["kind"] and row["reference_class"] == case["reference"], "row_identity_invalid")
            need((structure["classification"] == "METADATA_TAMPERED") == (case["kind"] == "malformed"), "metadata_classification_invalid")
            names = ["wic_metadata"] + (["wic_transform"] if expected["transform"] != "none" else []) + ["gdiplus_metadata"]
            need([leg["leg"] for leg in row["legs"]] == names, "leg_set_invalid")
            for leg in row["legs"]:
                _leg(leg, case, fixture, structure, record["source_identity"])
            need(row["classification"] == row_class(row), "row_classification_invalid")
            legs += len(row["legs"])
        need(legs <= BOUNDS["max_native_legs"], "leg_count_over_cap")
        actual = comparisons(record["cases"])
        need(record["comparisons"] == actual, "pixel_comparison_forged")
        for comparison in actual.values():
            relation = comparison["orientation_relation"]
            if relation is not None:
                need(relation["exact"] is True, "orientation_raster_mismatch")
        source_verified = _source_identity(record, source_root)
        return {"ok": True, "cases": len(CASES), "legs": legs,
                "source_identity_verified": source_verified, "closure_eligible": source_verified,
                "metadata_tampered": sum(c["kind"] == "malformed" for c in CASES),
                "orientation_relations_verified": sum(v["orientation_relation"] is not None for v in actual.values()),
                "icc_colour_conversion": "NOT_MEASURED", "jpeg_absolute_correctness": "NOT_VERIFIED"}
    except (OSError, ValueError, KeyError, TypeError, IndexError, AssertionError, StopIteration, subprocess.SubprocessError) as exc:
        return {"ok": False, "reason": type(exc).__name__ + ":" + str(exc)[:180],
                "source_identity_verified": False, "closure_eligible": False}
