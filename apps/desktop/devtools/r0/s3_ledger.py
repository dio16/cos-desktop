"""MP-R0A-S3: append-only run ledger, bounded loading and aggregation.

Python standard library only. This module records and aggregates observations; it
performs no measurement, launches nothing and executes no value from an input
document. Absent facts stay explicit `{"status", "reason"}` objects, never zeros.

Ledger format (version 1): UTF-8 JSON Lines, one record per line, append-only.
The first line is the header `{"record":"ledger_header","ledger_version":1}`; every
later line is one run entry:

    record          "run_entry"
    entry_version   1
    run_id          safe id, unique within the ledger
    source          {"git_sha": <40/64 hex>|null, "worktree_digest": "sha256:..."|null}
    candidate       {"id": safe id, "program_digest": "sha256:..."|null}
    host            {"os": windows|linux|macos, "arch": x86_64|aarch64}
    interpreter     {"name": safe id, "version": "X.Y.Z"}
    start_utc/end_utc  ISO-8601 UTC seconds, end >= start
    outcome         OBSERVED | CANCELLED | FAILED | NOT_MEASURED | NOT_IMPLEMENTED
    counters        every name in COUNTERS, an integer when measured, `null` when not
    entry_sha256    "sha256:" + SHA-256 of the canonical entry without this field

The per-run digest covers the entry body serialized with sorted keys and no whitespace,
so any later edit of a stored line is detected by `ledger verify` as DIGEST_MISMATCH.
An append writes the header once and then only ever adds a line: no existing byte is
rewritten, and a duplicate `run_id` is refused.

Loading is bounded on three axes: the file (MAX_LEDGER_BYTES), the number of entries
(MAX_LEDGER_ENTRIES) and a single line (MAX_ENTRY_BYTES). Each line is parsed with the
existing `contracts` rules (UTF-8, duplicate keys and non-finite numbers rejected), and
every failure is one closed code with an optional line number -- never input text.

Aggregation (`summary`) includes only runs that are OBSERVED *and* source- and
candidate-bound; everything else appears in `excluded` with a closed reason
(OUTCOME_*, SOURCE_NOT_BOUND, CANDIDATE_NOT_BOUND). A counter that no contributing run
measured is reported as `{"status": "NOT_MEASURED", "reason": ...}` rather than 0, and
an empty ledger reports NO_CONTRIBUTING_RUNS for every field.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any, NoReturn

import contracts

LEDGER_VERSION = 1
ENTRY_VERSION = 1
RECORD_HEADER = "ledger_header"
RECORD_RUN = "run_entry"

MAX_LEDGER_BYTES = 4 * 1024 * 1024
MAX_LEDGER_ENTRIES = 1024
MAX_ENTRY_BYTES = 64 * 1024

# The S3 outcome vocabulary extends the S1 closed list with CANCELLED; nothing else is
# accepted, so an unknown word can never leak into a summary.
OUTCOMES = ("OBSERVED", "CANCELLED", "FAILED", "NOT_MEASURED", "NOT_IMPLEMENTED")
CONTRIBUTING_OUTCOME = "OBSERVED"

# Closed counter vocabulary. A counter is an integer when it was measured and `null`
# when it was not; a missing fact is never a zero.
COUNTERS = (
    "frames_observed",
    "dirty_frames",
    "present_calls",
    "scope_ns",
)

# Closed exclusion vocabulary for the aggregation provenance.
EXCLUSIONS = (
    "OUTCOME_FAILED",
    "OUTCOME_CANCELLED",
    "OUTCOME_NOT_MEASURED",
    "OUTCOME_NOT_IMPLEMENTED",
    "SOURCE_NOT_BOUND",
    "CANDIDATE_NOT_BOUND",
)
NO_CONTRIBUTING_RUNS = "NO_CONTRIBUTING_RUNS"
NO_MEASURED_COUNTER = "NO_RUN_MEASURED_THIS_COUNTER"

ERROR_CODES = (
    "READ_FAILED",
    "TOO_LARGE",
    "INVALID_UTF8",
    "MALFORMED_JSON",
    "DUPLICATE_KEY",
    "NON_FINITE_NUMBER",
    "LEDGER_EMPTY",
    "LEDGER_HEADER_MISSING",
    "LEDGER_HEADER_INVALID",
    "LEDGER_ENTRY_LIMIT",
    "LEDGER_ENTRY_TOO_LARGE",
    "LEDGER_TOO_LARGE",
    "SCHEMA",
    "UNKNOWN_FIELD",
    "MISSING_FIELD",
    "INVALID_VALUE",
    "INVALID_TYPE",
    "DIGEST",
    "DIGEST_MISMATCH",
    "DUPLICATE_RUN",
    "LEDGER_VERSION",
)

HEADER_FIELDS = {"record", "ledger_version"}
IDENTITY_FIELDS = {"git_sha", "worktree_digest"}
CANDIDATE_FIELDS = {"id", "program_digest"}
HOST_FIELDS = {"os", "arch"}
INTERPRETER_FIELDS = {"name", "version"}
ENTRY_FIELDS = {
    "record",
    "entry_version",
    "run_id",
    "source",
    "candidate",
    "host",
    "interpreter",
    "start_utc",
    "end_utc",
    "outcome",
    "counters",
    "entry_sha256",
}

SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
GIT_SHA = re.compile(r"^[0-9a-f]{40}([0-9a-f]{24})?$")
SHA256_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
TIMESTAMP = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
OPERATING_SYSTEMS = ("windows", "linux", "macos")
ARCHITECTURES = ("x86_64", "aarch64")


class LedgerError(Exception):
    """One closed public error code; never input text, a path or a traceback."""

    def __init__(self, code: str, *, line: int | None = None):
        super().__init__(code)
        self.code = code
        self.line = line


def fail(code: str, *, line: int | None = None) -> NoReturn:
    assert code in ERROR_CODES
    raise LedgerError(code, line=line)


# --------------------------------------------------------------------------- schema


def _object(value: Any, fields: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail("INVALID_TYPE")
    for key in value:
        if key not in fields:
            fail("UNKNOWN_FIELD")
    for key in fields:
        if key not in value:
            fail("MISSING_FIELD")
    return value


def _text(value: Any, pattern: re.Pattern[str]) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        fail("INVALID_VALUE")
    return value


def _integer(value: Any) -> int:
    if type(value) is not int or value < 0 or value > (1 << 63) - 1:
        fail("INVALID_VALUE")
    return value


def _digest(value: Any) -> str:
    if not isinstance(value, str) or not SHA256_DIGEST.fullmatch(value):
        fail("DIGEST")
    return value


def validate_identity(value: Any) -> dict[str, str | None]:
    """Source identity: the commit plus the digest of the measured worktree.

    Either field may be `null`, which is the explicit "this run is not source-bound"
    state; such a run is excluded from the aggregation with `SOURCE_NOT_BOUND`.
    """
    identity = _object(value, IDENTITY_FIELDS)
    sha = identity["git_sha"]
    worktree = identity["worktree_digest"]
    return {
        "git_sha": None if sha is None else _text(sha, GIT_SHA),
        "worktree_digest": None if worktree is None else _digest(worktree),
    }


def validate_candidate(value: Any) -> dict[str, str | None]:
    """Candidate identity; a `null` program digest means "not bound" (`CANDIDATE_NOT_BOUND`)."""
    candidate = _object(value, CANDIDATE_FIELDS)
    program = candidate["program_digest"]
    return {
        "id": _text(candidate["id"], SAFE_ID),
        "program_digest": None if program is None else _digest(program),
    }


def validate_host(value: Any) -> dict[str, str]:
    host = _object(value, HOST_FIELDS)
    operating_system = host["os"]
    architecture = host["arch"]
    if operating_system not in OPERATING_SYSTEMS or architecture not in ARCHITECTURES:
        fail("INVALID_VALUE")
    return {"os": operating_system, "arch": architecture}


def validate_interpreter(value: Any) -> dict[str, str]:
    interpreter = _object(value, INTERPRETER_FIELDS)
    return {
        "name": _text(interpreter["name"], SAFE_ID),
        "version": _text(interpreter["version"], re.compile(r"^\d+\.\d+\.\d+$")),
    }


def validate_counters(value: Any) -> dict[str, int | None]:
    if not isinstance(value, dict):
        fail("INVALID_TYPE")
    for key in value:
        if key not in COUNTERS:
            fail("UNKNOWN_FIELD")
    # Every counter is present; `null` is the explicit "not measured", never 0.
    counters: dict[str, int | None] = {}
    for name in COUNTERS:
        if name not in value:
            fail("MISSING_FIELD")
        raw = value[name]
        counters[name] = None if raw is None else _integer(raw)
    return counters


def canonical(entry: dict[str, Any]) -> bytes:
    """Deterministic bytes for the per-run digest: sorted keys, no whitespace."""
    body = {key: value for key, value in entry.items() if key != "entry_sha256"}
    return json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def entry_digest(entry: dict[str, Any]) -> str:
    return "sha256:" + hashlib.sha256(canonical(entry)).hexdigest()


def validate_entry(value: Any) -> dict[str, Any]:
    """Validate one run entry, including its per-run digest."""
    entry = _object(value, ENTRY_FIELDS)
    if entry["record"] != RECORD_RUN:
        fail("INVALID_VALUE")
    if entry["entry_version"] != ENTRY_VERSION:
        fail("LEDGER_VERSION")
    validated: dict[str, Any] = {
        "record": RECORD_RUN,
        "entry_version": ENTRY_VERSION,
        "run_id": _text(entry["run_id"], SAFE_ID),
        "source": validate_identity(entry["source"]),
        "candidate": validate_candidate(entry["candidate"]),
        "host": validate_host(entry["host"]),
        "interpreter": validate_interpreter(entry["interpreter"]),
        "start_utc": _text(entry["start_utc"], TIMESTAMP),
        "end_utc": _text(entry["end_utc"], TIMESTAMP),
        "outcome": entry["outcome"],
        "counters": validate_counters(entry["counters"]),
        "entry_sha256": _digest(entry["entry_sha256"]),
    }
    if validated["outcome"] not in OUTCOMES:
        fail("INVALID_VALUE")
    if validated["end_utc"] < validated["start_utc"]:
        fail("INVALID_VALUE")
    if entry_digest(validated) != validated["entry_sha256"]:
        fail("DIGEST_MISMATCH")
    return validated


# --------------------------------------------------------------------------- ledger


def _header() -> dict[str, Any]:
    return {"record": RECORD_HEADER, "ledger_version": LEDGER_VERSION}


def _encode(document: dict[str, Any]) -> str:
    return json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def _read_bytes(path: str) -> bytes:
    """Bounded read with the ledger's own closed codes (same rule as the loader)."""
    try:
        size = Path(path).stat().st_size
    except OSError:
        fail("READ_FAILED")
    if size > MAX_LEDGER_BYTES:
        fail("LEDGER_TOO_LARGE")
    try:
        with open(path, "rb") as handle:
            data = handle.read(MAX_LEDGER_BYTES + 1)
    except OSError:
        fail("READ_FAILED")
    if len(data) > MAX_LEDGER_BYTES:
        fail("LEDGER_TOO_LARGE")
    return data


def _parse_line(text: str, line: int) -> Any:
    if len(text.encode("utf-8")) > MAX_ENTRY_BYTES:
        fail("LEDGER_ENTRY_TOO_LARGE", line=line)
    try:
        return contracts.loads_document(text)
    except contracts.ContractError as error:
        code = error.errors[0]["code"]
        # The existing loader rules are reused unchanged; only the name is remapped so
        # a ledger reader always sees a ledger-scoped closed code.
        remapped = {
            "TOO_LARGE": "LEDGER_ENTRY_TOO_LARGE",
            "MALFORMED_JSON": "MALFORMED_JSON",
            "DUPLICATE_KEY": "DUPLICATE_KEY",
            "NON_FINITE_NUMBER": "NON_FINITE_NUMBER",
            "INVALID_UTF8": "INVALID_UTF8",
        }.get(code, "MALFORMED_JSON")
        fail(remapped, line=line)


def load_ledger(path: str) -> list[dict[str, Any]]:
    """Read the append-only ledger: one header line, then run entries.

    Bounded on three axes (file size, entry count, entry size); the first line must be
    the versioned header and every later line a validated run entry with a matching
    per-run digest.
    """
    data = _read_bytes(path)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        fail("INVALID_UTF8")
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()  # the single trailing newline every append writes
    if not lines:
        fail("LEDGER_EMPTY")
    for index, line in enumerate(lines, start=1):
        if line == "":
            # A blank line can only come from editing; refuse instead of skipping it.
            fail("MALFORMED_JSON", line=index)
    if len(lines) - 1 > MAX_LEDGER_ENTRIES:
        fail("LEDGER_ENTRY_LIMIT")
    header = _parse_line(lines[0], 1)
    if not isinstance(header, dict) or "record" not in header:
        fail("LEDGER_HEADER_INVALID")
    if header.get("record") != RECORD_HEADER:
        fail("LEDGER_HEADER_INVALID")
    if set(header) != HEADER_FIELDS:
        fail("LEDGER_HEADER_INVALID")
    if header.get("ledger_version") != LEDGER_VERSION:
        fail("LEDGER_VERSION")
    entries: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, line in enumerate(lines[1:], start=2):
        entry = _parse_line(line, index)
        try:
            validated = validate_entry(entry)
        except LedgerError as error:
            # The line is what an operator needs; the code stays closed.
            fail(error.code, line=error.line if error.line is not None else index)
        if validated["run_id"] in seen:
            fail("DUPLICATE_RUN", line=index)
        seen.add(validated["run_id"])
        entries.append(validated)
    return entries


def append_entry(path: str, entry: dict[str, Any]) -> dict[str, Any]:
    """Append one run entry without rewriting a single existing byte."""
    validated = validate_entry(entry)
    target = Path(path)
    if target.exists():
        entries = load_ledger(path)
        if any(existing["run_id"] == validated["run_id"] for existing in entries):
            fail("DUPLICATE_RUN")
        existing_lines = len(entries)
        prefix = ""
    else:
        existing_lines = 0
        prefix = _encode(_header()) + "\n"
    if existing_lines + 1 > MAX_LEDGER_ENTRIES:
        fail("LEDGER_ENTRY_LIMIT")
    line = _encode(validated)
    if len(line.encode("utf-8")) > MAX_ENTRY_BYTES:
        fail("LEDGER_ENTRY_TOO_LARGE")
    if target.exists() and target.stat().st_size + len(line) + 1 > MAX_LEDGER_BYTES:
        fail("LEDGER_TOO_LARGE")
    with open(target, "a", encoding="utf-8", newline="\n") as handle:
        handle.write(prefix + line + "\n")
    return {"run_id": validated["run_id"], "entry_sha256": validated["entry_sha256"],
            "entries": existing_lines + 1}


def verify_ledger(path: str) -> dict[str, Any]:
    entries = load_ledger(path)
    return {"entries": len(entries),
            "run_ids": [entry["run_id"] for entry in entries]}


# --------------------------------------------------------------------------- summary


def _is_bound(entry: dict[str, Any]) -> tuple[bool, str]:
    """Whether a run may contribute; the reason is a closed exclusion code."""
    if entry["outcome"] != CONTRIBUTING_OUTCOME:
        return False, f"OUTCOME_{entry['outcome']}"
    if not entry["source"]["git_sha"] or not entry["source"]["worktree_digest"]:
        return False, "SOURCE_NOT_BOUND"
    if not entry["candidate"]["id"] or not entry["candidate"]["program_digest"]:
        return False, "CANDIDATE_NOT_BOUND"
    return True, ""


def summarise(entries: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate contributing runs with explicit provenance; never a zero stand-in."""
    contributing: list[str] = []
    excluded: list[dict[str, str]] = []
    for entry in entries:
        bound, reason = _is_bound(entry)
        if bound:
            contributing.append(entry["run_id"])
        else:
            excluded.append({"run_id": entry["run_id"], "reason": reason})
    contributing_ids = set(contributing)
    counters: dict[str, dict[str, Any]] = {}
    for name in COUNTERS:
        total = 0
        measured = 0
        for entry in entries:
            if entry["run_id"] not in contributing_ids:
                continue
            value = entry["counters"][name]
            if value is None:
                continue
            total += value
            measured += 1
        if measured == 0:
            counters[name] = {
                "status": "NOT_MEASURED",
                "reason": NO_CONTRIBUTING_RUNS if not contributing else NO_MEASURED_COUNTER,
            }
        else:
            counters[name] = {"status": "OBSERVED", "value": total, "runs": measured}
    return {
        "ledger_version": LEDGER_VERSION,
        "entries_total": len(entries),
        "contributing": contributing,
        "excluded": excluded,
        "runs": ({"status": "OBSERVED", "value": len(contributing)}
                 if contributing else
                 {"status": "NOT_MEASURED", "reason": NO_CONTRIBUTING_RUNS}),
        "counters": counters,
    }


# --------------------------------------------------------------------------- cli


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    ledger_commands = commands.add_parser("ledger", help="append-only run ledger")
    ledger_commands = ledger_commands.add_subparsers(dest="ledger_operation", required=True)
    append_parser = ledger_commands.add_parser("append", help="append one validated run entry")
    append_parser.add_argument("--ledger", required=True)
    append_parser.add_argument("--entry", required=True)
    verify_parser = ledger_commands.add_parser("verify", help="verify every per-run digest")
    verify_parser.add_argument("--ledger", required=True)
    summary_parser = commands.add_parser("summary", help="aggregate with provenance")
    summary_parser.add_argument("--ledger", required=True)
    args = parser.parse_args(argv)
    try:
        if args.operation == "ledger" and args.ledger_operation == "append":
            entry = contracts.load_document(args.entry, MAX_ENTRY_BYTES)
            appended = append_entry(args.ledger, entry)
            result = {"ok": True, **appended}
        elif args.operation == "ledger":
            verified = verify_ledger(args.ledger)
            result = {"ok": True, **verified}
        else:
            entries = load_ledger(args.ledger)
            result = {"ok": True, **summarise(entries)}
        print(json.dumps(result, separators=(",", ":")))
        return 0
    except LedgerError as error:
        payload = {"ok": False, "error": error.code}
        if error.line is not None:
            payload["line"] = error.line
        print(json.dumps(payload, separators=(",", ":")))
    except contracts.ContractError as error:
        print(json.dumps({"ok": False, "error": error.errors[0]["code"]},
                         separators=(",", ":")))
    except (OSError, ValueError, RuntimeError):
        print('{"ok":false,"error":"READ_FAILED"}')
    return 1


if __name__ == "__main__":
    sys.exit(main())
