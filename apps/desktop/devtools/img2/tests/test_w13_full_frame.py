"""W13 consumer tests: full-frame output fidelity and truthful refusal."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w11_jpeg_fixtures import fixture_bytes as jpeg_bytes
from w12_png_fixture import fixture_bytes as png_bytes
from w13_full_oracle import (EXPECTED_RGBA_BYTES, PIXELS, expected_row, level, verify_full_output,
                             verify_input_pixels)

EVIDENCE = Path(__file__).resolve().parents[5] / "docs/development/evidence-published/w13-latest"


class W13FullFrameTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.jpeg = jpeg_bytes("jpeg_gray_4k")
        cls.png = png_bytes()

    def test_both_inputs_prove_all_pixels_equal(self):
        jpeg = verify_input_pixels("jpeg", self.jpeg)
        png = verify_input_pixels("png", self.png)
        self.assertEqual(jpeg["compared_pixels"], PIXELS)
        self.assertEqual(png["compared_pixels"], PIXELS)
        self.assertTrue(jpeg["all_pixels_match"] and png["all_pixels_match"])
        self.assertEqual(jpeg["canonical_rgba_sha256"], png["canonical_rgba_sha256"])
        self.assertEqual(jpeg["encoded_bytes"], 589275)
        self.assertEqual(png["encoded_bytes"], 259602)

    def test_expected_row_matches_the_fixed_formula(self):
        for y in (0, 7, 8, 4095):
            row = expected_row(y)
            self.assertEqual(len(row), 4096 * 4)
            self.assertEqual(row[0], level(0, y // 8))
            self.assertEqual(row[3], 255)
            self.assertEqual(row[-4], level(4095 // 8, y // 8))

    def test_retained_full_frame_outputs_verify_independently(self):
        control = EVIDENCE / "native-full-matrix.json"
        if not control.is_file():
            raise ValueError("native_control_missing")
        record = json.loads(control.read_bytes())
        decodes = [row for row in record["cases"] if row["role"] == "decode"]
        self.assertEqual(len(decodes), 2)
        digests = set()
        for row in decodes:
            worker = row["workers"][0]
            data = bytes.fromhex(worker["output_hex"])
            result = verify_full_output(data)
            self.assertTrue(result["ok"], msg=row["case_id"])
            self.assertEqual(result, worker["oracle"])
            self.assertEqual(result["compared_pixels"], PIXELS)
            self.assertEqual(result["rows"], 4096)
            self.assertEqual(result["inflated_rgba_bytes"], EXPECTED_RGBA_BYTES)
            self.assertEqual("sha256:" + hashlib.sha256(data).hexdigest(), worker["output_sha256"])
            digests.add(result["canonical_rgba_sha256"])
        self.assertEqual(len(digests), 1)

    def test_corrupted_output_is_rejected(self):
        record = json.loads((EVIDENCE / "native-full-matrix.json").read_bytes())
        data = bytearray(bytes.fromhex(record["cases"][0]["workers"][0]["output_hex"]))
        with self.assertRaises(ValueError):
            verify_full_output(bytes(data[:-12]))
        with self.assertRaises(ValueError):
            verify_full_output(bytes(data) + b"\x00")


if __name__ == "__main__":
    unittest.main()
