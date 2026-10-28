"""The 24 W7 failure classes mutate a retained real Windows record.

Every mutation is fed to the real verify() and must be rejected; a passing
control record proves the mutations are the only reason for that verdict.
"""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w7_large_verify import BOUNDS, LIMITS, verify

ROOT = Path(__file__).resolve().parents[5]
REPORT = ROOT / "docs/development/evidence-published/img2-w7-latest/native-large-matrix.json"

NEGATIVE_NAMES = (
    "declared_width_off_by_one", "source_format_swapped", "declared_bpp_understated",
    "encoded_as_decoded_bytes", "8192_promoted_to_admitted", "16384_promoted_to_admitted",
    "roi_reduces_full_frame", "admitted_budget_tampered", "refusal_gains_onload",
    "refusal_gains_output", "refusal_claims_published_success", "terminal_stage_mismatch",
    "fixture_digest_mismatch", "old_source_commit", "decoder_identity_forged",
    "oracle_expected_digest_altered", "decoded_trace_has_tail", "missing_output_as_success",
    "timeout_as_confirmed_cancel", "private_as_wic_scratch", "refusal_as_16k_capability",
    "preview_promoted", "incomplete_capture_as_complete", "bounds_exceeded",
)


def _case(record, case_id):
    return next(row for row in record["cases"] if row["case_id"] == case_id)


def _fixture(record, case_id):
    return next(item for item in record["fixtures"] if item["case_id"] == case_id)


def mutation(record, number):
    if number == 1:
        _fixture(record, "png_4096")["width"] = 4095
    elif number == 2:
        _fixture(record, "bmp24_4096")["codec"] = "bgr565"
    elif number == 3:
        admitted = next(stage for stage in _case(record, "png_4096")["worker"]["stages"]
                        if stage["stage"] == "admitted")
        admitted["bits_per_pixel"] = 8
    elif number == 4:
        worker = _case(record, "png_4096")["worker"]
        admitted = next(stage for stage in worker["stages"] if stage["stage"] == "admitted")
        admitted["decoded_bytes_required"] = admitted["input_bytes"]
    elif number == 5:
        _case(record, "png_8192")["classification"] = "DECODED_AND_VERIFIED"
        _case(record, "png_8192")["worker"]["outcome"] = "decoded"
    elif number == 6:
        _case(record, "png_16384")["classification"] = "DECODED_AND_VERIFIED"
        _case(record, "png_16384")["worker"]["outcome"] = "decoded"
    elif number == 7:
        result = _case(record, "roi_8192_1x1")["worker"]["probe_result"]
        result["full_frame_decoded_bytes"] = 4
    elif number == 8:
        admitted = next(stage for stage in _case(record, "png_2048")["worker"]["stages"]
                        if stage["stage"] == "admitted")
        admitted["decoded_bytes_required"] += 1
    elif number == 9:
        worker = _case(record, "png_8192")["worker"]
        worker["stages"].insert(len(worker["stages"]) - 1, {
            "schema_version": 1, "seq": 0, "stage": "onload_entered",
            "clock_domain": "windows_qpc", "observer": "wic_probe",
            "pid": worker["pid"], "process_start_utc_ticks": worker["start_utc_ticks"],
            "qpc_ticks": worker["stages"][-1]["qpc_ticks"], "qpc_frequency": 10_000_000})
        worker["trace_utf8"] = "".join(json.dumps(stage) + "\n" for stage in worker["stages"])
        raw = worker["trace_utf8"].encode("utf-8")
        worker["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        worker["trace_bytes"] = len(raw)
        worker["trace_records"] = len(worker["stages"])
    elif number == 10:
        worker = _case(record, "png_8192")["worker"]
        worker["output_present"] = True
        worker["output_bytes"] = 1
        worker["output_hex"] = "00"
    elif number == 11:
        _case(record, "png_16384")["worker"]["published_success"] = True
    elif number == 12:
        worker = _case(record, "png_4096")["worker"]
        worker["stages"][-1]["outcome"] = "oversize"
        worker["trace_utf8"] = "".join(json.dumps(stage) + "\n" for stage in worker["stages"])
        raw = worker["trace_utf8"].encode("utf-8")
        worker["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        worker["trace_bytes"] = len(raw)
    elif number == 13:
        _fixture(record, "png_2048")["sha256"] = "sha256:" + "0" * 64
    elif number == 14:
        record["source_identity"]["source_commit"] = "edfcdf104f66e89bf9deae6b03bde22427c84e9c"
    elif number == 15:
        _case(record, "png_2048")["worker"]["probe_result"]["decoder"] = "ThirdPartyDecoder"
    elif number == 16:
        _case(record, "bmp24_2048")["worker"]["oracle"]["expected_rgba_sha256"] = "sha256:" + "1" * 64
    elif number == 17:
        worker = _case(record, "bgr565_2048")["worker"]
        worker["trace_utf8"] += "garbage"
        raw = worker["trace_utf8"].encode("utf-8")
        worker["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        worker["trace_bytes"] = len(raw)
        worker["trace_partial_bytes"] = len(raw) - raw.rfind(b"\n") - 1
    elif number == 18:
        worker = _case(record, "png_2048")["worker"]
        worker["output_present"] = False
        worker["published_success"] = False
        worker["output_hex"] = None
    elif number == 19:
        worker = _case(record, "png_4096")["worker"]
        worker["outcome"] = "cancelled"
        worker["cancellation"]["state"] = "stop_confirmed"
    elif number == 20:
        record["allocation_scope"]["decoder_internal_scratch_peak"] = 1
    elif number == 21:
        record["decode_limits"]["max_dimension"] = 16384
    elif number == 22:
        record["allocation_scope"]["preview_after_refusal"] = "PREVIEW_AVAILABLE"
    elif number == 23:
        window = _case(record, "png_2048")["worker"]["sample"]
        window["status"] = "complete"
        window["failure"] = "observer_capture_incomplete"
    elif number == 24:
        record["duration_ns"] = (BOUNDS["max_total_seconds"] + 1) * 1_000_000_000
    else:
        raise ValueError("unknown negative class")


class NativeNegativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = json.loads(REPORT.read_bytes())
        result = verify(cls.native, ROOT)
        if not result["ok"] or not result["closure_eligible"]:
            raise AssertionError("Real W7 control record is not eligible: " + str(result))
        cls.control = result

    def test_all24_mutation_classes_are_rejected(self):
        for number, name in enumerate(NEGATIVE_NAMES, 1):
            with self.subTest(number=number, mutation=name):
                record = copy.deepcopy(self.native)
                mutation(record, number)
                self.assertFalse(verify(record, ROOT)["ok"], name)

    def test_control_record_reports_expected_totals(self):
        self.assertEqual(self.control["decoded_verified"], 6)
        self.assertEqual(self.control["refused"], 4)
        self.assertEqual(self.control["tamper_not_verified"], 2)
        self.assertTrue(self.control["source_identity_verified"])

    def test_structural_replay_does_not_claim_fixed_source_identity(self):
        result = verify(self.native)
        self.assertTrue(result["ok"])
        self.assertFalse(result["source_identity_verified"])
        self.assertFalse(result["closure_eligible"])

    def test_corrupt_cases_cannot_be_reclassified_as_verified_decodes(self):
        record = copy.deepcopy(self.native)
        _case(record, "corrupt_4096_truncated")["classification"] = "DECODED_AND_VERIFIED"
        self.assertFalse(verify(record, ROOT)["ok"])

    def test_refusal_cannot_skip_the_onload_boundary_check(self):
        record = copy.deepcopy(self.native)
        worker = _case(record, "png_16384")["worker"]
        worker["stages"] = [stage for stage in worker["stages"] if stage["stage"] != "refused"]
        worker["trace_utf8"] = "".join(json.dumps(stage) + "\n" for stage in worker["stages"])
        raw = worker["trace_utf8"].encode("utf-8")
        worker["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        worker["trace_bytes"] = len(raw)
        worker["trace_records"] = len(worker["stages"])
        self.assertFalse(verify(record, ROOT)["ok"])

    def test_bounds_are_enforced_independently_of_reported_counts(self):
        for field in ("work_bytes", "duration_ns"):
            with self.subTest(field=field):
                record = copy.deepcopy(self.native)
                if field == "work_bytes":
                    record[field] = BOUNDS["max_work_bytes"] + 1
                else:
                    record[field] = (BOUNDS["max_total_seconds"] + 1) * 1_000_000_000
                self.assertFalse(verify(record, ROOT)["ok"])

    def test_limits_cannot_be_widened_to_admit_a_large_source(self):
        record = copy.deepcopy(self.native)
        record["decode_limits"]["max_pixels"] = LIMITS["max_pixels"] * 4
        self.assertFalse(verify(record, ROOT)["ok"])


if __name__ == "__main__":
    unittest.main()
