"""MP-TYP1-C-R2: capture identity, PNG integrity, pinned title and refusal coverage.

No window is launched and no desktop is required. These tests cover the parts that can
be established deterministically: PNG structural/decodable verification (IHDR alone is
never trusted), the geometry binding to the target rectangle, the pinned window title,
the all-label requirement, the capture-identity binding, and the rule that an occluded
target is a closed NOT_MEASURED refusal with no desktop state change.
"""

from __future__ import annotations

import contextlib
import io
import json
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import host_observe as observe  # noqa: E402

PROBE = Path(__file__).resolve().parents[2] / "r0" / "n03_probe.ps1"


def chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
    )


def png(width: int, height: int, rgb: tuple[int, int, int] = (23, 25, 28)) -> bytes:
    """A real, decodable 8-bit truecolour PNG."""
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
    return (
        observe.PNG_SIGNATURE
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def scanlines(width: int, height: int, rgb: tuple[int, int, int] = (23, 25, 28)) -> bytes:
    return b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))


def png_with_idat(width: int, height: int, image_data: bytes) -> bytes:
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        observe.PNG_SIGNATURE
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", image_data)
        + chunk(b"IEND", b"")
    )


class CountingStream(io.BytesIO):
    """A bounded reader that records how many bytes were actually read."""

    def __init__(self, data: bytes) -> None:
        super().__init__(data)
        self.bytes_read = 0

    def read(self, size: int = -1) -> bytes:
        data = super().read(size)
        self.bytes_read += len(data)
        return data


def png_header_only(width: int, height: int) -> bytes:
    """A header fragment: signature plus IHDR, no IDAT and no IEND."""
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
    "title": observe.EXPECTED_TITLE,
    "pid": 4242,
    "handle": 7,
    "left": 10,
    "top": 20,
    "width": 640,
    "height": 480,
}
RECT = {"left": 10, "top": 20, "width": 640, "height": 480}
LIVE = {"status": "OBSERVED", "pid": 4242, "title": observe.EXPECTED_TITLE, "rect": dict(RECT)}


def captured(**overrides) -> dict:
    value = {
        "status": "OBSERVED",
        "width": 640,
        "height": 480,
        "bytes": 64,
        "method": observe.BOUND_CAPTURE_METHOD,
        "handle": 7,
        "pid": 4242,
        "title": observe.EXPECTED_TITLE,
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


class PngIntegrityTest(unittest.TestCase):
    def test_a_complete_png_decodes_to_its_dimensions(self) -> None:
        self.assertEqual(observe.png_dimensions_verified(png(320, 200)), (320, 200))

    def test_a_header_fragment_is_not_a_png(self) -> None:
        # The MP-TYP1-C-R1 weakness: the signature plus IHDR was accepted as an image.
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(png_header_only(320, 200))
        self.assertEqual(raised.exception.code, "PNG_TRUNCATED")

    def test_a_truncated_png_is_refused(self) -> None:
        data = png(64, 48)
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(data[: len(data) // 2])
        self.assertIn(raised.exception.code, ("PNG_TRUNCATED", "PNG_UNDECODABLE"))

    def test_an_undecodable_image_data_chunk_is_refused(self) -> None:
        header = struct.pack(">IIBBBBB", 32, 32, 8, 2, 0, 0, 0)
        data = (
            observe.PNG_SIGNATURE
            + chunk(b"IHDR", header)
            + chunk(b"IDAT", b"this is not zlib data")
            + chunk(b"IEND", b"")
        )
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(data)
        self.assertEqual(raised.exception.code, "PNG_UNDECODABLE")

    def test_a_corrupt_crc_is_refused(self) -> None:
        data = bytearray(png(16, 16))
        data[-5] ^= 0xFF  # break the IEND CRC
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(bytes(data))
        self.assertEqual(raised.exception.code, "PNG_TRUNCATED")

    def test_a_zero_sized_image_is_refused(self) -> None:
        header = struct.pack(">IIBBBBB", 0, 0, 8, 2, 0, 0, 0)
        data = (
            observe.PNG_SIGNATURE
            + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(b""))
            + chunk(b"IEND", b"")
        )
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(data)
        self.assertEqual(raised.exception.code, "PNG_INVALID")

    def test_a_stripped_adler32_tail_is_refused(self) -> None:
        # Every scanline expands, but the zlib stream never ends: length alone is not
        # proof of a complete stream.
        image_data = zlib.compress(scanlines(16, 16))[:-4]
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(png_with_idat(16, 16, image_data))
        self.assertEqual(raised.exception.code, "PNG_UNDECODABLE")

    def test_surplus_compressed_data_after_the_stream_is_refused(self) -> None:
        image_data = zlib.compress(scanlines(16, 16)) + b"\x00\x00\x00\x00"
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(png_with_idat(16, 16, image_data))
        self.assertEqual(raised.exception.code, "PNG_TRAILING_DATA")

    def test_the_cumulative_idat_bound_is_enforced(self) -> None:
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(png(8, 8), max_idat=1)
        self.assertEqual(raised.exception.code, "PNG_TOO_LARGE")

    def test_an_oversized_screenshot_is_refused_before_a_full_read(self) -> None:
        cap = 1024
        stream = CountingStream(b"\x89PNG" + b"\x00" * (cap * 4))
        with self.assertRaises(observe.ObserveError) as raised:
            observe.read_bounded_stream(stream, cap)
        self.assertEqual(raised.exception.code, "PNG_TOO_LARGE")
        self.assertLessEqual(stream.bytes_read, cap + 1)
        self.assertLess(stream.bytes_read, cap * 4)

    def test_an_oversized_file_is_refused_from_its_size(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(b"x" * 4096)
            with self.assertRaises(observe.ObserveError) as raised:
                observe.read_bounded(path, cap=1024)
        self.assertEqual(raised.exception.code, "PNG_TOO_LARGE")

    def test_an_oversized_decode_is_refused_without_decoding_it(self) -> None:
        header = struct.pack(">IIBBBBB", 4096, 4096, 8, 6, 0, 0, 0)  # 67 MiB decoded
        data = (
            observe.PNG_SIGNATURE
            + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(b"\x00" * 16))
            + chunk(b"IEND", b"")
        )
        with self.assertRaises(observe.ObserveError) as raised:
            observe.png_dimensions_verified(data)
        self.assertEqual(raised.exception.code, "PNG_TOO_LARGE")


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
        self.assertFalse(identity["desktop_state_changed"])
        self.assertIn("live_reverification", identity)
        self.assertEqual(identity["content_cross_check"]["matched"], 49)
        self.assertEqual(identity["content_cross_check"]["distinct_sampled_colors"], 5)

    def test_a_screen_rectangle_capture_is_refused_even_with_matching_rect(self) -> None:
        # The MP-TYP1-C defect: a screen copy of the rectangle matched size and hash while
        # its content was a foreign window.
        with self.assertRaises(observe.ObserveError) as raised:
            observe.verify_capture_identity(
                captured(method="CopyFromScreen"), WINDOW, 4242, LIVE
            )
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
            {"status": "NOT_MEASURED", "reason": "WINDOW_OCCLUDED"},
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
            {"status": "OBSERVED", "pid": 1, "title": observe.EXPECTED_TITLE,
             "rect": dict(RECT)},
            {"status": "OBSERVED", "pid": 4242, "title": observe.EXPECTED_TITLE,
             "rect": {"left": 0, "top": 0, "width": 640, "height": 480}},
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
        self.assertEqual(checks[0]["status"], "FAIL")
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
    def _record(self, width: int = 640, height: int = 480, title: str = observe.EXPECTED_TITLE) -> dict:
        return {
            "ok": True,
            "window": dict(WINDOW, title=title, width=width, height=height),
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
            screenshot.write_bytes(png(640, 480))
            expected_digest = observe.sha256_bytes(screenshot.read_bytes())
            observed = observe.validate_probe_record(
                self._record(640, 480), 4242, screenshot
            )
        self.assertEqual(observed["screenshot"]["ihdr"], {"width": 640, "height": 480})
        self.assertEqual(observed["screenshot"]["digest"], expected_digest)

    def test_a_png_whose_dimensions_disagree_with_the_window_is_refused(self) -> None:
        record = self._record(640, 480)
        # Reported and actual image sizes agree with each other but not with the window.
        record["screenshot"]["width"] = 641
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png(641, 480))
            with self.assertRaises(observe.ObserveError) as raised:
                observe.validate_probe_record(record, 4242, screenshot)
        self.assertEqual(raised.exception.code, "SCREENSHOT_GEOMETRY_MISMATCH")

    def test_a_truncated_png_in_the_record_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png(640, 480)[:-200])
            with self.assertRaises(observe.ObserveError) as raised:
                observe.validate_probe_record(self._record(640, 480), 4242, screenshot)
        self.assertIn(raised.exception.code, ("PNG_TRUNCATED", "PNG_UNDECODABLE"))

    def test_a_wrong_or_missing_title_is_refused(self) -> None:
        for title in ("COS Desktop (other instance)", ""):
            with tempfile.TemporaryDirectory() as directory:
                screenshot = Path(directory) / "screenshot.png"
                screenshot.write_bytes(png(640, 480))
                with self.assertRaises(observe.ObserveError) as raised:
                    observe.validate_probe_record(
                        self._record(640, 480, title), 4242, screenshot
                    )
            self.assertEqual(raised.exception.code, "WINDOW_TITLE_MISMATCH")

    def test_a_probe_capture_refusal_is_recorded_as_not_measured(self) -> None:
        record = self._record(640, 480)
        record["screenshot"] = {"status": "NOT_MEASURED", "reason": "WINDOW_OCCLUDED"}
        with tempfile.TemporaryDirectory() as directory:
            observed = observe.validate_probe_record(
                record, 4242, Path(directory) / "absent.png"
            )
        self.assertEqual(observed["screenshot"]["status"], "NOT_MEASURED")
        self.assertEqual(observed["screenshot"]["reason"], "WINDOW_OCCLUDED")
        # An occluded target is not a failure of the run.
        checks = [
            {"name": "screenshot_is_the_target_window", "status": "NOT_MEASURED"},
            {"name": "latin_labels_present", "status": "PASS"},
        ]
        self.assertEqual(observe.result_of(checks), "PASS_WITH_NOT_MEASURED")

    def test_a_window_from_another_pid_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png(640, 480))
            with self.assertRaises(observe.ObserveError) as raised:
                observe.validate_probe_record(self._record(640, 480), 9999, screenshot)
        self.assertEqual(raised.exception.code, "RECORD_SCHEMA")

    def test_an_existing_output_directory_is_refused_before_launch(self) -> None:
        stdout = io.StringIO()
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(stdout):
            code = observe.main(["--exe", __file__, "--out", directory])
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(stdout.getvalue())["error"], "OUT_EXISTS")


class NoDesktopStateChangeTest(unittest.TestCase):
    def test_the_probe_performs_no_z_order_or_focus_operation(self) -> None:
        # The BLOCKING fix: no temporary topmost, no focus request, nothing that could be
        # left behind. An occluded target is refused instead.
        source = PROBE.read_text(encoding="utf-8")
        for forbidden in (
            "SetWindowPos",
            "SetForegroundWindow",
            "BringWindowToTop",
            "ShowWindow",
            "SetActiveWindow",
            "SetFocus",
            "MoveWindow",
        ):
            self.assertNotIn(forbidden, source, forbidden)
        self.assertIn("WINDOW_OCCLUDED", source)


if __name__ == "__main__":
    unittest.main()
