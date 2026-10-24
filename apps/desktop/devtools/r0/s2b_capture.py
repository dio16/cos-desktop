"""Approved S2A launch plus separate passive S2B capture/publication.

The wrapper neither changes S2A's three-artifact completion nor fabricates visual
observations in its S1 result. No additional child arguments or environment pass
through this interface. All outputs are fixed names under the owned run root.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import contracts
import driver
import s2b_contracts as s2b
from windows_owner import Pin

BUILD_FILE = Path(__file__).with_name("s2b-build.json")


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def read_pinned(path: Path, limit: int) -> bytes:
    with Pin(path) as pin:
        if pin.file is None or os.fstat(pin.file.fileno()).st_size > limit:
            s2b.fail("ARTIFACT_SIZE")
        value = pin.file.read(limit + 1)
        if len(value) > limit:
            s2b.fail("ARTIFACT_SIZE")
        pin.verify()
        return value


def require_marker(root: Path, name: str) -> None:
    """Commit marker is an empty directory, or an empty file in byte-bound archives."""
    target = root / name
    if target.is_dir():
        with Pin(target, directory=True) as pin:
            if any(target.iterdir()):
                s2b.fail("COMMIT_NOT_EMPTY")
            pin.verify()
        return
    with Pin(target) as pin:
        if pin.file is None or os.fstat(pin.file.fileno()).st_size != 0:
            s2b.fail("COMMIT_INVALID")
        pin.verify()


ARTIFACT_FILES = {
    "completion.json": driver.MAX_LOG_BYTES,
    "ledger.json": driver.MAX_LOG_BYTES,
    "result.json": driver.MAX_LOG_BYTES,
    "endpoint-availability.json": driver.MAX_LOG_BYTES,
    "run-id": 64,
    "s2b-capture.json": s2b.MAX_BYTES,
    "s2b-observation.json": contracts.MAX_MANIFEST_BYTES,
    "source-identity.json": contracts.MAX_MANIFEST_BYTES,
    "build-record.json": contracts.MAX_MANIFEST_BYTES,
}
TOTAL_COPY_BYTES = 6 * 1024 * 1024
MAX_MARKERS = 8
MAX_ENTRIES = 32
MARKER = re.compile(r"^(?:r0-s2b-committed-|s2b-committed-|committed-)[0-9a-f]{64}$")


def materialized_copy(output: Path, allowed_markers: set[str]) -> tuple[Path, str]:
    """Closed, bounded copy: only known artifacts plus exact commit-marker names.

    Every file is read once through a Pin that rejects reparse points and wrong
    types; unknown entries, links, oversized files and excessive totals fail.
    """
    import tempfile
    directory = tempfile.mkdtemp(prefix="s2b-replay-")
    try:
        copy = Path(directory) / "output"
        copy.mkdir()
        total = 0
        markers = 0
        entries = list(output.iterdir())
        if len(entries) > MAX_ENTRIES:
            s2b.fail("ENTRY_LIMIT")
        for entry in entries:
            name = entry.name
            if MARKER.fullmatch(name):
                if name not in allowed_markers:
                    s2b.fail("MARKER_UNEXPECTED")
                markers += 1
                if markers > MAX_MARKERS or markers > len(allowed_markers):
                    s2b.fail("MARKER_LIMIT")
                if entry.is_dir():
                    with Pin(entry, directory=True) as pin:
                        if any(entry.iterdir()):
                            s2b.fail("COMMIT_NOT_EMPTY")
                        pin.verify()
                else:
                    with Pin(entry) as pin:
                        if pin.file is None or os.fstat(pin.file.fileno()).st_size != 0:
                            s2b.fail("COMMIT_INVALID")
                        pin.verify()
                (copy / name).mkdir()
                continue
            limit = ARTIFACT_FILES.get(name)
            if limit is None:
                s2b.fail("UNEXPECTED_ENTRY")
            with Pin(entry) as pin:
                if pin.file is None:
                    s2b.fail("ARTIFACT_TYPE")
                size = os.fstat(pin.file.fileno()).st_size
                if size > limit or total + size > TOTAL_COPY_BYTES:
                    s2b.fail("ARTIFACT_SIZE")
                data = pin.file.read(limit + 1)
                if len(data) > limit:
                    s2b.fail("ARTIFACT_SIZE")
                pin.verify()
            total += len(data)
            (copy / name).write_bytes(data)
    except BaseException:
        import shutil
        shutil.rmtree(directory, ignore_errors=True)
        raise
    return copy, directory


def validate_build_record(document: object, *, live: bool) -> dict:
    value = s2b.object_fields(document, {
        "build_version", "source_commit", "observer_source_sha256", "executable_sha256",
        "baseline_source_commit", "baseline_executable_sha256", "feature", "gpui_pin",
        "target", "rustc", "cargo",
    })
    if type(value["build_version"]) is not int or value["build_version"] != 1:
        s2b.fail("BUILD_VERSION")
    for field in ("source_commit", "baseline_source_commit"):
        if not isinstance(value[field], str) or not contracts.GIT_SHA.fullmatch(value[field]):
            s2b.fail("SOURCE_IDENTITY")
    # A baseline/feature pair may share one product-source commit when the two builds
    # differ only by feature and executable digest (the 2026-10-25 baseline line does);
    # the identical-binary rule below still keeps the pair distinguishable.
    for key in ("observer_source_sha256", "executable_sha256", "baseline_executable_sha256"):
        s2b.digest(value[key])
    if ((live and value["observer_source_sha256"] != s2b.source_digest())
            or value["feature"] != s2b.FEATURE
            or value["gpui_pin"] != s2b.GPUI_PIN
            or value["target"] != "x86_64-pc-windows-msvc"
            or value["rustc"] != "1.98.1" or value["cargo"] != "1.98.1"
            or value["executable_sha256"] == value["baseline_executable_sha256"]):
        s2b.fail("BUILD_IDENTITY")
    return value


def build_record() -> tuple[dict, str]:
    """Fresh candidate validation only; replay uses the recorded bundle copy."""
    value = validate_build_record(contracts.load_document(str(BUILD_FILE), contracts.MAX_MANIFEST_BYTES), live=True)
    return value, driver.sha256_file(BUILD_FILE)


def approval_entry(manifest: dict) -> dict:
    """Replay lookup: bind only the manifest's own recorded identity to approvals."""
    identity = manifest["identity"]
    approved = driver.approved_digests()
    # Select by commit *and* executable digest: one source line may approve several
    # build variants (default/feature), so the commit alone is not unique.
    entries = [
        entry for entry in approved
        if entry["product_commit"] == identity["repository"]["sha"]
        and entry["executable_sha256"] == identity["executable_sha256"]
    ]
    if len(entries) != 1:
        s2b.fail("APPROVAL")
    return entries[0]


def binding(manifest: dict) -> dict:
    build, _ = build_record()
    identity = manifest["identity"]
    if (identity["repository"]["sha"] != build["source_commit"]
            or identity["executable_sha256"] != build["executable_sha256"]):
        s2b.fail("BUILD_IDENTITY")
    approved = driver.approved_digests()
    entries = [
        entry for entry in approved
        if entry["product_commit"] == build["source_commit"]
        and entry["executable_sha256"] == build["executable_sha256"]
    ]
    baseline = [
        entry for entry in approved
        if entry["product_commit"] == build["baseline_source_commit"]
        and entry["executable_sha256"] == build["baseline_executable_sha256"]
    ]
    if len(entries) != 1 or len(baseline) != 1:
        s2b.fail("APPROVAL")
    if any(entries[0][field] != baseline[0][field] for field in ("helper_sha256", "helper_source_sha256")):
        s2b.fail("HELPER_IDENTITY")
    return entries[0]


def require_source_agreement(recorded_build: dict, identity: dict) -> None:
    if recorded_build["observer_source_sha256"] != identity["observer_source_sha256"]:
        s2b.fail("SOURCE_IDENTITY")


def observation_document(manifest: dict, manifest_digest: str, build_digest: str, observer_source: str,
                         entry: dict, capture: dict, capture_bytes: bytes, output: Path) -> dict:
    # All identity content is closed technical data; never copy machine metadata,
    # fixture contents, file paths, input text, Task IDs or arbitrary labels.
    return {
        "observation_version": 1,
        "identity": {
            "source_commit": manifest["identity"]["repository"]["sha"],
            "manifest_sha256": manifest_digest,
            "build_sha256": build_digest,
            "observer_source_sha256": observer_source,
            "executable_sha256": entry["executable_sha256"],
            "helper_sha256": entry["helper_sha256"],
            "helper_source_sha256": entry["helper_source_sha256"],
            "fixture_sha256": manifest["workload"]["source"]["sha256"],
            "execution_id": capture["execution_id"], "pid": capture["pid"],
            "gpui_pin": s2b.GPUI_PIN, "feature": s2b.FEATURE,
        },
        "capture": {"name": s2b.FILE_NAME, "bytes": len(capture_bytes), "sha256": sha(capture_bytes)},
        "s2a": {name: sha(read_pinned(output / name, driver.MAX_LOG_BYTES))
                for name in ("ledger.json", "result.json", "completion.json")},
        "endpoints": s2b.endpoint_matrix(capture),
        "coverage": "LIMITED_SOURCE_OBSERVATIONS_ONLY",
        "frame_identity": "NOT_MEASURED",
        "cross_domain_latency": "NOT_MEASURED",
        "loss_scope": "OFFERED_FINALIZED_JOURNAL_PLUS_LOCAL_QUEUE",
        "unobserved_source_loss": "UNKNOWN",
    }


def verify_bundle(manifest_path: Path, output: Path, timeout_ms: int) -> dict:
    """Public independent replay, including unchanged S1/S2A semantic validation."""
    with Pin(manifest_path), Pin(output, directory=True) as root:
        with Pin(output / "source-identity.json") as identity_pin:
            if identity_pin.file is None:
                s2b.fail("ARTIFACT_SIZE")
            identity_bytes = identity_pin.file.read(contracts.MAX_MANIFEST_BYTES + 1)
            if len(identity_bytes) > contracts.MAX_MANIFEST_BYTES:
                s2b.fail("ARTIFACT_SIZE")
            identity = s2b.validate_source_identity(s2b.parse_bytes(identity_bytes, contracts.MAX_MANIFEST_BYTES))
            identity_pin.verify()
        manifest = s2b.validate_manifest(
            contracts.load_document(str(manifest_path), contracts.MAX_MANIFEST_BYTES),
            expected_driver_hash=identity["observer_source_sha256"],
        )
        entry = approval_entry(manifest)
        with Pin(output / "build-record.json") as build_pin:
            if build_pin.file is None:
                s2b.fail("ARTIFACT_SIZE")
            build_bytes = build_pin.file.read(contracts.MAX_MANIFEST_BYTES + 1)
            if len(build_bytes) > contracts.MAX_MANIFEST_BYTES:
                s2b.fail("ARTIFACT_SIZE")
            recorded_build = validate_build_record(s2b.parse_bytes(build_bytes, contracts.MAX_MANIFEST_BYTES), live=False)
            build_pin.verify()
        if (recorded_build["source_commit"] != manifest["identity"]["repository"]["sha"]
                or recorded_build["executable_sha256"] != entry["executable_sha256"]):
            s2b.fail("BUILD_IDENTITY")
        require_source_agreement(recorded_build, identity)
        build_digest = sha(build_bytes)
        completion_bytes = read_pinned(output / "completion.json", driver.MAX_LOG_BYTES)
        capture_bytes = read_pinned(output / s2b.FILE_NAME, s2b.MAX_BYTES)
        observation_bytes = read_pinned(output / s2b.OBSERVATION_NAME, contracts.MAX_MANIFEST_BYTES)
        allowed = {
            "committed-" + hashlib.sha256(completion_bytes).hexdigest(),
            "r0-s2b-committed-" + hashlib.sha256(capture_bytes).hexdigest(),
            "s2b-committed-" + hashlib.sha256(observation_bytes).hexdigest(),
        }
        copy, directory = materialized_copy(output, allowed)
        try:
            completion = driver.verify_publication(copy, manifest, timeout_ms)
        finally:
            import shutil
            shutil.rmtree(directory, ignore_errors=True)
        ledger = driver.parse_message(read_pinned(output / "ledger.json", driver.MAX_LOG_BYTES))
        # verify_publication already validated the complete ledger, S1 result and
        # pair identity; a failed/censored run is preserved, not upgraded to PASS.
        # The pre-read bytes are the only source for parsing, digests and markers.
        capture = s2b.validate_capture(
            s2b.parse_bytes(capture_bytes, s2b.MAX_BYTES),
            execution=completion["execution_id"], pid=ledger["pid"],
            executable=entry["executable_sha256"],
        )
        require_marker(output, "r0-s2b-committed-" + hashlib.sha256(capture_bytes).hexdigest())
        actual = s2b.parse_bytes(observation_bytes, contracts.MAX_MANIFEST_BYTES)
        expected = observation_document(manifest, driver.sha256_file(manifest_path), build_digest,
                                        identity["observer_source_sha256"], entry, capture, capture_bytes, output)
        if json.dumps(actual, sort_keys=True, separators=(",", ":")) != json.dumps(expected, sort_keys=True, separators=(",", ":")):
            s2b.fail("OBSERVATION_BINDING")
        require_marker(output, "s2b-committed-" + hashlib.sha256(observation_bytes).hexdigest())
        root.verify()
    detail = s2b.disposition_detail(
        capture, ledger, duration_ms=manifest["workload"]["duration_ms"])
    return {
        "ok": True, "scope": "SOURCE_BOUND_REPLAY",
        "execution_id": capture["execution_id"],
        "disposition": detail["status"],
        "disposition_reason": detail["reason"],
        "retained": capture["summary"]["retained"],
        "retention_dropped": capture["summary"]["retention_dropped"],
        "endpoints": expected["endpoints"],
        "full_coverage": False,
    }


def discard_partial(paths: list[Path]) -> None:
    """Remove S2B artifacts written by a refused publication, newest first.

    The S2A ledger/result/completion stay driver-owned and are never touched here; only
    the files this adapter created before a validation failure are removed, so a refused
    capture leaves no partially published S2B artifact.
    """
    for path in reversed(paths):
        try:
            if path.is_dir():
                path.rmdir()
            else:
                path.unlink(missing_ok=True)
        except OSError:
            pass


def launch(args: argparse.Namespace) -> int:
    manifest_path, output, profile = Path(args.manifest), Path(args.output_dir), Path(args.profile)
    with Pin(manifest_path), Pin(output, directory=True) as root, \
            Pin(profile, directory=True) as profile_pin, Pin(profile / "temp", directory=True) as temp_pin:
        manifest = s2b.validate_manifest(contracts.load_document(str(manifest_path), contracts.MAX_MANIFEST_BYTES))
        entry = binding(manifest)
        # No output is inserted before S2A's empty-directory/profile admission.
        driver.launch(args)
        driver.verify_publication(output, manifest, args.timeout_ms)
        ledger = driver.parse_message(read_pinned(output / "ledger.json", driver.MAX_LOG_BYTES))
        source = profile / "temp" / "r0-s2b.json"
        with Pin(source):
            raw = read_pinned(source, s2b.MAX_BYTES)
            capture = s2b.validate_capture(contracts.load_document(str(source), s2b.MAX_BYTES),
                execution=profile.name, pid=ledger["pid"], executable=entry["executable_sha256"])
            producer_marker = "r0-s2b-committed-" + hashlib.sha256(raw).hexdigest()
            require_marker(profile / "temp", producer_marker)
        profile_pin.verify()
        temp_pin.verify()
        root.verify()
        # Publish the S2B artifacts last, and remove every one of them if validation or the
        # bounded completion check fails: a refused capture leaves no partially published
        # S2B artifact behind (the S2A ledger/result/completion stay driver-owned).
        created: list[Path] = []
        try:
            with (output / s2b.FILE_NAME).open("xb") as file:
                file.write(raw)
                file.flush()
                os.fsync(file.fileno())
            created.append(output / s2b.FILE_NAME)
            # Preserve the producer's verified completion for standalone archive replay.
            (output / producer_marker).mkdir()
            created.append(output / producer_marker)
            build_bytes = read_pinned(BUILD_FILE, contracts.MAX_MANIFEST_BYTES)
            with (output / "build-record.json").open("xb") as handle:
                handle.write(build_bytes)
                handle.flush()
                os.fsync(handle.fileno())
            created.append(output / "build-record.json")
            document = observation_document(manifest, driver.sha256_file(manifest_path),
                                            sha(build_bytes), s2b.source_digest(), entry,
                                            capture, raw, output)
            driver.exclusive_write(output / s2b.OBSERVATION_NAME, document)
            created.append(output / s2b.OBSERVATION_NAME)
            driver.exclusive_write(output / "source-identity.json", s2b.source_identity())
            created.append(output / "source-identity.json")
            data = read_pinned(output / s2b.OBSERVATION_NAME, contracts.MAX_MANIFEST_BYTES)
            root.verify()
            (output / ("s2b-committed-" + hashlib.sha256(data).hexdigest())).mkdir()
            created.append(output / ("s2b-committed-" + hashlib.sha256(data).hexdigest()))
            result = verify_bundle(manifest_path, output, args.timeout_ms)
        except BaseException:
            discard_partial(created)
            raise
    print(json.dumps(result, separators=(",", ":")))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="S2B isolated approved capture; fixture mode only")
    for option in ("manifest", "executable", "fixture", "output-dir", "helper", "profile"):
        parser.add_argument("--" + option, required=True)
    parser.add_argument("--timeout-ms", type=int, default=driver.DEFAULT_TIMEOUT_MS)
    return launch(parser.parse_args(argv))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (s2b.ObservationError, contracts.ContractError, RuntimeError, OSError, subprocess.SubprocessError):
        print('{"ok":false,"error":"S2B_CAPTURE_INCOMPLETE"}', file=sys.stderr)
        sys.exit(1)
