"""N06 frame/CPU attribution: what the observer can genuinely count, and what it cannot.

Written *before* any code, from the feature-gated instrumentation and the gpui pin
(`395def7a067b32ed598fcb3f048969ada9b54942`). The S2B journal exposes six record kinds
(`s2b_contracts.SUPPORTED`) and explicitly lacks eleven events (`MISSING`). The pin's
`FrameTiming`/`PresentTiming` are consumed inside the app and never emitted, so nothing
about presentation or scanout reaches an independent observer without a probe this
repository does not have.

FEASIBILITY TABLE (kind -> source -> status)

    gpui-dirty                  journal (observer receives it)  COUNTABLE
    gpui-cpu-draw               journal                         COUNTABLE
    gpui-platform-submission    journal                         COUNTABLE
    input-dispatch              journal                         COUNTABLE
    local-surface-mutation      journal                         COUNTABLE
    local-notify                journal                         COUNTABLE
    frame-request               app-side only                   NOT_MEASURED
    first-frame                 app-side only                   NOT_MEASURED
    present-call                app-side only                   NOT_MEASURED
    present-return              app-side only                   NOT_MEASURED
    present                     DWM/OS probe                    NOT_MEASURED
    displayed                   DWM/OS probe                    NOT_MEASURED
    scanout                     display-driver probe            NOT_MEASURED
    gpu-submission              GPU/D3D probe                   NOT_MEASURED
    input-to-photon             end-to-end OS probe             NOT_MEASURED
    app-main-entry              app-side only                   NOT_MEASURED
    useful-shell                app-side only                   NOT_MEASURED

What this module therefore reports:
* `journal_events`: bounded per-kind and per-window counts of the records the observer
  already receives. They are *journal events*, not displayed frames.
* `process_cpu`: CPU time of the observed process over the run window, from the OS
  (`GetProcessTimes`), labelled `attribution: "process_over_window"`. It is never divided
  by a frame count and never presented as per-frame attribution.
* Everything above marked NOT_MEASURED stays NOT_MEASURED with a reason: an app-reported
  number is not an independent endpoint, and a missing probe is not filled in.

The feature-off build is untouched: this is a reader of records that already exist, so no
Rust change is required and none was made.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

import contracts
import s2b_contracts as s2b

ATTRIBUTION_VERSION = 1
MAX_CAPTURE_BYTES = 1024 * 1024
MAX_RECORDS = 4096
MAX_WINDOWS = 16
MAX_CPU_SECONDS = 600
CPU_PROBE_TIMEOUT_MS = 30_000

# Closed status vocabulary for every reported fact. The wire carries numeric kind codes
# (`s2b_contracts.SUPPORTED`); the names here are the same six kinds.
COUNTABLE = ("gpui-dirty", "gpui-cpu-draw", "gpui-platform-submission",
             "input-dispatch", "local-surface-mutation", "local-notify")
KIND_NAMES = {code: name for name, code in s2b.SUPPORTED.items()}
NOT_MEASURED_REASONS = {
    "frame-request": "APP_SIDE_ONLY",
    "first-frame": "APP_SIDE_ONLY",
    "present-call": "APP_SIDE_ONLY",
    "present-return": "APP_SIDE_ONLY",
    "present": "DWM_PROBE_UNAVAILABLE",
    "displayed": "DWM_PROBE_UNAVAILABLE",
    "scanout": "DISPLAY_DRIVER_PROBE_UNAVAILABLE",
    "gpu-submission": "GPU_PROBE_UNAVAILABLE",
    "input-to-photon": "END_TO_END_PROBE_UNAVAILABLE",
    "app-main-entry": "APP_SIDE_ONLY",
    "useful-shell": "APP_SIDE_ONLY",
}

ERROR_CODES = (
    "ARGS_TARGET",
    "ARGS_CAP",
    "READ_FAILED",
    "SCHEMA",
    "CAPTURE_TOO_LARGE",
    "CAPTURE_RECORD_LIMIT",
    "CPU_PROBE_MISSING",
    "CPU_PROBE_FAILED",
    "CPU_PROBE_TIMEOUT",
    "CPU_PROBE_JSON",
    "PROCESS_NOT_FOUND",
)

# The values are substituted into the script text because 
# makes PowerShell look for a command called ; both substitutions are integers.
CPU_SCRIPT = r"""
$process = Get-Process -Id __PID__ -ErrorAction Stop
$start = $process.TotalProcessorTime.TotalMilliseconds
$clock = [System.Diagnostics.Stopwatch]::StartNew()
Start-Sleep -Seconds __SECONDS__
$clock.Stop()
$process.Refresh()
$end = $process.TotalProcessorTime.TotalMilliseconds
@{ cpu_ms = [math]::Round($end - $start, 3); window_ms = [math]::Round($clock.Elapsed.TotalMilliseconds, 3) } |
    ConvertTo-Json -Compress
"""


class AttributionError(Exception):
    """One closed code; never input text, a path or a traceback."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def fail(code: str) -> None:
    raise AttributionError(code if code in ERROR_CODES else "SCHEMA")


# --------------------------------------------------------------------------- table


def feasibility() -> dict[str, Any]:
    """The table above, as data so a test asserts it instead of trusting prose."""
    kinds = {}
    for kind in COUNTABLE:
        kinds[kind] = {"status": "COUNTABLE", "source": "journal",
                       "reason": ""}
    for kind, reason in NOT_MEASURED_REASONS.items():
        kinds[kind] = {"status": "NOT_MEASURED", "source": "unavailable", "reason": reason}
    return {
        "attribution_version": ATTRIBUTION_VERSION,
        "gpui_pin": s2b.GPUI_PIN,
        "kinds": kinds,
        "frames_displayed": {"status": "NOT_MEASURED", "reason": "DWM_PROBE_UNAVAILABLE"},
        "scanout": {"status": "NOT_MEASURED", "reason": "DISPLAY_DRIVER_PROBE_UNAVAILABLE"},
        "per_frame_cpu": {"status": "NOT_MEASURED", "reason": "NO_FRAME_ATTRIBUTION"},
        "app_reported_stand_in": False,
    }


# --------------------------------------------------------------------------- frames


def load_capture(path: str) -> dict[str, Any]:
    try:
        size = Path(path).stat().st_size
    except OSError:
        fail("READ_FAILED")
    if size > MAX_CAPTURE_BYTES:
        fail("CAPTURE_TOO_LARGE")
    try:
        document = contracts.load_document(path, MAX_CAPTURE_BYTES)
    except contracts.ContractError:
        fail("SCHEMA")
    if not isinstance(document, dict) or "records" not in document:
        fail("SCHEMA")
    return document


def count_journal(document: dict[str, Any]) -> dict[str, Any]:
    """Bounded per-kind and per-window counts of the records the observer receives."""
    records = document["records"]
    if not isinstance(records, list):
        fail("SCHEMA")
    if len(records) > MAX_RECORDS:
        fail("CAPTURE_RECORD_LIMIT")
    by_kind = {kind: 0 for kind in COUNTABLE}
    by_window: dict[int, int] = {}
    unknown = 0
    for record in records:
        if not isinstance(record, dict) or "kind" not in record:
            fail("SCHEMA")
        raw = record["kind"]
        # A capture stores the numeric code; accept the name too so a hand-written
        # document and the wire record mean the same thing.
        kind = KIND_NAMES.get(raw) if type(raw) is int else raw
        if kind in by_kind:
            by_kind[kind] += 1
            window = record.get("window")
            if type(window) is int:
                if window not in by_window and len(by_window) >= MAX_WINDOWS:
                    fail("CAPTURE_RECORD_LIMIT")
                by_window[window] = by_window.get(window, 0) + 1
        else:
            unknown += 1
    counted = sum(by_kind.values())
    return {
        "status": "OBSERVED" if counted else "NOT_MEASURED",
        "reason": "" if counted else "NO_JOURNAL_RECORDS",
        # These are journal events, never displayed frames: the label is the point.
        "unit": "journal_events",
        "total": counted,
        "unclassified": unknown,
        "by_kind": by_kind,
        "by_window": {str(window): count for window, count in sorted(by_window.items())},
        "frames_displayed": {"status": "NOT_MEASURED", "reason": "DWM_PROBE_UNAVAILABLE"},
    }


# --------------------------------------------------------------------------- cpu


def sample_process_cpu(pid: int, seconds: int, powershell: str = "powershell") -> dict[str, Any]:
    """Process CPU time over a wall-clock window, labelled as such and nothing more."""
    if not 1 <= seconds <= MAX_CPU_SECONDS:
        fail("ARGS_CAP")
    script = CPU_SCRIPT.replace("__PID__", str(pid)).replace("__SECONDS__", str(seconds))
    command = [powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
               "-Command", script]
    try:
        done = subprocess.run(command, capture_output=True, check=False,
                              timeout=CPU_PROBE_TIMEOUT_MS / 1000 + seconds)
    except FileNotFoundError:
        fail("CPU_PROBE_MISSING")
    except subprocess.TimeoutExpired:
        fail("CPU_PROBE_TIMEOUT")
    except OSError:
        fail("CPU_PROBE_FAILED")
    if done.returncode != 0:
        # The message is localised, so the exit code alone decides: a probe that could not
        # read the process is a closed CPU_PROBE_FAILED, never a traceback.
        fail("CPU_PROBE_FAILED")
    try:
        payload = contracts.loads_document(done.stdout.decode("utf-8").strip())
    except (UnicodeDecodeError, contracts.ContractError):
        fail("CPU_PROBE_JSON")
    if not isinstance(payload, dict) or set(payload) != {"cpu_ms", "window_ms"}:
        fail("CPU_PROBE_JSON")
    cpu_ms = payload["cpu_ms"]
    window_ms = payload["window_ms"]
    if not isinstance(cpu_ms, (int, float)) or not isinstance(window_ms, (int, float)):
        fail("CPU_PROBE_JSON")
    return {
        "status": "OBSERVED",
        "attribution": "process_over_window",
        "cpu_ms": cpu_ms,
        "window_ms": window_ms,
        "pid": pid,
        "per_frame_cpu": {"status": "NOT_MEASURED", "reason": "NO_FRAME_ATTRIBUTION"},
        "note": "process CPU time over the window; not per-frame attribution",
    }


# --------------------------------------------------------------------------- cli


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    commands.add_parser("feasibility", help="what can and cannot be counted")
    frames_parser = commands.add_parser("frames", help="journal event counts of a capture")
    frames_parser.add_argument("--capture", required=True)
    cpu_parser = commands.add_parser("cpu", help="process CPU time over a window")
    cpu_parser.add_argument("--pid", type=int, required=True)
    cpu_parser.add_argument("--seconds", default="5")
    cpu_parser.add_argument("--powershell", default="powershell")
    args = parser.parse_args(argv)
    try:
        if args.operation == "feasibility":
            result = {"ok": True, **feasibility()}
        elif args.operation == "frames":
            result = {"ok": True, "journal_events": count_journal(load_capture(args.capture))}
        else:
            try:
                seconds = int(args.seconds, 10)
            except ValueError:
                fail("ARGS_CAP")
            result = {"ok": True, "process_cpu": sample_process_cpu(
                args.pid, seconds, args.powershell)}
        print(json.dumps(result, separators=(",", ":")))
        return 0
    except AttributionError as error:
        print(json.dumps({"ok": False, "error": error.code}, separators=(",", ":")))
    except contracts.ContractError as error:
        print(json.dumps({"ok": False, "error": error.errors[0]["code"]},
                         separators=(",", ":")))
    except (OSError, ValueError, RuntimeError, TypeError):
        print('{"ok":false,"error":"SCHEMA"}')
    return 1


if __name__ == "__main__":
    sys.exit(main())
