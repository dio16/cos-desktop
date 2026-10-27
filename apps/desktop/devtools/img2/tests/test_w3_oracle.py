"""Independent consumer tests for the MP-IMG2-W3 BMP565 oracle."""
from __future__ import annotations

import struct
import sys
from pathlib import Path
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w3_oracle import bmp565, compare  # noqa: E402


def bmp(rows, *, top_down=False, masks=(0xF800, 0x07E0, 0x001F), planes=1,
        depth=16, compression=3, offset=66, dib=40, declared_size=None):
    height, width = len(rows), len(rows[0])
    stride = ((width * 2 + 3) // 4) * 4
    payload = bytearray()
    ordered = rows if top_down else list(reversed(rows))
    for row in ordered:
        payload.extend(b"".join(struct.pack("<H", p) for p in row))
        payload.extend(b"\xA5" * (stride - width * 2))
    head = struct.pack("<2sIHHI", b"BM", 0, 0, 0, offset)
    dh = struct.pack("<IiiHHIIiiII", dib, width, -height if top_down else height,
                     planes, depth, compression, len(payload), 0, 0, 0, 0)
    data = bytearray(head + dh + struct.pack("<III", *masks))
    if offset > len(data):
        data.extend(b"\0" * (offset - len(data)))
    data.extend(payload)
    struct.pack_into("<I", data, 2, len(data) if declared_size is None else declared_size)
    return bytes(data)


def png(width, height, rgba):
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
    raw = b"".join(b"\0" + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


class W3OracleTests(unittest.TestCase):
    def test_bottom_up_padded_rows_expand_every_pixel(self):
        source = bmp([[0xF800, 0x07E0, 0x001F], [0xFFFF, 0, 0x8410], [0x0001, 0x0800, 0x7BEF]])
        decoded = bmp565(source)
        expected = bytes((255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255,
                          255, 255, 255, 255, 0, 0, 0, 255, 132, 130, 132, 255,
                          0, 0, 8, 255, 8, 0, 0, 255, 123, 125, 123, 255))
        self.assertEqual(decoded["row_order"], "bottom_up")
        self.assertEqual(decoded["encoded_row_stride"], 8)
        self.assertEqual(decoded["rgba"], expected)
        result = compare(source, png(3, 3, expected))
        self.assertTrue(result["ok"])
        self.assertEqual(result["mismatch_count"], 0)

    def test_top_down_and_odd_width(self):
        source = bmp([[0xF800], [0x07E0], [0x001F]], top_down=True)
        decoded = bmp565(source)
        self.assertEqual(decoded["row_order"], "top_down")
        self.assertEqual(decoded["encoded_row_stride"], 4)
        self.assertEqual(decoded["rgba"], bytes((255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255)))

    def test_pixel_difference_is_reported_and_bounded(self):
        source = bmp([[0] * 5])
        actual = bytes((1, 2, 3, 4)) * 5
        report = compare(source, png(5, 1, actual))
        self.assertFalse(report["ok"])
        self.assertEqual(report["mismatch_count"], 20)
        self.assertEqual(len(report["mismatches"]), 16)
        self.assertEqual(report["mismatches"][0], {"x": 0, "y": 0, "channel": "r", "expected": 0, "actual": 1})

    def test_rejects_bad_headers_masks_sizes_and_truncation(self):
        valid = bmp([[0]])
        cases = [valid[:-1], bmp([[0]], masks=(0, 0x07E0, 0x001F)),
                 bmp([[0]], masks=(0xF800, 0xF800, 0x001F)),
                 bmp([[0]], masks=(0x7C00, 0x03E0, 0x001F)),
                 bmp([[0]], planes=0), bmp([[0]], depth=24),
                 bmp([[0]], compression=0xFFFFFFFF), bmp([[0]], dib=108),
                 bmp([[0]], declared_size=999), bmp([[0]], offset=65)]
        for data in cases:
            with self.subTest(data=data[:12]):
                with self.assertRaises(ValueError):
                    bmp565(data)
        with self.assertRaises(ValueError):
            compare(valid, b"\x89PNG\r\n\x1a\n")

    def test_malformed_png_crc_fails(self):
        source = bmp([[0]])
        output = bytearray(png(1, 1, bytes((0, 0, 0, 255))))
        output[output.index(b"IDAT") + 4] ^= 1
        with self.assertRaises(ValueError):
            compare(source, bytes(output))


if __name__ == "__main__":
    unittest.main()
