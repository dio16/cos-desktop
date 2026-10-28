"""Fail-closed independent verification for the MP-IMG2-W7 large-input matrix.

The verifier recomputes fixture digests, geometry arithmetic, the documented
admission order, stage closure and the expected raster digest from the fixed
case definitions. It never treats an encoded-size shortcut, a small ROI or a
missing counter as evidence of a materialized full frame.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile

from w2_resource_verify import sha
from w7_large_fixtures import (BY_ID, CASES, expected_encoded_bytes, expected_rgba_digest,
                               expected_sha256)
from w7_large_oracle import iter_png_rows

ROOT = Path(__file__).resolve().parents[4]

BOUNDS = {
    "max_cases": 12, "max_workers": 1, "max_worker_seconds": 45, "max_total_seconds": 300,
    "max_output_bytes": 96 * 1024 * 1024, "max_work_bytes": 512 * 1024 * 1024,
    "max_trace_records": 128, "max_trace_bytes": 128 * 1024, "max_samples": 32,
    "max_capture_bytes": 64 * 1024, "max_report_bytes": 8 * 1024 * 1024,
    "max_observed_private_bytes": 768 * 1024 * 1024, "max_encoded_input_bytes": 64 * 1024 * 1024,
    "max_roi_output_bytes": 4,
}
LIMITS = {"max_dimension": 4096, "max_pixels": 16777216, "max_bit_depth": 8,
          "max_decoded_bytes": 67108864, "max_scratch_bytes": 67108864}
DECLARED_SCRATCH = 0
SOURCE_FILES = (
    "apps/desktop/devtools/img2/w7_large_fixtures.py",
    "apps/desktop/devtools/img2/w7_large_oracle.py",
    "apps/desktop/devtools/img2/w7_large_matrix.py",
    "apps/desktop/devtools/img2/w7_large_verify.py",
    "apps/desktop/devtools/img2/w6_memory_sample.py",
    "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "apps/desktop/devtools/img2/w2_resource_verify.py",
    "apps/desktop/devtools/img2/w2_independent_oracle.py",
    "apps/desktop/devtools/img2/w3_oracle.py",
    "apps/desktop/devtools/img2/w4_region_oracle.py",
    "apps/desktop/devtools/img2/w5_metadata_oracle.py",
    "apps/desktop/devtools/img2/w6_memory_fixtures.py",
    "apps/desktop/devtools/img2/w6_memory_verify.py",
    "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "apps/desktop/examples/decode_compare/wic_admission.ps1",
    "apps/desktop/examples/decode_compare.rs",
    "apps/desktop/examples/decode_compare/harness.rs",
    "apps/desktop/examples/decode_compare/inspect.rs",
    "apps/desktop/examples/decode_compare/pixels.rs",
    "crates/media/src/decode.rs",
    "Cargo.lock",
    "rust-toolchain.toml",
)
STAGES = ("process_observed", "input_opened", "metadata_available", "before_admission",
          "admission_started", "admitted", "refused", "before_onload", "onload_entered",
          "onload_returned", "after_onload", "conversion_entered", "conversion_returned",
          "copy_pixels_entered", "copy_pixels_returned", "encode_entered", "encode_returned",
          "before_publish", "output_published", "probe_exit")
DECODE_STAGES = tuple(name for name in STAGES if name not in ("refused",))
REFUSAL_STAGES = ("process_observed", "input_opened", "metadata_available", "before_admission",
                  "admission_started", "refused", "probe_exit")
REASON_BY_CODE = {
    "max_dimension_exceeded": "dimension",
    "max_pixels_exceeded": "pixels",
    "max_bit_depth_exceeded": "bit_depth",
    "max_decoded_bytes_exceeded": "decoded_bytes",
    "max_scratch_bytes_exceeded": "scratch_bytes",
}
DECODERS = {"png": "PngBitmapDecoder", "bmp24": "BmpBitmapDecoder", "bgr565": "BmpBitmapDecoder"}
UNMEASURED = (
    "WIC internal scratch peak", "actual allocator reservations",
    "true concurrent allocation peak", "WIC-only allocation delta",
    "in-decoder cooperative cancellation", "actual partial decode residency",
    "16K full-resolution decode", "true region/sequential/tiled decode",
    "product numeric memory SLO", "decoder adoption",
)
ALLOCATION_SCOPE = {
    "process_counters": "PROCESS_LEVEL_OBSERVATION",
    "atomic_snapshot": False,
    "decoder_internal_scratch_peak": "NOT_MEASURED",
    "explicit_known_reservations_bytes": "NOT_MEASURED",
    "actually_proven_peak_concurrent_allocation": "NOT_MEASURED",
    "wic_decoder_latency_ns": None,
    "boundary_hold": "ARTIFICIAL_INSTRUMENTATION_NOT_DECODER_TIME",
    "refusal_after_large_source": "UNAVAILABLE",
    "preview_after_refusal": "NOT_PROVEN",
}


def need(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum=None):
    if type(value) is not int:
        return False
    return True if minimum is None else value >= minimum


def row_bytes(codec, width):
    bits = 24 if codec in ("png", "bmp24") else 16
    return (width * bits + 7) // 8


def requirements(case):
    width, height = case["width"], case["height"]
    row = row_bytes(case["codec"], width)
    decoded = row * height
    bgra = width * height * 4
    scratch = bgra + DECLARED_SCRATCH
    return {"width": width, "height": height,
            "bits_per_channel": 8, "bits_per_pixel": 24 if case["codec"] != "bgr565" else 16,
            "row_bytes": row, "decoded_bytes_required": decoded,
            "bgra32_bytes_required": bgra, "scratch_bytes_required": scratch,
            "declared_scratch_bytes": DECLARED_SCRATCH}


def refusal_expectation(case):
    """The probe's documented admission order, recomputed from geometry."""
    values = requirements(case)
    if max(values["width"], values["height"]) > LIMITS["max_dimension"]:
        return "max_dimension_exceeded"
    if values["width"] * values["height"] > LIMITS["max_pixels"]:
        return "max_pixels_exceeded"
    if values["bits_per_channel"] > LIMITS["max_bit_depth"]:
        return "max_bit_depth_exceeded"
    if values["decoded_bytes_required"] > LIMITS["max_decoded_bytes"]:
        return "max_decoded_bytes_exceeded"
    if values["scratch_bytes_required"] > LIMITS["max_scratch_bytes"]:
        return "max_scratch_bytes_exceeded"
    return None


def _source_identity(record: dict, source_root) -> bool:
    identity = record.get("source_identity")
    need(isinstance(identity, dict), "source_identity_missing")
    commit = identity.get("source_commit")
    need(isinstance(commit, str) and len(commit) == 40 and all(c in "0123456789abcdef" for c in commit),
         "source_commit_invalid")
    files = identity.get("files")
    need(isinstance(files, dict) and set(files) == set(SOURCE_FILES), "source_file_set_invalid")
    for path, item in files.items():
        need(isinstance(item, dict), "source_file_entry_invalid")
        need(integer(item.get("bytes"), 0) and isinstance(item.get("raw_sha256"), str)
             and isinstance(item.get("lf_sha256"), str) and isinstance(item.get("git_blob_sha256"), str),
             "source_file_digest_invalid")
        need(item.get("unchanged_after_run") is True, "source_changed_during_run:" + path)
    need(isinstance(identity.get("pwsh_sha256"), str) and identity["pwsh_sha256"].startswith("sha256:"),
         "observer_executable_digest_invalid")
    need(isinstance(identity.get("harness_sha256"), str) and identity["harness_sha256"].startswith("sha256:"),
         "harness_digest_invalid")
    if source_root is None:
        return False
    root = Path(source_root)
    need(root.is_dir(), "source_root_invalid")
    git = os.environ.get("GIT_EXECUTABLE") or "git"
    try:
        blob = subprocess.run([git, "show", f"{commit}:{SOURCE_FILES[0]}"], cwd=root,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
    except (OSError, subprocess.SubprocessError) as exc:
        raise ValueError("git_unavailable:" + type(exc).__name__) from exc
    need(blob.returncode == 0, "source_commit_unavailable")
    for path, item in files.items():
        data = (root / path).read_bytes()
        normalized = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        current = subprocess.run([git, "show", f"{commit}:{path}"], cwd=root,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
        need(current.returncode == 0, "source_blob_missing:" + path)
        need(sha(data) == item["raw_sha256"], "source_raw_digest_mismatch:" + path)
        need(sha(normalized) == item["lf_sha256"], "source_lf_digest_mismatch:" + path)
        need(sha(current.stdout) == item["git_blob_sha256"], "source_git_blob_mismatch:" + path)
        need(current.stdout.replace(b"\r\n", b"\n").replace(b"\r", b"\n") == normalized,
             "source_content_not_pinned:" + path)
    return True


def _fixtures(record: dict) -> dict:
    fixtures = record.get("fixtures")
    need(isinstance(fixtures, list) and len(fixtures) == len(CASES), "fixture_set_invalid")
    by_id = {}
    for item in fixtures:
        need(isinstance(item, dict) and isinstance(item.get("case_id"), str), "fixture_entry_invalid")
        by_id[item["case_id"]] = item
    need(tuple(by_id) == tuple(case["id"] for case in CASES), "fixture_order_or_id_invalid")
    for case in CASES:
        item = by_id[case["id"]]
        expected = expected_sha256(case["id"])
        need(item["sha256"] == expected, "fixture_regeneration_mismatch:" + case["id"])
        need(item["bytes"] == expected_encoded_bytes(case["id"]),
             "fixture_encoded_bytes_mismatch:" + case["id"])
        need(item["codec"] == case["codec"] and item["width"] == case["width"]
             and item["height"] == case["height"], "fixture_declaration_mismatch:" + case["id"])
        need(item["bytes"] <= BOUNDS["max_encoded_input_bytes"], "encoded_input_over_bound")
    return by_id


def _geometry(record: dict) -> dict:
    geometry = record.get("geometry")
    need(isinstance(geometry, dict) and set(geometry) == {case["id"] for case in CASES},
         "geometry_set_invalid")
    for case in CASES:
        item = geometry[case["id"]]
        if case["role"] == "corrupt":
            # A damaged container may fail independent parsing outright or may
            # parse with a stale declaration; either way it must stay unverified.
            need("error" in item
                 or (item.get("width") == case["width"] and item.get("height") == case["height"]),
                 "corrupt_geometry_invalid:" + case["id"])
            continue
        need(item.get("width") == case["width"] and item.get("height") == case["height"],
             "encoded_geometry_mismatch:" + case["id"])
    return geometry


def _trace(worker: dict):
    raw = worker.get("trace_utf8")
    need(isinstance(raw, str), "trace_missing")
    data = raw.encode("utf-8")
    need(sha(data) == worker.get("trace_sha256"), "trace_digest_mismatch")
    need(len(data) == worker.get("trace_bytes") <= BOUNDS["max_trace_bytes"], "trace_size_invalid")
    prefix = data[:data.rfind(b"\n") + 1]
    stages = worker.get("stages")
    need(isinstance(stages, list) and len(stages) == worker.get("trace_records")
         and len(stages) <= BOUNDS["max_trace_records"], "trace_record_count_invalid")
    need(len(data) - len(prefix) == worker.get("trace_partial_bytes"), "trace_partial_invalid")
    parsed = [json.loads(line) for line in prefix.splitlines()]
    need(parsed == stages, "trace_payload_mismatch")
    tick = -1
    frequency = None
    for index, stage in enumerate(stages, 1):
        need(stage.get("schema_version") == 1 and stage.get("seq") == index, "stage_sequence_invalid")
        need(stage.get("stage") in STAGES, "stage_name_invalid")
        need(stage.get("clock_domain") == "windows_qpc" and stage.get("observer") == "wic_probe",
             "stage_clock_domain_invalid")
        need((stage.get("pid"), stage.get("process_start_utc_ticks"))
             == (worker.get("pid"), worker.get("start_utc_ticks")), "stage_identity_invalid")
        need(integer(stage.get("qpc_ticks"), tick), "stage_clock_order_invalid")
        tick = stage["qpc_ticks"]
        if frequency is None:
            frequency = stage.get("qpc_frequency")
        need(integer(frequency, 1) and stage.get("qpc_frequency") == frequency,
             "stage_clock_frequency_invalid")
    return stages, data


def _sample(worker: dict, case: dict):
    window = worker.get("sample")
    need(isinstance(window, dict), "process_sample_missing:" + case["id"])
    need(window.get("complete") is True and window.get("status") == "complete",
         "process_sample_incomplete:" + case["id"])
    need(window.get("case_ids") == [case["id"]], "process_sample_case_mismatch:" + case["id"])
    need(window.get("clock_domain") == "python_monotonic_ns", "process_sample_clock_invalid")
    need(window.get("atomic_snapshot") is False
         and window.get("aggregation") == "same_collection_window", "process_sample_claim_invalid")
    samples = window.get("samples")
    need(isinstance(samples, list) and len(samples) == 1, "process_sample_count_invalid")
    sample = samples[0]
    need(sample.get("case_id") == case["id"] and sample.get("pid") == worker.get("pid")
         and sample.get("start_utc_ticks") == worker.get("start_utc_ticks"),
         "process_sample_identity_mismatch")
    need(sample.get("identity_after_ticks") == sample.get("start_utc_ticks")
         and sample.get("exe_verified") is True, "process_sample_identity_changed")
    need(sample.get("alive_before") is True and sample.get("alive_after") is True,
         "process_sample_lifetime_invalid")
    for key in ("working_set_bytes", "private_bytes"):
        need(integer(sample.get(key), 1), "process_sample_counter_invalid:" + key)
    need(sample.get("peak_status") in ("MEASURED_PROCESS_LIFETIME_COUNTER", "NOT_MEASURED"),
         "process_sample_peak_status_invalid")
    totals = window.get("totals")
    need(isinstance(totals, dict)
         and totals.get("private_bytes") == sample["private_bytes"]
         and totals.get("working_set_bytes") == sample["working_set_bytes"], "process_sample_totals_invalid")
    need(totals["private_bytes"] <= BOUNDS["max_observed_private_bytes"],
         "observed_private_bound_exceeded")
    observer = window.get("observer")
    need(isinstance(observer, dict) and observer.get("capture_complete") is True
         and observer.get("timeout") is False and observer.get("exit_code") == 0,
         "process_observer_incomplete")
    need(observer.get("stdout_bytes", 0) <= BOUNDS["max_capture_bytes"]
         and observer.get("stderr_bytes", 0) <= BOUNDS["max_capture_bytes"], "process_capture_over_bound")
    return window


def _decode_case(worker: dict, case: dict, stages: list, expected_rgba: dict, fixture_bytes: int):
    result = worker.get("probe_result")
    need(isinstance(result, dict), "probe_result_missing")
    need(worker.get("outcome") == "decoded" and worker.get("exit_code") == 0,
         "decode_exit_invalid:" + case["id"])
    need(worker.get("trace_complete") is True and worker.get("trace_partial_bytes") == 0
         and worker.get("trace_utf8", "").encode("utf-8").endswith(b"\n"), "decode_trace_incomplete")
    need([stage["stage"] for stage in stages] == list(DECODE_STAGES), "decode_stage_sequence_invalid")
    values = requirements(case)
    admitted = next(stage for stage in stages if stage["stage"] == "admitted")
    need(all(type(admitted.get(key)) is int and admitted[key] == value
             for key, value in {"width": values["width"], "height": values["height"],
                                "bits_per_channel": values["bits_per_channel"],
                                "bits_per_pixel": values["bits_per_pixel"],
                                "row_bytes": values["row_bytes"],
                                "decoded_bytes_required": values["decoded_bytes_required"],
                                "bgra32_bytes_required": values["bgra32_bytes_required"],
                                "declared_scratch_bytes": values["declared_scratch_bytes"],
                                "scratch_bytes_required": values["scratch_bytes_required"],
                                "input_bytes": fixture_bytes}.items()),
         "admission_arithmetic_invalid:" + case["id"])
    need(result.get("admission_status") == "admitted" and result.get("materialization") == "completed",
         "decode_not_materialized:" + case["id"])
    need(result.get("decoder") == DECODERS[case["codec"]],
         "decoder_identity_mismatch:" + case["id"])
    terminal = stages[-1]
    need(terminal.get("stage") == "probe_exit" and terminal.get("complete") is True
         and terminal.get("outcome") == "decoded" and terminal.get("materialization") == "completed",
         "decode_terminal_stage_mismatch:" + case["id"])
    need(worker.get("output_present") is True and worker.get("published_success") is True,
         "decode_output_missing:" + case["id"])
    hexed = worker.get("output_hex")
    need(isinstance(hexed, str) and hexed, "decode_output_bytes_missing:" + case["id"])
    data = bytes.fromhex(hexed)
    need(worker.get("output_bytes") == len(data) <= BOUNDS["max_output_bytes"], "decode_output_size_invalid")
    need(worker.get("output_sha256") == sha(data), "decode_output_digest_mismatch")
    oracle = worker.get("oracle")
    need(isinstance(oracle, dict) and oracle.get("ok") is True
         and oracle.get("first_mismatch") is None, "decode_pixel_oracle_failed:" + case["id"])
    need(oracle.get("compared_pixels") == case["width"] * case["height"],
         "decode_compared_pixels_incomplete:" + case["id"])
    need(oracle.get("expected_rgba_sha256") == expected_rgba["rgba_sha256"],
         "decode_expected_digest_not_independent:" + case["id"])
    need(oracle.get("actual_rgba_sha256") == expected_rgba["rgba_sha256"],
         "decode_actual_digest_differs:" + case["id"])
    with tempfile.TemporaryDirectory() as folder:
        path = Path(folder) / "output.png"
        path.write_bytes(data)
        import hashlib
        digest = hashlib.sha256()
        rows = 0
        for row in iter_png_rows(path, BOUNDS["max_report_bytes"]):
            digest.update(row)
            rows += 1
        need(rows == case["height"], "decode_output_row_count_invalid")
        need("sha256:" + digest.hexdigest() == oracle["actual_rgba_sha256"],
             "decode_output_pixels_not_reproducible:" + case["id"])


def _refusal_case(worker: dict, case: dict, stages: list, expected_reason: str):
    result = worker.get("probe_result")
    need(isinstance(result, dict), "probe_result_missing")
    need(worker.get("outcome") == "oversize" and worker.get("exit_code") == 4,
         "refusal_exit_invalid:" + case["id"])
    need(worker.get("trace_complete") is True and worker.get("trace_partial_bytes") == 0,
         "refusal_trace_incomplete:" + case["id"])
    names = [stage["stage"] for stage in stages]
    need(names == list(REFUSAL_STAGES), "refusal_stage_sequence_invalid:" + case["id"])
    need(result.get("admission_status") == "refused", "refusal_not_recorded:" + case["id"])
    need(result.get("width") == case["width"] and result.get("height") == case["height"],
         "refusal_declared_geometry_mismatch:" + case["id"])
    need(result.get("reason_code") == expected_reason, "refusal_reason_mismatch:" + case["id"])
    need(result.get("decoder") == DECODERS[case["codec"]],
         "refusal_decoder_identity_mismatch:" + case["id"])
    terminal = stages[-1]
    need(terminal.get("stage") == "probe_exit" and terminal.get("complete") is True
         and terminal.get("outcome") == "oversize", "refusal_terminal_stage_mismatch:" + case["id"])
    need(result.get("materialization") == "not_requested", "refusal_requested_materialization")
    need(worker.get("output_present") is False and worker.get("published_success") is False,
         "refusal_published_output:" + case["id"])
    need(worker.get("output_bytes") == 0 and worker.get("output_hex") is None,
         "refusal_output_payload:" + case["id"])
    values = requirements(case)
    if case["role"] == "roi_refusal":
        need(result.get("roi_requested") is True and result.get("roi_output") == "not_produced",
             "roi_state_invalid:" + case["id"])
        need(result.get("roi_width") == case["roi_width"]
             and result.get("roi_height") == case["roi_height"], "roi_rectangle_invalid")
        need(result.get("roi_output_bytes_required") == case["roi_width"] * case["roi_height"] * 4,
             "roi_output_requirement_invalid")
        need(result.get("full_frame_decoded_bytes") == values["decoded_bytes_required"],
             "roi_reduced_full_frame_requirement")
        need(result.get("reason_code") != "roi_output_exceeded",
             "roi_output_bound_replaced_full_frame_budget")
        need(result.get("true_partial_decode") == "not_verified"
             and result.get("full_frame_materialization") == "not_observed",
             "roi_claimed_partial_decode")
    else:
        need(result.get("roi_requested") in (None, False), "unexpected_roi_request")
    return values


def verify(record: dict, source_root=None) -> dict:
    """Return a closed verdict; closure requires fixed-source verification."""
    try:
        need(isinstance(record, dict), "record_not_object")
        need(record.get("schema_version") == 1, "schema_version_invalid")
        need(record.get("bounds") == BOUNDS, "bounds_invalid")
        need(record.get("decode_limits") == LIMITS, "decode_limits_invalid")
        need(record.get("allocation_scope") == ALLOCATION_SCOPE, "allocation_scope_invalid")
        need(tuple(record.get("unmeasured") or ()) == UNMEASURED, "unmeasured_set_invalid")
        need(record.get("cleanup", {}).get("complete") is True
             and record["cleanup"].get("live_direct_children") == [], "cleanup_incomplete")
        need(record.get("work_root_disposable") is True, "work_root_disposition_invalid")
        need(integer(record.get("duration_ns"), 0)
             and record["duration_ns"] <= BOUNDS["max_total_seconds"] * 1_000_000_000,
             "run_deadline_exceeded")
        need(integer(record.get("work_bytes"), 0)
             and record["work_bytes"] <= BOUNDS["max_work_bytes"], "work_bound_exceeded")
        need(record.get("complete") is True, "run_incomplete")
        fixtures = _fixtures(record)
        _geometry(record)
        source_verified = _source_identity(record, source_root)
        cases = record.get("cases")
        need(isinstance(cases, list) and len(cases) == len(CASES), "case_count_invalid")
        need([row.get("case_id") for row in cases] == [case["id"] for case in CASES],
             "case_order_invalid")
        verified = 0
        refused = 0
        tamper = 0
        for case, row in zip(CASES, cases):
            need(isinstance(row, dict) and row.get("role") == case["role"], "case_role_mismatch")
            need(row.get("failure") is None, "case_failure:" + case["id"])
            worker = row.get("worker")
            need(isinstance(worker, dict), "case_worker_missing:" + case["id"])
            need(worker["fixture_id"] == case["id"] and worker["case_id"] == case["id"],
                 "worker_case_mismatch")
            need(worker.get("input_sha256") == fixtures[case["id"]]["sha256"], "worker_input_digest_mismatch")
            need(worker.get("exe_sha256") == record["source_identity"]["pwsh_sha256"],
                 "worker_executable_digest_mismatch")
            need(integer(worker.get("pid"), 1) and integer(worker.get("start_utc_ticks"), 1),
                 "worker_identity_missing")
            need(worker.get("reaped") is True and worker.get("alive_after_wait") is False,
                 "worker_not_reaped")
            need(worker.get("limits") == LIMITS, "worker_limits_changed")
            need(integer(worker.get("started_ns"), 1)
                 and integer(worker.get("exited_ns"), worker["started_ns"])
                 and worker["exited_ns"] - worker["started_ns"]
                 <= BOUNDS["max_worker_seconds"] * 1_000_000_000, "worker_deadline_invalid")
            stages, _ = _trace(worker)
            _sample(worker, case)
            need(worker.get("identity_verified") is True, "worker_identity_not_verified")
            need(worker.get("boundary_hold_is_instrumentation") is True, "hold_classification_mismatch")
            stdout = worker.get("stdout_utf8")
            need(isinstance(stdout, str) and worker.get("stdout_bytes") == len(stdout.encode("utf-8"))
                 <= BOUNDS["max_capture_bytes"], "worker_stdout_invalid")
            need(worker.get("stderr_bytes", 0) <= BOUNDS["max_capture_bytes"], "worker_stderr_over_bound")
            classification = row.get("classification")
            expected_reason = refusal_expectation(case)
            if case["role"] == "decode":
                need(classification == "DECODED_AND_VERIFIED", "decode_classification_invalid:" + case["id"])
                expected_rgba = expected_rgba_digest(case["id"])
                _decode_case(worker, case, stages, expected_rgba, fixtures[case["id"]]["bytes"])
                verified += 1
            elif case["role"] in ("refusal", "roi_refusal"):
                need(classification == "REFUSED_FULL_FRAME", "refusal_classification_invalid:" + case["id"])
                need(expected_reason is not None, "refusal_case_should_exceed_budget:" + case["id"])
                _refusal_case(worker, case, stages, expected_reason)
                refused += 1
            else:
                need(classification == "TAMPER_NOT_VERIFIED",
                     "corrupt_case_cannot_claim_success:" + case["id"])
                need(classification != "DECODED_AND_VERIFIED", "corrupt_case_promoted")
                tamper += 1
        need(verified == 6 and refused == 4 and tamper == 2, "classification_totals_invalid")
        return {"ok": True, "cases": len(cases), "decoded_verified": verified,
                "refused": refused, "tamper_not_verified": tamper,
                "source_identity_verified": source_verified,
                "closure_eligible": bool(source_verified),
                "fixture_regeneration_verified": True,
                "measurement_scope": "process-level observation; internal scratch, reservations and "
                                      "true concurrent peaks remain NOT_MEASURED"}
    except (OSError, ValueError, KeyError, TypeError, IndexError, json.JSONDecodeError) as exc:
        return {"ok": False, "error": type(exc).__name__, "reason": str(exc)[:200],
                "source_identity_verified": False, "closure_eligible": False}
