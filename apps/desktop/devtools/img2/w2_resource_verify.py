"""Fail-closed, portable verification of the bounded MP-IMG2-W2 record.

Report-only verification checks retained observations and arithmetic, not discarded
file bytes. Supplying a work root additionally rehashes actual captures/outputs.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import struct

BOUNDS = {
    "max_cases": 24, "max_workers": 3, "max_worker_seconds": 45,
    "max_total_seconds": 300, "max_output_bytes": 16 * 1024 * 1024,
    "max_work_bytes": 64 * 1024 * 1024, "max_trace_records": 128,
    "max_trace_bytes": 128 * 1024, "max_samples": 32,
    "max_capture_bytes": 64 * 1024, "max_report_bytes": 8 * 1024 * 1024,
    "max_observed_private_bytes": 768 * 1024 * 1024,
}
FIXTURES = {
    "small_png": ("small.png", "png", 16, 16, (17, 34, 51)),
    "small_bmp": ("small.bmp", "bmp", 16, 16, (17, 34, 51)),
    "small_jpeg": ("small.jpg", "jpeg", 32, 24, None),
    "preview_png": ("preview.png", "png", 256, 256, (17, 34, 51)),
    "preview_png_2": ("preview2.png", "png", 256, 256, (5, 19, 37)),
    "detail_png": ("detail.png", "png", 1024, 1024, (17, 34, 51)),
    "oversized_png": ("oversized.png", "png", 100000, 100000, None),
    "unsupported": ("unsupported.bin", "unknown", None, None, None),
}
EXPECTED_CASES = set(FIXTURES)
# One fixed bounded matrix; a report cannot make its own missing cases optional.
SCENARIOS = [("single_" + x, x, "success", "after_onload") for x in
             ("small_png", "small_bmp", "small_jpeg", "preview_png", "detail_png")]
SCENARIOS += [("refuse_" + x, x, "refusal", "") for x in ("oversized_png", "unsupported")]
SCENARIOS += [("cohort_a", "preview_png", "cohort", "after_onload"),
              ("cohort_b", "preview_png_2", "cohort", "after_onload"),
              ("cohort_detail", "detail_png", "cohort", "after_onload")]
SCENARIOS += [("cancel_" + x, "detail_png", "cancel", x) for x in
              ("before_admission", "before_onload", "after_onload", "before_publish")]
SCENARIOS += [("repeat_1", "preview_png", "cancel", "before_onload"),
              ("repeat_2", "preview_png", "cancel", "before_onload")]
SCENARIOS += [("onload_" + str(i), "detail_png", "onload_attempt", "before_onload")
              for i in range(1, 4)]
SCENARIOS += [("timeout", "detail_png", "timeout", "before_onload")]
STAGES = ("process_observed", "input_opened", "metadata_available", "before_admission",
          "admission_started", "admitted", "before_onload", "onload_entered",
          "onload_returned", "after_onload", "conversion_entered", "conversion_returned",
          "copy_pixels_entered", "copy_pixels_returned", "encode_entered", "encode_returned",
          "before_publish", "output_published", "probe_exit")
SOURCE_FILES = {
    "probe": "apps/desktop/examples/decode_compare/wic_probe.ps1",
    "admission": "apps/desktop/examples/decode_compare/wic_admission.ps1",
    "runner": "apps/desktop/devtools/img2/w2_resource_matrix.py",
    "verifier": "apps/desktop/devtools/img2/w2_resource_verify.py",
    "w1_generator": "docs/development/evidence-published/img2-w1-latest/run_matrix.py",
    "consumer_harness": "apps/desktop/examples/decode_compare/harness.rs",
    "consumer_entry": "apps/desktop/examples/decode_compare.rs",
    "media_budget": "crates/media/src/decode.rs",
    "cargo_lock": "Cargo.lock",
    "toolchain": "rust-toolchain.toml",
}
MEMORY_NAMES = ("encoded_input_bytes", "admitted_decoded_bytes", "bgra32_destination_bytes",
                "declared_scratch_bytes", "explicit_known_requirements_bytes",
                "working_set_bytes", "private_bytes", "concurrent_working_set_bytes",
                "concurrent_private_bytes", "decoder_internal_scratch_peak",
                "actually_proven_peak_concurrent_allocation", "explicit_known_reservations_bytes")


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def file_sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def canonical(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"),
                       ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")


def redact_paths(value, roots):
    """Replace only declared owned absolute prefixes; preserve every numeric value."""
    if isinstance(value, dict):
        return {k: redact_paths(v, roots) for k, v in value.items()}
    if isinstance(value, list):
        return [redact_paths(v, roots) for v in value]
    if isinstance(value, str):
        for root, label in roots:
            value = value.replace(str(root), label).replace(Path(root).as_posix(), label)
    return value


def raster_sha(case_id: str) -> str:
    _, _, w, h, color = FIXTURES[case_id]
    if color is None:
        raise ValueError("no_absolute_raster_oracle")
    return hashlib.sha256(bytes((*color, 255)) * (w * h)).hexdigest()


def _integer(value, minimum=0):
    return type(value) is int and value >= minimum


def _digest(value):
    return isinstance(value, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", value) is not None


def _relative(root: Path, name: str) -> Path:
    p = Path(name)
    if p.is_absolute() or not p.parts or any(x in ("..", ".") for x in p.parts):
        raise ValueError("unsafe_evidence_path")
    target = root / p
    if target.is_symlink() or not target.resolve().is_relative_to(root.resolve()):
        raise ValueError("unsafe_evidence_path")
    return target


def _closed_json(data: str):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate_json_key")
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=pairs,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite_json")))


def verify(report: object, work_root=None, source_root=None) -> dict:
    errors = []
    mode = "native_files" if work_root is not None else "report_only"
    try:
        _verify(report, Path(work_root) if work_root is not None else None,
                Path(source_root) if source_root is not None else None, errors)
    except (KeyError, TypeError, ValueError, OSError, OverflowError, struct.error) as exc:
        errors.append("invalid_or_incomplete_record:" + type(exc).__name__)
    return {"ok": not errors, "errors": errors, "mode": mode,
            "claims": ("retained observations plus available source/work file rehashes" if work_root
                       else "retained-observation structural replay; discarded bytes not reverified")}


def _verify(r, work, source, errors):
    def need(condition, token):
        if not condition:
            errors.append(token)
    if not isinstance(r, dict):
        raise ValueError("record_missing")
    need(r["schema_version"] == 1, "schema_invalid")
    b = r["bounds"]
    need(all(_integer(b[k], 1) and b[k] <= v for k, v in BOUNDS.items()), "bounds_invalid")
    need(len(canonical(r)) <= b["max_report_bytes"], "report_cap_exceeded")
    fixtures = r["fixtures"]
    need(isinstance(fixtures, list) and len(fixtures) == len(FIXTURES), "fixture_set_missing")
    ids = [f["case_id"] for f in fixtures]
    need(len(set(ids)) == len(ids) and set(ids) == EXPECTED_CASES, "fixture_case_set_invalid")
    need(r["expected_case_ids"] == sorted(EXPECTED_CASES), "expected_case_set_invalid")
    inventory = {f["case_id"]: f for f in fixtures}
    for f in fixtures:
        name, fmt, w, h, _ = FIXTURES[f["case_id"]]
        need((f["path"], f["format"], f["width"], f["height"]) == (name, fmt, w, h), "fixture_metadata_mismatch")
        need(_integer(f["bytes"], 1) and _digest(f["sha256"]), "fixture_identity_invalid")
        if work:
            p = _relative(work / "cases", f["path"])
            need(p.stat().st_size == f["bytes"] and file_sha(p) == f["sha256"], "fixture_digest_mismatch")
    identities = r["source_identity"]
    need(set(identities["files"]) == set(SOURCE_FILES), "source_set_invalid")
    for key, rel in SOURCE_FILES.items():
        item = identities["files"][key]
        need(item["path"] == rel and _digest(item["raw_sha256"]) and _digest(item["lf_sha256"])
             and _integer(item["bytes"], 1) and item["unchanged_after_run"] is True, "source_identity_invalid")
        if source:
            raw = (source / rel).read_bytes()
            normalized = raw.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
            need(sha(normalized) == item["lf_sha256"], "source_digest_mismatch")
            # Native workspace claims exact bytes; Git LF replay is named separately.
            if work:
                need(sha(raw) == item["raw_sha256"], "executed_source_bytes_mismatch")
    need(all(_digest(identities[k]) for k in ("pwsh_sha256", "harness_sha256")), "executable_digest_invalid")
    consumer = r["consumer"]
    need(consumer["exit_code"] == 0 and consumer["complete"] is True, "consumer_failed")
    cr = consumer["record"]
    need(sha(canonical(cr)) == consumer["canonical_record_sha256"], "consumer_record_digest_mismatch")
    if work:
        raw = (work / "consumer.json").read_bytes()
        original = _closed_json(raw.decode("utf-8"))
        need(sha(raw) == consumer["raw_record_sha256"]
             and sha(canonical(original)) == consumer["original_canonical_sha256"], "consumer_native_digest_mismatch")
        need(source is not None and redact_paths(original, [(work, "<work-root>"), (source, "<repository>")]) == cr,
             "consumer_native_payload_mismatch")
    rows = cr["observations"]
    expected_names = {x[0] for x in FIXTURES.values()}
    row_ids = [x["case_id"] for x in rows]
    need(len(rows) == len(expected_names) and len(set(row_ids)) == len(rows)
         and set(row_ids) == expected_names, "consumer_case_set_invalid")
    for row in rows:
        cid = next(k for k, v in FIXTURES.items() if v[0] == row["case_id"])
        if cid in ("unsupported", "oversized_png"):
            need(row["observed_outcome"] == ("unsupported" if cid == "unsupported" else "oversize")
                 and not row["output_present"], "consumer_refusal_invalid")
        else:
            need(row["observed_outcome"] == "decoded" and row["output_present"], "consumer_decode_missing")
            if cid == "small_jpeg":
                need(row["validity"] == "structure_verified", "jpeg_absolute_raster_claim")
            else:
                need(row["validity"] == "pixels_verified" and row["pixel_verified"] is True
                     and row["pixel_digest"] == raster_sha(cid), "consumer_pixel_oracle_mismatch")
    limits = r["decode_limits"]["limits"]
    need(limits == cr["decode_budget"]["limits"] if "limits" in cr["decode_budget"]
         else all(limits[k] == cr["decode_budget"][k] for k in limits), "budget_source_mismatch")
    need(set(limits) == {"max_dimension", "max_pixels", "max_bit_depth", "max_decoded_bytes", "max_scratch_bytes"}
         and all(_integer(v, 1) for v in limits.values()), "decode_limits_invalid")
    workers = r["workers"]
    specs = {x[0]: x for x in SCENARIOS}
    need(isinstance(workers, list) and len(workers) == len(specs) <= b["max_cases"], "worker_set_missing")
    worker_ids = [x["case_id"] for x in workers]
    need(len(set(worker_ids)) == len(worker_ids) and set(worker_ids) == set(specs), "worker_case_set_invalid")
    by_id = {x["case_id"]: x for x in workers}
    for w in workers:
        _, fixture_id, kind, hold = specs[w["case_id"]]
        need((w["fixture_id"], w["kind"], w["hold_stage"]) == (fixture_id, kind, hold), "worker_spec_mismatch")
        need(_integer(w["pid"], 1) and _integer(w["start_utc_ticks"], 1)
             and w["exe_sha256"] == identities["pwsh_sha256"]
             and w["input_sha256"] == inventory[fixture_id]["sha256"], "worker_identity_invalid")
        need(w["reaped"] is True and w["alive_after_wait"] is False, "worker_cleanup_invalid")
        need(w["clock_domain"] == "python_monotonic_ns" and w["cross_process_clock_subtraction"] is False,
             "cross_clock_comparison_invalid")
        need(0 <= w["started_ns"] <= w["exited_ns"] and w["exited_ns"] - w["started_ns"] <=
             (b["max_worker_seconds"] + 6) * 1_000_000_000, "worker_deadline_invalid")
        stages = w["stages"]
        need(isinstance(stages, list) and stages, "trace_empty")
        need(len(stages) <= b["max_trace_records"] and w["trace_bytes"] <= b["max_trace_bytes"], "trace_cap_exceeded")
        names = [s["stage"] for s in stages]
        need(len(names) == len(set(names)), "trace_stage_duplicate")
        ranks = {s: i for i, s in enumerate(STAGES)}
        previous = -1
        tick = -1
        frequency = None
        for i, s in enumerate(stages):
            need(s["schema_version"] == 1 and s["seq"] == i + 1 and s["clock_domain"] == "windows_qpc"
                 and s["observer"] == "wic_probe", "trace_schema_or_clock_invalid")
            need((s["pid"], s["process_start_utc_ticks"]) == (w["pid"], w["start_utc_ticks"]), "process_identity_mismatch")
            need(_integer(s["qpc_ticks"], 0) and s["qpc_ticks"] >= tick
                 and _integer(s["qpc_frequency"], 1), "trace_time_invalid")
            frequency = frequency or s["qpc_frequency"]
            need(s["qpc_frequency"] == frequency, "trace_frequency_changed")
            tick = s["qpc_ticks"]
            if s["stage"] == "refused":
                need("admitted" not in names and "onload_entered" not in names, "materialization_after_refusal")
            else:
                rank = ranks.get(s["stage"], -1)
                need(rank > previous, "trace_stage_order_invalid")
                previous = rank
        need(w["trace_complete"] == (names[-1:] == ["probe_exit"] and stages[-1].get("complete") is True),
             "trace_completeness_invalid")
        if "admitted" in names:
            admission = next(s for s in stages if s["stage"] == "admitted")
            width, height, bits = admission["width"], admission["height"], admission["bits_per_pixel"]
            row_bytes = (width * bits + 7) // 8
            need(width == inventory[fixture_id]["width"] and height == inventory[fixture_id]["height"]
                 and admission["row_bytes"] == row_bytes
                 and admission["decoded_bytes_required"] == row_bytes * height
                 and admission["bgra32_bytes_required"] == width * height * 4
                 and admission["scratch_bytes_required"] == width * height * 4 + admission["declared_scratch_bytes"],
                 "admission_arithmetic_mismatch")
        cancellation = w["cancellation"]
        if cancellation["requested_ns"] is not None:
            need(w["identity_verified"] is True and cancellation["waited"] is True
                 and cancellation["method"] == "direct_child_terminate_wait"
                 and w["started_ns"] <= cancellation["requested_ns"] <= w["exited_ns"]
                 and cancellation["observed_ns"] == w["exited_ns"], "cancellation_lifecycle_invalid")
            need(not w["output_present"] and not w["published_success"]
                 and w["outcome"] in ("cancelled", "timeout"), "cancelled_success_published")
        elif kind in ("cancel", "timeout"):
            need(False, "required_cancellation_not_reached")
        expected_outcomes = {
            "success": {"decoded"}, "cohort": {"decoded"}, "cancel": {"cancelled"},
            "timeout": {"timeout"}, "onload_attempt": {"decoded", "cancelled"},
            "refusal": {"oversize" if fixture_id == "oversized_png" else "unsupported"},
        }
        need(w["outcome"] in expected_outcomes[kind], "worker_expected_outcome_mismatch")
        if kind in ("cancel", "timeout"):
            need(cancellation["last_observed_stage"] == hold, "cancellation_boundary_mismatch")
        if kind == "onload_attempt":
            need(cancellation["reachability"] == "NOT_MEASURED", "in_decoder_cancel_falsely_claimed")
        if w["outcome"] == "decoded":
            need(w["exit_code"] == 0 and w["trace_complete"] and w["output_present"]
                 and w["published_success"] and "output_published" in names
                 and _integer(w["output_bytes"], 1) and _digest(w["output_sha256"])
                 and w["probe_result"]["outcome"] == "decoded", "success_output_missing")
            need(stages[-1]["outcome"] == "decoded", "terminal_outcome_mismatch")
        elif w["outcome"] in ("unsupported", "oversize"):
            need(not w["output_present"] and "onload_entered" not in names and w["trace_complete"], "refusal_output_invalid")
            need(w["exit_code"] == (4 if w["outcome"] == "oversize" else 2)
                 and w["probe_result"]["outcome"] == w["outcome"]
                 and stages[-1]["outcome"] == w["outcome"], "refusal_outcome_mismatch")
        else:
            need(w["outcome"] in ("cancelled", "timeout"), "unexpected_worker_failure")
        need(0 <= w["output_bytes"] <= b["max_output_bytes"], "output_cap_exceeded")
        if work:
            trace = _relative(work, w["trace_path"]).read_bytes()
            need(sha(trace) == w["trace_sha256"] and len(trace) == w["trace_bytes"], "trace_digest_mismatch")
            prefix = trace[:trace.rfind(b"\n") + 1]
            need(len(trace) - len(prefix) == w["trace_partial_bytes"], "trace_partial_length_mismatch")
            need([_closed_json(line.decode("utf-8")) for line in prefix.splitlines()] == stages, "trace_payload_mismatch")
            if w["output_present"]:
                output = _relative(work, w["output_path"])
                need(output.stat().st_size == w["output_bytes"] and file_sha(output) == w["output_sha256"], "output_digest_mismatch")
    cohorts = r["cohorts"]
    need(isinstance(cohorts, list) and 1 <= len(cohorts) <= b["max_samples"], "cohort_missing_or_capped")
    seen_concurrent = False
    for c in cohorts:
        need(c["complete"] is True and c["status"] == ("sampled_lower_bound" if len(c["case_ids"]) == 1 else "derived_window_sum")
             and c["atomic_snapshot"] is False and c["aggregation"] == "same_collection_window"
             and c["clock_domain"] == "python_monotonic_ns"
             and c["start_ns"] <= c["end_ns"], "cohort_incomplete_or_invalid")
        expected = c["case_ids"]
        need(len(expected) == len(set(expected)) and 1 <= len(expected) <= b["max_workers"], "cohort_case_set_invalid")
        samples = c["samples"]
        need(len(samples) == len(expected) and {s["case_id"] for s in samples} == set(expected), "cohort_sample_missing")
        for s in samples:
            w = by_id[s["case_id"]]
            need((s["pid"], s["start_utc_ticks"]) == (w["pid"], w["start_utc_ticks"])
                 and s["identity_after_ticks"] == s["start_utc_ticks"]
                 and s["exe_verified"] is True and s["alive_before"] is True and s["alive_after"] is True,
                 "cohort_identity_mismatch")
            need(w["started_ns"] <= c["start_ns"] <= c["end_ns"] <= w["exited_ns"], "cohort_outside_lifetime")
            need(_integer(s["working_set_bytes"], 1) and _integer(s["private_bytes"], 1), "cohort_memory_missing")
        need(c["totals"] == {k: sum(s[k] for s in samples) for k in ("working_set_bytes", "private_bytes")}, "cohort_total_mismatch")
        need(c["totals"]["private_bytes"] <= b["max_observed_private_bytes"], "observed_memory_budget_exceeded")
        if set(expected) == {"cohort_a", "cohort_b", "cohort_detail"}:
            seen_concurrent = True
    need(seen_concurrent, "concurrent_cohort_missing")
    memory = r["memory"]
    need(set(memory) == set(MEMORY_NAMES), "memory_field_set_invalid")
    for name in MEMORY_NAMES:
        m = memory[name]
        need(m["unit"] == "bytes" and isinstance(m["source"], str) and m["method"]
             and isinstance(m["limitations"], list) and _integer(m["sample_count"]), "memory_metadata_invalid")
        if name in ("decoder_internal_scratch_peak", "actually_proven_peak_concurrent_allocation", "explicit_known_reservations_bytes"):
            need(m["value"] is None and m["status"] == "not_measured" and not m["complete"], "unobserved_peak_claimed")
        elif name in ("working_set_bytes", "private_bytes", "concurrent_working_set_bytes", "concurrent_private_bytes"):
            key = "private_bytes" if "private" in name else "working_set_bytes"
            selected = [c for c in cohorts if len(c["case_ids"]) == (3 if name.startswith("concurrent_") else 1)]
            need(selected and m["sample_count"] == len(selected) and m["complete"] is True
                 and m["status"] == ("derived" if name.startswith("concurrent_") else "sampled_lower_bound")
                 and m["value"] == max(c["totals"][key] for c in selected), "memory_sample_summary_invalid")
        else:
            need(_integer(m["value"]) and m["status"] == "derived" and m["complete"] is True, "derived_memory_invalid")
    concurrent_admissions = [next(s for s in by_id[cid]["stages"] if s["stage"] == "admitted")
                             for cid in ("cohort_a", "cohort_b", "cohort_detail")]
    requirements = {
        "encoded_input_bytes": sum(a["input_bytes"] for a in concurrent_admissions),
        "admitted_decoded_bytes": sum(a["decoded_bytes_required"] for a in concurrent_admissions),
        "bgra32_destination_bytes": sum(a["bgra32_bytes_required"] for a in concurrent_admissions),
        "declared_scratch_bytes": sum(a["declared_scratch_bytes"] for a in concurrent_admissions),
    }
    requirements["explicit_known_requirements_bytes"] = sum(requirements.values())
    need(all(memory[k]["value"] == v for k, v in requirements.items()), "known_requirement_arithmetic_mismatch")
    need(r["cleanup"]["complete"] is True and r["cleanup"]["live_direct_children"] == []
         and r["cleanup"]["cancelled_output_paths_remaining"] == [], "cleanup_incomplete")
    need(r["work_bytes"] <= b["max_work_bytes"] and r["duration_ns"] <= b["max_total_seconds"] * 1_000_000_000,
         "work_or_total_deadline_exceeded")
    if work:
        need(work.is_dir() and sum(p.stat().st_size for p in work.rglob("*") if p.is_file()) <= b["max_work_bytes"],
             "native_work_cap_exceeded")


def verify_file(report_path, work_root=None, source_root=None):
    try:
        path = Path(report_path)
        if not 0 < path.stat().st_size <= BOUNDS["max_report_bytes"]:
            raise ValueError("record_size")
        report = _closed_json(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, UnicodeError):
        return {"ok": False, "errors": ["record_missing_incomplete_or_capped"], "mode": "report_only"}
    return verify(report, work_root, source_root)
