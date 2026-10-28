"""Fail-closed verification for the W8 JPEG matrices.

The verifier regenerates the fixtures, re-parses their markers, recomputes the
absolute DC reference, re-runs the canonical comparison from the retained output
bytes and refuses any record whose decoder, admission, publication or
classification claims contradict those independent values.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from w2_resource_verify import sha
from w7_large_oracle import iter_png_rows
from w8_jpeg_fixtures import BY_ID, CASES, fixture_bytes, measure
from w8_jpeg_reference import absolute_rgba, parse

ROOT = Path(__file__).resolve().parents[4]

BOUNDS = {"max_dimension": 2048, "max_pixels": 4194304, "max_source_depth": 8,
          "max_output_bgra32": 16 * 1024 * 1024, "max_declared_scratch": 16 * 1024 * 1024,
          "max_encoded_input": 16 * 1024 * 1024, "max_output_file": 32 * 1024 * 1024,
          "max_work_root": 256 * 1024 * 1024, "max_report": 8 * 1024 * 1024,
          "max_trace_records": 128, "max_trace_bytes": 128 * 1024,
          "max_capture_bytes": 64 * 1024, "max_worker_seconds": 45,
          "max_total_seconds": 300, "max_concurrent_workers": 1, "max_source_cases": 12,
          "max_decoder_legs": 24}
LIMITS = {"max_dimension": 2048, "max_pixels": 4194304, "max_bit_depth": 8,
          "max_decoded_bytes": 16 * 1024 * 1024, "max_scratch_bytes": 16 * 1024 * 1024}
SOURCE_FILES = (
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
    "JPEG absolute raster correctness for arbitrary sources",
    "progressive JPEG decode",
    "WIC internal scratch peak",
    "actual allocator reservations",
    "true concurrent allocation peak",
    "in-decoder cooperative cancellation",
    "GDI+ process lifetime and memory",
    "ICC colour fidelity",
    "EXIF orientation auto-application",
    "product decoder adoption",
    "product JPEG tolerance",
    "full MP-IMG2 Decode Exit",
)
CLASSIFICATIONS = ("DECODED_CONTENT_VERIFIED", "DECODED_DIFFERENTIAL_ONLY", "UNSUPPORTED",
                   "OVERSIZE", "CORRUPT_REFUSED", "TAMPER_NOT_VERIFIED", "TIMEOUT",
                   "CANCELLED", "UNVERIFIED_RACE", "NOT_RUN")
WIC_STAGES = ("process_observed", "input_opened", "metadata_available", "before_admission",
              "admission_started", "admitted", "before_onload", "onload_entered",
              "onload_returned", "after_onload", "conversion_entered", "conversion_returned",
              "copy_pixels_entered", "copy_pixels_returned", "encode_entered", "encode_returned",
              "before_publish", "output_published", "probe_exit")


def need(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum=None):
    if type(value) is not int:
        return False
    return True if minimum is None else value >= minimum


def requirements(case):
    precision = 8
    components = 1 if case["kind"] == "dc" else 3
    bits = precision * components
    row = (case["width"] * bits + 7) // 8
    decoded = row * case["height"]
    bgra = case["width"] * case["height"] * 4
    return {"width": case["width"], "height": case["height"], "bits_per_channel": precision,
            "bits_per_pixel": bits, "row_bytes": row, "decoded_bytes_required": decoded,
            "bgra32_bytes_required": bgra, "scratch_bytes_required": bgra,
            "declared_scratch_bytes": 0}


def canonical_compare(left_png: bytes, right_png: bytes, limit: int) -> dict:
    """Pixel statistics between two canonical PNGs, streamed row by row."""
    _need_bytes = need(isinstance(left_png, bytes) and isinstance(right_png, bytes),
                       "canonical_inputs_must_be_bytes")
    del _need_bytes
    with tempfile.TemporaryDirectory() as folder:
        left = Path(folder) / "left.png"
        right = Path(folder) / "right.png"
        left.write_bytes(left_png)
        right.write_bytes(right_png)
        rows = 0
        pixels = 0
        differing_pixels = 0
        differing_channels = 0
        total_difference = 0
        maximum = 0
        first = None
        left_rows = iter_png_rows(left, limit)
        right_rows = iter_png_rows(right, limit)
        while True:
            try:
                a = next(left_rows)
            except StopIteration:
                a = None
            try:
                b = next(right_rows)
            except StopIteration:
                b = None
            if a is None or b is None:
                need(a is None and b is None, "canonical_row_count_differs")
                break
            need(len(a) == len(b), "canonical_row_width_differs")
            row_differs = False
            for index in range(0, len(a), 4):
                if a[index:index + 4] != b[index:index + 4]:
                    differing_pixels += 1
                    row_differs = True
                    if first is None:
                        first = {"row": rows, "column": index // 4}
                        for channel in range(4):
                            if a[index + channel] != b[index + channel]:
                                first["channel"] = channel
                                first["left"] = a[index + channel]
                                first["right"] = b[index + channel]
                                break
                for channel in range(4):
                    delta = abs(a[index + channel] - b[index + channel])
                    if delta:
                        differing_channels += 1
                        total_difference += delta
                        maximum = max(maximum, delta)
            _ = row_differs
            pixels += len(a) // 4
            rows += 1
    return {"exact": differing_channels == 0, "pixels": pixels, "rows": rows,
            "differing_pixels": differing_pixels, "differing_channels": differing_channels,
            "maximum_channel_difference": maximum,
            "mean_channel_difference": (total_difference / differing_channels) if differing_channels else 0.0,
            "first_difference": first,
            "normalization": "both legs asked for canonical PNG; RGBA rows compared byte exactly"}


def _source_identity(record, source_root) -> bool:
    identity = record.get("source_identity")
    need(isinstance(identity, dict), "source_identity_missing")
    commit = identity.get("source_commit")
    need(isinstance(commit, str) and len(commit) == 40, "source_commit_invalid")
    files = identity.get("files")
    need(isinstance(files, dict) and set(files) == set(SOURCE_FILES), "source_file_set_invalid")
    for path, item in files.items():
        need(item.get("unchanged_after_run") is True, "source_changed_during_run:" + path)
    probes = identity.get("probes")
    need(isinstance(probes, dict) and set(probes) == {"wic", "gdiplus"}, "probe_identity_missing")
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


def _fixtures(record) -> dict:
    fixtures = record.get("fixtures")
    need(isinstance(fixtures, list) and len(fixtures) == len(CASES), "fixture_set_invalid")
    by_id = {item["case_id"]: item for item in fixtures}
    need(tuple(by_id) == tuple(case["id"] for case in CASES), "fixture_order_invalid")
    for case in CASES:
        item = by_id[case["id"]]
        if case["kind"] == "not_run":
            need(item["sha256"] is None and item["bytes"] == 0 and item["reference"] == "NOT_RUN",
                 "not_run_fixture_must_have_no_bytes:" + case["id"])
            continue
        measured = measure(case["id"])
        need(item["sha256"] == measured["sha256"], "fixture_regeneration_mismatch:" + case["id"])
        need(item["bytes"] == measured["bytes"] <= BOUNDS["max_encoded_input"], "fixture_bytes_invalid")
        need(item["width"] == case["width"] and item["height"] == case["height"],
             "fixture_dimensions_invalid:" + case["id"])
    return by_id


def _content(case_id: str) -> bytes:
    """Regenerate a fixture from the generator to feed the independent parse."""
    return fixture_bytes(case_id)


def _wic_leg(case, leg, fixture_bytes, expected_absolute):
    record = leg.get("leg")
    need(isinstance(record, dict), "wic_leg_missing:" + case["id"])
    result = record.get("probe_result")
    need(isinstance(result, dict), "wic_probe_result_missing")
    need(result.get("decoder") == "JpegBitmapDecoder", "wic_decoder_identity_invalid")
    need(record.get("limits") == LIMITS, "wic_limits_changed")
    need(result.get("admission_status") == "admitted", "wic_admission_not_recorded")
    values = requirements(case)
    for key in ("width", "height", "bits_per_channel", "bits_per_pixel", "row_bytes",
                "decoded_bytes_required", "bgra32_bytes_required", "scratch_bytes_required",
                "declared_scratch_bytes"):
        need(result.get(key) == values[key], "wic_requirement_mismatch:" + key)
    need(result.get("input_bytes") == len(fixture_bytes), "wic_input_bytes_mismatch")
    stages, _ = _trace(record)
    need([stage["stage"] for stage in stages] == list(WIC_STAGES), "wic_stage_sequence_invalid")
    need(record.get("trace_complete") is True and record.get("trace_partial_bytes") == 0,
         "wic_trace_incomplete")
    need(record.get("identity_verified") is True, "wic_identity_not_verified")
    need(isinstance(record.get("sample"), dict) and record["sample"].get("complete") is True,
         "wic_process_sample_missing")
    output = record.get("output") or {}
    need(output.get("present") is True and output.get("hex"), "wic_output_missing")
    data = bytes.fromhex(output["hex"])
    need(output["sha256"] == sha(data) and output["bytes"] == len(data), "wic_output_digest_mismatch")
    need(record.get("published_success") is True and record.get("output_present") is True,
         "wic_publication_inconsistent")
    return data


def _gdi_leg(case, leg):
    record = leg.get("leg")
    need(isinstance(record, dict), "gdiplus_leg_missing:" + case["id"])
    need(record.get("lifetime") == "NOT_MEASURED", "gdiplus_lifetime_must_stay_unmeasured")
    need(record.get("trace_utf8") is None and record.get("stages") == [],
         "gdiplus_must_not_fabricate_stages")
    result = record.get("probe_result")
    need(isinstance(result, dict), "gdiplus_probe_result_missing")
    need(result.get("decoder") == "Built-in JPEG Codec", "gdiplus_decoder_identity_invalid")
    output = record.get("output") or {}
    need(output.get("present") is True and output.get("hex"), "gdiplus_output_missing")
    data = bytes.fromhex(output["hex"])
    need(output["sha256"] == sha(data) and output["bytes"] == len(data), "gdiplus_output_digest_mismatch")
    return data


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
    tick = -1
    for index, stage in enumerate(stages, 1):
        need(stage.get("seq") == index and stage.get("stage") in WIC_STAGES, "stage_sequence_invalid")
        need((stage.get("pid"), stage.get("process_start_utc_ticks"))
             == (record.get("pid"), record.get("start_utc_ticks")), "stage_identity_invalid")
        need(integer(stage.get("qpc_ticks"), tick), "stage_clock_invalid")
        tick = stage["qpc_ticks"]
    return stages, data


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
        fixtures = _fixtures(record)
        source_verified = _source_identity(record, source_root)
        cases = record.get("cases")
        need(isinstance(cases, list) and len(cases) == len(CASES), "case_count_invalid")
        need([row.get("case_id") for row in cases] == [case["id"] for case in CASES],
             "case_order_invalid")
        absolute_verified = 0
        differential = 0
        not_verified = 0
        legs = 0
        for case, row in zip(CASES, cases):
            need(row.get("reference_class") == case["reference"], "reference_class_mismatch")
            if case["kind"] == "not_run":
                need(row.get("classification") == "NOT_RUN" and row.get("legs") == [],
                     "not_run_case_must_have_no_legs:" + case["id"])
                continue
            fixture_bytes = _content(case["id"])
            need(sha(fixture_bytes) == fixtures[case["id"]]["sha256"], "fixture_bytes_not_regenerated")
            need(row.get("classification") in CLASSIFICATIONS, "classification_vocabulary_invalid")
            expected = None
            if case["reference"] == "ABSOLUTE":
                expected = absolute_rgba(parse(fixture_bytes))
                need(row.get("absolute") == expected, "absolute_reference_mismatch:" + case["id"])
            needs_legs = case["kind"] in ("dc", "encoded")
            if needs_legs:
                legs_rows = row.get("legs")
                need(isinstance(legs_rows, list) and len(legs_rows) == 2, "decoder_leg_count_invalid")
                wic = _wic_leg(case, legs_rows[0], fixture_bytes, expected)
                gdi = _gdi_leg(case, legs_rows[1])
                legs += 2
                comparison = row.get("comparison")
                need(isinstance(comparison, dict) and "error" not in comparison,
                     "differential_comparison_missing:" + case["id"])
                recomputed = canonical_compare(wic, gdi, BOUNDS["max_report_bytes"])
                for key in ("exact", "pixels", "differing_pixels", "differing_channels",
                            "maximum_channel_difference", "first_difference"):
                    need(comparison.get(key) == recomputed[key],
                         "differential_statistics_mismatch:" + key + ":" + case["id"])
                need(abs(comparison.get("mean_channel_difference", -1)
                         - recomputed["mean_channel_difference"]) < 1e-9,
                     "differential_mean_mismatch:" + case["id"])
                if case["reference"] == "ABSOLUTE":
                    need(isinstance(expected, dict), "absolute_reference_missing:" + case["id"])
                    need(row.get("classification") == "DECODED_CONTENT_VERIFIED",
                         "absolute_case_not_verified:" + case["id"])
                    need(_rgba_digest(wic) == expected["rgba_sha256"],
                         "wic_absolute_digest_mismatch:" + case["id"])
                    need(_rgba_digest(gdi) == expected["rgba_sha256"],
                         "gdiplus_absolute_digest_mismatch:" + case["id"])
                    absolute_verified += 1
                else:
                    need(row.get("classification") == "DECODED_DIFFERENTIAL_ONLY",
                         "differential_case_misclassified:" + case["id"])
                    first_leg = row["legs"][0]
                    need(isinstance(first_leg, dict) and (first_leg.get("leg") or {}).get("output"),
                         "differential_output_missing")
                    differential += 1
            else:
                need(row.get("classification") == "TAMPER_NOT_VERIFIED",
                     "malformed_case_cannot_claim_content:" + case["id"])
                not_verified += 1
        need(integer(absolute_verified) and absolute_verified == 3, "absolute_reference_case_count_invalid")
        need(differential == 7, "differential_case_count_invalid")
        need(not_verified == 2, "malformed_case_count_invalid")
        need(legs == 20, "decoder_leg_total_invalid")
        return {"ok": True, "source_identity_verified": source_verified,
                "closure_eligible": bool(source_verified), "cases": len(cases),
                "absolute_verified": absolute_verified, "differential_only": differential,
                "tamper_not_verified": not_verified, "decoder_legs": legs,
                "fixture_regeneration_verified": True,
                "measurement_scope": "WIC process counters during an artificial hold; GDI+ lifetime, "
                                     "memory, internal scratch and reservations stay NOT_MEASURED"}
    except (OSError, ValueError, KeyError, TypeError, IndexError, json.JSONDecodeError) as exc:
        return {"ok": False, "error": type(exc).__name__, "reason": str(exc)[:200],
                "source_identity_verified": False, "closure_eligible": False}


def _rgba_digest(png_bytes: bytes) -> str:
    with tempfile.TemporaryDirectory() as folder:
        path = Path(folder) / "output.png"
        path.write_bytes(png_bytes)
        digest = hashlib.sha256()
        for row in iter_png_rows(path, BOUNDS["max_report"]):
            digest.update(row)
    return "sha256:" + digest.hexdigest()
