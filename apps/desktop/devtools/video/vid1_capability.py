"""MP-VID1 video capability/interop record builder.

Runs one operator-named PowerShell capability probe (absolute path, absolute interpreter,
no PATH search), validates its single bounded JSON document against a closed schema, and
writes a bounded record. The record states what the probe observed and what it could not
observe; it never fills a gap with a guess, never turns an app-reported number into an
independent one, and never selects a video route (that is MP-VID2's owner decision).

Bounded capture
---------------
The probe's stdout and stderr are drained incrementally by one reader thread each. As soon
as either stream would exceed ``--max-output-bytes`` the child is terminated and the run is
refused (``PROBE_OUTPUT_OVERSIZED`` / ``PROBE_STDERR_OVERSIZED``), so the cap is a memory
bound during capture, not a size check afterwards. The same bound is enforced inside the
probe for the tools it spawns (``PROCESS_OUTPUT_CAPPED``).

Closed vocabulary
-----------------
Every status is ``OBSERVED``, ``NOT_MEASURED`` or ``UNSUPPORTED``. A status other than
``OBSERVED`` must carry one of :data:`REASONS`; an ``OBSERVED`` entry must carry none. The
validator also enforces the probe's honesty boundary:

* MFT counts must carry the SDK flag they were produced with (0x1 sync, 0x2 async,
  0x4 hardware, 0x3F all), so an async-software count can never pass as a hardware count.
* A hardware-decode row may only be ``OBSERVED`` when the decoder log showed a hardware
  ``pix_fmt`` and at least one frame decoded. A reported DXVA GUID list is information and
  is never evidence.
* The presentation/copy/colour/device-loss endpoints may never be ``OBSERVED``.
* The route statement must be internally consistent: ``CONFIRMED_LIMITATION`` requires an
  observed decode side, an observed source scan with no presentation call site, and an
  explicit list of the evidence that is still missing; anything else is ``UNEXAMINED``.

Usage
-----
``vid1_capability.py --probe ABS_PS1 --powershell ABS_PWSH [--ffmpeg ABS_EXE]
[--scratch ABS_DIR] [--source-root ABS_DIR] [--out ABS_JSON]``
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import threading
from datetime import datetime, timezone
from pathlib import Path

PROBE_VERSION = 3
RECORD_VERSION = 3
TASK = "MP-VID1"

STATUS_OBSERVED = "OBSERVED"
STATUSES = (STATUS_OBSERVED, "NOT_MEASURED", "UNSUPPORTED")

#: Every reason token the probe may emit. Closed: an unregistered token is a refusal.
REASONS = frozenset(
    """
    COLOR_METADATA_NOT_PROBED
    COPY_COUNT_NOT_INSTRUMENTED
    D3D11_DEVICE_UNAVAILABLE
    DEVICE_LOSS_NOT_EXERCISED
    DXVA_PROFILE_NAMES_NOT_RESOLVED
    FIXTURE_GENERATION_FAILED
    FIXTURE_TOO_LARGE
    GIT_STATUS_NOT_MEASURED
    GIT_UNAVAILABLE
    GPU_SURFACE_IMPORT_UNVERIFIED
    HWACCEL_REQUEST_REJECTED
    HWACCEL_SOFTWARE_FALLBACK
    HWOUTPUT_CONVERT_FAILED
    INTEROP_DISABLED
    MF_COMPONENT_MISSING
    MF_STARTUP_FAILED
    MFT_ENUMERATION_FAILED
    PIN_API_NOT_INSPECTED
    PRESENTATION_PATH_UNMEASURED
    PROCESS_FAILED
    PROCESS_OUTPUT_CAPPED
    PROCESS_SPAWN_FAILED
    PROCESS_TIMEOUT
    PRODUCT_PATH_NOT_IMPLEMENTED
    PROBE_INTERNAL_ERROR
    RAW_OUTPUT_SIZE_UNEXPECTED
    SCRATCH_NOT_SUPPLIED
    SCRATCH_UNAVAILABLE
    SOURCE_SCAN_INSUFFICIENT
    SOURCE_SCAN_NOT_RUN
    SOURCE_SCAN_TRUNCATED
    SOURCE_SCAN_UNAVAILABLE
    SOURCE_SCAN_UNBOUND
    SYNC_STALL_NOT_INSTRUMENTED
    TOOL_NOT_FOUND
    TOOL_NOT_SUPPLIED
    TOOL_PATH_INVALID
    """.split()
)

#: Probe-level refusals (``ok: false``), a separate closed set from endpoint reasons.
PROBE_ERRORS = frozenset({"CAP_RANGE", "PROBE_INTERNAL_ERROR"})

KINDS = frozenset({"decode", "seek", "raw_output", "hw_decode", "hw_download"})
MFT_CATEGORIES = frozenset({"video_decoder", "video_encoder", "video_processor", "audio_decoder"})
FIXTURE_IDS = frozenset({"h264", "hevc10", "vp8"})
COMPONENT_NAMES = frozenset({"mfplat.dll", "mf.dll", "mfreadwrite.dll", "d3d11.dll", "dxva2.dll"})
EVIDENCE_CLASSES = frozenset({"PIX_FMT_OBSERVED", "NONE"})
HARDWARE_PIX_FMTS = frozenset({"d3d11", "d3d11va", "dxva2_vld", "cuda", "qsv", "vaapi", "vulkan"})
GROUND_STATUSES = frozenset({"OBSERVED", "DOCUMENTED", "NOT_MEASURED"})
ROUTE_STATUSES = frozenset({"CONFIRMED_LIMITATION", "UNEXAMINED"})
CORRECTION_STATUSES = frozenset({"CORRECTED_IN_THIS_RECORD", "PRE_FIX_OBSERVATION"})
#: How far the bounded source scan is bound to what it actually scanned.
SCAN_SIGNALS = frozenset({"COMPLETE_BOUND", "SHA_BOUND_TREE_STATE_UNKNOWN", "TRUNCATED", "UNBOUND", "NOT_RUN"})
INSPECTION_STATUSES = frozenset({"NOT_RUN", "OBSERVED"})
#: Evidence a completed route would still need; closed so it cannot drift into prose.
MISSING_EVIDENCE = frozenset(
    {
        "PIN_API_INSPECTION",
        "PIN_SURFACE_API",
        "COPY_COUNT",
        "SYNC_STALLS",
        "COLOR_METADATA",
        "DEVICE_LOSS",
        "PLAYBACK_LATENCY",
    }
)

#: The MFTEnumFlag values (Windows SDK 10.0.19041.0 `mfapi.h`) a probe must record.
MFT_FLAGS = {"sync": "0x1", "async": "0x2", "hardware": "0x4", "all": "0x3F"}

#: Endpoints this probe must never report as OBSERVED, with the reason it may carry.
UNOBSERVABLE_ENDPOINTS = {
    "gpu_surface_import": "GPU_SURFACE_IMPORT_UNVERIFIED",
    "presentation_route": "PRESENTATION_PATH_UNMEASURED",
    "copy_count": "COPY_COUNT_NOT_INSTRUMENTED",
    "synchronisation_stalls": "SYNC_STALL_NOT_INSTRUMENTED",
    "color_metadata": "COLOR_METADATA_NOT_PROBED",
    "device_loss": "DEVICE_LOSS_NOT_EXERCISED",
    "dxva_profile_names": "DXVA_PROFILE_NAMES_NOT_RESOLVED",
    "product_playback_latency": "PRODUCT_PATH_NOT_IMPLEMENTED",
}

CAP_RANGES = {
    "timeout_ms": (1_000, 120_000),
    "max_output_bytes": (1_024, 1_048_576),
    "max_fixture_bytes": (1_048_576, 134_217_728),
}
MAX_DOCUMENT_BYTES = 262_144
MAX_PROBE_BYTES = 4 * 1024 * 1024
MAX_INTEROP_ROWS = 24
MAX_FIXTURES = 8
MAX_COMPONENTS = 16
MAX_MFT_ROWS = 8
MAX_ADAPTERS = 8
MAX_ARGV = 32
MAX_ARG_CHARS = 1_024
MAX_TEXT_CHARS = 256
MAX_TOKEN_CHARS = 64
MAX_PIX_FMTS = 8
MAX_GUIDS = 16
MAX_COUNTER = 1 << 20
MAX_SOURCE_TOKENS = 16
MAX_SOURCE_MATCHES = 20

_TOKEN = re.compile(r"^[a-z0-9_.:\-]{1,%d}$" % MAX_TOKEN_CHARS)
_IDENTIFIER = re.compile(r"^[A-Za-z0-9_.:\-]{1,%d}$" % MAX_TOKEN_CHARS)
_GUID = re.compile(
    r"^\{?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\}?$"
)
_HEX32 = re.compile(r"^0x[0-9A-F]{8}$")
_FEATURE_LEVEL = re.compile(r"^0x[0-9A-F]{1,4}$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_ABSOLUTE = re.compile(r"^[A-Za-z]:[\\/]")


class Refusal(Exception):
    """A closed refusal: ``code`` is one of :data:`REFUSAL_CODES`, ``detail`` is a hint."""

    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code
        self.detail = detail


REFUSAL_CODES = frozenset(
    {
        "CAPS_OUT_OF_RANGE",
        "CAPS_MISMATCH",
        "PROBE_NOT_ABSOLUTE",
        "PROBE_NOT_FOUND",
        "PROBE_UNREADABLE",
        "PROBE_CHANGED",
        "PROBE_SPAWN_FAILED",
        "PROBE_TIMEOUT",
        "PROBE_EXIT_NONZERO",
        "PROBE_OUTPUT_EMPTY",
        "PROBE_OUTPUT_OVERSIZED",
        "PROBE_STDERR_OVERSIZED",
        "PROBE_JSON_INVALID",
        "PROBE_REFUSED",
        "PROBE_DOCUMENT_INVALID",
        "OUT_NOT_ABSOLUTE",
    }
)


# ---------------------------------------------------------------------------
# Small validation helpers (closed schema; every failure names the field)
# ---------------------------------------------------------------------------
def _fail(field: str, message: str) -> "None":
    raise Refusal("PROBE_DOCUMENT_INVALID", f"{field}: {message}")


def _object(value: object, field: str, keys: tuple[str, ...]) -> dict:
    if not isinstance(value, dict):
        _fail(field, "expected an object")
    unknown = set(value) - set(keys)
    missing = set(keys) - set(value)
    if unknown:
        _fail(field, f"unexpected key(s) {sorted(unknown)}")
    if missing:
        _fail(field, f"missing key(s) {sorted(missing)}")
    return value


def _text(value: object, field: str, *, maximum: int = MAX_TEXT_CHARS, empty: bool = True) -> str:
    if not isinstance(value, str):
        _fail(field, "expected a string")
    if len(value) > maximum:
        _fail(field, "string too long")
    if not empty and not value:
        _fail(field, "string must not be empty")
    return value


def _token(value: object, field: str) -> str:
    text = _text(value, field, maximum=MAX_TOKEN_CHARS, empty=False)
    if not _TOKEN.match(text):
        _fail(field, "not a lowercase token")
    return text


def _bool(value: object, field: str) -> bool:
    if not isinstance(value, bool):
        _fail(field, "expected a boolean")
    return value


def _int(value: object, field: str, *, minimum: int = 0, maximum: int = MAX_COUNTER) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        _fail(field, "expected an integer")
    if not minimum <= value <= maximum:
        _fail(field, f"out of range {minimum}..{maximum}")
    return value


def _optional_int(
    value: object, field: str, *, minimum: int = -(1 << 40), maximum: int = MAX_COUNTER
) -> int | None:
    if value is None:
        return None
    return _int(value, field, minimum=minimum, maximum=maximum)


def _list(value: object, field: str, *, maximum: int) -> list:
    if not isinstance(value, list):
        _fail(field, "expected an array")
    if len(value) > maximum:
        _fail(field, f"more than {maximum} entries")
    return value


def _status_reason(
    value: dict, field: str, *, reason: str = "reason", status: str = "status"
) -> tuple[str, str]:
    state = _text(value[status], f"{field}.{status}", maximum=32, empty=False)
    if state not in STATUSES:
        _fail(f"{field}.{status}", f"unregistered status {state!r}")
    why = _text(value[reason], f"{field}.{reason}", maximum=MAX_TOKEN_CHARS)
    if state == STATUS_OBSERVED:
        if why:
            _fail(f"{field}.{reason}", "an OBSERVED entry must carry no reason")
    elif not why:
        _fail(f"{field}.{reason}", "a non-OBSERVED entry must carry a reason")
    elif why not in REASONS:
        _fail(f"{field}.{reason}", f"unregistered reason {why!r}")
    return state, why


def _absolute_text(value: object, field: str) -> str:
    text = _text(value, field, maximum=MAX_ARG_CHARS, empty=False)
    if not os.path.isabs(text) or not _ABSOLUTE.match(text):
        _fail(field, "expected an absolute local path")
    return text


# ---------------------------------------------------------------------------
# Probe invocation with bounded capture
# ---------------------------------------------------------------------------
def _absolute_local(path_text: str, field: str, *, code: str = "PROBE_NOT_ABSOLUTE") -> Path:
    """Absolute local-disk path only: no relative form, no PATH lookup, no `..`."""
    if not path_text or not os.path.isabs(path_text) or not _ABSOLUTE.match(path_text):
        raise Refusal(code, f"{field}={path_text!r}")
    parts = re.split(r"[\\/]", path_text)
    if ".." in parts:
        raise Refusal(code, f"{field}={path_text!r}")
    return Path(path_text)


def _sha256_of(path: Path, *, cap: int = MAX_PROBE_BYTES) -> str:
    digest = hashlib.sha256()
    total = 0
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            total += len(block)
            if total > cap:
                raise Refusal("PROBE_UNREADABLE", f"{path} exceeds {cap} bytes")
            digest.update(block)
    return digest.hexdigest()


def _terminate(process: subprocess.Popen) -> None:
    """Terminate the direct child and reap it; safe to call more than once."""
    try:
        if process.poll() is None:
            process.kill()
    except OSError:
        pass
    try:
        process.wait(timeout=5)
    except (subprocess.TimeoutExpired, OSError):
        pass


def run_probe(
    *,
    probe: str,
    powershell: str,
    ffmpeg: str | None,
    scratch: str | None,
    source_root: str | None,
    git: str | None,
    keep_scratch: bool,
    caps: dict[str, int],
) -> dict:
    """Run the probe once with bounded capture and return the validated document."""
    script = _absolute_local(probe, "--probe")
    interpreter = _absolute_local(powershell, "--powershell")
    if not script.is_file():
        raise Refusal("PROBE_NOT_FOUND", str(script))
    if not interpreter.is_file():
        raise Refusal("PROBE_NOT_FOUND", str(interpreter))
    before = _sha256_of(script)
    argv = [
        str(interpreter),
        "-NoProfile",
        "-NonInteractive",
        "-File",
        str(script),
        "-FfmpegPath",
        ffmpeg or "",
        "-ScratchRoot",
        scratch or "",
        "-SourceRoot",
        source_root or "",
        "-GitPath",
        git or "",
        "-TimeoutMs",
        str(caps["timeout_ms"]),
        "-MaxOutputBytes",
        str(caps["max_output_bytes"]),
        "-MaxFixtureBytes",
        str(caps["max_fixture_bytes"]),
    ]
    if keep_scratch:
        argv.append("-KeepScratch")
    cap = caps["max_output_bytes"]
    outer_seconds = (caps["timeout_ms"] * 20 + 60_000) / 1_000
    try:
        process = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as error:
        raise Refusal("PROBE_SPAWN_FAILED", str(error)) from error
    buffers: dict[str, bytearray] = {"stdout": bytearray(), "stderr": bytearray()}
    overflow: list[str | None] = [None]

    def drain(name: str, stream) -> None:
        while True:
            block = stream.read(4096)
            if not block:
                return
            if overflow[0] is not None:
                return
            room = cap - len(buffers[name])
            if len(block) > room:
                buffers[name].extend(block[:room])
                overflow[0] = name
                _terminate(process)
                return
            buffers[name].extend(block)

    threads = [
        threading.Thread(target=drain, args=(name, stream), daemon=True)
        for name, stream in (("stdout", process.stdout), ("stderr", process.stderr))
    ]
    for thread in threads:
        thread.start()
    try:
        process.wait(timeout=outer_seconds)
    except subprocess.TimeoutExpired as error:
        _terminate(process)
        for thread in threads:
            thread.join(2)
        raise Refusal("PROBE_TIMEOUT", f"probe exceeded {outer_seconds:.1f}s") from error
    for thread in threads:
        thread.join(5)
    if overflow[0] == "stdout":
        raise Refusal("PROBE_OUTPUT_OVERSIZED", f"stdout exceeded {cap} bytes; the child was terminated")
    if overflow[0] == "stderr":
        raise Refusal("PROBE_STDERR_OVERSIZED", f"stderr exceeded {cap} bytes; the child was terminated")
    stdout = bytes(buffers["stdout"])
    after = _sha256_of(script)
    if after != before:
        raise Refusal("PROBE_CHANGED", "the probe script changed while it ran")
    if process.returncode != 0:
        raise Refusal("PROBE_EXIT_NONZERO", f"exit code {process.returncode}")
    if not stdout.strip():
        raise Refusal("PROBE_OUTPUT_EMPTY", "the probe printed no document")
    try:
        document = json.loads(stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal("PROBE_JSON_INVALID", str(error)[:200]) from error
    if not isinstance(document, dict):
        raise Refusal("PROBE_JSON_INVALID", "the document is not an object")
    if document.get("ok") is False:
        code = document.get("error")
        if code not in PROBE_ERRORS:
            raise Refusal("PROBE_REFUSED", f"unregistered probe error {code!r}")
        raise Refusal("PROBE_REFUSED", str(code))
    validate_document(document, caps=caps)
    return {
        "document": document,
        "probe_path": str(script),
        "probe_sha256": before,
        "powershell": str(interpreter),
        "exit_code": process.returncode,
        "stdout_bytes": len(stdout),
        "stderr_bytes": len(buffers["stderr"]),
    }


# ---------------------------------------------------------------------------
# Closed-schema validation of the probe document
# ---------------------------------------------------------------------------
TOP_LEVEL_KEYS = (
    "ok",
    "probe_version",
    "caps",
    "mft_flags",
    "host",
    "media_foundation",
    "d3d11",
    "tool",
    "fixtures",
    "interop",
    "source_scan",
    "pin_api_inspection",
    "route",
    "endpoints",
    "corrections",
)


def validate_document(document: dict, *, caps: dict[str, int]) -> None:
    _object(document, "document", TOP_LEVEL_KEYS)
    if document["ok"] is not True:
        _fail("ok", "expected true")
    version = _int(document["probe_version"], "probe_version")
    if version != PROBE_VERSION:
        _fail("probe_version", f"expected {PROBE_VERSION}")
    reported = _object(document["caps"], "caps", tuple(caps) + ("max_experiments",))
    for key, expected in caps.items():
        low, high = CAP_RANGES[key]
        value = _int(reported[key], f"caps.{key}", minimum=low, maximum=high)
        if value != expected:
            raise Refusal("CAPS_MISMATCH", f"caps.{key}={value} != requested {expected}")
    experiments = _int(reported["max_experiments"], "caps.max_experiments", minimum=1, maximum=64)
    if experiments > MAX_INTEROP_ROWS:
        _fail("caps.max_experiments", "exceeds the validator cap")
    validate_mft_flags(_object(document["mft_flags"], "mft_flags", tuple(MFT_FLAGS)))
    validate_host(
        _object(
            document["host"],
            "host",
            (
                "status",
                "reason",
                "os_description",
                "arch",
                "pwsh",
                "interop_enabled",
                "scratch_retained",
                "scratch_path",
            ),
        )
    )
    validate_media_foundation(
        _object(
            document["media_foundation"],
            "media_foundation",
            ("status", "reason", "mft_guid_source", "components", "mft_enumeration"),
        )
    )
    validate_d3d11(
        _object(
            document["d3d11"],
            "d3d11",
            (
                "status",
                "reason",
                "hardware_hresult",
                "hardware_feature_level",
                "warp_hresult",
                "warp_feature_level",
                "adapters",
            ),
        )
    )
    validate_tool(_object(document["tool"], "tool", ("status", "reason", "path", "sha256", "hwaccels")))
    validate_fixtures(document["fixtures"])
    validate_interop(document["interop"])
    validate_source_scan(document["source_scan"])
    validate_pin_api_inspection(document["pin_api_inspection"])
    validate_route(document["route"])
    validate_endpoints(document["endpoints"])
    validate_corrections(document["corrections"])
    validate_route_cross_checks(document)


def validate_route_cross_checks(document: dict) -> None:
    """The route statement must be grounded in the sections it cites."""
    route = document["route"]
    scan = document["source_scan"]
    inspection = document["pin_api_inspection"]
    if route["scan_signal"] != scan["signal"]:
        _fail("route.scan_signal", "does not match source_scan.signal")
    if route["pin_api_inspection"] != inspection["status"]:
        _fail("route.pin_api_inspection", "does not match pin_api_inspection.status")
    if route["status"] != "CONFIRMED_LIMITATION":
        return
    hardware = [
        row
        for row in document["interop"]
        if row["kind"] in {"hw_decode", "hw_download"}
        and row["status"] == STATUS_OBSERVED
        and row["hardware_frame_evidence"] == "PIX_FMT_OBSERVED"
    ]
    if not hardware:
        _fail("route.decode_side_observed", "a confirmed limitation requires an observed hardware decode row")
    if scan["signal"] != "COMPLETE_BOUND" or scan["git_tree_clean"] is not True:
        _fail("route.scan_signal", "a confirmed limitation requires a fully bound, complete scan")
    if inspection["status"] != "OBSERVED":
        _fail("route.pin_api_inspection", "a confirmed limitation requires an inspection of the pinned API")


def validate_mft_flags(flags: dict) -> None:
    """The flag values must be the SDK ones, so an async count can never look like hardware."""
    for key, expected in MFT_FLAGS.items():
        value = _text(flags[key], f"mft_flags.{key}", maximum=8, empty=False)
        if value != expected:
            _fail(f"mft_flags.{key}", f"expected {expected} (Windows SDK mfapi.h), got {value}")


def validate_host(host: dict) -> None:
    _status_reason(host, "host")
    _text(host["os_description"], "host.os_description")
    _text(host["arch"], "host.arch", maximum=32)
    _text(host["pwsh"], "host.pwsh", maximum=64)
    _bool(host["interop_enabled"], "host.interop_enabled")
    retained = _bool(host["scratch_retained"], "host.scratch_retained")
    if retained:
        _absolute_text(host["scratch_path"], "host.scratch_path")
    elif host["scratch_path"]:
        _fail("host.scratch_path", "a deleted scratch must not report a path")


def validate_media_foundation(section: dict) -> None:
    _status_reason(section, "media_foundation")
    _text(section["mft_guid_source"], "media_foundation.mft_guid_source")
    for index, component in enumerate(
        _list(section["components"], "media_foundation.components", maximum=MAX_COMPONENTS)
    ):
        field = f"media_foundation.components[{index}]"
        row = _object(component, field, ("name", "status", "reason", "version"))
        name = _text(row["name"], f"{field}.name", maximum=64, empty=False)
        if name.lower() not in COMPONENT_NAMES:
            _fail(f"{field}.name", f"unregistered component {name!r}")
        _status_reason(row, field)
        _text(row["version"], f"{field}.version", maximum=128)
    for index, row in enumerate(
        _list(section["mft_enumeration"], "media_foundation.mft_enumeration", maximum=MAX_MFT_ROWS)
    ):
        field = f"media_foundation.mft_enumeration[{index}]"
        entry = _object(
            row,
            field,
            (
                "category",
                "guid",
                "hresult",
                "all",
                "sync",
                "async",
                "hardware",
                "sync_status",
                "async_status",
                "hardware_status",
                "reason",
            ),
        )
        category = _text(entry["category"], f"{field}.category", maximum=32, empty=False)
        if category not in MFT_CATEGORIES:
            _fail(f"{field}.category", f"unregistered category {category!r}")
        if not _GUID.match(_text(entry["guid"], f"{field}.guid", maximum=40)):
            _fail(f"{field}.guid", "not a GUID")
        hresult = _text(entry["hresult"], f"{field}.hresult", maximum=10)
        if hresult and not _HEX32.match(hresult):
            _fail(f"{field}.hresult", "expected 0x + 8 hex digits or empty")
        for key in ("all", "sync", "async", "hardware"):
            _optional_int(entry[key], f"{field}.{key}", maximum=4096)
        for key in ("sync_status", "async_status", "hardware_status"):
            state = _text(entry[key], f"{field}.{key}", maximum=32, empty=False)
            if state not in STATUSES:
                _fail(f"{field}.{key}", f"unregistered status {state!r}")
        why = _text(entry["reason"], f"{field}.reason", maximum=MAX_TOKEN_CHARS)
        if why and why not in REASONS:
            _fail(f"{field}.reason", f"unregistered reason {why!r}")
        for key in ("sync", "async", "hardware"):
            count = entry[key]
            status_key = f"{key}_status"
            if entry[status_key] == STATUS_OBSERVED and count is None:
                _fail(f"{field}.{key}", "an OBSERVED count must not be null")
            if entry[status_key] != STATUS_OBSERVED and count is not None:
                _fail(f"{field}.{key}", "a NOT_MEASURED count must not carry a value")
        if entry["all"] is None and not why:
            _fail(f"{field}.reason", "a failed enumeration must carry a reason")


def validate_d3d11(section: dict) -> None:
    _status_reason(section, "d3d11")
    for key in ("hardware_hresult", "warp_hresult"):
        value = _text(section[key], f"d3d11.{key}", maximum=10)
        if value and not _HEX32.match(value):
            _fail(f"d3d11.{key}", "expected 0x + 8 hex digits or empty")
    for key in ("hardware_feature_level", "warp_feature_level"):
        value = _text(section[key], f"d3d11.{key}", maximum=8)
        if value and not _FEATURE_LEVEL.match(value):
            _fail(f"d3d11.{key}", "expected 0x + hex digits or empty")
    for index, adapter in enumerate(_list(section["adapters"], "d3d11.adapters", maximum=MAX_ADAPTERS)):
        field = f"d3d11.adapters[{index}]"
        row = _object(adapter, field, ("name", "driver_version"))
        _text(row["name"], f"{field}.name", empty=False)
        _text(row["driver_version"], f"{field}.driver_version", maximum=64)


def validate_tool(section: dict) -> None:
    state, _why = _status_reason(section, "tool")
    path = _text(section["path"], "tool.path")
    sha = _text(section["sha256"], "tool.sha256", maximum=64)
    hwaccels = _list(section["hwaccels"], "tool.hwaccels", maximum=16)
    for index, item in enumerate(hwaccels):
        _token(item, f"tool.hwaccels[{index}]")
    if state == STATUS_OBSERVED:
        if not path:
            _fail("tool.path", "an OBSERVED tool must carry its absolute path")
        if not os.path.isabs(path) or not _ABSOLUTE.match(path):
            _fail("tool.path", "not an absolute local path")
        if not _SHA256.match(sha):
            _fail("tool.sha256", "expected 64 lowercase hex digits")
    else:
        if path or sha:
            _fail("tool", "a tool that was not observed must not report a path or digest")


def validate_fixtures(fixtures: list) -> None:
    seen = set()
    for index, fixture in enumerate(_list(fixtures, "fixtures", maximum=MAX_FIXTURES)):
        field = f"fixtures[{index}]"
        row = _object(
            fixture,
            field,
            (
                "id",
                "container",
                "codec",
                "pixel_format",
                "status",
                "reason",
                "bytes",
                "sha256",
                "width",
                "height",
                "seconds",
                "generator_argv",
            ),
        )
        identifier = _text(row["id"], f"{field}.id", maximum=16, empty=False)
        if identifier not in FIXTURE_IDS:
            _fail(f"{field}.id", f"unregistered fixture {identifier!r}")
        if identifier in seen:
            _fail(f"{field}.id", "duplicate fixture")
        seen.add(identifier)
        _token(row["container"], f"{field}.container")
        _token(row["codec"], f"{field}.codec")
        _token(row["pixel_format"], f"{field}.pixel_format")
        state, _why = _status_reason(row, field)
        _optional_int(row["bytes"], f"{field}.bytes", maximum=134_217_728)
        _text(row["sha256"], f"{field}.sha256", maximum=64)
        _int(row["width"], f"{field}.width", minimum=1, maximum=16_384)
        _int(row["height"], f"{field}.height", minimum=1, maximum=16_384)
        _int(row["seconds"], f"{field}.seconds", minimum=1, maximum=10)
        argv = _list(row["generator_argv"], f"{field}.generator_argv", maximum=MAX_ARGV)
        for position, argument in enumerate(argv):
            _text(argument, f"{field}.generator_argv[{position}]", maximum=MAX_ARG_CHARS)
        if state == STATUS_OBSERVED:
            if row["bytes"] is None:
                _fail(f"{field}.bytes", "an OBSERVED fixture must carry its size")
            if not _SHA256.match(row["sha256"]):
                _fail(f"{field}.sha256", "an OBSERVED fixture must carry its SHA-256")
            if not argv:
                _fail(f"{field}.generator_argv", "an OBSERVED fixture must carry its generator argv")
            if not _ABSOLUTE.match(argv[0]):
                _fail(f"{field}.generator_argv[0]", "the generator must be an absolute tool path")
        elif row["sha256"]:
            _fail(f"{field}.sha256", "a fixture that was not observed must not report a digest")


def validate_interop(rows: list) -> None:
    seen = set()
    for index, row in enumerate(_list(rows, "interop", maximum=MAX_INTEROP_ROWS)):
        field = f"interop[{index}]"
        entry = _object(
            row,
            field,
            (
                "id",
                "kind",
                "container",
                "codec",
                "pixel_format",
                "hwaccel",
                "argv",
                "exit_code",
                "wall_ms",
                "status",
                "reason",
                "stdout_bytes",
                "stderr_bytes",
                "capped",
                "frames_decoded",
                "decoder_pix_fmts",
                "hardware_pix_fmt",
                "hardware_frame_evidence",
                "dxva_profile_guids",
                "output_bytes",
                "expected_output_bytes",
            ),
        )
        identifier = _token(entry["id"], f"{field}.id")
        if identifier in seen:
            _fail(f"{field}.id", "duplicate interop id")
        seen.add(identifier)
        kind = _text(entry["kind"], f"{field}.kind", maximum=32, empty=False)
        if kind not in KINDS:
            _fail(f"{field}.kind", f"unregistered kind {kind!r}")
        for key in ("container", "codec", "pixel_format", "hwaccel"):
            value = _text(entry[key], f"{field}.{key}", maximum=MAX_TOKEN_CHARS)
            if value and not _TOKEN.match(value):
                _fail(f"{field}.{key}", "not a lowercase token")
        argv = _list(entry["argv"], f"{field}.argv", maximum=MAX_ARGV)
        for position, argument in enumerate(argv):
            _text(argument, f"{field}.argv[{position}]", maximum=MAX_ARG_CHARS)
        exit_code = _optional_int(
            entry["exit_code"], f"{field}.exit_code", minimum=-(1 << 31), maximum=(1 << 31) - 1
        )
        _optional_int(entry["wall_ms"], f"{field}.wall_ms", maximum=600_000)
        state, why = _status_reason(entry, field)
        _int(entry["stdout_bytes"], f"{field}.stdout_bytes")
        _int(entry["stderr_bytes"], f"{field}.stderr_bytes")
        _bool(entry["capped"], f"{field}.capped")
        frames = _optional_int(entry["frames_decoded"], f"{field}.frames_decoded", maximum=1_000_000)
        fmts = _list(entry["decoder_pix_fmts"], f"{field}.decoder_pix_fmts", maximum=MAX_PIX_FMTS)
        for position, fmt in enumerate(fmts):
            _token(fmt, f"{field}.decoder_pix_fmts[{position}]")
        hardware_fmt = _text(entry["hardware_pix_fmt"], f"{field}.hardware_pix_fmt", maximum=32)
        if hardware_fmt and hardware_fmt not in HARDWARE_PIX_FMTS:
            _fail(f"{field}.hardware_pix_fmt", f"unregistered hardware format {hardware_fmt!r}")
        evidence = _text(entry["hardware_frame_evidence"], f"{field}.hardware_frame_evidence", maximum=32)
        if evidence not in EVIDENCE_CLASSES:
            _fail(f"{field}.hardware_frame_evidence", f"unregistered evidence class {evidence!r}")
        guids = _list(entry["dxva_profile_guids"], f"{field}.dxva_profile_guids", maximum=MAX_GUIDS)
        for position, guid in enumerate(guids):
            if not _GUID.match(_text(guid, f"{field}.dxva_profile_guids[{position}]", maximum=40)):
                _fail(f"{field}.dxva_profile_guids[{position}]", "not a GUID")
        output_bytes = _optional_int(entry["output_bytes"], f"{field}.output_bytes", maximum=134_217_728)
        expected_bytes = _optional_int(
            entry["expected_output_bytes"], f"{field}.expected_output_bytes", maximum=134_217_728
        )

        # Evidence consistency for every row, observed or not.
        if evidence == "PIX_FMT_OBSERVED":
            if hardware_fmt not in HARDWARE_PIX_FMTS:
                _fail(f"{field}.hardware_pix_fmt", "PIX_FMT_OBSERVED requires a hardware pix_fmt")
            if hardware_fmt not in fmts:
                _fail(f"{field}.decoder_pix_fmts", "the hardware pix_fmt must appear in the decoder log")
        if hardware_fmt and hardware_fmt not in fmts:
            _fail(f"{field}.decoder_pix_fmts", "the hardware pix_fmt must appear in the decoder log")
        if evidence == "NONE" and hardware_fmt:
            _fail(f"{field}.hardware_pix_fmt", "a row without evidence must not name a hardware pix_fmt")

        if state != STATUS_OBSERVED:
            continue
        # OBSERVED invariants: an observed run exited 0, carried its command and produced
        # real evidence. A DXVA GUID list is never evidence.
        if not argv:
            _fail(f"{field}.argv", "an OBSERVED row must record the command that established it")
        if not _ABSOLUTE.match(argv[0]):
            _fail(f"{field}.argv[0]", "an OBSERVED row must name an absolute local tool path")
        if not entry["pixel_format"]:
            _fail(f"{field}.pixel_format", "an OBSERVED row must name the media pixel format")
        if exit_code != 0:
            _fail(f"{field}.exit_code", "an OBSERVED row must have exit code 0")
        if entry["capped"]:
            _fail(f"{field}.capped", "an OBSERVED row must not have been capped")
        if kind in {"decode", "hw_decode", "hw_download"} and (frames is None or frames < 1):
            _fail(f"{field}.frames_decoded", "an OBSERVED decode must report decoded frames")
        if kind in {"hw_decode", "hw_download"} and evidence != "PIX_FMT_OBSERVED":
            _fail(
                f"{field}.hardware_frame_evidence",
                "an OBSERVED hardware row requires hardware frames observed in the decoder log",
            )
        if kind in {"raw_output", "hw_download"}:
            if output_bytes is None or expected_bytes is None:
                _fail(f"{field}.output_bytes", "an OBSERVED raw output must carry its byte counts")
            if output_bytes != expected_bytes:
                _fail(f"{field}.output_bytes", "raw output size does not match the declared expectation")


def validate_source_scan(section: object) -> None:
    scan = _object(
        section,
        "source_scan",
        (
            "status",
            "reason",
            "root",
            "git_head_sha",
            "git_tree_clean",
            "git_status_reason",
            "git_path",
            "file_cap",
            "file_size_cap",
            "files_scanned",
            "files_skipped_size",
            "cap_hit",
            "bytes_scanned",
            "tokens",
            "matches",
            "signal",
        ),
    )
    state, _why = _status_reason(scan, "source_scan")
    _text(scan["root"], "source_scan.root")
    sha = _text(scan["git_head_sha"], "source_scan.git_head_sha", maximum=64)
    if sha and not re.match(r"^[0-9a-f]{40}([0-9a-f]{24})?$", sha):
        _fail("source_scan.git_head_sha", "expected a lowercase commit SHA or empty")
    clean = scan["git_tree_clean"]
    if clean is not None:
        _bool(clean, "source_scan.git_tree_clean")
    git_reason = _text(scan["git_status_reason"], "source_scan.git_status_reason", maximum=MAX_TOKEN_CHARS)
    if git_reason and git_reason not in REASONS:
        _fail("source_scan.git_status_reason", f"unregistered reason {git_reason!r}")
    if sha and clean is None and not git_reason:
        _fail("source_scan.git_status_reason", "an unmeasured tree state must carry a reason")
    _text(scan["git_path"], "source_scan.git_path")
    _int(scan["file_cap"], "source_scan.file_cap", minimum=1, maximum=100_000)
    _int(scan["file_size_cap"], "source_scan.file_size_cap", minimum=1, maximum=1 << 40)
    _int(scan["files_scanned"], "source_scan.files_scanned", maximum=100_000)
    _int(scan["files_skipped_size"], "source_scan.files_skipped_size", maximum=100_000)
    _bool(scan["cap_hit"], "source_scan.cap_hit")
    _int(scan["bytes_scanned"], "source_scan.bytes_scanned", maximum=1 << 34)
    signal = _text(scan["signal"], "source_scan.signal", maximum=32, empty=False)
    if signal not in SCAN_SIGNALS:
        _fail("source_scan.signal", f"unregistered scan signal {signal!r}")
    tokens = _list(scan["tokens"], "source_scan.tokens", maximum=MAX_SOURCE_TOKENS)
    for index, token in enumerate(tokens):
        value = _text(token, f"source_scan.tokens[{index}]", maximum=MAX_TOKEN_CHARS, empty=False)
        if not _IDENTIFIER.match(value):
            _fail(f"source_scan.tokens[{index}]", "not a code identifier")
    matches = _list(scan["matches"], "source_scan.matches", maximum=MAX_SOURCE_MATCHES)
    for index, match in enumerate(matches):
        field = f"source_scan.matches[{index}]"
        row = _object(match, field, ("file", "line", "token"))
        _text(row["file"], f"{field}.file", maximum=512, empty=False)
        _int(row["line"], f"{field}.line", minimum=1, maximum=10_000_000)
        token = _text(row["token"], f"{field}.token", maximum=MAX_TOKEN_CHARS, empty=False)
        if token not in tokens:
            _fail(f"{field}.token", "matched token is not in the declared token list")

    # The signal must match the binding it claims, so a partial scan can never look whole.
    if signal == "NOT_RUN":
        if state == STATUS_OBSERVED:
            _fail("source_scan.signal", "NOT_RUN conflicts with OBSERVED")
        if matches:
            _fail("source_scan.matches", "a scan that did not run cannot report matches")
    else:
        if state != STATUS_OBSERVED:
            _fail("source_scan.status", f"signal {signal} requires an OBSERVED scan")
        if scan["files_scanned"] < 1:
            _fail("source_scan.files_scanned", "an observed scan must have scanned files")
        if signal == "UNBOUND":
            if sha:
                _fail("source_scan.signal", "UNBOUND conflicts with a recorded commit SHA")
        elif not sha:
            _fail("source_scan.git_head_sha", f"signal {signal} requires a commit binding")
        if signal == "TRUNCATED" and not (scan["cap_hit"] or scan["files_skipped_size"] > 0):
            _fail("source_scan.signal", "TRUNCATED requires a cap hit or skipped files")
        if signal in {"COMPLETE_BOUND", "SHA_BOUND_TREE_STATE_UNKNOWN"} and (
            scan["cap_hit"] or scan["files_skipped_size"] > 0
        ):
            _fail("source_scan.signal", f"{signal} conflicts with a truncated scan")
        if signal == "COMPLETE_BOUND":
            if clean is not True:
                _fail("source_scan.git_tree_clean", "COMPLETE_BOUND requires a clean tree")
        if signal == "SHA_BOUND_TREE_STATE_UNKNOWN" and clean is True:
            _fail("source_scan.git_tree_clean", "a clean tree is COMPLETE_BOUND, not unknown")


def validate_pin_api_inspection(section: object) -> None:
    inspection = _object(section, "pin_api_inspection", ("status", "reason", "pin", "api_surface", "findings"))
    state = _text(inspection["status"], "pin_api_inspection.status", maximum=32, empty=False)
    if state not in INSPECTION_STATUSES:
        _fail("pin_api_inspection.status", f"unregistered status {state!r}")
    why = _text(inspection["reason"], "pin_api_inspection.reason", maximum=MAX_TOKEN_CHARS)
    pin = _text(inspection["pin"], "pin_api_inspection.pin", maximum=128)
    surface = _text(inspection["api_surface"], "pin_api_inspection.api_surface", maximum=128)
    findings = _list(inspection["findings"], "pin_api_inspection.findings", maximum=16)
    for index, finding in enumerate(findings):
        _text(finding, f"pin_api_inspection.findings[{index}]", maximum=512, empty=False)
    if state == "NOT_RUN":
        if why not in REASONS:
            _fail("pin_api_inspection.reason", "a NOT_RUN inspection must carry a registered reason")
        if findings or pin or surface:
            _fail("pin_api_inspection", "an inspection that did not run cannot report findings")
    else:
        if why:
            _fail("pin_api_inspection.reason", "an OBSERVED inspection must carry no reason")
        if not pin or not surface or not findings:
            _fail("pin_api_inspection", "an OBSERVED inspection must name the pin, the API surface and findings")


def validate_route(section: object) -> None:
    route = _object(
        section,
        "route",
        (
            "route_id",
            "status",
            "reason",
            "decode_side_observed",
            "scan_signal",
            "scan_usable_as_support",
            "pin_api_inspection",
            "grounds",
            "missing_evidence",
        ),
    )
    route_id = _text(route["route_id"], "route.route_id", maximum=MAX_TOKEN_CHARS, empty=False)
    if not _IDENTIFIER.match(route_id):
        _fail("route.route_id", "not a bounded identifier")
    state = _text(route["status"], "route.status", maximum=32, empty=False)
    if state not in ROUTE_STATUSES:
        _fail("route.status", f"unregistered route status {state!r}")
    why = _text(route["reason"], "route.reason", maximum=MAX_TOKEN_CHARS)
    if state == "UNEXAMINED":
        if why not in REASONS:
            _fail("route.reason", "an UNEXAMINED route must carry a registered reason")
    elif why:
        _fail("route.reason", "a concluded route must carry no reason")
    decode_side = _bool(route["decode_side_observed"], "route.decode_side_observed")
    signal = _text(route["scan_signal"], "route.scan_signal", maximum=32, empty=False)
    if signal not in SCAN_SIGNALS:
        _fail("route.scan_signal", f"unregistered scan signal {signal!r}")
    usable = _bool(route["scan_usable_as_support"], "route.scan_usable_as_support")
    if usable != (signal == "COMPLETE_BOUND"):
        _fail("route.scan_usable_as_support", "only a COMPLETE_BOUND scan is usable as support")
    inspection = _text(route["pin_api_inspection"], "route.pin_api_inspection", maximum=32, empty=False)
    if inspection not in INSPECTION_STATUSES:
        _fail("route.pin_api_inspection", f"unregistered status {inspection!r}")
    grounds = _list(route["grounds"], "route.grounds", maximum=8)
    for index, ground in enumerate(grounds):
        field = f"route.grounds[{index}]"
        row = _object(ground, field, ("id", "status", "detail"))
        _token(row["id"], f"{field}.id")
        ground_state = _text(row["status"], f"{field}.status", maximum=32, empty=False)
        if ground_state not in GROUND_STATUSES:
            _fail(f"{field}.status", f"unregistered ground status {ground_state!r}")
        _text(row["detail"], f"{field}.detail", maximum=512, empty=False)
    missing = _list(route["missing_evidence"], "route.missing_evidence", maximum=12)
    for index, item in enumerate(missing):
        value = _text(item, f"route.missing_evidence[{index}]", maximum=MAX_TOKEN_CHARS, empty=False)
        if value not in MISSING_EVIDENCE:
            _fail(f"route.missing_evidence[{index}]", f"unregistered missing-evidence token {value!r}")
    if state == "CONFIRMED_LIMITATION":
        # A scan alone can never conclude a route: the pinned API itself must have been
        # inspected, the scan must be fully bound and complete, and the decode side observed.
        if not decode_side:
            _fail("route.decode_side_observed", "a confirmed limitation requires an observed decode side")
        if signal != "COMPLETE_BOUND" or not usable:
            _fail("route.scan_signal", "a confirmed limitation requires a COMPLETE_BOUND scan")
        if inspection != "OBSERVED":
            _fail("route.pin_api_inspection", "a confirmed limitation requires an inspection of the pinned API")
        if not grounds:
            _fail("route.grounds", "a confirmed limitation requires recorded grounds")
        if not missing:
            _fail("route.missing_evidence", "a confirmed limitation must still name the missing evidence")
    else:
        if not missing:
            _fail("route.missing_evidence", "an UNEXAMINED route must name what a route decision still needs")


def validate_endpoints(endpoints: list) -> None:
    seen = set()
    for index, endpoint in enumerate(_list(endpoints, "endpoints", maximum=16)):
        field = f"endpoints[{index}]"
        row = _object(endpoint, field, ("endpoint", "status", "reason"))
        name = _text(row["endpoint"], f"{field}.endpoint", maximum=32, empty=False)
        if name not in UNOBSERVABLE_ENDPOINTS:
            _fail(f"{field}.endpoint", f"unregistered endpoint {name!r}")
        if name in seen:
            _fail(f"{field}.endpoint", "duplicate endpoint")
        seen.add(name)
        state, why = _status_reason(row, field)
        if state == STATUS_OBSERVED:
            _fail(field, "this probe cannot observe these endpoints")
        if why != UNOBSERVABLE_ENDPOINTS[name]:
            _fail(f"{field}.reason", f"expected {UNOBSERVABLE_ENDPOINTS[name]}")
    missing = set(UNOBSERVABLE_ENDPOINTS) - seen
    if missing:
        _fail("endpoints", f"missing endpoint(s) {sorted(missing)}")


def validate_corrections(corrections: list) -> None:
    rows = _list(corrections, "corrections", maximum=8)
    if not rows:
        _fail("corrections", "the pre-fix observation must be preserved")
    for index, correction in enumerate(rows):
        field = f"corrections[{index}]"
        row = _object(correction, field, ("id", "status", "detail"))
        _token(row["id"], f"{field}.id")
        state = _text(row["status"], f"{field}.status", maximum=32, empty=False)
        if state not in CORRECTION_STATUSES:
            _fail(f"{field}.status", f"unregistered correction status {state!r}")
        _text(row["detail"], f"{field}.detail", maximum=1_024, empty=False)


# ---------------------------------------------------------------------------
# Record construction (derived facts only; no route selection)
# ---------------------------------------------------------------------------
def build_record(*, provenance: dict, document: dict, caps: dict[str, int]) -> dict:
    interop = document["interop"]
    fixtures = document["fixtures"]
    endpoints = document["endpoints"]
    route = document["route"]

    def count(rows: list, state: str) -> int:
        return sum(1 for row in rows if row["status"] == state)

    observed_paths: list[str] = []
    unavailable: list[str] = []
    for row in interop:
        if row["status"] == STATUS_OBSERVED:
            observed_paths.append(
                f"{row['kind']}:{row['id']} codec={row['codec']} hwaccel={row['hwaccel']}"
                f" hardware_pix_fmt={row['hardware_pix_fmt'] or '-'} frames={row['frames_decoded']}"
                f" wall_ms={row['wall_ms']}"
            )
        else:
            unavailable.append(f"{row['kind']}:{row['id']} {row['status']} {row['reason']}")
    for endpoint in endpoints:
        unavailable.append(f"endpoint:{endpoint['endpoint']} {endpoint['status']} {endpoint['reason']}")

    hardware_observed = [
        row
        for row in interop
        if row["kind"] in {"hw_decode", "hw_download"} and row["status"] == STATUS_OBSERVED
    ]
    tradeoffs = [
        "tool-side wall times include process startup and are not product playback latency or presentation latency",
        "no video route is selected or recommended by this record; the choice belongs to MP-VID2",
        "copy counts, download costs and synchronisation stalls into a presentation path are uninstrumented, so no zero-copy claim follows",
        "colour metadata, HDR/presentation semantics, device loss and decoder reset were not exercised",
    ]
    if hardware_observed:
        tradeoffs.append(
            "hardware frames were observed in the decoder log for "
            + ", ".join(sorted({row["codec"] for row in hardware_observed}))
            + "; importing those frames into the pinned GPUI revision remains unverified"
        )
    if any(row["status"] == "UNSUPPORTED" for row in interop):
        tradeoffs.append(
            "at least one requested hardware path fell back to software or failed; the exact reason is recorded per row"
        )
    tradeoffs.append(
        "the route statement is UNEXAMINED: the pinned GPUI presentation API was not inspected, so the scan is "
        "supporting material with its commit binding, caps and skipped files stated, not a conclusion"
    )

    return {
        "record_version": RECORD_VERSION,
        "task": TASK,
        "generated_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "probe": provenance,
        "caps": document["caps"],
        "mft_flags": document["mft_flags"],
        "host": document["host"],
        "media_foundation": document["media_foundation"],
        "d3d11": document["d3d11"],
        "tool": document["tool"],
        "fixtures": fixtures,
        "interop": interop,
        "source_scan": document["source_scan"],
        "pin_api_inspection": document["pin_api_inspection"],
        "route": route,
        "endpoints": endpoints,
        "corrections": document["corrections"],
        "summary": {
            "fixtures": len(fixtures),
            "fixtures_observed": count(fixtures, STATUS_OBSERVED),
            "interop_rows": len(interop),
            "interop_observed": count(interop, STATUS_OBSERVED),
            "interop_not_measured": count(interop, "NOT_MEASURED"),
            "interop_unsupported": count(interop, "UNSUPPORTED"),
            "endpoints_not_measured": count(endpoints, "NOT_MEASURED"),
            "hardware_decode_observed": len(
                [row for row in interop if row["kind"] == "hw_decode" and row["status"] == STATUS_OBSERVED]
            ),
            "hardware_download_observed": len(
                [row for row in interop if row["kind"] == "hw_download" and row["status"] == STATUS_OBSERVED]
            ),
            "route_status": route["status"],
            "route_reason": route["reason"],
            "scan_signal": document["source_scan"]["signal"],
            "scan_usable_as_support": route["scan_usable_as_support"],
            "source_files_scanned": document["source_scan"]["files_scanned"],
            "source_matches": len(document["source_scan"]["matches"]),
        },
        "vid2_material": {
            "observed_paths": observed_paths,
            "unavailable": unavailable,
            "tradeoffs": tradeoffs,
        },
        "caps_requested": caps,
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def parse_caps(values: dict[str, int]) -> dict[str, int]:
    for key, (low, high) in CAP_RANGES.items():
        value = values[key]
        if not isinstance(value, int) or isinstance(value, bool) or not low <= value <= high:
            raise Refusal("CAPS_OUT_OF_RANGE", f"{key}={value!r} outside {low}..{high}")
    return dict(values)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="vid1_capability.py",
        description="Run the MP-VID1 video capability probe and write its bounded record.",
    )
    parser.add_argument("--probe", required=True, help="absolute path of vid1_probe.ps1")
    parser.add_argument("--powershell", required=True, help="absolute path of pwsh.exe (no PATH search)")
    parser.add_argument("--ffmpeg", default="", help="absolute path of the experiment-only FFmpeg")
    parser.add_argument("--scratch", default="", help="absolute scratch directory for bounded interop runs")
    parser.add_argument(
        "--source-root", default="", help="absolute path of the checkout scanned for a presentation call site"
    )
    parser.add_argument("--git", default="", help="absolute path of git.exe used to bind the scan to a clean tree")
    parser.add_argument("--keep-scratch", action="store_true", help="keep the probe's scratch directory and its fixtures")
    parser.add_argument("--out", default="", help="absolute record path (prints to stdout when omitted)")
    parser.add_argument("--timeout-ms", type=int, default=15_000)
    parser.add_argument("--max-output-bytes", type=int, default=MAX_DOCUMENT_BYTES)
    parser.add_argument("--max-fixture-bytes", type=int, default=67_108_864)
    args = parser.parse_args(argv)
    try:
        caps = parse_caps(
            {
                "timeout_ms": args.timeout_ms,
                "max_output_bytes": args.max_output_bytes,
                "max_fixture_bytes": args.max_fixture_bytes,
            }
        )
        provenance = run_probe(
            probe=args.probe,
            powershell=args.powershell,
            ffmpeg=args.ffmpeg or None,
            scratch=args.scratch or None,
            source_root=args.source_root or None,
            git=args.git or None,
            keep_scratch=args.keep_scratch,
            caps=caps,
        )
        record = build_record(
            provenance={key: value for key, value in provenance.items() if key != "document"},
            document=provenance["document"],
            caps=caps,
        )
        payload = json.dumps(record, indent=2, ensure_ascii=False) + "\n"
        if len(payload.encode("utf-8")) > 4 * MAX_DOCUMENT_BYTES:
            raise Refusal("PROBE_OUTPUT_OVERSIZED", "record exceeds the write cap")
        if args.out:
            destination = _absolute_local(args.out, "--out", code="OUT_NOT_ABSOLUTE")
            if not destination.parent.is_dir():
                raise Refusal("OUT_NOT_ABSOLUTE", f"{destination} has no existing parent")
            destination.write_text(payload, encoding="utf-8")
            print(
                f"record={destination} bytes={len(payload.encode('utf-8'))} "
                f"route={record['route']['status']} "
                f"interop={record['summary']['interop_rows']} "
                f"observed={record['summary']['interop_observed']} "
                f"unsupported={record['summary']['interop_unsupported']} "
                f"scan_files={record['summary']['source_files_scanned']} "
                f"scan_matches={record['summary']['source_matches']}"
            )
        else:
            sys.stdout.write(payload)
        return 0
    except Refusal as refusal:
        if refusal.code not in REFUSAL_CODES:
            raise AssertionError(f"unregistered refusal code {refusal.code!r}")
        print(f"REFUSED {refusal.code}" + (f" {refusal.detail}" if refusal.detail else ""), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
