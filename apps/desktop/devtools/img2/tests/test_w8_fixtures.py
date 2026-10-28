"""W8 fixture determinism and independent JPEG reference tests."""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w8_jpeg_fixtures as fixtures
import w8_jpeg_reference as reference

ROOT = Path(__file__).resolve().parents[5]


class FixtureTests(unittest.TestCase):
    def test_case_set_is_fixed(self):
        self.assertEqual(len(fixtures.CASES), 12)
        kinds = [case["kind"] for case in fixtures.CASES]
        self.assertEqual((kinds.count("dc"), kinds.count("encoded"), kinds.count("malformed"),
                          kinds.count("not_run")), (3, 5, 2, 2))
        absolute = [case for case in fixtures.CASES if case["reference"] == "ABSOLUTE"]
        self.assertEqual(len(absolute), 3)
        levels = set()
        for case in absolute:
            self.assertGreaterEqual(len(case["levels"]), 1)
            levels.update(case["levels"])
        self.assertGreaterEqual(len(levels), 4)

    def test_runnable_fixtures_regenerate_byte_identically(self):
        for case in fixtures.CASES:
            if case["kind"] == "not_run":
                self.assertIsNone(fixtures.measure(case["id"])["sha256"])
                continue
            with self.subTest(case=case["id"]):
                first = fixtures.measure(case["id"])
                second = fixtures.measure(case["id"])
                self.assertEqual(first, second)
                with tempfile.TemporaryDirectory() as folder:
                    record = fixtures.write_fixture(case["id"], Path(folder) / "case.jpg")
                    self.assertEqual("sha256:" + record["sha256"], first["sha256"])
                    self.assertEqual(record["bytes"], first["bytes"])

    def test_encoded_fixtures_stay_inside_the_input_bound(self):
        for case in fixtures.CASES:
            if case["kind"] == "not_run":
                continue
            with self.subTest(case=case["id"]):
                self.assertLessEqual(fixtures.measure(case["id"])["bytes"],
                                     fixtures.MAX_ENCODED)

    def test_malformed_inputs_are_derived_and_differ(self):
        for case in fixtures.CASES:
            if case["kind"] != "malformed":
                continue
            with self.subTest(case=case["id"]):
                self.assertNotEqual(fixtures.measure(case["id"])["sha256"],
                                    fixtures.measure(case["source"])["sha256"])
                with self.assertRaises(ValueError):
                    reference.parse(fixtures.fixture_bytes(case["id"]))


class ReferenceTests(unittest.TestCase):
    def test_marker_structure_of_each_encoding(self):
        for case in fixtures.CASES:
            if case["kind"] not in ("dc", "encoded"):
                continue
            with self.subTest(case=case["id"]):
                structure = reference.parse(fixtures.fixture_bytes(case["id"]))
                frame = structure["frame"]
                self.assertEqual(frame["variant"], "baseline_dct")
                self.assertEqual(frame["precision"], 8)
                self.assertEqual((frame["width"], frame["height"]),
                                 (case["width"], case["height"]))
                if case["kind"] == "dc":
                    self.assertEqual(frame["component_count"], 1)
                else:
                    self.assertEqual(frame["component_count"], 3)
                    sampling = (frame["components"][0]["h"], frame["components"][0]["v"])
                    self.assertEqual(sampling, (2, 2) if case["sampling"] == "420" else (1, 1))
                self.assertIn(0, structure["quant"])

    def test_dc_reference_matches_declared_levels(self):
        for case in fixtures.CASES:
            if case["kind"] != "dc":
                continue
            with self.subTest(case=case["id"]):
                structure = reference.parse(fixtures.fixture_bytes(case["id"]))
                result = reference.absolute_rgba(structure)
                self.assertEqual(set(result["distinct_levels"]), set(case["levels"]))
                self.assertEqual(result["pixels"], case["width"] * case["height"])

    def test_reference_is_not_taken_from_a_decoder(self):
        source = Path(reference.__file__).read_text(encoding="utf-8")
        for forbidden in ("wic_probe", "gdiplus_probe", "subprocess", "PIL", "cv2"):
            self.assertNotIn(forbidden, source)

    def test_canonical_comparison_counts_exact_differences(self):
        from w8_jpeg_verify import canonical_compare
        import struct, zlib
        def png(pixels, width, height):
            raw = b"".join(b"\x00" + pixels[y * width * 4:(y + 1) * width * 4] for y in range(height))
            def chunk(tag, body):
                return (struct.pack(">I", len(body)) + tag + body
                        + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))
            return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
                    + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))
        base = bytes([10, 20, 30, 255]) * 4
        other = bytearray(base)
        other[4] = 11
        left = png(base, 2, 2)
        right = png(bytes(other), 2, 2)
        same = canonical_compare(left, left, 8 * 1024 * 1024)
        self.assertTrue(same["exact"])
        self.assertEqual(same["differing_channels"], 0)
        different = canonical_compare(left, right, 8 * 1024 * 1024)
        self.assertFalse(different["exact"])
        self.assertEqual(different["differing_pixels"], 1)
        self.assertEqual(different["differing_channels"], 1)
        self.assertEqual(different["maximum_channel_difference"], 1)
        self.assertEqual(different["first_difference"],
                         {"row": 0, "column": 1, "channel": 0, "left": 10, "right": 11})

    def test_eoi_and_segment_bounds_are_required(self):
        data = fixtures.fixture_bytes("jpeg_444_256")
        with self.assertRaises(ValueError):
            reference.parse(data[:-2])
        tampered = bytearray(data)
        sos = tampered.find(b"\xff\xda")
        tampered[sos + 2:sos + 4] = b"\x7f\xff"
        with self.assertRaises(ValueError):
            reference.parse(bytes(tampered))


if __name__ == "__main__":
    unittest.main()
