"""W6 fixture determinism, encoded layout, and independent raster checks."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w6_memory_fixtures as fixtures


def _png_from_rgba(width: int, height: int, rgba: bytes) -> bytes:
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            p = (y * width + x) * 4
            raw.extend(rgba[p:p + 3])

    def chunk(tag, body):
        return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) +
            chunk(b"IDAT", zlib.compress(bytes(raw))) + chunk(b"IEND", b""))


class W6FixtureTests(unittest.TestCase):
    def test_deterministic_generation_and_report_bounds(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            records_a = fixtures.generate(Path(a) / "cases")
            records_b = fixtures.generate(Path(b) / "cases")
            self.assertEqual(records_a, records_b)
            self.assertEqual([r["id"] for r in records_a], list(fixtures.FIXTURE_IDS))
            self.assertEqual(json.loads((Path(a) / "cases" / "manifest.json").read_text()), records_a)
            self.assertTrue(all(r["bytes"] <= 4 * 1024 * 1024 for r in records_a))
            self.assertTrue(all(len(r["sha256"]) == len(r["expected_rgba_sha256"]) == 64 for r in records_a))
            for record in records_a:
                data_a = (Path(a) / "cases" / record["path"]).read_bytes()
                data_b = (Path(b) / "cases" / record["path"]).read_bytes()
                self.assertEqual(data_a, data_b)
                self.assertEqual(hashlib.sha256(data_a).hexdigest(), record["sha256"])

    def test_source_layout_and_every_pixel_are_independently_checked(self):
        for record in fixtures.manifest():
            data = fixtures.fixture_bytes(record["id"])
            source = fixtures.source_rgba(record, data)
            self.assertEqual((source["width"], source["height"]), (record["width"], record["height"]))
            self.assertEqual(hashlib.sha256(source["rgba"]).hexdigest(), record["expected_rgba_sha256"])
            self.assertEqual(len(source["rgba"]), source["width"] * source["height"] * 4)
            if record["codec"] == "png":
                ihdr = struct.unpack_from(">IIBBBBB", data, 16)
                self.assertEqual(ihdr, (record["width"], record["height"], 8, 2, 0, 0, 0))
            else:
                fields = struct.unpack_from("<IiiHHII", data, 14)
                dib, width, signed_height, planes, depth, compression, image_size = fields
                self.assertEqual((dib, width, abs(signed_height), planes), (40, record["width"], record["height"], 1))
                self.assertEqual(depth, 24 if record["codec"] == "bmp24" else 16)
                self.assertEqual(compression, 0 if depth == 24 else 3)
                stride = (width * (depth // 8) + 3) & ~3
                self.assertEqual(struct.unpack_from("<I", data, 10)[0], 54 if depth == 24 else 66)
                self.assertEqual(len(data), (54 if depth == 24 else 66) + stride * record["height"])
                self.assertEqual(image_size, stride * record["height"])
                if depth == 16:
                    self.assertEqual(struct.unpack_from("<III", data, 54), (0xF800, 0x07E0, 0x001F))
            expected_png = _png_from_rgba(source["width"], source["height"], source["rgba"])
            result = fixtures.compare(record, data, expected_png)
            self.assertTrue(result["ok"], record["id"])
            self.assertEqual(result["mismatch_count"], 0)
            damaged = bytearray(expected_png)
            damaged[-5] ^= 1
            with self.assertRaises(ValueError):
                fixtures.compare(record, data, bytes(damaged))

    def test_rejects_unknown_ids_corrupt_sources_and_mismatched_output(self):
        with self.assertRaises(ValueError):
            fixtures.fixture_bytes("other")
        record = fixtures.manifest()[1]
        data = fixtures.fixture_bytes(record["id"])
        with self.assertRaises(ValueError):
            fixtures.source_rgba(record, data[:-1])
        wrong_record = dict(record, width=257)
        with self.assertRaises(ValueError):
            fixtures.source_rgba(wrong_record, data)
        source = fixtures.source_rgba(record, data)
        altered = bytearray(source["rgba"])
        altered[0] ^= 1
        result = fixtures.compare(record, data, _png_from_rgba(source["width"], source["height"], bytes(altered)))
        self.assertFalse(result["ok"])
        self.assertGreater(result["mismatch_count"], 0)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "exists"
            root.mkdir()
            with self.assertRaises(FileExistsError):
                fixtures.generate(root)


if __name__ == "__main__":
    unittest.main()
