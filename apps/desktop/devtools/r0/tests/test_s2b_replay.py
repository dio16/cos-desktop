"""Hermetic tests for the full-source replay and published-set audit path.

No test in this module claims a Native observation, a displayed frame or an
accepted measurement. The retained-bundle and published-set cases run only when
their inputs exist in this checkout; when they are absent the case is skipped
with `NOT_RUN`, which is fewer tests executed rather than success.
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parents[3]
sys.path.insert(0, str(ROOT))
import s2b_contracts as s2b  # noqa: E402
import s2b_replay  # noqa: E402

BUNDLE = REPO / "docs/development/evidence/r0a-s2b/native-bundles.zip"
PUBLISHED = REPO / "docs/development/evidence-published/r0a-capture-latest"
WINDOWS = os.name == "nt"


def digest(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def identity_for(files: dict[str, bytes]) -> dict:
    """Same construction as `s2b_contracts.source_digest`, from in-memory bytes."""
    aggregate = hashlib.sha256()
    recorded = {}
    for name in sorted(s2b.SOURCE_FILES):
        raw = files[name]
        normalized = raw.replace(b"\r\n", b"\n")
        aggregate.update(name.encode() + b"\0")
        aggregate.update(normalized)
        recorded[name] = digest(normalized)
    return {"record_version": 1,
            "observer_source_sha256": "sha256:" + aggregate.hexdigest(),
            "files": recorded}


def source_bodies() -> dict[str, bytes]:
    bodies = {name: f"// fixture body for {name}\nfn main() {{}}\n".encode()
              for name in s2b.SOURCE_FILES}
    # One CRLF body proves the recorded identity uses the normalised bytes.
    first = sorted(s2b.SOURCE_FILES)[0]
    bodies[first] = b"// crlf fixture\r\nfn main() {}\r\n"
    return bodies


def git(directory: Path, *arguments: str) -> str:
    result = subprocess.run(["git", "-C", str(directory), *arguments],
                            capture_output=True, check=False, timeout=30)
    if result.returncode != 0:
        raise AssertionError(result.stderr.decode("utf-8", "replace"))
    return result.stdout.decode("utf-8", "replace").strip()


def make_repository(directory: Path, bodies: dict[str, bytes]) -> str:
    for name, data in bodies.items():
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    git(directory, "init", "-q")
    git(directory, "-c", "core.autocrlf=false", "add", "-A")
    git(directory, "-c", "user.name=t", "-c", "user.email=t@example.invalid",
        "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture")
    return git(directory, "rev-parse", "HEAD")


def write_tree(directory: Path, files: dict[str, bytes]) -> Path:
    for name, data in files.items():
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    return directory


def index(files: dict[str, bytes]) -> bytes:
    return json.dumps({"record_version": 1,
                       "files": {name: digest(data) for name, data in files.items()}},
                      sort_keys=True).encode() + b"\n"


@unittest.skipUnless(WINDOWS, "the shared Pin reader requires Windows")
class BundleIntegrity(unittest.TestCase):
    def reject(self, task, code: str) -> None:
        with self.assertRaises(s2b.ObservationError) as error:
            task()
        self.assertEqual(str(error.exception), code)

    def test_member_names_refuse_traversal_absolute_and_ambiguous_shapes(self):
        for good in ("n02d/ledger.json", "n04b/producer/r0-s2b.json",
                     "build-record.json", "source-identity.json"):
            self.assertEqual(s2b_replay.bundle_member(good), good)
        for bad in ("", "/n02d/a", "n02d/../a", "n02d/./a", "n02d\\a", "C:/n02d/a",
                    "n02d/a.", "n02d/a ", "digests.json", "unknown/a", "n02d//a",
                    "n02d/" + "a" * 600, "n02d/a\x00b", 7, None):
            self.reject(lambda bad=bad: s2b_replay.bundle_member(bad), "BUNDLE_MEMBER")

    def test_index_must_cover_every_file_member_exactly_once(self):
        with tempfile.TemporaryDirectory() as work:
            root = write_tree(Path(work), {"n02d/a.json": b"a", "n02d/b.json": b"b"})
            (root / "digests.json").write_bytes(index({"n02d/a.json": b"a"}))
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST_SET")
            (root / "digests.json").write_bytes(
                index({"n02d/a.json": b"a", "n02d/b.json": b"b", "n02d/c.json": b"c"}))
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST_SET")
            (root / "digests.json").write_bytes(
                index({"n02d/a.json": b"a", "n02d/b.json": b"b"}))
            self.assertEqual(s2b_replay.verify_digests(root)["entries"], 2)

    def test_commit_marker_directories_are_bound_to_the_empty_digest(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            marker_name = "n02d/committed-" + "a" * 64
            marker = root / marker_name
            marker.mkdir(parents=True)
            value = {"record_version": 1, "files": {marker_name: s2b_replay.EMPTY_DIGEST}}
            (root / "digests.json").write_text(json.dumps(value), encoding="utf-8")
            self.assertEqual(s2b_replay.verify_digests(root)["entries"], 1)
            value["files"][marker_name] = digest(b"not empty")
            (root / "digests.json").write_text(json.dumps(value), encoding="utf-8")
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST")
            value["files"][marker_name] = s2b_replay.EMPTY_DIGEST
            (root / "digests.json").write_text(json.dumps(value), encoding="utf-8")
            (marker / "payload").write_bytes(b"hidden")
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST")
            (marker / "payload").unlink()
            (root / "digests.json").write_text(
                json.dumps({"record_version": 1, "files": {
                    marker_name: s2b_replay.EMPTY_DIGEST,
                    "n02d/0.json": s2b_replay.EMPTY_DIGEST}}), encoding="utf-8")
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST_SET")

    def test_index_mismatch_is_a_closed_refusal(self):
        with tempfile.TemporaryDirectory() as work:
            root = write_tree(Path(work), {"n02d/a.json": b"stored"})
            value = {"record_version": 1, "files": {"n02d/a.json": "sha256:" + "0" * 64}}
            (root / "digests.json").write_text(json.dumps(value), encoding="utf-8")
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST")

    def test_producer_relocation_is_bound_not_guessed(self):
        with tempfile.TemporaryDirectory() as work:
            root = write_tree(Path(work), {"producer/n02d/r0-s2b.json": b"producer"})
            value = {"record_version": 1,
                     "files": {"n02d/producer/r0-s2b.json": digest(b"producer")}}
            (root / "digests.json").write_text(json.dumps(value), encoding="utf-8")
            self.assertEqual(s2b_replay.verify_digests(root)["entries"], 1)
            value["files"]["n02d/producer/r0-s2b.json"] = digest(b"forged")
            (root / "digests.json").write_text(json.dumps(value), encoding="utf-8")
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST")
            # A relocation mismatch in the other direction is not silently mapped.
            (root / "digests.json").write_text(json.dumps(
                {"record_version": 1, "files": {"n02d/r0-s2b.json": digest(b"producer")}}),
                encoding="utf-8")
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGEST_SET")

    def test_index_duplicate_nonfinite_and_version_are_rejected(self):
        with tempfile.TemporaryDirectory() as work:
            root = write_tree(Path(work), {"n02d/a.json": b"a"})
            (root / "digests.json").write_bytes(
                b'{"record_version":1,"files":{"n02d/a.json":"' + digest(b"a").encode()
                + b'","n02d/a.json":"' + digest(b"a").encode() + b'"}}')
            self.reject(lambda: s2b_replay.verify_digests(root), "DUPLICATE_KEY")
            (root / "digests.json").write_bytes(
                b'{"record_version":1,"files":{"n02d/a.json":NaN}}')
            self.reject(lambda: s2b_replay.verify_digests(root), "NON_FINITE_NUMBER")
            (root / "digests.json").write_bytes(
                b'{"record_version":2,"files":{}}')
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_DIGESTS")
            (root / "digests.json").write_bytes(b'{"record_version":1}')
            self.reject(lambda: s2b_replay.verify_digests(root), "SCHEMA")

    def test_entry_and_size_limits_are_enforced(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            files = {f"n02d/{index}.json": b"x" for index in range(257)}
            write_tree(root, files)
            (root / "digests.json").write_bytes(index(files))
            self.reject(lambda: s2b_replay.verify_digests(root), "BUNDLE_ENTRIES")
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            big = {"n02d/big.bin": b"x" * (s2b_replay.MAX_BUNDLE_FILE_BYTES + 1)}
            write_tree(root, big)
            (root / "digests.json").write_bytes(index(big))
            self.reject(lambda: s2b_replay.verify_digests(root), "ARTIFACT_SIZE")


@unittest.skipUnless(WINDOWS, "the shared Pin reader requires Windows")
class SourceTreeBinding(unittest.TestCase):
    def test_recorded_identity_is_recomputable_from_the_declared_commit(self):
        with tempfile.TemporaryDirectory() as work:
            bodies = source_bodies()
            commit = make_repository(Path(work), bodies)
            identity = identity_for(bodies)
            status = s2b_replay.source_tree_status(Path(work), identity, commit)
            self.assertEqual(status["status"], "VERIFIED")
            self.assertEqual(status["files"], len(s2b.SOURCE_FILES))
            self.assertEqual(status["aggregate"], identity["observer_source_sha256"])
            self.assertEqual(
                s2b_replay.require_source_tree(Path(work), identity, commit)["status"],
                "VERIFIED")

    def test_a_different_commit_does_not_reproduce_the_recorded_identity(self):
        with tempfile.TemporaryDirectory() as work:
            directory = Path(work)
            bodies = source_bodies()
            first = make_repository(directory, bodies)
            identity = identity_for(bodies)
            changed = sorted(s2b.SOURCE_FILES)[1]
            (directory / changed).write_bytes(b"// changed source\n")
            git(directory, "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                "-c", "commit.gpgsign=false", "commit", "-q", "-am", "change")
            second = git(directory, "rev-parse", "HEAD")
            self.assertNotEqual(first, second)
            status = s2b_replay.source_tree_status(directory, identity, second)
            self.assertEqual(status["status"], "MISMATCH")
            self.assertEqual(status["mismatched"], [changed])
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.require_source_tree(directory, identity, second)
            self.assertEqual(str(error.exception), "SOURCE_TREE_MISMATCH")

    def test_an_unavailable_commit_is_a_closed_refusal_not_a_pass(self):
        with tempfile.TemporaryDirectory() as work:
            directory = Path(work)
            bodies = source_bodies()
            make_repository(directory, bodies)
            identity = identity_for(bodies)
            status = s2b_replay.source_tree_status(directory, identity, "0" * 40)
            self.assertEqual(status["status"], "UNAVAILABLE")
            self.assertEqual(status["reason"], "SOURCE_COMMIT_UNAVAILABLE")
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.require_source_tree(directory, identity, "0" * 40)
            self.assertEqual(str(error.exception), "SOURCE_COMMIT_UNAVAILABLE")

    def test_a_deleted_source_file_cannot_be_bound(self):
        with tempfile.TemporaryDirectory() as work:
            directory = Path(work)
            bodies = source_bodies()
            make_repository(directory, bodies)
            identity = identity_for(bodies)
            removed = sorted(s2b.SOURCE_FILES)[2]
            (directory / removed).unlink()
            git(directory, "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                "-c", "commit.gpgsign=false", "commit", "-q", "-am", "delete")
            head = git(directory, "rev-parse", "HEAD")
            status = s2b_replay.source_tree_status(directory, identity, head)
            self.assertEqual(status["status"], "UNAVAILABLE")
            self.assertEqual(status["reason"], "SOURCE_FILE_MISSING")


@unittest.skipUnless(WINDOWS, "the shared Pin reader requires Windows")
class RootBinding(unittest.TestCase):
    SOURCE = "sha256:" + "a" * 64
    COMMIT = "b" * 40
    BASE_COMMIT = "c" * 40
    EXE = "sha256:" + "d" * 64
    BASE_EXE = "sha256:" + "e" * 64

    def documents(self) -> tuple[dict, dict, dict, dict]:
        identity = {"record_version": 1, "observer_source_sha256": self.SOURCE,
                    "files": {name: "sha256:" + "f" * 64 for name in s2b.SOURCE_FILES}}
        manifest = {"identity": {"driver_hash": self.SOURCE,
                                 "repository": {"sha": self.COMMIT},
                                 "executable_sha256": self.EXE,
                                 "build": {"features": [s2b.FEATURE], "profile": "dev"}}}
        default = {"identity": {"driver_hash": self.SOURCE,
                                "repository": {"sha": self.BASE_COMMIT},
                                "executable_sha256": self.BASE_EXE,
                                "build": {"features": [], "profile": "dev"}}}
        build = {"source_commit": self.COMMIT, "baseline_source_commit": self.BASE_COMMIT,
                 "executable_sha256": self.EXE,
                 "baseline_executable_sha256": self.BASE_EXE,
                 "observer_source_sha256": self.SOURCE}
        return identity, manifest, default, build

    def tree(self, work: Path, identity: dict, build: dict) -> None:
        raw_identity = json.dumps(identity, sort_keys=True).encode()
        raw_build = json.dumps(build, sort_keys=True).encode()
        write_tree(work, {"source-identity.json": raw_identity,
                          "build-record.json": raw_build,
                          "n02d/source-identity.json": raw_identity,
                          "n02d/build-record.json": raw_build})

    def test_root_and_tag_copies_must_agree(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            identity, manifest, default, build = self.documents()
            self.tree(root, identity, build)
            self.assertEqual(s2b_replay.verify_root_binding(
                root, "n02d", identity, manifest, default, build)["status"], "VERIFIED")
            other = dict(build, source_commit="d" * 40)
            (root / "build-record.json").write_bytes(json.dumps(other).encode())
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.verify_root_binding(root, "n02d", identity, manifest, default, build)
            self.assertEqual(str(error.exception), "BUNDLE_ROOT")

    def test_default_manifest_must_name_the_recorded_baseline(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            identity, manifest, default, build = self.documents()
            self.tree(root, identity, build)
            wrong = json.loads(json.dumps(default))
            wrong["identity"]["repository"]["sha"] = "e" * 40
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.verify_root_binding(root, "n02d", identity, manifest, wrong, build)
            self.assertEqual(str(error.exception), "BUNDLE_ROOT")
            wrong = json.loads(json.dumps(default))
            wrong["identity"]["build"]["features"] = [s2b.FEATURE]
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.verify_root_binding(root, "n02d", identity, manifest, wrong, build)
            self.assertEqual(str(error.exception), "BUNDLE_ROOT")

    def test_manifest_build_record_and_source_identity_must_agree(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            identity, manifest, default, build = self.documents()
            self.tree(root, identity, build)
            wrong = json.loads(json.dumps(manifest))
            wrong["identity"]["driver_hash"] = "sha256:" + "2" * 64
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.verify_root_binding(root, "n02d", identity, wrong, default, build)
            self.assertEqual(str(error.exception), "SOURCE_IDENTITY")
            wrong = json.loads(json.dumps(manifest))
            wrong["identity"]["repository"]["sha"] = "f" * 40
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.verify_root_binding(root, "n02d", identity, wrong, default, build)
            self.assertEqual(str(error.exception), "BUILD_IDENTITY")


@unittest.skipUnless(WINDOWS, "the shared Pin reader requires Windows")
class ReplayCommandLine(unittest.TestCase):
    def invoke(self, *arguments: str, cwd: str | None = None) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-B", str(ROOT / "s2b_replay.py"), *arguments],
            capture_output=True, cwd=cwd, check=False, timeout=120)

    def parse(self, result: subprocess.CompletedProcess) -> dict:
        lines = result.stdout.decode("utf-8").splitlines()
        self.assertEqual(len(lines), 1)
        return json.loads(lines[0])

    def test_relative_archive_and_unknown_tag_are_closed(self):
        result = self.invoke("bundle", "--bundle-zip", "relative.zip",
                             "--bundle-tag", "n02d", "--timeout-ms", "1000")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.parse(result)["error"], "BUNDLE_PATH")
        with tempfile.TemporaryDirectory() as work:
            archive = Path(work) / "bundle.zip"
            archive.write_bytes(b"not read: the tag is refused first")
            result = self.invoke("bundle", "--bundle-zip", str(archive),
                                 "--bundle-tag", "n99z", "--timeout-ms", "1000")
            self.assertEqual(result.returncode, 1)
            self.assertEqual(self.parse(result)["error"], "BUNDLE_TAG")
            result = self.invoke("bundle", "--bundle-zip", str(archive),
                                 "--bundle-tag", "n02d", "--timeout-ms", "0")
            self.assertEqual(self.parse(result)["error"], "BUNDLE_TIMEOUT")

    def test_truncated_bundle_is_one_closed_code(self):
        with tempfile.TemporaryDirectory() as work:
            archive = Path(work) / "bundle.zip"
            with zipfile.ZipFile(archive, "w") as handle:
                handle.writestr("n02d/ledger.json", b"{}")
                handle.writestr("digests.json", b"{}")
            raw = archive.read_bytes()
            archive.write_bytes(raw[: len(raw) // 2])
            result = self.invoke("bundle", "--bundle-zip", str(archive),
                                 "--bundle-tag", "n02d", "--timeout-ms", "1000")
            self.assertEqual(result.returncode, 1)
            self.assertEqual(self.parse(result)["error"], "ZIP_INVALID")

    def test_audit_prefix_and_set_directory_are_closed(self):
        result = self.invoke("audit", "--set-dir", "does-not-exist", "--prefix", "a1-0c6")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.parse(result)["error"], "SET_PATH")


@unittest.skipUnless(WINDOWS, "the shared Pin reader requires Windows")
@unittest.skipUnless(BUNDLE.is_file(), "NOT_RUN: the retained S2B bundle is absent")
class RetainedBundle(unittest.TestCase):
    def test_full_source_replay_reproduces_the_recorded_identity(self):
        report = s2b_replay.replay_bundle(str(BUNDLE), "n02d", 120_000)
        self.assertTrue(report["ok"])
        self.assertEqual(report["scope"], "SOURCE_BOUND_REPLAY")
        self.assertEqual(report["bundle"]["integrity"]["status"], "VERIFIED")
        self.assertEqual(report["bundle"]["integrity"]["entries"], 35)
        self.assertEqual(report["retained"], 256)
        self.assertEqual(report["disposition"], "CANCELLED")
        self.assertEqual(report["disposition_reason"], "CLOSED_BEFORE_DECLARED_DURATION")
        # The retained replay record predates the scope-based cancellation rule.
        self.assertEqual(report["retained_replay"]["status"], "DIVERGED")
        self.assertFalse(report["full_coverage"])
        # Measured before this suite existed: the declared commit `edfebdaf` is in
        # this checkout and reproduces the recorded identity for all ten files.
        self.assertEqual(report["source_tree"]["status"], "VERIFIED")
        self.assertEqual(report["source_tree"]["files"], len(s2b.SOURCE_FILES))
        self.assertTrue(report["source_bound"])
        self.assertEqual(report["verdict"], "VERIFIED")
        self.assertEqual(report["unverified"], [])

    def test_n04b_tag_is_replayed_from_the_same_archive(self):
        report = s2b_replay.replay_bundle(str(BUNDLE), "n04b", 120_000)
        self.assertEqual(report["scope"], "SOURCE_BOUND_REPLAY")
        self.assertEqual(report["source_tree"]["status"], "VERIFIED")
        self.assertEqual(report["retained"], 34)
        self.assertEqual(report["retention_dropped"], 0)


@unittest.skipUnless(WINDOWS, "the shared Pin reader requires Windows")
@unittest.skipUnless(PUBLISHED.is_dir(), "NOT_RUN: the published capture set is absent")
class PublishedSetAudit(unittest.TestCase):
    PREFIX = "a1-0c6"

    def copy_set(self, work: Path) -> Path:
        target = work / "set"
        target.mkdir()
        for path in PUBLISHED.iterdir():
            if path.is_file():
                shutil.copyfile(path, target / path.name)
        return target

    def test_a_sanitised_set_is_not_verified_and_says_what_is_missing(self):
        report = s2b_replay.audit_set(PUBLISHED, self.PREFIX)
        self.assertTrue(report["ok"])
        self.assertEqual(report["scope"], "PUBLISHED_SET_AUDIT")
        self.assertEqual(report["record_block"], "wm_close_runs/local-accounting-0c6")
        self.assertEqual(report["verdict"], "NOT_VERIFIED")
        self.assertEqual(report["source_tree"]["status"], "VERIFIED")
        for name in ("commit-markers", "executable-bytes", "helper-bytes",
                     "fixture-bytes", "source-files-as-bytes", "feature-manifest",
                     "observation-endpoints-legacy-version"):
            self.assertIn(name, report["unverified"])
        self.assertIn("published:ledger.json", report["verified"])
        self.assertIn("source-tree", report["verified"])
        # The tracked build record is stored LF while its recorded digest is the
        # documented working-tree CRLF form; only that recomputation is accepted.
        self.assertEqual(report["crlf_restored"], ["build-record.json"])
        self.assertEqual(report["digest_forms"]["build-record.json"],
                         "DOCUMENTED_CRLF_RESTORATION")
        self.assertFalse(report["full_coverage"])

    def test_tampered_published_bytes_are_refused_against_the_tracked_record(self):
        with tempfile.TemporaryDirectory() as work:
            target = self.copy_set(Path(work))
            capture = target / f"{self.PREFIX}-s2b-capture.json"
            raw = bytearray(capture.read_bytes())
            raw[len(raw) // 2] ^= 1
            capture.write_bytes(bytes(raw))
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.audit_set(target, self.PREFIX)
            self.assertEqual(str(error.exception), "PUBLISHED_DIGEST")

    def test_a_missing_required_artifact_is_not_silently_unverified(self):
        with tempfile.TemporaryDirectory() as work:
            target = self.copy_set(Path(work))
            (target / f"{self.PREFIX}-build-record.json").unlink()
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.audit_set(target, self.PREFIX)
            self.assertEqual(str(error.exception), "PUBLISHED_MISSING")

    def test_a_forged_declared_commit_is_refused_by_the_source_binding(self):
        with tempfile.TemporaryDirectory() as work:
            target = self.copy_set(Path(work))
            head = git(REPO, "rev-parse", "HEAD")
            build_path = target / f"{self.PREFIX}-build-record.json"
            observation_path = target / f"{self.PREFIX}-s2b-observation.json"
            build = json.loads(build_path.read_bytes())
            build["source_commit"] = head
            build_path.write_bytes(json.dumps(build, indent=2).encode() + b"\n")
            observation = json.loads(observation_path.read_bytes())
            observation["identity"]["source_commit"] = head
            observation["identity"]["build_sha256"] = digest(build_path.read_bytes())
            observation_path.write_bytes(json.dumps(observation, indent=2).encode() + b"\n")
            record_path = target / "record.json"
            record = json.loads(record_path.read_bytes())
            ledger_digest = digest((target / f"{self.PREFIX}-ledger.json").read_bytes())
            matched = 0
            for _, block in s2b_replay.flatten_record(record):
                if block["published"].get("ledger.json") == ledger_digest:
                    block["published"]["build-record.json"] = digest(build_path.read_bytes())
                    block["published"]["s2b-observation.json"] = digest(
                        observation_path.read_bytes())
                    matched += 1
            self.assertEqual(matched, 1)
            record_path.write_bytes(json.dumps(record, indent=2).encode() + b"\n")
            report = s2b_replay.audit_set(target, self.PREFIX)
            self.assertFalse(report["ok"])
            self.assertEqual(report["verdict"], "REFUSED")
            self.assertEqual(report["refusals"], ["SOURCE_TREE_MISMATCH"])
            self.assertEqual(report["source_tree"]["status"], "MISMATCH")
            self.assertTrue(report["source_tree"]["mismatched"])

    def test_a_stale_ledger_binary_identity_is_refused(self):
        with tempfile.TemporaryDirectory() as work:
            target = self.copy_set(Path(work))
            ledger_path = target / f"{self.PREFIX}-ledger.json"
            old = digest(ledger_path.read_bytes())
            ledger = json.loads(ledger_path.read_bytes())
            ledger["executable_sha256"] = "sha256:" + "0" * 64
            ledger_path.write_bytes(json.dumps(ledger, indent=2).encode() + b"\n")
            record_path = target / "record.json"
            record = json.loads(record_path.read_bytes())
            matched = 0
            for _, block in s2b_replay.flatten_record(record):
                if block["published"].get("ledger.json") == old:
                    block["published"]["ledger.json"] = digest(ledger_path.read_bytes())
                    matched += 1
            self.assertEqual(matched, 1)
            record_path.write_bytes(json.dumps(record, indent=2).encode() + b"\n")
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_replay.audit_set(target, self.PREFIX)
            self.assertEqual(str(error.exception), "IDENTITY_MISMATCH")


if __name__ == "__main__":
    unittest.main()
