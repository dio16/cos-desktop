"""MP-R0C-S1 workload manifest, deterministic trace and result contracts.

Development-only, Python standard library only. This module validates versioned
workload definitions, generates a deterministic planned action trace from a fixed
seed, and validates bounded result records. It performs no launch, no measurement
and executes no value from a manifest.

The v1 input set is the implemented shell only: keyboard, held-key, pointer,
scroll, posted-message saturation and their keyboard/scroll+projection
combinations. Popup interaction and entity-update bursts have no implemented
injection path in this build and are refused as NOT_IMPLEMENTED rather than
reported as run.
"""

from __future__ import annotations

import hashlib
import json
import re
from typing import Any

import contracts

MANIFEST_VERSION = "r0c-workload-1"
TRACE_VERSION = "r0c-trace-1"
RESULT_VERSION = "r0c-result-1"
MAX_MANIFEST_BYTES = 64 * 1024
MAX_TRACE_BYTES = 2 * 1024 * 1024
MAX_RESULT_BYTES = 2 * 1024 * 1024
MAX_ACTIONS = 100_000
MAX_DURATION_MS = 300_000
MIN_DURATION_MS = 500
MAX_HZ = 200.0
MIN_HZ = 0.1
MAX_BURST = 64
MAX_REPEATS = 16

FAMILIES = (
    "keyboard",
    "held-key",
    "pointer",
    "scroll",
    "posted-saturation",
    "combined",
)
# Declared but not implemented in this build. A manifest that selects one is
# refused, and a result that claims it was run is invalid.
NOT_IMPLEMENTED_FAMILIES = ("popup", "entity-update")
ACTION_KINDS = ("keyboard", "held-key", "pointer", "scroll", "posted-saturation")

KEYS = ("TAB", "ENTER", "SPACE", "LEFT", "RIGHT", "UP", "DOWN", "ESCAPE")

# Journal endpoints the S2B instrumentation implements at the pin. Whether one was
# actually observed in a run is decided from retained records, never assumed.
JOURNAL_KINDS = {
    "input-dispatch": 0,
    "gpui-dirty": 1,
    "gpui-cpu-draw": 2,
    "gpui-platform-submission": 3,
    "local-surface-mutation": 8,
    "local-notify": 9,
}
ENDPOINTS_OBSERVED = tuple(JOURNAL_KINDS)
# Everything else has no probe in this build. The reason is a closed code.
ENDPOINTS_NOT_MEASURED = {
    "frame-request": "NO_ENDPOINT: app-side frame request has no probe at the pin",
    "frame-processing-started": "NO_ENDPOINT: no frame-processing-start source at the pin",
    "present-call": "NO_ENDPOINT: present-call is app-side only at the pin",
    "present-return": "NO_ENDPOINT: present-return is app-side only at the pin",
    "present": "NO_DWM_ENDPOINT: no independent present/displayed probe",
    "displayed": "NO_DWM_ENDPOINT: no independent displayed-frame probe",
    "scanout": "NO_DISPLAY_DRIVER_PROBE: scanout is not observable on this host",
    "input-to-photon": "NO_END_TO_END_ENDPOINT: no OS input-to-photon probe",
    "gpu-submission": "NO_GPU_TIMESTAMP_QUERY: no GPU/D3D probe on this host",
    "gpu-execution": "NO_GPU_TIMESTAMP_QUERY: no GPU timestamp query on this host",
    "dwm-composition": "NO_DWM_ENDPOINT: DWM composition timing is not queried",
}
MAX_LAG_MS = 250
CLOSE_TIMEOUT_MS = 5_000

_DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
_SHA = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}\Z")
_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}\Z")


class ContractError(Exception):
    """A bounded contract failure with a stable code and JSON path."""

    def __init__(self, code: str, path: str):
        super().__init__(code)
        self.code = code
        self.path = path

    def as_dict(self) -> dict[str, str]:
        return {"code": self.code, "path": self.path}


def _fail(code: str, path: str) -> None:
    raise ContractError(code, path)


def _object(value: Any, path: str, required: tuple[str, ...], optional: tuple[str, ...] = ()) -> dict[str, Any]:
    if not isinstance(value, dict):
        _fail("INVALID_TYPE", path)
    keys = set(value)
    if not set(required) <= keys:
        _fail("MISSING_FIELD", path)
    if keys - set(required) - set(optional):
        _fail("UNKNOWN_FIELD", path)
    return value


def _array(value: Any, path: str, *, nonempty: bool = False, maximum: int = MAX_ACTIONS) -> list[Any]:
    if not isinstance(value, list):
        _fail("INVALID_TYPE", path)
    if nonempty and not value:
        _fail("EMPTY_ARRAY", path)
    if len(value) > maximum:
        _fail("OUT_OF_RANGE", path)
    return value


def _string(value: Any, path: str, *, pattern: re.Pattern[str] | None = None, maximum: int = 512) -> str:
    if not isinstance(value, str) or len(value) > maximum:
        _fail("INVALID_TYPE", path)
    if pattern is not None and not pattern.fullmatch(value):
        _fail("INVALID_VALUE", path)
    return value


def _int(value: Any, path: str, minimum: int = 0, maximum: int | None = None) -> int:
    if type(value) is not int or value < minimum or (maximum is not None and value > maximum):
        _fail("OUT_OF_RANGE", path)
    return value


def _number(value: Any, path: str, minimum: float, maximum: float) -> float:
    if type(value) not in (int, float) or not (minimum <= float(value) <= maximum):
        _fail("OUT_OF_RANGE", path)
    return float(value)


def _digest(value: Any, path: str) -> str:
    if not isinstance(value, str) or not _DIGEST.fullmatch(value):
        _fail("INVALID_DIGEST", path)
    return value


def _sha(value: Any, path: str) -> str:
    if not isinstance(value, str) or not _SHA.fullmatch(value):
        _fail("INVALID_VALUE", path)
    return value


def _enum(value: Any, path: str, allowed: tuple[str, ...]) -> str:
    if not isinstance(value, str) or value not in allowed:
        _fail("INVALID_ENUM", path)
    return value


def _boolean(value: Any, path: str) -> bool:
    if type(value) is not bool:
        _fail("INVALID_TYPE", path)
    return value


def canonical(document: Any) -> str:
    return json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def digest_of(document: Any) -> str:
    return "sha256:" + hashlib.sha256(canonical(document).encode("ascii")).hexdigest()


def load_manifest(path: str) -> dict[str, Any]:
    return validate_manifest(contracts.load_document(path, MAX_MANIFEST_BYTES))


def validate_manifest(document: Any) -> dict[str, Any]:
    manifest = _object(
        document,
        "$",
        (
            "manifest_version",
            "scenario",
            "identity",
            "window",
            "actions",
            "lifecycle",
            "expected",
            "endpoints",
        ),
    )
    if manifest["manifest_version"] != MANIFEST_VERSION:
        _fail("INVALID_VERSION", "$.manifest_version")

    scenario = _object(
        manifest["scenario"],
        "$.scenario",
        (
            "id",
            "version",
            "family",
            "seed",
            "fixture_sha256",
            "duration_ms",
            "warmup_ms",
            "active_start_ms",
            "active_end_ms",
            "repeats",
            "order",
        ),
    )
    _string(scenario["id"], "$.scenario.id", pattern=_ID)
    _int(scenario["version"], "$.scenario.version", 1, 1_000)
    family = _enum(scenario["family"], "$.scenario.family", FAMILIES + NOT_IMPLEMENTED_FAMILIES)
    if family in NOT_IMPLEMENTED_FAMILIES:
        _fail("NOT_IMPLEMENTED", "$.scenario.family")
    _int(scenario["seed"], "$.scenario.seed", 0, (1 << 32) - 1)
    _digest(scenario["fixture_sha256"], "$.scenario.fixture_sha256")
    duration = _int(scenario["duration_ms"], "$.scenario.duration_ms", MIN_DURATION_MS, MAX_DURATION_MS)
    warmup = _int(scenario["warmup_ms"], "$.scenario.warmup_ms", 0, duration)
    start = _int(scenario["active_start_ms"], "$.scenario.active_start_ms", warmup, duration)
    end = _int(scenario["active_end_ms"], "$.scenario.active_end_ms", start, duration)
    if end <= start:
        _fail("OUT_OF_RANGE", "$.scenario.active_end_ms")
    repeats = _int(scenario["repeats"], "$.scenario.repeats", 1, MAX_REPEATS)
    if repeats != 1:
        # S1 executes one pass; a multi-pass contract needs its own trace/capture
        # alignment before it can be accepted.
        _fail("NOT_IMPLEMENTED", "$.scenario.repeats")
    order = _array(scenario["order"], "$.scenario.order", nonempty=True, maximum=MAX_REPEATS)
    for index, item in enumerate(order):
        _enum(item, f"$.scenario.order[{index}]", FAMILIES + NOT_IMPLEMENTED_FAMILIES)

    identity = _object(
        manifest["identity"],
        "$.identity",
        ("repository_sha", "executable_sha256", "helper_sha256", "gpui_revision", "build_profile", "features"),
    )
    _sha(identity["repository_sha"], "$.identity.repository_sha")
    _digest(identity["executable_sha256"], "$.identity.executable_sha256")
    _digest(identity["helper_sha256"], "$.identity.helper_sha256")
    _sha(identity["gpui_revision"], "$.identity.gpui_revision")
    _enum(identity["build_profile"], "$.identity.build_profile", ("dev", "release"))
    features = _array(identity["features"], "$.identity.features", maximum=32)
    for index, feature in enumerate(features):
        _string(feature, f"$.identity.features[{index}]", pattern=_ID)

    window = _object(manifest["window"], "$.window", ("generation", "title"))
    _int(window["generation"], "$.window.generation", 1, 1_000_000)
    if window["title"] != "COS Desktop":
        _fail("INVALID_VALUE", "$.window.title")

    actions = _array(manifest["actions"], "$.actions", nonempty=True, maximum=32)
    for index, action in enumerate(actions):
        _validate_action(action, f"$.actions[{index}]")
    kinds: list[str] = []
    for action in actions:
        if action["kind"] not in kinds:
            kinds.append(action["kind"])
    if order != kinds:
        # The declared execution order must be exactly the action-kind order the
        # planner emits; an unexecuted order entry is not accepted.
        _fail("INVALID_VALUE", "$.scenario.order")
    if sum(action["count"] for action in actions) > MAX_ACTIONS:
        _fail("OUT_OF_RANGE", "$.actions")
    if family == "combined":
        if len(kinds) < 2:
            _fail("INVALID_VALUE", "$.scenario.family")
    elif kinds != [family]:
        # A single-family scenario must declare exactly its family's actions.
        _fail("INVALID_VALUE", "$.scenario.family")

    lifecycle = _object(
        manifest["lifecycle"],
        "$.lifecycle",
        ("close", "timeout_ms", "recovery", "cancel"),
    )
    _enum(lifecycle["close"], "$.lifecycle.close", ("wm_close",))
    _int(lifecycle["timeout_ms"], "$.lifecycle.timeout_ms", 1_000, MAX_DURATION_MS)
    _enum(lifecycle["recovery"], "$.lifecycle.recovery", ("none", "close"))
    _enum(lifecycle["cancel"], "$.lifecycle.cancel", ("immediate",))

    expected = _object(manifest["expected"], "$.expected", ("functional_state", "notes"))
    _array(expected["functional_state"], "$.expected.functional_state", nonempty=True, maximum=32)
    for index, item in enumerate(expected["functional_state"]):
        _string(item, f"$.expected.functional_state[{index}]")
    _array(expected["notes"], "$.expected.notes", maximum=32)

    endpoints = _object(manifest["endpoints"], "$.endpoints", ("observed", "not_measured"))
    observed = _array(endpoints["observed"], "$.endpoints.observed", maximum=32)
    for index, name in enumerate(observed):
        _enum(name, f"$.endpoints.observed[{index}]", ENDPOINTS_OBSERVED)
    if set(observed) != set(ENDPOINTS_OBSERVED):
        _fail("ENDPOINT_DECLARATION_INCOMPLETE", "$.endpoints.observed")
    missing = _array(endpoints["not_measured"], "$.endpoints.not_measured", maximum=32)
    for index, item in enumerate(missing):
        entry = _object(item, f"$.endpoints.not_measured[{index}]", ("name", "reason"))
        _enum(entry["name"], f"$.endpoints.not_measured[{index}].name", tuple(ENDPOINTS_NOT_MEASURED))
        _string(entry["reason"], f"$.endpoints.not_measured[{index}].reason")

    return manifest


def _validate_action(action: Any, path: str) -> None:
    entry = _object(
        action,
        path,
        ("kind", "requested_hz", "burst", "count"),
        ("keys", "button", "delta"),
    )
    kind = _enum(entry["kind"], f"{path}.kind", ACTION_KINDS)
    if kind in NOT_IMPLEMENTED_FAMILIES:
        _fail("NOT_IMPLEMENTED", f"{path}.kind")
    _number(entry["requested_hz"], f"{path}.requested_hz", MIN_HZ, MAX_HZ)
    burst = _int(entry["burst"], f"{path}.burst", 1, MAX_BURST)
    if burst != 1:
        # S1 has no burst execution effect; accepting another value would be a
        # manifest that claims traffic it does not generate.
        _fail("NOT_IMPLEMENTED", f"{path}.burst")
    _int(entry["count"], f"{path}.count", 1, MAX_ACTIONS)
    if kind in ("keyboard", "held-key"):
        keys = _array(entry.get("keys"), f"{path}.keys", nonempty=True, maximum=len(KEYS))
        for index, key in enumerate(keys):
            _enum(key, f"{path}.keys[{index}]", KEYS)
    elif kind == "pointer":
        _enum(entry.get("button", "left"), f"{path}.button", ("left",))
    elif kind == "scroll":
        _int(entry.get("delta", 120), f"{path}.delta", -1_200, 1_200)


def _splitmix64(seed: int, sequence: int) -> int:
    """Deterministic PRNG independent of the Python version."""
    value = (seed & 0xFFFFFFFFFFFFFFFF) + (sequence * 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF
    value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
    value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
    return (value ^ (value >> 31)) & 0xFFFFFFFFFFFFFFFF


def plan_trace(manifest: dict[str, Any]) -> dict[str, Any]:
    """Generate the deterministic planned action trace from the manifest seed."""
    scenario = manifest["scenario"]
    seed = scenario["seed"]
    start_ns = scenario["active_start_ms"] * 1_000_000
    actions: list[dict[str, Any]] = []
    sequence = 0
    counts: dict[str, int] = {}
    for action in manifest["actions"]:
        kind = action["kind"]
        interval_ns = max(1, int(round(1_000_000_000 / action["requested_hz"])))
        for index in range(action["count"]):
            noise = _splitmix64(seed, sequence) % (max(1, interval_ns // 8) + 1)
            planned_ns = start_ns + index * interval_ns + noise
            payload = _payload(kind, action, seed, sequence)
            actions.append(
                {
                    "sequence": sequence,
                    "kind": kind,
                    "planned_ns": planned_ns,
                    "requested_hz": action["requested_hz"],
                    "burst_index": index % action["burst"],
                    "payload": payload,
                }
            )
            sequence += 1
            counts[kind] = counts.get(kind, 0) + 1
    actions.sort(key=lambda item: (item["planned_ns"], item["sequence"]))
    end_ns = scenario["active_end_ms"] * 1_000_000
    if actions and actions[-1]["planned_ns"] > end_ns:
        _fail("OUT_OF_RANGE", "$.actions")
    trace = {
        "trace_version": TRACE_VERSION,
        "scenario": {
            "id": scenario["id"],
            "version": scenario["version"],
            "family": scenario["family"],
            "seed": seed,
            "fixture_sha256": scenario["fixture_sha256"],
            "active_start_ms": scenario["active_start_ms"],
            "active_end_ms": scenario["active_end_ms"],
        },
        "counts": counts,
        "actions": actions,
    }
    trace["trace_sha256"] = digest_of(trace)
    return trace


def _payload(kind: str, action: dict[str, Any], seed: int, sequence: int) -> dict[str, Any]:
    if kind in ("keyboard", "held-key"):
        keys = action["keys"]
        return {"key": keys[_splitmix64(seed, sequence + 1) % len(keys)]}
    if kind == "pointer":
        return {
            "x": 80 + (_splitmix64(seed, sequence + 2) % 800),
            "y": 80 + (_splitmix64(seed, sequence + 3) % 480),
            "button": action.get("button", "left"),
        }
    if kind == "scroll":
        delta = action.get("delta", 120)
        return {"delta": delta if sequence % 2 == 0 else -delta}
    return {}


def validate_trace(trace: Any, manifest: dict[str, Any]) -> dict[str, Any]:
    document = _object(trace, "$", ("trace_version", "scenario", "counts", "actions", "trace_sha256"))
    if len(canonical(document).encode("ascii")) > MAX_TRACE_BYTES:
        _fail("OUT_OF_RANGE", "$")
    if document["trace_version"] != TRACE_VERSION:
        _fail("INVALID_VERSION", "$.trace_version")
    scenario = _object(document["scenario"], "$.scenario", ("id", "version", "family", "seed", "fixture_sha256", "active_start_ms", "active_end_ms"))
    expected = manifest["scenario"]
    for key in ("id", "version", "family", "seed", "fixture_sha256"):
        if scenario[key] != expected[key]:
            _fail("TRACE_MISMATCH", f"$.scenario.{key}")
    actions = _array(document["actions"], "$.actions", nonempty=True, maximum=MAX_ACTIONS)
    for index, item in enumerate(actions):
        entry = _object(item, f"$.actions[{index}]", ("sequence", "kind", "planned_ns", "requested_hz", "burst_index", "payload"))
        _int(entry["sequence"], f"$.actions[{index}].sequence", 0, MAX_ACTIONS)
        _enum(entry["kind"], f"$.actions[{index}].kind", ACTION_KINDS)
        _int(entry["planned_ns"], f"$.actions[{index}].planned_ns", 0, MAX_DURATION_MS * 1_000_000)
        if not (scenario["active_start_ms"] * 1_000_000 <= entry["planned_ns"] <= scenario["active_end_ms"] * 1_000_000):
            _fail("OUT_OF_RANGE", f"$.actions[{index}].planned_ns")
    digest = _digest(document["trace_sha256"], "$.trace_sha256")
    body = {key: value for key, value in document.items() if key != "trace_sha256"}
    if digest != digest_of(body):
        _fail("TRACE_MISMATCH", "$.trace_sha256")
    return document


def classify(
    planned_ns: int,
    actual_ns: int | None,
    delivered: bool,
    *,
    interval_ns: int,
    cancelled: bool = False,
    suppressed: bool = False,
    max_lag_ns: int = MAX_LAG_MS * 1_000_000,
) -> str:
    """Closed classification for one planned action."""
    if cancelled:
        return "cancelled"
    if suppressed:
        return "suppressed"
    if not delivered or actual_ns is None:
        return "failed"
    late = actual_ns - planned_ns
    if late < 0:
        return "early"
    if late <= interval_ns:
        return "on-time"
    if late <= max_lag_ns:
        return "late"
    return "dropped"


def endpoint_matrix(records: dict[int, int] | None = None) -> list[dict]:
    """Endpoint availability: implemented and observed are separate decisions.

    A journal endpoint is OBSERVED only when the verified capture actually retained
    records of that kind in this run; an implemented endpoint with no retained record
    is NOT_MEASURED, never a zero and never an observation.
    """
    counts = records or {}
    endpoints: list[dict] = []
    for name, kind in JOURNAL_KINDS.items():
        count = int(counts.get(kind, 0))
        if count > 0:
            endpoints.append({"name": name, "status": "OBSERVED", "records": count})
        else:
            endpoints.append(
                {"name": name, "status": "NOT_MEASURED", "reason": "NO_RETAINED_VALID_RECORD"}
            )
    endpoints.extend(
        {"name": name, "status": "NOT_MEASURED", "reason": reason}
        for name, reason in ENDPOINTS_NOT_MEASURED.items()
    )
    return endpoints


def validate_result(document: Any) -> dict[str, Any]:
    result = _object(
        document,
        "$",
        (
            "result_version",
            "scenario",
            "trace_sha256",
            "execution_id",
            "requested_cadence",
            "observed_cadence",
            "cadence_note",
            "actions",
            "journal",
            "endpoints",
            "cleanup",
            "limits",
            "delivered_actions",
            "probe_identity",
            "time_correspondence",
            "status",
        ),
        ("reason",),
    )
    if result["result_version"] != RESULT_VERSION:
        _fail("INVALID_VERSION", "$.result_version")
    scenario = _object(result["scenario"], "$.scenario", ("id", "version", "family", "seed"))
    _string(scenario["id"], "$.scenario.id", pattern=_ID)
    _int(scenario["version"], "$.scenario.version", 1, 1_000)
    _enum(scenario["family"], "$.scenario.family", FAMILIES)
    _int(scenario["seed"], "$.scenario.seed", 0, (1 << 32) - 1)
    _digest(result["trace_sha256"], "$.trace_sha256")
    _string(result["execution_id"], "$.execution_id", pattern=re.compile(r"[0-9a-f]{32}\Z"))
    requested = result["requested_cadence"]
    observed = result["observed_cadence"]
    if not isinstance(requested, dict) or not isinstance(observed, dict):
        _fail("INVALID_TYPE", "$.requested_cadence")
    if set(requested) != set(observed.get("posting_attempt_hz", {})):
        _fail("INVALID_VALUE", "$.observed_cadence.posting_attempt_hz")
    for kind in requested:
        _enum(kind, "$.requested_cadence", ACTION_KINDS)
        _number(requested[kind], f"$.requested_cadence.{kind}", 0.0, MAX_HZ)
    for name in ("posting_attempt_hz", "postmessage_success_hz"):
        values = observed.get(name)
        if not isinstance(values, dict) or set(values) != set(requested):
            _fail("INVALID_VALUE", f"$.observed_cadence.{name}")
        for kind, value in values.items():
            if value is not None:
                _number(value, f"$.observed_cadence.{name}.{kind}", 0.0, MAX_HZ)
    application = observed.get("application_processing_hz")
    if application is not None:
        _number(application, "$.observed_cadence.application_processing_hz", 0.0, MAX_HZ)
    _string(result["cadence_note"], "$.cadence_note")
    delivered = _int(result["delivered_actions"], "$.delivered_actions", 0, MAX_ACTIONS)
    actions = _array(result["actions"], "$.actions", nonempty=True, maximum=MAX_ACTIONS)
    for index, item in enumerate(actions):
        entry = _object(
            item,
            f"$.actions[{index}]",
            ("sequence", "kind", "planned_ns", "actual_ns", "attempted", "delivered",
             "suppressed", "classification"),
            optional=("excluded_reason",),
        )
        _int(entry["sequence"], f"$.actions[{index}].sequence", 0, MAX_ACTIONS)
        _enum(entry["kind"], f"$.actions[{index}].kind", ACTION_KINDS)
        _int(entry["planned_ns"], f"$.actions[{index}].planned_ns", 0, MAX_DURATION_MS * 1_000_000)
        if entry["actual_ns"] is not None:
            _int(entry["actual_ns"], f"$.actions[{index}].actual_ns", 0, 2 * MAX_DURATION_MS * 1_000_000)
        _boolean(entry["attempted"], f"$.actions[{index}].attempted")
        _boolean(entry["delivered"], f"$.actions[{index}].delivered")
        _boolean(entry["suppressed"], f"$.actions[{index}].suppressed")
        _enum(
            entry["classification"],
            f"$.actions[{index}].classification",
            # The v1 probe can produce only these classes. A class it cannot emit
            # is not accepted in a result.
            ("on-time", "early", "late", "suppressed", "failed"),
        )
        if "excluded_reason" in entry:
            _string(entry["excluded_reason"], f"$.actions[{index}].excluded_reason")
    journal = _object(
        result["journal"], "$.journal", ("source", "counts", "note"),
        optional=("capture_sha256", "observation_sha256"),
    )
    if "capture_sha256" in journal:
        _digest(journal["capture_sha256"], "$.journal.capture_sha256")
    if "observation_sha256" in journal:
        _digest(journal["observation_sha256"], "$.journal.observation_sha256")
    _enum(journal["source"], "$.journal.source", ("s2b-journal", "none"))
    counts = journal["counts"]
    if not isinstance(counts, dict):
        _fail("INVALID_TYPE", "$.journal.counts")
    for key, value in counts.items():
        if not isinstance(key, str) or type(value) is not int or value < 0:
            _fail("INVALID_TYPE", "$.journal.counts")
    _string(journal["note"], "$.journal.note")
    endpoints = _array(result["endpoints"], "$.endpoints", nonempty=True, maximum=64)
    seen: set[str] = set()
    for index, item in enumerate(endpoints):
        entry = _object(item, f"$.endpoints[{index}]", ("name", "status"), optional=("reason", "records"))
        name = _string(entry["name"], f"$.endpoints[{index}].name")
        status = _enum(entry["status"], f"$.endpoints[{index}].status", ("OBSERVED", "NOT_MEASURED"))
        if status == "OBSERVED":
            if name not in JOURNAL_KINDS:
                _fail("PROMOTED_ENDPOINT", f"$.endpoints[{index}].status")
            if _int(entry.get("records", 0), f"$.endpoints[{index}].records", 1, MAX_ACTIONS) < 1:
                _fail("PROMOTED_ENDPOINT", f"$.endpoints[{index}].records")
        if name in seen:
            _fail("DUPLICATE_ENDPOINT", f"$.endpoints[{index}].name")
        seen.add(name)
        if status == "NOT_MEASURED" and "reason" not in entry:
            _fail("MISSING_FIELD", f"$.endpoints[{index}].reason")
        if status == "OBSERVED" and name in ENDPOINTS_NOT_MEASURED:
            _fail("PROMOTED_ENDPOINT", f"$.endpoints[{index}].status")
    cleanup = _object(
        result["cleanup"], "$.cleanup",
        ("wrapper_exit", "child_reaped", "close_posted", "window_gone", "timed_out", "cancelled"),
    )
    _int(cleanup["wrapper_exit"], "$.cleanup.wrapper_exit", -1, 255)
    for key in ("child_reaped", "close_posted", "window_gone", "timed_out", "cancelled"):
        _boolean(cleanup[key], f"$.cleanup.{key}")
    limits = _object(result["limits"], "$.limits", ("max_actions", "max_lag_ms", "probe_timeout_ms", "capture_timeout_ms", "output_bytes"))
    for key in ("max_actions", "max_lag_ms", "probe_timeout_ms", "capture_timeout_ms", "output_bytes"):
        _int(limits[key], f"$.limits.{key}", 0, MAX_ACTIONS * 1024)
    probe_identity = _object(
        result["probe_identity"], "$.probe_identity",
        ("generation", "window_pid", "launcher_pid", "helper_pid"),
    )
    for key in ("generation", "window_pid", "launcher_pid", "helper_pid"):
        value = probe_identity[key]
        if value is not None:
            _int(value, f"$.probe_identity.{key}", 1, 1 << 31)
    correspondence = _object(
        result["time_correspondence"],
        "$.time_correspondence",
        ("method", "capture_lifetime_inside", "journal_range",
         "input_start_wall_ns", "input_end_wall_ns",
         "capture_start_wall_ns", "capture_end_wall_ns", "scope_ns"),
        ("scope_source", "journal_clock_mapping", "journal_range_reason"),
    )
    _enum(correspondence["method"], "$.time_correspondence.method",
          ("wall-clock-capture-lifetime",))
    _boolean(correspondence["capture_lifetime_inside"],
             "$.time_correspondence.capture_lifetime_inside")
    _enum(correspondence["journal_range"], "$.time_correspondence.journal_range",
          ("UNKNOWN", "PROVEN", "MISMATCH"))
    if correspondence["journal_range"] == "UNKNOWN" and "journal_range_reason" not in correspondence:
        _fail("MISSING_FIELD", "$.time_correspondence.journal_range_reason")
    for key in ("input_start_wall_ns", "input_end_wall_ns",
                "capture_start_wall_ns", "capture_end_wall_ns", "scope_ns"):
        _int(correspondence[key], f"$.time_correspondence.{key}", 0, 1 << 62)
    if "scope_source" in correspondence:
        _enum(correspondence["scope_source"], "$.time_correspondence.scope_source",
              ("s2b-manifest-duration",))
    if "journal_clock_mapping" in correspondence:
        _enum(correspondence["journal_clock_mapping"], "$.time_correspondence.journal_clock_mapping",
              ("NONE",))
    status = _enum(result["status"], "$.status", ("COMPLETE", "FAILED", "CANCELLED", "NOT_RUN"))
    if status == "COMPLETE":
        if delivered != len(actions):
            _fail("INVALID_VALUE", "$.delivered_actions")
        if (
            cleanup["wrapper_exit"] != 0
            or cleanup["child_reaped"] is not True
            or cleanup["close_posted"] is not True
            or cleanup["window_gone"] is not True
            or cleanup["timed_out"] is not False
            or cleanup["cancelled"] is not False
        ):
            _fail("CLEANUP_UNCONFIRMED", "$.cleanup")
        if any(probe_identity[key] is None for key in
               ("generation", "window_pid", "launcher_pid", "helper_pid")):
            _fail("PROBE_IDENTITY_INCOMPLETE", "$.probe_identity")
    if "reason" in result:
        _string(result["reason"], "$.reason")
    return result


def check_identity(manifest: dict[str, Any], result: dict[str, Any]) -> None:
    scenario = manifest["scenario"]
    for key in ("id", "version", "family", "seed"):
        if result["scenario"][key] != scenario[key]:
            _fail("IDENTITY_MISMATCH", f"$.scenario.{key}")


def write_trace(trace: dict[str, Any], path: str) -> None:
    text = json.dumps(trace, sort_keys=True, indent=2) + "\n"
    if len(text.encode("utf-8")) > MAX_TRACE_BYTES:
        _fail("OUT_OF_RANGE", "$")
    with open(path, "x", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def result_bytes(document: dict[str, Any]) -> int:
    """Encoded size of a result document, used to enforce the write cap."""
    return len((json.dumps(document, sort_keys=True, indent=2) + "\n").encode("utf-8"))
