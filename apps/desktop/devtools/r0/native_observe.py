"""N03 independent observation: window identity, screenshot and a UIA tree snapshot.

Python standard library only. The OS capabilities Python does not expose (Win32 window
enumeration, UI Automation, screen capture) are obtained by running `n03_probe.ps1`
through Windows PowerShell; this module never reads application-private state, only the
window the OS exposes.

The probe returns one bounded JSON document and every failure here is a closed code:

    ARGS_TARGET, ARGS_CAP, PROBE_MISSING, PROBE_FAILED, PROBE_TIMEOUT, PROBE_JSON,
    SCHEMA, SCREENSHOT_MISSING, SCREENSHOT_MISMATCH, RECORD_TOO_LARGE, WRITE_FAILED

The probe reports its own closed codes, which are passed through unchanged:
    TARGET_REQUIRED, CAP_RANGE, SCREENSHOT_PATH_REQUIRED, WIN32_UNAVAILABLE,
    WINDOW_NOT_FOUND, WINDOW_RECT_UNAVAILABLE

RECORD SCHEMA (observation_version 1):

    observation_version  1
    observed_utc         ISO-8601 UTC seconds
    window               {status, title, pid, handle, left, top, width, height}
    screenshot           {status, path?, digest?, width?, height?, bytes?, reason?}
    uia                  {status, reason, digest?, elements[], focused, truncated,
                          depth_limited, elements_visited}
    caps                 {max_elements, max_depth, max_screenshot_bytes}
    ok                   true when a window was observed (screenshot/UIA may be
                         NOT_MEASURED with a reason)

Every element is `{depth, name, control_type, automation_id, focused, left, top, width,
height}`. `NOT_MEASURED` is explicit: a refused screenshot or UIA tree carries a `reason`
instead of zeros, and the record is still written so the absence is evidence.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import re
import struct
import subprocess
import sys
from pathlib import Path
from typing import Any

import contracts

OBSERVATION_VERSION = 1
PROBE = Path(__file__).resolve().parent / "n03_probe.ps1"
MAX_PROBE_BYTES = 8 * 1024 * 1024
MAX_RECORD_BYTES = 16 * 1024 * 1024
MAX_ELEMENTS_RANGE = (1, 5000)
MAX_DEPTH_RANGE = (1, 64)
MAX_SCREENSHOT_RANGE = (1024, 64 * 1024 * 1024)
PROBE_TIMEOUT_MS = 120_000
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")

ERROR_CODES = (
    "ARGS_TARGET",
    "ARGS_CAP",
    "PROBE_MISSING",
    "PROBE_FAILED",
    "PROBE_TIMEOUT",
    "PROBE_JSON",
    "SCHEMA",
    "SCREENSHOT_MISSING",
    "SCREENSHOT_MISMATCH",
    "RECORD_TOO_LARGE",
    "WRITE_FAILED",
    # Reported by n03_probe.ps1; kept in the same closed vocabulary.
    "TARGET_REQUIRED",
    "CAP_RANGE",
    "SCREENSHOT_PATH_REQUIRED",
    "WIN32_UNAVAILABLE",
    "WINDOW_NOT_FOUND",
    "WINDOW_RECT_UNAVAILABLE",
)

WINDOW_FIELDS = {"status", "title", "pid", "handle", "left", "top", "width", "height"}
ELEMENT_FIELDS = {"depth", "name", "control_type", "automation_id", "focused",
                  "left", "top", "width", "height"}
UIA_FIELDS = {"status", "reason", "elements", "focused", "truncated", "depth_limited",
              "elements_visited"}
CAPS_FIELDS = {"max_elements", "max_depth", "max_screenshot_bytes"}
PROBE_FIELDS = {"ok", "window", "screenshot", "uia", "caps"}


class ObservationError(Exception):
    """One closed code; never input text, a path or a traceback."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def fail(code: str) -> None:
    assert code in ERROR_CODES
    raise ObservationError(code)


# --------------------------------------------------------------------------- arguments


def bounded_integer(value: str, bounds: tuple[int, int]) -> int:
    try:
        parsed = int(value, 10)
    except (TypeError, ValueError):
        fail("ARGS_CAP")
    if not bounds[0] <= parsed <= bounds[1]:
        fail("ARGS_CAP")
    return parsed


def validate_target(title: str | None, pid: int | None) -> None:
    if bool(title) == bool(pid):
        # Exactly one target selector, so a run can never observe two different windows.
        fail("ARGS_TARGET")


# --------------------------------------------------------------------------- probe


def probe_command(powershell: str, *, title: str | None, pid: int | None,
                  screenshot: Path, max_elements: int, max_depth: int,
                  max_screenshot_bytes: int) -> list[str]:
    command = [powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
               "-File", str(PROBE),
               "-Screenshot", str(screenshot),
               "-MaxElements", str(max_elements),
               "-MaxDepth", str(max_depth),
               "-MaxScreenshotBytes", str(max_screenshot_bytes)]
    if title is not None:
        command.extend(["-Title", title])
    else:
        command.extend(["-ProcessId", str(pid)])
    return command


def run_probe(command: list[str]) -> dict[str, Any]:
    try:
        done = subprocess.run(command, capture_output=True, check=False,
                              timeout=PROBE_TIMEOUT_MS / 1000)
    except FileNotFoundError:
        fail("PROBE_MISSING")
    except subprocess.TimeoutExpired:
        fail("PROBE_TIMEOUT")
    except OSError:
        fail("PROBE_FAILED")
    if len(done.stdout) > MAX_PROBE_BYTES:
        fail("PROBE_FAILED")
    try:
        text = done.stdout.decode("utf-8").strip()
    except UnicodeDecodeError:
        fail("PROBE_JSON")
    if not text:
        fail("PROBE_FAILED")
    try:
        document = contracts.loads_document(text)
    except contracts.ContractError:
        fail("PROBE_JSON")
    if not isinstance(document, dict):
        fail("SCHEMA")
    if document.get("ok") is not True:
        code = document.get("error")
        # The probe's own codes are closed too; anything else is a probe failure.
        fail(code if isinstance(code, str) and re.fullmatch(r"[A-Z_]+", code)
             else "PROBE_FAILED")
    return document


# --------------------------------------------------------------------------- schema


def _object(value: Any, fields: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != fields:
        fail("SCHEMA")
    return value


def _integer(value: Any) -> int:
    if type(value) is not int:
        fail("SCHEMA")
    return value


def validate_window(value: Any) -> dict[str, Any]:
    window = _object(value, WINDOW_FIELDS)
    if window["status"] != "OBSERVED":
        fail("SCHEMA")
    if not isinstance(window["title"], str) or not window["title"]:
        fail("SCHEMA")
    for key in ("pid", "handle", "left", "top", "width", "height"):
        _integer(window[key])
    if window["width"] <= 0 or window["height"] <= 0:
        fail("SCHEMA")
    return window


def validate_uia(value: Any) -> dict[str, Any]:
    uia = _object(value, UIA_FIELDS)
    if uia["status"] not in ("OBSERVED", "NOT_MEASURED"):
        fail("SCHEMA")
    if uia["status"] == "NOT_MEASURED" and not isinstance(uia["reason"], str):
        fail("SCHEMA")
    if not isinstance(uia["elements"], list):
        fail("SCHEMA")
    for element in uia["elements"]:
        item = _object(element, ELEMENT_FIELDS)
        for key in ("depth", "left", "top", "width", "height"):
            _integer(item[key])
        if not isinstance(item["name"], str) or not isinstance(item["control_type"], str):
            fail("SCHEMA")
        if not isinstance(item["automation_id"], str) or type(item["focused"]) is not bool:
            fail("SCHEMA")
    if uia["focused"] is not None:
        focused = _object(uia["focused"], {"name", "control_type", "automation_id"})
        for key in focused:
            if not isinstance(focused[key], str):
                fail("SCHEMA")
    if type(uia["truncated"]) is not bool or type(uia["depth_limited"]) is not bool:
        fail("SCHEMA")
    _integer(uia["elements_visited"])
    return uia


def png_size(path: Path) -> tuple[int, int]:
    """Width and height straight from the IHDR chunk, so the digest covers real bytes."""
    try:
        with open(path, "rb") as handle:
            header = handle.read(33)
    except OSError:
        fail("SCREENSHOT_MISSING")
    if len(header) < 33 or header[:8] != PNG_SIGNATURE or header[12:16] != b"IHDR":
        fail("SCREENSHOT_MISMATCH")
    width, height = struct.unpack(">II", header[16:24])
    return width, height


def screenshot_record(document: dict[str, Any], path: Path) -> dict[str, Any]:
    reported = document["screenshot"]
    if not isinstance(reported, dict) or reported.get("status") not in (
            "OBSERVED", "NOT_MEASURED"):
        fail("SCHEMA")
    if reported["status"] != "OBSERVED":
        reason = reported.get("reason")
        return {"status": "NOT_MEASURED",
                "reason": reason if isinstance(reason, str) else "SCREENSHOT_FAILED"}
    if not path.is_file():
        fail("SCREENSHOT_MISSING")
    width, height = png_size(path)
    if (width, height) != (_integer(reported.get("width")), _integer(reported.get("height"))):
        fail("SCREENSHOT_MISMATCH")
    data = path.read_bytes()
    return {
        "status": "OBSERVED",
        "path": str(path),
        "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
        "width": width,
        "height": height,
        "bytes": len(data),
    }


def canonical_digest(value: Any) -> str:
    body = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return "sha256:" + hashlib.sha256(body.encode()).hexdigest()


def build_record(document: dict[str, Any], screenshot: Path,
                 caps: dict[str, int]) -> dict[str, Any]:
    _object(document, PROBE_FIELDS)
    window = validate_window(document["window"])
    uia = validate_uia(document["uia"])
    reported_caps = _object(document["caps"], CAPS_FIELDS)
    for key, value in caps.items():
        if _integer(reported_caps[key]) != value:
            fail("SCHEMA")
    if len(uia["elements"]) > caps["max_elements"]:
        # The probe is bounded by the same cap; a larger list is a schema violation.
        fail("SCHEMA")
    uia_record = {
        "status": uia["status"],
        "reason": uia["reason"],
        "digest": canonical_digest(uia["elements"]) if uia["status"] == "OBSERVED" else None,
        "elements": uia["elements"],
        "focused": uia["focused"],
        "truncated": uia["truncated"],
        "depth_limited": uia["depth_limited"],
        "elements_visited": uia["elements_visited"],
    }
    return {
        "observation_version": OBSERVATION_VERSION,
        "observed_utc": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "window": window,
        "screenshot": screenshot_record(document, screenshot),
        "uia": uia_record,
        "caps": caps,
        "ok": True,
    }


def write_record(record: dict[str, Any], path: Path) -> None:
    body = json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
    if len(body) > MAX_RECORD_BYTES:
        fail("RECORD_TOO_LARGE")
    try:
        path.write_bytes(body)
    except OSError:
        fail("WRITE_FAILED")


# --------------------------------------------------------------------------- cli


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    # Both selectors are parsed separately and rejected by validate_target, so the
    # failure is the closed ARGS_TARGET code rather than an argparse usage error.
    parser.add_argument("--title", help="exact window title (a substring match is a fallback)")
    parser.add_argument("--pid", type=int, help="owning process id of the window")
    parser.add_argument("--out", required=True, help="record path (JSON)")
    parser.add_argument("--screenshot", required=True, help="screenshot path (PNG)")
    parser.add_argument("--max-elements", default="400")
    parser.add_argument("--max-depth", default="12")
    parser.add_argument("--max-screenshot-bytes", default=str(32 * 1024 * 1024))
    parser.add_argument("--powershell", default="powershell")
    args = parser.parse_args(argv)
    try:
        validate_target(args.title, args.pid)
        max_elements = bounded_integer(args.max_elements, MAX_ELEMENTS_RANGE)
        max_depth = bounded_integer(args.max_depth, MAX_DEPTH_RANGE)
        max_screenshot = bounded_integer(args.max_screenshot_bytes, MAX_SCREENSHOT_RANGE)
        screenshot = Path(args.screenshot)
        caps = {"max_elements": max_elements, "max_depth": max_depth,
                "max_screenshot_bytes": max_screenshot}
        command = probe_command(args.powershell, title=args.title, pid=args.pid,
                                screenshot=screenshot, max_elements=max_elements,
                                max_depth=max_depth,
                                max_screenshot_bytes=max_screenshot)
        document = run_probe(command)
        record = build_record(document, screenshot, caps)
        write_record(record, Path(args.out))
        print(json.dumps({
            "ok": True,
            "record": args.out,
            "window": {"title": record["window"]["title"], "pid": record["window"]["pid"]},
            "screenshot": record["screenshot"].get("digest"),
            "uia": {"status": record["uia"]["status"],
                    "elements": len(record["uia"]["elements"]),
                    "truncated": record["uia"]["truncated"]},
        }, separators=(",", ":")))
        return 0
    except ObservationError as error:
        print(json.dumps({"ok": False, "error": error.code}, separators=(",", ":")))
    except contracts.ContractError as error:
        print(json.dumps({"ok": False, "error": error.errors[0]["code"]},
                         separators=(",", ":")))
    except (OSError, ValueError, RuntimeError, TypeError):
        print('{"ok":false,"error":"PROBE_FAILED"}')
    return 1


if __name__ == "__main__":
    sys.exit(main())
