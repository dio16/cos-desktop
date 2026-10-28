"""W7 fixture determinism, independent oracle and admission arithmetic tests."""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w7_large_fixtures as fixtures
import w7_large_oracle as oracle
from w7_large_verify import (BOUNDS, LIMITS, refusal_expectation, requirements,
                             row_bytes)

ROOT = Path(__file__).resolve().parents[5]


class FixtureTests(unittest.TestCase):
    def test_case_set_is_fixed_and_bounded(self):
        self.assertEqual(len(fixtures.CASES), 12)
        self.assertEqual(len({case["id"] for case in fixtures.CASES}), 12)
        roles = [case["role"] for case in fixtures.CASES]
        self.assertEqual((roles.count("decode"), roles.count("refusal"),
                          roles.count("roi_refusal"), roles.count("corrupt")), (6, 2, 2, 2))
        for case in fixtures.CASES:
            self.assertLessEqual(max(case["width"], case["height"]), 16384)
            self.assertIn(case["codec"], ("png", "bmp24", "bgr565"))

    def test_small_fixtures_regenerate_byte_identically(self):
        for case in fixtures.CASES:
            if max(case["width"], case["height"]) > 2048:
                continue
            with self.subTest(case=case["id"]):
                first = fixtures.expected_sha256(case["id"])
                second = fixtures.expected_sha256(case["id"])
                self.assertEqual(first, second)
                with tempfile.TemporaryDirectory() as folder:
                    record = fixtures.write_fixture(case["id"], Path(folder) / "case.png")
                    self.assertEqual("sha256:" + record["sha256"], first)
                    self.assertEqual(record["bytes"], fixtures.expected_encoded_bytes(case["id"]))

    def test_large_encodings_stay_inside_the_input_bound(self):
        for case in fixtures.CASES:
            with self.subTest(case=case["id"]):
                self.assertLessEqual(fixtures.expected_encoded_bytes(case["id"]),
                                     fixtures.MAX_ENCODED_BYTES)

    def test_pattern_digest_matches_parsed_source_for_every_codec(self):
        for case in fixtures.CASES:
            if case["role"] != "decode" or case["width"] != 2048:
                continue
            with self.subTest(codec=case["codec"]):
                with tempfile.TemporaryDirectory() as folder:
                    path = Path(folder) / "case.bin"
                    fixtures.write_fixture(case["id"], path)
                    parsed = oracle.expected_digest(case, path, BOUNDS["max_report_bytes"])
                pattern = fixtures.expected_rgba_digest(case["id"])
                self.assertEqual(parsed["rgba_sha256"], pattern["rgba_sha256"])
                self.assertEqual(parsed["pixels"], case["width"] * case["height"])

    def test_corrupt_inputs_differ_from_their_source(self):
        for case in fixtures.CASES:
            if case["role"] != "corrupt":
                continue
            with self.subTest(case=case["id"]):
                source = fixtures.BY_ID[case["source"]]
                self.assertNotEqual(fixtures.expected_sha256(case["id"]),
                                    fixtures.expected_sha256(source["id"]))
                if case["damage"] == "truncated":
                    self.assertLess(fixtures.expected_encoded_bytes(case["id"]),
                                    fixtures.expected_encoded_bytes(source["id"]))


class OracleTests(unittest.TestCase):
    def test_crc_failure_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "case.png"
            fixtures.write_fixture("png_2048", path)
            data = bytearray(path.read_bytes())
            data[40] ^= 0xFF
            path.write_bytes(bytes(data))
            with self.assertRaises(ValueError):
                oracle.png_header(path, BOUNDS["max_report_bytes"])

    def test_truncated_stream_cannot_be_completed(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "case.png"
            fixtures.write_fixture("corrupt_4096_truncated", path)
            with self.assertRaises(ValueError):
                list(oracle.iter_png_rows(path, BOUNDS["max_report_bytes"]))

    def test_bgr565_masks_and_row_decode(self):
        case = fixtures.BY_ID["bgr565_2048"]
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "case.bin"
            fixtures.write_fixture(case["id"], path)
            rows = list(oracle.iter_bmp_rows(path))
        self.assertEqual(len(rows), case["height"])
        first = rows[0]
        self.assertEqual(len(first), case["width"] * 4)
        r, g, b = fixtures._pixel(0, 0)
        self.assertEqual(first[:4], bytes([(r >> 3) << 3 | (r >> 5),
                                           (g >> 2) << 2 | (g >> 4),
                                           (b >> 3) << 3 | (b >> 5), 0xFF]))

    def test_byte_addition_matches_modulo_arithmetic(self):
        for length in (1, 2, 3, 4095, 4096):
            left = bytes((index * 37) & 0xFF for index in range(length))
            right = bytes((index * 91 + 7) & 0xFF for index in range(length))
            self.assertEqual(oracle._add_bytes(left, right),
                             bytes((a + b) & 0xFF for a, b in zip(left, right)))


class ArithmeticTests(unittest.TestCase):
    def test_rows_and_requirements_are_exact(self):
        self.assertEqual(row_bytes("png", 4096), 12288)
        self.assertEqual(row_bytes("bgr565", 4097), (4097 * 16 + 7) // 8)
        values = requirements(fixtures.BY_ID["png_4096"])
        self.assertEqual(values["decoded_bytes_required"], 4096 * 4096 * 3)
        self.assertEqual(values["bgra32_bytes_required"], 67108864)
        self.assertEqual(values["scratch_bytes_required"], 67108864)

    def test_documented_refusal_order(self):
        self.assertEqual(refusal_expectation(fixtures.BY_ID["png_8192"]), "max_dimension_exceeded")
        self.assertEqual(refusal_expectation(fixtures.BY_ID["png_16384"]), "max_dimension_exceeded")
        self.assertIsNone(refusal_expectation(fixtures.BY_ID["png_4096"]))
        self.assertEqual(LIMITS["max_dimension"], 4096)
        self.assertEqual(LIMITS["max_pixels"], 16777216)


if __name__ == "__main__":
    unittest.main()
