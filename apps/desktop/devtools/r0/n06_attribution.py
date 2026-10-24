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

INDEPENDENT PROBE (the gap this file used to report as NOT_MEASURED)

`n06_probe.ps1` supplies the missing OS-level endpoints for one declared wall-clock window,
and each one degrades on its own to NOT_MEASURED with a closed reason code:

* `window_content_changes` - `PrintWindow(PW_RENDERFULLCONTENT)` asks the owning window to
  render its content into a device context at a requested interval; the reported count is how
  many times consecutive sampled captures differed. `PrintWindow` is not a compositor or
  presentation counter, so these image differences have an unknown relationship to presented
  frames; they are not a frame count, not scanout, not a displayed-frame signal and not an
  app-reported number. The achieved cadence is recorded because the sampler itself costs time.
* `compositor_desktop_wide` - `DwmGetCompositionTimingInfo` deltas. These counters cover the
  whole desktop compositor, not one app; they can never be reported `per_app`.
* `process_cpu` - `GetProcessTimes` for the target pid over the same window
  (`attribution: "process_over_window"`), never per-frame.
* `gpu_engine_utilization` - PDH `GPU Engine(*)/Utilization Percentage` for the engine
  instances attributed to the target pid, sampled once per second and summed over engines
  (so it can exceed 100); NOT_MEASURED when PDH or the pid's instances are unavailable.
* `app_displayed_frames` stays NOT_MEASURED (`NO_PER_WINDOW_DISPLAY_COUNTER`): this tool has
  no independent presentation counter, so N06's independent-frame-count criterion is
  NOT_MEASURED until such a basis exists. No field in a record is a presented/dispatched
  frame count.
* `per_frame_cpu` is only ever a *derived ratio* over the declared window
  (cpu_ms / observed sampled content changes). The denominator is sampled image
  differences, not frames, so the ratio is per observed content change and never per frame;
  it is not a per-frame measurement. It is NOT_MEASURED when there is no CPU value
  (`CPU_NOT_MEASURED`), no observed content-change endpoint (`FRAMES_NOT_MEASURED`), no
  sampled change (`NO_FRAME_CHANGES`), or a CPU delta at or below the scheduler clock
  resolution (`CPU_BELOW_CLOCK_RESOLUTION`): a zero numerator is a resolution floor, never a
  measured zero ratio.

The probe stdout is accepted under a 1 MiB READ cap (the consumer reads at most 1 MiB + 1
byte and refuses anything larger). That cap bounds what this tool reads; it is not an
enforced write quota on the probe process.

The feature-off build is untouched: this is a reader of OS observations, so no Rust change
is required and none was made.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
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

# --------------------------------------------------------------------------- probe caps
#
# Every value below is a bound, not a measurement: the probe refuses anything outside the
# range instead of clamping it silently.
PROBE = Path(__file__).resolve().parent / "n06_probe.ps1"
PROBE_VERSION = 1
PROBE_RECORD_VERSION = 2
MAX_PROBE_BYTES = 1024 * 1024
MAX_RECORD_BYTES = 4 * 1024 * 1024
SECONDS_RANGE = (1, 120)
INTERVAL_RANGE = (10, 1000)
MAX_SAMPLES_RANGE = (10, 10_000)
MAX_WINDOW_PIXELS_RANGE = (10_000, 16_000_000)
PROBE_TIMEOUT_SLACK_S = 30
WINDOW_MS_CEILING_SLACK_S = 60
PROBE_FRAME_METHOD = "PrintWindow(PW_RENDERFULLCONTENT)"
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
EXECUTABLE_IDENTITY_REASONS = {"EXECUTABLE_IDENTITY_NOT_SUPPLIED"}
# Closed reasons for a NOT_MEASURED derived ratio. `CPU_BELOW_CLOCK_RESOLUTION` is not a
# zero measurement: GetProcessTimes quantises, so a zero delta means "below one tick".
PER_FRAME_NOT_MEASURED_REASONS = {
    "CPU_NOT_MEASURED",
    "FRAMES_NOT_MEASURED",
    "NO_FRAME_CHANGES",
    "CPU_BELOW_CLOCK_RESOLUTION",
}

# Closed vocabulary for a NOT_MEASURED endpoint reported by the probe. An unregistered
# word is a probe-schema violation, never a new kind of value.
NOT_MEASURED_PATHS = {
    "WINDOW_NOT_FOUND",
    "WINDOW_MINIMIZED",
    "WINDOW_TOO_LARGE",
    "WINDOW_CAPTURE_FAILED",
    "WINDOW_CAPTURE_UNIFORM",
    "NO_SAMPLES",
    "PROCESS_NOT_READABLE",
    "PROCESS_ENDED_DURING_WINDOW",
    "DWM_TIMING_UNAVAILABLE",
    "DWM_COUNTER_REGRESSED",
    "GPU_COUNTER_UNAVAILABLE",
    "GPU_NO_SAMPLES",
    "GPU_NO_PID_INSTANCES",
}
APP_DISPLAYED_REASON = "NO_PER_WINDOW_DISPLAY_COUNTER"
# Probe-level `ok: false` codes; each maps to one of this module's closed error codes.
PROBE_ERROR_CODES = ("CAP_RANGE", "PROCESS_NOT_FOUND", "PROBE_UNAVAILABLE", "PROBE_FAILED")


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
    "ARGS_PID",
    "READ_FAILED",
    "SCHEMA",
    "CAPTURE_TOO_LARGE",
    "CAPTURE_SCHEMA",
    "CAPTURE_RECORD_LIMIT",
    "CPU_PROBE_MISSING",
    "CPU_PROBE_FAILED",
    "CPU_PROBE_TIMEOUT",
    "CPU_PROBE_JSON",
    "PROCESS_NOT_FOUND",
    # Independent probe (n06_probe.ps1) and its record/compare steps.
    "PROBE_MISSING",
    "PROBE_UNAVAILABLE",
    "PROBE_FAILED",
    "PROBE_TIMEOUT",
    "PROBE_JSON",
    "PROBE_EXIT",
    "PROBE_OVERSIZE",
    "PROBE_SCHEMA",
    "ARGS_IDENTITY",
    "RECORD_TOO_LARGE",
    "WRITE_FAILED",
    "COMPARE_INPUT",
)

# `-Command` receives one script string, so the pid and the sleep duration are substituted
# into the text rather than passed as arguments: PowerShell would otherwise parse an
# argument as a command name. Both substituted values are integers, never caller text.
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
        # The independent probe is a separate endpoint set, not a relabel of the journal
        # table above. `frames_displayed` of this app still has no counter. Statuses here
        # describe capability, never a measured value.
        "independent_probe": {
            "probe": PROBE.name,
            "probe_version": PROBE_VERSION,
            "capability_note": "capability, not a measurement: PROBE AVAILABLE means a probe "
                               "exists; every run still reports OBSERVED or NOT_MEASURED per "
                               "endpoint",
            "endpoints": {
                "window_content_changes": {
                    "status": "AVAILABLE",
                    "unit": "sampled_window_content_changes",
                    "reason": "",
                    "note": "sampled image differences with an unknown relationship to "
                            "presented frames; not a frame count",
                },
                "compositor_desktop_wide": {
                    "status": "AVAILABLE",
                    "unit": "compositor_frames_whole_desktop",
                    "per_app": False,
                    "reason": "",
                    "note": "DWM counters cover the whole desktop compositor, not one app",
                },
                "process_cpu": {
                    "status": "AVAILABLE",
                    "unit": "process_ms_over_window",
                    "attribution": "process_over_window",
                    "reason": "",
                },
                "gpu_engine_utilization": {
                    "status": "AVAILABLE",
                    "unit": "pdh_gpu_engine_percent_summed_over_engines",
                    "reason": "",
                },
                "app_displayed_frames": {
                    "status": "NOT_MEASURED",
                    "reason": APP_DISPLAYED_REASON,
                },
                "frame_criterion": {
                    "status": "NOT_MEASURED",
                    "reason": "NO_INDEPENDENT_PRESENTATION_COUNTER",
                    "note": "N06's independent frame count stays NOT_MEASURED until an "
                            "independent presentation counter exists; sampled image differences "
                            "are not presented frames",
                },
                "per_frame_cpu": {
                    "status": "DERIVED_RATIO_IF_CHANGES",
                    "reason": "",
                    "note": "derived per observed sampled content change; never per frame",
                },
            },
            "app_reported_stand_in": False,
        },
    }


# --------------------------------------------------------------------------- frames


def load_capture(path: str) -> dict[str, Any]:
    """Bounded load plus the existing S2B contract; nothing is counted from raw JSON.

    `s2b_contracts.validate_capture` is the only capture validator, so a document that is
    merely shaped like a capture (say `{"records": [{"kind": 1}]}`) is refused instead of
    being elevated into an observation.
    """
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
    if not isinstance(document, dict):
        fail("SCHEMA")
    try:
        s2b.validate_capture(document)
    except s2b.ObservationError:
        fail("CAPTURE_SCHEMA")
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
    # A contract capture binds the executable and the gpui pin, but it carries no source
    # identity (no commit, no worktree digest), so these counts are observation-level and
    # are never claimed as source-bound evidence.
    provenance = {
        "status": "NOT_MEASURED",
        "reason": "NO_SOURCE_IDENTITY_IN_CAPTURE",
        "source_bound": False,
        "executable_sha256": document.get("executable_sha256"),
        "gpui_pin": document.get("gpui_pin"),
        "execution_id": document.get("execution_id"),
    }
    return {
        "provenance": provenance,
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
    """Process CPU time over a wall-clock window, labelled as such and nothing more.

    The spawned interpreter is resolved to an absolute executable image first: a bare name
    would let the OS search the application directory or a PATH shim (for example a
    `powershell.bat`) before the intended interpreter.
    """
    if not 1 <= seconds <= MAX_CPU_SECONDS:
        fail("ARGS_CAP")
    try:
        resolved = _resolve_powershell(powershell)
    except AttributionError as error:
        fail("CPU_PROBE_MISSING" if error.code == "PROBE_MISSING" else error.code)
    script = CPU_SCRIPT.replace("__PID__", str(pid)).replace("__SECONDS__", str(seconds))
    # `resolved` is an absolute path to a regular file, never a bare command name.
    command = [resolved, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
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


# ------------------------------------------------------------------ probe plumbing


def _bounded_int(value: Any, bounds: tuple[int, int], code: str = "ARGS_CAP") -> int:
    if type(value) is not int or not bounds[0] <= value <= bounds[1]:
        fail(code)
    return value


def _resolve_powershell(powershell: str) -> str:
    """Absolute path to the spawned shell; a bare name is resolved here, never by the OS.

    On Windows a bare name is completed only with executable-image extensions. A PATH shim
    such as `powershell.bat` is not a spawnable image without a shell and is exactly the
    application-directory/PATH-search surface this resolves away; it is never selected.
    """
    if not isinstance(powershell, str) or not powershell:
        fail("PROBE_MISSING")
    candidate = Path(powershell)
    if candidate.is_absolute():
        if not candidate.is_file():
            fail("PROBE_MISSING")
        return str(candidate.resolve())
    if os.name == "nt" and not candidate.suffix:
        found = shutil.which(powershell + ".exe") or shutil.which(powershell + ".com")
    else:
        found = shutil.which(powershell)
    if not found:
        fail("PROBE_MISSING")
    resolved = Path(found)
    if os.name == "nt" and resolved.suffix.lower() not in (".exe", ".com"):
        # Defense in depth: never return a script shim as the interpreter.
        fail("PROBE_MISSING")
    return str(resolved.resolve())


def probe_command(powershell: str, probe: Path, *, pid: int, seconds: int, interval_ms: int,
                  max_samples: int, max_window_pixels: int) -> list[str]:
    return [powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
            "-File", str(Path(probe).resolve()),
            "-ProcessId", str(pid),
            "-Seconds", str(seconds),
            "-IntervalMs", str(interval_ms),
            "-MaxSamples", str(max_samples),
            "-MaxWindowPixels", str(max_window_pixels)]


def run_probe(command: list[str], timeout_s: float) -> dict[str, Any]:
    """Bounded run: the output is written to a temporary file and read back under a cap, so
    a probe that prints without bound cannot be buffered whole in memory."""
    with tempfile.TemporaryFile() as sink:
        try:
            process = subprocess.Popen(command, stdout=sink, stderr=subprocess.DEVNULL)
        except FileNotFoundError:
            fail("PROBE_MISSING")
        except OSError:
            fail("PROBE_FAILED")
        try:
            process.wait(timeout=timeout_s)
        except subprocess.TimeoutExpired:
            process.kill()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass
            fail("PROBE_TIMEOUT")
        sink.seek(0)
        data = sink.read(MAX_PROBE_BYTES + 1)
    if len(data) > MAX_PROBE_BYTES:
        if process.poll() is None:
            process.kill()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass
        fail("PROBE_OVERSIZE")
    if process.returncode != 0:
        # Valid JSON followed by a non-zero exit is not evidence: the exit code is part of
        # the probe contract, exactly like `ok: true`.
        fail("PROBE_EXIT")
    try:
        text = data.decode("utf-8").strip()
    except UnicodeDecodeError:
        fail("PROBE_JSON")
    if not text:
        fail("PROBE_FAILED")
    try:
        document = contracts.loads_document(text)
    except contracts.ContractError:
        fail("PROBE_JSON")
    if not isinstance(document, dict):
        fail("PROBE_SCHEMA")
    if document.get("ok") is not True:
        code = document.get("error")
        mapping = {"CAP_RANGE": "ARGS_CAP", "PROCESS_NOT_FOUND": "PROCESS_NOT_FOUND",
                   "PROBE_UNAVAILABLE": "PROBE_UNAVAILABLE", "PROBE_FAILED": "PROBE_FAILED"}
        fail(mapping.get(code, "PROBE_FAILED") if isinstance(code, str) else "PROBE_FAILED")
    return document


# ------------------------------------------------------------------ probe schema
#
# The probe document is accepted only in exactly this shape. A missing key, an extra key
# (for example an app-supplied `frames_displayed`) or a value outside its bound is a
# PROBE_SCHEMA failure: nothing is coerced into a measurement.


def _object(value: Any, fields: tuple[str, ...]) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != set(fields):
        fail("PROBE_SCHEMA")
    return value


def _integer(value: Any, *, minimum: int = 0,
             maximum: int | None = None) -> int:
    if type(value) is not int or value < minimum or (
            maximum is not None and value > maximum):
        fail("PROBE_SCHEMA")
    return value


def _number(value: Any, *, minimum: float = 0.0,
            maximum: float | None = None) -> float:
    if type(value) is bool or not isinstance(value, (int, float)):
        fail("PROBE_SCHEMA")
    number = float(value)
    if not math.isfinite(number) or number < minimum or (
            maximum is not None and number > maximum):
        fail("PROBE_SCHEMA")
    return number


def _string(value: Any) -> str:
    if not isinstance(value, str):
        fail("PROBE_SCHEMA")
    return value


def _boolean(value: Any) -> bool:
    if type(value) is not bool:
        fail("PROBE_SCHEMA")
    return value


def _status_reason(value: dict[str, Any]) -> tuple[str, str]:
    status = value.get("status")
    reason = value.get("reason")
    if status not in ("OBSERVED", "NOT_MEASURED") or not isinstance(reason, str):
        fail("PROBE_SCHEMA")
    if status == "OBSERVED" and reason:
        fail("PROBE_SCHEMA")
    if status == "NOT_MEASURED" and reason not in NOT_MEASURED_PATHS:
        fail("PROBE_SCHEMA")
    return status, reason


def validate_probe(document: dict[str, Any], expected: dict[str, Any]) -> dict[str, Any]:
    _object(document, ("ok", "probe_version", "pid", "window_ms", "caps", "window",
                       "cpu", "dwm", "frames", "gpu"))
    if document["ok"] is not True:
        fail("PROBE_SCHEMA")
    _integer(document["probe_version"], minimum=PROBE_VERSION, maximum=PROBE_VERSION)
    if _integer(document["pid"], minimum=1) != expected["pid"]:
        fail("PROBE_SCHEMA")
    caps = _object(document["caps"], ("seconds", "interval_ms", "max_samples",
                                      "max_window_pixels"))
    for name, bound in (("seconds", SECONDS_RANGE[1]), ("interval_ms", INTERVAL_RANGE[1]),
                        ("max_samples", MAX_SAMPLES_RANGE[1]),
                        ("max_window_pixels", MAX_WINDOW_PIXELS_RANGE[1])):
        _integer(caps[name], minimum=1, maximum=bound)
        if caps[name] != expected["caps"][name]:
            fail("PROBE_SCHEMA")
    ceiling_ms = (expected["caps"]["seconds"] + WINDOW_MS_CEILING_SLACK_S) * 1000
    _number(document["window_ms"], minimum=1.0, maximum=float(ceiling_ms))

    window = _object(document["window"], ("status", "reason", "title", "handle", "left",
                                          "top", "width", "height", "visible", "minimized"))
    window_status, _ = _status_reason(window)
    _string(window["title"])
    handle = _integer(window["handle"])
    left = _integer(window["left"], minimum=-10_000_000)
    top = _integer(window["top"], minimum=-10_000_000)
    width = _integer(window["width"])
    height = _integer(window["height"])
    visible = _boolean(window["visible"])
    _boolean(window["minimized"])
    if window_status == "OBSERVED":
        if handle < 1 or width < 1 or height < 1 or not visible:
            fail("PROBE_SCHEMA")
        if width * height > expected["caps"]["max_window_pixels"]:
            fail("PROBE_SCHEMA")
    else:
        if handle or width or height or left or top or visible:
            fail("PROBE_SCHEMA")

    cpu = _object(document["cpu"], ("status", "reason", "cpu_ms", "source", "attribution"))
    cpu_status, _ = _status_reason(cpu)
    if cpu["source"] != "GetProcessTimes" or cpu["attribution"] != "process_over_window":
        fail("PROBE_SCHEMA")
    cpu_ms = _number(cpu["cpu_ms"])
    if cpu_status == "NOT_MEASURED" and cpu_ms != 0.0:
        fail("PROBE_SCHEMA")

    dwm = _object(document["dwm"], ("status", "reason", "error", "composed_delta",
                                    "displayed_delta", "dropped_delta", "missed_delta",
                                    "late_delta"))
    dwm_status, _ = _status_reason(dwm)
    _string(dwm["error"])
    deltas = {}
    for name in ("composed_delta", "displayed_delta", "dropped_delta", "missed_delta",
                 "late_delta"):
        deltas[name] = _integer(dwm[name])
    if dwm_status == "NOT_MEASURED" and any(deltas.values()):
        fail("PROBE_SCHEMA")

    frames = _object(document["frames"], ("status", "reason", "method", "samples",
                                          "changes", "uniform_captures", "failed_captures",
                                          "interval_ms"))
    frames_status, _ = _status_reason(frames)
    if frames["method"] != PROBE_FRAME_METHOD:
        fail("PROBE_SCHEMA")
    interval = _integer(frames["interval_ms"], minimum=1, maximum=1000)
    if interval != expected["caps"]["interval_ms"]:
        fail("PROBE_SCHEMA")
    samples = _integer(frames["samples"], maximum=expected["caps"]["max_samples"])
    changes = _integer(frames["changes"], maximum=samples)
    uniform = _integer(frames["uniform_captures"], maximum=samples)
    failed = _integer(frames["failed_captures"], maximum=samples)
    if changes > samples - failed:
        fail("PROBE_SCHEMA")
    if frames_status == "OBSERVED" and (samples < 1 or uniform >= samples):
        fail("PROBE_SCHEMA")

    gpu = _object(document["gpu"], ("status", "reason", "error", "samples",
                                    "engine_instances", "mean_percent_summed_over_engines",
                                    "max_percent_summed_over_engines", "engine_types"))
    gpu_status, _ = _status_reason(gpu)
    _string(gpu["error"])
    gpu_samples = _integer(gpu["samples"], maximum=10_000)
    instances = _integer(gpu["engine_instances"], maximum=10_000)
    mean = _number(gpu["mean_percent_summed_over_engines"], maximum=1_000_000.0)
    maximum = _number(gpu["max_percent_summed_over_engines"], maximum=1_000_000.0)
    _string(gpu["engine_types"])
    if maximum < mean:
        fail("PROBE_SCHEMA")
    if gpu_status == "OBSERVED":
        if gpu_samples < 1 or instances < 1:
            fail("PROBE_SCHEMA")
    elif gpu_samples or instances or mean or maximum or gpu["engine_types"]:
        fail("PROBE_SCHEMA")
    return document


def _probe_digest(path: Path) -> str | None:
    try:
        size = path.stat().st_size
        if size > MAX_PROBE_BYTES:
            return None
        return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError:
        return None


# ------------------------------------------------------------------ attribution record


def build_record(document: dict[str, Any], *, probe: Path, probe_sha256: str | None,
                 executable_sha256: str | None = None) -> dict[str, Any]:
    if executable_sha256 is not None and (
            not isinstance(executable_sha256, str) or not SHA256.fullmatch(executable_sha256)):
        fail("ARGS_IDENTITY")
    window = document["window"]
    frames = document["frames"]
    dwm = document["dwm"]
    cpu = document["cpu"]
    gpu = document["gpu"]
    caps = document["caps"]
    measured_ms = float(document["window_ms"])
    samples = frames["samples"]
    achieved = round(measured_ms / samples, 3) if samples else None
    window_observation = (
        "largest visible top-level window owned by the target pid when the probe started; "
        "title/geometry are the OS values, not app-reported ones")
    frames_observation = (
        "sampled window-content differences: PrintWindow(PW_RENDERFULLCONTENT) asks the owning "
        "window to render its content into a device context at the requested interval; the "
        "count is how many times consecutive sampled captures differed. PrintWindow is not a "
        "compositor or presentation counter, so the relationship of these image differences to "
        "presented frames is unknown and unquantified; this is not a frame count, not scanout "
        "and not a displayed-frame signal.")
    frames_caveat = (
        "PrintWindow asks the owning window to render into a device context, so a possible "
        "observer effect cannot be excluded and is not quantified; the sampler's own cost is "
        "outside the measured process CPU. Subsampling: one byte (the blue channel) is read "
        "per sampled pixel group, at a stride of 64, 128 or 256 bytes depending on window "
        "size, so un-sampled pixels are invisible and a small or localised change can be "
        "missed; the count is sampled image differences, not frames.")
    compositor_observation = (
        "DwmGetCompositionTimingInfo deltas read before and after the declared sampling "
        "window, for the whole desktop compositor; desktop-wide and never attributable to "
        "this app alone. One warm-up read is discarded first so the baseline is a second "
        "call: fields documented as valid only after a second call are not diffed from an "
        "unvalidated first call. This host refused the API (DWM_TIMING_UNAVAILABLE), so the "
        "success branch is implemented but unexercised here")
    cpu_observation = (
        "process CPU time from GetProcessTimes; the two reads bracket the declared sampling "
        "window (immediately before and after it), total across threads, never per frame")
    gpu_observation = (
        "mean of sampled sums of PDH \\GPU Engine(*)\\Utilization Percentage over the engine "
        "instances attributed to the target pid; sampled about once per second, summed over "
        "engines so it can exceed 100; not a count of frames presented by this app")
    gpu_caveat = (
        "PDH attributes GPU work to the process that submits it; helper or child processes "
        "are not included")
    content_observed = frames["status"] == "OBSERVED"
    compositor_observed = dwm["status"] == "OBSERVED"
    cpu_observed = cpu["status"] == "OBSERVED"
    gpu_observed = gpu["status"] == "OBSERVED"

    record: dict[str, Any] = {
        "attribution_version": ATTRIBUTION_VERSION,
        "record_kind": "n06_independent_attribution",
        "record_version": PROBE_RECORD_VERSION,
        "observed_utc": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "probe": {
            "path": str(probe),
            "sha256": probe_sha256,
            "sha256_reason": "" if probe_sha256 else "PROBE_SOURCE_UNREADABLE",
            "version": document["probe_version"],
        },
        "target": {
            "pid": document["pid"],
            "window": {key: window[key] for key in (
                "status", "reason", "title", "handle", "left", "top", "width", "height",
                "visible", "minimized")},
            "window_observation": window_observation,
            # The binary identity is supplied by the calling runner, not measured here.
            # ASSERTED (never OBSERVED) keeps a runner assertion distinct from a probe
            # observation; the same digest and the pre-launch verification live in the run's
            # retained input-declaration.json / run-summary.json.
            "executable": {
                "status": "ASSERTED" if executable_sha256 else "NOT_MEASURED",
                "reason": "" if executable_sha256 else "EXECUTABLE_IDENTITY_NOT_SUPPLIED",
                "sha256": executable_sha256,
                "identity_source": (
                    "calling_runner_prelaunch_digest_verification"
                    if executable_sha256 else ""),
                "observation": (
                    "not a probe measurement: the calling runner verified this digest before "
                    "launch and the retained run notes (input-declaration.json / "
                    "run-summary.json executable_sha256) carry the same value")
                if executable_sha256 else
                ("no executable identity was supplied by the calling runner; this block is "
                 "NOT_MEASURED rather than inferred"),
            },
        },
        "declared_window": {
            "requested_seconds": caps["seconds"],
            "measured_ms": measured_ms,
            "requested_interval_ms": caps["interval_ms"],
            "samples": samples,
            "achieved_mean_interval_ms": achieved,
            "note": (
                "measured_ms brackets the content-sampling loop; the CPU and DWM deltas are "
                "read immediately before and after that loop"),
        },
        "frames": {
            "window_content_changes": {
                "status": frames["status"],
                "reason": frames["reason"],
                "unit": "sampled_window_content_changes",
                "method": frames["method"],
                "samples": samples,
                # The value is null when the endpoint is NOT_MEASURED; the sample counts
                # above stay as diagnostics of what was attempted.
                "changes": frames["changes"] if content_observed else None,
                "uniform_captures": frames["uniform_captures"],
                "failed_captures": frames["failed_captures"],
                "requested_interval_ms": frames["interval_ms"],
                "achieved_mean_interval_ms": achieved,
                "observation": frames_observation,
                "caveat": frames_caveat,
            },
            "compositor_desktop_wide": {
                "status": dwm["status"],
                "reason": dwm["reason"],
                "error": dwm["error"],
                "unit": "compositor_frames_whole_desktop",
                "per_app": False,
                "composed_delta": dwm["composed_delta"] if compositor_observed else None,
                "displayed_delta": dwm["displayed_delta"] if compositor_observed else None,
                "dropped_delta": dwm["dropped_delta"] if compositor_observed else None,
                "missed_delta": dwm["missed_delta"] if compositor_observed else None,
                "late_delta": dwm["late_delta"] if compositor_observed else None,
                "observation": compositor_observation,
            },
            "app_displayed_frames": {
                "status": "NOT_MEASURED",
                "reason": APP_DISPLAYED_REASON,
                "observation": (
                    "independent frame count: NOT_MEASURED. No independent presentation or "
                    "compositor counter for this app exists in this tool, so N06's "
                    "independent-frame-count criterion stays NOT_MEASURED until such a basis "
                    "exists; the content sampler counts sampled image differences only and its "
                    "relationship to presented frames is unknown. No field in this record is a "
                    "presented or displayed frame count."),
            },
        },
        "cpu": {
            "status": cpu["status"],
            "reason": cpu["reason"],
            "attribution": "process_over_window",
            "source": "GetProcessTimes",
            "cpu_ms": cpu["cpu_ms"] if cpu_observed else None,
            "window_ms": measured_ms,
            "observation": cpu_observation,
            "quantisation_note": (
                "GetProcessTimes is quantised by the scheduler clock; a delta below one tick "
                "reads 0 ms, which means less than one tick, not zero CPU"),
        },
        "gpu": {
            "status": gpu["status"],
            "reason": gpu["reason"],
            "error": gpu["error"],
            "unit": "pdh_gpu_engine_percent_summed_over_engines",
            # No engine/sample count is reported without an observed sample.
            "samples": gpu["samples"] if gpu_observed else None,
            "engine_instances": gpu["engine_instances"] if gpu_observed else None,
            "engine_types": gpu["engine_types"] if gpu_observed else "",
            "mean_percent_summed_over_engines": (
                gpu["mean_percent_summed_over_engines"] if gpu_observed else None),
            "max_percent_summed_over_engines": (
                gpu["max_percent_summed_over_engines"] if gpu_observed else None),
            "observation": gpu_observation,
            "caveat": gpu_caveat,
        },
        "per_frame_cpu": _derived_ratio(cpu, frames, measured_ms),
        "app_reported_stand_in": False,
        "caps": dict(caps),
    }
    return record


def _derived_ratio(cpu: dict[str, Any], frames: dict[str, Any],
                   measured_ms: float) -> dict[str, Any]:
    if cpu["status"] != "OBSERVED":
        return {"status": "NOT_MEASURED", "reason": "CPU_NOT_MEASURED"}
    if frames["status"] != "OBSERVED":
        return {"status": "NOT_MEASURED", "reason": "FRAMES_NOT_MEASURED"}
    if float(cpu["cpu_ms"]) <= 0.0:
        # A zero GetProcessTimes delta is a scheduler-clock resolution floor (below one
        # tick), not a measured zero: 0 ms / changes would be a fabricated exact zero.
        return {"status": "NOT_MEASURED", "reason": "CPU_BELOW_CLOCK_RESOLUTION"}
    if frames["changes"] < 1:
        return {"status": "NOT_MEASURED", "reason": "NO_FRAME_CHANGES"}
    changes = frames["changes"]
    return {
        "status": "DERIVED",
        "kind": "ratio",
        "numerator_ms": cpu["cpu_ms"],
        "denominator": changes,
        "denominator_unit": "sampled_window_content_changes",
        "value_ms_per_observed_change": round(float(cpu["cpu_ms"]) / changes, 6),
        "window_ms": measured_ms,
        "direction": (
            "the denominator is sampled image differences, not frames, so this ratio is per "
            "observed sampled content change and never per presented frame; it says nothing "
            "about per-frame CPU cost"),
        "reason": "",
        "note": (
            "derived over the declared window; per observed sampled content change; not a "
            "per-frame measurement"),
    }


def write_record(record: dict[str, Any], path: str | Path) -> None:
    body = json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
    if len(body) > MAX_RECORD_BYTES:
        fail("RECORD_TOO_LARGE")
    try:
        Path(path).write_bytes(body)
    except OSError:
        fail("WRITE_FAILED")


def load_record(path: str | Path) -> dict[str, Any]:
    try:
        size = Path(path).stat().st_size
    except OSError:
        fail("COMPARE_INPUT")
    if size > MAX_RECORD_BYTES:
        fail("COMPARE_INPUT")
    try:
        document = contracts.load_document(str(path), MAX_RECORD_BYTES)
    except contracts.ContractError:
        fail("COMPARE_INPUT")
    try:
        validate_record(document)
    except AttributionError as error:
        # A malformed comparison input is an input problem, not a probe-schema problem.
        fail("COMPARE_INPUT" if error.code == "PROBE_SCHEMA" else error.code)
    return document


RECORD_FIELDS = ("attribution_version", "record_kind", "record_version", "observed_utc",
                 "probe", "target", "declared_window", "frames", "cpu", "gpu",
                 "per_frame_cpu", "app_reported_stand_in", "caps")
WINDOW_RECORD_FIELDS = ("status", "reason", "title", "handle", "left", "top", "width",
                        "height", "visible", "minimized")
EXECUTABLE_FIELDS = ("status", "reason", "sha256", "identity_source", "observation")
DECLARED_FIELDS = ("requested_seconds", "measured_ms", "requested_interval_ms", "samples",
                   "achieved_mean_interval_ms", "note")
CAPS_FIELDS = ("seconds", "interval_ms", "max_samples", "max_window_pixels")
CPU_FIELDS = ("status", "reason", "attribution", "source", "cpu_ms", "window_ms",
              "observation", "quantisation_note")
FRAME_BLOCK_FIELDS = ("status", "reason", "unit", "method", "samples", "changes",
                      "uniform_captures", "failed_captures", "requested_interval_ms",
                      "achieved_mean_interval_ms", "observation", "caveat")
COMPOSITOR_FIELDS = ("status", "reason", "error", "unit", "per_app", "composed_delta",
                     "displayed_delta", "dropped_delta", "missed_delta", "late_delta",
                     "observation")
APP_DISPLAYED_FIELDS = ("status", "reason", "observation")
GPU_FIELDS = ("status", "reason", "error", "unit", "samples", "engine_instances",
              "engine_types", "mean_percent_summed_over_engines",
              "max_percent_summed_over_engines", "observation", "caveat")
RATIO_DERIVED_FIELDS = ("status", "kind", "numerator_ms", "denominator", "denominator_unit",
                        "value_ms_per_observed_change", "window_ms", "direction", "reason",
                        "note")
RATIO_NOT_MEASURED_FIELDS = ("status", "reason")
CONTENT_UNIT = "sampled_window_content_changes"
COMPOSITOR_UNIT = "compositor_frames_whole_desktop"
GPU_UNIT = "pdh_gpu_engine_percent_summed_over_engines"
IDENTITY_SOURCE = "calling_runner_prelaunch_digest_verification"
PROBE_SHA_REASONS = {"PROBE_SOURCE_UNREADABLE"}
OBSERVED_UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
# The retained N06 records predate the null form for NOT_MEASURED values and store an
# explicit 0 instead. That representation is tolerated only for records that bind one of
# these documented probe digests; any other record must use null. Verifiable, not blanket.
LEGACY_ZERO_PROBE_SHA256 = frozenset((
    "sha256:faaa3b129b2f82ef6cb906c94a4e7f7d2e3b249cdcc963a002e72a63c96dd89a",
    "sha256:45433d6d29bfac83a5f8d93e0af37e30f1e3800384882e51210398a2a85bc1d5",
))


def _compare_fail() -> None:
    fail("COMPARE_INPUT")


def _record_str(value: Any, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value.strip()):
        _compare_fail()
    return value


def _record_int(value: Any, *, minimum: int = 0,
                maximum: int | None = None) -> int:
    if type(value) is not int or value < minimum or (
            maximum is not None and value > maximum):
        _compare_fail()
    return value


def _record_number(value: Any, *, minimum: float = 0.0,
                   maximum: float | None = None) -> float:
    if type(value) is bool or not isinstance(value, (int, float)):
        _compare_fail()
    number = float(value)
    if not math.isfinite(number) or number < minimum or (
            maximum is not None and number > maximum):
        _compare_fail()
    return number


def _record_bool(value: Any) -> bool:
    if type(value) is not bool:
        _compare_fail()
    return value


def _absent(value: Any, *, legacy_zero: bool) -> bool:
    """A NOT_MEASURED value is null.

    The explicit numeric zero is the pre-null *legacy representation* that the retained N06
    records carry. It is accepted only when the record is identifiably legacy, i.e. it binds
    one of the documented pre-null probe digests (`LEGACY_ZERO_PROBE_SHA256`), the endpoint
    status is NOT_MEASURED (the caller is already in that branch), and the value is exactly
    zero. A current-form record (any other probe digest) must use null here, and a nonzero
    value is never accepted.
    """
    if value is None:
        return True
    if type(value) is bool:
        return False
    return legacy_zero and isinstance(value, (int, float)) and float(value) == 0.0


def _record_status(value: Any, allowed: tuple[str, ...]) -> str:
    if value not in allowed:
        _compare_fail()
    return value


def _record_reason(value: Any, allowed: set[str] | tuple[str, ...]) -> str:
    if not isinstance(value, str) or value not in allowed:
        _compare_fail()
    return value


def _validate_window_record(window: dict[str, Any]) -> None:
    status = _record_status(window["status"], ("OBSERVED", "NOT_MEASURED"))
    _record_str(window["title"], allow_empty=True)
    handle = _record_int(window["handle"])
    left = _record_int(window["left"], minimum=-10_000_000)
    top = _record_int(window["top"], minimum=-10_000_000)
    width = _record_int(window["width"])
    height = _record_int(window["height"])
    visible = _record_bool(window["visible"])
    _record_bool(window["minimized"])
    if status == "OBSERVED":
        if window["reason"] != "":
            _compare_fail()
        if handle < 1 or width < 1 or height < 1 or visible is not True:
            _compare_fail()
    else:
        _record_reason(window["reason"], NOT_MEASURED_PATHS)
        if handle or width or height or left or top or visible is not False:
            _compare_fail()


def _validate_executable_record(executable: dict[str, Any]) -> None:
    # ASSERTED is the current caller assertion; OBSERVED is the documented legacy tolerance
    # for retained pre-C-4 records only. Neither is a probe measurement.
    status = executable["status"]
    if status in ("ASSERTED", "OBSERVED"):
        if not isinstance(executable["sha256"], str) or not SHA256.fullmatch(
                executable["sha256"]):
            _compare_fail()
        if executable["identity_source"] != IDENTITY_SOURCE:
            _compare_fail()
        _record_str(executable["observation"])
    elif status == "NOT_MEASURED":
        _record_reason(executable["reason"], EXECUTABLE_IDENTITY_REASONS)
        if executable["sha256"] is not None or executable["identity_source"] != "":
            _compare_fail()
        _record_str(executable["observation"])
    else:
        _compare_fail()


def validate_record(document: Any) -> dict[str, Any]:
    """Closed *semantic* validation of a record this module wrote.

    Field presence alone is not enough: every endpoint's status/value/reason triple is
    checked against the invariants the writer guarantees, so a tampered record cannot smuggle
    a value into a NOT_MEASURED endpoint (or a reason into an observed one). NOT_MEASURED
    numeric values are null; the explicit legacy zero from pre-null retained records is
    accepted for replay only.
    """
    _object(document, RECORD_FIELDS)
    if document["attribution_version"] != ATTRIBUTION_VERSION:
        _compare_fail()
    if document["record_kind"] != "n06_independent_attribution":
        _compare_fail()
    if document["record_version"] != PROBE_RECORD_VERSION:
        _compare_fail()
    if document["app_reported_stand_in"] is not False:
        # An app-supplied value must never become an independent endpoint.
        _compare_fail()
    if not isinstance(document["observed_utc"], str) or not OBSERVED_UTC.fullmatch(
            document["observed_utc"]):
        _compare_fail()

    probe = _object(document["probe"], ("path", "sha256", "sha256_reason", "version"))
    _record_str(probe["path"])
    _record_int(probe["version"], minimum=PROBE_VERSION, maximum=PROBE_VERSION)
    if probe["sha256"] is None:
        _record_reason(probe["sha256_reason"], PROBE_SHA_REASONS)
    else:
        if not isinstance(probe["sha256"], str) or not SHA256.fullmatch(probe["sha256"]):
            _compare_fail()
        if probe["sha256_reason"] != "":
            _compare_fail()
    # The explicit-zero legacy representation is admissible only for records that bind a
    # documented pre-null probe digest; a current-form record must use null.
    legacy_zero = probe["sha256"] in LEGACY_ZERO_PROBE_SHA256

    target = _object(document["target"], ("pid", "window", "window_observation",
                                          "executable"))
    _record_int(target["pid"], minimum=1)
    _validate_window_record(_object(target["window"], WINDOW_RECORD_FIELDS))
    _record_str(target["window_observation"])
    _validate_executable_record(_object(target["executable"], EXECUTABLE_FIELDS))

    declared = _object(document["declared_window"], DECLARED_FIELDS)
    seconds = _record_int(declared["requested_seconds"], minimum=SECONDS_RANGE[0],
                          maximum=SECONDS_RANGE[1])
    measured = _record_number(declared["measured_ms"], minimum=1.0,
                              maximum=(seconds + WINDOW_MS_CEILING_SLACK_S) * 1000.0)
    interval = _record_int(declared["requested_interval_ms"], minimum=INTERVAL_RANGE[0],
                           maximum=INTERVAL_RANGE[1])
    samples = _record_int(declared["samples"], maximum=MAX_SAMPLES_RANGE[1])
    achieved = declared["achieved_mean_interval_ms"]
    if samples == 0:
        if achieved is not None:
            _compare_fail()
    else:
        achieved_number = _record_number(achieved, minimum=1.0)
        if abs(achieved_number - round(measured / samples, 3)) > 1e-6:
            _compare_fail()
    _record_str(declared["note"])
    caps = _object(document["caps"], CAPS_FIELDS)
    if _record_int(caps["seconds"], minimum=SECONDS_RANGE[0],
                   maximum=SECONDS_RANGE[1]) != seconds:
        _compare_fail()
    if _record_int(caps["interval_ms"], minimum=INTERVAL_RANGE[0],
                   maximum=INTERVAL_RANGE[1]) != interval:
        _compare_fail()
    max_samples = _record_int(caps["max_samples"], minimum=MAX_SAMPLES_RANGE[0],
                              maximum=MAX_SAMPLES_RANGE[1])
    if samples > max_samples:
        _compare_fail()
    _record_int(caps["max_window_pixels"], minimum=MAX_WINDOW_PIXELS_RANGE[0],
                maximum=MAX_WINDOW_PIXELS_RANGE[1])

    frames = _object(document["frames"], ("window_content_changes",
                                          "compositor_desktop_wide",
                                          "app_displayed_frames"))
    content = _object(frames["window_content_changes"], FRAME_BLOCK_FIELDS)
    content_status = _record_status(content["status"], ("OBSERVED", "NOT_MEASURED"))
    if content["unit"] != CONTENT_UNIT or content["method"] != PROBE_FRAME_METHOD:
        _compare_fail()
    _record_str(content["observation"])
    _record_str(content["caveat"])
    content_samples = _record_int(content["samples"], maximum=max_samples)
    uniform = _record_int(content["uniform_captures"], maximum=content_samples)
    failed = _record_int(content["failed_captures"], maximum=content_samples)
    if content["requested_interval_ms"] != interval:
        _compare_fail()
    if content["achieved_mean_interval_ms"] != achieved:
        _compare_fail()
    changes: int | None
    if content_status == "OBSERVED":
        if content["reason"] != "":
            _compare_fail()
        if content_samples < 1 or uniform >= content_samples:
            _compare_fail()
        changes = _record_int(content["changes"], maximum=content_samples - failed)
    else:
        _record_reason(content["reason"], NOT_MEASURED_PATHS)
        if not _absent(content["changes"], legacy_zero=legacy_zero):
            _compare_fail()
        changes = None

    compositor = _object(frames["compositor_desktop_wide"], COMPOSITOR_FIELDS)
    compositor_status = _record_status(compositor["status"], ("OBSERVED", "NOT_MEASURED"))
    if compositor["per_app"] is not False:
        # These counters are whole-desktop by construction; a per-app claim is invalid.
        _compare_fail()
    if compositor["unit"] != COMPOSITOR_UNIT:
        _compare_fail()
    _record_str(compositor["error"], allow_empty=True)
    _record_str(compositor["observation"])
    delta_fields = ("composed_delta", "displayed_delta", "dropped_delta", "missed_delta",
                    "late_delta")
    if compositor_status == "OBSERVED":
        if compositor["reason"] != "":
            _compare_fail()
        for name in delta_fields:
            _record_int(compositor[name])
    else:
        _record_reason(compositor["reason"], NOT_MEASURED_PATHS)
        for name in delta_fields:
            if not _absent(compositor[name], legacy_zero=legacy_zero):
                _compare_fail()

    displayed = _object(frames["app_displayed_frames"], APP_DISPLAYED_FIELDS)
    if displayed["status"] != "NOT_MEASURED":
        # No per-window display counter exists; an OBSERVED claim is fabricated.
        _compare_fail()
    if displayed["reason"] != APP_DISPLAYED_REASON:
        _compare_fail()
    _record_str(displayed["observation"])

    cpu = _object(document["cpu"], CPU_FIELDS)
    cpu_status = _record_status(cpu["status"], ("OBSERVED", "NOT_MEASURED"))
    if cpu["attribution"] != "process_over_window" or cpu["source"] != "GetProcessTimes":
        _compare_fail()
    _record_str(cpu["observation"])
    _record_str(cpu["quantisation_note"])
    if abs(_record_number(cpu["window_ms"], minimum=1.0) - measured) > 1e-6:
        _compare_fail()
    cpu_ms: float | None
    if cpu_status == "OBSERVED":
        if cpu["reason"] != "":
            _compare_fail()
        cpu_ms = _record_number(cpu["cpu_ms"])
    else:
        _record_reason(cpu["reason"], NOT_MEASURED_PATHS)
        if not _absent(cpu["cpu_ms"], legacy_zero=legacy_zero):
            _compare_fail()
        cpu_ms = None

    gpu = _object(document["gpu"], GPU_FIELDS)
    gpu_status = _record_status(gpu["status"], ("OBSERVED", "NOT_MEASURED"))
    if gpu["unit"] != GPU_UNIT:
        _compare_fail()
    _record_str(gpu["error"], allow_empty=True)
    _record_str(gpu["engine_types"], allow_empty=True)
    _record_str(gpu["observation"])
    _record_str(gpu["caveat"])
    if gpu_status == "OBSERVED":
        if gpu["reason"] != "":
            _compare_fail()
        # No engine or sample count without an observed sample.
        _record_int(gpu["samples"], minimum=1, maximum=10_000)
        _record_int(gpu["engine_instances"], minimum=1, maximum=10_000)
        mean = _record_number(gpu["mean_percent_summed_over_engines"], maximum=1_000_000.0)
        maximum = _record_number(gpu["max_percent_summed_over_engines"], maximum=1_000_000.0)
        if maximum < mean:
            _compare_fail()
    else:
        _record_reason(gpu["reason"], NOT_MEASURED_PATHS)
        for name in ("samples", "engine_instances", "mean_percent_summed_over_engines",
                     "max_percent_summed_over_engines"):
            if not _absent(gpu[name], legacy_zero=legacy_zero):
                _compare_fail()
        if gpu["engine_types"] != "":
            _compare_fail()

    ratio_value = document["per_frame_cpu"]
    if not isinstance(ratio_value, dict):
        _compare_fail()
    ratio_status = ratio_value.get("status")
    if ratio_status == "DERIVED":
        ratio = _object(ratio_value, RATIO_DERIVED_FIELDS)
        if ratio["kind"] != "ratio" or ratio["reason"] != "":
            _compare_fail()
        numerator = _record_number(ratio["numerator_ms"], minimum=1e-12)
        denominator = _record_int(ratio["denominator"], minimum=1)
        if ratio["denominator_unit"] != CONTENT_UNIT:
            _compare_fail()
        value = _record_number(ratio["value_ms_per_observed_change"], minimum=1e-12)
        if abs(value - round(numerator / denominator, 6)) > 1e-9:
            _compare_fail()
        if abs(_record_number(ratio["window_ms"], minimum=1.0) - measured) > 1e-6:
            _compare_fail()
        _record_str(ratio["direction"])
        _record_str(ratio["note"])
        # A derived ratio requires an observed positive CPU numerator and an observed
        # content-change denominator that equals the record's own observed changes. The
        # numerator must be the record's measured CPU value: a self-consistent fabricated
        # numerator/value pair is still a tamper, not a derived measurement.
        if cpu_status != "OBSERVED" or cpu_ms is None or cpu_ms <= 0:
            _compare_fail()
        if numerator != cpu_ms:
            _compare_fail()
        if content_status != "OBSERVED" or denominator != changes:
            _compare_fail()
    elif ratio_status == "NOT_MEASURED":
        ratio = _object(ratio_value, RATIO_NOT_MEASURED_FIELDS)
        reason = _record_reason(ratio["reason"], PER_FRAME_NOT_MEASURED_REASONS)
        if reason == "CPU_NOT_MEASURED" and cpu_status != "NOT_MEASURED":
            _compare_fail()
        elif reason == "FRAMES_NOT_MEASURED" and content_status != "NOT_MEASURED":
            _compare_fail()
        elif reason == "NO_FRAME_CHANGES" and (
                content_status != "OBSERVED" or changes is None or changes >= 1):
            _compare_fail()
        elif reason == "CPU_BELOW_CLOCK_RESOLUTION" and (
                cpu_status != "OBSERVED" or cpu_ms is None or cpu_ms > 0
                or content_status != "OBSERVED"):
            _compare_fail()
    else:
        _compare_fail()
    return document


# ------------------------------------------------------------------ comparison


def _record_sha256(document: dict[str, Any]) -> str:
    body = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return "sha256:" + hashlib.sha256(body.encode()).hexdigest()


def _endpoint_state(value: dict[str, Any]) -> dict[str, Any]:
    return {"status": value["status"], "reason": value["reason"]}


def _difference(left: dict[str, Any], right: dict[str, Any], field: str,
                unit: str, label: str) -> dict[str, Any]:
    if left["status"] != "OBSERVED" or right["status"] != "OBSERVED":
        return {
            "status": "NOT_MEASURED",
            "reason": "INPUT_NOT_MEASURED",
            "a0_status": left["status"],
            "a0_reason": left["reason"],
            "a1_status": right["status"],
            "a1_reason": right["reason"],
        }
    a0 = left[field]
    a1 = right[field]
    delta = round(float(a1) - float(a0), 6)
    return {"status": "OBSERVED", "a0": a0, "a1": a1, "a1_minus_a0": delta,
            "unit": unit, "label": label}


MAX_QUALIFICATIONS = 8
MAX_QUALIFICATION_CHARS = 2000


def _qualifications(values: list[str] | None) -> list[str]:
    """Bounded caller-recorded qualifications (declared run facts, never measurements)."""
    if not values:
        return []
    if not isinstance(values, list) or len(values) > MAX_QUALIFICATIONS:
        fail("ARGS_CAP")
    result = []
    for value in values:
        if not isinstance(value, str) or not value.strip() or control_character(value):
            fail("ARGS_CAP")
        if len(value) > MAX_QUALIFICATION_CHARS:
            fail("ARGS_CAP")
        result.append(value.strip())
    return result


def control_character(value: str) -> bool:
    return any(ord(character) < 0x20 and character not in "\t\n" for character in value)


def compare_records(a0: dict[str, Any], a1: dict[str, Any],
                    qualifications: list[str] | None = None) -> dict[str, Any]:
    validate_record(a0)
    validate_record(a1)
    declared = _qualifications(qualifications)
    a0_frames = a0["frames"]["window_content_changes"]
    a1_frames = a1["frames"]["window_content_changes"]
    a0_ratio = a0["per_frame_cpu"]
    a1_ratio = a1["per_frame_cpu"]
    deltas = {
        "process_cpu_ms": _difference(
            a0["cpu"], a1["cpu"], "cpu_ms", "ms",
            "process CPU ms over each declared window"),
        "sampled_window_content_changes": _difference(
            a0_frames, a1_frames, "changes", "sampled_window_content_changes",
            "observed window-content changes per declared window"),
        "gpu_mean_percent_summed_over_engines": _difference(
            a0["gpu"], a1["gpu"], "mean_percent_summed_over_engines", "percent",
            "mean PDH GPU engine utilisation percent summed over engines"),
    }
    if a0_ratio["status"] == "DERIVED" and a1_ratio["status"] == "DERIVED":
        value0 = a0_ratio["value_ms_per_observed_change"]
        value1 = a1_ratio["value_ms_per_observed_change"]
        deltas["cpu_ms_per_observed_change"] = {
            "status": "DERIVED", "a0": value0, "a1": value1,
            "a1_minus_a0": round(float(value1) - float(value0), 6),
            "unit": "ms_per_sampled_window_content_change",
            "label": "derived ratio per observed sampled content change, descriptive only",
        }
    else:
        deltas["cpu_ms_per_observed_change"] = {
            "status": "NOT_MEASURED",
            "reason": "INPUT_NOT_MEASURED",
            "a0_status": a0_ratio["status"],
            "a0_reason": a0_ratio.get("reason", ""),
            "a1_status": a1_ratio["status"],
            "a1_reason": a1_ratio.get("reason", ""),
        }
    endpoints = {
        "frames.app_displayed_frames": (a0["frames"]["app_displayed_frames"],
                                        a1["frames"]["app_displayed_frames"]),
        "frames.compositor_desktop_wide": (a0["frames"]["compositor_desktop_wide"],
                                           a1["frames"]["compositor_desktop_wide"]),
        "frames.window_content_changes": (a0_frames, a1_frames),
        "cpu.process_over_window": (a0["cpu"], a1["cpu"]),
        "gpu.engine_utilization": (a0["gpu"], a1["gpu"]),
    }
    residual = []
    for name, (left, right) in endpoints.items():
        if left["status"] != "OBSERVED" or right["status"] != "OBSERVED":
            residual.append({"endpoint": name, "a0": _endpoint_state(left),
                             "a1": _endpoint_state(right)})
    if a0_ratio["status"] != "DERIVED" or a1_ratio["status"] != "DERIVED":
        residual.append({
            "endpoint": "per_frame_cpu.derived_ratio",
            "a0": {"status": a0_ratio["status"], "reason": a0_ratio.get("reason", "")},
            "a1": {"status": a1_ratio["status"], "reason": a1_ratio.get("reason", "")},
        })
    return {
        "comparison_version": 1,
        "comparison_kind": "descriptive_paired_run",
        "app_reported_stand_in": False,
        "inputs": {
            "a0": {
                "label": "A0",
                "executable_sha256": a0["target"]["executable"]["sha256"],
                "executable_identity_status": a0["target"]["executable"]["status"],
                "input_record_sha256": _record_sha256(a0),
                "probe_sha256": a0["probe"]["sha256"],
                "pid": a0["target"]["pid"],
                "window_ms": a0["declared_window"]["measured_ms"],
            },
            "a1": {
                "label": "A1",
                "executable_sha256": a1["target"]["executable"]["sha256"],
                "executable_identity_status": a1["target"]["executable"]["status"],
                "input_record_sha256": _record_sha256(a1),
                "probe_sha256": a1["probe"]["sha256"],
                "pid": a1["target"]["pid"],
                "window_ms": a1["declared_window"]["measured_ms"],
            },
        },
        "input_naming": (
            "executable_sha256 is the caller-verified binary digest supplied to the probe run; "
            "executable_identity_status is ASSERTED for records written after C-4, and OBSERVED "
            "only as a documented legacy status for retained pre-C-4 records; "
            "input_record_sha256 is the digest of the attribution record JSON that was "
            "compared; probe_sha256 is the digest of the probe script that produced it"),
        "runs": {
            "a0": {
                "cpu": _endpoint_state(a0["cpu"]),
                "frames": _endpoint_state(a0_frames),
                "gpu": _endpoint_state(a0["gpu"]),
                "per_frame_cpu": {"status": a0_ratio["status"],
                                  "reason": a0_ratio.get("reason", "")},
            },
            "a1": {
                "cpu": _endpoint_state(a1["cpu"]),
                "frames": _endpoint_state(a1_frames),
                "gpu": _endpoint_state(a1["gpu"]),
                "per_frame_cpu": {"status": a1_ratio["status"],
                                  "reason": a1_ratio.get("reason", "")},
            },
        },
        "deltas": deltas,
        "residual_not_measured": residual,
        "qualifications": declared,
        "note": (
            "one declared window per binary, run sequentially; this comparison contains one "
            "repetition and makes no claim about repetition spread, no noise floor is "
            "established and there is no randomized order, so the deltas are descriptive and "
            "are not a controlled estimator, a threshold or a ratification. CPU values are "
            "quantised by the OS scheduler clock, so small CPU deltas are within one tick of "
            "each other"),
    }



# --------------------------------------------------------------------------- cli


def _run_attribute(args: argparse.Namespace) -> dict[str, Any]:
    pid = args.pid
    if type(pid) is not int or pid < 1:
        fail("ARGS_PID")
    seconds = _bounded_int(args.seconds, SECONDS_RANGE)
    interval_ms = _bounded_int(args.interval_ms, INTERVAL_RANGE)
    max_samples = _bounded_int(args.max_samples, MAX_SAMPLES_RANGE)
    max_window_pixels = _bounded_int(args.max_window_pixels, MAX_WINDOW_PIXELS_RANGE)
    # The caps must be self-consistent: the requested cadence can never exceed the sample cap.
    if math.ceil(seconds * 1000 / interval_ms) > max_samples:
        fail("ARGS_CAP")
    powershell = _resolve_powershell(args.powershell)
    probe = Path(args.probe)
    if not probe.is_file():
        fail("PROBE_MISSING")
    executable_sha256 = args.executable_sha256
    if executable_sha256 is not None:
        if not isinstance(executable_sha256, str) or not SHA256.fullmatch(executable_sha256):
            fail("ARGS_IDENTITY")
    caps = {"seconds": seconds, "interval_ms": interval_ms, "max_samples": max_samples,
            "max_window_pixels": max_window_pixels}
    document = run_probe(
        probe_command(powershell, probe, pid=pid, seconds=seconds, interval_ms=interval_ms,
                      max_samples=max_samples, max_window_pixels=max_window_pixels),
        timeout_s=seconds + PROBE_TIMEOUT_SLACK_S)
    validate_probe(document, {"pid": pid, "caps": caps})
    record = build_record(document, probe=probe, probe_sha256=_probe_digest(probe),
                          executable_sha256=executable_sha256)
    if args.out:
        write_record(record, args.out)
    return record


def _run_compare(args: argparse.Namespace) -> dict[str, Any]:
    a0 = load_record(args.a0)
    a1 = load_record(args.a1)
    comparison = compare_records(a0, a1, qualifications=args.qualification)
    if args.out:
        write_record(comparison, args.out)
    return comparison


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
    attribute_parser = commands.add_parser(
        "attribute", help="independent attribution over one declared wall-clock window")
    attribute_parser.add_argument("--pid", type=int, required=True)
    attribute_parser.add_argument("--seconds", type=int, default=10)
    attribute_parser.add_argument("--interval-ms", type=int, default=20)
    attribute_parser.add_argument("--max-samples", type=int, default=2000)
    attribute_parser.add_argument("--max-window-pixels", type=int, default=4_000_000)
    attribute_parser.add_argument("--out", default=None)
    attribute_parser.add_argument(
        "--executable-sha256", default=None,
        help="approved digest of the launched binary, verified by the calling runner")
    attribute_parser.add_argument("--powershell", default="powershell")
    # Test seam: the packaged probe is the default; a test points the *real* CLI at a fake
    # probe so the fail-closed path is exercised end to end rather than monkeypatched.
    attribute_parser.add_argument("--probe", default=str(PROBE))
    compare_parser = commands.add_parser(
        "compare", help="descriptive A0-versus-A1 comparison of two attribution records")
    compare_parser.add_argument("--a0", required=True)
    compare_parser.add_argument("--a1", required=True)
    compare_parser.add_argument("--out", default=None)
    # Caller-declared run facts (for example a capture disposition) that qualification
    # requires but that no probe endpoint can measure. Bounded and closed.
    compare_parser.add_argument("--qualification", action="append", default=None)
    args = parser.parse_args(argv)
    try:
        if args.operation == "feasibility":
            result = {"ok": True, **feasibility()}
        elif args.operation == "frames":
            result = {"ok": True, "journal_events": count_journal(load_capture(args.capture))}
        elif args.operation == "cpu":
            try:
                seconds = int(args.seconds, 10)
            except ValueError:
                fail("ARGS_CAP")
            result = {"ok": True, "process_cpu": sample_process_cpu(
                args.pid, seconds, args.powershell)}
        elif args.operation == "attribute":
            record = _run_attribute(args)
            result = {"ok": True, "record": args.out, "pid": record["target"]["pid"],
                      "window_ms": record["declared_window"]["measured_ms"],
                      "cpu": {"status": record["cpu"]["status"],
                              "cpu_ms": record["cpu"]["cpu_ms"]},
                      "frames": {
                          "status": record["frames"]["window_content_changes"]["status"],
                          "changes": record["frames"]["window_content_changes"]["changes"],
                          "samples": record["frames"]["window_content_changes"]["samples"]},
                      "app_displayed_frames": record["frames"]["app_displayed_frames"],
                      "gpu": {"status": record["gpu"]["status"],
                              "reason": record["gpu"]["reason"]},
                      "per_frame_cpu": record["per_frame_cpu"]}
        else:
            comparison = _run_compare(args)
            result = {"ok": True, "comparison": args.out,
                      "deltas": comparison["deltas"],
                      "residual_not_measured": comparison["residual_not_measured"],
                      "qualifications": comparison["qualifications"]}
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
