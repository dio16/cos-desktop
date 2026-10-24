"""MP-TYP1-C: observe the built fixture shell through the existing independent probe.

Python standard library only. This module launches the already-built fixture binary
and hands the window to the repository's independent observer,
`apps/desktop/devtools/r0/native_observe.py` (which runs `n03_probe.ps1`). It never
reads application-private state and never claims a controlled DPI step: the window is
captured at whatever scale this host currently uses.

Observed here:
    window identity and rectangle, a real PNG capture (digest + IHDR dimensions),
    the UI Automation tree, and the host scale factor of that window.

Closed error vocabulary (never a traceback, never a value from an input file):

    ARGS, ARGS_EXE, EXE_MISSING, OUT_EXISTS, LAUNCH_FAILED, PROBE_FAILED,
    PROBE_TIMEOUT, WINDOW_NOT_FOUND, RECORD_READ, RECORD_SCHEMA,
    SCREENSHOT_MISMATCH, PNG_INVALID, DPI_UNAVAILABLE, STOP_FAILED, WRITE_FAILED

Everything this process cannot establish (the display-scale ladder, visual quality and
the rest) is written as NOT_MEASURED with a reason and a human procedure; nothing here
declares MP-TYP1 complete.
"""

from __future__ import annotations

import argparse
import ctypes
import datetime
import hashlib
import json
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

OBSERVATION_VERSION = 1
WINDOW_TITLE = "COS Desktop"
MAX_RECORD_BYTES = 16 * 1024 * 1024
MAX_LOG_BYTES = 256 * 1024
MAX_ATTEMPTS = 40
ATTEMPT_DELAY_S = 0.4
PROBE_TIMEOUT_S = 130.0
STOP_TIMEOUT_S = 10.0
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")

# Anchors the wired shell exposes: Latin region labels and the fixture's Japanese task
# title, which the task-rail row exposes as "<display id> — <title>".
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
)

NOT_MEASURED = (
    {
        "item": "display_scale_ladder",
        "reason": "changing the display scale needs interactive Windows display control; "
        "the observation below is the host's current scale, not a ladder step",
        "human_procedure": "Settings > System > Display > Scale: 100/125/150/175/200 %, "
        "one run per scale: cargo run --locked -p cos-desktop -- --fixture",
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


def ihdr_dimensions(data: bytes) -> tuple[int, int]:
    """Width and height from the PNG IHDR chunk, or a closed PNG_INVALID."""
    if len(data) < 24 or data[:8] != PNG_SIGNATURE or data[12:16] != b"IHDR":
        fail("PNG_INVALID")
    width = int.from_bytes(data[16:20], "big")
    height = int.from_bytes(data[20:24], "big")
    if width <= 0 or height <= 0:
        fail("PNG_INVALID")
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


def scale_from_dpi(dpi: int) -> float:
    if type(dpi) is not int or dpi <= 0:
        fail("DPI_UNAVAILABLE")
    return dpi / 96.0


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


def validate_probe_record(record: dict[str, Any], pid: int, screenshot: Path) -> dict[str, Any]:
    window = record.get("window")
    shot = record.get("screenshot")
    uia = record.get("uia")
    if not isinstance(window, dict) or window.get("status") != "OBSERVED":
        fail("RECORD_SCHEMA")
    if not isinstance(shot, dict) or not isinstance(uia, dict):
        fail("RECORD_SCHEMA")
    if window.get("pid") != pid:
        fail("RECORD_SCHEMA")
    if not isinstance(window.get("title"), str) or not window["title"]:
        fail("RECORD_SCHEMA")
    if window.get("width", 0) <= 0 or window.get("height", 0) <= 0:
        fail("RECORD_SCHEMA")
    if shot.get("status") != "OBSERVED":
        fail("SCREENSHOT_MISMATCH")
    digest = shot.get("digest")
    if not isinstance(digest, str) or not SHA256.fullmatch(digest):
        fail("RECORD_SCHEMA")
    try:
        data = screenshot.read_bytes()
    except OSError:
        fail("SCREENSHOT_MISMATCH")
    width, height = ihdr_dimensions(data)
    if (width, height) != (shot.get("width"), shot.get("height")):
        fail("SCREENSHOT_MISMATCH")
    if sha256_bytes(data) != digest:
        fail("SCREENSHOT_MISMATCH")
    if uia.get("status") not in ("OBSERVED", "NOT_MEASURED"):
        fail("RECORD_SCHEMA")
    elements = uia.get("elements")
    if not isinstance(elements, list):
        fail("RECORD_SCHEMA")
    return {
        "window": {
            "title": window["title"],
            "pid": window["pid"],
            "handle": window["handle"],
            "left": window["left"],
            "top": window["top"],
            "width": window["width"],
            "height": window["height"],
        },
        "screenshot": {
            "status": "OBSERVED",
            "path": str(screenshot).replace("\\", "/"),
            "digest": digest,
            "digest_recomputed": sha256_bytes(data),
            "ihdr": {"width": width, "height": height},
            "bytes": len(data),
        },
        "uia": {
            "status": uia["status"],
            "reason": uia.get("reason", ""),
            "elements": elements,
            "truncated": uia.get("truncated", False),
            "depth_limited": uia.get("depth_limited", False),
            "elements_visited": uia.get("elements_visited", 0),
        },
    }


def observe_window(
    python: str, observer: Path, pid: int, probe_record: Path, screenshot: Path
) -> dict[str, Any]:
    """Run the packaged independent observer until the new window exists."""
    command = [
        python,
        "-B",
        str(observer),
        "--pid",
        str(pid),
        "--out",
        str(probe_record),
        "--screenshot",
        str(screenshot),
        "--max-elements",
        "400",
        "--max-depth",
        "12",
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
            return read_json(probe_record)
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


def build_record(
    exe: Path, exe_digest: str, observed: dict[str, Any], scale: dict[str, Any],
    analysis: dict[str, Any], stop_record: dict[str, Any]
) -> dict[str, Any]:
    uia = observed["uia"]
    mixed = analysis["mixed_script_count"] > 0
    ax_status = "OBSERVED" if uia["status"] == "OBSERVED" else "NOT_MEASURED"
    checks = [
        {
            "name": "window_observed",
            "status": "PASS",
            "detail": f"{observed['window']['title']} {observed['window']['width']}x"
            f"{observed['window']['height']} at ({observed['window']['left']},"
            f"{observed['window']['top']})",
        },
        {
            "name": "screenshot_is_a_real_capture",
            "status": "PASS",
            "detail": f"IHDR {observed['screenshot']['ihdr']['width']}x"
            f"{observed['screenshot']['ihdr']['height']}, "
            f"{observed['screenshot']['bytes']} bytes, digest recomputed equal",
        },
        {
            "name": "ax_tree_observed",
            "status": "PASS" if ax_status == "OBSERVED" else "NOT_MEASURED",
            "detail": f"{analysis['element_count']} elements, "
            f"{analysis['named_element_count']} named"
            if ax_status == "OBSERVED"
            else uia.get("reason", ""),
        },
        {
            "name": "latin_labels_present",
            "status": "PASS"
            if ax_status == "OBSERVED" and analysis["expected_latin_found"]
            else ("NOT_MEASURED" if ax_status != "OBSERVED" else "FAIL"),
            "detail": f"found {analysis['expected_latin_found']}, missing "
            f"{analysis['expected_latin_missing']}",
        },
        {
            "name": "japanese_label_present",
            "status": "PASS"
            if ax_status == "OBSERVED" and analysis["expected_japanese_found"]
            else ("NOT_MEASURED" if ax_status != "OBSERVED" else "FAIL"),
            "detail": f"found {analysis['expected_japanese_found']}, missing "
            f"{analysis['expected_japanese_missing']}",
        },
        {
            "name": "mixed_script_presence_in_ax_tree",
            "status": "PASS"
            if ax_status == "OBSERVED" and mixed
            else ("NOT_MEASURED" if ax_status != "OBSERVED" else "FAIL"),
            "detail": f"{analysis['mixed_script_count']} element names carry both scripts",
        },
    ]
    return {
        "typography_observation_version": OBSERVATION_VERSION,
        "observed_utc": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "executable": {"path": str(exe).replace("\\", "/"), "digest": exe_digest},
        "window": observed["window"],
        "screenshot": observed["screenshot"],
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
        "checks": checks,
        "app_stop": stop_record,
        "not_measured": list(NOT_MEASURED),
        "declares_mptyp1_complete": False,
        "statement": "Host-scale observation only; the display-scale ladder, mixed-script "
        "visual quality and every NOT_MEASURED item still need native runs. This does not "
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
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--observer", default=str(default_observer()))
    args = parser.parse_args(argv)
    process: subprocess.Popen | None = None
    try:
        exe = Path(args.exe)
        if not exe.is_file():
            fail("EXE_MISSING")
        observer = Path(args.observer)
        if not observer.is_file():
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
        log_path = out / "app.log"
        exe_digest = sha256_bytes(exe.read_bytes())
        process = launch(exe, log_path)
        document = observe_window(
            args.python, observer, process.pid, probe_record, screenshot
        )
        observed = validate_probe_record(document, process.pid, screenshot)
        scale = host_scale(observed["window"]["handle"])
        analysis = analyze_elements(observed["uia"]["elements"])
        stop_record = stop(process)
        process = None
        record = build_record(exe, exe_digest, observed, scale, analysis, stop_record)
        digest = write_record(record, out / "observation.json")
        print(
            json.dumps(
                {
                    "ok": True,
                    "record": str(out / "observation.json"),
                    "record_digest": digest,
                    "window": {
                        "title": record["window"]["title"],
                        "width": record["window"]["width"],
                        "height": record["window"]["height"],
                    },
                    "screenshot": record["screenshot"]["digest"],
                    "ihdr": record["screenshot"]["ihdr"],
                    "scale": record["scale"],
                    "ax_elements": record["uia"]["element_count"],
                    "mixed_script_count": record["labels"]["mixed_script_count"],
                    "found_latin": record["labels"]["found_latin"],
                    "found_japanese": record["labels"]["found_japanese"],
                },
                separators=(",", ":"),
            )
        )
        return 0
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


def default_observer() -> Path:
    return Path(__file__).resolve().parents[1] / "r0/native_observe.py"


if __name__ == "__main__":
    sys.exit(main())
