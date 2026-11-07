"""Independent full-source replay of a retained S2B bundle, plus published-set audit.

`s2b_capture.verify_bundle` replays one committed capture directory, but it trusts
three things it never checks itself: the bundle's integrity index (`digests.json`),
the bundle-root copies of the manifest/build/source-identity records, and the
recorded source commit. This module adds exactly those bindings, because
"full-source replay" means the recorded source identity must be reproducible from
the recorded commit, not merely present in the same archive.

What is checked here, in order:

1. `digests.json` covers every file member of the archive exactly once and every
   digest is recomputed from the extracted bytes. Producer records live under
   `producer/<tag>/` in the archive while the index names them `<tag>/producer/`;
   that documented relocation is applied, never guessed.
2. The bundle-root `feature-manifest.json`, `default-manifest.json`,
   `build-record.json` and `source-identity.json` agree with the replayed tag's
   copies and with each other.
3. Every file recorded in `source-identity.json` is read from the declared source
   commit (`git cat-file blob`), normalised like `s2b_contracts.source_digest`
   (CRLF -> LF), and the aggregate must reproduce `observer_source_sha256`.
4. Only then does the existing `s2b_capture.verify_bundle` run the unchanged
   S1/S2B contract validation and publish `scope: SOURCE_BOUND_REPLAY`.

`audit` performs the strongest bounded check available for a sanitised published
set (for example `docs/development/evidence-published/r0a-capture-latest/`): it
verifies the published bytes against the tracked `record.json` digests, then binds
capture/ledger/result/completion/observation/build/source-identity and the
source commit. Bindings the published set cannot supply (empty commit-marker
directories, executable/helper/fixture bytes, the original launch declaration)
stay `NOT_VERIFIED`; they never become PASS.

Outcomes are closed: `VERIFIED`, `NOT_VERIFIED`, `REFUSED`, plus fixed error
codes for a refused operation. No aggregation, no percentiles, no Domain
semantics and no Native/display claim lives here.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import contracts
import driver
import s2b_capture
import s2b_contracts as s2b
import s2b_validate
from s2b_capture import read_pinned
from windows_owner import Pin

BUNDLE_TAGS = ("n02d", "n04b")
MAX_OUTPUT_BYTES = 1024 * 1024
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024
MAX_DIGESTS_BYTES = 256 * 1024
MAX_RECORD_BYTES = 1024 * 1024
MAX_BUNDLE_FILE_BYTES = 1024 * 1024
MAX_BUNDLE_TOTAL_BYTES = 6 * 1024 * 1024
MAX_BUNDLE_ENTRIES = 256
MAX_SOURCE_FILE_BYTES = 512 * 1024
MAX_SOURCE_TOTAL_BYTES = 2 * 1024 * 1024
GIT_TIMEOUT_S = 30
AUDIT_CORE = ("ledger.json", "result.json", "completion.json",
              "s2b-capture.json", "s2b-observation.json",
              "build-record.json", "source-identity.json")
AUDIT_OPTIONAL = ("endpoint-availability.json",)
MARKER_PREFIXES = ("committed-", "r0-s2b-committed-", "s2b-committed-")
# Published bindings that a sanitised set cannot carry. They are named in every
# audit result so a reader never mistakes a partial audit for a full replay.
UNAVAILABLE_BINDINGS = (
    "commit-markers", "executable-bytes", "helper-bytes", "fixture-bytes",
    "source-files-as-bytes",
)
# Path separators are '/'; `bundle_member` refuses a literal backslash explicitly
# because the character class also admits it as an escaped literal.
MEMBER = re.compile(r"^[A-Za-z0-9_.\-]+(?:/[A-Za-z0-9_.\-]+)*$")


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


EMPTY_DIGEST = "sha256:" + hashlib.sha256(b"").hexdigest()


def require(condition: bool, code: str) -> None:
    if not condition:
        s2b.fail(code)


def bundle_member(name: object) -> str:
    """A digest-index key: relative, plain-ASCII, no traversal or device shape."""
    if not isinstance(name, str) or not 0 < len(name) <= 512 or not MEMBER.fullmatch(name):
        s2b.fail("BUNDLE_MEMBER")
    if "\\" in name or "\x00" in name:
        s2b.fail("BUNDLE_MEMBER")
    parts = name.split("/")
    if any(part in (".", "..") or part.endswith((".", " ")) for part in parts):
        s2b.fail("BUNDLE_MEMBER")
    if parts[0] in BUNDLE_TAGS:
        return name
    if len(parts) == 1 and parts[0] in s2b_validate.ZIP_FILES and parts[0] != "digests.json":
        return name
    s2b.fail("BUNDLE_MEMBER")


def digest_path(root: Path, name: str) -> Path:
    """Map an indexed name to the extracted layout (producer relocation is fixed)."""
    parts = name.split("/")
    if len(parts) >= 3 and parts[0] in BUNDLE_TAGS and parts[1] == "producer":
        return root.joinpath("producer", parts[0], *parts[2:])
    return root.joinpath(*parts)


def extracted_name(root: Path, path: Path) -> str:
    """Inverse of `digest_path` for the walk that proves exact coverage."""
    relative = path.relative_to(root).as_posix()
    parts = relative.split("/")
    if len(parts) >= 3 and parts[0] == "producer" and parts[1] in BUNDLE_TAGS:
        return f"{parts[1]}/producer/" + "/".join(parts[2:])
    return relative


def verify_digests(root: Path) -> dict:
    """Recompute the retained integrity index over the extracted archive.

    Commit markers are directories, and the index records each with the digest of
    the empty byte string; a reported name may therefore resolve to an empty
    directory. Directory members that the index does not name are structural
    (`n02d/`, `producer/<tag>/`) and are not coverage requirements.
    """
    document = s2b.parse_bytes(read_pinned(root / "digests.json", MAX_DIGESTS_BYTES),
                               MAX_DIGESTS_BYTES)
    value = s2b.object_fields(document, {"record_version", "files"})
    if type(value["record_version"]) is not int or value["record_version"] != 1:
        s2b.fail("BUNDLE_DIGESTS")
    if not isinstance(value["files"], dict):
        s2b.fail("BUNDLE_DIGESTS")
    recorded = {}
    for name, digest in value["files"].items():
        recorded[bundle_member(name)] = s2b.digest(digest)
    observed = set()
    count = 0
    for path in root.rglob("*"):
        count += 1
        if count > MAX_BUNDLE_ENTRIES:
            s2b.fail("BUNDLE_ENTRIES")
        if path.is_dir():
            continue
        name = extracted_name(root, path)
        if name == "digests.json":
            continue
        require(name not in observed, "BUNDLE_DIGEST_SET")
        observed.add(name)
    # Markers are validated before coverage so an unexpected payload inside a
    # marker directory is reported as a marker violation, not as a mere set gap.
    for name in sorted(recorded):
        path = digest_path(root, name)
        if path.is_dir() and (recorded[name] != EMPTY_DIGEST or any(path.iterdir())):
            s2b.fail("BUNDLE_DIGEST")
    if not observed <= set(recorded):
        s2b.fail("BUNDLE_DIGEST_SET")
    total = 0
    for name in sorted(recorded):
        path = digest_path(root, name)
        if path.is_dir():
            continue
        if not path.is_file():
            s2b.fail("BUNDLE_DIGEST_SET")
        data = read_pinned(path, MAX_BUNDLE_FILE_BYTES)
        total += len(data)
        require(total <= MAX_BUNDLE_TOTAL_BYTES, "BUNDLE_SIZE")
        if sha(data) != recorded[name]:
            s2b.fail("BUNDLE_DIGEST")
    return {"status": "VERIFIED", "entries": len(recorded), "bytes": total}


def _git(repo: Path, arguments: list[str]) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            ["git", "-C", str(repo), *arguments],
            capture_output=True, check=False, timeout=GIT_TIMEOUT_S,
        )
    except (OSError, subprocess.SubprocessError):
        s2b.fail("SOURCE_TREE_UNAVAILABLE")


def source_tree(repo: Path, commit: str) -> dict:
    """Recompute the recorded source identity from one commit's immutable blobs."""
    if not isinstance(commit, str) or not contracts.GIT_SHA.fullmatch(commit):
        s2b.fail("SOURCE_COMMIT")
    if _git(repo, ["cat-file", "-e", commit + "^{commit}"]).returncode != 0:
        s2b.fail("SOURCE_COMMIT_UNAVAILABLE")
    aggregate = hashlib.sha256()
    files = {}
    total = 0
    for name in sorted(s2b.SOURCE_FILES):
        result = _git(repo, ["cat-file", "blob", f"{commit}:{name}"])
        if result.returncode != 0:
            s2b.fail("SOURCE_FILE_MISSING")
        if len(result.stdout) > MAX_SOURCE_FILE_BYTES:
            s2b.fail("SOURCE_FILE_SIZE")
        total += len(result.stdout)
        require(total <= MAX_SOURCE_TOTAL_BYTES, "SOURCE_FILE_SIZE")
        normalized = result.stdout.replace(b"\r\n", b"\n")
        aggregate.update(name.encode() + b"\0")
        aggregate.update(normalized)
        files[name] = sha(normalized)
    return {"aggregate": "sha256:" + aggregate.hexdigest(), "files": files, "bytes": total}


def source_tree_status(repo: Path, identity: dict, commit: str) -> dict:
    """Status form used by the audit; unavailability is reported, never raised."""
    try:
        actual = source_tree(repo, commit)
    except s2b.ObservationError as error:
        return {"status": "UNAVAILABLE", "reason": str(error), "commit": commit}
    recorded = identity["files"]
    mismatched = sorted(name for name in recorded if actual["files"].get(name) != recorded[name])
    if mismatched or actual["aggregate"] != identity["observer_source_sha256"]:
        return {"status": "MISMATCH", "commit": commit, "files": len(recorded),
                "mismatched": mismatched, "aggregate": actual["aggregate"]}
    return {"status": "VERIFIED", "commit": commit, "files": len(recorded),
            "mismatched": [], "aggregate": actual["aggregate"]}


def require_source_tree(repo: Path, identity: dict, commit: str) -> dict:
    """A full-source replay refuses rather than replaying an unbindable source."""
    status = source_tree_status(repo, identity, commit)
    if status["status"] == "UNAVAILABLE":
        s2b.fail(status["reason"])
    if status["status"] != "VERIFIED":
        s2b.fail("SOURCE_TREE_MISMATCH")
    return status


def verify_root_binding(root: Path, tag: str, identity: dict, manifest: dict,
                        default: dict, build: dict) -> dict:
    """Bundle-root copies must agree with each other and with the replayed tag."""
    for name in ("source-identity.json", "build-record.json"):
        root_bytes = read_pinned(root / name, contracts.MAX_MANIFEST_BYTES)
        tag_bytes = read_pinned(root / tag / name, contracts.MAX_MANIFEST_BYTES)
        if root_bytes != tag_bytes:
            s2b.fail("BUNDLE_ROOT")
    source = identity["observer_source_sha256"]
    if (manifest["identity"].get("driver_hash") != source
            or default["identity"].get("driver_hash") != source
            or build["observer_source_sha256"] != source):
        s2b.fail("SOURCE_IDENTITY")
    if (manifest["identity"]["repository"]["sha"] != build["source_commit"]
            or manifest["identity"]["executable_sha256"] != build["executable_sha256"]):
        s2b.fail("BUILD_IDENTITY")
    if (default["identity"]["repository"]["sha"] != build["baseline_source_commit"]
            or default["identity"]["build"]["features"] != []
            or default["identity"]["build"]["profile"] != "dev"):
        # The default manifest documents the A0 baseline build of the same source
        # line, not the feature build replayed here, so only its source binding is
        # asserted. Its executable digest is reported by the caller, not accepted.
        s2b.fail("BUNDLE_ROOT")
    return {"status": "VERIFIED", "manifests": 2, "root_copies": 2}


def retained_replay_comparison(root: Path, tag: str, report: dict) -> dict:
    """Compare against a retained replay record without editing or upgrading it."""
    path = root / f"replay-{tag}.json"
    if not path.is_file():
        return {"status": "NOT_RETAINED"}
    recorded = s2b.parse_bytes(read_pinned(path, MAX_DIGESTS_BYTES), MAX_DIGESTS_BYTES)
    if not isinstance(recorded, dict):
        s2b.fail("REPLAY_RECORD")
    fields = ("ok", "scope", "execution_id", "disposition", "retained", "retention_dropped")
    differences = []
    for field in fields:
        if recorded.get(field) != report.get(field):
            differences.append({"field": field, "recorded": recorded.get(field),
                                "replayed": report.get(field)})
    return {"status": "DIVERGED" if differences else "MATCH", "differences": differences}


def replay_bundle(zip_path: str, tag: str, timeout_ms: int,
                  repo_root: Path | None = None) -> dict:
    """Full-source replay of one retained bundle archive, bounded and read-only."""
    if not isinstance(zip_path, str) or not os.path.isabs(zip_path):
        s2b.fail("BUNDLE_PATH")
    require(tag in BUNDLE_TAGS, "BUNDLE_TAG")
    require(type(timeout_ms) is int and 1 <= timeout_ms <= driver.MAX_TIMEOUT_MS, "BUNDLE_TIMEOUT")
    archive = Path(zip_path)
    require(archive.is_file(), "BUNDLE_PATH")
    repo = Path(repo_root) if repo_root is not None else Path(__file__).resolve().parents[4]
    directory = None
    try:
        directory = tempfile.mkdtemp(prefix="s2b-full-replay-")
        root = Path(directory)
        s2b_validate.safe_extract(zip_path, root)
        integrity = verify_digests(root)
        identity = s2b.validate_source_identity(
            s2b.parse_bytes(read_pinned(root / "source-identity.json", contracts.MAX_MANIFEST_BYTES),
                            contracts.MAX_MANIFEST_BYTES))
        manifest = s2b.validate_manifest(
            contracts.load_document(str(root / "feature-manifest.json"), contracts.MAX_MANIFEST_BYTES),
            expected_driver_hash=identity["observer_source_sha256"])
        default = contracts.validate_manifest(
            contracts.load_document(str(root / "default-manifest.json"), contracts.MAX_MANIFEST_BYTES))
        build = s2b_capture.validate_build_record(
            s2b.parse_bytes(read_pinned(root / "build-record.json", contracts.MAX_MANIFEST_BYTES),
                            contracts.MAX_MANIFEST_BYTES), live=False)
        binding = verify_root_binding(root, tag, identity, manifest, default, build)
        tree = source_tree_status(repo, identity, build["source_commit"])
        if tree["status"] == "MISMATCH":
            # A recorded commit that exists but does not reproduce the recorded
            # sources is a forged or stale binding, never a limited success.
            s2b.fail("SOURCE_TREE_MISMATCH")
        report = s2b_capture.verify_bundle(root / "feature-manifest.json", root / tag, timeout_ms)
        retained = retained_replay_comparison(root, tag, report)
        unverified = ([] if tree["status"] == "VERIFIED"
                      else ["source-tree:" + tree["reason"]])
        return {
            "ok": True,
            "scope": report["scope"],
            "verdict": "VERIFIED" if not unverified else "NOT_VERIFIED",
            "source_bound": tree["status"] == "VERIFIED",
            "unverified": unverified,
            "bundle": {"tag": tag, "sha256": file_digest(archive),
                       "bytes": archive.stat().st_size,
                       "integrity": integrity, "binding": binding},
            "source_tree": tree,
            "execution_id": report["execution_id"],
            "disposition": report["disposition"],
            "disposition_reason": report["disposition_reason"],
            "retained": report["retained"],
            "retention_dropped": report["retention_dropped"],
            "endpoints": report["endpoints"],
            "retained_replay": retained,
            "full_coverage": report["full_coverage"],
        }
    finally:
        if directory:
            shutil.rmtree(directory, ignore_errors=True)


def file_digest(path: Path) -> str:
    """Streamed, bounded identity digest; the archive is never read into memory."""
    aggregate = hashlib.sha256()
    total = 0
    with Pin(path) as pin:
        if pin.file is None:
            s2b.fail("BUNDLE_PATH")
        while True:
            chunk = pin.file.read(64 * 1024)
            if not chunk:
                break
            total += len(chunk)
            require(total <= MAX_ARCHIVE_BYTES, "BUNDLE_SIZE")
            aggregate.update(chunk)
        pin.verify()
    return "sha256:" + aggregate.hexdigest()


def audit_names(prefix: str) -> dict:
    """Only the fixed published names; nothing is discovered from the directory."""
    if not isinstance(prefix, str) or not re.fullmatch(r"[0-9a-z][0-9a-z-]{1,40}", prefix):
        s2b.fail("AUDIT_PREFIX")
    return {name: f"{prefix}-{name}" for name in AUDIT_CORE + AUDIT_OPTIONAL}


def flatten_record(record: dict) -> list[tuple[str, dict]]:
    blocks = []
    for key in ("runs", "wm_close_runs", "keyboard_run", "local_tab_run", "failed_attempts"):
        group = record.get(key)
        if not isinstance(group, dict):
            continue
        if isinstance(group.get("published"), dict):
            blocks.append((key, group))
        else:
            for name, block in group.items():
                if isinstance(block, dict) and isinstance(block.get("published"), dict):
                    blocks.append((f"{key}/{name}", block))
    return blocks


def select_record_block(record: dict, ledger_digest: str) -> tuple[str, dict]:
    matches = [(name, block) for name, block in flatten_record(record)
               if block["published"].get("ledger.json") == ledger_digest]
    if not matches:
        s2b.fail("RECORD_MISSING")
    if len(matches) > 1:
        s2b.fail("RECORD_AMBIGUOUS")
    return matches[0]


def evidence_form(raw: bytes, recorded: str) -> str | None:
    """Documented published-byte form, else None.

    `record.json` states that a published text file may legitimately carry the
    working-tree CRLF digest while the git blob keeps LF; `s3_native_replay` already
    accepts exactly that recomputed restoration. Nothing else is accepted.
    """
    if sha(raw) == recorded:
        return "PUBLISHED_BYTES"
    if b"\r" not in raw and sha(raw.replace(b"\n", b"\r\n")) == recorded:
        return "DOCUMENTED_CRLF_RESTORATION"
    return None


def same_bytes(raw: bytes, recorded: object) -> bool:
    return isinstance(recorded, str) and evidence_form(raw, recorded) is not None


def audit_published_digests(block: dict, data: dict[str, bytes]) -> tuple[list[str], dict]:
    """Every core published file must be named in the tracked record with its digest."""
    verified = []
    forms = {}
    published = block["published"]
    for name in AUDIT_CORE:
        if name not in published:
            s2b.fail("RECORD_INCOMPLETE")
        form = evidence_form(data[name], published[name])
        if form is None:
            s2b.fail("PUBLISHED_DIGEST")
        forms[name] = form
        verified.append("published:" + name)
    for name in AUDIT_OPTIONAL:
        if name in published:
            if name not in data:
                s2b.fail("PUBLISHED_MISSING")
            form = evidence_form(data[name], published[name])
            if form is None:
                s2b.fail("PUBLISHED_DIGEST")
            forms[name] = form
            verified.append("published:" + name)
    return verified, forms


def materialize_s1(data: dict[str, bytes]) -> Path:
    """A bounded canonical copy so the unchanged S1 validator can run."""
    directory = Path(tempfile.mkdtemp(prefix="s2b-audit-s1-"))
    try:
        for name in ("completion.json", "ledger.json", "result.json",
                     "endpoint-availability.json"):
            (directory / name).write_bytes(data[name])
    except BaseException:
        shutil.rmtree(directory, ignore_errors=True)
        raise
    return directory


def audit_manifest(manifest_path: Path | None, observation: dict) -> tuple[dict, str | None]:
    """Optional manifest binding; returns (record, reason it is unverified)."""
    if manifest_path is None:
        return {}, "MANIFEST_UNAVAILABLE"
    size = manifest_path.stat().st_size if manifest_path.is_file() else -1
    if not 0 < size <= contracts.MAX_MANIFEST_BYTES:
        s2b.fail("MANIFEST_UNAVAILABLE")
    raw = read_pinned(manifest_path, contracts.MAX_MANIFEST_BYTES)
    if not same_bytes(raw, observation["identity"].get("manifest_sha256")):
        s2b.fail("MANIFEST_DIGEST")
    manifest = s2b.validate_manifest(
        contracts.load_document(str(manifest_path), contracts.MAX_MANIFEST_BYTES),
        expected_driver_hash=observation["identity"].get("observer_source_sha256"))
    return manifest, None


def audit_set(set_dir: Path, prefix: str, record_path: Path | None = None,
              manifest_path: Path | None = None, timeout_ms: int | None = None,
              repo_root: Path | None = None) -> dict:
    """Best bounded audit of a sanitised published S2B set; never a silent PASS."""
    set_dir = set_dir.resolve()
    require(set_dir.is_dir(), "SET_PATH")
    names = audit_names(prefix)
    record_file = (record_path if record_path is not None else set_dir / "record.json").resolve()
    manifest_path = manifest_path.resolve() if manifest_path is not None else None
    repo = Path(repo_root).resolve() if repo_root is not None else Path(__file__).resolve().parents[4]
    record = s2b.parse_bytes(read_pinned(record_file, MAX_RECORD_BYTES), MAX_RECORD_BYTES)
    if not isinstance(record, dict):
        s2b.fail("RECORD_MISSING")
    data = {}
    for name, filename in names.items():
        path = set_dir / filename
        if not path.is_file():
            if name in AUDIT_CORE:
                s2b.fail("PUBLISHED_MISSING")
            continue
        data[name] = read_pinned(path, MAX_BUNDLE_FILE_BYTES)
    block_name, block = select_record_block(record, sha(data["ledger.json"]))
    verified, forms = audit_published_digests(block, data)

    ledger = s2b.parse_bytes(data["ledger.json"], driver.MAX_LOG_BYTES)
    if not isinstance(ledger, dict):
        s2b.fail("SCHEMA")
    execution = ledger.get("execution_id")
    if not isinstance(execution, str) or not re.fullmatch(r"[0-9a-f]{32}", execution):
        s2b.fail("EXECUTION")
    pid = ledger.get("pid")
    executable = ledger.get("executable_sha256")
    s2b.integer(pid, low=1, high=(1 << 32) - 1)
    s2b.digest(executable)
    capture = s2b.validate_capture(
        s2b.parse_bytes(data["s2b-capture.json"], s2b.MAX_BYTES),
        execution=execution, pid=pid, executable=executable)
    identity = s2b.validate_source_identity(
        s2b.parse_bytes(data["source-identity.json"], contracts.MAX_MANIFEST_BYTES))
    build = s2b_capture.validate_build_record(
        s2b.parse_bytes(data["build-record.json"], contracts.MAX_MANIFEST_BYTES), live=False)
    s2b_capture.require_source_agreement(build, identity)

    observation = s2b.parse_bytes(data["s2b-observation.json"], contracts.MAX_MANIFEST_BYTES)
    if not isinstance(observation, dict):
        s2b.fail("SCHEMA")
    version = observation.get("observation_version")
    if type(version) is not int or version not in (1, 2, 3, 4):
        s2b.fail("OBSERVATION_BINDING")
    observed = observation.get("identity")
    if not isinstance(observed, dict):
        s2b.fail("OBSERVATION_BINDING")
    for field in ("source_commit", "manifest_sha256", "build_sha256",
                  "observer_source_sha256", "executable_sha256", "execution_id", "pid"):
        if field not in observed:
            s2b.fail("OBSERVATION_BINDING")
    if (observed["executable_sha256"] != executable
            or observed["executable_sha256"] != build["executable_sha256"]):
        s2b.fail("IDENTITY_MISMATCH")
    if (observed["execution_id"] != execution or observed["pid"] != pid
            or observed["source_commit"] != build["source_commit"]):
        s2b.fail("IDENTITY_MISMATCH")
    if (observed["observer_source_sha256"] != identity["observer_source_sha256"]
            or build["observer_source_sha256"] != identity["observer_source_sha256"]):
        s2b.fail("SOURCE_IDENTITY")
    if observed["build_sha256"] != sha(data["build-record.json"]) and not same_bytes(
            data["build-record.json"], observed["build_sha256"]):
        s2b.fail("BUILD_IDENTITY")
    binding = observation.get("capture")
    s2a = observation.get("s2a")
    if (not isinstance(binding, dict) or binding.get("name") != s2b.FILE_NAME
            or not same_bytes(data["s2b-capture.json"], binding.get("sha256"))
            or binding.get("bytes") != len(data["s2b-capture.json"])):
        s2b.fail("OBSERVATION_BINDING")
    if not isinstance(s2a, dict):
        s2b.fail("OBSERVATION_BINDING")
    for artifact in ("ledger.json", "result.json", "completion.json"):
        if not same_bytes(data[artifact], s2a.get(artifact)):
            s2b.fail("OBSERVATION_BINDING")
    verified.extend(["capture-contract", "source-identity", "build-record",
                     "source-agreement", "executable-identity", "observation-binding"])

    unverified = list(UNAVAILABLE_BINDINGS)
    refusals = []
    if version != 4:
        unverified.append("observation-endpoints-legacy-version")
    else:
        if observation.get("endpoints") != s2b.endpoint_matrix(capture):
            s2b.fail("OBSERVATION_BINDING")
        if observation.get("frame_identity") != s2b_capture.frame_identity_result(capture):
            s2b.fail("OBSERVATION_BINDING")
        verified.append("observation-endpoints")

    manifest, manifest_gap = audit_manifest(manifest_path, observation)
    if manifest_gap:
        unverified.append("feature-manifest")
    else:
        if (manifest["identity"]["executable_sha256"] != executable
                or manifest["identity"]["repository"]["sha"] != build["source_commit"]
                or manifest["workload"]["source"]["sha256"] != ledger.get("fixture_sha256")):
            s2b.fail("MANIFEST_BINDING")
        entry = s2b_capture.approval_entry(manifest)
        if (observed.get("helper_sha256") != entry["helper_sha256"]
                or observed.get("helper_source_sha256") != entry["helper_source_sha256"]
                or observed.get("fixture_sha256") != manifest["workload"]["source"]["sha256"]):
            s2b.fail("HELPER_IDENTITY")
        verified.extend(["feature-manifest", "approval-entry", "helper-identity",
                         "fixture-identity"])

    if manifest and timeout_ms is not None and "endpoint-availability.json" in data:
        require(type(timeout_ms) is int and 1 <= timeout_ms <= driver.MAX_TIMEOUT_MS,
                "AUDIT_TIMEOUT")
        directory = materialize_s1(data)
        try:
            driver.publication_contents(directory, manifest, timeout_ms)
        finally:
            shutil.rmtree(directory, ignore_errors=True)
        verified.append("s1-publication")
    elif manifest:
        unverified.append("original-launch-timeout")

    markers = sorted(name for name in block["published"]
                     if any(name.startswith(head) for head in MARKER_PREFIXES))
    if markers:
        unverified.remove("commit-markers")
        verified.append("commit-markers")
    tree = source_tree_status(repo, identity, build["source_commit"])
    if tree["status"] == "VERIFIED":
        verified.append("source-tree")
    elif tree["status"] == "MISMATCH":
        # The audit still reports what it did verify; the verdict is REFUSED and
        # the exact commit/file mismatch is carried in `source_tree`.
        refusals.append("SOURCE_TREE_MISMATCH")
    else:
        unverified.append("source-tree:" + tree["reason"])

    disposition = s2b.disposition_detail(
        capture, ledger,
        duration_ms=manifest["workload"]["duration_ms"] if manifest else None)
    return {
        "ok": not refusals,
        "scope": "PUBLISHED_SET_AUDIT",
        "set": set_dir.name,
        "prefix": prefix,
        "record_block": block_name,
        "verdict": ("REFUSED" if refusals else
                    "NOT_VERIFIED" if unverified else "VERIFIED"),
        "refusals": refusals,
        "verified": sorted(verified),
        "digest_forms": forms,
        "crlf_restored": sorted(name for name, form in forms.items()
                                if form != "PUBLISHED_BYTES"),
        "unverified": sorted(set(unverified)),
        "source_tree": tree,
        "execution_id": execution,
        "disposition": disposition["status"],
        "disposition_reason": disposition["reason"],
        "retained": capture["summary"]["retained"],
        "retention_dropped": capture["summary"]["retention_dropped"],
        "observation_version": version,
        "full_coverage": False,
    }


def emit(result: dict) -> None:
    text = json.dumps(result, sort_keys=True, separators=(",", ":"))
    if len(text.encode("utf-8")) > MAX_OUTPUT_BYTES:
        s2b.fail("OUTPUT_LIMIT")
    sys.stdout.write(text + "\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    bundle = commands.add_parser("bundle", help="full-source replay of a retained bundle")
    bundle.add_argument("--bundle-zip", required=True)
    bundle.add_argument("--bundle-tag", required=True)
    bundle.add_argument("--timeout-ms", type=int, required=True)
    bundle.add_argument("--repo-root")
    audit = commands.add_parser("audit", help="bounded audit of a sanitised published set")
    audit.add_argument("--set-dir", required=True)
    audit.add_argument("--prefix", required=True)
    audit.add_argument("--record")
    audit.add_argument("--manifest")
    audit.add_argument("--timeout-ms", type=int)
    audit.add_argument("--repo-root")
    args = parser.parse_args(argv)
    try:
        if args.operation == "bundle":
            emit(replay_bundle(args.bundle_zip, args.bundle_tag, args.timeout_ms,
                               Path(args.repo_root) if args.repo_root else None))
        else:
            result = audit_set(Path(args.set_dir), args.prefix,
                               Path(args.record) if args.record else None,
                               Path(args.manifest) if args.manifest else None,
                               args.timeout_ms,
                               Path(args.repo_root) if args.repo_root else None)
            emit(result)
            return 0 if result["verdict"] != "REFUSED" else 1
        return 0
    except s2b.ObservationError as error:
        emit({"ok": False, "error": str(error)})
    except contracts.ContractError as error:
        emit({"ok": False, "error": error.errors[0]["code"]})
    except (RuntimeError, OSError, subprocess.SubprocessError):
        emit({"ok": False, "error": "REPLAY_INCOMPLETE"})
    return 1


if __name__ == "__main__":
    sys.exit(main())
