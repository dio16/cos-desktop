"""MP-TYP1-C: pure DPI layout matrix coverage (no GPU, no window, no desktop).

The matrix must be a real measurement of the accepted token source: these tests read
the repository tokens, check the ladder maths and the layout invariants, and prove the
CLI refuses a drifted, incomplete or non-finite token file with a closed code.
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import dpi_matrix as matrix  # noqa: E402

TOKENS = matrix.default_tokens_path()
SCALES = (1.00, 1.25, 1.50, 1.75, 2.00)


def run_cli(argv: list[str], tokens_path: Path) -> tuple[int, dict]:
    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        code = matrix.main(["--tokens", str(tokens_path), *argv])
    return code, json.loads(stdout.getvalue())


class TokenSourceTest(unittest.TestCase):
    def test_accepted_tokens_are_read_from_the_repository_source(self) -> None:
        tokens = matrix.load_tokens(TOKENS)
        self.assertEqual(set(tokens), set(matrix.REQUIRED_TOKENS))
        matrix.verify_accepted(tokens)  # the accepted set, not a copy of the harness
        self.assertEqual(tokens["TASK_RAIL_DIP"], 240.0)
        self.assertEqual(tokens["INSPECTOR_DIP"], 320.0)
        self.assertEqual(tokens["SEPARATOR_PHYSICAL_PIXELS"], 1.0)
        self.assertEqual(tokens["FOCUS_OUTLINE_DIP"], 2.0)
        self.assertEqual(tokens["TYPE_XS_LINE_HEIGHT_DIP"], 16.0)
        self.assertEqual(tokens["TYPE_DISPLAY_JAPANESE_LINE_HEIGHT_DIP"], 36.0)

    def _temp_tokens(self, text: str) -> Path:
        handle = tempfile.NamedTemporaryFile(
            "w", suffix=".rs", delete=False, encoding="utf-8", newline="\n"
        )
        self.addCleanup(Path(handle.name).unlink)
        handle.write(text)
        handle.close()
        return Path(handle.name)

    def test_missing_constant_is_a_closed_error(self) -> None:
        source = TOKENS.read_text(encoding="utf-8")
        trimmed = source.replace("pub const FOCUS_OUTLINE_DIP", "const FOCUS_OUTLINE_DIP")
        path = self._temp_tokens(trimmed)
        with self.assertRaises(matrix.MatrixError) as raised:
            matrix.load_tokens(path)
        self.assertEqual(raised.exception.code, "TOKENS_MISSING")

    def test_drifted_value_is_refused(self) -> None:
        source = TOKENS.read_text(encoding="utf-8")
        self.assertIn("pub const TASK_RAIL_DIP: f32 = 240.0;", source)
        drifted = source.replace(
            "pub const TASK_RAIL_DIP: f32 = 240.0;", "pub const TASK_RAIL_DIP: f32 = 260.0;"
        )
        path = self._temp_tokens(drifted)
        with self.assertRaises(matrix.MatrixError) as raised:
            matrix.verify_accepted(matrix.load_tokens(path))
        self.assertEqual(raised.exception.code, "TOKENS_DRIFT")

    def test_non_finite_or_zero_value_is_refused(self) -> None:
        source = TOKENS.read_text(encoding="utf-8")
        broken = source.replace(
            "pub const FOCUS_OUTLINE_DIP: f32 = 2.0;", "pub const FOCUS_OUTLINE_DIP: f32 = 0.0;"
        )
        with self.assertRaises(matrix.MatrixError) as raised:
            matrix.load_tokens(self._temp_tokens(broken))
        self.assertEqual(raised.exception.code, "TOKENS_INVALID")

    def test_absent_file_is_a_closed_error(self) -> None:
        with self.assertRaises(matrix.MatrixError) as raised:
            matrix.load_tokens(Path(tempfile.gettempdir()) / "no-such-tokens-file.rs")
        self.assertEqual(raised.exception.code, "TOKENS_READ")


class LadderTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.tokens = matrix.load_tokens(TOKENS)
        matrix.verify_accepted(cls.tokens)
        cls.rows = matrix.build_rows(cls.tokens)
        cls.checks = matrix.check_invariants(cls.rows)

    def test_the_ladder_is_the_declared_scale_set(self) -> None:
        self.assertEqual(tuple(row["scale"] for row in self.rows), SCALES)

    def test_separator_is_exactly_one_physical_pixel_at_every_scale(self) -> None:
        for row in self.rows:
            separator = row["separator"]
            self.assertAlmostEqual(separator["dip"] * row["scale"], 1.0, delta=1e-9)
            self.assertEqual(separator["physical_rounded"], 1)
            self.assertGreaterEqual(separator["physical"], 1.0 - 1e-9)

    def test_focus_outline_is_never_thinner_than_the_hairline(self) -> None:
        for row in self.rows:
            focus = row["focus_outline"]
            separator = row["separator"]
            self.assertEqual(focus["dip"], 2.0)
            self.assertGreaterEqual(focus["dip"], separator["dip"])
            self.assertGreater(focus["physical"], separator["physical"])
            self.assertEqual(focus["physical_rounded"], round(row["scale"] * 2.0 + 1e-9))

    def test_no_zero_or_negative_width(self) -> None:
        for row in self.rows:
            for key in ("separator", "focus_outline", "task_rail", "inspector"):
                self.assertGreater(row[key]["dip"], 0.0, f"{key} at {row['scale']}")
                self.assertGreater(row[key]["physical"], 0.0, f"{key} at {row['scale']}")
            for entry in row["line_heights"]:
                self.assertGreater(entry["size_dip"], 0.0, entry["token"])
                self.assertGreater(entry["latin"]["dip"], 0.0, entry["token"])
                self.assertGreater(entry["japanese"]["dip"], 0.0, entry["token"])

    def test_physical_widths_are_monotonic_across_the_scale_ladder(self) -> None:
        for key in ("focus_outline", "task_rail", "inspector"):
            values = [row[key]["physical"] for row in self.rows]
            for earlier, later in zip(values, values[1:]):
                self.assertGreaterEqual(later, earlier, key)
        for index in range(len(self.rows[0]["line_heights"])):
            for side in ("latin", "japanese"):
                values = [row["line_heights"][index][side]["physical"] for row in self.rows]
                for earlier, later in zip(values, values[1:]):
                    self.assertGreaterEqual(later, earlier, side)

    def test_japanese_line_heights_are_declared_but_not_used_by_the_adapter(self) -> None:
        first = self.rows[0]
        gaps = [entry["japanese_gap_dip"] for entry in first["line_heights"]]
        self.assertEqual(gaps, [2.0, 2.0, 1.0, 1.0, 0.0, 2.0, 4.0])
        for row in self.rows:
            for entry in row["line_heights"]:
                self.assertFalse(entry["japanese_used_by_adapter"], entry["token"])

    def test_every_layout_invariant_passes(self) -> None:
        failures = [check for check in self.checks if check["status"] != "PASS"]
        self.assertEqual(failures, [])
        self.assertFalse(matrix.failed(self.checks))

    def test_cli_writes_a_record_with_the_decomposed_matrix(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "dpi_matrix.json"
            code, summary = run_cli(["--out", str(out)], TOKENS)
            self.assertEqual(code, 0)
            self.assertTrue(summary["ok"])
            record = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(record["typography_matrix_version"], matrix.MATRIX_VERSION)
        self.assertEqual(record["scales"], list(SCALES))
        self.assertEqual(len(record["rows"]), len(SCALES))
        self.assertEqual(record["tokens"]["digest"][:7], "sha256:")
        self.assertFalse(record["adapter"]["japanese_line_height_used"])
        self.assertFalse(record["declares_mptyp1_complete"])
        self.assertTrue(record["not_measured"])
        for item in record["not_measured"]:
            self.assertEqual(set(item), {"item", "reason", "human_procedure"})
            self.assertTrue(item["reason"])
            self.assertTrue(item["human_procedure"])

    def test_cli_refuses_a_drifted_token_source(self) -> None:
        source = TOKENS.read_text(encoding="utf-8")
        drifted = source.replace(
            "pub const INSPECTOR_DIP: f32 = 320.0;", "pub const INSPECTOR_DIP: f32 = 300.0;"
        )
        handle = tempfile.NamedTemporaryFile(
            "w", suffix=".rs", delete=False, encoding="utf-8", newline="\n"
        )
        self.addCleanup(Path(handle.name).unlink)
        handle.write(drifted)
        handle.close()
        with tempfile.TemporaryDirectory() as directory:
            code, summary = run_cli(
                ["--out", str(Path(directory) / "matrix.json")], Path(handle.name)
            )
        self.assertEqual(code, 2)
        self.assertFalse(summary["ok"])
        self.assertEqual(summary["error"], "TOKENS_DRIFT")


if __name__ == "__main__":
    unittest.main()
