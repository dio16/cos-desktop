"""Independent replay of W6 process-counter windows, not allocator measurements."""
from __future__ import annotations

import json
from pathlib import Path

from w2_resource_verify import BOUNDS as W2_BOUNDS, STAGES, canonical, sha
from w6_memory_fixtures import compare, fixture_bytes, manifest, source_rgba
from w6_memory_sample import SAMPLE_SCRIPT

BOUNDS = dict(W2_BOUNDS)
LIMITS = {"max_dimension": 1024, "max_pixels": 1048576, "max_bit_depth": 8,
          "max_decoded_bytes": 4194304, "max_scratch_bytes": 8388608}
SOURCE_FILES = (
    "apps/desktop/devtools/img2/w6_memory_fixtures.py",
    "apps/desktop/devtools/img2/w6_memory_sample.py",
    "apps/desktop/devtools/img2/w6_memory_matrix.py",
    "apps/desktop/devtools/img2/w6_memory_verify.py",
    "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "apps/desktop/devtools/img2/w2_resource_verify.py",
    "apps/desktop/devtools/img2/w2_independent_oracle.py",
    "apps/desktop/devtools/img2/w3_oracle.py",
    "apps/desktop/devtools/img2/w4_region_oracle.py",
    "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "apps/desktop/examples/decode_compare/wic_admission.ps1",
    "apps/desktop/examples/decode_compare.rs",
    "apps/desktop/examples/decode_compare/harness.rs",
    "apps/desktop/examples/decode_compare/inspect.rs",
    "apps/desktop/examples/decode_compare/pixels.rs",
    "crates/media/src/decode.rs", "Cargo.lock", "rust-toolchain.toml",
)
COUNTERS = ("working_set_bytes", "private_bytes")
ALLOCATION_SCOPE = {
    "window_sum": "DERIVED_WINDOW_SUM",
    "maximum": "MAX_OBSERVED_WINDOW_SUM",
    "atomic_snapshot": False,
    "decoder_internal_scratch_peak": "NOT_MEASURED",
    "explicit_known_reservations_bytes": "NOT_MEASURED",
    "actually_proven_peak_concurrent_allocation": "NOT_MEASURED",
    "wic_decoder_latency_ns": None,
    "boundary_hold": "ARTIFICIAL_INSTRUMENTATION_NOT_DECODER_TIME",
    "lifetime_peak": "PROCESS_LIFETIME_OS_COUNTER_NOT_CONCURRENT_ALLOCATION",
}


def groups():
    result = []
    for size in (256, 1024):
        for codec in ("png", "bmp24", "bgr565"):
            fid = f"{codec}_{size}"
            result.append({"id": "single_" + fid, "fixtures": [fid],
                           "boundary": "after_onload", "action": "release", "windows": 2})
    for label, size, boundary, actions in (
        ("small", 256, "after_onload", ("release", "release")),
        ("large", 1024, "after_onload", ("release", "release")),
        ("before", 256, "before_onload", ("cancel", "timeout")),
    ):
        for repeat, action in enumerate(actions, 1):
            result.append({"id": f"cohort_{label}_{repeat}",
                           "fixtures": [f"{c}_{size}" for c in ("png", "bmp24", "bgr565")],
                           "boundary": boundary, "action": action, "windows": 3})
    return result


def need(condition, error):
    if not condition:
        raise ValueError(error)


def integer(value, minimum=0):
    return type(value) is int and value >= minimum


def requirements(fixtures):
    """Known logical requirements; conversion bytes are counted exactly once."""
    result = {k: sum(f[k] for f in fixtures) for k in
              ("bytes", "decoded_bytes_required", "bgra32_bytes_required", "declared_scratch_bytes")}
    return {"encoded_input_bytes": result["bytes"],
            "decoded_bytes_required": result["decoded_bytes_required"],
            "BGRA32_bytes_required": result["bgra32_bytes_required"],
            "declared_scratch_bytes": result["declared_scratch_bytes"],
            "sum_of_known_requirements": sum(result.values()),
            "classification": "LOGICAL_REQUIREMENTS_NOT_RESERVATIONS"}


def window_metrics(windows):
    complete = [w for w in windows if w["complete"]]
    return {"complete_window_count": len(complete),
            "incomplete_window_count": len(windows) - len(complete),
            "max_observed_window_working_set": max((w["totals"]["working_set_bytes"] for w in complete), default=None),
            "max_observed_window_private": max((w["totals"]["private_bytes"] for w in complete), default=None),
            "classification": "MAX_OBSERVED_WINDOW_SUM"}


def comparison_rows(experiments, fixtures):
    inventory = {f["id"]: f for f in fixtures}
    rows = []
    for group in experiments:
        for worker in group["workers"]:
            fixture = inventory[worker["fixture_id"]]
            samples = [s for w in group["windows"] if w["complete"]
                       for s in w["samples"] if s["case_id"] == worker["case_id"]]
            rows.append({"group_id": group["id"], "case_id": worker["case_id"],
                         "codec": fixture["codec"], "size": fixture["width"],
                         "workers": len(group["workers"]), "boundary": group["boundary"],
                         "max_sampled_working_set": max((s["working_set_bytes"] for s in samples), default=None),
                         "max_sampled_private": max((s["private_bytes"] for s in samples), default=None),
                         "scope": "MAX_OBSERVED_PROCESS_SAMPLE_NOT_ALLOCATION_PEAK"})
    return rows


def matched_differences(experiments):
    inventory = {g["id"]: g for g in experiments}
    result = []
    for repeat in (1, 2):
        before, after = (inventory[f"cohort_{name}_{repeat}"] for name in ("before", "small"))
        metrics = {}
        for field in ("max_observed_window_working_set", "max_observed_window_private"):
            a, b = after["metrics"][field], before["metrics"][field]
            metrics[field] = a - b if a is not None and b is not None else None
        result.append({"before_group": before["id"], "after_group": after["id"],
                       "classification": "MATCHED_RUN_DIFFERENCE", "same_processes": False,
                       "same_instant": False, "difference": metrics})
    return result


def _worker(w, fixture, spec, executable_sha):
    kind = "cohort" if len(spec["fixtures"]) > 1 else "success"
    if spec["action"] != "release":
        kind = spec["action"]
    need((w["case_id"], w["fixture_id"], w["kind"], w["hold_stage"]) ==
         (spec["id"] + "_" + fixture["id"], fixture["id"], kind, spec["boundary"]), "worker_spec_mismatch")
    need(integer(w["pid"], 1) and integer(w["start_utc_ticks"], 1)
         and w["exe_sha256"] == executable_sha
         and w["input_sha256"] == "sha256:" + fixture["sha256"], "worker_identity_mismatch")
    need(w["reaped"] is True and w["alive_after_wait"] is False, "worker_not_reaped")
    need(w["clock_domain"] == "python_monotonic_ns" and w["cross_process_clock_subtraction"] is False,
         "worker_clock_invalid")
    need(integer(w["started_ns"], 1) and integer(w["exited_ns"], w["started_ns"])
         and w["exited_ns"] - w["started_ns"] <= 51_000_000_000, "worker_lifetime_invalid")
    raw = w["trace_utf8"].encode("utf-8")
    prefix = raw[:raw.rfind(b"\n") + 1]
    stages = w["stages"]
    need(sha(raw) == w["trace_sha256"] and len(raw) == w["trace_bytes"] <= BOUNDS["max_trace_bytes"]
         and len(stages) == w["trace_records"] <= BOUNDS["max_trace_records"]
         and len(raw) - len(prefix) == w["trace_partial_bytes"]
         and [json.loads(line) for line in prefix.splitlines()] == stages, "trace_payload_mismatch")
    expected_names = list(STAGES) if spec["action"] == "release" else list(STAGES[:STAGES.index(spec["boundary"]) + 1])
    need([s["stage"] for s in stages] == expected_names, "stage_population_invalid")
    tick = -1
    frequency = stages[0]["qpc_frequency"]
    for index, stage in enumerate(stages, 1):
        need(stage["schema_version"] == 1 and stage["seq"] == index
             and stage["clock_domain"] == "windows_qpc" and stage["observer"] == "wic_probe"
             and (stage["pid"], stage["process_start_utc_ticks"]) == (w["pid"], w["start_utc_ticks"]),
             "stage_identity_invalid")
        need(integer(stage["qpc_ticks"], tick) and integer(frequency, 1)
             and stage["qpc_frequency"] == frequency, "stage_clock_invalid")
        tick = stage["qpc_ticks"]
    hold = stages[STAGES.index(spec["boundary"])]
    need(hold.get("artificial_instrumentation_hold") is True, "hold_evidence_missing")
    admission = next(s for s in stages if s["stage"] == "admitted")
    values = {"width": fixture["width"], "height": fixture["height"],
              "bits_per_channel": fixture["bits_per_channel"], "bits_per_pixel": fixture["bits_per_pixel"],
              "row_bytes": fixture["logical_row_bytes"], "decoded_bytes_required": fixture["decoded_bytes_required"],
              "bgra32_bytes_required": fixture["bgra32_bytes_required"],
              "declared_scratch_bytes": 0, "scratch_bytes_required": fixture["scratch_bytes_required"],
              "input_bytes": fixture["bytes"]}
    need(all(type(admission.get(k)) is int and admission[k] == v for k, v in values.items()), "admission_arithmetic_invalid")
    need(w["limits"] == LIMITS, "worker_limits_changed")
    need(w["stdout_bytes"] == len(w["stdout_utf8"].encode("utf-8")) <= BOUNDS["max_capture_bytes"]
         and integer(w["stderr_bytes"]) and w["stderr_bytes"] <= BOUNDS["max_capture_bytes"], "worker_capture_invalid")
    result = w["probe_result"]
    need((json.loads(w["stdout_utf8"]) if w["stdout_utf8"] else None) == result, "worker_stdout_mismatch")
    stop = w["cancellation"]
    if spec["action"] == "release":
        need(w["outcome"] == "decoded" and w["exit_code"] == 0 and w["trace_complete"] is True
             and w["published_success"] is True and w["output_present"] is True, "decode_publication_invalid")
        need(stop["state"] == "not_requested" and stop["requested_ns"] is None
             and stop["kill_issued"] is False and stop["cause"] == "not_requested", "unexpected_stop")
        need(isinstance(result, dict) and result["outcome"] == "decoded"
             and result["materialization"] == "completed" and result["admission_status"] == "admitted",
             "probe_outcome_invalid")
        decoder = "PngBitmapDecoder" if fixture["codec"] == "png" else "BmpBitmapDecoder"
        need(result["pixel_format"] == fixture["pixel_format"] and result["decoder"] == decoder
             and all(result.get(k) == v for k, v in values.items()), "source_format_or_requirements_invalid")
        terminal = stages[-1]
        need(terminal.get("complete") is True and all(terminal.get(k) == result.get(k) for k in
             ("outcome", "reason_code", "admission_status", "materialization", "output_bytes")), "terminal_result_mismatch")
        output = bytes.fromhex(w["output_hex"])
        need(0 < len(output) == w["output_bytes"] == result["output_bytes"] <= BOUNDS["max_output_bytes"]
             and sha(output) == w["output_sha256"], "output_payload_invalid")
        oracle = compare(fixture, fixture_bytes(fixture["id"]), output)
        need(oracle["ok"] is True and oracle == w["oracle"], "pixel_oracle_mismatch")
    else:
        outcome = "cancelled" if spec["action"] == "cancel" else "timeout"
        need(w["outcome"] == outcome and w["trace_complete"] is False
             and stop["state"] == "stop_confirmed", "required_stop_unconfirmed")
        need(stop["alive_at_request"] is True and stop["exit_before_request"] is False
             and stop["returncode_before_request"] is None and stop["hold_established"] is True
             and stop["cause"] == "boundary_hold" and stop["kill_issued"] is True
             and stop["kill_error"] is None and stop["waited"] is True and stop["reap_confirmed"] is True
             and stop["method"] == "direct_child_terminate_wait" and stop["reachability"] == "reached"
             and stop["last_observed_stage"] == spec["boundary"] and stop["reason"] == spec["action"]
             and w["started_ns"] <= stop["requested_ns"] <= stop["observed_ns"] == w["exited_ns"]
             and stop["exit_after_request"] == w["exit_code"] != 0, "stop_lifecycle_invalid")
        need(result is None and w["published_success"] is False and w["output_present"] is False
             and w["output_bytes"] == 0 and w["output_hex"] is None and w["output_sha256"] is None
             and w["oracle"] is None, "output_after_stop")
    return hold, frequency


def _window(window, workers, index, previous_end, script_sha, executable_sha, holds):
    need(window["index"] == index and window["case_ids"] == [w["case_id"] for w in workers], "window_identity_invalid")
    start, end = window["start_ns"], window["end_ns"]
    need(integer(start, previous_end or 1) and integer(end, start)
         and end - start <= 6_000_000_000 and window["clock_domain"] == "python_monotonic_ns"
         and window["gap_ns"] == (start - previous_end if previous_end is not None else None), "window_time_or_gap_invalid")
    need(window["classification"] == "DERIVED_WINDOW_SUM" and window["atomic_snapshot"] is False
         and window["aggregation"] == "same_collection_window", "false_peak_claim")
    observer = window["observer"]
    need(observer["exe_sha256"] == executable_sha and observer["script_sha256"] == script_sha, "observer_identity_invalid")
    for field in ("stdout", "stderr"):
        need(observer[field + "_bytes"] == len(observer[field + "_utf8"].encode("utf-8"))
             <= BOUNDS["max_capture_bytes"], "observer_capture_invalid")
    need(window["request_identities"] == [{"case_id": w["case_id"], "pid": w["pid"],
         "start_utc_ticks": w["start_utc_ticks"]} for w in workers], "sample_request_identity_invalid")
    if not window["complete"]:
        need(window["status"] == "incomplete" and isinstance(window["failure"], str)
             and window["failure"] and window["totals"] is None, "incomplete_window_misrepresented")
        return end
    need(window["complete"] is True and window["status"] == "complete" and window["failure"] is None
         and observer["exit_code"] == 0 and observer["timeout"] is False
         and observer["capture_complete"] is True, "sampler_not_complete")
    native = json.loads(observer["stdout_utf8"])
    need(native["errors"] == [] and native["samples"] == window["samples"]
         and all(native[k] == window[k] for k in ("qpc_start", "qpc_end", "qpc_frequency")),
         "sampler_payload_mismatch")
    need(integer(window["qpc_start"], 1) and integer(window["qpc_end"], window["qpc_start"])
         and integer(window["qpc_frequency"], 1), "sampler_clock_invalid")
    samples = window["samples"]
    need(len(samples) == len(workers), "window_worker_missing")
    tick = window["qpc_start"]
    for order, (sample, worker, (hold, frequency)) in enumerate(zip(samples, workers, holds)):
        need((sample["case_id"], sample["pid"], sample["start_utc_ticks"], sample["identity_after_ticks"], sample["order"]) ==
             (worker["case_id"], worker["pid"], worker["start_utc_ticks"], worker["start_utc_ticks"], order), "sample_identity_or_order_invalid")
        need(sample["exe_verified"] is True and sample["alive_before"] is True and sample["alive_after"] is True
             and worker["started_ns"] <= start <= end <= worker["exited_ns"], "sample_lifetime_invalid")
        need(frequency == window["qpc_frequency"] and hold["qpc_ticks"] <= window["qpc_start"]
             and integer(sample["qpc_start"], tick) and integer(sample["qpc_end"], sample["qpc_start"])
             and sample["qpc_end"] <= window["qpc_end"], "sample_clock_or_boundary_invalid")
        tick = sample["qpc_end"]
        if worker["trace_complete"]:
            after_hold = worker["stages"][STAGES.index(worker["hold_stage"]) + 1]
            need(window["qpc_end"] <= after_hold["qpc_ticks"], "sample_outside_hold")
        need(all(integer(sample[k], 1) for k in COUNTERS), "sample_counter_invalid")
        peak = sample["peak_working_set_bytes"]
        need((sample["peak_status"] == "NOT_MEASURED" and peak is None) or
             (sample["peak_status"] == "MEASURED_PROCESS_LIFETIME_COUNTER"
              and integer(peak, sample["working_set_bytes"])), "lifetime_peak_invalid")
    totals = {k: sum(s[k] for s in samples) for k in COUNTERS}
    need(window["totals"] == totals and totals["private_bytes"] <= BOUNDS["max_observed_private_bytes"], "window_totals_or_bound_invalid")
    return end


def verify(record, source_root=None):
    try:
        return _verify(record, Path(source_root) if source_root is not None else None)
    except (ValueError, KeyError, TypeError, IndexError, OSError, OverflowError) as exc:
        return {"ok": False, "error": str(exc)[:200], "closure_eligible": False}


def _verify(record, source):
    need(len(canonical(record)) <= BOUNDS["max_report_bytes"], "report_cap_exceeded")
    need(record["schema_version"] == 1 and record["complete"] is True
         and record["bounds"] == BOUNDS and record["decode_limits"] == LIMITS, "record_bounds_invalid")
    need(record["allocation_scope"] == ALLOCATION_SCOPE, "allocation_scope_escalation")
    need(record["sampler_script_sha256"] == sha(SAMPLE_SCRIPT.encode("utf-8")), "sampler_script_identity_invalid")
    need(record["cleanup"] == {"complete": True, "live_direct_children": []}
         and record["work_root_disposable"] is True and integer(record["work_bytes"])
         and record["work_bytes"] <= BOUNDS["max_work_bytes"]
         and integer(record["duration_ns"], 1) and record["duration_ns"] <= BOUNDS["max_total_seconds"] * 1_000_000_000,
         "run_resource_or_cleanup_invalid")
    identity = record["source_identity"]
    need(set(identity["files"]) == set(SOURCE_FILES) and len(identity["source_commit"]) == 40,
         "source_set_invalid")
    for path in SOURCE_FILES:
        item = identity["files"][path]
        need(item["unchanged_after_run"] is True and integer(item["bytes"], 1), "source_changed")
        if source:
            data = (source / path).read_bytes()
            need(sha(data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")) == item["lf_sha256"], "source_digest_mismatch")
    expected = manifest()
    need(record["fixtures"] == expected, "fixture_set_or_digest_invalid")
    inventory = {f["id"]: f for f in expected}
    for fixture in expected:
        data = fixture_bytes(fixture["id"])
        parsed = source_rgba(fixture, data)
        need(len(data) == fixture["bytes"] and sha(data) == "sha256:" + fixture["sha256"]
             and sha(parsed["rgba"]) == "sha256:" + fixture["expected_rgba_sha256"], "fixture_oracle_invalid")
        need(all(parsed[k] == fixture[k] for k in
             ("width", "height", "pixel_format", "bits_per_channel", "bits_per_pixel")), "fixture_format_invalid")
        row = (parsed["width"] * parsed["bits_per_pixel"] + 7) // 8
        need(fixture["logical_row_bytes"] == row and fixture["decoded_bytes_required"] == row * parsed["height"]
             and fixture["bgra32_bytes_required"] == parsed["width"] * parsed["height"] * 4
             and fixture["scratch_bytes_required"] == fixture["bgra32_bytes_required"] + fixture["declared_scratch_bytes"],
             "fixture_requirements_invalid")
    specs, experiments = groups(), record["experiments"]
    need(len(experiments) == len(specs) and [g["id"] for g in experiments] == [s["id"] for s in specs], "experiment_set_invalid")
    need(sum(len(g["workers"]) for g in experiments) == 24 <= BOUNDS["max_cases"]
         and sum(len(g["windows"]) for g in experiments) <= BOUNDS["max_samples"], "worker_or_window_cap_exceeded")
    end = 0
    eligible = True
    for group, spec in zip(experiments, specs):
        workers = group["workers"]
        need(group["boundary"] == spec["boundary"] and group["action"] == spec["action"]
             and len(workers) == len(spec["fixtures"]) <= BOUNDS["max_workers"]
             and len(group["windows"]) == spec["windows"], "group_scope_or_window_missing")
        need(integer(group["start_ns"], end) and integer(group["end_ns"], group["start_ns"]), "group_overlap_or_clock_invalid")
        end = group["end_ns"]
        holds = [_worker(w, inventory[fid], spec, identity["pwsh_sha256"])
                 for w, fid in zip(workers, spec["fixtures"])]
        need(len({(w["pid"], w["start_utc_ticks"]) for w in workers}) == len(workers), "worker_identity_duplicate")
        previous_end = None
        for index, window in enumerate(group["windows"]):
            previous_end = _window(window, workers, index, previous_end,
                                   record["sampler_script_sha256"], identity["pwsh_sha256"], holds)
            need(group["start_ns"] <= window["start_ns"] <= window["end_ns"] <= group["end_ns"], "window_group_time_invalid")
        need(group["metrics"] == window_metrics(group["windows"]), "observed_maximum_invalid")
        need(group["requirements"] == requirements([inventory[f] for f in spec["fixtures"]]), "logical_requirements_invalid")
        shortfall = "Single-worker two-window series preserves the global32-window cap; three windows remain the multiworker target." if len(workers) == 1 else None
        need(group["target_shortfall_reason"] == shortfall, "sampling_shortfall_hidden")
        eligible = eligible and group["metrics"]["incomplete_window_count"] == 0
    need(record["comparison"] == comparison_rows(experiments, expected)
         and record["matched_run_differences"] == matched_differences(experiments), "comparison_scope_or_value_invalid")
    need(record["measurement_complete"] == eligible, "missing_sample_promoted")
    consumer = record["consumer"]
    need(consumer["exit_code"] == 0 and consumer["resolved_limits"] == LIMITS, "consumer_budget_invalid")
    return {"ok": True, "closure_eligible": eligible, "workers": 24, "experiments": len(experiments),
            "windows": sum(len(g["windows"]) for g in experiments),
            "decoded": sum(w["outcome"] == "decoded" for g in experiments for w in g["workers"]),
            "pixel_verified": sum(w["oracle"] is not None for g in experiments for w in g["workers"]),
            "comparison": record["comparison"], "allocation_scope": ALLOCATION_SCOPE}
