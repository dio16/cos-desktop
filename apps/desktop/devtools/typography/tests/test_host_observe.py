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
HOST_OBSERVE = Path(observe.__file__)


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
            observed = observe.validate_probe_record(record, 4242, screenshot)
        self.assertEqual(observed["screenshot"]["status"], "NOT_MEASURED")
        self.assertEqual(observed["refused"], "SCREENSHOT_GEOMETRY_MISMATCH")

    def test_a_truncated_png_in_the_record_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            screenshot = Path(directory) / "screenshot.png"
            screenshot.write_bytes(png(640, 480)[:-200])
            observed = observe.validate_probe_record(
                self._record(640, 480), 4242, screenshot
            )
        self.assertIn(observed["refused"], ("PNG_TRUNCATED", "PNG_UNDECODABLE"))

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


class AcceptanceRecordTest(unittest.TestCase):
    def _record(self) -> dict:
        observed = {
            "window": dict(WINDOW),
            "screenshot": {"status": "NOT_MEASURED", "reason": "WINDOW_OCCLUDED"},
            "uia": {
                "status": "OBSERVED",
                "reason": "",
                "elements": [
                    element("Task Rail"),
                    element("Inspector"),
                    element("Context Bar"),
                    element("Candidate Dock"),
                    element("Task-184 — 新製品キービジュアル"),
                ],
                "truncated": False,
                "depth_limited": False,
                "elements_visited": 5,
            },
        }
        analysis = observe.analyze_elements(observed["uia"]["elements"])
        scale = {
            "status": "OBSERVED",
            "dpi": 96,
            "scale": 1.0,
            "method": "GetDpiForWindow",
            "controlled_ladder_step": False,
        }
        wait = {
            "mode": "read-only observation of GetForegroundWindow",
            "outcome": observe.REASON_FOREGROUND_WAIT_EXPIRED,
            "requested_s": 30.0,
            "elapsed_s": 30.001,
            "polls": 121,
        }
        return observe.build_record(
            Path("cos-desktop.exe"),
            "sha256:" + "0" * 64,
            observed,
            scale,
            analysis,
            {"method": "taskkill /T /F", "exit_code": 1},
            wait,
        )

    def test_the_record_claims_no_foreground_grant_and_states_the_human_step(self) -> None:
        record = self._record()
        launch = record["launch"]
        self.assertEqual(launch["foreground_grant"], "none")
        self.assertNotIn("foreground_permission", launch)
        self.assertIn("the user", launch["foreground"])
        self.assertIn("brings COS Desktop to the foreground", record["acceptance_procedure"]["human_step"])
        self.assertEqual(
            record["acceptance_procedure"]["scale_order"],
            ["100 %", "125 %", "150 %", "175 %", "200 %"],
        )
        self.assertFalse(record["desktop_state_changed"])
        self.assertNotIn("AllowSetForegroundWindow", json.dumps(record))
        self.assertEqual(record["screenshot"]["status"], "NOT_MEASURED")
        self.assertEqual(
            record["foreground_wait"]["outcome"],
            observe.REASON_FOREGROUND_WAIT_EXPIRED,
        )
        self.assertEqual(record["foreground_wait"]["elapsed_s"], 30.001)
        self.assertEqual(record["result"], "PASS_WITH_NOT_MEASURED")
        self.assertFalse(record["declares_mptyp1_complete"])


class ForegroundWaitTest(unittest.TestCase):
    def _clock(self, step: float = 0.3):
        state = {"now": 0.0}

        def clock() -> float:
            value = state["now"]
            state["now"] += step
            return value

        return clock

    def _poll(self, seen: list[dict]):
        state = {"index": 0}

        def poll() -> dict:
            index = min(state["index"], len(seen) - 1)
            state["index"] += 1
            return seen[index]

        return poll

    def _foreground(self, pid: int, title: str = observe.EXPECTED_TITLE) -> dict:
        return {"status": "OBSERVED", "handle": 42, "pid": pid, "title": title,
                "rect": {"left": 0, "top": 0, "width": 640, "height": 480}}

    def test_the_wait_uses_only_read_only_observation(self) -> None:
        source = HOST_OBSERVE.read_text(encoding="utf-8")
        self.assertIn("GetForegroundWindow", source)
        self.assertIn("def wait_for_foreground(", source)
        for forbidden in NoDesktopStateChangeTest.FORBIDDEN:
            self.assertNotIn(forbidden, source, forbidden)

    def test_a_foreground_transition_to_the_target_is_detected(self) -> None:
        polls = self._poll(
            [
                self._foreground(1, "other"),
                self._foreground(1, "other"),
                self._foreground(4242),
            ]
        )
        wait = observe.wait_for_foreground(
            4242, 10.0, poll=polls, clock=self._clock(), sleep=lambda _s: None
        )
        self.assertEqual(wait["outcome"], "FOREGROUNDED")
        self.assertEqual(wait["polls"], 3)
        self.assertEqual(wait["handle"], 42)
        self.assertGreaterEqual(wait["elapsed_s"], 0.0)
        self.assertEqual(wait["requested_s"], 10.0)

    def test_a_transition_to_a_different_window_does_not_count(self) -> None:
        polls = self._poll([self._foreground(7, "some other app")])
        wait = observe.wait_for_foreground(
            4242, 1.0, poll=polls, clock=self._clock(), sleep=lambda _s: None
        )
        self.assertEqual(wait["outcome"], observe.REASON_FOREGROUND_WAIT_EXPIRED)
        self.assertEqual(wait["observed"], observe.REASON_FOREGROUND_IS_NOT_THE_TARGET)
        self.assertNotIn("last_foreground", wait)
        self.assertNotIn("pid", wait)
        self.assertNotIn("title", wait)
        self.assertGreaterEqual(wait["elapsed_s"], 1.0)

    def test_a_same_pid_untitled_window_does_not_count(self) -> None:
        polls = self._poll([self._foreground(4242, "COS Desktop (other)")])
        wait = observe.wait_for_foreground(
            4242, 0.5, poll=polls, clock=self._clock(), sleep=lambda _s: None
        )
        self.assertEqual(wait["outcome"], observe.REASON_FOREGROUND_WAIT_EXPIRED)

    def test_an_expired_wait_is_the_closed_not_measured_reason(self) -> None:
        wait = {
            "outcome": observe.REASON_FOREGROUND_WAIT_EXPIRED,
            "elapsed_s": 30.0,
            "requested_s": 30.0,
        }
        shot = observe.resolve_screenshot(
            {"status": "NOT_MEASURED", "reason": "WINDOW_OCCLUDED"}, wait
        )
        self.assertEqual(shot["status"], "NOT_MEASURED")
        self.assertEqual(shot["reason"], "FOREGROUND_WAIT_EXPIRED")
        self.assertEqual(shot["detail"], "WINDOW_OCCLUDED")
        self.assertEqual(shot["elapsed_s"], 30.0)
        self.assertNotIn("digest", shot)
        self.assertNotIn("path", shot)

    def test_a_capture_is_kept_only_when_the_wait_foregrounded(self) -> None:
        captured_shot = {"status": "OBSERVED", "digest": "sha256:" + "a" * 64}
        kept = observe.resolve_screenshot(
            captured_shot, {"outcome": "FOREGROUNDED", "elapsed_s": 1.0}
        )
        self.assertEqual(kept["status"], "OBSERVED")
        self.assertEqual(kept["digest"], captured_shot["digest"])
        # An expired wait must be incapable of recording a capture, even one that exists.
        expired = observe.resolve_screenshot(
            captured_shot,
            {"outcome": observe.REASON_FOREGROUND_WAIT_EXPIRED, "elapsed_s": 30.0},
        )
        self.assertEqual(expired["status"], "NOT_MEASURED")
        self.assertEqual(expired["reason"], observe.REASON_FOREGROUND_WAIT_EXPIRED)
        self.assertNotIn("digest", expired)
        self.assertNotIn("path", expired)

    def test_an_expired_wait_removes_any_capture_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            kept_path = Path(directory) / "kept.png"
            kept_path.write_bytes(png(8, 8))
            observe.discard_capture(kept_path, {"outcome": "FOREGROUNDED"})
            self.assertTrue(kept_path.exists())
            observe.discard_capture(
                kept_path, {"outcome": observe.REASON_FOREGROUND_WAIT_EXPIRED}
            )
            self.assertFalse(kept_path.exists())

    def test_the_record_carries_no_foreign_window_identifier(self) -> None:
        sentinel_title = "FOREIGN_WINDOW_MARKER"
        sentinel_pid = 987654
        polls = self._poll(
            [{"status": "OBSERVED", "handle": 5, "pid": sentinel_pid, "title": sentinel_title}]
        )
        wait = observe.wait_for_foreground(
            4242, 0.5, poll=polls, clock=self._clock(), sleep=lambda _s: None
        )
        self.assertEqual(wait["outcome"], observe.REASON_FOREGROUND_WAIT_EXPIRED)
        self.assertEqual(wait["observed"], observe.REASON_FOREGROUND_IS_NOT_THE_TARGET)
        record = observe.build_record(
            Path("cos-desktop.exe"),
            "sha256:" + "0" * 64,
            {
                "window": dict(WINDOW),
                "screenshot": {"status": "NOT_MEASURED", "reason": "FOREGROUND_WAIT_EXPIRED"},
                "uia": {"status": "OBSERVED", "reason": "", "elements": [element("Task Rail")],
                        "truncated": False, "depth_limited": False, "elements_visited": 1},
            },
            {"status": "OBSERVED", "dpi": 96, "scale": 1.0, "method": "GetDpiForWindow",
             "controlled_ladder_step": False},
            observe.analyze_elements([element("Task Rail")]),
            {"method": "taskkill /T /F", "exit_code": 1},
            wait,
            {"status": "NOT_APPLICABLE"},
        )
        body = json.dumps(record)
        self.assertNotIn(sentinel_title, body)
        self.assertNotIn(str(sentinel_pid), body)
        self.assertNotIn("last_foreground", body)
        self.assertEqual(record["foreground_binding"], {"status": "NOT_APPLICABLE"})

    def test_a_capture_with_a_different_waited_handle_is_refused(self) -> None:
        binding = observe.verify_waited_handle(
            {"outcome": "FOREGROUNDED", "handle": 11}, {"handle": 11}
        )
        self.assertEqual(binding["status"], "OBSERVED")
        self.assertTrue(binding["captured_handle_matches_waited"])
        with self.assertRaises(observe.ObserveError) as raised:
            observe.verify_waited_handle(
                {"outcome": "FOREGROUNDED", "handle": 11}, {"handle": 22}
            )
        self.assertEqual(raised.exception.code, "FOREGROUND_HANDLE_MISMATCH")

    def test_the_wait_bound_is_validated(self) -> None:
        self.assertEqual(observe.bounded_wait("30"), 30.0)
        self.assertEqual(observe.bounded_wait("0"), 0.0)
        for value in ("-1", "120.5", "nan", "inf", "soon"):
            with self.assertRaises(observe.ObserveError) as raised:
                observe.bounded_wait(value)
            self.assertEqual(raised.exception.code, "ARGS")


class PrivacySweepTest(unittest.TestCase):
    MARKER = "FOREIGN_FOCUS_MARKER"

    def _document(self) -> dict:
        return {
            "ok": True,
            "window": dict(WINDOW),
            "screenshot": {"status": "NOT_MEASURED", "reason": "WINDOW_OCCLUDED"},
            "uia": {
                "status": "OBSERVED",
                "reason": "",
                "elements": [element("Task Rail")],
                "focused": {
                    "name": self.MARKER,
                    "control_type": "ControlType.Hyperlink",
                    "automation_id": "",
                },
                "truncated": False,
                "depth_limited": False,
                "elements_visited": 1,
            },
        }

    def test_the_probe_record_is_sanitised_before_it_is_written(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            observe.write_probe_record(out, self._document())
            body = (out / "probe.json").read_text(encoding="utf-8")
        self.assertNotIn(self.MARKER, body)
        # The global focus element is gone; per-element state inside the target subtree
        # (for example "focused": false) is not a foreign identity and is kept.
        self.assertNotIn("focused", json.loads(body)["uia"])

    def test_no_file_this_harness_writes_carries_a_foreign_identifier(self) -> None:
        document = self._document()
        observed = {
            "window": dict(WINDOW),
            "screenshot": {
                "status": "NOT_MEASURED",
                "reason": observe.REASON_FOREGROUND_WAIT_EXPIRED,
                "elapsed_s": 30.0,
            },
            "uia": document["uia"],
        }
        wait = {
            "outcome": observe.REASON_FOREGROUND_WAIT_EXPIRED,
            "elapsed_s": 30.0,
            "polls": 121,
            "observed": observe.REASON_FOREGROUND_IS_NOT_THE_TARGET,
        }
        record = observe.build_record(
            Path("cos-desktop.exe"),
            "sha256:" + "0" * 64,
            observed,
            {"status": "OBSERVED", "dpi": 96, "scale": 1.0, "method": "GetDpiForWindow",
             "controlled_ladder_step": False},
            observe.analyze_elements([element("Task Rail")]),
            {"method": "taskkill /T /F", "exit_code": 1},
            wait,
            {"status": "NOT_APPLICABLE"},
        )
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            observe.write_probe_record(out, document)
            observe.write_record(record, out / "observation.json")
            (out / "app.log").write_bytes(b"")
            import dpi_matrix

            with contextlib.redirect_stdout(io.StringIO()):
                dpi_matrix.main(["--out", str(out / "dpi_matrix.json")])
            for name in ("probe.json", "observation.json", "app.log", "dpi_matrix.json"):
                body = (out / name).read_text(encoding="utf-8")
                self.assertNotIn(self.MARKER, body, name)


class ExpiredWaitCaptureTest(unittest.TestCase):
    def test_an_expired_wait_with_a_successful_probe_capture_writes_no_image(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(png(640, 480))
            observed = {
                "window": dict(WINDOW),
                "screenshot": {
                    "status": "OBSERVED",
                    "digest": observe.sha256_bytes(path.read_bytes()),
                    "ihdr": {"width": 640, "height": 480},
                    "bytes": path.stat().st_size,
                    "path": str(path),
                    "raw": captured(),
                },
                "uia": {
                    "status": "OBSERVED",
                    "reason": "",
                    "elements": [element("Task Rail")],
                    "truncated": False,
                    "depth_limited": False,
                    "elements_visited": 1,
                },
            }
            wait = {
                "outcome": observe.REASON_FOREGROUND_WAIT_EXPIRED,
                "elapsed_s": 30.0,
                "polls": 121,
            }
            observe.discard_capture(path, wait)
            resolved = observe.finalise_screenshot(
                observed, wait, 4242, live_lookup=lambda _handle: LIVE
            )
            self.assertEqual(resolved["status"], "NOT_MEASURED")
            self.assertEqual(resolved["reason"], observe.REASON_FOREGROUND_WAIT_EXPIRED)
            self.assertNotIn("digest", resolved)
            self.assertNotIn("path", resolved)
            self.assertFalse(path.exists())
            observed["screenshot"] = resolved
            record = observe.build_record(
                Path("cos-desktop.exe"),
                "sha256:" + "0" * 64,
                observed,
                {"status": "OBSERVED", "dpi": 96, "scale": 1.0,
                 "method": "GetDpiForWindow", "controlled_ladder_step": False},
                observe.analyze_elements([element("Task Rail")]),
                {"method": "taskkill /T /F", "exit_code": 1},
                wait,
                {"status": "NOT_APPLICABLE"},
            )
        self.assertNotIn("digest", record["screenshot"])
        self.assertNotIn("path", record["screenshot"])


class RefusedCaptureTest(unittest.TestCase):
    def test_a_refused_capture_is_deleted_and_recorded_without_a_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(png(640, 480))
            shot = observe.refuse_capture(path, "FOREGROUND_HANDLE_MISMATCH")
            self.assertFalse(path.exists())
            self.assertEqual(shot["status"], "NOT_MEASURED")
            self.assertEqual(shot["reason"], "FOREGROUND_HANDLE_MISMATCH")
            self.assertNotIn("path", shot)
            self.assertNotIn("digest", shot)
            record = observe.build_record(
                Path("cos-desktop.exe"),
                "sha256:" + "0" * 64,
                {
                    "window": dict(WINDOW),
                    "screenshot": shot,
                    "uia": {
                        "status": "OBSERVED",
                        "reason": "",
                        "elements": [element("Task Rail")],
                        "truncated": False,
                        "depth_limited": False,
                        "elements_visited": 1,
                    },
                },
                {"status": "OBSERVED", "dpi": 96, "scale": 1.0,
                 "method": "GetDpiForWindow", "controlled_ladder_step": False},
                observe.analyze_elements([element("Task Rail")]),
                {"method": "taskkill /T /F", "exit_code": 1},
                {"outcome": "FOREGROUNDED", "elapsed_s": 1.0, "handle": 7},
                {"status": "NOT_APPLICABLE"},
            )
        check = next(
            item
            for item in record["checks"]
            if item["name"] == "screenshot_is_the_target_window"
        )
        self.assertEqual(check["status"], "FAIL")
        self.assertEqual(record["result"], "FAIL")
        self.assertEqual(record["capture_refusal"], "FOREGROUND_HANDLE_MISMATCH")
        self.assertNotIn("digest", record["screenshot"])
        self.assertNotIn("path", record["screenshot"])
        self.assertFalse(path.exists())


class RejectionDispositionTest(unittest.TestCase):
    def _observed(self, path: Path) -> dict:
        return {
            "window": dict(WINDOW),
            "screenshot": {
                "status": "OBSERVED",
                "digest": observe.sha256_bytes(path.read_bytes()),
                "ihdr": {"width": 640, "height": 480},
                "bytes": path.stat().st_size,
                "path": str(path),
                "raw": captured(),
            },
            "uia": {
                "status": "OBSERVED",
                "reason": "",
                "elements": [element("Task Rail")],
                "truncated": False,
                "depth_limited": False,
                "elements_visited": 1,
            },
        }

    def _dispose(self, observed: dict, wait: dict, path: Path, live=None):
        return observe.dispose_capture(
            observed,
            wait,
            4242,
            path,
            live_lookup=((lambda _handle: live) if live is not None else (lambda _handle: LIVE)),
        )

    def test_a_png_invalid_capture_is_deleted_and_recorded_with_a_closed_code(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(png(640, 480)[:-200])
            observed = observe.validate_probe_record(
                {
                    "ok": True,
                    "window": dict(WINDOW),
                    "screenshot": {"status": "OBSERVED", "width": 640, "height": 480,
                                   "bytes": 64},
                    "uia": {"status": "OBSERVED", "reason": "",
                            "elements": [element("Task Rail")]},
                },
                4242,
                path,
            )
            observed, _binding, refusal = self._dispose(
                observed, {"outcome": "FOREGROUNDED", "handle": 7}, path
            )
            self.assertFalse(path.exists())
        self.assertIn(refusal, ("PNG_TRUNCATED", "PNG_UNDECODABLE"))
        self.assertEqual(observed["screenshot"]["status"], "NOT_MEASURED")
        self.assertEqual(observed["screenshot"]["reason"], refusal)
        self.assertNotIn("path", observed["screenshot"])
        self.assertNotIn("digest", observed["screenshot"])

    def test_a_hwnd_mismatch_capture_is_deleted_and_recorded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(png(640, 480))
            observed, binding, refusal = self._dispose(
                self._observed(path), {"outcome": "FOREGROUNDED", "handle": 999}, path
            )
            self.assertFalse(path.exists())
        self.assertEqual(refusal, "FOREGROUND_HANDLE_MISMATCH")
        self.assertEqual(binding, {"status": "NOT_APPLICABLE"})
        self.assertEqual(observed["screenshot"]["reason"], "FOREGROUND_HANDLE_MISMATCH")
        self.assertNotIn("path", observed["screenshot"])
        self.assertNotIn("digest", observed["screenshot"])

    def test_an_identity_mismatch_capture_is_deleted_and_recorded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(png(640, 480))
            wrong_live = {"status": "OBSERVED", "pid": 1, "title": observe.EXPECTED_TITLE,
                          "rect": dict(RECT)}
            observed, _binding, refusal = self._dispose(
                self._observed(path), {"outcome": "FOREGROUNDED", "handle": 7}, path,
                live=wrong_live,
            )
            self.assertFalse(path.exists())
        self.assertEqual(refusal, "CAPTURE_IDENTITY_MISMATCH")
        self.assertEqual(observed["screenshot"]["reason"], "CAPTURE_IDENTITY_MISMATCH")
        self.assertNotIn("path", observed["screenshot"])
        self.assertNotIn("digest", observed["screenshot"])

    def test_a_rejected_capture_record_has_no_path_digest_and_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(png(640, 480))
            observed, binding, refusal = self._dispose(
                self._observed(path), {"outcome": "FOREGROUNDED", "handle": 999}, path
            )
        record = observe.build_record(
            Path("cos-desktop.exe"),
            "sha256:" + "0" * 64,
            observed,
            {"status": "OBSERVED", "dpi": 96, "scale": 1.0,
             "method": "GetDpiForWindow", "controlled_ladder_step": False},
            observe.analyze_elements(observed["uia"]["elements"]),
            {"method": "taskkill /T /F", "exit_code": 1},
            {"outcome": "FOREGROUNDED", "elapsed_s": 1.0, "handle": 999},
            binding,
        )
        self.assertNotIn("digest", record["screenshot"])
        self.assertNotIn("path", record["screenshot"])
        self.assertEqual(record["capture_refusal"], refusal)
        self.assertEqual(record["result"], "FAIL")

    def test_an_unaccepted_capture_never_survives_an_abnormal_exit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screenshot.png"
            path.write_bytes(png(640, 480))
            with self.assertRaises(RuntimeError):
                try:
                    raise RuntimeError("validation failed after the probe wrote the image")
                finally:
                    observe.cleanup_unaccepted_capture(path, False)
            self.assertFalse(path.exists())
            kept = Path(directory) / "kept.png"
            kept.write_bytes(png(8, 8))
            observe.cleanup_unaccepted_capture(kept, True)
            self.assertTrue(kept.exists())


class NoDesktopStateChangeTest(unittest.TestCase):
    FORBIDDEN = (
        "AllowSetForegroundWindow",
        "SetWindowPos",
        "SetForegroundWindow",
        "BringWindowToTop",
        "ShowWindow",
        "SetActiveWindow",
        "SetFocus",
        "MoveWindow",
    )

    def test_the_observation_code_performs_no_foreground_or_z_order_operation(self) -> None:
        # The border is absolute: neither the probe nor the harness may influence which
        # window is in front. The user foregrounds COS Desktop; an occluded target is
        # refused instead.
        for source_path in (PROBE, HOST_OBSERVE):
            source = source_path.read_text(encoding="utf-8")
            for forbidden in self.FORBIDDEN:
                self.assertNotIn(forbidden, source, f"{forbidden} in {source_path.name}")
        self.assertIn("WINDOW_OCCLUDED", PROBE.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
