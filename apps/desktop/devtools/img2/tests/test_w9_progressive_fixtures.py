"""W9 progressive fixture determinism and multi-scan marker tests."""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w9_progressive_fixtures as fixtures
import w9_progressive_reference as reference


class FixtureTests(unittest.TestCase):
    def test_case_set_is_fixed(self):
        self.assertEqual(len(fixtures.CASES), 5)
        kinds = [case["kind"] for case in fixtures.CASES]
        self.assertEqual((kinds.count("progressive"), kinds.count("malformed"),
                          kinds.count("control")), (2, 2, 1))
        self.assertEqual([case["width"] for case in fixtures.CASES if case["kind"] == "progressive"],
                         [256, 1024])

    def test_every_fixture_regenerates_byte_identically(self):
        for case in fixtures.CASES:
            with self.subTest(case=case["id"]):
                first = fixtures.measure(case["id"])
                second = fixtures.measure(case["id"])
                self.assertEqual(first, second)
                with tempfile.TemporaryDirectory() as folder:
                    record = fixtures.write_fixture(case["id"], Path(folder) / "case.jpg")
                    self.assertEqual("sha256:" + record["sha256"], first["sha256"])

    def test_encoded_sizes_stay_inside_the_input_bound(self):
        for case in fixtures.CASES:
            with self.subTest(case=case["id"]):
                self.assertLessEqual(fixtures.measure(case["id"])["bytes"], fixtures.MAX_ENCODED)


class MarkerTests(unittest.TestCase):
    def test_progressive_streams_are_valid_multi_scan(self):
        for case in fixtures.CASES:
            if case["kind"] != "progressive":
                continue
            with self.subTest(case=case["id"]):
                structure = reference.parse_scans(fixtures.fixture_bytes(case["id"]))
                self.assertEqual(structure["frame"]["variant"], "progressive_dct")
                self.assertEqual(structure["frame"]["precision"], 8)
                self.assertEqual((structure["frame"]["width"], structure["frame"]["height"]),
                                 (case["width"], case["height"]))
                classification = reference.classify(structure)
                self.assertEqual(classification["structure"], "VALID_PROGRESSIVE_STRUCTURE")
                self.assertGreaterEqual(classification["dc_scans"], 1)
                self.assertGreaterEqual(classification["ac_scans"], 1)
                self.assertEqual(classification["entropy_verified"], "NOT_VERIFIED")
                scans = structure["scans"]
                self.assertGreaterEqual(len(scans), 2)
                self.assertEqual((scans[0]["ss"], scans[0]["se"]), (0, 0))
                self.assertGreaterEqual(scans[1]["ss"], 1)

    def test_baseline_control_is_not_progressive(self):
        structure = reference.parse_scans(fixtures.fixture_bytes("baseline_control_420_256"))
        self.assertEqual(structure["frame"]["variant"], "baseline_dct")
        self.assertEqual(reference.classify(structure)["structure"], "BASELINE_CONTROL")

    def test_malformed_streams_fail_structure_parsing(self):
        for case in fixtures.CASES:
            if case["kind"] != "malformed":
                continue
            with self.subTest(case=case["id"]):
                with self.assertRaises(ValueError):
                    reference.parse_scans(fixtures.fixture_bytes(case["id"]))

    def test_progressive_constraints_reject_invalid_parameters(self):
        frame = {"variant": "progressive_dct"}
        self.assertTrue(reference.progressive_constraints(frame, 0, 0, 0, 0))
        self.assertFalse(reference.progressive_constraints(frame, 0, 1, 0, 0))
        self.assertFalse(reference.progressive_constraints(frame, 40, 63, 0, 0))
        self.assertFalse(reference.progressive_constraints(frame, 5, 2, 0, 0))

    def test_eoi_and_trailing_bytes_are_required(self):
        data = fixtures.fixture_bytes("progressive_gray_256")
        with self.assertRaises(ValueError):
            reference.parse_scans(data[:-2])
        with self.assertRaises(ValueError):
            reference.parse_scans(data + b"\x00")


if __name__ == "__main__":
    unittest.main()
