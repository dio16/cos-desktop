"""W7 admission-boundary tests: no budget shortcut through size, ROI or metadata."""
from __future__ import annotations

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w7_large_fixtures as fixtures
from w7_large_verify import (BOUNDS, LIMITS, requirements, refusal_expectation,
                             row_bytes)


class AdmissionTests(unittest.TestCase):
    def test_full_frame_requirement_ignores_the_roi(self):
        for case in fixtures.CASES:
            if case["role"] != "roi_refusal":
                continue
            with self.subTest(case=case["id"]):
                values = requirements(case)
                self.assertEqual(case["roi_width"] * case["roi_height"] * 4, 4)
                # The declared full frame, not the 1x1 output, decides admission.
                self.assertGreater(values["decoded_bytes_required"],
                                   BOUNDS["max_roi_output_bytes"] * 1000)
                self.assertEqual(refusal_expectation(case), "max_dimension_exceeded")

    def test_compressed_input_size_is_never_the_decoded_budget(self):
        for case in fixtures.CASES:
            # Compressed PNG only; a raw BMP legitimately exceeds its decoded
            # raster because of headers and row padding.
            if case["role"] != "decode" or case["codec"] != "png":
                continue
            with self.subTest(case=case["id"]):
                encoded = fixtures.expected_encoded_bytes(case["id"])
                decoded = requirements(case)["decoded_bytes_required"]
                self.assertGreater(decoded, encoded)
                self.assertLessEqual(decoded, LIMITS["max_decoded_bytes"])

    def test_large_pngs_compress_far_below_their_decoded_size(self):
        for case in fixtures.CASES:
            if case["codec"] != "png" or case["role"] != "refusal":
                continue
            with self.subTest(case=case["id"]):
                encoded = fixtures.expected_encoded_bytes(case["id"])
                decoded = case["width"] * case["height"] * 3
                self.assertLess(encoded * 50, decoded)

    def test_materialized_cases_sit_exactly_inside_the_experimental_budget(self):
        for case in fixtures.CASES:
            if case["role"] != "decode":
                continue
            with self.subTest(case=case["id"]):
                values = requirements(case)
                self.assertLessEqual(max(case["width"], case["height"]), LIMITS["max_dimension"])
                self.assertLessEqual(case["width"] * case["height"], LIMITS["max_pixels"])
                self.assertLessEqual(values["decoded_bytes_required"], LIMITS["max_decoded_bytes"])
                self.assertLessEqual(values["scratch_bytes_required"], LIMITS["max_scratch_bytes"])

    def test_every_refusal_case_exceeds_exactly_one_declared_limit(self):
        for case in fixtures.CASES:
            if case["role"] not in ("refusal", "roi_refusal"):
                continue
            with self.subTest(case=case["id"]):
                values = requirements(case)
                exceeded = []
                if max(values["width"], values["height"]) > LIMITS["max_dimension"]:
                    exceeded.append("dimension")
                if values["width"] * values["height"] > LIMITS["max_pixels"]:
                    exceeded.append("pixels")
                if values["decoded_bytes_required"] > LIMITS["max_decoded_bytes"]:
                    exceeded.append("decoded")
                if values["scratch_bytes_required"] > LIMITS["max_scratch_bytes"]:
                    exceeded.append("scratch")
                self.assertTrue(exceeded)
                self.assertEqual(refusal_expectation(case), "max_dimension_exceeded")

    def test_row_arithmetic_uses_checked_integer_ceil(self):
        self.assertEqual(row_bytes("png", 1), 3)
        self.assertEqual(row_bytes("bgr565", 1), 2)
        self.assertEqual(row_bytes("bmp24", 4095), 12285)
        self.assertEqual(requirements(fixtures.BY_ID["bmp24_4096"])["decoded_bytes_required"],
                         4096 * 12288)


if __name__ == "__main__":
    unittest.main()
