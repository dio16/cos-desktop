"""Matched-raster consumer tests: JPEG and PNG must agree pixel for pixel."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w12_matched_oracle as matched
from w12_png_fixture import fixture_bytes as png_bytes
from w12_png_oracle import compare_all_pixels, compare_roi, verify_png
from w11_jpeg_fixtures import fixture_bytes as jpeg_bytes
from w11_jpeg_oracle import verify_fixture as verify_jpeg

EVIDENCE = Path(__file__).resolve().parents[5] / "docs/development/evidence-published/w12-latest"


class W12MatchedRasterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.jpeg = jpeg_bytes("jpeg_gray_4k")
        cls.png = png_bytes()

    def test_both_fixtures_are_structurally_valid_and_bounded(self):
        jpeg = verify_jpeg("jpeg_gray_4k", self.jpeg)
        png = verify_png(self.png)
        self.assertEqual((jpeg["geometry"]["width"], jpeg["geometry"]["height"]), (4096, 4096))
        self.assertEqual(jpeg["geometry"]["bits_per_pixel"], 8)
        self.assertEqual((png["width"], png["height"], png["depth"], png["color_type"]),
                         (4096, 4096, 8, 0))
        self.assertEqual(png["rows"], 4096)
        self.assertEqual(png["inflated_bytes"], (4096 + 1) * 4096)
        self.assertLessEqual(len(self.jpeg), 16 * 1024 * 1024)
        self.assertLessEqual(len(self.png), 16 * 1024 * 1024)

    def test_every_png_pixel_matches_the_jpeg_pattern(self):
        result = compare_all_pixels(self.png)
        self.assertTrue(result["ok"])
        self.assertEqual(result["compared_pixels"], 4096 * 4096)
        self.assertIsNone(result["first_mismatch"])
        equality = matched.compare_jpeg_png_equality(self.jpeg, self.png)
        self.assertTrue(equality["ok"])
        self.assertEqual(equality["compared_pixels"], 4096 * 4096)
        self.assertEqual(equality["jpeg_sha256"], "sha256:" + hashlib.sha256(self.jpeg).hexdigest())

    def test_roi_comparison_covers_both_fixed_rectangles(self):
        control = EVIDENCE / "native-matched-matrix.json"
        if not control.is_file():
            raise ValueError("native_control_missing")
        record = json.loads(control.read_bytes())
        cases = {row["case_id"]: row for row in record["cases"]}
        for case_id in ("S01", "S03"):
            roi = cases[case_id]["roi"]
            with self.subTest(case_id=case_id, roi=roi):
                output = bytes.fromhex(cases[case_id]["workers"][0]["output_hex"])
                # The oracle is defined on bounded decoder ROI output, never on the
                # 4096-square source raster.
                result = compare_roi(output, roi)
                self.assertTrue(result["ok"])
                self.assertEqual(result["compared_pixels"], roi[2] * roi[3])

    def test_roi_bounds_are_enforced(self):
        control = json.loads((EVIDENCE / "native-matched-matrix.json").read_bytes())
        output = bytes.fromhex(control["cases"][2]["workers"][0]["output_hex"])
        for roi in ([0, 0, 257, 1], [-1, 0, 1, 1], [4095, 0, 2, 1], [0, 0, 0, 4]):
            with self.subTest(roi=roi):
                with self.assertRaises(ValueError):
                    compare_roi(output, roi)


if __name__ == "__main__":
    unittest.main()
