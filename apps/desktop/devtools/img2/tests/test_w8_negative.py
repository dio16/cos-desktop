"""The 32 W8 failure classes mutate the retained real Windows record."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w8_jpeg_verify import BOUNDS, LIMITS, verify

ROOT = Path(__file__).resolve().parents[5]
REPORT = ROOT / "docs/development/evidence-published/img2-w8-latest/native-decoder-matrix.json"

NEGATIVE_NAMES = (
    "source_width_tampered", "source_height_tampered", "sof_precision_tampered",
    "sampling_factor_tampered", "container_marker_tampered", "input_sha_tampered",
    "decoder_identity_tampered", "decoder_version_tampered", "decoded_requirement_understated",
    "bgra_requirement_understated", "encoded_as_decoded", "admission_stage_forged",
    "decoder_outcome_forged", "output_bytes_forged", "output_sha_forged",
    "unpublished_as_success", "expected_digest_tampered", "actual_digest_tampered",
    "differential_promoted_to_absolute", "agreement_as_absolute_proof", "mismatch_zeroed",
    "first_mismatch_tampered", "normalization_hidden", "icc_as_applied",
    "orientation_as_applied", "malformed_as_verified", "unsupported_as_decoded",
    "trace_tail_unparsed", "incomplete_capture_as_complete", "process_identity_swapped",
    "old_source_commit", "timeout_as_confirmed_cancel",
    "wic_outcome_flipped_corrupt", "gdiplus_outcome_flipped_error",
    "malformed_legs_removed", "malformed_output_forged",
    "malformed_worker_outcome_mismatch", "gdiplus_width_forged",
    "gdiplus_input_bytes_forged", "gdiplus_output_bytes_forged",
    "malformed_outcome_key_removed", "malformed_trace_payload_tampered",
    "gdiplus_output_bytes_removed",
)


def _case(record, case_id):
    return next(row for row in record["cases"] if row["case_id"] == case_id)


def _leg(record, case_id, candidate):
    row = _case(record, case_id)
    return next(leg for leg in row["legs"] if leg["candidate"] == candidate)


def _rewrite_trace(record):
    worker = _leg(record, "jpeg_420_256", "wic")["leg"]
    worker["trace_utf8"] = "".join(json.dumps(stage) + "\n" for stage in worker["stages"])
    raw = worker["trace_utf8"].encode("utf-8")
    worker["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
    worker["trace_bytes"] = len(raw)
    worker["trace_records"] = len(worker["stages"])
    worker["trace_partial_bytes"] = 0


def mutation(record, number):
    if number == 1:
        record["fixtures"][0]["width"] = 7
    elif number == 2:
        record["fixtures"][2]["height"] = 23
    elif number == 3:
        _case(record, "dc_gray_16x16")["structure"]["frame"]["precision"] = 12
    elif number == 4:
        frame = _case(record, "jpeg_420_256")["structure"]["frame"]
        frame["components"][0]["h"] = 1
    elif number == 5:
        _leg(record, "jpeg_444_256", "wic")["leg"]["probe_result"]["decoder"] = "UnknownJpeg"
    elif number == 6:
        record["fixtures"][3]["sha256"] = "sha256:" + "0" * 64
    elif number == 7:
        _leg(record, "dc_gray_8x8", "gdiplus")["leg"]["probe_result"]["decoder"] = "ExternalCodec"
    elif number == 8:
        record["source_identity"]["probes"]["wic"] = "sha256:" + "1" * 64
    elif number == 9:
        _leg(record, "dc_gray_16x16", "wic")["leg"]["probe_result"]["decoded_bytes_required"] = 1
    elif number == 10:
        _leg(record, "jpeg_420_256", "wic")["leg"]["probe_result"]["bgra32_bytes_required"] = 4
    elif number == 11:
        _leg(record, "jpeg_444_256", "wic")["leg"]["probe_result"]["decoded_bytes_required"] = 13627
    elif number == 12:
        _case(record, "dc_gray_32x24")["structure"]["frame"]["variant"] = "progressive_dct"
    elif number == 13:
        worker = _leg(record, "dc_gray_8x8", "wic")["leg"]
        worker["stages"][-1]["outcome"] = "oversize"
        _rewrite_trace(record)
    elif number == 14:
        _leg(record, "jpeg_420_1024", "wic")["leg"]["output"]["bytes"] += 1
    elif number == 15:
        _leg(record, "jpeg_444_1024", "gdiplus")["leg"]["output"]["sha256"] = "sha256:" + "2" * 64
    elif number == 16:
        worker = _leg(record, "jpeg_444_256", "wic")["leg"]
        worker["published_success"] = False
    elif number == 17:
        _case(record, "dc_gray_16x16")["absolute"]["rgba_sha256"] = "sha256:" + "3" * 64
    elif number == 18:
        _leg(record, "dc_gray_32x24", "wic")["leg"]["output"]["hex"] = \
            _leg(record, "dc_gray_32x24", "gdiplus")["leg"]["output"]["hex"]
    elif number == 19:
        _case(record, "jpeg_444_256")["classification"] = "DECODED_CONTENT_VERIFIED"
    elif number == 20:
        _case(record, "jpeg_420_1024")["reference_class"] = "ABSOLUTE"
    elif number == 21:
        comparison = _case(record, "jpeg_444_256")["comparison"]
        comparison["exact"] = False
        comparison["differing_pixels"] = 1
        comparison["differing_channels"] = 1
    elif number == 22:
        _case(record, "jpeg_420_256")["comparison"]["first_difference"] = {
            "row": 0, "column": 0, "channel": 0, "left": 0, "right": 1}
    elif number == 23:
        _case(record, "jpeg_444_1024")["comparison"]["normalization"] = "unknown"
    elif number == 24:
        record["unmeasured"] = [item for item in record["unmeasured"] if "ICC" not in item]
    elif number == 25:
        _case(record, "jpeg_420_256")["structure"]["frame"]["orientation_applied"] = True
    elif number == 26:
        _case(record, "malformed_truncated")["classification"] = "DECODED_CONTENT_VERIFIED"
    elif number == 27:
        _leg(record, "malformed_bad_length", "wic")["leg"]["probe_result"]["outcome"] = "decoded"
    elif number == 28:
        worker = _leg(record, "jpeg_420_256", "wic")["leg"]
        worker["trace_utf8"] += "garbage"
        raw = worker["trace_utf8"].encode("utf-8")
        worker["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        worker["trace_bytes"] = len(raw)
        worker["trace_partial_bytes"] = len(raw) - raw.rfind(b"\n") - 1
    elif number == 29:
        sample = _leg(record, "jpeg_420_256", "wic")["leg"]["sample"]
        sample["status"] = "complete"
        sample["failure"] = "observer_capture_incomplete"
    elif number == 30:
        wic = _leg(record, "jpeg_444_256", "wic")["leg"]
        other = _leg(record, "jpeg_420_256", "wic")["leg"]
        wic["pid"], other["pid"] = other["pid"], wic["pid"]
    elif number == 31:
        record["source_identity"]["source_commit"] = "f080f4c5987b9c5745ad42bfa2a8975e4747e5ed"
    elif number == 32:
        worker = _leg(record, "jpeg_420_256", "wic")["leg"]
        worker["outcome"] = "cancelled"
        worker["cancellation"] = dict(worker.get("cancellation") or {}, state="stop_confirmed")
    elif number == 33:
        worker = _leg(record, "dc_gray_8x8", "wic")["leg"]
        worker["outcome"] = "corrupt"
        worker["probe_result"]["outcome"] = "corrupt"
    elif number == 34:
        _leg(record, "dc_gray_16x16", "gdiplus")["leg"]["probe_result"]["outcome"] = "error"
    elif number == 35:
        _case(record, "malformed_truncated")["legs"] = []
    elif number == 36:
        leg = _leg(record, "malformed_truncated", "wic")["leg"]
        leg["output"]["sha256"] = "sha256:" + "5" * 64
    elif number == 37:
        _leg(record, "malformed_truncated", "wic")["leg"]["outcome"] = "error"
    elif number == 38:
        _leg(record, "dc_gray_8x8", "gdiplus")["leg"]["probe_result"]["width"] = 2048
    elif number == 39:
        _leg(record, "dc_gray_16x16", "gdiplus")["leg"]["probe_result"]["input_bytes"] = 1
    elif number == 40:
        _leg(record, "jpeg_420_256", "gdiplus")["leg"]["probe_result"]["output_bytes"] = 1
    elif number == 41:
        leg = _leg(record, "malformed_truncated", "wic")["leg"]
        leg.pop("outcome", None)
    elif number == 42:
        leg = _leg(record, "malformed_bad_length", "wic")["leg"]
        leg["stages"][-1]["outcome"] = "decoded"
        raw = "".join(json.dumps(stage) + "\n" for stage in leg["stages"]).encode("utf-8")
        leg["trace_utf8"] = raw.decode("utf-8")
        leg["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        leg["trace_bytes"] = len(raw)
        leg["trace_records"] = len(leg["stages"])
        leg["trace_partial_bytes"] = 0
    elif number == 43:
        _leg(record, "dc_gray_32x24", "gdiplus")["leg"]["probe_result"].pop("output_bytes", None)
    else:
        raise ValueError("unknown negative class")


class NativeNegativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = json.loads(REPORT.read_bytes())
        result = verify(cls.native, ROOT)
        if not result["ok"] or not result["closure_eligible"]:
            raise AssertionError("Real W8 control record is not eligible: " + str(result))
        cls.control = result

    def test_all_required_mutation_classes_are_rejected(self):
        for number, name in enumerate(NEGATIVE_NAMES, 1):
            with self.subTest(number=number, mutation=name):
                record = copy.deepcopy(self.native)
                mutation(record, number)
                self.assertFalse(verify(record, ROOT)["ok"], name)

    def test_control_record_reports_expected_totals(self):
        self.assertEqual(self.control["absolute_verified"], 3)
        self.assertEqual(self.control["differential_only"], 5)
        self.assertEqual(self.control["tamper_not_verified"], 2)
        self.assertEqual(self.control["decoder_legs"], 16)

    def test_structural_replay_does_not_claim_fixed_source_identity(self):
        result = verify(self.native)
        self.assertTrue(result["ok"])
        self.assertFalse(result["source_identity_verified"])
        self.assertFalse(result["closure_eligible"])

    def test_bounds_are_enforced_independently_of_reported_counts(self):
        record = copy.deepcopy(self.native)
        record["work_bytes"] = BOUNDS["max_work_root"] + 1
        self.assertFalse(verify(record, ROOT)["ok"])
        record = copy.deepcopy(self.native)
        record["duration_ns"] = (BOUNDS["max_total_seconds"] + 1) * 1_000_000_000
        self.assertFalse(verify(record, ROOT)["ok"])

    def test_limits_cannot_be_widened_to_accept_a_larger_source(self):
        record = copy.deepcopy(self.native)
        record["decode_limits"]["max_dimension"] = LIMITS["max_dimension"] * 2
        self.assertFalse(verify(record, ROOT)["ok"])


if __name__ == "__main__":
    unittest.main()
