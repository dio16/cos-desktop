"""The 40 W9 failure classes mutate the retained real Windows record."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w9_progressive_verify import BOUNDS, LIMITS, verify

ROOT = Path(__file__).resolve().parents[5]
REPORT = ROOT / "docs/development/evidence-published/img2-w9-latest/native-decoder-matrix.json"

NEGATIVE_NAMES = (
    "sof2_to_sof0", "scan_count_forged", "sos_selector_changed", "spectral_selection_changed",
    "successive_approximation_changed", "scan_order_changed", "entropy_truncated", "eoi_removed",
    "segment_length_invalid", "fixture_sha_changed", "source_width_changed", "source_height_changed",
    "precision_changed", "sampling_changed", "encoded_as_decoded", "decoded_requirement_understated",
    "admission_forged", "candidate_changed", "decoder_identity_changed", "case_id_changed",
    "worker_outcome_changed", "probe_outcome_changed", "terminal_outcome_changed",
    "publication_forged", "output_bytes_changed", "output_sha_changed", "rgba_digest_changed",
    "differing_pixels_forged", "first_mismatch_forged", "differential_promoted_to_absolute",
    "malformed_promoted_to_content", "malformed_leg_removed", "malformed_outcome_key_removed",
    "malformed_output_digest_forged", "malformed_trace_tail", "trace_sha_mismatch",
    "incomplete_capture_as_complete", "process_identity_swapped", "old_source_commit",
    "stop_classification_forged",
)


def _case(record, case_id):
    return next(row for row in record["cases"] if row["case_id"] == case_id)


def _leg(record, case_id, candidate):
    return next(leg for leg in _case(record, case_id)["legs"] if leg["candidate"] == candidate)


def mutation(record, number):
    if number == 1:
        _case(record, "progressive_gray_256")["structure"]["frame"]["variant"] = "baseline_dct"
    elif number == 2:
        _case(record, "progressive_gray_1024")["structure"]["classification"]["scans"] = 9
    elif number == 3:
        _case(record, "progressive_gray_256")["structure"]["scans"][1]["components"][0]["id"] = 7
    elif number == 4:
        _case(record, "progressive_gray_256")["structure"]["scans"][1]["ss"] = 40
    elif number == 5:
        _case(record, "progressive_gray_1024")["structure"]["scans"][0]["al"] = 3
    elif number == 6:
        scans = _case(record, "progressive_gray_256")["structure"]["scans"]
        scans.reverse()
    elif number == 7:
        _case(record, "progressive_gray_256")["structure"]["scans"][1]["entropy_bytes"] -= 100
    elif number == 8:
        _case(record, "progressive_gray_1024")["structure"]["markers"] = [216, 219, 194, 196, 218]
    elif number == 9:
        _case(record, "progressive_gray_256")["structure"]["scans"][0]["entropy_offset"] += 4
    elif number == 10:
        record["fixtures"][0]["sha256"] = "sha256:" + "0" * 64
    elif number == 11:
        record["fixtures"][0]["width"] = 32
    elif number == 12:
        record["fixtures"][1]["height"] = 1023
    elif number == 13:
        _case(record, "progressive_gray_256")["structure"]["frame"]["precision"] = 12
    elif number == 14:
        _case(record, "progressive_gray_1024")["structure"]["frame"]["components"][0]["sampling"] = 0x22
    elif number == 15:
        _leg(record, "progressive_gray_256", "wic")["leg"]["probe_result"]["decoded_bytes_required"] = 9827
    elif number == 16:
        _leg(record, "progressive_gray_1024", "wic")["leg"]["probe_result"]["decoded_bytes_required"] = 1
    elif number == 17:
        _case(record, "progressive_gray_256")["structure"]["classification"]["structure"] = "VALID_PROGRESSIVE_STRUCTURE"
        _leg(record, "progressive_gray_256", "wic")["leg"]["probe_result"]["admission_status"] = "not_enforced"
    elif number == 18:
        _leg(record, "progressive_gray_256", "wic")["candidate"] = "gdiplus"
    elif number == 19:
        _leg(record, "progressive_gray_1024", "gdiplus")["leg"]["probe_result"]["decoder"] = "UnknownJpeg"
    elif number == 20:
        _leg(record, "progressive_gray_256", "wic")["case_id"] = "other"
    elif number == 21:
        _leg(record, "progressive_gray_256", "wic")["leg"]["outcome"] = "corrupt"
        _leg(record, "progressive_gray_256", "wic")["leg"]["probe_result"]["outcome"] = "corrupt"
    elif number == 22:
        _leg(record, "progressive_gray_1024", "gdiplus")["leg"]["probe_result"]["outcome"] = "error"
    elif number == 23:
        leg = _leg(record, "progressive_gray_256", "wic")["leg"]
        leg["stages"][-1]["outcome"] = "oversize"
        raw = "".join(json.dumps(stage) + "\n" for stage in leg["stages"]).encode("utf-8")
        leg["trace_utf8"] = raw.decode("utf-8")
        leg["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        leg["trace_bytes"] = len(raw)
    elif number == 24:
        _leg(record, "progressive_gray_1024", "wic")["leg"]["published_success"] = False
    elif number == 25:
        _leg(record, "progressive_gray_256", "wic")["leg"]["output"]["bytes"] += 1
    elif number == 26:
        _leg(record, "progressive_gray_1024", "gdiplus")["leg"]["output"]["sha256"] = "sha256:" + "1" * 64
    elif number == 27:
        # RGBA cannot be forged without breaking the canonical recomputation below.
        _case(record, "progressive_gray_256")["comparison"]["pixels"] += 1
    elif number == 28:
        _case(record, "progressive_gray_256")["comparison"]["differing_pixels"] = 3
    elif number == 29:
        _case(record, "progressive_gray_1024")["comparison"]["first_difference"] = {
            "row": 0, "column": 0, "channel": 0, "left": 0, "right": 1}
    elif number == 30:
        _case(record, "progressive_gray_256")["reference_class"] = "ABSOLUTE"
    elif number == 31:
        _case(record, "malformed_scan_truncated")["classification"] = "DIFFERENTIAL_VERIFIED"
    elif number == 32:
        _case(record, "malformed_spectral_invalid")["legs"] = []
    elif number == 33:
        _leg(record, "malformed_scan_truncated", "wic")["leg"].pop("outcome", None)
    elif number == 34:
        _leg(record, "malformed_scan_truncated", "wic")["leg"]["output"]["sha256"] = "sha256:" + "2" * 64
    elif number == 35:
        leg = _leg(record, "malformed_scan_truncated", "wic")["leg"]
        leg["trace_utf8"] += "garbage"
        raw = leg["trace_utf8"].encode("utf-8")
        leg["trace_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        leg["trace_bytes"] = len(raw)
        leg["trace_partial_bytes"] = len(raw) - raw.rfind(b"\n") - 1
    elif number == 36:
        _leg(record, "progressive_gray_256", "wic")["leg"]["trace_sha256"] = "sha256:" + "3" * 64
    elif number == 37:
        sample = _leg(record, "progressive_gray_1024", "wic")["leg"]["sample"]
        sample["status"] = "complete"
        sample["failure"] = "observer_capture_incomplete"
    elif number == 38:
        wic = _leg(record, "progressive_gray_256", "wic")["leg"]
        other = _leg(record, "progressive_gray_1024", "wic")["leg"]
        wic["pid"], other["pid"] = other["pid"], wic["pid"]
    elif number == 39:
        record["source_identity"]["source_commit"] = "05ea4d10ef6a24ce3a710d3de7365c023c60e838"
    elif number == 40:
        worker = _leg(record, "progressive_gray_256", "wic")["leg"]
        worker["outcome"] = "cancelled"
        worker["cancellation"] = dict(worker.get("cancellation") or {}, state="stop_confirmed")
    else:
        raise ValueError("unknown negative class")


class ProgressiveNegativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = json.loads(REPORT.read_bytes())
        result = verify(cls.native, ROOT)
        if not result["ok"] or not result["closure_eligible"]:
            raise AssertionError("Real W9 control record is not eligible: " + str(result))
        cls.control = result

    def test_all40_mutation_classes_are_rejected(self):
        for number, name in enumerate(NEGATIVE_NAMES, 1):
            with self.subTest(number=number, mutation=name):
                record = copy.deepcopy(self.native)
                mutation(record, number)
                self.assertFalse(verify(record, ROOT)["ok"], name)

    def test_control_record_reports_expected_totals(self):
        self.assertEqual(self.control["progressive_verified"], 2)
        self.assertEqual(self.control["malformed"], 2)
        self.assertEqual(self.control["control"], 1)
        self.assertEqual(self.control["decoder_legs"], 10)

    def test_structural_replay_does_not_claim_fixed_source_identity(self):
        result = verify(self.native)
        self.assertTrue(result["ok"])
        self.assertFalse(result["source_identity_verified"])
        self.assertFalse(result["closure_eligible"])

    def test_bounds_and_limits_are_enforced(self):
        record = copy.deepcopy(self.native)
        record["work_bytes"] = BOUNDS["max_work_root"] + 1
        self.assertFalse(verify(record, ROOT)["ok"])
        record = copy.deepcopy(self.native)
        record["decode_limits"]["max_pixels"] = LIMITS["max_pixels"] * 2
        self.assertFalse(verify(record, ROOT)["ok"])


if __name__ == "__main__":
    unittest.main()
