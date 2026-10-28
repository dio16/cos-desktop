"""Fail-closed negative mutations for the W10 metadata verifier."""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from w10_metadata_verify import verify

RECORD = Path(__file__).resolve().parents[5] / "docs/development/evidence-published/w10-latest/native-metadata-matrix.json"
NEGATIVE_NAMES = (
    "orientation_value_forged", "orientation_presence_forged", "transform_request_forged",
    "transform_applied_forged", "transform_dims_swapped", "icc_presence_forged",
    "icc_length_forged", "icc_digest_forged", "truncated_icc_promoted", "truncated_icc_length_forged",
    "wic_colour_contexts_forged", "gdiplus_icc_on_absent", "structure_classification_flipped",
    "unknown_marker_added", "fixture_digest_forged", "fixture_bytes_forged", "leg_removed",
    "leg_order_swapped", "leg_case_binding_forged", "leg_candidate_binding_forged",
    "tamper_leg_promoted", "wellformed_leg_demoted", "refused_leg_publishes", "decoded_output_missing",
    "incomplete_run", "unmeasured_boundary_changed", "work_bytes_over_cap", "structure_not_reproducible",
    "expected_block_forged", "icc_partial_dropped",
    "png_bytes_forged", "rgba_digest_forged", "pixel_relation_forged",
    "input_digest_forged", "probe_digest_forged", "exit_outcome_forged",
    "cleanup_incomplete", "duplicate_case", "worker_not_reaped", "bound_omitted",
)


def _row(record, case_id):
    return next(row for row in record["cases"] if row["case_id"] == case_id)


def _leg(record, case_id, name):
    return next(leg for leg in _row(record, case_id)["legs"] if leg["leg"] == name)


def mutation(record, number):
    if number == 1:
        _leg(record, "exif_orient6", "wic_metadata")["probe_result"]["orientation_raw"] = 3
    elif number == 2:
        _leg(record, "meta_plain", "wic_metadata")["probe_result"]["orientation_present"] = True
    elif number == 3:
        _leg(record, "exif_orient6", "wic_transform")["probe_result"]["transform_requested"] = "explicit_rotate_270"
    elif number == 4:
        _leg(record, "exif_orient8", "wic_transform")["probe_result"]["transform_applied"] = "by_probe_no_rotation"
    elif number == 5:
        leg = _leg(record, "exif_orient6", "wic_transform")
        leg["probe_result"]["output_width"] = 256
        leg["probe_result"]["output_height"] = 192
    elif number == 6:
        _leg(record, "meta_plain", "gdiplus_metadata")["probe_result"]["icc_present"] = True
    elif number == 7:
        _leg(record, "icc_valid", "wic_metadata")["probe_result"]["icc_length"] = 512
    elif number == 8:
        _leg(record, "icc_valid", "gdiplus_metadata")["probe_result"]["icc_sha256"] = "sha256:" + "0" * 64
    elif number == 9:
        leg = _leg(record, "icc_truncated", "gdiplus_metadata")
        leg["probe_result"]["icc_sha256"] = _row(record, "icc_truncated")["expected"]["icc_sha256"]
    elif number == 10:
        _leg(record, "icc_truncated", "gdiplus_metadata")["probe_result"]["icc_length"] = 540
    elif number == 11:
        _leg(record, "icc_valid", "wic_metadata")["probe_result"]["colour_contexts"] = 2
    elif number == 12:
        _leg(record, "exif_orient6", "gdiplus_metadata")["probe_result"]["icc_present"] = True
    elif number == 13:
        row = _row(record, "exif_orient6")
        row["structure"]["classification"] = "METADATA_TAMPERED"
    elif number == 14:
        _row(record, "meta_plain")["structure"]["unknown_markers"] = ["XYZ"]
    elif number == 15:
        record["fixtures"][0]["sha256"] = "sha256:" + "0" * 64
    elif number == 16:
        record["fixtures"][0]["bytes"] = 1
    elif number == 17:
        row = _row(record, "meta_plain")
        row["legs"] = [leg for leg in row["legs"] if leg["leg"] != "gdiplus_metadata"]
    elif number == 18:
        row = _row(record, "icc_valid")
        row["legs"].reverse()
    elif number == 19:
        _leg(record, "icc_valid", "wic_metadata")["case_id"] = "meta_plain"
    elif number == 20:
        _leg(record, "icc_valid", "wic_metadata")["candidate"] = "gdiplus"
    elif number == 21:
        _leg(record, "icc_truncated", "wic_metadata")["classification"] = "DECODED_METADATA_OBSERVED"
    elif number == 22:
        _leg(record, "icc_valid", "wic_metadata")["classification"] = "DECODED_TAMPER_NOT_VERIFIED"
    elif number == 23:
        leg = _leg(record, "exif_truncated", "wic_metadata")
        leg["output"] = {"present": True, "bytes": 16, "sha256": "sha256:" + "0" * 64}
    elif number == 24:
        _leg(record, "icc_valid", "gdiplus_metadata")["output"] = {"present": False, "bytes": 0, "sha256": None}
    elif number == 25:
        record["complete"] = False
    elif number == 26:
        record["unmeasured"] = record["unmeasured"][:-1]
    elif number == 27:
        record["work_bytes"] = 10 ** 12
    elif number == 28:
        _row(record, "icc_valid")["structure"]["markers"] = ["SOI", "APP2"]
    elif number == 29:
        _row(record, "icc_valid")["expected"]["icc_present"] = False
    elif number == 30:
        _row(record, "icc_truncated")["structure"]["icc_partial"] = None
    elif number == 31:
        leg = _leg(record, "icc_valid", "gdiplus_metadata")
        leg["output"]["hex"] = "00" + leg["output"]["hex"][2:]
    elif number == 32:
        _leg(record, "icc_valid", "wic_metadata")["output"]["rgba_sha256"] = "sha256:" + "0" * 64
    elif number == 33:
        record["comparisons"]["exif_orient6"]["orientation_relation"]["exact"] = False
    elif number == 34:
        _leg(record, "meta_plain", "wic_metadata")["input_sha256"] = "sha256:" + "0" * 64
    elif number == 35:
        _leg(record, "meta_plain", "wic_metadata")["probe_sha256"] = "sha256:" + "0" * 64
    elif number == 36:
        _leg(record, "meta_plain", "wic_metadata")["exit_code"] = 3
    elif number == 37:
        record["cleanup"]["complete"] = False
    elif number == 38:
        record["cases"][1] = copy.deepcopy(record["cases"][0])
    elif number == 39:
        _leg(record, "meta_plain", "wic_metadata")["reaped"] = False
    elif number == 40:
        record["bounds"].pop("max_capture_bytes")
    else:
        raise ValueError("unknown mutation")
    # Keep the captured JSON coherent so mutations exercise semantic validation,
    # not merely a mismatch between two copies of the same result.
    for row in record["cases"]:
        for leg in row["legs"]:
            leg["stdout_utf8"] = json.dumps(leg["probe_result"], separators=(",", ":")) + "\n"
            leg["stdout_bytes"] = len(leg["stdout_utf8"].encode("utf-8"))


class NegativeMutations(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.record = json.loads(RECORD.read_text(encoding="utf-8"))
        result = verify(cls.record)
        if not result["ok"]:
            raise AssertionError("Retained Windows control is invalid: " + str(result))

    def test_retained_record_passes_structure_checks(self):
        result = verify(copy.deepcopy(self.record))
        self.assertTrue(result["ok"], result.get("reason"))

    def test_every_mutation_is_rejected(self):
        accepted = []
        for number, name in enumerate(NEGATIVE_NAMES, 1):
            mutated = copy.deepcopy(self.record)
            mutation(mutated, number)
            if verify(mutated)["ok"]:
                accepted.append((number, name))
        self.assertEqual(accepted, [])


if __name__ == "__main__":
    unittest.main()
