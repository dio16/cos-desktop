"""Portable replay of retained W3 input/output bytes, accounting and lifecycles."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re

from w3_oracle import bmp565, compare
from w2_resource_verify import canonical, sha, STAGES

BOUNDS = {"max_cases": 24, "max_workers": 1, "max_worker_seconds": 45,
          "max_total_seconds": 300, "max_encoded_bytes": 1024 * 1024,
          "max_output_bytes": 16 * 1024 * 1024, "max_work_bytes": 64 * 1024 * 1024,
          "max_trace_records": 128, "max_trace_bytes": 128 * 1024,
          "max_capture_bytes": 64 * 1024, "max_report_bytes": 8 * 1024 * 1024}
POSITIVE = ("primaries", "midtones", "narrow_bottom", "odd_bottom", "odd_top")
FIXTURES = POSITIVE + ("truncated", "bad_masks", "overlap_masks", "bad_header", "inconsistent", "unsupported", "not_bgr565")
SOURCE_FILES = (
    "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "apps/desktop/examples/decode_compare/wic_admission.ps1",
    "apps/desktop/examples/decode_compare/harness.rs",
    "apps/desktop/examples/decode_compare/inspect.rs",
    "apps/desktop/examples/decode_compare/pixels.rs",
    "apps/desktop/examples/decode_compare.rs",
    "apps/desktop/devtools/img2/w3_fixtures.py",
    "apps/desktop/devtools/img2/w3_oracle.py",
    "apps/desktop/devtools/img2/w3_native_matrix.py",
    "apps/desktop/devtools/img2/w3_verify.py",
    "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "apps/desktop/devtools/img2/w2_resource_verify.py",
    "apps/desktop/devtools/img2/w2_independent_oracle.py",
    "crates/media/src/decode.rs", "Cargo.lock", "rust-toolchain.toml")


def scenarios(limits):
    rows = [{"id": "format_" + cid, "fixture": cid, "kind": "normal", "limits": dict(limits)} for cid in FIXTURES]
    for key, required in (("max_dimension", 3), ("max_pixels", 9), ("max_bit_depth", 6),
                          ("max_decoded_bytes", 18), ("max_scratch_bytes", 36)):
        for suffix, value in (("at", required), ("over", required - 1)):
            budget = dict(limits)
            budget[key] = value
            rows.append({"id": key + "_" + suffix, "fixture": "odd_bottom", "kind": "normal", "limits": budget})
    rows += [{"id": kind, "fixture": "odd_top", "kind": kind, "limits": dict(limits)} for kind in ("cancel", "timeout")]
    return rows


def _need(condition, code):
    if not condition:
        raise ValueError(code)


def _digest(value):
    return isinstance(value, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", value) is not None


def _int(value, minimum=0):
    return type(value) is int and value >= minimum


def _verify(r, source_root):
    _need(r["schema_version"] == 1 and r["complete"] is True, "incomplete_report")
    b = r["bounds"]
    _need(set(b) == set(BOUNDS) and all(_int(b[k], 1) and b[k] <= v for k, v in BOUNDS.items()), "bounds_invalid")
    _need(len(canonical(r)) <= b["max_report_bytes"], "report_cap")
    _need(_int(r["duration_ns"]) and r["duration_ns"] <= b["max_total_seconds"] * 1_000_000_000, "global_deadline")
    _need(_int(r["work_bytes"]) and r["work_bytes"] <= b["max_work_bytes"], "work_cap")
    files = r["source_identity"]["files"]
    _need(set(files) == set(SOURCE_FILES), "source_set_invalid")
    for name, item in files.items():
        _need(_digest(item["raw_sha256"]) and _digest(item["lf_sha256"]) and _int(item["bytes"], 1), "source_identity_invalid")
        if source_root:
            data = (Path(source_root) / name).read_bytes()
            _need(sha(data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")) == item["lf_sha256"], "source_digest_mismatch")
    for key in ("pwsh_sha256", "harness_sha256"):
        _need(_digest(r["source_identity"][key]), "executable_identity_invalid")
    fixtures = r["fixtures"]
    ids = [f["id"] for f in fixtures]
    _need(len(ids) == len(FIXTURES) and len(set(ids)) == len(ids) and set(ids) == set(FIXTURES), "fixture_set_invalid")
    inventory = {f["id"]: f for f in fixtures}
    _need(sum(f["bytes"] for f in fixtures) <= b["max_encoded_bytes"], "encoded_cap")
    for f in fixtures:
        data = bytes.fromhex(f["input_hex"])
        _need(len(data) == f["bytes"] and hashlib.sha256(data).hexdigest() == f["sha256"], "fixture_digest_mismatch")
        if f["id"] in POSITIVE:
            original = bmp565(data)
            for field in ("width", "height", "encoded_row_stride", "logical_row_bytes", "logical_source_bytes"):
                _need(original[field] == f[field], "fixture_accounting_mismatch")
            _need(original["row_order"].replace("_", "-") == f["row_order"], "fixture_row_order_mismatch")
            _need(original["rgba"].hex() == f["expected_rgba_hex"] and
                  hashlib.sha256(original["rgba"]).hexdigest() == f["expected_rgba_sha256"], "fixture_pixel_expectation_mismatch")
            required_size = {"primaries": (5, 1), "midtones": (7, 2),
                             "narrow_bottom": (1, 3), "odd_bottom": (3, 3), "odd_top": (3, 3)}
            _need((original["width"], original["height"]) == required_size[f["id"]] and
                  original["row_order"] == ("top_down" if f["id"] == "odd_top" else "bottom_up"),
                  "required_fixture_coverage_missing")
    limits = r["decode_limits"]
    _need(set(limits) == {"max_dimension", "max_pixels", "max_bit_depth", "max_decoded_bytes", "max_scratch_bytes"}
          and all(_int(v, 1) for v in limits.values()), "budget_invalid")
    consumer = r["consumer"]
    cr = consumer["record"]
    _need(consumer["exit_code"] == 0 and not cr["observations_truncated"] and not cr["cases_truncated"], "consumer_incomplete")
    _need(all(cr["decode_budget"][k] == v for k, v in limits.items()), "budget_source_mismatch")
    observations = cr["observations"]
    cids = [x["case_id"] for x in observations]
    _need(len(cids) == len(POSITIVE) and set(cids) == {x + ".bmp" for x in POSITIVE}, "consumer_case_set_invalid")
    for row in observations:
        f = inventory[Path(row["case_id"]).stem]
        _need(row["observed_outcome"] == "decoded" and row["output_present"] and row["output_bytes"] > 0
              and row["validity"] == "pixels_verified" and row["pixel_verified"] is True
              and row["pixel_digest"] == f["expected_rgba_sha256"], "consumer_pixel_mismatch")
    specs = {s["id"]: s for s in scenarios(limits)}
    workers = r["workers"]
    ids = [w["case_id"] for w in workers]
    _need(len(ids) == len(specs) <= b["max_cases"] and len(set(ids)) == len(ids) and set(ids) == set(specs), "worker_case_set_invalid")
    capability = []
    for w in workers:
        s = specs[w["case_id"]]
        f = inventory[s["fixture"]]
        _need(w["fixture_id"] == s["fixture"] and w["kind"] == s["kind"] and w["limits"] == s["limits"], "worker_spec_mismatch")
        _need(w["input_sha256"] == "sha256:" + f["sha256"] and
              w["exe_sha256"] == r["source_identity"]["pwsh_sha256"], "worker_identity_mismatch")
        _need(w["reaped"] is True and w["alive_after_wait"] is False, "worker_not_reaped")
        _need(_int(w["pid"], 1) and _int(w["start_utc_ticks"], 1), "worker_lifetime_missing")
        _need(0 <= w["started_ns"] <= w["exited_ns"] and
              w["exited_ns"] - w["started_ns"] <= (b["max_worker_seconds"] + 5) * 1_000_000_000, "worker_deadline")
        _need(w["clock_domain"] == "python_monotonic_ns" and
              w["cross_process_clock_subtraction"] is False, "cross_clock_comparison_invalid")
        sample = w["os_identity_sample"]
        if sample is not None:
            _need(sample["complete"] is True and len(sample["samples"]) == 1 and w["identity_verified"] is True,
                  "os_identity_sample_incomplete")
            observed = sample["samples"][0]
            _need(observed["pid"] == w["pid"] and observed["start_utc_ticks"] == w["start_utc_ticks"] ==
                  observed["identity_after_ticks"] and observed["exe_verified"] is True and
                  observed["alive_before"] is True and observed["alive_after"] is True,
                  "os_identity_mismatch")
        else:
            _need(s["fixture"] not in POSITIVE and s["fixture"] != "not_bgr565",
                  "os_identity_sample_missing")
        raw = w["trace_utf8"].encode("utf-8")
        stages = w["stages"]
        _need(raw.endswith(b"\n") and sha(raw) == w["trace_sha256"] and len(raw) == w["trace_bytes"] <= b["max_trace_bytes"], "trace_identity_invalid")
        _need([json.loads(x) for x in raw.splitlines()] == stages and 0 < len(stages) <= b["max_trace_records"], "trace_payload_invalid")
        previous, ticks, frequency = -1, -1, None
        for seq, stage in enumerate(stages, 1):
            if stage["stage"] == "refused":
                _need("admitted" not in [x["stage"] for x in stages] and
                      "onload_entered" not in [x["stage"] for x in stages], "materialization_after_refusal")
                rank = previous
            else:
                rank = STAGES.index(stage["stage"])
                _need(rank > previous, "trace_sequence_or_lifetime_invalid")
            _need(stage["seq"] == seq and stage["pid"] == w["pid"] and
                  stage["process_start_utc_ticks"] == w["start_utc_ticks"] and stage["qpc_ticks"] >= ticks,
                  "trace_sequence_or_lifetime_invalid")
            _need(stage["schema_version"] == 1 and stage["observer"] == "wic_probe" and
                  stage["clock_domain"] == "windows_qpc" and stage["qpc_frequency"] > 0 and
                  (frequency is None or frequency == stage["qpc_frequency"]), "trace_clock_invalid")
            previous, ticks, frequency = rank, stage["qpc_ticks"], stage["qpc_frequency"]
        names = [x["stage"] for x in stages]
        _need(len(names) == len(set(names)) and names[:2] == ["process_observed", "input_opened"],
              "native_stage_set_incomplete")
        _need(w["stdout_bytes"] <= b["max_capture_bytes"] and w["stderr_bytes"] <= b["max_capture_bytes"], "capture_cap")
        cancel = w["cancellation"]
        if s["kind"] in ("cancel", "timeout"):
            _need(cancel["state"] == "stop_confirmed" and cancel["hold_established"] is True and
                  cancel["cause"] == "boundary_hold" and cancel["kill_issued"] is True and cancel["reap_confirmed"] is True and
                  cancel["alive_at_request"] is True and cancel["exit_after_request"] not in (None, 0) and
                  stages[-1].get("artificial_instrumentation_hold") is True,
                  "stop_not_confirmed")
            _need(stages[-1]["stage"] == w["hold_stage"] and
                  w["started_ns"] <= cancel["requested_ns"] <= cancel["observed_ns"] == w["exited_ns"] and
                  cancel["exit_after_request"] == w["exit_code"], "stop_lifecycle_invalid")
            _need(names == list(STAGES[:STAGES.index(w["hold_stage"]) + 1]),
                  "native_stage_set_incomplete")
            if s["kind"] == "timeout":
                _need(cancel["reason"] == "timeout" and
                      cancel["requested_ns"] - w["started_ns"] >= 5_000_000_000, "timeout_not_elapsed")
            _need(w["outcome"] == ("cancelled" if s["kind"] == "cancel" else "timeout") and
                  not w["published_success"] and not w["output_present"] and w["output_hex"] is None,
                  "success_after_stop")
            continue
        _need(cancel["state"] == "not_requested" and w["trace_complete"] is True and
              names[-1] == "probe_exit" and stages[-1]["complete"] is True, "native_incomplete")
        result = w["probe_result"]
        _need(json.loads(w["stdout_utf8"]) == result, "stdout_result_mismatch")
        over = s["id"].endswith("_over")
        malformed = s["fixture"] not in POSITIVE and s["fixture"] != "not_bgr565"
        if over or malformed:
            allowed = ("oversize",) if over else ("corrupt", "unsupported")
            _need(w["outcome"] in allowed and result["outcome"] == w["outcome"] and
                  result["reason_code"] in ("invalid_metadata", "win_unknown_image_format", "win_bad_image",
                    "win_bad_header", "win_bad_stream_data", "max_dimension_exceeded", "max_pixels_exceeded",
                    "max_bit_depth_exceeded", "max_decoded_bytes_exceeded", "max_scratch_bytes_exceeded"), "refusal_class_invalid")
            if over:
                _need(result["reason_code"] == s["id"][:-5] + "_exceeded" and
                      result["admission_status"] == "refused", "budget_refusal_reason_invalid")
                _need(names == list(STAGES[:5]) + ["refused", "probe_exit"],
                      "native_stage_set_incomplete")
            _need("onload_entered" not in names and not w["output_present"] and not w["published_success"] and
                  w["output_hex"] is None and result["materialization"] == "not_requested", "output_after_refusal")
            continue
        _need(w["outcome"] == "decoded" and result["outcome"] == "decoded" and w["exit_code"] == 0 and
              result["admission_status"] == "admitted" and result["materialization"] == "completed", "decode_missing")
        _need(names == list(STAGES), "native_stage_set_incomplete")
        _need((result["width"], result["height"]) == (f["width"], f["height"]),
              "source_dimensions_invalid")
        _need(names.index("admitted") < names.index("onload_entered") < names.index("output_published"), "admission_order_invalid")
        actual = "Bgr555" if s["fixture"] == "not_bgr565" else "Bgr565"
        depth = 5 if actual == "Bgr555" else 6
        _need(result["pixel_format"] == actual and result["bits_per_channel"] == depth and result["bits_per_pixel"] == 16, "source_format_invalid")
        _need(result["row_bytes"] == f["logical_row_bytes"] and result["decoded_bytes_required"] == f["logical_source_bytes"] and
              result["bgra32_bytes_required"] == f["width"] * f["height"] * 4 and
              result["scratch_bytes_required"] == result["bgra32_bytes_required"] + result["declared_scratch_bytes"], "admission_accounting_invalid")
        admission = next(stage for stage in stages if stage["stage"] == "admitted")
        _need(all(admission[key] == result[key] for key in
                  ("width", "height", "bits_per_channel", "bits_per_pixel", "row_bytes",
                   "decoded_bytes_required", "bgra32_bytes_required", "scratch_bytes_required",
                   "declared_scratch_bytes", "input_bytes")), "trace_admission_mismatch")
        output = bytes.fromhex(w["output_hex"] or "")
        _need(w["output_present"] and w["published_success"] and 0 < len(output) == w["output_bytes"] <= b["max_output_bytes"]
              and sha(output) == w["output_sha256"], "decoded_output_missing")
        if actual == "Bgr565":
            oracle = compare(bytes.fromhex(f["input_hex"]), output)
            _need(oracle == w["oracle"] and oracle["ok"] is True, "pixel_mismatch")
            capability.append({"case_id": w["case_id"], "format": actual, "width": f["width"], "height": f["height"],
                               "depth": depth, "bpp": 16, "pixel_verified": True})
    _need(r["cleanup"] == {"complete": True, "live_direct_children": []}, "cleanup_incomplete")
    return {"ok": True, "fixture_count": len(fixtures), "worker_count": len(workers),
            "consumer_pixels_verified": len(observations), "capability_rows": capability,
            "unmeasured": r["unmeasured"], "meaning": "replay of retained raw Native bytes and observations; not a new WIC run"}


def verify(record, source_root=None):
    try:
        return _verify(record, source_root)
    except (ValueError, KeyError, TypeError, IndexError, OSError, OverflowError) as exc:
        return {"ok": False, "error": str(exc)[:160]}
