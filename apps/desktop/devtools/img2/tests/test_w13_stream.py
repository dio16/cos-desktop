"""W13 adversarial PNG-stream tests: the strict reader must reject malformed zlib.

Each case takes the real retained 4096-square output, rebuilds a *structurally
valid* PNG (chunk lengths and CRCs recomputed) with a specific defect, and
requires rejection. This is what separates "all pixels matched" from "the
stream was complete and bounded".
"""
from __future__ import annotations

import json
from pathlib import Path
import struct
import sys
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w13_full_oracle import verify_full_output

EVIDENCE = Path(__file__).resolve().parents[5] / "docs/development/evidence-published/w13-latest"


def _chunk(tag: bytes, body: bytes) -> bytes:
    return (struct.pack(">I", len(body)) + tag + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))


def _parts(data: bytes):
    """Return (ihdr_body, compressed) from a valid PNG."""
    offset, ihdr, compressed = 8, None, bytearray()
    while offset < len(data):
        length = struct.unpack_from(">I", data, offset)[0]
        tag = data[offset + 4:offset + 8]
        body = data[offset + 8:offset + 8 + length]
        if tag == b"IHDR":
            ihdr = body
        elif tag == b"IDAT":
            compressed.extend(body)
        offset += 12 + length
    assert ihdr is not None
    return ihdr, bytes(compressed)


def _build(ihdr: bytes, compressed: bytes) -> bytes:
    return (b"\x89PNG\r\n\x1a\n" + _chunk(b"IHDR", ihdr)
            + _chunk(b"IDAT", compressed) + _chunk(b"IEND", b""))


class W13StreamStrictnessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        control = EVIDENCE / "native-full-matrix.json"
        if not control.is_file():
            raise ValueError("native_control_missing")
        record = json.loads(control.read_bytes())
        cls.png = bytes.fromhex(record["cases"][0]["workers"][0]["output_hex"])
        cls.ihdr, cls.compressed = _parts(cls.png)
        cls.raw = zlib.decompress(cls.compressed)

    def test_intact_stream_is_accepted(self):
        result = verify_full_output(self.png)
        self.assertTrue(result["ok"])
        self.assertEqual(result["rows"], 4096)
        self.assertEqual(result["inflated_scanline_bytes"], 67112960)
        self.assertEqual(result["compressed_bytes"], len(self.compressed))

    def test_missing_adler32_trailer_is_rejected(self):
        broken = _build(self.ihdr, self.compressed[:-4])
        with self.assertRaises(ValueError):
            verify_full_output(broken)

    def test_truncated_stream_is_rejected(self):
        broken = _build(self.ihdr, self.compressed[:len(self.compressed) // 2])
        with self.assertRaises(ValueError):
            verify_full_output(broken)

    def test_extra_compressed_bytes_after_stream_are_rejected(self):
        broken = _build(self.ihdr, self.compressed + zlib.compress(b"extra")[2:])
        with self.assertRaises(ValueError):
            verify_full_output(broken)

    def test_extra_inflated_data_is_rejected(self):
        extra = zlib.compressobj(6)
        payload = extra.compress(self.raw) + extra.compress(b"\x00" * (1 << 20)) + extra.flush()
        broken = _build(self.ihdr, payload)
        with self.assertRaises(ValueError):
            verify_full_output(broken)

    def test_oversized_inflation_is_rejected_before_matching(self):
        # A stream whose valid scanlines are followed by extra rows must fail even
        # though every one of the first 4096 rows is correct.
        extra_rows = self.raw + b"\x00" * (4096 * 4 + 1)
        broken = _build(self.ihdr, zlib.compress(extra_rows, 6))
        with self.assertRaises(ValueError):
            verify_full_output(broken)

    def test_wrong_ihdr_and_bad_crc_are_rejected(self):
        with self.assertRaises(ValueError):
            verify_full_output(self.png[:16] + struct.pack(">I", 4095) + self.png[20:])
        broken = bytearray(self.png)
        broken[30] ^= 0x01
        with self.assertRaises(ValueError):
            verify_full_output(bytes(broken))


if __name__ == "__main__":
    unittest.main()
