"""MP-R0A-S1: development-only workload-manifest and result contracts.

Python standard library only. This module validates data shape and bounded loading;
it performs no measurement, launches nothing, fetches nothing and executes no value
from an input document. Absent facts stay explicit `{"status", "reason"}` objects or
`null`, never inferred.
"""

from __future__ import annotations

import json
import math
import os
import re
from typing import Any, NoReturn

MANIFEST_VERSION = 1
RESULT_VERSION = 1
MAX_MANIFEST_BYTES = 256 * 1024
MAX_RESULT_BYTES = 1024 * 1024

UNAVAILABLE_STATUSES = ("UNAVAILABLE", "NOT_MEASURED", "NOT_IMPLEMENTED")
RUN_STATUSES = ("OBSERVED", "NOT_IMPLEMENTED", "NOT_MEASURED", "FAILED")
SURFACE_STATUSES = ("IMPLEMENTED", "NOT_IMPLEMENTED")
SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
GIT_SHA = re.compile(r"^[0-9a-f]{40}([0-9a-f]{24})?$")
SHA256_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")

ERROR_CODES = (
    "READ_FAILED",
    "TOO_LARGE",
    "INVALID_UTF8",
    "MALFORMED_JSON",
    "DUPLICATE_KEY",
    "NON_FINITE_NUMBER",
    "UNSUPPORTED_VERSION",
    "INVALID_TYPE",
    "MISSING_FIELD",
    "UNKNOWN_FIELD",
    "INVALID_VALUE",
    "IDENTITY_MISMATCH",
)


class ContractError(Exception):
    """Bounded failure list; never contains input text or private paths."""

    def __init__(self, errors: list[dict[str, str]]):
        super().__init__(errors[0]["code"] if errors else "INVALID_VALUE")
        self.errors = errors


def _error(code: str, path: str) -> dict[str, str]:
    assert code in ERROR_CODES
    return {"code": code, "path": path}


def _raise(code: str, path: str) -> NoReturn:
    raise ContractError([_error(code, path)])


def _reject_constant(_: str) -> Any:
    _raise("NON_FINITE_NUMBER", "$")


def _pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    seen: set[str] = set()
    for key, _ in pairs:
        if key in seen:
            _raise("DUPLICATE_KEY", "$")
        seen.add(key)
    return dict(pairs)


def _bounded_bytes(path: str, limit: int, error_path: str = "$") -> bytes:
    try:
        size = os.stat(path).st_size
    except OSError:
        _raise_with("READ_FAILED", error_path)
    if size > limit:
        _raise_with("TOO_LARGE", error_path)
    try:
        with open(path, "rb") as handle:
            data = handle.read(limit + 1)
    except OSError:
        _raise_with("READ_FAILED", error_path)
    if len(data) > limit:
        _raise_with("TOO_LARGE", error_path)
    return data


def _raise_with(code: str, path: str) -> NoReturn:
    raise ContractError([_error(code, path)])


def load_document(path: str, limit: int) -> Any:
    """Read bounded UTF-8 JSON with duplicate-key and non-finite rejection."""
    data = _bounded_bytes(path, limit)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        _raise_with("INVALID_UTF8", "$")
    try:
        return json.loads(text, object_pairs_hook=_pairs, parse_constant=_reject_constant)
    except ContractError:
        raise
    except (json.JSONDecodeError, RecursionError, ValueError, OverflowError):
        _raise_with("MALFORMED_JSON", "$")


def _object(value: Any, path: str, required: tuple[str, ...], optional: tuple[str, ...] = ()) -> dict[str, Any]:
    if not isinstance(value, dict):
        _raise("INVALID_TYPE", path)
    allowed = set(required) | set(optional)
    for key in value:
        if key not in allowed:
            _raise("UNKNOWN_FIELD", path)
    for key in required:
        if key not in value:
            _raise("MISSING_FIELD", f"{path}.{key}")
    return value


def _array(value: Any, path: str, *, nonempty: bool = False) -> list[Any]:
    if not isinstance(value, list):
        _raise("INVALID_TYPE", path)
    if nonempty and not value:
        _raise("INVALID_VALUE", path)
    return value


def _string(value: Any, path: str, *, pattern: re.Pattern[str] | None = None) -> str:
    if not isinstance(value, str):
        _raise("INVALID_TYPE", path)
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        _raise("INVALID_VALUE", path)
    if pattern is not None and not pattern.fullmatch(value):
        _raise("INVALID_VALUE", path)
    return value


def _int(value: Any, path: str, minimum: int = 0, maximum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        _raise("INVALID_TYPE", path)
    if value < minimum or (maximum is not None and value > maximum):
        _raise("INVALID_VALUE", path)
    return value


def _number(value: Any, path: str, minimum: float | None = None) -> float | int:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        _raise("INVALID_TYPE", path)
    if isinstance(value, float) and not math.isfinite(value):
        _raise("NON_FINITE_NUMBER", path)
    if minimum is not None and value < minimum:
        _raise("INVALID_VALUE", path)
    return value


def _boolean(value: Any, path: str) -> bool:
    if not isinstance(value, bool):
        _raise("INVALID_TYPE", path)
    return value


def _enum(value: Any, path: str, allowed: tuple[str, ...]) -> str:
    text = _string(value, path)
    if text not in allowed:
        _raise("INVALID_VALUE", path)
    return text


def _digest(value: Any, path: str) -> str:
    return _string(value, path, pattern=SHA256_DIGEST)


def _git_sha(value: Any, path: str) -> str:
    return _string(value, path, pattern=GIT_SHA)


def _unavailable(value: Any, path: str) -> dict[str, Any] | None:
    """Allow a real value or an explicit {status, reason}; never a guessed value."""
    if value is None:
        _raise("INVALID_TYPE", path)
    if isinstance(value, dict):
        item = _object(value, path, ("status", "reason"))
        _enum(item["status"], f"{path}.status", UNAVAILABLE_STATUSES)
        _string(item["reason"], f"{path}.reason")
        if not item["reason"]:
            _raise("MISSING_FIELD", f"{path}.reason")
        return item
    return None


def _optional_unavailable(container: dict[str, Any], key: str, path: str, validate: Any) -> None:
    value = container[key]
    if isinstance(value, dict) and set(value) == {"status", "reason"}:
        _unavailable(value, f"{path}.{key}")
    else:
        validate(value, f"{path}.{key}")



def canonical_definition(manifest: dict[str, Any]) -> str:
    """Canonical definition hash: sorted-key compact JSON of scenario + workload."""
    import hashlib

    try:
        payload = json.dumps(
            {"scenario": manifest["scenario"], "workload": manifest["workload"]},
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
    except (UnicodeEncodeError, ValueError):
        _raise("INVALID_VALUE", "$.identity.definition_hash")
    return "sha256:" + hashlib.sha256(payload).hexdigest()


# ---------------------------------------------------------------- manifest ----


def validate_manifest(document: Any) -> dict[str, Any]:
    root = _object(
        document,
        "$",
        (
            "manifest_version",
            "scenario",
            "identity",
            "machine",
            "workload",
            "surface",
        ),
    )
    if _int(root["manifest_version"], "$.manifest_version") != MANIFEST_VERSION:
        _raise("UNSUPPORTED_VERSION", "$.manifest_version")

    scenario = _object(root["scenario"], "$.scenario", ("id", "version", "benchmark_revision"))
    _string(scenario["id"], "$.scenario.id", pattern=SAFE_ID)
    _string(scenario["version"], "$.scenario.version", pattern=SAFE_ID)
    _string(scenario["benchmark_revision"], "$.scenario.benchmark_revision", pattern=SAFE_ID)

    identity = _object(
        root["identity"],
        "$.identity",
        (
            "repository",
            "schema_hash",
            "definition_hash",
            "driver_hash",
            "gpui_revision",
            "build",
            "executable_sha256",
            "toolchain",
        ),
    )
    repository = _object(identity["repository"], "$.identity.repository", ("sha", "dirty", "content_hash"))
    _git_sha(repository["sha"], "$.identity.repository.sha")
    _boolean(repository["dirty"], "$.identity.repository.dirty")
    _optional_unavailable(repository, "content_hash", "$.identity.repository", _digest)
    _digest(identity["schema_hash"], "$.identity.schema_hash")
    _digest(identity["definition_hash"], "$.identity.definition_hash")
    _optional_unavailable(identity, "driver_hash", "$.identity", _digest)
    _string(identity["gpui_revision"], "$.identity.gpui_revision", pattern=SAFE_ID)
    build = _object(identity["build"], "$.identity.build", ("profile", "features", "flags"))
    _string(build["profile"], "$.identity.build.profile", pattern=SAFE_ID)
    for index, feature in enumerate(_array(build["features"], "$.identity.build.features")):
        _string(feature, f"$.identity.build.features[{index}]", pattern=SAFE_ID)
    for index, flag in enumerate(_array(build["flags"], "$.identity.build.flags")):
        _string(flag, f"$.identity.build.flags[{index}]")
    _optional_unavailable(identity, "executable_sha256", "$.identity", _digest)
    toolchain = _object(identity["toolchain"], "$.identity.toolchain", ("rustc", "cargo", "target"))
    for key in ("rustc", "cargo", "target"):
        _string(toolchain[key], f"$.identity.toolchain.{key}")

    machine = _object(
        root["machine"],
        "$.machine",
        (
            "os",
            "cpu",
            "ram_bytes",
            "gpu",
            "displays",
            "window",
            "power_profile",
            "background_contention",
            "thermal_state",
        ),
    )
    os_info = _object(machine["os"], "$.machine.os", ("edition", "build"))
    _string(os_info["edition"], "$.machine.os.edition")
    _string(os_info["build"], "$.machine.os.build")
    cpu = _object(machine["cpu"], "$.machine.cpu", ("model", "logical_count"))
    _string(cpu["model"], "$.machine.cpu.model")
    _int(cpu["logical_count"], "$.machine.cpu.logical_count", minimum=1)
    _optional_unavailable(machine, "ram_bytes", "$.machine", lambda v, p: _int(v, p, minimum=1))
    _optional_unavailable(machine, "gpu", "$.machine", lambda v, p: _object(v, p, ("adapter", "driver")))
    displays = _array(machine["displays"], "$.machine.displays", nonempty=True)
    for index, display in enumerate(displays):
        item = _object(display, f"$.machine.displays[{index}]", ("width", "height", "refresh_hz", "dpi"))
        _int(item["width"], f"$.machine.displays[{index}].width", minimum=1)
        _int(item["height"], f"$.machine.displays[{index}].height", minimum=1)
        _number(item["refresh_hz"], f"$.machine.displays[{index}].refresh_hz", minimum=0)
        _number(item["dpi"], f"$.machine.displays[{index}].dpi", minimum=0)
    window = _object(machine["window"], "$.machine.window", ("geometry", "visibility", "occlusion"))
    _string(window["geometry"], "$.machine.window.geometry")
    _enum(window["visibility"], "$.machine.window.visibility", ("VISIBLE", "OCCLUDED", "HIDDEN"))
    _optional_unavailable(window, "occlusion", "$.machine.window", lambda v, p: _enum(v, p, ("NONE", "PARTIAL", "FULL")))
    _optional_unavailable(machine, "power_profile", "$.machine", lambda v, p: _string(v, p))
    _optional_unavailable(machine, "background_contention", "$.machine", lambda v, p: _string(v, p))
    _optional_unavailable(machine, "thermal_state", "$.machine", lambda v, p: _string(v, p))

    workload = _object(
        root["workload"],
        "$.workload",
        (
            "generator",
            "distribution",
            "parameters",
            "source",
            "representation_policy",
            "actions",
            "duration_ms",
            "repetitions",
            "warmup",
            "cache_actions",
            "cache_state",
            "viewport",
            "endpoints",
            "run_id",
            "pair_id",
            "variant",
            "order",
            "expected_state",
            "recovery",
            "tool",
        ),
    )
    generator = _object(workload["generator"], "$.workload.generator", ("version", "revision", "seed"))
    _string(generator["version"], "$.workload.generator.version", pattern=SAFE_ID)
    _string(generator["revision"], "$.workload.generator.revision", pattern=SAFE_ID)
    _int(generator["seed"], "$.workload.generator.seed")
    _string(workload["distribution"], "$.workload.distribution", pattern=SAFE_ID)
    _parameters(workload["parameters"], "$.workload.parameters")
    source = _object(workload["source"], "$.workload.source", ("identity", "sha256"))
    _string(source["identity"], "$.workload.source.identity", pattern=SAFE_ID)
    _optional_unavailable(source, "sha256", "$.workload.source", _digest)
    _string(workload["representation_policy"], "$.workload.representation_policy", pattern=SAFE_ID)
    actions = _array(workload["actions"], "$.workload.actions", nonempty=True)
    for index, action in enumerate(actions):
        item = _object(action, f"$.workload.actions[{index}]", ("step", "requested_cadence_hz", "burst"))
        _string(item["step"], f"$.workload.actions[{index}].step", pattern=SAFE_ID)
        _optional_unavailable(item, "requested_cadence_hz", f"$.workload.actions[{index}]", lambda v, p: _number(v, p, minimum=0))
        _int(item["burst"], f"$.workload.actions[{index}].burst", minimum=1)
    _int(workload["duration_ms"], "$.workload.duration_ms", minimum=1)
    _int(workload["repetitions"], "$.workload.repetitions", minimum=1)
    _int(workload["warmup"], "$.workload.warmup")
    _array(workload["cache_actions"], "$.workload.cache_actions")
    _optional_unavailable(workload, "cache_state", "$.workload", lambda v, p: _enum(v, p, ("WARM", "COLD", "UNKNOWN")))
    _string(workload["viewport"], "$.workload.viewport")
    endpoints = _array(workload["endpoints"], "$.workload.endpoints", nonempty=True)
    for index, endpoint in enumerate(endpoints):
        item = _object(
            endpoint,
            f"$.workload.endpoints[{index}]",
            ("id", "definition", "start", "end", "correlation"),
        )
        _string(item["id"], f"$.workload.endpoints[{index}].id", pattern=SAFE_ID)
        for key in ("definition", "start", "end", "correlation"):
            _string(item[key], f"$.workload.endpoints[{index}].{key}")
    for key in ("run_id", "pair_id", "variant", "order"):
        _string(workload[key], f"$.workload.{key}", pattern=SAFE_ID)
    _string(workload["expected_state"], "$.workload.expected_state")
    _string(workload["recovery"], "$.workload.recovery")
    tool = _object(workload["tool"], "$.workload.tool", ("name", "version", "config"))
    _string(tool["name"], "$.workload.tool.name", pattern=SAFE_ID)
    _string(tool["version"], "$.workload.tool.version")
    _string(tool["config"], "$.workload.tool.config")

    surface = _object(root["surface"], "$.surface", ("id", "status", "reason"))
    _string(surface["id"], "$.surface.id", pattern=SAFE_ID)
    status = _enum(surface["status"], "$.surface.status", SURFACE_STATUSES)
    _string(surface["reason"], "$.surface.reason")
    if status == "IMPLEMENTED" and surface["reason"]:
        _raise("INVALID_VALUE", "$.surface.reason")
    if canonical_definition(root) != identity["definition_hash"]:
        _raise("INVALID_VALUE", "$.identity.definition_hash")
    return root


# ------------------------------------------------------------------ result ----



def _scalar(value: Any, path: str) -> None:
    if isinstance(value, bool):
        return
    if isinstance(value, str):
        try:
            encoded = value.encode("utf-8")
        except UnicodeEncodeError:
            _raise("INVALID_VALUE", path)
        if len(encoded) > 128:
            _raise("INVALID_VALUE", path)
        return
    if isinstance(value, int):
        if abs(value) > 2**53:
            _raise("INVALID_VALUE", path)
        return
    if isinstance(value, float):
        _number(value, path)
        return
    _raise("INVALID_TYPE", path)


def _parameters(value: Any, path: str) -> None:
    if not isinstance(value, dict):
        _raise("INVALID_TYPE", path)
    if len(value) > 32:
        _raise("INVALID_VALUE", path)
    for key, item in value.items():
        if not isinstance(key, str) or not SAFE_ID.fullmatch(key):
            _raise("INVALID_VALUE", path)
        _scalar(item, path)


def _resource_group(container: dict[str, Any], key: str, path: str, numbers: tuple[str, ...]) -> None:
    fields = ("status", "reason") + numbers
    item = _object(container[key], f"{path}.{key}", fields)
    status = _enum(item["status"], f"{path}.{key}.status", RUN_STATUSES)
    _string(item["reason"], f"{path}.{key}.reason")
    if status == "OBSERVED":
        if item["reason"]:
            _raise("INVALID_VALUE", f"{path}.{key}.reason")
        for name in numbers:
            _int(item[name], f"{path}.{key}.{name}", minimum=0)
    else:
        if not item["reason"]:
            _raise("MISSING_FIELD", f"{path}.{key}.reason")
        for name in numbers:
            if item[name] is not None:
                _raise("INVALID_VALUE", f"{path}.{key}.{name}")


def _metric(item: Any, path: str) -> None:
    metric = _object(
        item,
        path,
        (
            "id",
            "unit",
            "method",
            "sample_unit",
            "counts",
            "percentiles",
            "estimator",
            "validity",
            "spread",
        ),
    )
    _string(metric["id"], f"{path}.id", pattern=SAFE_ID)
    _string(metric["unit"], f"{path}.unit", pattern=SAFE_ID)
    _string(metric["method"], f"{path}.method", pattern=SAFE_ID)
    _string(metric["sample_unit"], f"{path}.sample_unit", pattern=SAFE_ID)
    counts = _object(
        metric["counts"],
        f"{path}.counts",
        ("eligible", "observed", "failed", "censored", "excluded"),
    )
    for key in ("eligible", "observed", "failed", "censored", "excluded"):
        _int(counts[key], f"{path}.counts.{key}")
    if counts["observed"] + counts["failed"] + counts["censored"] + counts["excluded"] > counts["eligible"]:
        _raise("INVALID_VALUE", f"{path}.counts")
    percentiles = _object(metric["percentiles"], f"{path}.percentiles", ("min", "max", "p50", "p95", "p99"))
    for key, value in percentiles.items():
        if value is None:
            continue
        _number(value, f"{path}.percentiles.{key}")
    _string(metric["estimator"], f"{path}.estimator", pattern=SAFE_ID)
    _string(metric["validity"], f"{path}.validity")
    _string(metric["spread"], f"{path}.spread")
    if any(value is not None for value in percentiles.values()):
        # MP-R0A-S1 implements no estimator; any percentile claim is unverifiable.
        _raise("INVALID_VALUE", f"{path}.percentiles")


def _endpoint(item: Any, path: str) -> None:
    endpoint = _object(
        item,
        path,
        ("id", "status", "reason", "provider", "version", "unit", "clock", "correlation"),
    )
    _string(endpoint["id"], f"{path}.id", pattern=SAFE_ID)
    status = _enum(endpoint["status"], f"{path}.status", RUN_STATUSES)
    _string(endpoint["reason"], f"{path}.reason")
    _optional_unavailable(endpoint, "provider", path, lambda v, p: _string(v, p, pattern=SAFE_ID))
    _optional_unavailable(endpoint, "version", path, lambda v, p: _string(v, p))
    _optional_unavailable(endpoint, "unit", path, lambda v, p: _string(v, p, pattern=SAFE_ID))
    _optional_unavailable(endpoint, "clock", path, lambda v, p: _string(v, p))
    _optional_unavailable(endpoint, "correlation", path, lambda v, p: _string(v, p))
    if status == "OBSERVED" and endpoint["reason"]:
        _raise("INVALID_VALUE", f"{path}.reason")
    if status != "OBSERVED" and not endpoint["reason"]:
        _raise("MISSING_FIELD", f"{path}.reason")


def _run(item: Any, path: str) -> None:
    run = _object(
        item,
        path,
        (
            "run_id",
            "pair_id",
            "scenario_id",
            "variant",
            "status",
            "reason",
            "endpoints",
            "metrics",
            "resources",
            "frames",
            "instrumentation",
            "failures",
            "outliers",
            "evidence",
        ),
    )
    for key in ("run_id", "pair_id", "scenario_id", "variant"):
        _string(run[key], f"{path}.{key}", pattern=SAFE_ID)
    status = _enum(run["status"], f"{path}.status", RUN_STATUSES)
    _string(run["reason"], f"{path}.reason")
    if status == "OBSERVED" and run["reason"]:
        _raise("INVALID_VALUE", f"{path}.reason")
    if status != "OBSERVED" and not run["reason"]:
        _raise("MISSING_FIELD", f"{path}.reason")
    endpoint_ids: set[str] = set()
    for index, entry in enumerate(_array(run["endpoints"], f"{path}.endpoints")):
        _endpoint(entry, f"{path}.endpoints[{index}]")
        if entry["id"] in endpoint_ids:
            _raise("INVALID_VALUE", f"{path}.endpoints[{index}].id")
        endpoint_ids.add(entry["id"])
    metric_ids: set[str] = set()
    for index, entry in enumerate(_array(run["metrics"], f"{path}.metrics")):
        _metric(entry, f"{path}.metrics[{index}]")
        if entry["id"] in metric_ids:
            _raise("INVALID_VALUE", f"{path}.metrics[{index}].id")
        metric_ids.add(entry["id"])

    resources = _object(
        run["resources"],
        f"{path}.resources",
        ("process", "media", "gpu", "devices", "sampling"),
    )
    _resource_group(resources, "process", f"{path}.resources", ("private_bytes", "working_set_bytes"))
    _resource_group(
        resources,
        "media",
        f"{path}.resources",
        ("decoded_bytes", "scratch_bytes", "staging_bytes", "cache_bytes"),
    )
    _resource_group(
        resources,
        "gpu",
        f"{path}.resources",
        ("dedicated_bytes", "shared_bytes", "committed_bytes"),
    )
    _resource_group(resources, "devices", f"{path}.resources", ("count", "generations"))
    _string(resources["sampling"], f"{path}.resources.sampling")

    frames = _object(
        run["frames"],
        f"{path}.frames",
        (
            "requested",
            "processed",
            "submitted",
            "present",
            "displayed",
            "missed",
            "dropped",
            "stalled",
            "eligibility",
            "idle",
            "rule",
            "reason",
        ),
    )
    counters = (
        "requested",
        "processed",
        "submitted",
        "present",
        "displayed",
        "missed",
        "dropped",
        "stalled",
        "eligibility",
        "idle",
    )
    missing = []
    for name in counters:
        value = frames[name]
        if value is None:
            missing.append(name)
        else:
            _int(value, f"{path}.frames.{name}", minimum=0)
    _string(frames["rule"], f"{path}.frames.rule")
    _string(frames["reason"], f"{path}.frames.reason")
    if status == "OBSERVED":
        if len(missing) == len(counters):
            _raise("MISSING_FIELD", f"{path}.frames")
        if missing and not frames["reason"]:
            _raise("MISSING_FIELD", f"{path}.frames.reason")
    elif status in ("NOT_MEASURED", "NOT_IMPLEMENTED"):
        if len(missing) != len(counters):
            _raise("INVALID_VALUE", f"{path}.frames")
        if not frames["reason"]:
            _raise("MISSING_FIELD", f"{path}.frames.reason")
    else:  # FAILED keeps whatever was actually captured
        if missing and not frames["reason"]:
            _raise("MISSING_FIELD", f"{path}.frames.reason")
    if (
        frames["present"] is not None
        and frames["submitted"] is not None
        and frames["present"] > frames["submitted"]
    ):
        _raise("INVALID_VALUE", f"{path}.frames")
    if (
        frames["displayed"] is not None
        and frames["present"] is not None
        and frames["displayed"] > frames["present"]
    ):
        _raise("INVALID_VALUE", f"{path}.frames")

    instrumentation = _object(
        run["instrumentation"],
        f"{path}.instrumentation",
        ("tool", "revision", "config", "probes", "overhead", "loss", "floor"),
    )
    _string(instrumentation["tool"], f"{path}.instrumentation.tool", pattern=SAFE_ID)
    _string(instrumentation["revision"], f"{path}.instrumentation.revision")
    _string(instrumentation["config"], f"{path}.instrumentation.config")
    probes = _object(instrumentation["probes"], f"{path}.instrumentation.probes", ("supported", "missing"))
    for index, probe in enumerate(_array(probes["supported"], f"{path}.instrumentation.probes.supported")):
        _string(probe, f"{path}.instrumentation.probes.supported[{index}]", pattern=SAFE_ID)
    for index, probe in enumerate(_array(probes["missing"], f"{path}.instrumentation.probes.missing")):
        _string(probe, f"{path}.instrumentation.probes.missing[{index}]", pattern=SAFE_ID)
    _string(instrumentation["overhead"], f"{path}.instrumentation.overhead")
    _string(instrumentation["loss"], f"{path}.instrumentation.loss")
    _string(instrumentation["floor"], f"{path}.instrumentation.floor")
    for key in ("failures", "outliers", "evidence"):
        for index, entry in enumerate(_array(run[key], f"{path}.{key}")):
            _string(entry, f"{path}.{key}[{index}]")

    if status == "OBSERVED":
        if not run["evidence"]:
            _raise("MISSING_FIELD", f"{path}.evidence")
        if not any(endpoint["status"] == "OBSERVED" for endpoint in run["endpoints"]):
            _raise("INVALID_VALUE", f"{path}.endpoints")
        if not any(metric["counts"]["observed"] > 0 for metric in run["metrics"]):
            _raise("INVALID_VALUE", f"{path}.metrics")
    elif status in ("NOT_MEASURED", "NOT_IMPLEMENTED"):
        if any(metric["counts"]["observed"] > 0 for metric in run["metrics"]):
            _raise("INVALID_VALUE", f"{path}.metrics")
        if any(endpoint["status"] == "OBSERVED" for endpoint in run["endpoints"]):
            _raise("INVALID_VALUE", f"{path}.endpoints")


def validate_result(document: Any) -> dict[str, Any]:
    root = _object(document, "$", ("result_version", "manifest", "runs", "evidence"))
    if _int(root["result_version"], "$.result_version") != RESULT_VERSION:
        _raise("UNSUPPORTED_VERSION", "$.result_version")
    manifest = _object(
        root["manifest"],
        "$.manifest",
        ("scenario_id", "scenario_version", "benchmark_revision", "definition_hash"),
    )
    for key in ("scenario_id", "scenario_version", "benchmark_revision"):
        _string(manifest[key], f"$.manifest.{key}", pattern=SAFE_ID)
    _digest(manifest["definition_hash"], "$.manifest.definition_hash")
    for index, run in enumerate(_array(root["runs"], "$.runs", nonempty=True)):
        _run(run, f"$.runs[{index}]")
    for index, evidence in enumerate(_array(root["evidence"], "$.evidence")):
        _string(evidence, f"$.evidence[{index}]")
    return root


def check_identity(manifest: dict[str, Any], result: dict[str, Any]) -> None:
    """Result must reference the same scenario/definition identity and run selection."""
    scenario = manifest["scenario"]
    identity = manifest["identity"]
    reference = result["manifest"]
    if (
        reference["scenario_id"] != scenario["id"]
        or reference["scenario_version"] != scenario["version"]
        or reference["benchmark_revision"] != scenario["benchmark_revision"]
        or reference["definition_hash"] != identity["definition_hash"]
    ):
        _raise("IDENTITY_MISMATCH", "$.manifest")
    workload = manifest["workload"]
    for index, run in enumerate(result["runs"]):
        if (
            run["scenario_id"] != scenario["id"]
            or run["run_id"] != workload["run_id"]
            or run["pair_id"] != workload["pair_id"]
            or run["variant"] != workload["variant"]
        ):
            _raise("IDENTITY_MISMATCH", f"$.runs[{index}]")
        if manifest["surface"]["status"] == "NOT_IMPLEMENTED" and run["status"] == "OBSERVED":
            _raise("INVALID_VALUE", f"$.runs[{index}].status")
        declared = {endpoint["id"] for endpoint in workload["endpoints"]}
        for position, endpoint in enumerate(run["endpoints"]):
            if endpoint["status"] == "OBSERVED" and endpoint["id"] not in declared:
                _raise("INVALID_VALUE", f"$.runs[{index}].endpoints[{position}].id")
