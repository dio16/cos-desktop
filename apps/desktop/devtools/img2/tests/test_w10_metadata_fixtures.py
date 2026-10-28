"""Hermetic W10 fixture and independent parser checks."""

from __future__ import annotations

import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import w10_metadata_fixtures as fixtures
import w10_metadata_reference as reference


class MetadataParsing(unittest.TestCase):
    def test_valid_exif_orientations_and_icc_are_parsed_from_encoded_bytes(self):
        expected = {"exif_orient1": 1, "exif_orient3": 3,
                    "exif_orient6": 6, "exif_orient8": 8}
        for case_id, orientation in expected.items():
            parsed = reference.parse(fixtures.fixture_bytes(case_id))
            self.assertEqual(parsed["exif"]["orientation"], orientation)
            self.assertEqual(parsed["classification"], "METADATA_PARSED")
        parsed_icc = reference.parse(fixtures.fixture_bytes("icc_valid"))
        self.assertEqual(parsed_icc["icc"]["bytes"], 540)
        self.assertEqual(parsed_icc["icc"]["sha256"], fixtures.sha(fixtures.ICC_PROFILE))
        combined = reference.parse(fixtures.fixture_bytes("exif_icc_combined"))
        self.assertEqual(combined["exif"]["orientation"], 6)
        self.assertEqual(combined["icc"]["bytes"], 540)
        self.assertEqual(reference.parse(fixtures.fixture_bytes("meta_plain"))["classification"],
                         "METADATA_ABSENT")

        for case_id in ("exif_truncated", "exif_invalid", "icc_truncated", "icc_invalid",
                        "icc_signature_invalid"):
            parsed = reference.parse(fixtures.fixture_bytes(case_id))
            self.assertEqual(parsed["classification"], "METADATA_TAMPERED", case_id)
        partial = reference.parse(fixtures.fixture_bytes("icc_truncated"))["icc_partial"]
        self.assertEqual(partial["bytes"], 26)
        self.assertEqual(partial["declared_size"], 540)
        self.assertNotEqual(partial["payload_sha256"], fixtures.sha(fixtures.ICC_PROFILE))

    def test_jpeg_segment_bounds_and_stream_completion_are_required(self):
        valid = fixtures.fixture_bytes("meta_plain")
        for broken in (valid[:-1], valid[:-2], valid[:2] + b"\xff\xe0\x00", valid + b"x"):
            with self.subTest(length=len(broken)), self.assertRaises(ValueError):
                reference.parse(broken)

    def test_exif_offsets_duplicates_and_orientation_values_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "exif_signature_missing"):
            reference.parse_exif(b"Exif\x00\x01" + b"II*\x00\x08\x00\x00\x00\x00\x00")
        with self.assertRaisesRegex(ValueError, "exif_orientation_value_invalid"):
            reference.parse_exif(b"Exif\x00\x00II*\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00")
        with self.assertRaisesRegex(ValueError, "exif_duplicate_tag"):
            tiff = (b"II" + struct.pack("<H", 42) + struct.pack("<I", 8) + struct.pack("<H", 2) +
                    struct.pack("<HHIHH", 0x0112, 3, 1, 1, 0) * 2 + struct.pack("<I", 0))
            reference.parse_exif(b"Exif\x00\x00" + tiff)
        offset_tiff = (b"II" + struct.pack("<H", 42) + struct.pack("<I", 8) + struct.pack("<H", 1) +
                       struct.pack("<HHI", 0x0112, 5, 1) + struct.pack("<I", 0xFFFFFFFF) +
                       struct.pack("<I", 0))
        with self.assertRaisesRegex(ValueError, "exif_value_offset_invalid"):
            reference.parse_exif(b"Exif\x00\x00" + offset_tiff)

    def test_icc_tag_table_bounds_and_overlap_are_rejected(self):
        body = b"ICC_PROFILE\x00\x01\x01" + fixtures.ICC_PROFILE
        with self.assertRaisesRegex(ValueError, "icc_tag_bounds_invalid"):
            broken = bytearray(body)
            broken[14 + 132 + 4:14 + 132 + 12] = struct.pack(">II", 0xFFFFFFFC, 20)
            reference.parse_icc(bytes(broken))
        with self.assertRaisesRegex(ValueError, "icc_declared_size_mismatch"):
            broken = bytearray(body)
            broken[14:18] = struct.pack(">I", len(fixtures.ICC_PROFILE) + 8)
            reference.parse_icc(bytes(broken))
        with self.assertRaisesRegex(ValueError, "icc_signature_invalid"):
            broken = bytearray(body)
            broken[14 + 36:14 + 40] = b"nope"
            reference.parse_icc(bytes(broken))
        overlap = bytearray(body)
        first_offset, first_size = struct.unpack_from(">II", overlap, 14 + 132 + 4)
        struct.pack_into(">II", overlap, 14 + 132 + 12 + 4, first_offset, first_size)
        with self.assertRaisesRegex(ValueError, "icc_tag_overlap"):
            reference.parse_icc(bytes(overlap))

    def test_write_fixture_uses_only_the_supplied_temporary_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "combined.jpg"
            record = fixtures.write_fixture("exif_icc_combined", path)
            self.assertEqual(record["case_id"], "exif_icc_combined")
            self.assertEqual(record["sha256"], fixtures.sha(path.read_bytes()))
            self.assertEqual(record["expected_icc_sha256"], fixtures.sha(fixtures.ICC_PROFILE))
            self.assertEqual((record["expected_output_width"], record["expected_output_height"]), (192, 256))


if __name__ == "__main__":
    unittest.main()
