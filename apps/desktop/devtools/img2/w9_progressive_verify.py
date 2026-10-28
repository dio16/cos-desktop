"""Fail-closed verification for the W9 progressive JPEG matrix."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess

from w2_resource_verify import sha
from w8_jpeg_verify import canonical_compare
from w9_progressive_fixtures import CASES, fixture_bytes, measure
from w9_progressive_reference import classify, parse_scans

ROOT = Path(__file__).resolve().parents[4]
BOUNDS = {"max_dimension": 2048, "max_pixels": 4194304, "max_source_depth": 8,
          "max_output_bgra32": 16 * 1024 * 1024, "max_declared_scratch": 16 * 1024 * 1024,
          "max_encoded_input": 16 * 1024 * 1024, "max_output_file": 32 * 1024 * 1024,
          "max_work_root": 256 * 1024 * 1024, "max_report_bytes": 8 * 1024 * 1024,
          "max_trace_records": 128, "max_trace_bytes": 128 * 1024, "max_capture_bytes": 64 * 1024,
          "max_worker_seconds": 45, "max_total_seconds": 300, "max_concurrent_workers": 1,
          "max_native_legs": 10, "max_source_cases": 5}
LIMITS = {"max_dimension": 2048, "max_pixels": 4194304, "max_bit_depth": 8,
          "max_decoded_bytes": 16 * 1024 * 1024, "max_scratch_bytes": 16 * 1024 * 1024}
SOURCE_FILES = (
    "apps/desktop/devtools/img2/w9_progressive_fixtures.py",
    "apps/desktop/devtools/img2/w9_progressive_reference.py",
    "apps/desktop/devtools/img2/w9_progressive_matrix.py",
    "apps/desktop/devtools/img2/w9_progressive_verify.py",
    "apps/desktop/devtools/img2/w8_jpeg_fixtures.py",
    "apps/desktop/devtools/img2/w8_jpeg_reference.py",
    "apps/desktop/devtools/img2/w8_jpeg_matrix.py",
    "apps/desktop/devtools/img2/w8_jpeg_verify.py",
    "apps/desktop/devtools/img2/w7_large_oracle.py",
    "apps/desktop/devtools/img2/w6_memory_sample.py",
    "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "apps/desktop/devtools/img2/w2_resource_verify.py",
    "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "apps/desktop/examples/decode_compare/gdiplus_probe.ps1",
    "apps/desktop/examples/decode_compare/wic_admission.ps1",
    "apps/desktop/examples/decode_compare.rs",
    "apps/desktop/examples/decode_compare/harness.rs",
    "apps/desktop/examples/decode_compare/inspect.rs",
    "apps/desktop/examples/decode_compare/pixels.rs",
    "crates/media/src/decode.rs",
    "Cargo.lock",
    "rust-toolchain.toml",
)
UNMEASURED = (
    "progressive entropy content", "progressive refinement scans", "all progressive JPEGs",
    "absolute raster correctness for arbitrary JPEGs", "WIC internal scratch peak",
    "actual allocator reservations", "true concurrent allocation peak",
    "in-decoder cooperative cancellation", "GDI+ process lifetime and memory",
    "progressive scan-level partial materialization", "product decoder adoption",
    "product JPEG tolerance", "full MP-IMG2 Decode Exit",
)
WIC_STAGES = ("process_observed", "input_opened", "metadata_available", "before_admission",
              "admission_started", "admitted", "refused", "before_onload", "onload_entered",
              "onload_returned", "after_onload", "conversion_entered", "conversion_returned",
              "copy_pixels_entered", "copy_pixels_returned", "encode_entered", "encode_returned",
              "before_publish", "output_published", "probe_exit")
DECODE_STAGES = tuple(stage for stage in WIC_STAGES if stage != "refused")
CANONICAL_NORMALIZATION = "both legs asked for canonical PNG; RGBA rows compared byte exactly"
FRAME_KEYS = {"marker", "variant", "precision", "width", "height", "components", "component_count"}
COMPONENT_KEYS = {"id", "sampling", "h", "v", "quant_table"}
SCAN_KEYS = {"components", "ss", "se", "ah", "al", "frame_variant", "entropy_offset",
             "entropy_bytes", "restart_markers"}


def need(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum=None):
    if type(value) is not int:
        return False
    return True if minimum is None else value >= minimum


def requirements(case):
    # Progressive sources are single-component grayscale; the baseline control is
    # a three-component colour JPEG, so its declared depth differs.
    bits = 24 if case.get("kind") == "control" else 8
    row = (case["width"] * bits + 7) // 8
    return {"width": case["width"], "height": case["height"], "bits_per_channel": 8,
            "bits_per_pixel": bits, "row_bytes": row, "decoded_bytes_required": row * case["height"],
            "bgra32_bytes_required": case["width"] * case["height"] * 4,
            "scratch_bytes_required": case["width"] * case["height"] * 4,
            "declared_scratch_bytes": 0}


def _source_identity(record, source_root) -> bool:
    identity = record.get("source_identity")
    need(isinstance(identity, dict), "source_identity_missing")
    commit = identity.get("source_commit")
    need(isinstance(commit, str) and len(commit) == 40, "source_commit_invalid")
    files = identity.get("files")
    need(isinstance(files, dict) and set(files) == set(SOURCE_FILES), "source_file_set_invalid")
    for path, item in files.items():
        need(item.get("unchanged_after_run") is True, "source_changed_during_run:" + path)
    probes, lf_probes = identity.get("probes"), identity.get("probes_lf")
    need(isinstance(probes, dict) and set(probes) == {"wic", "gdiplus"}, "probe_identity_missing")
    need(isinstance(lf_probes, dict) and set(lf_probes) == {"wic", "gdiplus"}, "probe_lf_identity_missing")
    for name, relative in (("wic", "apps/desktop/examples/decode_compare/wic_probe.ps1"),
                           ("gdiplus", "apps/desktop/examples/decode_compare/gdiplus_probe.ps1")):
        data = (ROOT / relative).read_bytes()
        normalized = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        crlf = normalized.replace(b"\n", b"\r\n")
        need(probes[name] in (sha(data), sha(normalized), sha(crlf)), "probe_digest_mismatch:" + name)
        need(lf_probes[name] == sha(normalized), "probe_lf_digest_mismatch:" + name)
    if source_root is None:
        return False
    root = Path(source_root)
    git = os.environ.get("GIT_EXECUTABLE") or "git"
    for path, item in files.items():
        data = (root / path).read_bytes()
        normalized = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        current = subprocess.run([git, "show", f"{commit}:{path}"], cwd=root,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
        need(current.returncode == 0, "source_blob_missing:" + path)
        need(sha(normalized) == item["lf_sha256"], "source_lf_digest_mismatch:" + path)
        need(sha(current.stdout) == item["git_blob_sha256"], "source_git_blob_mismatch:" + path)
    return True


def _trace(record):
    raw = record.get("trace_utf8")
    need(isinstance(raw, str), "wic_trace_missing")
    data = raw.encode("utf-8")
    need(sha(data) == record.get("trace_sha256"), "trace_digest_mismatch")
    need(len(data) == record.get("trace_bytes") <= BOUNDS["max_trace_bytes"], "trace_size_invalid")
    prefix = data[:data.rfind(b"\n") + 1]
    stages = record.get("stages")
    need(isinstance(stages, list) and len(stages) == record.get("trace_records")
         and len(stages) <= BOUNDS["max_trace_records"], "trace_record_count_invalid")
    need(len(data) - len(prefix) == record.get("trace_partial_bytes"), "trace_partial_invalid")
    need([json.loads(line) for line in prefix.splitlines()] == stages, "trace_payload_mismatch")
    for index, stage in enumerate(stages, 1):
        need(stage.get("seq") == index and stage.get("stage") in WIC_STAGES, "stage_sequence_invalid")
        need((stage.get("pid"), stage.get("process_start_utc_ticks"))
             == (record.get("pid"), record.get("start_utc_ticks")), "stage_identity_invalid")
    return stages, data


def _wic_progressive(case, leg, fixture):
    need(leg.get("candidate") == "wic" and leg.get("case_id") == case["id"], "wic_leg_identity_invalid")
    record = leg.get("leg") or {}
    result = record.get("probe_result") or {}
    need(result.get("probe") == "wic-gdiplus-v1" and result.get("decoder") == "JpegBitmapDecoder",
         "wic_identity_invalid")
    need(result.get("outcome") == "decoded" and record.get("outcome") == "decoded"
         and record.get("exit_code") == 0, "wic_decode_outcome_invalid")
    values = requirements(case)
    for key in ("width", "height", "bits_per_channel", "bits_per_pixel", "row_bytes",
                "decoded_bytes_required", "bgra32_bytes_required", "scratch_bytes_required",
                "declared_scratch_bytes"):
        need(result.get(key) == values[key], "wic_requirement_mismatch:" + key)
    need(result.get("input_bytes") == len(fixture), "wic_input_bytes_mismatch")
    need(result.get("admission_status") == "admitted", "wic_admission_forged")
    stages, _ = _trace(record)
    need([stage["stage"] for stage in stages] == list(DECODE_STAGES), "wic_stage_sequence_invalid")
    terminal = stages[-1]
    need(terminal.get("outcome") == "decoded" and terminal.get("complete") is True,
         "wic_terminal_outcome_mismatch")
    need(record.get("trace_complete") is True and record.get("trace_partial_bytes") == 0
         and str(record.get("trace_utf8")).endswith("\n"), "wic_trace_incomplete")
    need(record.get("identity_verified") is True, "wic_identity_not_verified")
    sample = record.get("sample")
    need(isinstance(sample, dict) and sample.get("complete") is True
         and sample.get("failure") is None, "wic_process_sample_missing")
    output = record.get("output") or {}
    need(output.get("present") is True and output.get("hex"), "wic_output_missing")
    data = bytes.fromhex(output["hex"])
    need(output["sha256"] == sha(data) and output["bytes"] == len(data), "wic_output_digest_mismatch")
    need(record.get("published_success") is True and record.get("output_present") is True,
         "wic_publication_inconsistent")
    return data


def _gdi_progressive(case, leg, fixture):
    need(leg.get("candidate") == "gdiplus" and leg.get("case_id") == case["id"],
         "gdiplus_leg_identity_invalid")
    record = leg.get("leg") or {}
    result = record.get("probe_result") or {}
    need(record.get("lifetime") == "NOT_MEASURED" and record.get("stages") == [],
         "gdiplus_must_not_fabricate_stages")
    need(result.get("probe") == "system-drawing-gdiplus-v1"
         and result.get("decoder") == "Built-in JPEG Codec" and result.get("outcome") == "decoded",
         "gdiplus_identity_invalid")
    need(record.get("exit_code") == 0, "gdiplus_exit_invalid")
    need(result.get("width") == case["width"] and result.get("height") == case["height"],
         "gdiplus_geometry_mismatch")
    need(result.get("input_bytes") == len(fixture), "gdiplus_input_bytes_mismatch")
    output = record.get("output") or {}
    need(output.get("present") is True and output.get("hex"), "gdiplus_output_missing")
    data = bytes.fromhex(output["hex"])
    need(output["sha256"] == sha(data) and output["bytes"] == len(data), "gdiplus_output_digest_mismatch")
    need(result.get("output_bytes") == len(data), "gdiplus_declared_output_bytes_mismatch")
    return data


def _serialized_structure(structure):
    return {"frame": structure["frame"], "scans": structure["scans"], "quant": {str(key): value for key, value in structure["quant"].items()},
            "huffman": [{"table_class": table_class, "table_id": table_id, **body}
                        for (table_class, table_id), body in sorted(structure["huffman"].items())],
            "restart_interval": structure["restart_interval"], "bytes": structure["bytes"],
            "markers": structure["markers"], "classification": classify(structure)}


def _structure(row, expected_sof2):
    structure = row.get("structure")
    need(isinstance(structure, dict), "structure_missing")
    frame = structure.get("frame")
    need(isinstance(frame, dict) and set(frame) == FRAME_KEYS, "frame_structure_invalid")
    need(frame.get("variant") == ("progressive_dct" if expected_sof2 else "baseline_dct"),
         "frame_variant_mismatch")
    need(isinstance(frame.get("components"), list), "frame_components_invalid")
    for component in frame["components"]:
        need(set(component) == COMPONENT_KEYS, "component_structure_invalid")
    scans = structure.get("scans")
    need(isinstance(scans, list) and scans, "scan_list_missing")
    for scan in scans:
        need(set(scan) == SCAN_KEYS, "scan_structure_invalid")
    return structure, scans


def verify(record: dict, source_root=None) -> dict:
    try:
        need(isinstance(record, dict), "record_not_object")
        need(record.get("schema_version") == 1, "schema_version_invalid")
        need(record.get("bounds") == BOUNDS, "bounds_invalid")
        need(record.get("decode_limits") == LIMITS, "decode_limits_invalid")
        need(tuple(record.get("unmeasured") or ()) == UNMEASURED, "unmeasured_set_invalid")
        need(record.get("complete") is True, "run_incomplete")
        need(record.get("cleanup", {}).get("complete") is True
             and record["cleanup"].get("live_direct_children") == [], "cleanup_incomplete")
        need(integer(record.get("duration_ns"), 0)
             and record["duration_ns"] <= BOUNDS["max_total_seconds"] * 1_000_000_000,
             "run_deadline_exceeded")
        need(integer(record.get("work_bytes"), 0)
             and record["work_bytes"] <= BOUNDS["max_work_root"], "work_bound_exceeded")
        fixtures = record.get("fixtures")
        need(isinstance(fixtures, list) and len(fixtures) == len(CASES), "fixture_set_invalid")
        by_id = {item["case_id"]: item for item in fixtures}
        need(tuple(by_id) == tuple(case["id"] for case in CASES), "fixture_order_invalid")
        for case in CASES:
            measured = measure(case["id"])
            need(by_id[case["id"]]["sha256"] == measured["sha256"], "fixture_regeneration_mismatch")
            need(by_id[case["id"]]["bytes"] == measured["bytes"] <= BOUNDS["max_encoded_input"],
                 "fixture_bytes_invalid")
            if by_id[case["id"]]["kind"] == "malformed":
                need(by_id[case["id"]]["width"] == case["width"]
                     and by_id[case["id"]]["height"] == case["height"], "fixture_geometry_mismatch")
            else:
                parsed = parse_scans(fixture_bytes(case["id"]))
                need(by_id[case["id"]]["width"] == parsed["frame"]["width"]
                     and by_id[case["id"]]["height"] == parsed["frame"]["height"],
                     "fixture_geometry_mismatch")
        source_verified = _source_identity(record, source_root)
        cases = record.get("cases")
        need(isinstance(cases, list) and len(cases) == len(CASES), "case_count_invalid")
        need([row.get("case_id") for row in cases] == [case["id"] for case in CASES], "case_order_invalid")
        progressive = malformed = control = legs = 0
        for case, row in zip(CASES, cases):
            need(row.get("reference_class") == case["reference"], "reference_class_mismatch")
            need(row.get("kind") == by_id[case["id"]]["kind"], "case_kind_mismatch")
            fixture = fixture_bytes(case["id"])
            need(sha(fixture) == by_id[case["id"]]["sha256"], "fixture_bytes_not_regenerated")
            if case["kind"] in ("progressive", "control"):
                structure, scans = _structure(row, case["kind"] == "progressive")
                parsed = parse_scans(fixture)
                need(structure == _serialized_structure(parsed), "structure_recomputation_mismatch")
                need(structure["frame"]["width"] == case["width"]
                     and structure["frame"]["height"] == case["height"], "marker_geometry_mismatch")
                recomputed = classify(parsed)
                if case["kind"] == "progressive":
                    need(recomputed["structure"] == "VALID_PROGRESSIVE_STRUCTURE",
                         "progressive_structure_invalid:" + case["id"])
                    need(recomputed["dc_scans"] >= 1 and recomputed["ac_scans"] >= 1,
                         "progressive_missing_dc_or_ac_scan:" + case["id"])
                    need(recomputed["scans"] >= 2, "progressive_single_scan:" + case["id"])
                    need(len(scans) == recomputed["scans"], "scan_count_mismatch:" + case["id"])
                legs_rows = row.get("legs")
                need(isinstance(legs_rows, list) and len(legs_rows) == 2, "decoder_leg_count_invalid")
                wic = _wic_progressive(case, legs_rows[0], fixture)
                gdi = _gdi_progressive(case, legs_rows[1], fixture)
                legs += 2
                comparison = row.get("comparison")
                need(isinstance(comparison, dict), "differential_comparison_missing")
                need(comparison.get("normalization") == CANONICAL_NORMALIZATION,
                     "normalization_policy_changed")
                recomputed_comparison = canonical_compare(wic, gdi, BOUNDS["max_report_bytes"])
                for key in ("exact", "pixels", "differing_pixels", "differing_channels",
                            "maximum_channel_difference", "first_difference"):
                    need(comparison.get(key) == recomputed_comparison[key],
                         "differential_statistics_mismatch:" + key)
                need(abs(comparison.get("mean_channel_difference", -1)
                         - recomputed_comparison["mean_channel_difference"]) < 1e-9,
                     "differential_mean_mismatch")
                if case["kind"] == "progressive":
                    need(row.get("classification") == "DIFFERENTIAL_VERIFIED",
                         "progressive_classification_invalid:" + case["id"])
                    need(all(leg["classification"] == "DECODED_DIFFERENTIAL_ONLY" for leg in legs_rows),
                         "progressive_leg_classification_invalid")
                    progressive += 1
                else:
                    need(row.get("classification") == "CONTROL_VERIFIED", "control_classification_invalid")
                    control += 1
            else:
                need(row.get("structure_error") is not None, "malformed_structure_must_fail:" + case["id"])
                need(row.get("classification") in ("REFUSED", "DECODED_TAMPER_NOT_VERIFIED", "NOT_VERIFIED"),
                     "malformed_classification_invalid:" + case["id"])
                legs_rows = row.get("legs")
                need(isinstance(legs_rows, list) and len(legs_rows) == 2
                     and [leg.get("candidate") for leg in legs_rows] == ["wic", "gdiplus"],
                     "malformed_legs_missing:" + case["id"])
                for leg in legs_rows:
                    record_leg = leg.get("leg") or {}
                    result = record_leg.get("probe_result") or {}
                    observed = result.get("outcome")
                    need(observed in ("decoded", "corrupt", "error", "unsupported", "oversize"),
                         "malformed_outcome_invalid:" + case["id"])
                    if leg.get("candidate") == "wic":
                        need(record_leg.get("outcome") == observed, "malformed_worker_outcome_mismatch")
                        stages, _ = _trace(record_leg)
                        terminal = stages[-1] if stages else {}
                        need(terminal.get("stage") == "probe_exit" and terminal.get("outcome") == observed,
                             "malformed_terminal_outcome_mismatch")
                        need(record_leg.get("trace_complete") is True
                             and record_leg.get("trace_partial_bytes") == 0
                             and str(record_leg.get("trace_utf8")).endswith("\n"),
                             "malformed_complete_trace_has_partial_tail")
                    output = record_leg.get("output") or {}
                    if observed == "decoded":
                        need(leg.get("classification") == "DECODED_TAMPER_NOT_VERIFIED",
                             "malformed_decoded_leg_promoted")
                        need(record_leg.get("exit_code") == 0 and output.get("present") is True
                             and isinstance(output.get("hex"), str)
                             and output.get("sha256") == sha(bytes.fromhex(output["hex"])),
                             "malformed_decoded_leg_inconsistent")
                    else:
                        need(leg.get("classification") in ("CORRUPT_REFUSED", "UNSUPPORTED", "OVERSIZE",
                                                           "TAMPER_NOT_VERIFIED"),
                             "malformed_failure_leg_classification_invalid")
                        need(record_leg.get("exit_code") not in (0, None)
                             and output.get("present") is False, "malformed_failure_leg_inconsistent")
                    legs += 1
                malformed += 1
        need(progressive == 2 and malformed == 2 and control == 1, "case_totals_invalid")
        need(legs == 10, "decoder_leg_total_invalid")
        return {"ok": True, "cases": len(cases), "progressive_verified": progressive,
                "malformed": malformed, "control": control, "decoder_legs": legs,
                "source_identity_verified": source_verified, "closure_eligible": bool(source_verified),
                "fixture_regeneration_verified": True,
                "measurement_scope": "WIC process counters under an artificial hold; entropy content, "
                                     "refinement scans, GDI+ memory and internal scratch stay NOT_MEASURED"}
    except (OSError, ValueError, KeyError, TypeError, IndexError, json.JSONDecodeError) as exc:
        return {"ok": False, "error": type(exc).__name__, "reason": str(exc)[:200],
                "source_identity_verified": False, "closure_eligible": False}
