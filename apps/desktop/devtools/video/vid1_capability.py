"""MP-VID1 video capability/interop record builder.

Runs one operator-named PowerShell capability probe (absolute path, absolute interpreter,
no PATH search), validates its single bounded JSON document against a closed schema, and
writes a bounded record. The record states what the probe observed and what it could not
observe; it never fills a gap with a guess, never turns an app-reported number into an
independent one, and never selects a video route (that is MP-VID2's owner decision).

Closed vocabulary
-----------------
Every status is ``OBSERVED``, ``NOT_MEASURED`` or ``UNSUPPORTED``. A status other than
``OBSERVED`` must carry one of :data:`REASONS`; an ``OBSERVED`` entry must carry none. The
validator also enforces the probe's honesty boundary: a hardware-decode row may only be
``OBSERVED`` when the log demonstrated hardware frames (a hardware decoder ``pix_fmt`` or
reported DXVA decoder GUIDs), and the presentation/copy/colour/device-loss endpoints may
never be ``OBSERVED`` by this probe.

Usage
-----
``vid1_capability.py --probe ABS_PS1 --powershell ABS_PWSH [--ffmpeg ABS_EXE]
[--scratch ABS_DIR] [--out ABS_JSON]``
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

PROBE_VERSION = 1
RECORD_VERSION = 1
TASK = "MP-VID1"

STATUS_OBSERVED = "OBSERVED"
STATUSES = (STATUS_OBSERVED, "NOT_MEASURED", "UNSUPPORTED")

#: Every reason token the probe may emit. Closed: an unregistered token is a refusal, not a
#: value to copy through.
REASONS = frozenset(
    """
    COLOR_METADATA_NOT_PROBED
    COPY_COUNT_NOT_INSTRUMENTED
    D3D11_DEVICE_UNAVAILABLE
    DEVICE_LOSS_NOT_EXERCISED
    DXVA_PROFILE_NAMES_NOT_RESOLVED
    FIXTURE_GENERATION_FAILED
    FIXTURE_TOO_LARGE
    GPU_SURFACE_IMPORT_UNVERIFIED
    HWACCEL_SOFTWARE_FALLBACK
    HWOUTPUT_CONVERT_FAILED
    INTEROP_DISABLED
    MF_COMPONENT_MISSING
    MF_STARTUP_FAILED
    MFT_ENUMERATION_FAILED
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

_TOKEN = re.compile(r"^[a-z0-9_.:\-]{1,%d}$" % MAX_TOKEN_CHARS)
_GUID = re.compile(
    r"^\{?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\}?$"
)
_HEX32 = re.compile(r"^0x[0-9A-F]{8}$")
_FEATURE_LEVEL = re.compile(r"^0x[0-9A-F]{1,4}$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")


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


# ---------------------------------------------------------------------------
# Probe invocation
# ---------------------------------------------------------------------------
def _absolute_local(path_text: str, field: str, *, code: str = "PROBE_NOT_ABSOLUTE") -> Path:
    """Absolute local-disk path only: no relative form, no PATH lookup, no `..`."""
    if not path_text or not os.path.isabs(path_text):
        raise Refusal(code, f"{field}={path_text!r}")
    if not re.match(r"^[A-Za-z]:[\\/]", path_text):
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


def run_probe(
    *,
    probe: str,
    powershell: str,
    ffmpeg: str | None,
    scratch: str | None,
    caps: dict[str, int],
) -> dict:
    """Run the probe once and return the validated document plus provenance."""
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
        "-TimeoutMs",
        str(caps["timeout_ms"]),
        "-MaxOutputBytes",
        str(caps["max_output_bytes"]),
        "-MaxFixtureBytes",
        str(caps["max_fixture_bytes"]),
    ]
    # The probe bounds every child by `timeout_ms`; the outer bound covers the declared
    # experiment budget (fixtures + interop rows) without ever being unbounded.
    outer_seconds = (caps["timeout_ms"] * 20 + 60_000) / 1_000
    try:
        completed = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=outer_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise Refusal("PROBE_TIMEOUT", f"{timeout_detail(error)}") from error
    except OSError as error:
        raise Refusal("PROBE_SPAWN_FAILED", str(error)) from error
    stdout = completed.stdout or b""
    after = _sha256_of(script)
    if after != before:
        raise Refusal("PROBE_CHANGED", "the probe script changed while it ran")
    if completed.returncode != 0:
        raise Refusal("PROBE_EXIT_NONZERO", f"exit code {completed.returncode}")
    if len(stdout) > caps["max_output_bytes"]:
        raise Refusal("PROBE_OUTPUT_OVERSIZED", f"{len(stdout)} > {caps['max_output_bytes']}")
    if not stdout.strip():
        raise Refusal("PROBE_OUTPUT_EMPTY", "the probe printed no document")
    try:
        document = json.loads(stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal("PROBE_JSON_INVALID", truncate(str(error), 200)) from error
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
        "exit_code": completed.returncode,
        "stdout_bytes": len(stdout),
    }


def timeout_detail(error: subprocess.TimeoutExpired) -> str:
    return f"probe exceeded {error.timeout:.1f}s"


def truncate(text: str, maximum: int) -> str:
    return text if len(text) <= maximum else text[:maximum] + "…"


# ---------------------------------------------------------------------------
# Closed-schema validation of the probe document
# ---------------------------------------------------------------------------
TOP_LEVEL_KEYS = (
    "ok",
    "probe_version",
    "caps",
    "host",
    "media_foundation",
    "d3d11",
    "tool",
    "fixtures",
    "interop",
    "endpoints",
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

    validate_host(
        _object(
            document["host"],
            "host",
            ("status", "reason", "os_description", "arch", "pwsh", "interop_enabled"),
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
    validate_endpoints(document["endpoints"])


def validate_host(host: dict) -> None:
    _status_reason(host, "host")
    _text(host["os_description"], "host.os_description")
    _text(host["arch"], "host.arch", maximum=32)
    _text(host["pwsh"], "host.pwsh", maximum=64)
    _bool(host["interop_enabled"], "host.interop_enabled")


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
            ("category", "guid", "all_status", "all", "hardware_status", "hardware", "sync", "reason", "hresult"),
        )
        category = _text(entry["category"], f"{field}.category", maximum=32, empty=False)
        if category not in MFT_CATEGORIES:
            _fail(f"{field}.category", f"unregistered category {category!r}")
        if not _GUID.match(_text(entry["guid"], f"{field}.guid", maximum=40)):
            _fail(f"{field}.guid", "not a GUID")
        for status_key in ("all_status", "hardware_status"):
            state = _text(entry[status_key], f"{field}.{status_key}", maximum=32, empty=False)
            if state not in STATUSES:
                _fail(f"{field}.{status_key}", f"unregistered status {state!r}")
        _optional_int(entry["all"], f"{field}.all", maximum=4096)
        _optional_int(entry["hardware"], f"{field}.hardware", maximum=4096)
        _optional_int(entry["sync"], f"{field}.sync", maximum=4096)
        why = _text(entry["reason"], f"{field}.reason", maximum=MAX_TOKEN_CHARS)
        if why and why not in REASONS:
            _fail(f"{field}.reason", f"unregistered reason {why!r}")
        hresult = _text(entry["hresult"], f"{field}.hresult", maximum=10)
        if hresult and not _HEX32.match(hresult):
            _fail(f"{field}.hresult", "expected 0x + 8 hex digits or empty")


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
        if not os.path.isabs(path) or not re.match(r"^[A-Za-z]:[\\/]", path):
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
            ("id", "container", "codec", "pixel_format", "status", "reason", "bytes", "width", "height", "seconds"),
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
        _int(row["width"], f"{field}.width", minimum=1, maximum=16_384)
        _int(row["height"], f"{field}.height", minimum=1, maximum=16_384)
        _int(row["seconds"], f"{field}.seconds", minimum=1, maximum=10)
        if state == STATUS_OBSERVED and row["bytes"] is None:
            _fail(f"{field}.bytes", "an OBSERVED fixture must carry its size")


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
                "truncated",
                "frames_decoded",
                "decoder_pix_fmts",
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
        exit_code = _optional_int(entry["exit_code"], f"{field}.exit_code", minimum=-(1 << 31), maximum=(1 << 31) - 1)
        _optional_int(entry["wall_ms"], f"{field}.wall_ms", maximum=600_000)
        state, why = _status_reason(entry, field)
        _int(entry["stdout_bytes"], f"{field}.stdout_bytes")
        _int(entry["stderr_bytes"], f"{field}.stderr_bytes")
        _bool(entry["truncated"], f"{field}.truncated")
        frames = _optional_int(entry["frames_decoded"], f"{field}.frames_decoded", maximum=1_000_000)
        fmts = _list(entry["decoder_pix_fmts"], f"{field}.decoder_pix_fmts", maximum=MAX_PIX_FMTS)
        for position, fmt in enumerate(fmts):
            _token(fmt, f"{field}.decoder_pix_fmts[{position}]")
        guids = _list(entry["dxva_profile_guids"], f"{field}.dxva_profile_guids", maximum=MAX_GUIDS)
        for position, guid in enumerate(guids):
            if not _GUID.match(_text(guid, f"{field}.dxva_profile_guids[{position}]", maximum=40)):
                _fail(f"{field}.dxva_profile_guids[{position}]", "not a GUID")
        output_bytes = _optional_int(entry["output_bytes"], f"{field}.output_bytes", maximum=134_217_728)
        expected_bytes = _optional_int(
            entry["expected_output_bytes"], f"{field}.expected_output_bytes", maximum=134_217_728
        )

        if state != STATUS_OBSERVED:
            if exit_code == 0 and kind in {"hw_decode", "hw_download"} and not why:
                _fail(f"{field}.reason", "a non-OBSERVED hardware row needs a reason")
            continue
        # OBSERVED invariants: an observed run exited 0, carried its command and produced
        # real evidence.
        if not argv:
            _fail(f"{field}.argv", "an OBSERVED row must record the command that established it")
        first = argv[0] if argv else ""
        if not os.path.isabs(first) or not re.match(r"^[A-Za-z]:[\\/]", first):
            _fail(f"{field}.argv[0]", "an OBSERVED row must name an absolute local tool path")
        if not entry["pixel_format"]:
            _fail(f"{field}.pixel_format", "an OBSERVED row must name the media pixel format")
        if exit_code != 0:
            _fail(f"{field}.exit_code", "an OBSERVED row must have exit code 0")
        if kind in {"decode", "hw_decode", "hw_download"} and (frames is None or frames < 1):
            _fail(f"{field}.frames_decoded", "an OBSERVED decode must report decoded frames")
        if kind == "hw_decode":
            hardware_formats = {"d3d11", "d3d11va", "dxva2_vld", "cuda", "qsv", "vaapi", "vulkan"}
            if not (any(fmt in hardware_formats for fmt in fmts) or guids):
                _fail(field, "OBSERVED hardware decode requires hardware frames or DXVA GUIDs")
        if kind in {"raw_output", "hw_download"}:
            if output_bytes is None or expected_bytes is None:
                _fail(f"{field}.output_bytes", "an OBSERVED raw output must carry its byte counts")
            if output_bytes != expected_bytes:
                _fail(f"{field}.output_bytes", "raw output size does not match the declared expectation")


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


# ---------------------------------------------------------------------------
# Record construction (derived facts only; no route selection)
# ---------------------------------------------------------------------------
def build_record(*, provenance: dict, document: dict, caps: dict[str, int]) -> dict:
    interop = document["interop"]
    fixtures = document["fixtures"]
    endpoints = document["endpoints"]

    def count(rows: list, state: str) -> int:
        return sum(1 for row in rows if row["status"] == state)

    observed_paths: list[str] = []
    unavailable: list[str] = []
    for row in interop:
        if row["status"] == STATUS_OBSERVED:
            observed_paths.append(
                f"{row['kind']}:{row['id']} codec={row['codec']} hwaccel={row['hwaccel']}"
                f" frames={row['frames_decoded']} wall_ms={row['wall_ms']}"
            )
        else:
            unavailable.append(f"{row['kind']}:{row['id']} {row['status']} {row['reason']}")
    for endpoint in endpoints:
        unavailable.append(f"endpoint:{endpoint['endpoint']} {endpoint['status']} {endpoint['reason']}")

    hardware_observed = [
        row
        for row in interop
        if row["kind"] == "hw_decode" and row["status"] == STATUS_OBSERVED
    ]
    downloads_observed = [
        row for row in interop if row["kind"] == "hw_download" and row["status"] == STATUS_OBSERVED
    ]
    tradeoffs = [
        "tool-side wall times include process startup and are not product playback latency or presentation latency",
        "no video route is selected or recommended by this record; the choice belongs to MP-VID2",
        "copy counts, download costs and synchronisation stalls into a presentation path are uninstrumented, so no zero-copy claim follows",
        "colour metadata, HDR/presentation semantics, device loss and decoder reset were not exercised",
    ]
    if hardware_observed:
        tradeoffs.append(
            "hardware decode was observed for "
            + ", ".join(sorted({row["codec"] for row in hardware_observed}))
            + "; importing those frames into the pinned GPUI revision remains unverified"
        )
    if downloads_observed:
        tradeoffs.append(
            "a single hardware-frame download was observed for "
            + ", ".join(sorted({row["codec"] for row in downloads_observed}))
            + "; the number of copies on a presentation path is not measured"
        )
    if any(row["status"] == "UNSUPPORTED" for row in interop):
        tradeoffs.append(
            "at least one requested hardware path fell back to software or failed; the exact reason is recorded per row"
        )

    return {
        "record_version": RECORD_VERSION,
        "task": TASK,
        "generated_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "probe": provenance,
        "caps": document["caps"],
        "host": document["host"],
        "media_foundation": document["media_foundation"],
        "d3d11": document["d3d11"],
        "tool": document["tool"],
        "fixtures": fixtures,
        "interop": interop,
        "endpoints": endpoints,
        "summary": {
            "fixtures": len(fixtures),
            "fixtures_observed": count(fixtures, STATUS_OBSERVED),
            "interop_rows": len(interop),
            "interop_observed": count(interop, STATUS_OBSERVED),
            "interop_not_measured": count(interop, "NOT_MEASURED"),
            "interop_unsupported": count(interop, "UNSUPPORTED"),
            "endpoints_not_measured": count(endpoints, "NOT_MEASURED"),
            "hardware_decode_observed": len(hardware_observed),
            "hardware_download_observed": len(downloads_observed),
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
            caps=caps,
        )
        record = build_record(
            provenance={key: value for key, value in provenance.items() if key != "document"},
            document=provenance["document"],
            caps=caps,
        )
        payload = json.dumps(record, indent=2, ensure_ascii=False) + "\n"
        if len(payload.encode("utf-8")) > 4 * MAX_DOCUMENT_BYTES:
            raise Refusal("PROBE_DOCUMENT_INVALID", "record exceeds the write cap")
        if args.out:
            destination = _absolute_local(args.out, "--out", code="OUT_NOT_ABSOLUTE")
            if not destination.parent.is_dir():
                raise Refusal("OUT_NOT_ABSOLUTE", f"{destination} has no existing parent")
            destination.write_text(payload, encoding="utf-8")
            print(
                f"record={destination} bytes={len(payload.encode('utf-8'))} "
                f"interop={record['summary']['interop_rows']} "
                f"observed={record['summary']['interop_observed']} "
                f"not_measured={record['summary']['interop_not_measured']} "
                f"unsupported={record['summary']['interop_unsupported']}"
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
