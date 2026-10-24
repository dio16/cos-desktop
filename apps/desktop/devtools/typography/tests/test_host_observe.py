"""MP-TYP1-C-R1: pure coverage for capture identity, label completeness and refusals.

No window is launched and no desktop is required: these tests cover the parts that can
be established deterministically - PNG IHDR parsing, host-scale arithmetic, the
all-label requirement, the capture-identity binding that rejects a foreign or unbound
image, and the honest NOT_MEASURED path when the probe refuses to capture.
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


WINDOW = {
    "status": "OBSERVED",
    "title": "COS Desktop",
    "pid": 4242,
    "handle": 7,
    "left": 10,
    "top": 20,
    "width": 640,
    "height": 480,
}
RECT = {"left": 10, "top": 20, "width": 640, "height": 480}
LIVE = {"status": "OBSERVED", "pid": 4242, "title": "COS Desktop", "rect": dict(RECT)}


def captured(**overrides) -> dict:
    value = {
        "status": "OBSERVED",
        "width": 640,
        "height": 480,
        "bytes": 64,
        "method": observe.BOUND_CAPTURE_METHOD,
        "handle": 7,
        "pid": 4242,
        "title": "COS Desktop",
        "rect": dict(RECT),
        "still_current": True,
        "content_cross_check": {
            "status": "OBSERVED",
            "method": "bound capture versus display over the on-screen region",
            "samples": 49,
            "matched": 49,
            "tolerance": 24,
            "distinct_sampled_colors": 5,
            "non_black_samples": 49,
            "visible_region": {"left": 10, "top": 20, "width": 640, "height": 480},
        },
    }
    value.update(overrides)
    return value


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
        for dpi, scale in {96: 1.0, 120: 1.25, 144: 1.5, 168: 1.75, 192: 2.0}.items():
            self.assertEqual(observe.scale_from_dpi(dpi), scale)

    def test_unusable_dpi_is_a_closed_code(self) -> None:
        for dpi in (0, -96):
            with self.assertRaises(observe.ObserveError) as raised:
                observe.scale_from_dpi(dpi)
            self.assertEqual(raised.exception.code, "DPI_UNAVAILABLE")


class CaptureIdentityTest(unittest.TestCase):
    def test_a_bound_capture_with_matching_identity_is_accepted(self) -> None:
        identity = observe.verify_capture_identity(captured(), WINDOW, 4242, LIVE)
        self.assertEqual(identity["method"], observe.BOUND_CAPTURE_METHOD)
        self.assertEqual(identity["rect"], RECT)
        self.assertFalse(identity["screen_copy_used_for_capture"])
        self.assertIn("live_reverification", identity)
        self.assertEqual(identity["content_cross_check"]["matched"], 49)
        self.assertEqual(identity["content_cross_check"]["distinct_sampled_colors"], 5)

    def test_a_screen_rectangle_capture_is_refused_even_with_matching_rect(self) -> None:
        # The MP-TYP1-C defect: the image was a screen copy of the target rectangle, so
        # its dimensions and hash matched while its content was a foreign window.
        screen_copy = captured(method="CopyFromScreen")
        with self.assertRaises(observe.ObserveError) as raised:
            observe.verify_capture_identity(screen_copy, WINDOW, 4242, LIVE)
        self.assertEqual(raised.exception.code, "CAPTURE_NOT_BOUND")

    def test_a_capture_of_a_different_window_is_refused(self) -> None:
        for overrides in (
            {"handle": 8},
            {"pid": 9999},
            {"title": "some other app"},
            {"rect": {"left": 0, "top": 0, "width": 640, "height": 480}},
        ):
            with self.assertRaises(observe.ObserveError) as raised:
                observe.verify_capture_identity(captured(**overrides), WINDOW, 4242, LIVE)
            self.assertEqual(raised.exception.code, "CAPTURE_IDENTITY_MISMATCH")

    def test_a_window_that_changed_during_capture_is_refused(self) -> None:
        with self.assertRaises(observe.ObserveError) as raised:
            observe.verify_capture_identity(
                captured(still_current=False), WINDOW, 4242, LIVE
            )
        self.assertEqual(raised.exception.code, "CAPTURE_IDENTITY_MISMATCH")

    def test_a_failed_or_missing_content_cross_check_is_refused(self) -> None:
        for cross in (
            {"status": "OBSERVED", "samples": 49, "matched": 10, "tolerance": 24,
             "distinct_sampled_colors": 5, "non_black_samples": 49},
            {"status": "NOT_MEASURED", "reason": "WINDOW_OFFSCREEN"},
            {"status": "OBSERVED", "samples": 49, "matched": 49, "tolerance": 24,
             "distinct_sampled_colors": 1, "non_black_samples": 49},
            {"status": "OBSERVED", "samples": 49, "matched": 49, "tolerance": 24,
             "distinct_sampled_colors": 5, "non_black_samples": 0},
            None,
        ):
            with self.assertRaises(observe.ObserveError) as raised:
                observe.verify_capture_identity(
                    captured(content_cross_check=cross), WINDOW, 4242, LIVE
                )
            self.assertEqual(raised.exception.code, "CAPTURE_CONTENT_MISMATCH")

    def test_live_disagreement_is_refused(self) -> None:
        for live in (
            {"status": "NOT_MEASURED", "reason": "WIN32_UNAVAILABLE"},
            {"status": "OBSERVED", "pid": 1, "title": "COS Desktop", "rect": dict(RECT)},
            {
                "status": "OBSERVED",
                "pid": 4242,
                "title": "COS Desktop",
                "rect": {"left": 0, "top": 0, "width": 640, "height": 480},
            },
        ):
            with self.assertRaises(observe.ObserveError) as raised:
                observe.verify_capture_identity(captured(), WINDOW, 4242, live)
            self.assertEqual(raised.exception.code, "CAPTURE_IDENTITY_MISMATCH")


class LabelCompletenessTest(unittest.TestCase):
    def test_all_expected_labels_pass(self) -> None:
        analysis = observe.analyze_elements(
            [
                element("Task Rail"),
                element("Inspector"),
                element("Context Bar"),
                element("Candidate Dock"),
                element("Task-184 — 新製品キービジュアル"),
            ]
        )
        checks = observe.evaluate_labels(analysis, "OBSERVED")
        self.assertEqual([check["status"] for check in checks], ["PASS", "PASS", "PASS"])
        self.assertEqual(observe.result_of(checks), "PASS")

    def test_a_missing_latin_label_fails_instead_of_passing_on_a_subset(self) -> None:
        analysis = observe.analyze_elements(
            [element("Task Rail"), element("Inspector"), element("新製品キービジュアル")]
        )
        checks = observe.evaluate_labels(analysis, "OBSERVED")
        self.assertEqual(
            checks[0]["status"],
            "FAIL",
            "a partial surface-label set must not pass as complete",
        )
        self.assertEqual(observe.result_of(checks), "FAIL")

    def test_a_missing_japanese_label_fails(self) -> None:
        analysis = observe.analyze_elements(
            [
                element("Task Rail"),
                element("Inspector"),
                element("Context Bar"),
                element("Candidate Dock"),
            ]
        )
        checks = observe.evaluate_labels(analysis, "OBSERVED")
        self.assertEqual(checks[1]["status"], "FAIL")
        self.assertEqual(observe.result_of(checks), "FAIL")

    def test_an_unavailable_ax_tree_is_not_measured_not_failed(self) -> None:
        checks = observe.evaluate_labels({}, "NOT_MEASURED")
        self.assertEqual([check["status"] for check in checks], ["NOT_MEASURED"] * 3)
        self.assertEqual(observe.result_of(checks), "PASS_WITH_NOT_MEASURED")


class ProbeRecordTest(unittest.TestCase):
    def _record(self, width: int = 640, height: int = 480) -> dict:
        return {
            "ok": True,
            "window": dict(WINDOW, width=width, height=height),
            "screenshot": {
                "status": "OBSERVED",
                "width": width,
                "height": height,
                "bytes": 64,
            },
            "uia": {
                "status": "OBSERVED",
                "reason": "",
                "elements": [element("Task Rail")],
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
            expected_digest = observe.sha256_bytes(screenshot.read_bytes())
            observed = observe.validate_probe_record(
                self._record(640, 480), 4242, screenshot
            )
        self.assertEqual(observed["screenshot"]["ihdr"], {"width": 640, "height": 480})
        # The digest is computed from the bytes, never accepted from the record.
        self.assertEqual(observed["screenshot"]["digest"], expected_digest)

    def test_a_probe_capture_refusal_is_recorded_as_not_measured(self) -> None:
        record = self._record(640, 480)
        record["screenshot"] = {
            "status": "NOT_MEASURED",
            "reason": "WINDOW_CAPTURE_FAILED",
        }
        with tempfile.TemporaryDirectory() as directory:
            observed = observe.validate_probe_record(
                record, 4242, Path(directory) / "absent.png"
            )
        self.assertEqual(observed["screenshot"]["status"], "NOT_MEASURED")
        self.assertEqual(observed["screenshot"]["reason"], "WINDOW_CAPTURE_FAILED")

    def test_an_image_whose_ihdr_disagrees_with_the_record_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png_header(640, 480))
            with self.assertRaises(observe.ObserveError) as raised:
                observe.validate_probe_record(self._record(641, 480), 4242, screenshot)
        self.assertEqual(raised.exception.code, "SCREENSHOT_MISMATCH")

    def test_an_observed_record_without_identity_fields_is_not_identity(self) -> None:
        # MP-TYP1-C regression: a record can say OBSERVED and match the rectangle while
        # carrying no bound-capture identity; that must never count as identity.
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png_header(640, 480))
            observed = observe.validate_probe_record(
                self._record(640, 480), 4242, screenshot
            )
        with self.assertRaises(observe.ObserveError) as raised:
            observe.verify_capture_identity(
                observed["screenshot"]["raw"], WINDOW, 4242, LIVE
            )
        self.assertEqual(raised.exception.code, "CAPTURE_NOT_BOUND")

    def test_a_window_from_another_pid_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png_header(640, 480))
            with self.assertRaises(observe.ObserveError) as raised:
                observe.validate_probe_record(self._record(640, 480), 9999, screenshot)
        self.assertEqual(raised.exception.code, "RECORD_SCHEMA")

    def test_an_existing_output_directory_is_refused_before_launch(self) -> None:
        stdout = io.StringIO()
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(stdout):
            code = observe.main(["--exe", __file__, "--out", directory])
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(stdout.getvalue())["error"], "OUT_EXISTS")


if __name__ == "__main__":
    unittest.main()
