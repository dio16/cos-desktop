"""MP-TYP1-C: pure coverage for the host observation helper.

No window is launched and no desktop is required: these tests cover the parts that
can be established deterministically — PNG IHDR parsing, host-scale arithmetic, the
label analysis that decides whether the AX tree carries the expected Japanese/Latin
anchors, and the fail-closed handling of a probe record whose screenshot disagrees
with its digest.
"""

from __future__ import annotations

import contextlib
import io
import json
import struct
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import host_observe as observe  # noqa: E402


def png_header(width: int, height: int) -> bytes:
    return (
        observe.PNG_SIGNATURE
        + struct.pack(">I", 13)
        + b"IHDR"
        + struct.pack(">II", width, height)
        + b"\x08\x06\x00\x00\x00"
    )


def element(name: str, depth: int = 1) -> dict:
    return {
        "depth": depth,
        "name": name,
        "control_type": "ControlType.Text",
        "automation_id": "",
        "focused": False,
        "left": 0,
        "top": 0,
        "width": 10,
        "height": 10,
    }


class PngHeaderTest(unittest.TestCase):
    def test_dimensions_come_from_the_ihdr_chunk(self) -> None:
        self.assertEqual(observe.ihdr_dimensions(png_header(1440, 900)), (1440, 900))

    def test_a_non_png_is_refused_with_a_closed_code(self) -> None:
        with self.assertRaises(observe.ObserveError) as raised:
            observe.ihdr_dimensions(b"not a png at all")
        self.assertEqual(raised.exception.code, "PNG_INVALID")

    def test_zero_sized_ihdr_is_refused(self) -> None:
        with self.assertRaises(observe.ObserveError) as raised:
            observe.ihdr_dimensions(png_header(0, 900))
        self.assertEqual(raised.exception.code, "PNG_INVALID")


class HostScaleTest(unittest.TestCase):
    def test_dpi_maps_onto_the_declared_scale_ladder(self) -> None:
        expected = {96: 1.0, 120: 1.25, 144: 1.5, 168: 1.75, 192: 2.0}
        for dpi, scale in expected.items():
            self.assertEqual(observe.scale_from_dpi(dpi), scale)
            self.assertFalse(scale <= 0.0)

    def test_unusable_dpi_is_a_closed_code(self) -> None:
        for dpi in (0, -96):
            with self.assertRaises(observe.ObserveError) as raised:
                observe.scale_from_dpi(dpi)
            self.assertEqual(raised.exception.code, "DPI_UNAVAILABLE")


class LabelAnalysisTest(unittest.TestCase):
    def test_mixed_script_anchors_are_detected(self) -> None:
        elements = [
            element("Task Rail"),
            element("Inspector"),
            element("task-184 — 新製品キービジュアル"),
            element("Candidate Dock"),
            element(""),
        ]
        analysis = observe.analyze_elements(elements)
        self.assertEqual(analysis["element_count"], 5)
        self.assertEqual(analysis["named_element_count"], 4)
        self.assertEqual(analysis["expected_latin_found"], ["Task Rail", "Inspector", "Candidate Dock"])
        self.assertEqual(analysis["expected_latin_missing"], ["Context Bar"])
        self.assertEqual(analysis["expected_japanese_found"], ["新製品キービジュアル"])
        self.assertEqual(analysis["mixed_script_count"], 1)
        self.assertEqual(
            analysis["mixed_script_examples"], ["task-184 — 新製品キービジュアル"]
        )

    def test_a_latin_only_tree_reports_the_missing_japanese_anchor(self) -> None:
        analysis = observe.analyze_elements([element("Inspector"), element("Search tasks")])
        self.assertEqual(analysis["japanese_name_count"], 0)
        self.assertEqual(analysis["mixed_script_count"], 0)
        self.assertEqual(analysis["expected_japanese_missing"], ["新製品キービジュアル"])
        self.assertEqual(analysis["expected_latin_found"], ["Inspector"])

    def test_halfwidth_katakana_counts_as_japanese(self) -> None:
        analysis = observe.analyze_elements([element("abc ｱｲｳ")])
        self.assertEqual(analysis["mixed_script_count"], 1)


class ProbeRecordTest(unittest.TestCase):
    def _record(self, digest: str, width: int, height: int) -> dict:
        return {
            "window": {
                "status": "OBSERVED",
                "title": "COS Desktop",
                "pid": 4242,
                "handle": 7,
                "left": 10,
                "top": 20,
                "width": width,
                "height": height,
            },
            "screenshot": {
                "status": "OBSERVED",
                "path": "screenshot.png",
                "digest": digest,
                "width": width,
                "height": height,
                "bytes": 64,
            },
            "uia": {
                "status": "OBSERVED",
                "reason": "",
                "digest": "sha256:" + "0" * 64,
                "elements": [element("Inspector")],
                "focused": None,
                "truncated": False,
                "depth_limited": False,
                "elements_visited": 1,
            },
        }

    def test_a_matching_record_is_accepted_and_the_digest_recomputed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png_header(640, 480))
            record = self._record(
                observe.sha256_bytes(screenshot.read_bytes()), 640, 480
            )
            observed = observe.validate_probe_record(record, 4242, screenshot)
        self.assertEqual(observed["screenshot"]["ihdr"], {"width": 640, "height": 480})
        self.assertEqual(
            observed["screenshot"]["digest"], observed["screenshot"]["digest_recomputed"]
        )

    def test_a_digest_that_does_not_cover_the_bytes_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png_header(640, 480))
            record = self._record("sha256:" + "a" * 64, 640, 480)
            with self.assertRaises(observe.ObserveError) as raised:
                observe.validate_probe_record(record, 4242, screenshot)
        self.assertEqual(raised.exception.code, "SCREENSHOT_MISMATCH")

    def test_a_window_from_another_pid_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png_header(640, 480))
            record = self._record(
                observe.sha256_bytes(screenshot.read_bytes()), 640, 480
            )
            with self.assertRaises(observe.ObserveError) as raised:
                observe.validate_probe_record(record, 9999, screenshot)
        self.assertEqual(raised.exception.code, "RECORD_SCHEMA")

    def test_an_existing_output_directory_is_refused_before_launch(self) -> None:
        stdout = io.StringIO()
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(stdout):
            code = observe.main(["--exe", __file__, "--out", directory])
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(stdout.getvalue())["error"], "OUT_EXISTS")


if __name__ == "__main__":
    unittest.main()
