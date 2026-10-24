"""MP-R0A-S3: import a canonical S2 result into the append-only run ledger.

Python standard library only. The adapter is a *minimal boundary*: it validates the S2
result with the existing `contracts` loader rules, binds its identity, maps only the
fields that have a real counterpart onto S3 counters, and appends one ledger entry per
run through `s3_ledger.append_entry` (so the chain, the per-run digest, the duplicate
refusal and the caps are exactly the ledger's). It performs no measurement and executes
no value from the document.

RECONCILIATION TABLE (S2 result -> S3 counters). Matched means the S2 field is the same
observation; unmatched means no counterpart exists, and the S3 counter is recorded as
`null` (explicitly not measured) rather than 0 or a substituted metric:

    S3 counter        S2 source                      state
    frames_observed   runs[].frames.processed        matched
    dirty_frames      --                              UNMATCHED (no S2 dirty-frame counter)
    present_calls     runs[].frames.present          matched
    scope_ns          --                              UNMATCHED (S2 records no scope duration)

S2 fields with no S3 counterpart are recorded as unmatched, never folded into a counter:
`frames.{requested,submitted,displayed,missed,dropped,stalled,eligibility,idle,rule,
reason}`, `resources.{process,media,gpu,devices,sampling}`, `endpoints`, `metrics`,
`instrumentation`, `failures`, `outliers`, `evidence`, `run.{pair_id,variant,reason}` and
`manifest.{scenario_version,benchmark_revision,definition_hash}` (the manifest fields are
verified for identity, not counted).

IDENTITY. A run entry is only *contributing* when the source and the candidate are bound:
`git_sha` and `worktree_digest` come from the retained bundle (`build-record.json`'s
`baseline_source_commit` and a digest over its recorded source file set), `program_digest`
from `build-record.json`'s `executable_sha256`. If the caller cannot supply them the entry
is still appended with `null`s, so the run is traceable and excluded with
`SOURCE_NOT_BOUND`/`CANDIDATE_NOT_BOUND` instead of being silently dropped.

OUTCOME. The S2 status is propagated verbatim (OBSERVED, NOT_MEASURED, NOT_IMPLEMENTED,
FAILED) and nothing else; the S3 vocabulary's CANCELLED has no S2 producer and is never
invented here.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from typing import Any

import contracts
import s3_ledger as ledger

IMPORT_VERSION = 1

# S3 counter -> S2 path. `None` is an explicit "no counterpart": the counter is `null`.
RECONCILIATION: dict[str, str | None] = {
    "frames_observed": "frames.processed",
    "dirty_frames": None,
    "present_calls": "frames.present",
    "scope_ns": None,
}

# S2 fields that exist but have no S3 counterpart. They are reported, never imported.
UNMATCHED_S2_FIELDS = (
    "frames.requested",
    "frames.submitted",
    "frames.displayed",
    "frames.missed",
    "frames.dropped",
    "frames.stalled",
    "frames.eligibility",
    "frames.idle",
    "frames.rule",
    "frames.reason",
    "resources.process",
    "resources.media",
    "resources.gpu",
    "resources.devices",
    "resources.sampling",
    "endpoints",
    "metrics",
    "instrumentation",
    "failures",
    "outliers",
    "evidence",
    "run.pair_id",
    "run.variant",
    "run.reason",
    "manifest.scenario_version",
    "manifest.benchmark_revision",
    "manifest.definition_hash",
)

IMPORT_ERRORS = (
    "IMPORT_SCHEMA",
    "IMPORT_IDENTITY",
    "IMPORT_RUN_ID",
    "IMPORT_EMPTY",
    "IMPORT_RESULT_VERSION",
)

SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA = re.compile(r"^[0-9a-f]{40}([0-9a-f]{24})?$")


class ImportError_(Exception):
    """One closed public code; never input text, a path or a traceback."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def fail(code: str) -> None:
    assert code in IMPORT_ERRORS
    raise ImportError_(code)


# --------------------------------------------------------------------------- mapping


def _lookup(run: dict[str, Any], path: str) -> Any:
    value: Any = run
    for part in path.split("."):
        value = value[part]
    return value


def counters_for(run: dict[str, Any]) -> dict[str, int | None]:
    """Map one S2 run onto the S3 counter vocabulary; unmatched counters stay `null`."""
    counters: dict[str, int | None] = {}
    for name in ledger.COUNTERS:
        source = RECONCILIATION[name]
        if source is None:
            counters[name] = None
            continue
        value = _lookup(run, source)
        # The S2 contract already guarantees an integer or null; null is "not measured",
        # never 0.
        counters[name] = None if value is None else int(value)
    return counters


def reconciliation_report() -> dict[str, Any]:
    """The A1 table, as data so a test can assert it instead of trusting the docstring."""
    return {
        "counters": {name: {"source": source,
                            "state": "matched" if source else "UNMATCHED"}
                     for name, source in RECONCILIATION.items()},
        "unmatched_s2_fields": list(UNMATCHED_S2_FIELDS),
    }


def run_id_for(run: dict[str, Any]) -> str:
    """A ledger run id that is unique per result and traceable back to the S2 run."""
    candidate = f"{run['run_id']}.{run['variant']}"
    if not SAFE_ID.fullmatch(candidate):
        fail("IMPORT_RUN_ID")
    return candidate


# --------------------------------------------------------------------------- identity


def canonical_digest(value: Any) -> str:
    body = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return "sha256:" + hashlib.sha256(body.encode()).hexdigest()


def source_identity_from_build_record(build_record: dict[str, Any]) -> dict[str, str | None]:
    """Bind the ledger identity to what the retained bundle actually recorded.

    `git_sha` is the recorded baseline commit; `worktree_digest` is the digest of the
    recorded source file set (`files` in `source-identity.json`), so the same bundle always
    yields the same identity and a different source set cannot borrow it.
    """
    commit = build_record.get("baseline_source_commit")
    git_sha = commit if isinstance(commit, str) and GIT_SHA.fullmatch(commit) else None
    files = build_record.get("files")
    worktree = canonical_digest(files) if files is not None else None
    return {"git_sha": git_sha, "worktree_digest": worktree}


def program_digest_from_build_record(build_record: dict[str, Any]) -> str | None:
    executable = build_record.get("executable_sha256")
    if not isinstance(executable, str):
        return None
    return executable if executable.startswith("sha256:") else f"sha256:{executable}"


# --------------------------------------------------------------------------- import


def bodies_for_result(
    result: Any,
    *,
    source: dict[str, str | None] | None = None,
    program_digest: str | None = None,
    expected_manifest: dict[str, str] | None = None,
    host: dict[str, str] | None = None,
    interpreter: dict[str, str] | None = None,
) -> list[dict[str, Any]]:
    """Validate a canonical S2 result and turn every run into an S3 entry body."""
    try:
        document = contracts.validate_result(result)
    except contracts.ContractError as error:
        code = error.errors[0]["code"]
        fail("IMPORT_RESULT_VERSION" if code == "UNSUPPORTED_VERSION" else "IMPORT_SCHEMA")
    if expected_manifest is not None:
        for key in ("scenario_id", "scenario_version", "benchmark_revision",
                    "definition_hash"):
            if document["manifest"][key] != expected_manifest.get(key):
                # Result-to-manifest identity is checked before anything is written.
                fail("IMPORT_IDENTITY")
    runs = document["runs"]
    if not runs:
        fail("IMPORT_EMPTY")
    bodies: list[dict[str, Any]] = []
    seen: set[str] = set()
    for run in runs:
        run_id = run_id_for(run)
        if run_id in seen:
            fail("IMPORT_RUN_ID")
        seen.add(run_id)
        bodies.append({
            "record": ledger.RECORD_RUN,
            "entry_version": ledger.ENTRY_VERSION,
            "run_id": run_id,
            "source": source or {"git_sha": None, "worktree_digest": None},
            "candidate": {"id": "s2-candidate", "program_digest": program_digest},
            "host": host or {"os": "windows", "arch": "x86_64"},
            "interpreter": interpreter or {"name": "cpython", "version": "3.13.14"},
            "start_utc": "1970-01-01T00:00:00Z",
            "end_utc": "1970-01-01T00:00:00Z",
            "outcome": run["status"],
            "counters": counters_for(run),
        })
    return bodies


def import_result(path: str, ledger_path: str, **kwargs: Any) -> dict[str, Any]:
    """Append every run of one S2 result; nothing is written when validation fails."""
    document = contracts.load_document(path, contracts.MAX_RESULT_BYTES)
    bodies = bodies_for_result(document, **kwargs)
    appended = []
    for body in bodies:
        appended.append(ledger.append_entry(ledger_path, body))
    entries = ledger.load_ledger(ledger_path)
    return {
        "ok": True,
        "import_version": IMPORT_VERSION,
        "runs": appended,
        "ledger_digest": ledger.chain_head(entries),
        "reconciliation": reconciliation_report(),
    }


# --------------------------------------------------------------------------- cli


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    import_parser = commands.add_parser("import", help="import one S2 result into a ledger")
    import_parser.add_argument("--result", required=True)
    import_parser.add_argument("--ledger", required=True)
    import_parser.add_argument("--git-sha")
    import_parser.add_argument("--worktree-digest")
    import_parser.add_argument("--program-digest")
    import_parser.add_argument("--definition-hash")
    import_parser.add_argument("--scenario-id")
    import_parser.add_argument("--scenario-version")
    import_parser.add_argument("--benchmark-revision")
    args = parser.parse_args(argv)
    try:
        source = {"git_sha": args.git_sha, "worktree_digest": args.worktree_digest}
        expected = None
        if args.definition_hash or args.scenario_id:
            expected = {
                "scenario_id": args.scenario_id,
                "scenario_version": args.scenario_version,
                "benchmark_revision": args.benchmark_revision,
                "definition_hash": args.definition_hash,
            }
        result = import_result(
            args.result,
            args.ledger,
            source=source,
            program_digest=args.program_digest,
            expected_manifest=expected,
        )
        print(json.dumps(result, separators=(",", ":")))
        return 0
    except ImportError_ as error:
        print(json.dumps({"ok": False, "error": error.code}, separators=(",", ":")))
    except ledger.LedgerError as error:
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
