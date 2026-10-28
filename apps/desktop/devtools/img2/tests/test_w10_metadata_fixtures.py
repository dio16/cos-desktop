"""Hermetic W10 fixture and independent parser checks."""

from __future__ import annotations

import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import w10_metadata_fixtures as fixtures
import w10_metadata_reference as reference


class FixtureStructure(unittest.TestCase):
    def test_fixtures_are_deterministic(self):
        for case_id in fixtures.CASE_IDS:
            self.assertEqual(fixtures.fixture_bytes(case_id), fixtures.fixture_bytes(case_id))

    def test_every_fixture_is_a_real_baseline_stream(self):
        for case_id in fixtures.CASE_IDS:
            data = fixtures.fixture_bytes(case_id)
            self.assertEqual(data[:2], b"\xff\xd8")
            structure = reference.parse(data)
            self.assertIn("SOF0", structure["markers"])
            self.assertIn("SOS", structure["markers"])
            self.assertEqual(structure["unknown_markers"], [])

    def test_orientation_values_round_trip(self):
        for case_id, expected in (("exif_orient1", 1), ("exif_orient3", 3),
                                  ("exif_orient6", 6), ("exif_orient8", 8)):
            structure = reference.parse(fixtures.fixture_bytes(case_id))
            self.assertEqual(structure["exif"]["orientation"], expected)
            self.assertEqual(structure["classification"], "METADATA_PARSED")

    def test_expected_dims_follow_the_transform(self):
        expected = {"meta_plain": (256, 192), "exif_orient1": (256, 192),
                    "exif_orient3": (256, 192), "exif_orient6": (192, 256),
                    "exif_orient8": (192, 256), "icc_valid": (256, 192),
                    "exif_icc_combined": (192, 256)}
        for case_id, dims in expected.items():
            case = fixtures.case_by_id(case_id)
            self.assertEqual(fixtures.expected_dims(case), dims)

    def test_icc_header_is_self_consistent(self):
        structure = reference.parse(fixtures.fixture_bytes("icc_valid"))
        icc = structure["icc"]
        self.assertEqual(icc["bytes"], len(fixtures.ICC_PROFILE))
        self.assertEqual(icc["colour_space"], "RGB")
        self.assertEqual(struct.unpack(">I", fixtures.ICC_PROFILE[:4])[0], len(fixtures.ICC_PROFILE))
        self.assertEqual(fixtures.ICC_PROFILE[36:40], b"acsp")
        self.assertEqual(icc["sha256"], fixtures.sha(fixtures.ICC_PROFILE))

    def test_truncated_metadata_is_never_parsed_as_valid(self):
        for case_id in ("exif_truncated", "icc_truncated"):
            structure = reference.parse(fixtures.fixture_bytes(case_id))
            self.assertEqual(structure["classification"], "METADATA_TAMPERED")
        truncated = reference.parse(fixtures.fixture_bytes("icc_truncated"))
        self.assertIsNone(truncated["icc"])
        self.assertEqual(truncated["icc_partial"]["bytes"], 26)
        self.assertNotEqual(truncated["icc_partial"]["payload_sha256"], fixtures.sha(fixtures.ICC_PROFILE))

    def test_duplicate_segments_are_rejected(self):
        base = fixtures.fixture_bytes("exif_orient6")
        duplicated = base[:2] + fixtures.exif_app1(3) + base[2:]
        with self.assertRaises(ValueError):
            reference.parse(duplicated)

    def test_bad_icc_declared_size_is_rejected(self):
        broken = bytearray(fixtures.ICC_PROFILE)
        broken[0:4] = struct.pack(">I", len(fixtures.ICC_PROFILE) + 8)
        with self.assertRaises(ValueError):
            reference.parse_icc(b"ICC_PROFILE\x00\x01\x01" + bytes(broken))

    def test_bad_exif_signature_is_rejected(self):
        with self.assertRaises(ValueError):
            reference.parse_exif(b"Exif\x00\x01" + b"II*\x00\x08\x00\x00\x00\x00\x00")

    def test_write_fixture_reports_the_declared_contract(self):
        root = Path(__file__).resolve().parent / "_w10_fixture_probe"
        root.mkdir(exist_ok=True)
        try:
            record = fixtures.write_fixture("exif_icc_combined", root / "combined.jpg")
            self.assertEqual(record["expected_transform"], "rotate_90_cw")
            self.assertEqual((record["expected_output_width"], record["expected_output_height"]), (192, 256))
            self.assertTrue(record["expected_icc"])
            self.assertEqual(record["expected_icc_sha256"], fixtures.sha(fixtures.ICC_PROFILE))
            self.assertEqual(record["sha256"], fixtures.sha((root / "combined.jpg").read_bytes()))
        finally:
            for path in root.iterdir():
                path.unlink()
            root.rmdir()


if __name__ == "__main__":
    unittest.main()
