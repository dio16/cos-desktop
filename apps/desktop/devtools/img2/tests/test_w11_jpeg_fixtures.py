"""W11 streaming JPEG fixture identity and independent bounded-oracle tests."""
from __future__ import annotations

import hashlib
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w11_jpeg_fixtures as fixtures
import w11_jpeg_oracle as oracle

EXPECTED = {
    "jpeg_gray_4k": (4096, 4096, 589275, "9ea35ad3a09de79119d8c8335092251cd4aa36bf9177ea4a0b3575e4500c16d6"),
    "jpeg_gray_16k": (16384, 16384, 9424021, "5a12d606db03426128909796f8dcd16e836f04ac15a3484cdd19e5d8c41cb390"),
}


def _png(width: int, height: int, rgba: bytes) -> bytes:
    def chunk(tag: bytes, payload: bytes) -> bytes:
        return (struct.pack(">I", len(payload)) + tag + payload
                + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))
    rows = b"".join(b"\x00" + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def _scan_offset(data: bytes) -> int:
    pos = 2
    while data[pos:pos + 2] != b"\xff\xda":
        length = int.from_bytes(data[pos + 2:pos + 4], "big")
        pos += length + 2
    return pos + 2 + int.from_bytes(data[pos + 2:pos + 4], "big")


class W11FixtureTests(unittest.TestCase):
    def test_fixed_fixtures_are_bounded_and_deterministic(self):
        self.assertEqual([(f["id"], f["width"], f["height"], f["codec"]) for f in fixtures.FIXTURES],
                         [("jpeg_gray_4k", 4096, 4096, "jpeg"), ("jpeg_gray_16k", 16384, 16384, "jpeg")])
        for fixture_id, (width, height, size, digest) in EXPECTED.items():
            with self.subTest(fixture=fixture_id):
                metric = fixtures.measure(fixture_id)
                self.assertEqual(metric, {"bytes": size, "sha256": "sha256:" + digest})
                data = fixtures.fixture_bytes(fixture_id)
                self.assertLessEqual(len(data), fixtures.MAX_ENCODED)
                self.assertEqual(hashlib.sha256(data).hexdigest(), digest)
                record = oracle.verify_fixture(fixture_id, data)
                self.assertEqual(record["geometry"]["width"], width)
                self.assertEqual(record["geometry"]["height"], height)
                self.assertEqual(record["geometry"]["pixel_format"], "Gray8")
                self.assertEqual(record["geometry"]["decoder"], "JpegBitmapDecoder")

    def test_write_fixture_is_exclusive_and_matches_encoded_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source.jpg"
            record = fixtures.write_fixture("jpeg_gray_4k", path)
            self.assertEqual(path.stat().st_size, EXPECTED["jpeg_gray_4k"][2])
            self.assertEqual(record["sha256"], EXPECTED["jpeg_gray_4k"][3])
            with self.assertRaises(FileExistsError):
                fixtures.write_fixture("jpeg_gray_4k", path)

    def test_roi_expected_pixels_use_absolute_block_coordinates(self):
        data = fixtures.fixture_bytes("jpeg_gray_4k")
        roi = [15, 7, 18, 10]  # Crosses both an 8-pixel block edge and a block-row edge.
        expected = bytearray()
        for y in range(roi[1], roi[1] + roi[3]):
            for x in range(roi[0], roi[0] + roi[2]):
                gray = 48 + 20 * ((3 * (x // 8) + 5 * (y // 8) + ((x // 8) ^ (y // 8))) % 9)
                expected.extend((gray, gray, gray, 255))
        exact = oracle.compare_output(data, _png(roi[2], roi[3], bytes(expected)), roi)
        self.assertTrue(exact["ok"])
        self.assertEqual(exact["compared_pixels"], roi[2] * roi[3])
        altered = bytearray(expected)
        altered[4 * 3] ^= 1
        different = oracle.compare_output(data, _png(roi[2], roi[3], bytes(altered)), roi)
        self.assertFalse(different["ok"])
        self.assertEqual(different["first_mismatch"]["x"], roi[0] + 3)

    def test_truncation_table_pattern_and_roi_corruption_rejected(self):
        data = fixtures.fixture_bytes("jpeg_gray_4k")
        with self.assertRaises(ValueError):
            oracle.source_geometry(data[:-2])
        bad_table = bytearray(data)
        dqt = bad_table.find(b"\xff\xdb")
        bad_table[dqt + 5] ^= 1
        with self.assertRaises(ValueError):
            oracle.verify_fixture("jpeg_gray_4k", bytes(bad_table))
        changed_pattern = bytearray(data)
        entropy = _scan_offset(data)
        # Alter one DC amplitude bit, leaving frame, category and scan framing intact.
        changed_pattern[entropy] ^= 0x08
        with self.assertRaises(ValueError):
            oracle.verify_fixture("jpeg_gray_4k", bytes(changed_pattern))
        with self.assertRaises(ValueError):
            oracle.compare_output(data, b"not a png", [4095, 4095, 2, 1])

    def test_png_preflight_bounds_inflate_and_requires_complete_geometry(self):
        source = fixtures.fixture_bytes("jpeg_gray_4k")
        pixel = bytes((48, 48, 48, 255))
        valid = _png(1, 1, pixel)

        with self.assertRaises(ValueError):
            oracle.compare_output(source, valid[:-12], [0, 0, 1, 1])
        with self.assertRaises(ValueError):
            oracle.compare_output(source, valid + b"\x00", [0, 0, 1, 1])
        with self.assertRaises(ValueError):
            oracle.compare_output(source, _png(2, 1, pixel * 2), [0, 0, 1, 1])

        def chunk(tag: bytes, body: bytes) -> bytes:
            return (struct.pack(">I", len(body)) + tag + body
                    + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))
        excess_raw = b"\x00" + bytes(10000)
        excess = (b"\x89PNG\r\n\x1a\n"
                  + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0))
                  + chunk(b"IDAT", zlib.compress(excess_raw)) + chunk(b"IEND", b""))
        with self.assertRaises(ValueError):
            oracle.compare_output(source, excess, [0, 0, 1, 1])


if __name__ == "__main__":
    unittest.main()
