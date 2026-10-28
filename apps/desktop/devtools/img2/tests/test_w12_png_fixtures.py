"""W12 matched-raster PNG fixture identity, oracle and equality tests."""

from __future__ import annotations

import hashlib
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w11_jpeg_fixtures as jpeg_fixtures
import w12_matched_oracle as matched
import w12_png_fixture as fixtures
import w12_png_oracle as oracle

SIGNATURE = b"\x89PNG\r\n\x1a\n"
FIXTURE_ID = "png_gray_matched_4k"
# Pinned from the fixed raster, fixed zlib level 6 and filter type 0.
EXPECTED_BYTES = 259602
EXPECTED_SHA256 = "784105420468e055ba712aaa7d16bf3a33dc82c32b8372951238c108ca8c53d4"
JPEG_SHA256 = "9ea35ad3a09de79119d8c8335092251cd4aa36bf9177ea4a0b3575e4500c16d6"
_INFLATED_BYTES = (4096 + 1) * 4096
_BAND_ROWS: dict[int, bytes] = {}


def _level(bx: int, by: int) -> int:
    return 48 + 20 * ((3 * bx + 5 * by + (bx ^ by)) % 9)


def _expected_row(y: int) -> bytes:
    band = y // 8
    cached = _BAND_ROWS.get(band)
    if cached is None:
        cached = b"".join(bytes((_level(bx, band),)) * 8 for bx in range(512))
        _BAND_ROWS[band] = cached
    return cached


def _chunk(tag: bytes, body: bytes) -> bytes:
    return (struct.pack(">I", len(body)) + tag + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))


def _build(row, width: int = 4096, height: int = 4096, depth: int = 8, color: int = 0,
           rows: int | None = None, filter_type: int = 0) -> bytes:
    """Stream synthetic PNG rows into bounded IDAT chunks."""
    parts = [SIGNATURE, _chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, depth, color, 0, 0, 0))]
    compressor = zlib.compressobj(6)
    pending = bytearray()
    total = height if rows is None else rows

    def flush(force: bool = False) -> None:
        data = compressor.compress(bytes(pending))
        pending.clear()
        if data:
            parts.append(_chunk(b"IDAT", data))
        if force:
            tail = compressor.flush()
            if tail:
                parts.append(_chunk(b"IDAT", tail))

    for index in range(total):
        pending.append(filter_type)
        pending.extend(row(index))
        if len(pending) >= (1 << 20):
            flush()
    flush(True)
    parts.append(_chunk(b"IEND", b""))
    return b"".join(parts)


def _rgba_png(width: int, height: int, x0: int, y0: int, bad: tuple[int, int] | None = None) -> bytes:
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            gray = _level((x0 + x) // 8, (y0 + y) // 8)
            pixel = (gray ^ 1, gray, gray, 255) if bad == (x, y) else (gray, gray, gray, 255)
            rows.extend(pixel)
    return (SIGNATURE
            + _chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + _chunk(b"IDAT", zlib.compress(bytes(rows), 6))
            + _chunk(b"IEND", b""))


def _split(data: bytes):
    chunks = []
    offset = 8
    while offset < len(data):
        length = struct.unpack_from(">I", data, offset)[0]
        tag = data[offset + 4:offset + 8]
        chunks.append((tag, data[offset + 8:offset + 8 + length]))
        offset += 12 + length
    return chunks


class FixtureTests(unittest.TestCase):
    def test_fixture_spec_and_bytes_are_fixed_and_bounded(self):
        self.assertEqual([(f["id"], f["width"], f["height"], f["codec"]) for f in fixtures.FIXTURES],
                         [(FIXTURE_ID, 4096, 4096, "png")])
        data = fixtures.fixture_bytes()
        self.assertIs(data, fixtures.fixture_bytes())
        self.assertEqual(len(data), EXPECTED_BYTES)
        self.assertEqual(hashlib.sha256(data).hexdigest(), EXPECTED_SHA256)
        self.assertLessEqual(len(data), fixtures.MAX_ENCODED)
        measured = fixtures.measure()
        self.assertEqual(measured, {"bytes": EXPECTED_BYTES, "sha256": "sha256:" + EXPECTED_SHA256})
        measured["bytes"] = 0
        self.assertEqual(fixtures.measure()["bytes"], EXPECTED_BYTES)

    def test_exclusive_create_and_bit_identical_regeneration(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "matched.png"
            record = fixtures.write_png(path)
            self.assertEqual(record["case_id"], FIXTURE_ID)
            self.assertEqual(record["bytes"], EXPECTED_BYTES)
            self.assertEqual(record["sha256"], EXPECTED_SHA256)
            self.assertEqual(path.read_bytes(), fixtures.fixture_bytes())
            before = path.read_bytes()
            with self.assertRaises(FileExistsError):
                fixtures.write_png(path)
            self.assertEqual(path.read_bytes(), before)
        self.assertEqual(fixtures.measure(), {"bytes": EXPECTED_BYTES, "sha256": "sha256:" + EXPECTED_SHA256})


class OracleTests(unittest.TestCase):
    def test_structure_and_row_stream_are_complete(self):
        data = fixtures.fixture_bytes()
        info = oracle.verify_png(data)
        self.assertEqual((info["width"], info["height"], info["depth"], info["color_type"]),
                         (4096, 4096, 8, 0))
        self.assertEqual(info["bytes"], len(data))
        self.assertEqual(info["sha256"], "sha256:" + EXPECTED_SHA256)
        self.assertEqual(info["rows"], 4096)
        self.assertEqual(info["inflated_bytes"], _INFLATED_BYTES)
        self.assertEqual(oracle.png_geometry(data),
                         {"width": 4096, "height": 4096, "depth": 8, "color_type": 0})

        rows = list(oracle.stream_png_rows(data))
        self.assertEqual(len(rows), 4096)
        self.assertTrue(all(len(row) == 4096 for row in rows))
        self.assertEqual(rows[0][:16], bytes((48,)) * 8 + bytes((128,)) * 8)
        self.assertEqual(rows[4095][4092:], bytes((88,)) * 4)

    def test_every_pixel_matches_the_fixed_formula(self):
        result = oracle.compare_all_pixels(fixtures.fixture_bytes())
        self.assertTrue(result["ok"])
        self.assertEqual(result["compared_pixels"], 4096 * 4096)
        self.assertIsNone(result["first_mismatch"])
        self.assertEqual(result["expected_sha256"], result["actual_sha256"])

    def test_single_pixel_mismatch_is_located(self):
        wrong = _build(row=lambda y: (_expected_row(y)[:-1] + bytes((_expected_row(y)[-1] ^ 1,)))
                       if y == 4095 else _expected_row(y))
        result = oracle.compare_all_pixels(wrong)
        self.assertFalse(result["ok"])
        self.assertEqual(result["compared_pixels"], 4096 * 4096)
        mismatch = result["first_mismatch"]
        self.assertEqual((mismatch["x"], mismatch["y"]), (4095, 4095))
        self.assertEqual(mismatch["expected"], 88)
        self.assertEqual(mismatch["actual"], 89)
        self.assertNotEqual(result["expected_sha256"], result["actual_sha256"])

    def test_wrong_dimension_depth_colour_and_method_rejected(self):
        dimension = _build(width=4095, row=lambda index: bytes(4095), rows=1)
        depth = _build(depth=16, row=lambda index: bytes(8192), rows=1)
        colour = _build(color=2, row=lambda index: bytes(12288), rows=1)
        for data, reason in ((dimension, "png_geometry_mismatch"),
                             (depth, "png_depth_unsupported"),
                             (colour, "png_colour_type_unsupported")):
            with self.subTest(reason=reason):
                with self.assertRaisesRegex(ValueError, reason):
                    oracle.compare_all_pixels(data)
                with self.assertRaisesRegex(ValueError, reason):
                    oracle.verify_png(data)

    def test_bad_crc_rejected(self):
        data = bytearray(fixtures.fixture_bytes())
        data[29] ^= 0xFF  # inside the IHDR CRC
        with self.assertRaisesRegex(ValueError, "png_crc_invalid"):
            oracle.compare_all_pixels(bytes(data))
        with self.assertRaisesRegex(ValueError, "png_crc_invalid"):
            oracle.verify_png(bytes(data))

    def test_missing_iend_and_trailing_bytes_rejected(self):
        data = fixtures.fixture_bytes()
        with self.assertRaisesRegex(ValueError, "png_iend_missing"):
            oracle.compare_all_pixels(data[:-12])
        with self.assertRaisesRegex(ValueError, "png_trailing_bytes"):
            oracle.compare_all_pixels(data + b"\x00")
        with self.assertRaisesRegex(ValueError, "png_trailing_bytes"):
            oracle.verify_png(data + b"\x00")

    def test_excess_inflated_payload_rejected(self):
        extra_row = _build(row=lambda index: bytes(4096), rows=4097)
        with self.assertRaisesRegex(ValueError, "png_inflate_excess"):
            oracle.compare_all_pixels(extra_row)
        with self.assertRaisesRegex(ValueError, "png_inflate_excess"):
            oracle.verify_png(extra_row)

    def test_missing_inflate_tail_rejected(self):
        chunks = _split(fixtures.fixture_bytes())
        index = max(i for i, (tag, body) in enumerate(chunks) if tag == b"IDAT" and len(body) > 32)
        rebuilt = [SIGNATURE]
        for position, (tag, body) in enumerate(chunks):
            if position == index:
                body = body[:-32]
            rebuilt.append(_chunk(tag, body))
        with self.assertRaisesRegex(ValueError, "png_inflate_incomplete"):
            oracle.compare_all_pixels(b"".join(rebuilt))

    def test_unknown_filter_type_rejected(self):
        filtered = _build(row=_expected_row, rows=1, filter_type=5)
        with self.assertRaisesRegex(ValueError, "png_filter_type_invalid"):
            oracle.compare_all_pixels(filtered)

    def test_roi_comparison_is_bounded_and_exact(self):
        roi = [117, 93, 256, 256]
        good = _rgba_png(256, 256, 117, 93)
        result = oracle.compare_roi(good, roi)
        self.assertTrue(result["ok"])
        self.assertEqual(result["compared_pixels"], 256 * 256)
        self.assertEqual(result["expected_sha256"], result["actual_sha256"])
        self.assertEqual((result["width"], result["height"]), (256, 256))
        self.assertEqual(result["expected_rgba_sha256"], result["expected_sha256"])
        self.assertEqual(result["actual_rgba_sha256"], result["actual_sha256"])

        # WIC may emit CRC-valid ancillary chunks; those must not be rejected.
        chunks = _split(good)
        annotated = (SIGNATURE + _chunk(*chunks[0]) + _chunk(b"tEXt", b"Comment\x00w12")
                     + b"".join(_chunk(tag, body) for tag, body in chunks[1:]))
        self.assertTrue(oracle.compare_roi(annotated, roi)["ok"])

        single = oracle.compare_roi(_rgba_png(1, 1, 17, 23), [17, 23, 1, 1])
        self.assertTrue(single["ok"])
        self.assertEqual(single["compared_pixels"], 1)

        bad = oracle.compare_roi(_rgba_png(256, 256, 117, 93, bad=(5, 7)), roi)
        self.assertFalse(bad["ok"])
        self.assertEqual((bad["first_mismatch"]["x"], bad["first_mismatch"]["y"]), (122, 100))

        for out_of_bounds in ([4095, 4095, 2, 1], [-1, 0, 1, 1], [0, -1, 1, 1],
                              [0, 0, 0, 1], [0, 0, 1, 0], [0, 0, 384, 384], [0, 0, 257, 1]):
            with self.subTest(roi=out_of_bounds):
                with self.assertRaisesRegex(ValueError, "roi_out_of_bounds"):
                    oracle.compare_roi(good, out_of_bounds)
        with self.assertRaisesRegex(ValueError, "roi_invalid"):
            oracle.compare_roi(good, (117, 93, 256, 256))  # type: ignore[arg-type]
        with self.assertRaisesRegex(ValueError, "png_geometry_mismatch"):
            oracle.compare_roi(good, [117, 93, 255, 256])

    def test_matched_jpeg_png_equality(self):
        jpeg = jpeg_fixtures.fixture_bytes("jpeg_gray_4k")
        self.assertEqual(hashlib.sha256(jpeg).hexdigest(), JPEG_SHA256)
        png = fixtures.fixture_bytes()
        result = matched.compare_jpeg_png_equality(jpeg, png)
        self.assertTrue(result["ok"])
        self.assertEqual(result["compared_pixels"], 4096 * 4096)
        self.assertEqual(result["expected_sha256"], result["actual_sha256"])
        self.assertEqual(result["jpeg_sha256"], "sha256:" + JPEG_SHA256)
        self.assertEqual(result["png_sha256"], "sha256:" + EXPECTED_SHA256)

        wrong = _build(row=lambda y: (_expected_row(y)[:-1] + bytes((_expected_row(y)[-1] ^ 1,)))
                       if y == 100 else _expected_row(y))
        mismatch = matched.compare_jpeg_png_equality(jpeg, wrong)
        self.assertFalse(mismatch["ok"])
        self.assertEqual((mismatch["first_mismatch"]["x"], mismatch["first_mismatch"]["y"]), (4095, 100))


if __name__ == "__main__":
    unittest.main()
