"""W4 fixture determinism and independent source-oracle identity."""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import w4_fixtures
import w4_region_oracle as oracle


class W4FixtureTests(unittest.TestCase):
    def test_generation_is_deterministic_and_bounded(self):
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            a = w4_fixtures.generate(Path(first) / "cases")
            b = w4_fixtures.generate(Path(second) / "cases")
            self.assertEqual(len(a), 9)
            self.assertEqual([(f["id"], f["sha256"], f["bytes"]) for f in a],
                             [(f["id"], f["sha256"], f["bytes"]) for f in b])
            self.assertTrue(all(f["bytes"] <= 1024 * 1024 for f in a))

    def test_independent_sources_reconstruct_expected_rgba(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = w4_fixtures.generate(Path(tmp) / "cases")
            for record in records:
                if "expected_rgba_hex" not in record:
                    continue
                data = (Path(tmp) / "cases" / record["path"]).read_bytes()
                source = oracle.source_rgba(record, data)
                self.assertEqual((source["width"], source["height"]),
                                 (record["width"], record["height"]))
                self.assertEqual(source["rgba"].hex(), record["expected_rgba_hex"],
                                 record["id"])

    def test_roi_slice_is_row_exact(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = {f["id"]: f for f in w4_fixtures.generate(Path(tmp) / "cases")}
            record = records["png_rgb"]
            data = (Path(tmp) / "cases" / record["path"]).read_bytes()
            source = oracle.source_rgba(record, data)
            pixels = oracle.slice_rgba(source, 2, 1, 3, 2)
            self.assertEqual(len(pixels), 3 * 2 * 4)
            self.assertEqual(pixels[:4], source["rgba"][(1 * 7 + 2) * 4:(1 * 7 + 2) * 4 + 4])
            self.assertEqual(pixels[-4:], source["rgba"][(2 * 7 + 4) * 4:(2 * 7 + 4) * 4 + 4])

    def test_bgr555_control_is_not_certified_as_565(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = {f["id"]: f for f in w4_fixtures.generate(Path(tmp) / "cases")}
            data = (Path(tmp) / "cases" / records["bmp555_control"]["path"]).read_bytes()
            result = oracle.source_rgba(records["bmp555_control"], data)
            self.assertEqual((result["width"], result["height"]), (3, 3))
            self.assertIn("5/5/5", result["method"])


if __name__ == "__main__":
    unittest.main()
