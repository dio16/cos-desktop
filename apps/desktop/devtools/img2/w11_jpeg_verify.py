"""Strict replay of the bounded W11 JPEG admission/ROI experiment.

A portable replay can validate observations, never establish Git source closure.
Separate held processes expose boundary counters; they are not allocation deltas.
"""
from __future__ import annotations

from functools import lru_cache
import json
import os
from pathlib import Path
import re
import subprocess

from w2_resource_verify import sha
from w6_memory_sample import SAMPLE_SCRIPT
from w7_large_verify import SOURCE_FILES as W7_SOURCE_FILES, _trace
from w11_jpeg_fixtures import FIXTURES, fixture_bytes
from w11_jpeg_oracle import compare_output, source_geometry, verify_fixture

BOUNDS = {
    "max_cases": 5, "max_workers": 1, "max_worker_seconds": 45,
    "max_total_seconds": 300, "max_encoded_input_bytes": 16 * 1024 * 1024,
    "max_roi_output_bytes": 262144, "max_output_bytes": 1024 * 1024,
    "max_work_bytes": 128 * 1024 * 1024, "max_report_bytes": 8 * 1024 * 1024,
    "max_trace_records": 128, "max_trace_bytes": 128 * 1024,
    "max_capture_bytes": 64 * 1024, "max_observed_private_bytes": 768 * 1024 * 1024,
    "max_worker_runs": 9,
}
LIMITS = {"max_dimension": 4096, "max_pixels": 16777216, "max_bit_depth": 8,
          "max_decoded_bytes": 67108864, "max_scratch_bytes": 67108864}
SCENARIOS = (
    {"id": "S01", "fixture_id": "jpeg_gray_4k", "roi": [17, 23, 1, 1],
     "max_dimension": 4096, "role": "decode"},
    {"id": "S02", "fixture_id": "jpeg_gray_4k", "roi": [117, 93, 256, 256],
     "max_dimension": 4096, "role": "decode"},
    {"id": "S03", "fixture_id": "jpeg_gray_16k", "roi": None,
     "max_dimension": 4096, "role": "refuse"},
    {"id": "S04", "fixture_id": "jpeg_gray_16k", "roi": [17, 23, 1, 1],
     "max_dimension": 4096, "role": "refuse"},
    {"id": "S05", "fixture_id": "jpeg_gray_4k", "roi": None,
     "max_dimension": 4095, "role": "refuse"},
)
HOLDS = ("before_onload", "after_onload", "before_publish")
SOURCE_FILES = tuple(dict.fromkeys((*W7_SOURCE_FILES,
    "apps/desktop/devtools/img2/w8_jpeg_fixtures.py",
    "apps/desktop/devtools/img2/w11_jpeg_fixtures.py",
    "apps/desktop/devtools/img2/w11_jpeg_oracle.py",
    "apps/desktop/devtools/img2/w11_jpeg_matrix.py",
    "apps/desktop/devtools/img2/w11_jpeg_verify.py",
    "apps/desktop/devtools/img2/tests/test_w11_jpeg_fixtures.py",
    "apps/desktop/devtools/img2/tests/test_w11_jpeg_negative.py",
)))
ALLOCATION_SCOPE = {
    "process_counters": "PROCESS_LEVEL_OBSERVATION",
    "boundary_comparison": "SEPARATE_PROCESS_LIFETIMES_NOT_ALLOCATION_DELTA",
    "atomic_snapshot": False,
    "decoder_internal_scratch_peak": "NOT_MEASURED",
    "allocator_reservations": "NOT_MEASURED",
    "true_concurrent_allocation_peak": "NOT_MEASURED",
    "wic_decoder_latency_ns": None,
    "boundary_hold": "ARTIFICIAL_INSTRUMENTATION_NOT_DECODER_TIME",
    "true_partial_decode": "NOT_VERIFIED",
    "gdi_16k": "NOT_RUN",
}
UNMEASURED = (
    "WIC internal scratch", "allocator reservations", "true concurrent allocation peak",
    "cooperative in-decoder cancellation", "product memory SLO", "16K full decode memory",
    "true partial/region/sequential/tiled decode", "zero metadata allocation",
)
DECODE_STAGES = (
    "process_observed", "input_opened", "metadata_available", "before_admission",
    "admission_started", "admitted", "before_onload", "onload_entered",
    "onload_returned", "after_onload", "conversion_entered", "conversion_returned",
    "copy_pixels_entered", "copy_pixels_returned", "encode_entered", "encode_returned",
    "before_publish", "output_published", "probe_exit",
)
REFUSAL_STAGES = ("process_observed", "input_opened", "metadata_available",
                  "before_admission", "admission_started", "refused", "probe_exit")
FORMATS = {"Gray8": 8, "Bgr24": 24, "Bgr32": 32, "Bgra32": 32, "Pbgra32": 32}


def need(condition, reason):
    if not condition:
        raise ValueError(reason)


def integer(value, minimum=0):
    return type(value) is int and value >= minimum


def digest(value):
    return isinstance(value, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", value) is not None


def _source(record, source_root):
    identity = record["source_identity"]
    commit = identity["source_commit"]
    need(isinstance(commit, str) and re.fullmatch(r"[0-9a-f]{40}", commit), "source_commit_invalid")
    files = identity["files"]
    need(isinstance(files, dict) and set(files) == set(SOURCE_FILES), "source_set_invalid")
    need(digest(identity["pwsh_sha256"]), "executable_identity_invalid")
    for name, row in files.items():
        need(integer(row["bytes"]) and row["unchanged_after_run"] is True, "source_changed:" + name)
        need(all(digest(row[k]) for k in ("raw_sha256", "lf_sha256", "git_blob_sha256")), "source_digest_invalid")
    if source_root is None:
        return False
    root = Path(source_root)
    git = os.environ.get("GIT_EXECUTABLE") or "git"
    for name, row in files.items():
        path = root / name
        need(not any(p.is_symlink() or p.is_junction() for p in (path, *path.parents)), "source_reparse")
        current = path.read_bytes().replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        blob = subprocess.run([git, "show", f"{commit}:{name}"], cwd=root,
                              capture_output=True, check=True, timeout=15).stdout
        need(sha(blob) == row["git_blob_sha256"], "source_blob_mismatch:" + name)
        need(sha(blob.replace(b"\r\n", b"\n").replace(b"\r", b"\n")) == row["lf_sha256"]
             == sha(current), "source_lf_mismatch:" + name)
    return True


@lru_cache(maxsize=2)
def _expected_fixture(fixture_id):
    data = fixture_bytes(fixture_id)
    need(0 < len(data) <= BOUNDS["max_encoded_input_bytes"], "fixture_cap")
    verify_fixture(fixture_id, data)
    return len(data), sha(data), source_geometry(data)


def _fixtures(record):
    rows = record["fixtures"]
    ids = [f["id"] for f in FIXTURES]
    need(isinstance(rows, list) and [r["case_id"] for r in rows] == ids, "fixture_order_or_set_invalid")
    result = {}
    for row, spec in zip(rows, FIXTURES):
        size, encoded_digest, geometry = _expected_fixture(spec["id"])
        need(row["bytes"] == size and row["sha256"] == encoded_digest, "fixture_identity_invalid")
        need((row["width"], row["height"], row["codec"]) ==
             (spec["width"], spec["height"], "jpeg"), "fixture_geometry_invalid")
        need(row["geometry"] == geometry, "independent_geometry_mismatch")
        need(row["path"] == spec["id"] + ".jpg", "fixture_path_invalid")
        need(isinstance(row["method"], str) and row["method"], "fixture_method_missing")
        result[spec["id"]] = row
    return result


def _requirements(result, fixture):
    fmt = result["pixel_format"]
    need(fmt in FORMATS, "unmodelled_actual_wic_format")
    width, height = fixture["width"], fixture["height"]
    bits = FORMATS[fmt]
    need(result["width"] == width and result["height"] == height
         and result["bits_per_channel"] == 8 and result["bits_per_pixel"] == bits,
         "source_format_or_geometry_contradiction")
    need(result["decoder"] == "JpegBitmapDecoder", "decoder_identity_invalid")
    row_bytes = (width * bits + 7) // 8
    return {"row_bytes": row_bytes, "decoded_bytes_required": row_bytes * height,
            "bgra32_bytes_required": width * height * 4,
            "scratch_bytes_required": width * height * 4, "declared_scratch_bytes": 0,
            "input_bytes": fixture["bytes"]}


def _memory(worker, hold, executable_sha):
    sample = worker["sample"]
    need(sample["index"] == 0 and sample["case_ids"] == [worker["case_id"]], "sample_case_invalid")
    start, end = sample["start_ns"], sample["end_ns"]
    need(integer(start, worker["started_ns"]) and integer(end, start)
         and end <= worker["exited_ns"] and end - start <= 6_000_000_000
         and sample["gap_ns"] is None and sample["clock_domain"] == "python_monotonic_ns", "sample_time_invalid")
    need(sample["classification"] == "DERIVED_WINDOW_SUM" and sample["atomic_snapshot"] is False
         and sample["aggregation"] == "same_collection_window", "sample_false_peak_claim")
    need(sample["complete"] is True and sample["status"] == "complete" and sample["failure"] is None,
         "sample_incomplete")
    observer = sample["observer"]
    need(observer["exe_sha256"] == executable_sha and observer["script_sha256"] == sha(SAMPLE_SCRIPT.encode()),
         "observer_identity_invalid")
    need(observer["exit_code"] == 0 and observer["timeout"] is False
         and observer["capture_complete"] is True, "observer_incomplete")
    for field in ("stdout", "stderr"):
        raw = observer[field + "_utf8"]
        need(isinstance(raw, str) and len(raw) <= BOUNDS["max_capture_bytes"], "observer_capture_invalid")
        need(len(raw.encode()) == observer[field + "_bytes"] <= BOUNDS["max_capture_bytes"], "observer_capture_invalid")
    native = json.loads(observer["stdout_utf8"])
    need(native["errors"] == [] and native["samples"] == sample["samples"], "observer_payload_invalid")
    for k in ("qpc_start", "qpc_end", "qpc_frequency"):
        need(native[k] == sample[k] and integer(sample[k], 1), "sample_qpc_invalid")
    need(sample["qpc_end"] >= sample["qpc_start"], "sample_qpc_reversed")
    expected_id = {"case_id": worker["case_id"], "pid": worker["pid"], "start_utc_ticks": worker["start_utc_ticks"]}
    need(sample["request_identities"] == [expected_id] and len(sample["samples"]) == 1, "sample_identity_invalid")
    counter = sample["samples"][0]
    need(all(counter[k] == v for k, v in expected_id.items()) and counter["identity_after_ticks"] == worker["start_utc_ticks"]
         and counter["order"] == 0 and counter["exe_verified"] is True
         and counter["alive_before"] is True and counter["alive_after"] is True, "sample_lifetime_invalid")
    stages = worker["stages"]
    index = [r["stage"] for r in stages].index(hold)
    parked = stages[index]
    need(parked.get("artificial_instrumentation_hold") is True
         and parked["qpc_frequency"] == sample["qpc_frequency"]
         and parked["qpc_ticks"] <= sample["qpc_start"] <= counter["qpc_start"]
         <= counter["qpc_end"] <= sample["qpc_end"] <= stages[index + 1]["qpc_ticks"], "sample_outside_hold")
    need(all(integer(counter[k], 1) for k in ("working_set_bytes", "private_bytes")), "sample_counter_invalid")
    totals = {k: counter[k] for k in ("working_set_bytes", "private_bytes")}
    need(sample["totals"] == totals and totals["private_bytes"] <= BOUNDS["max_observed_private_bytes"], "sample_totals_invalid")
    peak = counter["peak_working_set_bytes"]
    need((counter["peak_status"] == "NOT_MEASURED" and peak is None) or
         (counter["peak_status"] == "MEASURED_PROCESS_LIFETIME_COUNTER" and integer(peak, counter["working_set_bytes"])),
         "lifetime_peak_invalid")


def _worker(worker, scenario, fixture, hold, identity):
    cid = scenario["id"] + "_" + hold
    need(worker["case_id"] == cid and worker["fixture_id"] == scenario["fixture_id"]
         and worker["hold_stage"] == hold, "worker_case_invalid")
    need(worker["kind"] == ("decode" if scenario["role"] == "decode" else "oversize"), "worker_kind_invalid")
    limits = dict(LIMITS, max_dimension=scenario["max_dimension"])
    need(worker["limits"] == limits and worker["input_sha256"] == fixture["sha256"]
         and worker["exe_sha256"] == identity["pwsh_sha256"], "worker_binding_invalid")
    need(integer(worker["pid"], 1) and integer(worker["start_utc_ticks"], 1), "worker_identity_invalid")
    need(worker["reaped"] is True and worker["alive_after_wait"] is False
         and worker["identity_verified"] is True, "worker_not_reaped_or_identified")
    need(integer(worker["started_ns"], 1) and integer(worker["exited_ns"], worker["started_ns"])
         and worker["exited_ns"] - worker["started_ns"] <= BOUNDS["max_worker_seconds"] * 1_000_000_000,
         "worker_deadline_invalid")
    need(worker["clock_domain"] == "python_monotonic_ns" and worker["cross_process_clock_subtraction"] is False
         and worker["boundary_hold_is_instrumentation"] is True, "worker_clock_or_hold_claim_invalid")
    need(worker["cancellation"]["state"] == "not_requested" and worker["removed_owned_output_paths"] == [], "unexpected_stop")
    cancellation = worker["cancellation"]
    need(cancellation["requested_ns"] is None and cancellation["observed_ns"] is None
         and cancellation["method"] is None and cancellation["kill_issued"] is False
         and cancellation["kill_error"] is None and cancellation["reap_confirmed"] is False
         and cancellation["exit_after_request"] is None, "unrequested_stop_contradiction")
    stages, _ = _trace(worker)
    if not isinstance(stages, list):
        raise ValueError("trace_stage_list_missing")
    names = [s["stage"] for s in stages]
    expected = DECODE_STAGES if scenario["role"] == "decode" else REFUSAL_STAGES
    need(names == list(expected) and worker["trace_complete"] is True and worker["trace_partial_bytes"] == 0,
         "stage_order_or_completion_invalid")
    need(worker["trace_path"].replace("\\", "/") == f"workers/{cid}/observe/trace.jsonl"
         and worker["output_path"].replace("\\", "/") == f"workers/{cid}/output.png", "foreign_worker_path")
    for field in ("stdout", "stderr"):
        text = worker[field + "_utf8"]
        need(isinstance(text, str) and len(text) <= BOUNDS["max_capture_bytes"]
             and len(text.encode()) == worker[field + "_bytes"] <= BOUNDS["max_capture_bytes"], "capture_size_invalid")
    result = worker["probe_result"]
    need(isinstance(result, dict) and json.loads(worker["stdout_utf8"]) == result, "probe_capture_mismatch")
    terminal = stages[-1]
    need(terminal["complete"] is True and all(terminal[k] == result[k] for k in
         ("outcome", "reason_code", "admission_status", "materialization")), "terminal_result_contradiction")
    _memory(worker, hold, identity["pwsh_sha256"])
    requirements = _requirements(result, fixture)
    decode = scenario["role"] == "decode"
    for k, value in requirements.items():
        if decode or result.get(k) is not None:
            need(result.get(k) == value, "admission_requirement_invalid:" + k)
    roi = scenario["roi"]
    need(result.get("roi_requested") in ((True,) if roi is not None else (None, False)), "roi_request_invalid")
    if roi is not None:
        width, height = roi[2:]
        need([result.get("roi_" + k) for k in ("x", "y", "width", "height")] == roi
             and result["roi_output_bytes_required"] == width * height * 4 <= BOUNDS["max_roi_output_bytes"]
             and result["full_frame_decoded_bytes"] == requirements["decoded_bytes_required"], "roi_or_full_frame_charge_invalid")
        need(result["true_partial_decode"] == "not_verified" and result["sequential_decode"] == "not_verified"
             and result["tiled_decode"] == "not_verified", "false_partial_decode_claim")
    if decode:
        if roi is None:
            raise ValueError("decode_roi_missing")
        need(worker["exit_code"] == 0 and worker["outcome"] == result["outcome"] == "decoded"
             and result["admission_status"] == "admitted" and result["materialization"] == "completed", "decode_not_completed")
        admitted = stages[names.index("admitted")]
        need(admitted["bits_per_channel"] == 8 and admitted["bits_per_pixel"] == FORMATS[result["pixel_format"]],
             "admitted_format_contradiction")
        for key, value in requirements.items():
            need(admitted.get(key) == value, "admitted_requirement_contradiction:" + key)
        need(admitted.get("width") == fixture["width"] and admitted.get("height") == fixture["height"],
             "admitted_geometry_contradiction")
        need(result["decoder_cache_mode"] == "OnLoad" and result["full_frame_materialization"] == "observed"
             and result["roi_output"] == "produced_pending_publish", "roi_capability_invalid")
        need(worker["output_present"] is True and worker["published_success"] is True, "output_missing")
        encoded = worker["output_hex"]
        need(isinstance(encoded, str) and 0 < len(encoded) <= BOUNDS["max_output_bytes"] * 2, "output_hex_cap")
        data = bytes.fromhex(encoded)
        need(len(data) == worker["output_bytes"] <= BOUNDS["max_output_bytes"]
             and sha(data) == worker["output_sha256"], "output_identity_invalid")
        oracle = compare_output(fixture_bytes(scenario["fixture_id"]), data, roi)
        need(oracle["ok"] is True and oracle == worker["oracle"], "roi_pixel_oracle_failed")
        return len(data)
    need(worker["exit_code"] == 4 and worker["outcome"] == result["outcome"] == "oversize"
         and result["reason_code"] == "max_dimension_exceeded" and result["admission_status"] == "refused"
         and result["materialization"] == "not_requested", "pre_onload_refusal_invalid")
    need(max(fixture["width"], fixture["height"]) > limits["max_dimension"], "refusal_not_required")
    need(worker["output_present"] is False and worker["published_success"] is False
         and worker["output_hex"] is None and worker["output_bytes"] == 0
         and worker["output_sha256"] is None and worker["oracle"] is None, "refusal_published_output")
    if roi is not None:
        need(result["roi_output"] == "not_produced" and result["full_frame_materialization"] == "not_observed",
             "refusal_roi_materialized")
    return 0


def verify(record, source_root=None):
    """Validate every required observation; only actual Git binding grants closure."""
    try:
        need(isinstance(record, dict) and record["schema_version"] == 1 and record["complete"] is True, "run_incomplete")
        need(record["bounds"] == BOUNDS and record["decode_limits"] == LIMITS, "fixed_bounds_invalid")
        need(record["allocation_scope"] == ALLOCATION_SCOPE and record["unmeasured"] == list(UNMEASURED), "false_measurement_claim")
        need(record["cleanup"] == {"complete": True, "live_direct_children": []}
             and record["work_root_disposable"] is True, "cleanup_incomplete")
        need(integer(record["duration_ns"]) and record["duration_ns"] <= BOUNDS["max_total_seconds"] * 1_000_000_000
             and integer(record["work_bytes"]) and record["work_bytes"] <= BOUNDS["max_work_bytes"], "run_bound_invalid")
        need(record["sampler_script_sha256"] == sha(SAMPLE_SCRIPT.encode()), "sampler_source_invalid")
        platform = record["platform"]
        need(platform["windows"] is True and all(isinstance(platform[k], str) and platform[k] for k in
             ("powershell", "dotnet", "os")), "native_platform_invalid")
        fixtures = _fixtures(record)
        source_verified = _source(record, source_root)
        rows = record["cases"]
        need(isinstance(rows, list) and [r["case_id"] for r in rows] == [s["id"] for s in SCENARIOS], "scenario_order_invalid")
        seen = set()
        previous_end = 0
        count = 0
        for row, scenario in zip(rows, SCENARIOS):
            need(row["fixture_id"] == scenario["fixture_id"] and row["role"] == scenario["role"]
                 and row["roi"] == scenario["roi"] and row["failure"] is None, "scenario_identity_invalid")
            need(row["limits"] == dict(LIMITS, max_dimension=scenario["max_dimension"]), "scenario_budget_invalid")
            decode = scenario["role"] == "decode"
            need(row["classification"] == ("ROI_OUTPUT_VERIFIED" if decode else "REFUSED_FULL_FRAME"), "classification_invalid")
            holds = HOLDS if decode else ("before_admission",)
            workers = row["workers"]
            need(isinstance(workers, list) and len(workers) == len(holds), "worker_run_set_invalid")
            for worker, hold in zip(workers, holds):
                _worker(worker, scenario, fixtures[scenario["fixture_id"]], hold, record["source_identity"])
                life = worker["pid"], worker["start_utc_ticks"]
                need(life not in seen, "worker_lifetime_reused")
                seen.add(life)
                need(worker["started_ns"] >= previous_end, "simultaneous_workers")
                previous_end = worker["exited_ns"]
                count += 1
        need(count == BOUNDS["max_worker_runs"], "worker_run_count_invalid")
        return {"ok": True, "closure_eligible": source_verified, "source_identity_verified": source_verified,
                "cases": len(rows), "workers": count, "roi_outputs_verified": 6, "refused_before_onload": 3,
                "true_partial_decode": "NOT_VERIFIED", "16k_full_decode_memory": "NOT_MEASURED",
                "gdi_16k": "NOT_RUN", "memory_comparison": "SEPARATE_PROCESS_LIFETIMES"}
    except (OSError, ValueError, KeyError, TypeError, UnicodeError, subprocess.SubprocessError) as exc:
        return {"ok": False, "closure_eligible": False, "source_identity_verified": False,
                "reason": type(exc).__name__ + ":" + str(exc)[:240]}
