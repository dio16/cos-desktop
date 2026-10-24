"""MP-TYP1-C-R1: observe the fixture shell through the independent probe, with proof.

Python standard library only. This module launches the already-built fixture binary and
runs the repository's independent probe, `apps/desktop/devtools/r0/n03_probe.ps1`,
through its documented invocation
(`powershell -NoProfile -File n03_probe.ps1 -ProcessId <pid> -Screenshot <png>`). The
probe's own record is validated here, including the identity fields, because the r0
driver normalizes the screenshot record down to status/size/digest.

MP-TYP1-C recorded a screenshot whose hash matched its record but whose pixels belonged
to a foreign window: the probe copied the screen rectangle, not the target HWND. This
revision therefore refuses any capture that is not provably the target window:

    binding      the PNG must come from `PrintWindow(PW_RENDERFULLCONTENT)` on the
                 target HWND - a screen copy can never satisfy this
    identity     the record's handle, pid, title and capture rectangle must equal the
                 observed window, and `still_current` must be true
    content      the probe must have agreed, on a 7x7 sampled grid, between the bound
                 capture and the display over the window's on-screen region
                 (>= 45 of 49 samples within 24 RGB levels), and the capture must not
                 be a blank composite (>= 4 distinct sampled colours, >= 45 non-black
                 samples). A foreground window is not required: the binding is the HWND,
                 and a foreign window on top makes the sampled agreement fail.
    geometry     the PNG must decode to exactly the target window rectangle and the
                 capture rectangle; a structurally incomplete or undecodable image, or
                 a dimension mismatch, is refused (IHDR-only checks are not enough). The
                 zlib stream must be complete (`eof`, no unused data, no unconsumed tail),
                 and the image file is read with a hard cap so an oversized screenshot is
                 refused before it is loaded.
    title        the window title must equal the pinned `COS Desktop`; a different or
                 missing title is refused, never recorded as whatever was found.

Nothing on the desktop is changed to make a capture possible: there is no raise, no
focus request, no z-order operation and no foreground-permission grant. The foreground
is the user's action: bring COS Desktop to the front (or run the documented command from
a terminal that is already foreground) while `--wait-for-foreground <seconds>` (bounded
0..120, polled every 250 ms with `GetForegroundWindow` only) watches for it. A wait that
expires records `NOT_MEASURED / FOREGROUND_WAIT_EXPIRED` with the elapsed wait and writes
no screenshot; an occluded target is `NOT_MEASURED / WINDOW_OCCLUDED`. An observation
never leaves a side effect behind and never takes over the desktop.

The acceptance order is 100 % first, then 125/150/175/200 %: each scale is a separate
human-driven observation, and this tool cannot change the display scale itself.
    live check   this process re-queries the HWND itself (IsWindow,
                 GetWindowThreadProcessId, GetWindowTextW, GetWindowRect) before the
                 application is stopped; any disagreement refuses the capture

If the probe cannot capture or cannot prove identity, the screenshot becomes
`NOT_MEASURED` with the closed reason and the run keeps the UI Automation evidence; a
missing expected label is a failure, never a silent pass.

Closed error vocabulary (never a traceback):

    ARGS, ARGS_EXE, EXE_MISSING, OUT_EXISTS, LAUNCH_FAILED, PROBE_FAILED,
    PROBE_TIMEOUT, WINDOW_NOT_FOUND, RECORD_READ, RECORD_SCHEMA,
    SCREENSHOT_MISMATCH, PNG_INVALID, DPI_UNAVAILABLE, STOP_FAILED, WRITE_FAILED,
    CAPTURE_NOT_BOUND, CAPTURE_IDENTITY_MISMATCH, CAPTURE_CONTENT_MISMATCH

Everything this process cannot establish stays NOT_MEASURED with a reason and a human
procedure; nothing here declares MP-TYP1 complete.
"""

from __future__ import annotations

import argparse
import ctypes
import datetime
import hashlib
import json
import math
import re
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path
from typing import Any

OBSERVATION_VERSION = 2
# Pinned: a window with any other title is not the shell this harness observes.
EXPECTED_TITLE = "COS Desktop"
MAX_PNG_CHUNKS = 4096
MAX_CHUNK_BYTES = 4 * 1024 * 1024
MAX_IDAT_BYTES = 32 * 1024 * 1024
MAX_DECODED_BYTES = 64 * 1024 * 1024
MAX_RECORD_BYTES = 16 * 1024 * 1024
MAX_ATTEMPTS = 40
ATTEMPT_DELAY_S = 0.4
PROBE_TIMEOUT_S = 130.0
FOREGROUND_POLL_INTERVAL_S = 0.25
MAX_FOREGROUND_WAIT_S = 120.0
REASON_FOREGROUND_WAIT_EXPIRED = "FOREGROUND_WAIT_EXPIRED"
MAX_SCREENSHOT_BYTES = 32 * 1024 * 1024
STOP_TIMEOUT_S = 10.0
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")

BOUND_CAPTURE_METHOD = "PrintWindow(PW_RENDERFULLCONTENT)"
CROSS_CHECK_SAMPLES = 49
CROSS_CHECK_REQUIRED = 45
CROSS_CHECK_DISTINCT_COLORS = 4

EXPECTED_LATIN = ("Task Rail", "Inspector", "Context Bar", "Candidate Dock")
EXPECTED_JAPANESE = ("新製品キービジュアル",)
MIXED_SCRIPT_CAP = 8

ERROR_CODES = (
    "ARGS",
    "ARGS_EXE",
    "EXE_MISSING",
    "OUT_EXISTS",
    "LAUNCH_FAILED",
    "PROBE_FAILED",
    "PROBE_TIMEOUT",
    "WINDOW_NOT_FOUND",
    "RECORD_READ",
    "RECORD_SCHEMA",
    "SCREENSHOT_MISMATCH",
    "PNG_INVALID",
    "DPI_UNAVAILABLE",
    "STOP_FAILED",
    "WRITE_FAILED",
    "CAPTURE_NOT_BOUND",
    "CAPTURE_IDENTITY_MISMATCH",
    "CAPTURE_CONTENT_MISMATCH",
    "WINDOW_TITLE_MISMATCH",
    "PNG_TRUNCATED",
    "PNG_UNDECODABLE",
    "PNG_TRAILING_DATA",
    "PNG_TOO_LARGE",
    "SCREENSHOT_GEOMETRY_MISMATCH",
)

NOT_MEASURED = (
    {
        "item": "display_scale_ladder",
        "reason": "changing the display scale needs interactive Windows display control; "
        "the observation below is the host's current scale, not a ladder step",
        "human_procedure": "bring COS Desktop to the foreground (or run from a foreground "
        "terminal), then Settings > System > Display > Scale: 100 % first, then 125/150/"
        "175/200 %, one observation per scale: cargo run --locked -p cos-desktop -- "
        "--fixture",
    },
    {
        "item": "pixel_evidence_for_wired_typography",
        "reason": "no identity-proven screenshot exists for this run; the window cannot be "
        "foregrounded from here, and a pixel claim without that capture would be fabricated",
        "human_procedure": "bring COS Desktop to the foreground, then run: python -B "
        "apps/desktop/devtools/typography/host_observe.py --out <fresh dir>; require "
        "screenshot.status OBSERVED with identity in the record",
    },
    {
        "item": "structural_hairline_drawn",
        "reason": "a drawn hairline is a pixel fact; it cannot be read from the AX tree and "
        "no identity-proven screenshot exists for this run",
        "human_procedure": "after foregrounding and capturing as above, check the dock group "
        "separator is a visible one-physical-pixel line in the screenshot",
    },
    {
        "item": "cross_monitor_move",
        "reason": "moving a live window between monitors needs interactive input",
        "human_procedure": "drag the window across two monitors with different scales and "
        "check the separator, focus ring and truncation at the boundary",
    },
    {
        "item": "mixed_script_visual_quality",
        "reason": "tofu, clipping and baseline jitter need native rendering and inspection",
        "human_procedure": "at each scale, inspect Japanese/Latin strings in the task rail, "
        "dock and stop panel for missing glyphs, clipping and baseline jitter",
    },
    {
        "item": "resolved_face_per_run",
        "reason": "which family DirectWrite selects per run is a native rasterisation fact",
        "human_procedure": "at each scale, confirm Latin runs use Segoe UI Variable (or "
        "Segoe UI) and Japanese runs render without fallback-tofu",
    },
    {
        "item": "hairline_blur_and_pixel_snapping",
        "reason": "sub-pixel separator sharpness needs native screenshots at fractional DPI",
        "human_procedure": "at 125/150/175/200 %, check the dock group separator stays one "
        "crisp physical pixel and does not grey out",
    },
    {
        "item": "high_contrast_focus",
        "reason": "High Contrast theming needs native theme switching and inspection",
        "human_procedure": "enable Settings > Accessibility > Contrast themes and confirm the "
        "2-DIP focus ring stays visible on every control",
    },
    {
        "item": "text_layout_frame_cost",
        "reason": "no native frame instrumentation is available to this harness",
        "human_procedure": "run the R0 instrumented build at each scale and compare "
        "text/layout frame cost against the ratified §10 gates",
    },
    {
        "item": "focus_selection_scroll_retention_across_dpi_change",
        "reason": "a DPI change cannot be triggered from this process without native display "
        "control",
        "human_procedure": "focus a task row, scroll the rail and dock, change the scale, and "
        "confirm focus, selection and scroll anchor survive",
    },
)


class ObserveError(Exception):
    """One closed code; never input text, a path or a traceback."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def fail(code: str) -> None:
    raise ObserveError(code if code in ERROR_CODES else "PROBE_FAILED")


def sha256_bytes(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def read_bounded_stream(stream: Any, cap: int) -> bytes:
    """Read at most cap + 1 bytes; more than cap is a closed PNG_TOO_LARGE."""
    try:
        data = stream.read(cap + 1)
    except OSError:
        fail("SCREENSHOT_MISMATCH")
    if not isinstance(data, bytes):
        fail("SCREENSHOT_MISMATCH")
    if len(data) > cap:
        fail("PNG_TOO_LARGE")
    return data


def read_bounded(path: Path, *, cap: int = MAX_SCREENSHOT_BYTES) -> bytes:
    """Read the screenshot with the cap enforced at read time.

    The size is checked before opening, and the open handle is read with `cap + 1` as its
    maximum so an oversized input is never loaded into memory in full.
    """
    try:
        size = path.stat().st_size
    except OSError:
        fail("SCREENSHOT_MISMATCH")
    if size > cap:
        fail("PNG_TOO_LARGE")
    try:
        with open(path, "rb") as handle:
            return read_bounded_stream(handle, cap)
    except OSError:
        fail("SCREENSHOT_MISMATCH")


def png_dimensions_verified(
    data: bytes,
    *,
    max_idat: int = MAX_IDAT_BYTES,
    max_decoded: int = MAX_DECODED_BYTES,
) -> tuple[int, int]:
    """Verify the PNG is structurally complete and decodable within bounded caps.

    Walks every chunk to IEND with matching CRCs, requires IHDR first and IDAT present,
    refuses interlaced/unsupported layouts, then inflates the image data at a bounded
    length and checks every scanline filter byte. IHDR alone is never trusted.
    """
    if len(data) < 8 or data[:8] != PNG_SIGNATURE:
        fail("PNG_INVALID")
    offset = 8
    chunks = 0
    header = None
    image_data = bytearray()
    seen_end = False
    while offset < len(data):
        if chunks >= MAX_PNG_CHUNKS:
            fail("PNG_TOO_LARGE")
        if offset + 8 > len(data):
            fail("PNG_TRUNCATED")
        length = int.from_bytes(data[offset:offset + 4], "big")
        kind = data[offset + 4:offset + 8]
        if length > MAX_CHUNK_BYTES:
            fail("PNG_TOO_LARGE")
        payload_end = offset + 8 + length
        if payload_end + 4 > len(data):
            fail("PNG_TRUNCATED")
        payload = data[offset + 8:payload_end]
        crc = int.from_bytes(data[payload_end:payload_end + 4], "big")
        if zlib.crc32(kind + payload) & 0xFFFFFFFF != crc:
            fail("PNG_TRUNCATED")
        if chunks == 0:
            if kind != b"IHDR" or length != 13:
                fail("PNG_INVALID")
            header = struct.unpack(">IIBBBBB", payload)
        elif kind == b"IDAT":
            if len(image_data) + length > max_idat:
                # The effective bound is enforced while walking, before accumulating.
                fail("PNG_TOO_LARGE")
            image_data += payload
        elif kind == b"IEND":
            if length != 0:
                fail("PNG_INVALID")
            seen_end = True
            offset = payload_end + 4
            break
        offset = payload_end + 4
        chunks += 1
    if not seen_end:
        fail("PNG_TRUNCATED")
    if offset != len(data):
        fail("PNG_INVALID")
    if header is None or not image_data:
        fail("PNG_INVALID")
    width, height, bit_depth, color_type, compression, filter_method, interlace = header
    if width <= 0 or height <= 0:
        fail("PNG_INVALID")
    if compression != 0 or filter_method != 0 or interlace != 0:
        # Interlaced/palette/low-bit-depth images are not produced by the probe and would
        # need a full decoder; refuse rather than guess.
        fail("PNG_INVALID")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if channels is None or bit_depth not in (8, 16):
        fail("PNG_INVALID")
    scanline = (width * bit_depth * channels + 7) // 8
    expected = height * (1 + scanline)
    if expected > max_decoded:
        fail("PNG_TOO_LARGE")
    try:
        inflater = zlib.decompressobj()
        raw = inflater.decompress(bytes(image_data), expected + 1)
    except zlib.error:
        fail("PNG_UNDECODABLE")
    if not inflater.eof:
        # The stream never reached its end marker (for example a stripped Adler-32):
        # complete scanlines are not proof of a complete stream.
        fail("PNG_UNDECODABLE")
    if len(raw) != expected:
        fail("PNG_UNDECODABLE")
    if inflater.unused_data or inflater.unconsumed_tail:
        # Bytes after the end of the zlib stream are not this image.
        fail("PNG_TRAILING_DATA")
    for row in range(height):
        if raw[row * (1 + scanline)] > 4:
            fail("PNG_UNDECODABLE")
    return width, height


def has_japanese(text: str) -> bool:
    return any(
        "\u3040" <= char <= "\u30ff"
        or "\u3400" <= char <= "\u4dbf"
        or "\u4e00" <= char <= "\u9fff"
        or "\uf900" <= char <= "\ufaff"
        or "\uff66" <= char <= "\uff9d"
        for char in text
    )


def has_latin(text: str) -> bool:
    return any(("A" <= char <= "Z") or ("a" <= char <= "z") for char in text)


def analyze_elements(elements: list[dict[str, Any]]) -> dict[str, Any]:
    """Pure label analysis: expected anchors, script coverage and mixed-script rows."""
    names = [item["name"] for item in elements if isinstance(item.get("name"), str)]
    populated = [name for name in names if name]
    japanese = [name for name in populated if has_japanese(name)]
    latin = [name for name in populated if has_latin(name)]
    mixed = [name for name in populated if has_japanese(name) and has_latin(name)]
    found_latin = [label for label in EXPECTED_LATIN if any(label in name for name in populated)]
    found_japanese = [
        label for label in EXPECTED_JAPANESE if any(label in name for name in populated)
    ]
    return {
        "element_count": len(elements),
        "named_element_count": len(populated),
        "japanese_name_count": len(japanese),
        "latin_name_count": len(latin),
        "mixed_script_count": len(mixed),
        "mixed_script_examples": mixed[:MIXED_SCRIPT_CAP],
        "expected_latin_found": found_latin,
        "expected_latin_missing": [
            label for label in EXPECTED_LATIN if label not in found_latin
        ],
        "expected_japanese_found": found_japanese,
        "expected_japanese_missing": [
            label for label in EXPECTED_JAPANESE if label not in found_japanese
        ],
    }


def evaluate_labels(analysis: dict[str, Any], ax_status: str) -> list[dict[str, Any]]:
    """Every expected label must be present; a subset or a missing label fails."""
    if ax_status != "OBSERVED":
        return [
            {
                "name": "latin_labels_present",
                "status": "NOT_MEASURED",
                "detail": "UI Automation tree unavailable",
            },
            {
                "name": "japanese_label_present",
                "status": "NOT_MEASURED",
                "detail": "UI Automation tree unavailable",
            },
            {
                "name": "mixed_script_presence_in_ax_tree",
                "status": "NOT_MEASURED",
                "detail": "UI Automation tree unavailable",
            },
        ]
    return [
        {
            "name": "latin_labels_present",
            "status": "PASS" if not analysis["expected_latin_missing"] else "FAIL",
            "detail": f"found {analysis['expected_latin_found']}, missing "
            f"{analysis['expected_latin_missing']}",
        },
        {
            "name": "japanese_label_present",
            "status": "PASS" if not analysis["expected_japanese_missing"] else "FAIL",
            "detail": f"found {analysis['expected_japanese_found']}, missing "
            f"{analysis['expected_japanese_missing']}",
        },
        {
            "name": "mixed_script_presence_in_ax_tree",
            "status": "PASS" if analysis["mixed_script_count"] > 0 else "FAIL",
            "detail": f"{analysis['mixed_script_count']} element names carry both scripts",
        },
    ]


def scale_from_dpi(dpi: int) -> float:
    if type(dpi) is not int or dpi <= 0:
        fail("DPI_UNAVAILABLE")
    return dpi / 96.0


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    ]


def live_window(handle: int) -> dict[str, Any]:
    """Independent re-query of the captured HWND through Win32, or a closed reason."""
    try:
        user32 = ctypes.windll.user32
        user32.IsWindow.argtypes = [ctypes.c_void_p]
        user32.IsWindow.restype = ctypes.c_bool
        user32.GetWindowThreadProcessId.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint),
        ]
        user32.GetWindowThreadProcessId.restype = ctypes.c_uint
        user32.GetWindowTextW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_int]
        user32.GetWindowTextW.restype = ctypes.c_int
        user32.GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.POINTER(RECT)]
        user32.GetWindowRect.restype = ctypes.c_bool
        handle_pointer = ctypes.c_void_p(handle)
        if not user32.IsWindow(handle_pointer):
            return {"status": "NOT_MEASURED", "reason": "WINDOW_GONE"}
        pid = ctypes.c_uint(0)
        user32.GetWindowThreadProcessId(handle_pointer, ctypes.byref(pid))
        title = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(handle_pointer, title, 511)
        rect = RECT()
        if not user32.GetWindowRect(handle_pointer, ctypes.byref(rect)):
            return {"status": "NOT_MEASURED", "reason": "WINDOW_RECT_UNAVAILABLE"}
        return {
            "status": "OBSERVED",
            "pid": int(pid.value),
            "title": title.value,
            "rect": {
                "left": int(rect.left),
                "top": int(rect.top),
                "width": int(rect.right - rect.left),
                "height": int(rect.bottom - rect.top),
            },
        }
    except (AttributeError, OSError, TypeError, ValueError):
        return {"status": "NOT_MEASURED", "reason": "WIN32_UNAVAILABLE"}


def host_scale(handle: int) -> dict[str, Any]:
    """The observed scale of the *captured window*; never a controlled ladder step."""
    try:
        user32 = ctypes.windll.user32
        user32.GetDpiForWindow.argtypes = [ctypes.c_void_p]
        user32.GetDpiForWindow.restype = ctypes.c_uint
        dpi = int(user32.GetDpiForWindow(ctypes.c_void_p(handle)))
        if dpi > 0:
            return {
                "status": "OBSERVED",
                "dpi": dpi,
                "scale": scale_from_dpi(dpi),
                "method": "GetDpiForWindow",
                "controlled_ladder_step": False,
            }
    except (AttributeError, OSError, TypeError, ValueError):
        pass
    try:
        import winreg

        with winreg.OpenKey(
            winreg.HKEY_CURRENT_USER, r"Control Panel\Desktop\WindowMetrics"
        ) as key:
            value, _ = winreg.QueryValueEx(key, "AppliedDPI")
        dpi = int(value)
        return {
            "status": "OBSERVED",
            "dpi": dpi,
            "scale": scale_from_dpi(dpi),
            "method": "HKCU AppliedDPI fallback",
            "controlled_ladder_step": False,
        }
    except (ImportError, OSError, TypeError, ValueError):
        fail("DPI_UNAVAILABLE")


def read_json(path: Path) -> dict[str, Any]:
    try:
        size = path.stat().st_size
    except OSError:
        fail("RECORD_READ")
    if size > MAX_RECORD_BYTES:
        fail("RECORD_READ")
    try:
        document = json.loads(path.read_bytes().decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        fail("RECORD_READ")
    if not isinstance(document, dict):
        fail("RECORD_SCHEMA")
    return document


def _window(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("status") != "OBSERVED":
        fail("RECORD_SCHEMA")
    for key in ("pid", "handle", "left", "top", "width", "height"):
        if type(value.get(key)) is not int:
            fail("RECORD_SCHEMA")
    if value.get("title") != EXPECTED_TITLE:
        fail("WINDOW_TITLE_MISMATCH")
    if value["width"] <= 0 or value["height"] <= 0:
        fail("RECORD_SCHEMA")
    return value


def _rect(window: dict[str, Any]) -> dict[str, int]:
    return {
        "left": window["left"],
        "top": window["top"],
        "width": window["width"],
        "height": window["height"],
    }


def validate_probe_record(document: dict[str, Any], pid: int, screenshot: Path) -> dict[str, Any]:
    """Validate the probe record; image bytes are hashed here, identity is proven later."""
    if not isinstance(document, dict) or document.get("ok") is not True:
        fail("RECORD_SCHEMA")
    window = _window(document.get("window"))
    shot = document.get("screenshot")
    uia = document.get("uia")
    if not isinstance(shot, dict) or not isinstance(uia, dict):
        fail("RECORD_SCHEMA")
    if window["pid"] != pid:
        fail("RECORD_SCHEMA")
    if uia.get("status") not in ("OBSERVED", "NOT_MEASURED"):
        fail("RECORD_SCHEMA")
    if not isinstance(uia.get("elements"), list):
        fail("RECORD_SCHEMA")
    if shot.get("status") == "NOT_MEASURED":
        if not isinstance(shot.get("reason"), str) or not shot["reason"]:
            fail("RECORD_SCHEMA")
        return {
            "window": window,
            "screenshot": {"status": "NOT_MEASURED", "reason": shot["reason"]},
            "uia": uia,
        }
    if shot.get("status") != "OBSERVED":
        fail("RECORD_SCHEMA")
    data = read_bounded(screenshot)
    width, height = png_dimensions_verified(data)
    if (width, height) != (shot.get("width"), shot.get("height")):
        fail("SCREENSHOT_MISMATCH")
    if (width, height) != (window["width"], window["height"]):
        fail("SCREENSHOT_GEOMETRY_MISMATCH")
    return {
        "window": window,
        "screenshot": {
            "status": "OBSERVED",
            "digest": sha256_bytes(data),
            "ihdr": {"width": width, "height": height},
            "bytes": len(data),
            "path": str(screenshot).replace("\\", "/"),
            "raw": shot,
        },
        "uia": uia,
    }


def verify_capture_identity(
    captured: dict[str, Any],
    window: dict[str, Any],
    launched_pid: int,
    live: dict[str, Any],
) -> dict[str, Any]:
    """Refuse a capture that is not provably the target window (pure)."""
    if captured.get("method") != BOUND_CAPTURE_METHOD:
        fail("CAPTURE_NOT_BOUND")
    if captured.get("handle") != window["handle"]:
        fail("CAPTURE_IDENTITY_MISMATCH")
    if captured.get("pid") != launched_pid or window["pid"] != launched_pid:
        fail("CAPTURE_IDENTITY_MISMATCH")
    if captured.get("title") != window["title"]:
        fail("CAPTURE_IDENTITY_MISMATCH")
    expected_rect = _rect(window)
    if captured.get("rect") != expected_rect:
        fail("CAPTURE_IDENTITY_MISMATCH")
    if captured.get("still_current") is not True:
        fail("CAPTURE_IDENTITY_MISMATCH")
    cross = captured.get("content_cross_check")
    if not isinstance(cross, dict) or cross.get("status") != "OBSERVED":
        fail("CAPTURE_CONTENT_MISMATCH")
    samples = cross.get("samples")
    matched = cross.get("matched")
    colors = cross.get("distinct_sampled_colors")
    non_black = cross.get("non_black_samples")
    if type(samples) is not int or type(matched) is not int:
        fail("CAPTURE_CONTENT_MISMATCH")
    if type(colors) is not int or type(non_black) is not int:
        fail("CAPTURE_CONTENT_MISMATCH")
    if samples < CROSS_CHECK_SAMPLES or matched < CROSS_CHECK_REQUIRED:
        fail("CAPTURE_CONTENT_MISMATCH")
    if colors < CROSS_CHECK_DISTINCT_COLORS or non_black < CROSS_CHECK_REQUIRED:
        fail("CAPTURE_CONTENT_MISMATCH")
    if live.get("status") != "OBSERVED":
        fail("CAPTURE_IDENTITY_MISMATCH")
    if (
        live.get("pid") != launched_pid
        or live.get("title") != window["title"]
        or live.get("rect") != expected_rect
    ):
        fail("CAPTURE_IDENTITY_MISMATCH")
    return {
        "method": captured["method"],
        "handle": captured["handle"],
        "pid": launched_pid,
        "title": captured["title"],
        "rect": expected_rect,
        "still_current": True,
        "foreground_at_capture": captured.get("foreground_at_capture"),
        "content_cross_check": {
            "status": "OBSERVED",
            "samples": samples,
            "matched": matched,
            "tolerance": cross.get("tolerance"),
            "distinct_sampled_colors": colors,
            "non_black_samples": non_black,
            "visible_region": cross.get("visible_region"),
            "method": cross.get("method"),
        },
        "live_reverification": "IsWindow, GetWindowThreadProcessId, GetWindowTextW and "
        "GetWindowRect agreed with the capture before the application was stopped",
        "desktop_state_changed": False,
        "screen_copy_used_for_capture": False,
    }


def foreground_identity() -> dict[str, Any]:
    """Read-only identity of the current foreground window."""
    try:
        user32 = ctypes.windll.user32
        user32.GetForegroundWindow.restype = ctypes.c_void_p
        handle = int(user32.GetForegroundWindow() or 0)
        if handle == 0:
            return {"status": "OBSERVED", "handle": 0, "pid": 0, "title": ""}
        info = live_window(handle)
        if info.get("status") == "OBSERVED":
            info["handle"] = handle
        return info
    except (AttributeError, OSError, TypeError, ValueError):
        return {"status": "NOT_MEASURED", "reason": "WIN32_UNAVAILABLE"}


def bounded_wait(value: str) -> float:
    """Validate --wait-for-foreground: 0..MAX, finite; anything else is ARGS."""
    try:
        seconds = float(value)
    except (TypeError, ValueError):
        fail("ARGS")
    if not math.isfinite(seconds) or seconds < 0.0 or seconds > MAX_FOREGROUND_WAIT_S:
        fail("ARGS")
    return seconds


def wait_for_foreground(
    pid: int,
    timeout_s: float,
    *,
    poll: Any = None,
    clock: Any = time.monotonic,
    sleep: Any = time.sleep,
    interval_s: float = FOREGROUND_POLL_INTERVAL_S,
) -> dict[str, Any]:
    """Observe only: poll GetForegroundWindow until it is our window, or the wait expires.

    No window is raised, focused, moved, restacked or granted foreground rights. The poll
    results and the clock are injectable so the transition and expiry paths are testable.
    """
    observed_foreground = poll if poll is not None else foreground_identity
    started = clock()
    polls = 0
    last: dict[str, Any] = {"status": "NOT_MEASURED", "reason": "WIN32_UNAVAILABLE"}
    while True:
        polls += 1
        last = observed_foreground()
        if (
            last.get("status") == "OBSERVED"
            and last.get("pid") == pid
            and last.get("title") == EXPECTED_TITLE
        ):
            return {
                "mode": "read-only observation of GetForegroundWindow",
                "outcome": "FOREGROUNDED",
                "requested_s": timeout_s,
                "elapsed_s": round(clock() - started, 3),
                "polls": polls,
                "handle": last.get("handle"),
            }
        elapsed = clock() - started
        if elapsed >= timeout_s:
            return {
                "mode": "read-only observation of GetForegroundWindow",
                "outcome": REASON_FOREGROUND_WAIT_EXPIRED,
                "requested_s": timeout_s,
                "elapsed_s": round(elapsed, 3),
                "polls": polls,
                "last_foreground": {"pid": last.get("pid"), "title": last.get("title")},
            }
        sleep(interval_s)


def resolve_screenshot(shot: dict[str, Any], wait: dict[str, Any]) -> dict[str, Any]:
    """A capture always wins; otherwise the wait decides the closed NOT_MEASURED reason."""
    if shot.get("status") == "OBSERVED":
        return shot
    if wait.get("outcome") == REASON_FOREGROUND_WAIT_EXPIRED:
        return {
            "status": "NOT_MEASURED",
            "reason": REASON_FOREGROUND_WAIT_EXPIRED,
            "detail": shot.get("reason"),
            "elapsed_s": wait.get("elapsed_s"),
        }
    return {"status": "NOT_MEASURED", "reason": shot.get("reason")}


def observe_window(
    powershell: str, probe: Path, pid: int, screenshot: Path
) -> dict[str, Any]:
    """Run the packaged independent probe until the new window exists."""
    command = [
        powershell,
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        str(probe),
        "-Screenshot",
        str(screenshot),
        "-ProcessId",
        str(pid),
        "-MaxElements",
        "400",
        "-MaxDepth",
        "12",
        "-MaxScreenshotBytes",
        str(MAX_SCREENSHOT_BYTES),
    ]
    last_code = "WINDOW_NOT_FOUND"
    for _ in range(MAX_ATTEMPTS):
        try:
            done = subprocess.run(
                command, capture_output=True, check=False, timeout=PROBE_TIMEOUT_S
            )
        except subprocess.TimeoutExpired:
            fail("PROBE_TIMEOUT")
        except OSError:
            fail("PROBE_FAILED")
        try:
            summary = json.loads(done.stdout.decode("utf-8").strip() or "{}")
        except (UnicodeDecodeError, json.JSONDecodeError):
            fail("PROBE_FAILED")
        if isinstance(summary, dict) and summary.get("ok") is True:
            return summary
        last_code = (
            summary.get("error") if isinstance(summary.get("error"), str) else "PROBE_FAILED"
        )
        if last_code != "WINDOW_NOT_FOUND":
            fail(last_code)
        time.sleep(ATTEMPT_DELAY_S)
    fail(last_code)


def launch(exe: Path, log_path: Path) -> subprocess.Popen:
    try:
        log = open(log_path, "wb")
    except OSError:
        fail("LAUNCH_FAILED")
    try:
        return subprocess.Popen(
            [str(exe), "--fixture"],
            cwd=str(exe.parent),
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
    except OSError:
        log.close()
        fail("LAUNCH_FAILED")


def stop(process: subprocess.Popen) -> dict[str, Any]:
    try:
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            capture_output=True,
            check=False,
            timeout=STOP_TIMEOUT_S,
        )
    except (OSError, subprocess.TimeoutExpired):
        fail("STOP_FAILED")
    try:
        code = process.wait(timeout=STOP_TIMEOUT_S)
    except subprocess.TimeoutExpired:
        fail("STOP_FAILED")
    return {"method": "taskkill /T /F", "exit_code": code}


def result_of(checks: list[dict[str, Any]]) -> str:
    if any(check["status"] == "FAIL" for check in checks):
        return "FAIL"
    if any(check["status"] == "NOT_MEASURED" for check in checks):
        return "PASS_WITH_NOT_MEASURED"
    return "PASS"


def build_record(
    exe: Path,
    exe_digest: str,
    observed: dict[str, Any],
    scale: dict[str, Any],
    analysis: dict[str, Any],
    stop_record: dict[str, Any],
    wait: dict[str, Any] | None = None,
) -> dict[str, Any]:
    window = observed["window"]
    screenshot = observed["screenshot"]
    uia = observed["uia"]
    ax_status = "OBSERVED" if uia["status"] == "OBSERVED" else "NOT_MEASURED"
    if screenshot["status"] == "OBSERVED":
        identity = screenshot["identity"]
        shot_detail = (
            f"{identity['method']} on HWND {identity['handle']} with a verified "
            f"foreground content cross-check "
            f"({identity['content_cross_check']['matched']}/"
            f"{identity['content_cross_check']['samples']} samples)"
        )
    else:
        shot_detail = f"refused: {screenshot['reason']}"
        if screenshot.get("detail"):
            shot_detail += f" (probe: {screenshot['detail']})"
        if screenshot.get("elapsed_s") is not None:
            shot_detail += f" after {screenshot['elapsed_s']}s of read-only waiting"
    checks: list[dict[str, Any]] = [
        {
            "name": "window_observed",
            "status": "PASS",
            "detail": f"{window['title']} {window['width']}x{window['height']} at "
            f"({window['left']}, {window['top']})",
        },
        {
            "name": "screenshot_is_the_target_window",
            "status": "PASS" if screenshot["status"] == "OBSERVED" else "NOT_MEASURED",
            "detail": shot_detail,
        },
        {
            "name": "ax_tree_observed",
            "status": "PASS" if ax_status == "OBSERVED" else "NOT_MEASURED",
            "detail": (
                f"{analysis['element_count']} elements, "
                f"{analysis['named_element_count']} named"
                if ax_status == "OBSERVED"
                else uia.get("reason", "")
            ),
        },
    ]
    checks.extend(evaluate_labels(analysis, ax_status))
    return {
        "typography_observation_version": OBSERVATION_VERSION,
        "observed_utc": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "executable": {"path": str(exe).replace("\\", "/"), "digest": exe_digest},
        "window": window,
        "screenshot": screenshot,
        "uia": {
            "status": uia["status"],
            "reason": uia.get("reason", ""),
            "element_count": analysis["element_count"],
            "named_element_count": analysis["named_element_count"],
            "truncated": uia.get("truncated", False),
            "depth_limited": uia.get("depth_limited", False),
            "elements_visited": uia.get("elements_visited", 0),
        },
        "labels": {
            "expected_latin": list(EXPECTED_LATIN),
            "expected_japanese": list(EXPECTED_JAPANESE),
            "found_latin": analysis["expected_latin_found"],
            "missing_latin": analysis["expected_latin_missing"],
            "found_japanese": analysis["expected_japanese_found"],
            "missing_japanese": analysis["expected_japanese_missing"],
            "japanese_name_count": analysis["japanese_name_count"],
            "latin_name_count": analysis["latin_name_count"],
            "mixed_script_count": analysis["mixed_script_count"],
            "mixed_script_examples": analysis["mixed_script_examples"],
        },
        "scale": scale,
        "launch": {
            "procedure": "the documented human command: the fixture binary is started "
            "with --fixture and creates/shows its own window",
            "window_created_by": "the application",
            "foreground": "the user's action: the tool never grants, requests or takes "
            "the foreground, so whether the window ends up in front is not the tool",
            "foreground_grant": "none",
            "probe_state_change": "none: no raise, focus, move or z-order operation",
        },
        "foreground_wait": wait
        if wait is not None
        else {"outcome": "NOT_REQUESTED", "requested_s": 0.0},
        "desktop_state_changed": False,
        "acceptance_procedure": {
            "human_step": "the user brings COS Desktop to the foreground (click its window "
            "or taskbar entry) or runs the documented command from a terminal that is "
            "already foreground; the tool never does this",
            "capture_step": "the observation then captures the target HWND with "
            "PrintWindow, re-queries the HWND identity and cross-checks the display over "
            "the window's on-screen region",
            "no_automation": "no raise, focus, move, z-order change or foreground grant is "
            "performed by the tool; an occluded window is NOT_MEASURED / WINDOW_OCCLUDED",
            "scale_order": ["100 %", "125 %", "150 %", "175 %", "200 %"],
            "scale_procedure": "Settings > System > Display > Scale; one observation per "
            "scale, 100 % first",
        },
        "checks": checks,
        "result": result_of(checks),
        "app_stop": stop_record,
        "not_measured": list(NOT_MEASURED),
        "declares_mptyp1_complete": False,
        "statement": "Host-scale observation only. A screenshot is recorded only with a "
        "bound-capture identity proof; otherwise it stays NOT_MEASURED. This does not "
        "declare MP-TYP1 complete.",
    }


def write_record(record: dict[str, Any], path: Path) -> str:
    body = json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    try:
        path.write_bytes(body)
    except OSError:
        fail("WRITE_FAILED")
    return sha256_bytes(body)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", default=str(default_exe()))
    parser.add_argument("--out", required=True, help="fresh output directory")
    parser.add_argument("--powershell", default="powershell")
    parser.add_argument("--probe", default=str(default_probe()))
    parser.add_argument(
        "--wait-for-foreground",
        default="0",
        help="bounded read-only wait in seconds (0..%s) for the user to foreground the "
        "window; polled every %ss with GetForegroundWindow only"
        % (MAX_FOREGROUND_WAIT_S, FOREGROUND_POLL_INTERVAL_S),
    )
    args = parser.parse_args(argv)
    process: subprocess.Popen | None = None
    try:
        exe = Path(args.exe)
        if not exe.is_file():
            fail("EXE_MISSING")
        probe = Path(args.probe)
        if not probe.is_file():
            fail("PROBE_FAILED")
        out = Path(args.out)
        if out.exists():
            fail("OUT_EXISTS")
        try:
            out.mkdir(parents=True)
        except OSError:
            fail("WRITE_FAILED")
        probe_record = out / "probe.json"
        screenshot = out / "screenshot.png"
        exe_digest = sha256_bytes(exe.read_bytes())
        wait_s = bounded_wait(args.wait_for_foreground)
        process = launch(exe, out / "app.log")
        wait = wait_for_foreground(process.pid, wait_s)
        document = observe_window(
            args.powershell, probe, process.pid, screenshot
        )
        observed = validate_probe_record(document, process.pid, screenshot)
        if observed["screenshot"]["status"] == "OBSERVED":
            live = live_window(observed["window"]["handle"])
            identity = verify_capture_identity(
                observed["screenshot"]["raw"], observed["window"], process.pid, live
            )
            observed["screenshot"] = {
                "status": "OBSERVED",
                "method": identity["method"],
                "identity": identity,
                "digest": observed["screenshot"]["digest"],
                "ihdr": observed["screenshot"]["ihdr"],
                "bytes": observed["screenshot"]["bytes"],
                "path": observed["screenshot"]["path"],
            }
        else:
            observed["screenshot"] = resolve_screenshot(observed["screenshot"], wait)
        scale = host_scale(observed["window"]["handle"])
        analysis = analyze_elements(observed["uia"]["elements"])
        stop_record = stop(process)
        process = None
        record = build_record(
            exe, exe_digest, observed, scale, analysis, stop_record, wait
        )
        try:
            (out / "probe.json").write_text(
                json.dumps(document, sort_keys=True, separators=(",", ":")), encoding="utf-8"
            )
        except OSError:
            fail("WRITE_FAILED")
        digest = write_record(record, out / "observation.json")
        print(
            json.dumps(
                {
                    "ok": True,
                    "record": str(out / "observation.json"),
                    "record_digest": digest,
                    "result": record["result"],
                    "window": {
                        "title": record["window"]["title"],
                        "width": record["window"]["width"],
                        "height": record["window"]["height"],
                    },
                    "screenshot": (
                        record["screenshot"]["digest"]
                        if record["screenshot"]["status"] == "OBSERVED"
                        else f"NOT_MEASURED:{record['screenshot']['reason']}"
                    ),
                    "foreground_wait": {
                        "outcome": record["foreground_wait"]["outcome"],
                        "elapsed_s": record["foreground_wait"].get("elapsed_s"),
                    },
                    "scale": record["scale"]["scale"],
                    "ax_elements": record["uia"]["element_count"],
                    "labels_missing": record["labels"]["missing_latin"]
                    + record["labels"]["missing_japanese"],
                    "mixed_script_count": record["labels"]["mixed_script_count"],
                },
                separators=(",", ":"),
            )
        )
        return 2 if record["result"] == "FAIL" else 0
    except ObserveError as error:
        print(json.dumps({"ok": False, "error": error.code}, separators=(",", ":")))
    except (OSError, ValueError, TypeError):
        print(json.dumps({"ok": False, "error": "PROBE_FAILED"}, separators=(",", ":")))
    finally:
        if process is not None:
            try:
                subprocess.run(
                    ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                    capture_output=True,
                    check=False,
                    timeout=STOP_TIMEOUT_S,
                )
            except (OSError, subprocess.TimeoutExpired):
                pass
    return 1


def default_exe() -> Path:
    return Path(__file__).resolve().parents[4] / "target/debug/cos-desktop.exe"


def default_probe() -> Path:
    return Path(__file__).resolve().parents[1] / "r0/n03_probe.ps1"


if __name__ == "__main__":
    sys.exit(main())
