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

IDENTITY. A run entry is only *contributing* when the source and the candidate are bound.
The operational input is the retained record: `--bundle` reads `build-record.json`'s
`baseline_source_commit` and `executable_sha256` plus `source-identity.json`'s file set, so
the identity is a property of the retained artifact, not of caller text. Free-form
`--git-sha/--worktree-digest/--program-digest` are accepted only with the explicit
`--allow-caller-identity` acknowledgement and are reported as caller-provided. Without
either, the CLI refuses with `IMPORT_IDENTITY`. When a field is not taken from a record the
entry body uses defaults (`candidate.id = "s2-candidate"`, `host = windows/x86_64`,
interpreter `cpython 3.13.14`, both UTC timestamps `1970-01-01T00:00:00Z`); the import
response records those placeholders and the identity origin, and unbound runs are excluded
with `SOURCE_NOT_BOUND`/`CANDIDATE_NOT_BOUND` instead of being silently dropped.

RESULT BINDING. With `--bundle`, the identity proof also binds the result: the tag's own
`result.json` is read from the bundle and the supplied `--result` must have the same
canonical digest, otherwise the import is refused with `IMPORT_IDENTITY` before any
write. A tag whose bundle carries no `result.json` cannot prove a separate file, so it is
refused unless `--allow-caller-identity` explicitly falls back to caller-provided identity
(that fallback is visibly distinct: `candidate.id = "s2-candidate-caller"`, origin
`caller`, no `record_sha256`).

BUNDLE BOUNDS. Every retained-bundle read is bounded: at most 16 MiB for the file, 1 MiB
per member we open, and 256 members in the archive. A violation is refused with the named
codes `IMPORT_BUNDLE_LIMIT` / `IMPORT_BUNDLE_UNREADABLE` before anything is written.

PROVENANCE CONVENTION. The ledger entry schema is fixed, so the bundle digest is not
embedded in the entry; it is reported in the import response as `identity.record_sha256`
and must be retained alongside the ledger (this is the documented convention). Record-bound
entries use `candidate.id = "s2-candidate"`; caller-acknowledged imports use
`s2-candidate-caller`, which a later ledger reader can distinguish.

SCENARIO BINDING. The four manifest expectations (`--scenario-id`, `--scenario-version`,
`--benchmark-revision`, `--definition-hash`) must be supplied together or not at all; a
partial set is refused with `IMPORT_IDENTITY`. Supplying none is a refusal unless
`--allow-unbound-scenario` explicitly acknowledges the caller obligation, in which case the
response records `manifest_binding = "NOT_ASSERTED"`; supplying all four refuses any result
whose manifest does not match before anything is written.

COUNTER SEMANTICS. The S3 counter definition for `frames_observed` is not present in this
repository, so the matched mapping to S2 `frames.processed` carries the semantic risk
recorded in `COUNTER_SEMANTICS` (a processed frame need not be a frame the renderer
observed); it is a declared equivalence, not a proven one.

OUTCOME. The S2 status is propagated verbatim (OBSERVED, NOT_MEASURED, NOT_IMPLEMENTED,
FAILED) and nothing else; the S3 vocabulary's CANCELLED has no S2 producer and is never
invented here.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import sys
import zipfile
from pathlib import Path
from typing import Any, NoReturn

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
    "IMPORT_BUNDLE_LIMIT",
    "IMPORT_BUNDLE_UNREADABLE",
)

# Bounds for every retained-bundle read: the whole file, each member we open, and the
# number of members the archive may contain. A named code is reported, never a traceback.
MAX_BUNDLE_BYTES = 16 * 1024 * 1024
MAX_BUNDLE_MEMBER_BYTES = 1 * 1024 * 1024
MAX_BUNDLE_MEMBERS = 256

# Caller-acknowledged imports are marked in the ledger body so a later reader can tell
# them apart from record-bound imports; the entry schema itself stays unchanged.
CALLER_CANDIDATE_ID = "s2-candidate-caller"

SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA = re.compile(r"^[0-9a-f]{40}([0-9a-f]{24})?$")


class S3ImportError(Exception):
    """One closed public code; never input text, a path or a traceback."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def fail(code: str) -> NoReturn:
    assert code in IMPORT_ERRORS
    raise S3ImportError(code)


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


# No S3 counter definition for `frames_observed` exists in this repository: the "matched"
# mapping is asserted by this adapter, and its semantic risk is recorded as data rather
# than hidden in the docstring.
COUNTER_SEMANTICS: dict[str, dict[str, str]] = {
    "frames_observed": {
        "s2_source": "runs[].frames.processed",
        "state": "matched (semantic risk recorded)",
        "risk": ("no S3 counter definition is present in this repository; S2 "
                 "frames.processed may include frames the renderer did not observe "
                 "(skipped or duplicated), so this is a declared wire-level mapping, "
                 "not a proven S3 semantic equivalence"),
    },
    "present_calls": {
        "s2_source": "runs[].frames.present",
        "state": "matched (same definition gap)",
        "risk": ("no S3 counter definition is present in this repository; the mapping "
                 "to the S2 present count is asserted, not proven"),
    },
}


def reconciliation_report() -> dict[str, Any]:
    """The A1 table, as data so a test can assert it instead of trusting the docstring."""
    return {
        "counters": {name: {"source": source,
                            "state": "matched" if source else "UNMATCHED"}
                     for name, source in RECONCILIATION.items()},
        "unmatched_s2_fields": list(UNMATCHED_S2_FIELDS),
        "counter_semantics": COUNTER_SEMANTICS,
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
    candidate_id: str = "s2-candidate",
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
            "candidate": {"id": candidate_id, "program_digest": program_digest},
            "host": host or {"os": "windows", "arch": "x86_64"},
            "interpreter": interpreter or {"name": "cpython", "version": "3.13.14"},
            "start_utc": "1970-01-01T00:00:00Z",
            "end_utc": "1970-01-01T00:00:00Z",
            "outcome": run["status"],
            "counters": counters_for(run),
        })
    return bodies


def import_result(
    path: str,
    ledger_path: str,
    *,
    source: dict[str, str | None] | None = None,
    program_digest: str | None = None,
    build_record: dict[str, Any] | None = None,
    build_files: dict[str, str] | None = None,
    expected_manifest: dict[str, str] | None = None,
    host: dict[str, str] | None = None,
    interpreter: dict[str, str] | None = None,
    record_sha256: str | None = None,
    expected_result_sha256: str | None = None,
    candidate_id: str = "s2-candidate",
) -> dict[str, Any]:
    """Append every run of one S2 result; nothing is written when validation fails.

    With `build_record`, source and program identity come from that retained record and a
    conflicting explicit value is refused. With `expected_result_sha256` the loaded result
    must match that canonical digest, so a bundle can bind both identity and result. Without
    a record the identity is caller-provided, the response marks it as such, and
    `candidate_id` can mark the ledger body (the CLI uses "s2-candidate-caller"). The whole
    batch is validated by `s3_ledger.append_entries` before the first byte is written.
    """
    if not SAFE_ID.fullmatch(candidate_id):
        fail("IMPORT_IDENTITY")
    origin = "caller"
    if build_record is not None:
        recorded = source_identity_from_build_record(
            {**build_record, **({"files": build_files} if build_files is not None else {})})
        recorded_program = program_digest_from_build_record(build_record)
        if source is not None and source != recorded:
            fail("IMPORT_IDENTITY")
        if program_digest is not None and program_digest != recorded_program:
            fail("IMPORT_IDENTITY")
        source, program_digest = recorded, recorded_program
        origin = "build-record"
    document = contracts.load_document(path, contracts.MAX_RESULT_BYTES)
    if expected_result_sha256 is not None and \
            canonical_digest(document) != expected_result_sha256:
        # The result is part of the retained-record proof: a different (even schema-valid)
        # result must never be imported under that record's identity.
        fail("IMPORT_IDENTITY")
    bodies = bodies_for_result(document, source=source, program_digest=program_digest,
                               expected_manifest=expected_manifest, host=host,
                               interpreter=interpreter, candidate_id=candidate_id)
    appended = ledger.append_entries(ledger_path, bodies)
    entries = ledger.load_ledger(ledger_path)
    identity = {
        "origin": origin,
        "candidate_id": candidate_id,
        "placeholders": ["candidate.id", "host", "interpreter", "start_utc", "end_utc"],
        "note": ("candidate.id, host, interpreter and the UTC timestamps are defaults/"
                 "placeholders unless a retained record supplies them; a run is "
                 "contributing only when the source and program digests are bound"),
    }
    if record_sha256 is not None:
        identity["record_sha256"] = record_sha256
    return {
        "ok": True,
        "import_version": IMPORT_VERSION,
        "runs": appended,
        "ledger_digest": ledger.chain_head(entries),
        "identity": identity,
        "manifest_binding": "ASSERTED" if expected_manifest is not None else "NOT_ASSERTED",
        "reconciliation": reconciliation_report(),
    }


# --------------------------------------------------------------------------- cli


def _bundle_member(archive: zipfile.ZipFile, name: str,
                   *, required: bool = True) -> bytes | None:
    """Read one archive member within the per-member bound; closed codes only."""
    try:
        info = archive.getinfo(name)
    except KeyError:
        if required:
            fail("IMPORT_BUNDLE_UNREADABLE")
        return None
    if info.file_size > MAX_BUNDLE_MEMBER_BYTES:
        fail("IMPORT_BUNDLE_LIMIT")
    with archive.open(name) as handle:
        data = handle.read(MAX_BUNDLE_MEMBER_BYTES + 1)
    if len(data) > MAX_BUNDLE_MEMBER_BYTES:
        fail("IMPORT_BUNDLE_LIMIT")
    return data


def _bundle_identity(
    bundle: str, tag: str
) -> tuple[dict[str, Any], dict[str, str], str, dict[str, Any] | None]:
    """Read the retained bundle bounded: build record, source set, digest and its result.

    Returns the bundle digest and the tag's own `result.json` (None when the tag carries
    none) so the caller can bind the supplied result to the retained record.
    """
    try:
        size = Path(bundle).stat().st_size
    except OSError:
        fail("IMPORT_BUNDLE_UNREADABLE")
    if size > MAX_BUNDLE_BYTES:
        fail("IMPORT_BUNDLE_LIMIT")
    try:
        with open(bundle, "rb") as handle:
            data = handle.read(MAX_BUNDLE_BYTES + 1)
    except OSError:
        fail("IMPORT_BUNDLE_UNREADABLE")
    if len(data) > MAX_BUNDLE_BYTES:
        fail("IMPORT_BUNDLE_LIMIT")
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    try:
        archive = zipfile.ZipFile(io.BytesIO(data))
    except zipfile.BadZipFile:
        fail("IMPORT_BUNDLE_UNREADABLE")
    with archive:
        if len(archive.infolist()) > MAX_BUNDLE_MEMBERS:
            fail("IMPORT_BUNDLE_LIMIT")
        build_raw = _bundle_member(archive, f"{tag}/build-record.json")
        identity_raw = _bundle_member(archive, f"{tag}/source-identity.json")
        result_raw = _bundle_member(archive, f"{tag}/result.json", required=False)
        try:
            build = json.loads(build_raw)
            identity = json.loads(identity_raw)
            result = None if result_raw is None else json.loads(result_raw)
        except ValueError:
            fail("IMPORT_BUNDLE_UNREADABLE")
    if not isinstance(build, dict) or not isinstance(identity, dict):
        fail("IMPORT_BUNDLE_UNREADABLE")
    files = identity.get("files")
    if not isinstance(files, dict) or not files:
        fail("IMPORT_BUNDLE_UNREADABLE")
    return build, files, digest, result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=("Caller obligation: identity comes from the retained bundle "
                "(--bundle) unless --allow-caller-identity is given, and the four "
                "manifest expectations must be supplied together or explicitly "
                "waived with --allow-unbound-scenario."),
    )
    commands = parser.add_subparsers(dest="operation", required=True)
    import_parser = commands.add_parser("import", help="import one S2 result into a ledger")
    import_parser.add_argument("--result", required=True)
    import_parser.add_argument("--ledger", required=True)
    import_parser.add_argument("--bundle",
                               help="retained S2 bundle zip; identity comes from its "
                                    "build-record.json and source-identity.json")
    import_parser.add_argument("--bundle-tag", default="n02d")
    import_parser.add_argument("--allow-caller-identity", action="store_true",
                               help="acknowledge that the free-form identity flags below "
                                    "are caller-provided, not from a retained record")
    import_parser.add_argument("--git-sha")
    import_parser.add_argument("--worktree-digest")
    import_parser.add_argument("--program-digest")
    import_parser.add_argument("--definition-hash")
    import_parser.add_argument("--scenario-id")
    import_parser.add_argument("--scenario-version")
    import_parser.add_argument("--benchmark-revision")
    import_parser.add_argument("--allow-unbound-scenario", action="store_true",
                               help="acknowledge that the scenario is not bound to an "
                                    "expected manifest; the response records NOT_ASSERTED")
    args = parser.parse_args(argv)
    try:
        identity_flags = (args.git_sha, args.worktree_digest, args.program_digest)
        manifest_values = (args.scenario_id, args.scenario_version,
                           args.benchmark_revision, args.definition_hash)
        build_record = build_files = record_sha = None
        expected_result_digest = None
        candidate_id = "s2-candidate"
        if args.bundle:
            build_record, build_files, record_sha, bundle_result = _bundle_identity(
                args.bundle, args.bundle_tag)
            if bundle_result is None:
                # No retained result means no result proof: a separate file is trusted
                # only after the caller explicitly acknowledges caller identity.
                if not args.allow_caller_identity:
                    fail("IMPORT_IDENTITY")
                build_record = build_files = record_sha = None
                candidate_id = CALLER_CANDIDATE_ID
            else:
                if any(identity_flags) or args.allow_caller_identity:
                    fail("IMPORT_IDENTITY")
                expected_result_digest = canonical_digest(bundle_result)
        elif not args.allow_caller_identity:
            fail("IMPORT_IDENTITY")
        if build_record is None and args.allow_caller_identity:
            candidate_id = CALLER_CANDIDATE_ID
        if any(manifest_values) and not all(manifest_values):
            fail("IMPORT_IDENTITY")
        if not any(manifest_values) and not args.allow_unbound_scenario:
            fail("IMPORT_IDENTITY")
        expected = None
        if all(manifest_values):
            expected = {
                "scenario_id": args.scenario_id,
                "scenario_version": args.scenario_version,
                "benchmark_revision": args.benchmark_revision,
                "definition_hash": args.definition_hash,
            }
        result = import_result(
            args.result,
            args.ledger,
            source=None if build_record is not None else {
                "git_sha": args.git_sha, "worktree_digest": args.worktree_digest},
            program_digest=None if build_record is not None else args.program_digest,
            build_record=build_record,
            build_files=build_files,
            expected_manifest=expected,
            record_sha256=record_sha,
            expected_result_sha256=expected_result_digest,
            candidate_id=candidate_id,
        )
        print(json.dumps(result, separators=(",", ":")))
        return 0
    except S3ImportError as error:
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
