"""Fail-closed verifier for the W4 ROI-output versus partial-decode experiment.

It reads the retained actual WIC record and independently checks source identity,
case coverage, ROI bounds, full-frame budgets, stage populations, terminal truth,
pixel output and capability claims. It never derives an expectation from the
producer's output and never promotes rectangle output to region decode.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

from w2_resource_verify import canonical, sha, STAGES
from w4_region_oracle import compare_roi

ROOT = Path(__file__).resolve().parents[4]
BOUNDS = {
    "max_cases": 24, "max_worker_seconds": 45, "max_total_seconds": 300,
    "max_input_bytes": 1024 * 1024, "max_output_bytes": 16 * 1024 * 1024,
    "max_work_bytes": 64 * 1024 * 1024, "max_trace_records": 128,
    "max_trace_bytes": 128 * 1024, "max_capture_bytes": 64 * 1024,
    "max_report_bytes": 8 * 1024 * 1024,
}
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
    "apps/desktop/devtools/img2/w3_fixtures.py",
    "apps/desktop/devtools/img2/w3_oracle.py",
    "apps/desktop/devtools/img2/w4_fixtures.py",
    "apps/desktop/devtools/img2/w4_region_oracle.py",
    "apps/desktop/devtools/img2/w4_region_matrix.py",
    "apps/desktop/devtools/img2/w4_region_verify.py",
)
BASE_LIMITS = {"max_dimension": 64, "max_pixels": 4096, "max_bit_depth": 8,
               "max_decoded_bytes": 65536, "max_scratch_bytes": 131072}
LARGE_LIMITS = {**BASE_LIMITS, "max_decoded_bytes": 1024, "max_scratch_bytes": 4096}
POSITIVE = [
    ("roi_full_png", "png_rgb", "0,0,7,5", BASE_LIMITS, 65536),
    ("roi_center_png", "png_rgb", "2,1,3,2", BASE_LIMITS, 65536),
    ("roi_top_left_1x1", "png_rgb", "0,0,1,1", BASE_LIMITS, 65536),
    ("roi_bottom_right_1x1", "png_rgb", "6,4,1,1", BASE_LIMITS, 65536),
    ("roi_vertical_rgba", "png_rgba", "0,0,1,4", BASE_LIMITS, 65536),
    ("roi_horizontal_rgba", "png_rgba", "0,2,6,1", BASE_LIMITS, 65536),
    ("roi_odd_bmp24", "bmp24", "1,1,5,3", BASE_LIMITS, 65536),
    ("roi_boundary_bmp24", "bmp24", "1,1,6,4", BASE_LIMITS, 65536),
    ("roi_full_bgr565", "bmp565", "0,0,5,1", BASE_LIMITS, 65536),
    ("roi_center_bgr565_odd", "bmp565_odd", "1,1,1,1", BASE_LIMITS, 65536),
    ("roi_full_bgr555_control", "bmp555_control", "0,0,3,3", BASE_LIMITS, 65536),
]
REFUSALS = [
    ("roi_negative_x", "png_rgb", "-1,0,1,1", BASE_LIMITS, 65536, "corrupt", "invalid_roi"),
    ("roi_negative_y", "png_rgb", "0,-1,1,1", BASE_LIMITS, 65536, "corrupt", "invalid_roi"),
    ("roi_zero_width", "png_rgb", "1,1,0,1", BASE_LIMITS, 65536, "corrupt", "invalid_roi"),
    ("roi_zero_height", "png_rgb", "1,1,1,0", BASE_LIMITS, 65536, "corrupt", "invalid_roi"),
    ("roi_x_outside", "png_rgb", "7,0,1,1", BASE_LIMITS, 65536, "corrupt", "invalid_roi"),
    ("roi_y_outside", "png_rgb", "0,5,1,1", BASE_LIMITS, 65536, "corrupt", "invalid_roi"),
    ("roi_arithmetic_overflow", "png_rgb", "9223372036854775807,0,1,1", BASE_LIMITS, 65536, "corrupt", "invalid_roi"),
    ("roi_output_budget", "png_rgb", "0,0,7,5", BASE_LIMITS, 100, "oversize", "roi_output_exceeded"),
    ("full_frame_budget_small_roi", "png_large", "0,0,1,1", LARGE_LIMITS, 65536, "oversize", "max_decoded_bytes_exceeded"),
    ("malformed_source", "bmp565_truncated", "0,0,1,1", BASE_LIMITS, 65536, "corrupt", "invalid_metadata"),
    ("unsupported_source", "bmp565_unsupported", "0,0,1,1", BASE_LIMITS, 65536, "unsupported", "win_unknown_image_format"),
]
STOPS = [
    ("cancel_roi", "png_rgb", "2,1,3,2", BASE_LIMITS, 65536, "cancel"),
    ("timeout_roi", "png_rgb", "2,1,3,2", BASE_LIMITS, 65536, "timeout"),
]
CASE_IDS = tuple(c[0] for c in POSITIVE + REFUSALS + STOPS)


def _need(condition, message):
    if not condition:
        raise ValueError(message)


def _int(text):
    value = int(text)
    _need(value >= 0 and value <= 2**31 - 1, "roi_component_out_of_range")
    return value


def _spec(roi):
    parts = roi.split(",")
    _need(len(parts) == 4 and all(p.isdigit() and (p == "0" or not p.startswith("0")) for p in parts),
          "roi_syntax_invalid")
    return tuple(_int(p) for p in parts)


def full_frame_bytes(fixture):
    cid = fixture["id"]
    bpp = {"png_rgb": 24, "bmp24": 24, "png_rgba": 32, "png_large": 24,
           "bmp565": 16, "bmp565_odd": 16, "bmp555_control": 16}.get(cid)
    _need(bpp is not None, "fixture_format_unknown")
    return ((fixture["width"] * bpp + 7) // 8) * fixture["height"]


def _verify(record, source_root):
    _need(record["schema_version"] == 1 and record["complete"] is True, "incomplete_report")
    _need(record["bounds"] == BOUNDS, "bounds_invalid")
    _need(record["cleanup"] == {"complete": True, "live_direct_children": []}, "cleanup_incomplete")
    _need(record["work_root_disposable"] is True, "work_root_not_disposable")
    fixtures = record["fixtures"]
    by_id = {f["id"]: f for f in fixtures}
    _need(len(by_id) == len(fixtures) and set(by_id) == {
        "png_rgb", "png_rgba", "bmp24", "png_large", "bmp565", "bmp565_odd",
        "bmp555_control", "bmp565_truncated", "bmp565_unsupported"}, "fixture_set_invalid")
    for f in fixtures:
        data = bytes.fromhex(f["input_hex"])
        _need(len(data) == f["bytes"] and sha(data) == "sha256:" + f["sha256"], "fixture_identity_invalid")
        _need(len(data) <= BOUNDS["max_input_bytes"], "fixture_input_cap")
        if "expected_rgba_hex" in f:
            _need(hashlib.sha256(bytes.fromhex(f["expected_rgba_hex"])).hexdigest() ==
                  f["expected_rgba_sha256"], "fixture_oracle_identity_invalid")
    identity = record["source_identity"]
    _need(set(identity["files"]) == set(SOURCE_FILES), "source_set_invalid")
    if source_root is not None:
        for name in SOURCE_FILES:
            data = (Path(source_root) / name).read_bytes()
            _need(sha(data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")) == identity["files"][name]["lf_sha256"],
                  "source_digest_mismatch")
    workers = record["workers"]
    _need(len(workers) == BOUNDS["max_cases"] == len(CASE_IDS), "worker_count_invalid")
    _need([w["case_id"] for w in workers] == list(CASE_IDS), "case_order_or_set_invalid")
    expected = {c[0]: c for c in POSITIVE + REFUSALS + STOPS}
    capability = []
    boundary = None
    seen = set()
    for w in workers:
        c = expected[w["case_id"]]
        cid, fid, roi, limits, cap = c[:5]
        entry = c[5] if len(c) > 5 else None
        reason = c[6] if len(c) > 6 else None
        _need(w["case_id"] not in seen, "duplicate_case")
        seen.add(w["case_id"])
        _need(w["fixture_id"] == fid and w["limits"] == limits, "case_limits_or_fixture_invalid")
        fixture = by_id[fid]
        _need(w["input_sha256"] == "sha256:" + fixture["sha256"], "worker_input_mismatch")
        _need(w["exe_sha256"] == identity["pwsh_sha256"], "worker_executable_mismatch")
        _need(w["reaped"] is True and w["alive_after_wait"] is False, "worker_not_reaped")
        stages = w["stages"]
        _need(len(stages) <= BOUNDS["max_trace_records"], "trace_record_cap")
        _need(len(w["trace_utf8"].encode()) <= BOUNDS["max_trace_bytes"], "trace_cap")
        _need(len(w["stdout_utf8"].encode()) <= BOUNDS["max_capture_bytes"], "capture_cap")
        raw = w["trace_utf8"].encode()
        _need(w["trace_sha256"] == sha(raw) and w["trace_bytes"] == len(raw) and
              w["trace_records"] == len(stages), "trace_identity_invalid")
        complete = raw[:raw.rfind(b"\n") + 1]
        _need([json.loads(line) for line in complete.splitlines()] == stages, "trace_payload_invalid")
        names = [s["stage"] for s in stages]
        for seq, stage in enumerate(stages, 1):
            _need(stage["seq"] == seq and stage["pid"] == w["pid"] and
                  stage["process_start_utc_ticks"] == w["start_utc_ticks"], "trace_sequence_or_lifetime_invalid")
        result = w["probe_result"] or {}
        _need((json.loads(w["stdout_utf8"]) if w["stdout_utf8"].strip() else None) ==
              (w["probe_result"] or None), "stdout_result_mismatch")
        try:
            x, y, width, height = _spec(roi)
        except ValueError:
            x = y = width = height = None
        if entry in ("cancel", "timeout"):
            _need(w["output_present"] is False and w["output_hex"] is None, "output_after_stop")
            _need(w["cancellation"]["state"] == "stop_confirmed" and
                  w["cancellation"]["hold_established"] is True and
                  w["cancellation"]["kill_issued"] is True and
                  w["cancellation"]["reap_confirmed"] is True and
                  w["cancellation"]["alive_at_request"] is True and
                  w["cancellation"]["exit_after_request"] not in (None, 0), "stop_not_confirmed")
            _need(names == list(STAGES[:STAGES.index("before_publish") + 1]),
                  "native_stage_set_incomplete")
            _need(stages[-1].get("artificial_instrumentation_hold") is True and
                  stages[-1]["stage"] == "before_publish", "stop_not_held")
            _need(w["outcome"] == ("cancelled" if entry == "cancel" else "timeout") and
                  result.get("materialization") != "completed" and not w["published_success"], "success_after_stop")
            continue
        _need(w["cancellation"]["state"] == "not_requested", "unexpected_stop")
        _need(w["trace_complete"] is True and names[-1] == "probe_exit" and
              stages[-1]["complete"] is True, "native_incomplete")
        _need(all(stages[-1][key] == result[key] for key in
                  ("outcome", "reason_code", "admission_status", "materialization")),
              "probe_exit_result_mismatch")
        if (cid not in ("malformed_source", "unsupported_source") and
                (entry is None or cid in ("roi_output_budget", "full_frame_budget_small_roi"))):
            _need(None not in (x, y, width, height), "roi_syntax_invalid")
            _need(result["roi_requested"] is True and result["decoder_cache_mode"] == "OnLoad" and
                  (result["roi_x"], result["roi_y"], result["roi_width"], result["roi_height"]) ==
                  (x, y, width, height), "roi_identity_invalid")
            _need(result["full_frame_decoded_bytes"] == full_frame_bytes(fixture),
                  "full_frame_requirement_invalid")
            _need(result["roi_output_bytes_required"] == width * height * 4, "roi_requirement_invalid")
        if entry is None:
            _need(w["exit_code"] == 0 and w["outcome"] == "decoded" and
                  result["outcome"] == "decoded" and result["admission_status"] == "admitted" and
                  result["materialization"] == "completed", "decode_missing")
            _need(names == list(STAGES), "native_stage_set_incomplete")
            _need(result["full_frame_materialization"] == "observed", "full_frame_not_observed")
            _need(result["true_partial_decode"] == "not_verified" and
                  result["sequential_decode"] == "not_verified" and
                  result["tiled_decode"] == "not_verified", "false_partial_decode_claim")
            _need(w["output_present"] and w["published_success"] and w["output_hex"] and
                  w["output_bytes"] <= BOUNDS["max_output_bytes"], "roi_output_missing")
            output = bytes.fromhex(w["output_hex"])
            _need(sha(output) == w["output_sha256"] and w["output_bytes"] == len(output), "roi_output_identity_invalid")
            _need(len(output) <= cap, "roi_output_cap")
            oracle = compare_roi(fixture, bytes.fromhex(fixture["input_hex"]), output, x, y, width, height)
            _need(oracle["ok"] is True and oracle == w["oracle"], "roi_pixel_mismatch")
            _need(result["roi_output"] == "produced_pending_publish", "roi_output_state_invalid")
            row = {
                "case_id": cid, "source_format": result["pixel_format"],
                "bits_per_channel": result["bits_per_channel"], "bits_per_pixel": result["bits_per_pixel"],
                "width": width, "height": height, "x": x, "y": y,
                "roi_output": "verified",
                "true_partial_decode": "not_verified",
                "sequential_decode": "not_verified", "tiled_decode": "not_verified",
                "full_frame_materialization": "observed",
                "internal_decoder_scratch": "not_measured",
                "preview_fallback": "bounded_full_frame",
            }
            if result["pixel_format"] == "Bgr565":
                _need(result["bits_per_channel"] == 6 and result["bits_per_pixel"] == 16,
                      "bgr565_depth_invalid")
                row["bgr565_limited_proof"] = True
            capability.append(row)
            continue
        # Refusals must never materialize, publish, or pass OnLoad.
        _need(w["exit_code"] != 0 and w["outcome"] == entry and result["outcome"] == entry, "refusal_class_invalid")
        _need(reason is not None and result["reason_code"] == reason, "refusal_reason_invalid")
        _need(result["admission_status"] == ("not_enforced" if cid == "unsupported_source" else "refused"),
              "refusal_admission_invalid")
        _need(result["materialization"] == "not_requested", "materialization_after_refusal")
        _need(w["output_present"] is False and w["output_hex"] is None and not w["published_success"],
              "output_after_refusal")
        if cid in ("malformed_source", "unsupported_source"):
            _need("onload_entered" not in names and "admitted" not in names, "materialization_after_refusal")
        if cid == "full_frame_budget_small_roi":
            _need(names == ["process_observed", "input_opened", "metadata_available", "before_admission",
                            "admission_started", "refused", "probe_exit"], "native_stage_set_incomplete")
            _need(result["full_frame_decoded_bytes"] > limits["max_decoded_bytes"] and
                  result["roi_output_bytes_required"] == 4 and
                  result["reason_code"] == "max_decoded_bytes_exceeded", "full_frame_budget_bypass")
            boundary = {"case_id": cid, "roi_width": width, "roi_height": height,
                        "roi_output_bytes_required": result["roi_output_bytes_required"],
                        "full_frame_decoded_bytes": result["full_frame_decoded_bytes"],
                        "max_decoded_bytes": limits["max_decoded_bytes"],
                        "outcome": result["outcome"], "reason_code": result["reason_code"],
                        "roi_output": "refused", "true_partial_decode": "not_verified"}
        elif cid == "roi_output_budget":
            _need(result["roi_output_bytes_required"] > cap and
                  result["reason_code"] == "roi_output_exceeded", "roi_output_budget_invalid")
        elif cid in ("malformed_source", "unsupported_source"):
            _need(names == ["process_observed", "input_opened", "refused", "probe_exit"],
                  "native_stage_set_incomplete")
        else:
            _need(names == ["process_observed", "input_opened", "metadata_available", "before_admission",
                            "admission_started", "refused", "probe_exit"], "native_stage_set_incomplete")
    _need(len(capability) == len(POSITIVE), "capability_row_missing")
    return {
        "ok": True, "fixture_count": len(fixtures), "worker_count": len(workers),
        "roi_pixels_verified": len(capability), "capability_rows": capability,
        "budget_boundary": boundary,
        "unmeasured": record["unmeasured"],
        "meaning": "Actual WIC rectangle output and refusals; rectangle output is not region decode.",
    }


def verify(record, source_root=None):
    try:
        return _verify(record, source_root)
    except (ValueError, KeyError, TypeError, IndexError, OSError, OverflowError) as exc:
        return {"ok": False, "error": str(exc)[:160]}


def dump(record, path):
    data = canonical(record)
    Path(path).write_bytes(data)
    return data
