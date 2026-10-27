"""Fail-closed verifier for the W5 orientation/ICC metadata experiment."""
from __future__ import annotations

import json
from pathlib import Path

from w2_resource_verify import sha, STAGES
from w5_metadata_oracle import compare, orientation_spec, source

ROOT = Path(__file__).resolve().parents[4]
BOUNDS = {"max_cases": 16, "max_worker_seconds": 45, "max_total_seconds": 300,
          "max_input_bytes": 1024 * 1024, "max_output_bytes": 16 * 1024 * 1024,
          "max_work_bytes": 64 * 1024 * 1024, "max_trace_records": 128,
          "max_trace_bytes": 128 * 1024, "max_capture_bytes": 64 * 1024,
          "max_report_bytes": 8 * 1024 * 1024, "max_metadata_bytes": 64 * 1024}
LIMITS = {"max_dimension": 64, "max_pixels": 4096, "max_bit_depth": 8,
          "max_decoded_bytes": 65536, "max_scratch_bytes": 131072}
SOURCE_FILES = (
    "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "apps/desktop/examples/decode_compare/wic_admission.ps1",
    "apps/desktop/examples/decode_compare/harness.rs",
    "apps/desktop/examples/decode_compare/inspect.rs",
    "apps/desktop/examples/decode_compare/pixels.rs",
    "apps/desktop/tests/decode_compare_harness.rs",
    "apps/desktop/tests/wic_admission_selftest.ps1",
    "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "apps/desktop/devtools/img2/w2_resource_verify.py",
    "apps/desktop/devtools/img2/w4_region_verify.py",
    "apps/desktop/devtools/img2/w5_metadata_fixtures.py",
    "apps/desktop/devtools/img2/w5_metadata_oracle.py",
    "apps/desktop/devtools/img2/w5_metadata_matrix.py",
    "apps/desktop/devtools/img2/w5_metadata_verify.py",
)
CASES = [
    ("png_plain", "png_plain", "absent", None, False),
    ("bmp24_plain", "bmp24_plain", "absent", None, False),
    ("tiff_ori1", "tiff_ori1", "valid", 1, False),
    ("tiff_ori3", "tiff_ori3", "valid", 3, False),
    ("tiff_ori6", "tiff_ori6", "valid", 6, False),
    ("tiff_ori8", "tiff_ori8", "valid", 8, False),
    ("tiff_ori_invalid", "tiff_ori_invalid", "invalid", 99, False),
    ("tiff_ori_conflict", "tiff_ori_conflict", "conflicting", 6, False),
    ("tiff_icc_absent", "tiff_icc_absent", "valid", 1, False),
    ("tiff_icc_valid", "tiff_icc_valid", "valid", 1, False),
    ("tiff_icc_truncated", "tiff_icc_truncated", "valid", 1, False),
    ("tiff_ori6_apply", "tiff_ori6_apply", "valid", 6, True),
    ("tiff_icc_oversized", "tiff_icc_oversized", "valid", 1, False),
    ("unsupported_source", "unsupported_source", "absent", None, False),
    ("cancel_metadata", "tiff_ori6", "valid", 6, False),
    ("timeout_metadata", "tiff_ori6", "valid", 6, False),
]
CASE_IDS = tuple(c[0] for c in CASES)


def admission_math(fixture, result):
    bpp = {"Bgr24": 24, "Bgr32": 32, "Bgra32": 32, "Bgr565": 16, "Bgr555": 16}[result["pixel_format"]]
    row = (fixture["width"] * bpp + 7) // 8
    decoded = row * fixture["height"]
    bgra = fixture["width"] * fixture["height"] * 4
    scratch = bgra + result["declared_scratch_bytes"]
    return {"row_bytes": row, "decoded_bytes_required": decoded,
            "bgra32_bytes_required": bgra, "scratch_bytes_required": scratch}


def _need(cond, msg):
    if not cond:
        raise ValueError(msg)


def _verify(record, source_root):
    _need(record["schema_version"] == 1 and record["complete"] is True, "incomplete_report")
    _need(record["bounds"] == BOUNDS, "bounds_invalid")
    _need(record["cleanup"] == {"complete": True, "live_direct_children": []}, "cleanup_incomplete")
    fixtures = {f["id"]: f for f in record["fixtures"]}
    _need(len(fixtures) == len(record["fixtures"]) and set(fixtures) == {
        "png_plain", "bmp24_plain", "tiff_ori1", "tiff_ori3", "tiff_ori6", "tiff_ori8",
        "tiff_ori_invalid", "tiff_ori_conflict", "tiff_icc_absent", "tiff_icc_valid",
        "tiff_icc_truncated", "tiff_ori6_apply", "tiff_icc_oversized", "unsupported_source"},
        "fixture_set_invalid")
    for f in fixtures.values():
        data = bytes.fromhex(f["input_hex"])
        _need(len(data) == f["bytes"] and sha(data) == "sha256:" + f["sha256"], "fixture_identity_invalid")
        _need(len(data) <= BOUNDS["max_input_bytes"], "fixture_input_cap")
    identity = record["source_identity"]
    _need(set(identity["files"]) == set(SOURCE_FILES), "source_set_invalid")
    if source_root is not None:
        for name in SOURCE_FILES:
            data = (Path(source_root) / name).read_bytes()
            _need(sha(data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")) ==
                  identity["files"][name]["lf_sha256"], "source_digest_mismatch")
    workers = record["workers"]
    _need(len(workers) == len(CASE_IDS), "worker_count_invalid")
    _need([w["case_id"] for w in workers] == list(CASE_IDS), "case_order_or_set_invalid")
    capabilities = []
    for w in workers:
        cid = w["case_id"]
        expected = next(c for c in CASES if c[0] == cid)
        _, fid, _kind, value, apply_orientation = expected
        _need(w["fixture_id"] == fid and w["metadata_requested"] is True, "case_metadata_mode_invalid")
        fixture = fixtures[fid]
        data = bytes.fromhex(fixture["input_hex"])
        _need(w["input_sha256"] == "sha256:" + fixture["sha256"], "worker_input_mismatch")
        _need(w["exe_sha256"] == identity["pwsh_sha256"] and w["reaped"] is True, "worker_identity_invalid")
        stages = w["stages"]
        raw = w["trace_utf8"].encode()
        _need(w["trace_sha256"] == sha(raw) and w["trace_bytes"] == len(raw) and
              w["trace_records"] == len(stages), "trace_identity_invalid")
        _need([json.loads(line) for line in raw[:raw.rfind(b"\n") + 1].splitlines()] == stages,
              "trace_payload_invalid")
        names = [s["stage"] for s in stages]
        for seq, stage in enumerate(stages, 1):
            _need(stage["seq"] == seq and stage["pid"] == w["pid"], "trace_sequence_or_lifetime_invalid")
        result = w["probe_result"] or {}
        _need((json.loads(w["stdout_utf8"]) if w["stdout_utf8"].strip() else None) ==
              (w["probe_result"] or None), "stdout_result_mismatch")
        if cid in ("cancel_metadata", "timeout_metadata"):
            cancel = w["cancellation"]
            _need(cancel["state"] == "stop_confirmed" and cancel["cause"] == "boundary_hold" and
                  cancel["method"] == "direct_child_terminate_wait" and cancel["kill_error"] is None and
                  cancel["waited"] is True and cancel["exit_before_request"] is False and
                  cancel["returncode_before_request"] is None and
                  cancel["last_observed_stage"] == "before_publish" and
                  cancel["reason"] == ("cancel" if cid == "cancel_metadata" else "timeout") and
                  cancel["reachability"] == "reached" and cancel["hold_established"] is True and
                  cancel["kill_issued"] is True and cancel["reap_confirmed"] is True and
                  cancel["exit_after_request"] == w["exit_code"] not in (None, 0),
                  "stop_not_confirmed")
            _need(names == list(STAGES[:STAGES.index("before_publish") + 1]), "native_stage_set_incomplete")
            expected_outcome = "cancelled" if cid == "cancel_metadata" else "timeout"
            _need(w["outcome"] == expected_outcome and not w["published_success"] and
                  w["output_present"] is False and w["alive_after_wait"] is False and
                  cancel["requested_ns"] is not None and cancel["observed_ns"] is not None and
                  w["started_ns"] <= cancel["requested_ns"] <= cancel["observed_ns"] == w["exited_ns"],
                  "success_after_stop")
            continue
        _need(w["trace_complete"] is True and names[-1] == "probe_exit" and
              stages[-1]["complete"] is True, "native_incomplete")
        _need(all(stages[-1][k] == result[k] for k in
                  ("outcome", "reason_code", "admission_status", "materialization")),
              "probe_exit_result_mismatch")
        if cid == "unsupported_source":
            _need(w["outcome"] == "unsupported" and w["exit_code"] == 2 and
                  result["outcome"] == "unsupported" and
                  result["admission_status"] == "not_enforced" and
                  names == ["process_observed", "input_opened", "refused", "probe_exit"],
                  "unsupported_classification_invalid")
            _need(w["published_success"] is False and w["output_present"] is False and
                  w["output_bytes"] == 0 and w["output_hex"] is None and
                  w["output_sha256"] is None and result["materialization"] != "completed",
                  "output_after_refusal")
            capabilities.append({"case_id": cid, "codec": "bmp", "orientation_metadata": "UNAVAILABLE",
                                 "ICC_presence": "UNAVAILABLE", "ICC_readability": "UNAVAILABLE",
                                 "ICC_application": "NOT_VERIFIED", "colour_transform_fidelity": "NOT_VERIFIED",
                                 "full_frame_admission": "not_enforced", "decoded_output": "UNSUPPORTED",
                                 "limitation": "unsupported container is not a metadata capability"})
            continue
        if cid == "tiff_icc_oversized":
            _need(result.get("metadata_available") is True and result.get("icc_present") is True,
                  "metadata_presence_mismatch")
            cap_source = source(fixture, data)
            _need((fixture["width"], fixture["height"]) ==
                  (cap_source["width"], cap_source["height"]), "source_dimensions_mismatch")
            _need(cap_source["orientation_values"] == [1], "orientation_value_mismatch")
            _need(cap_source.get("icc") is not None and
                  cap_source["icc"]["declared"] > BOUNDS["max_metadata_bytes"],
                  "metadata_cap_not_evidenced")
            math = admission_math(fixture, result)
            _need(all(result[k] == v for k, v in math.items()), "full_frame_requirement_invalid")
            admitted = next(st for st in stages if st["stage"] == "admitted")
            _need(all(admitted[k] == v for k, v in math.items()), "trace_admission_mismatch")
            _need(w["outcome"] == "oversize" and not w["published_success"] and
                  w["output_present"] is False and w["output_hex"] is None and
                  w["output_bytes"] == 0 and result["materialization"] == "not_requested",
                  "metadata_cap_not_closed")
            _need((result["width"], result["height"]) == (fixture["width"], fixture["height"]),
                  "source_dimensions_mismatch")
            _need(result["pixel_format"] == "Bgr24" and result["bits_per_channel"] == 8 and
                  result["bits_per_pixel"] == 24, "source_format_mismatch")
            _need(result["decoder"] == "TiffBitmapDecoder", "decoder_identity_mismatch")
            _need(result.get("orientation_query") in
                  ("/ifd/{ushort=274}", "/ifd/274", "System.Photo.Orientation") and
                  result.get("orientation_raw") == 1, "orientation_query_mismatch")
            _need(w["exit_code"] == 4 and result["outcome"] == "oversize" and
                  result["reason_code"] == "metadata_cap_exceeded" and
                  result["admission_status"] == "refused" and result["materialization"] == "not_requested",
                  "metadata_cap_classification_invalid")
            _need(names == list(STAGES[:6]) + ["refused", "probe_exit"], "native_stage_set_incomplete")
            _need(w["output_present"] is False and result["icc_readable"] is False, "metadata_cap_not_closed")
            capabilities.append({"case_id": cid, "codec": "tiff", "orientation_metadata": "OBSERVED",
                                 "orientation_value": 1, "orientation_application": "NOT_RUN",
                                 "ICC_presence": "OBSERVED", "ICC_readability": "REFUSED_CAP",
                                 "ICC_application": "NOT_VERIFIED", "colour_transform_fidelity": "NOT_VERIFIED",
                                 "full_frame_admission": "admitted_then_metadata_refused",
                                 "decoded_output": "REFUSED", "limitation": "metadata byte bound"})
            continue
        _need(w["exit_code"] == 0 and w["outcome"] == "decoded" and result["outcome"] == "decoded" and
              result["admission_status"] == "admitted" and result["materialization"] == "completed" and
              w["published_success"] is True, "decode_missing")
        _need(names == list(STAGES), "native_stage_set_incomplete")
        expected_format = {"png_plain": "Bgr24", "bmp24_plain": "Bgr24"}.get(
            fid, "Bgr24" if fid.startswith("tiff") else None)
        _need((result["width"], result["height"]) == (fixture["width"], fixture["height"]),
              "source_dimensions_mismatch")
        _need(result["pixel_format"] == expected_format and result["bits_per_channel"] == 8 and
              result["bits_per_pixel"] == 24, "source_format_mismatch")
        expected_decoder = {"png": "PngBitmapDecoder", "bmp": "BmpBitmapDecoder",
                            "tif": "TiffBitmapDecoder"}[fixture["container"]]
        _need(result["decoder"] == expected_decoder, "decoder_identity_mismatch")
        if result.get("orientation_present"):
            _need(result.get("orientation_query") in
                  ("/ifd/{ushort=274}", "/ifd/274", "System.Photo.Orientation"),
                  "orientation_query_mismatch")
        expected_metadata = {"png_plain": True, "bmp24_plain": False}.get(fid, True)
        _need(result.get("metadata_available") is expected_metadata, "metadata_availability_mismatch")
        math = admission_math(fixture, result)
        _need(all(result[k] == v for k, v in math.items()), "full_frame_requirement_invalid")
        admitted = next(s for s in stages if s["stage"] == "admitted")
        _need(all(admitted[k] == v for k, v in math.items()), "trace_admission_mismatch")
        source_oracle = source(fixture, data)
        _need((fixture["width"], fixture["height"]) ==
              (source_oracle["width"], source_oracle["height"]), "source_dimensions_mismatch")
        status, _raw_value = orientation_spec(source_oracle["orientation_values"])
        _need(result["orientation_present"] == (status != "absent"), "orientation_presence_mismatch")
        if status != "absent":
            _need(result["orientation_readable"] is True and result["orientation_raw"] == value,
                  "orientation_value_mismatch")
        mapping = {1: "identity", 3: "rotate_180", 6: "rotate_90_cw", 8: "rotate_90_ccw"}
        if status == "valid":
            expected_interpretation = mapping[value]
        elif status == "invalid":
            expected_interpretation = "invalid_or_unsupported"
        elif status == "conflicting":
            expected_interpretation = "rotate_90_cw"
        else:
            expected_interpretation = "absent"
        _need(result["orientation_interpreted"] == expected_interpretation,
              "orientation_interpretation_mismatch")
        output = bytes.fromhex(w["output_hex"]) if w["output_hex"] else None
        _need(w["output_present"] and output is not None and sha(output) == w["output_sha256"],
              "decoded_output_missing")
        oracle = compare(fixture, data, output, apply_orientation)
        _need(oracle["ok"] is True and oracle == w["oracle"], "orientation_pixel_mismatch")
        _need((result["output_width"], result["output_height"]) == (oracle["width"], oracle["height"]),
              "output_dimensions_mismatch")
        _need(output is not None and w["output_bytes"] == len(output) <= BOUNDS["max_output_bytes"], "output_cap")
        icc_expected = oracle.get("icc_expected_sha256")
        if cid == "tiff_icc_valid":
            _need(result["icc_present"] is True and result["icc_readable"] is True and
                  result.get("colour_contexts", 0) >= 1 and
                  result["icc_sha256"] == icc_expected and
                  result["icc_length"] == oracle["icc_declared_length"], "icc_digest_mismatch")
            icc = {"presence": "VERIFIED", "readability": "VERIFIED", "application": "NOT_VERIFIED"}
        elif cid == "tiff_icc_truncated":
            _need(result.get("metadata_available") is True and result["icc_present"] is False and
                  result["icc_readable"] is False, "icc_invalid_tag_accepted")
            icc = {"presence": "INVALID_TAG_IGNORED", "readability": "NOT_READABLE", "application": "NOT_VERIFIED"}
        elif cid == "tiff_icc_absent":
            _need(result["icc_present"] is False, "icc_presence_mismatch")
            icc = {"presence": "ABSENT", "readability": "NOT_APPLICABLE", "application": "NOT_VERIFIED"}
        else:
            _need(result["icc_present"] is False, "icc_presence_mismatch")
            icc = {"presence": "ABSENT", "readability": "NOT_APPLICABLE", "application": "NOT_VERIFIED"}
        _need(result["transform_requested"] == ("explicit_rotate_90" if apply_orientation else "none"),
              "transform_request_mismatch")
        _need(result["transform_applied"] == ("by_probe" if apply_orientation else "not_requested"),
              "transform_application_mismatch")
        _need(result.get("colour_transform_verified", "NOT_VERIFIED") == "NOT_VERIFIED" and
              result.get("icc_application", "NOT_VERIFIED") == "NOT_VERIFIED", "false_colour_claim")
        _need(oracle["orientation_application"] == (
            "applied_by_probe" if apply_orientation else "not_observed" if status in ("valid", "invalid", "conflicting") else "not_observed"),
            "orientation_application_mismatch")
        capabilities.append({
            "case_id": cid, "codec": fixture["container"], "source_format": result["pixel_format"],
            "orientation_metadata": ("CONFLICTING" if status == "conflicting"
                                     else "OBSERVED" if status != "absent" else "ABSENT"),
            "orientation_value": value if status != "absent" else None,
            "orientation_application": "VERIFIED" if apply_orientation else "NOT_OBSERVED",
            "ICC_presence": icc["presence"], "ICC_readability": icc["readability"],
            "ICC_application": icc["application"], "colour_transform_fidelity": "NOT_VERIFIED",
            "full_frame_admission": "ENFORCED", "decoded_output": "VERIFIED",
            "measured_source_dimensions": [fixture["width"], fixture["height"]],
            "measured_output_dimensions": [oracle["width"], oracle["height"]],
            "limitation": "rectangle/format conversion is not ICC colour conversion; WIC did not auto-rotate"
                          if not apply_orientation else "explicit probe rotation, not a product policy",
        })
    _need(len(capabilities) == 14, "capability_row_missing")
    return {"ok": True, "fixture_count": len(fixtures), "worker_count": len(workers),
            "capability_rows": capabilities,
            "orientation_verified": sum(1 for c in capabilities if c.get("orientation_application") == "VERIFIED"),
            "unmeasured": record["unmeasured"],
            "meaning": "Actual WIC metadata observation; orientation/ICC presence is not transform fidelity."}


def verify(record, source_root=None):
    try:
        return _verify(record, source_root)
    except (ValueError, KeyError, TypeError, IndexError, OSError, OverflowError, AssertionError) as exc:
        return {"ok": False, "error": str(exc)[:160]}
