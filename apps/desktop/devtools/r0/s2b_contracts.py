"""Version 1 S2B observation contract. Wire validity is not Native or display proof.

S1/S2A remain unchanged. No aggregation, percentiles, hidden fallback or Domain
semantics live here. All public errors use fixed codes without input contents.
"""
from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import NoReturn

import contracts

MAX_BYTES = 1024 * 1024
MAX_RECORDS = 256
MAX_WINDOWS = 16
MAX_DURATION_NS = 30_000_000_000
FEATURE = "r0-s2b-instrumentation"
GPUI_PIN = "395def7a067b32ed598fcb3f048969ada9b54942"
FILE_NAME = "s2b-capture.json"
OBSERVATION_NAME = "s2b-observation.json"
SOURCE_FILES = (
    "Cargo.lock", "apps/desktop/Cargo.toml", "crates/desktop-gpui/Cargo.toml",
    "apps/desktop/src/main.rs", "apps/desktop/src/r0_s2b.rs",
    "crates/desktop-gpui/src/lib.rs", "crates/desktop-gpui/src/r0_probe.rs",
    "apps/desktop/devtools/r0/s2b_contracts.py",
    "apps/desktop/devtools/r0/s2b_capture.py",
    "apps/desktop/devtools/r0/s2b_validate.py",
)
ROOT_FIELDS = {
    "capture_version", "execution_id", "pid", "executable_sha256", "gpui_pin",
    "feature", "target", "clock", "scope_start_ns", "scope_end_ns", "records", "summary",
}
RECORD_FIELDS = {
    "seq", "kind", "window", "lifetime", "start_ns", "end_ns", "dirty_ns",
    "invalidations", "paired_draw_start_ns", "paired_draw_end_ns", "category",
    "caused_invalidation", "lost",
}
COUNT_FIELDS = {
    "journal_lost", "journal_seen", "local_received", "local_enqueued", "local_dropped",
    "local_undrained", "eligible", "retained", "retention_dropped", "excluded", "pre_scope",
    "post_scope", "invalid_clock", "barrier_spans", "correlation_invalid",
    "input_unassociated", "unmatched_submission", "undrained_journal",
}
SUPPORTED = {
    "input-dispatch": 0, "gpui-dirty": 1, "gpui-cpu-draw": 2,
    "gpui-platform-submission": 3, "local-surface-mutation": 8, "local-notify": 9,
}
MISSING = (
    "app-main-entry", "frame-request", "first-frame", "useful-shell", "gpu-submission",
    "present-call", "present-return", "present", "displayed", "scanout", "input-to-photon",
)


class ObservationError(Exception):
    """One closed public error code; never a user value or path."""


def fail(code: str) -> NoReturn:
    raise ObservationError(code)


def object_fields(value: object, fields: set[str]) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        fail("SCHEMA")
    return value


def integer(value: object, *, low: int = 0, high: int = (1 << 64) - 1) -> int:
    if type(value) is not int or not low <= value <= high:
        fail("INTEGER")
    return value


def digest(value: object) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", value):
        fail("DIGEST")
    return value


def parse_bytes(data: bytes, limit: int) -> object:
    """Strict loader for bytes already read from one pinned handle."""
    if len(data) > limit:
        fail("TOO_LARGE")
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        fail("INVALID_UTF8")
    def pairs(items):
        seen = set()
        for key, _ in items:
            if key in seen:
                fail("DUPLICATE_KEY")
            seen.add(key)
        return dict(items)
    def reject(_):
        fail("NON_FINITE_NUMBER")
    try:
        return json.loads(text, object_pairs_hook=pairs, parse_constant=reject)
    except ObservationError:
        raise
    except (json.JSONDecodeError, RecursionError, ValueError, OverflowError):
        fail("MALFORMED_JSON")


def source_digest() -> str:
    value = hashlib.sha256()
    root = Path(__file__).resolve().parents[4]
    for name in sorted(SOURCE_FILES):
        value.update(name.encode() + b"\0")
        value.update((root / name).read_bytes().replace(b"\r\n", b"\n"))
    return "sha256:" + value.hexdigest()


def validate_manifest(document: object, expected_driver_hash: str | None = None) -> dict:
    manifest = contracts.validate_manifest(document)
    identity = manifest["identity"]
    if identity["build"]["features"] != [FEATURE] or identity["gpui_revision"] != GPUI_PIN:
        fail("INSTRUMENTATION_IDENTITY")
    if identity["build"]["profile"] != "dev" or identity["repository"]["dirty"] is not False:
        fail("BUILD_IDENTITY")
    # A fresh candidate manifest must match the live sources; replay of a
    # recorded bundle compares against its pinned source identity instead, so a
    # later consumer revision cannot invalidate historical evidence.
    if expected_driver_hash is None:
        if identity["driver_hash"] != source_digest():
            fail("SOURCE_IDENTITY")
    elif identity["driver_hash"] != expected_driver_hash:
        fail("SOURCE_IDENTITY")
    if identity["toolchain"]["target"] != "x86_64-pc-windows-msvc":
        fail("TARGET")
    if not isinstance(manifest["workload"]["source"]["sha256"], str):
        fail("FIXTURE_IDENTITY")
    return manifest


def source_identity() -> dict:
    """Run-time record of the exact consumer/build sources and their digest."""
    root = Path(__file__).resolve().parents[4]
    files = {}
    for name in sorted(SOURCE_FILES):
        files[name] = "sha256:" + hashlib.sha256((root / name).read_bytes().replace(b"\r\n", b"\n")).hexdigest()
    return {"record_version": 1, "observer_source_sha256": source_digest(), "files": files}


def validate_source_identity(document: object) -> dict:
    value = object_fields(document, {"record_version", "observer_source_sha256", "files"})
    if type(value["record_version"]) is not int or value["record_version"] != 1:
        fail("SOURCE_IDENTITY_VERSION")
    digest(value["observer_source_sha256"])
    files = value["files"]
    if not isinstance(files, dict) or set(files) != set(SOURCE_FILES):
        fail("SOURCE_IDENTITY")
    for entry in files.values():
        digest(entry)
    return value


def validate_capture(document: object, *, execution: str | None = None,
                     pid: int | None = None, executable: str | None = None) -> dict:
    value = object_fields(document, ROOT_FIELDS)
    if type(value["capture_version"]) is not int or value["capture_version"] != 1:
        fail("VERSION")
    if not isinstance(value["execution_id"], str) or not re.fullmatch(r"[0-9a-f]{32}", value["execution_id"]):
        fail("EXECUTION")
    integer(value["pid"], low=1, high=(1 << 32) - 1)
    digest(value["executable_sha256"])
    if ((execution is not None and value["execution_id"] != execution)
            or (pid is not None and value["pid"] != pid)
            or (executable is not None and value["executable_sha256"] != executable)):
        fail("IDENTITY_MISMATCH")
    if value["feature"] != FEATURE or value["gpui_pin"] != GPUI_PIN or value["target"] != "x86_64-pc-windows-msvc":
        fail("INSTRUMENTATION_IDENTITY")
    clock = object_fields(value["clock"], {
        "domain", "origin_ns", "unit", "resolution", "uncertainty", "mapping",
        "paired_reads", "min_positive_delta_ns",
    })
    if (clock["domain"] != "native-process-instant-v1" or type(clock["origin_ns"]) is not int
            or clock["origin_ns"] != 0 or clock["unit"] != "ns" or clock["resolution"] != "NOT_MEASURED"
            or clock["uncertainty"] != "NOT_MEASURED" or clock["mapping"] != "NONE"
            or type(clock["paired_reads"]) is not int or clock["paired_reads"] != 128):
        fail("CLOCK_DOMAIN")
    if clock["min_positive_delta_ns"] is not None:
        integer(clock["min_positive_delta_ns"], low=1)
    start, end = integer(value["scope_start_ns"]), integer(value["scope_end_ns"])
    if end < start or end - start > MAX_DURATION_NS:
        fail("CAPTURE_SCOPE")
    records = value["records"]
    if not isinstance(records, list) or len(records) > MAX_RECORDS:
        fail("RECORD_LIMIT")
    counts = [0] * 10
    windows, lifetimes, closed, spans = {}, {}, {}, set()
    power_boundary = start
    for seq, raw in enumerate(records, 1):
        record = object_fields(raw, RECORD_FIELDS)
        if integer(record["seq"], low=1) != seq:
            fail("SEQUENCE")
        kind = integer(record["kind"], high=9)
        counts[kind] += 1
        nonnull = {key for key in RECORD_FIELDS - {"seq", "kind"} if record[key] is not None}
        required = {
            0: {"start_ns", "end_ns", "category", "caused_invalidation"},
            1: {"window", "lifetime", "start_ns", "end_ns"},
            2: {"window", "lifetime", "start_ns", "end_ns", "invalidations"},
            3: {"window", "lifetime", "start_ns", "end_ns"},
            4: {"window", "lifetime", "start_ns", "end_ns"},
            5: {"lost"}, 6: {"start_ns", "end_ns"}, 7: {"start_ns", "end_ns"},
            8: {"window", "lifetime", "start_ns", "end_ns", "category"},
            9: {"window", "lifetime", "start_ns", "end_ns"},
        }[kind]
        optional = {"dirty_ns"} if kind == 2 else ({
            "dirty_ns", "invalidations", "paired_draw_start_ns", "paired_draw_end_ns",
        } if kind == 3 else set())
        if not required <= nonnull or not nonnull <= required | optional:
            fail("RECORD_SEMANTICS")
        for name in nonnull - {"caused_invalidation"}:
            integer(record[name])
        if kind != 5:
            begin, finish = record["start_ns"], record["end_ns"]
            if not start <= begin <= finish <= end:
                fail("TIMESTAMP")
            if kind not in (0, 2, 3) and begin != finish:
                fail("POINT_EVENT")
            if kind in (0, 2, 3) and begin < power_boundary:
                fail("POWER_BOUNDARY")
            if kind == 7:
                power_boundary = finish
        if record["window"] is not None:
            window, lifetime = record["window"], integer(record["lifetime"], low=1, high=MAX_WINDOWS)
            if (window in windows and windows[window] != lifetime) or (lifetime in lifetimes and lifetimes[lifetime] != window):
                fail("WINDOW_LIFETIME")
            windows[window], lifetimes[lifetime] = lifetime, window
            if window in closed and record["end_ns"] > closed[window]:
                fail("CLOSED_WINDOW")
            if kind == 4:
                if window in closed:
                    fail("CLOSED_WINDOW")
                closed[window] = record["end_ns"]
        if kind == 0:
            if type(record["caused_invalidation"]) is not bool:
                fail("INPUT_CATEGORY")
            integer(record["category"], high=14)
        if kind == 8:
            integer(record["category"], high=2)
        if record["dirty_ns"] is not None and not start <= record["dirty_ns"] <= record["start_ns"]:
            fail("DIRTY_ORDER")
        paired = record["paired_draw_start_ns"] is not None
        if (record["paired_draw_end_ns"] is not None) != paired:
            fail("PAIR")
        if kind == 3:
            if (record["invalidations"] is not None) != paired or (record["dirty_ns"] is not None and not paired):
                fail("PAIR")
            if paired and not start <= record["paired_draw_start_ns"] <= record["paired_draw_end_ns"] <= record["start_ns"]:
                fail("PAIR_ORDER")
            if paired and record["dirty_ns"] is not None and record["dirty_ns"] > record["paired_draw_start_ns"]:
                fail("DIRTY_ORDER")
        if kind in (2, 3):
            span = (kind, record["window"], record["start_ns"], record["end_ns"])
            if span in spans:
                fail("DUPLICATE_EVENT")
            spans.add(span)
        if kind == 5 and record["lost"] == 0:
            fail("LOSS")
    summary = object_fields(value["summary"], COUNT_FIELDS | {
        "cancelled", "source_coverage", "unoffered_loss", "unfinalized_tail", "kind_counts",
    })
    for field in COUNT_FIELDS:
        integer(summary[field])
    if (type(summary["cancelled"]) is not bool or summary["cancelled"] != (end - start < MAX_DURATION_NS)
            or summary["source_coverage"] != "OFFERED_FINALIZED_JOURNAL_ONLY"
            or summary["unoffered_loss"] != "UNKNOWN" or summary["unfinalized_tail"] != "UNKNOWN"):
        fail("COVERAGE")
    kinds = summary["kind_counts"]
    if not isinstance(kinds, list) or len(kinds) != 10:
        fail("COUNTS")
    for count in kinds:
        integer(count)
    received = summary["journal_seen"] + summary["local_received"]
    accounted = sum(summary[name] for name in (
        "eligible", "excluded", "pre_scope", "post_scope", "invalid_clock",
        "barrier_spans", "correlation_invalid",
    ))
    if received >= 1 << 64 or accounted != received:
        fail("COUNTS")
    if (summary["local_received"] < counts[8] + counts[9]
            or summary["journal_seen"] < sum(counts[:8])
            or summary["unmatched_submission"] < sum(
                1 for item in records if item["kind"] == 3 and item["paired_draw_start_ns"] is None)):
        fail("COUNTS")
    if (kinds != counts or summary["retained"] != len(records)
            or summary["eligible"] < len(records)
            or summary["eligible"] - len(records) != summary["retention_dropped"]
            or summary["eligible"] > summary["journal_seen"] + summary["local_received"]
            or summary["local_received"] + summary["local_undrained"] != summary["local_enqueued"]
            or summary["retention_dropped"] > 0 and len(records) != MAX_RECORDS
            or not counts[0] <= summary["input_unassociated"] <= summary["eligible"]
            or summary["unmatched_submission"] > summary["eligible"]
            or sum(item["lost"] or 0 for item in records) > summary["journal_lost"]):
        fail("COUNTS")
    if len(windows) > MAX_WINDOWS:
        fail("WINDOW_LIMIT")
    return value


def endpoint_matrix(capture: dict) -> dict:
    counts = capture["summary"]["kind_counts"]
    result = {
        name: {"status": "OBSERVED" if counts[kind] else "NOT_MEASURED",
               "reason": "RETAINED_SOURCE_RECORD" if counts[kind] else "NO_RETAINED_VALID_RECORD"}
        for name, kind in SUPPORTED.items()
    }
    result.update({name: {"status": "NOT_MEASURED", "reason": "NOT_EXPOSED_BY_SELECTED_PROBE"}
                   for name in MISSING})
    return result


def disposition(capture: dict, ledger: dict) -> str:
    """Called after both contracts validate; process failure cannot become success."""
    if ledger["timed_out"] or ledger["exit_code"] != 0:
        return "FAILED"
    if any(capture["summary"]["kind_counts"][kind] for kind in SUPPORTED.values()):
        return "OBSERVED"
    return "NOT_MEASURED"
