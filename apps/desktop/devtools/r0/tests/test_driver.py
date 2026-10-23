"""MP-R0A-S2A R5 boundary regressions. Standard library only.

No test-only executable authorization path exists. Offline tests exercise identity,
parsing, ownership and failure behavior; the controlled smoke run exercises the real
approved Desktop through the reviewed helper.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import uuid
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parents[3]
sys.path.insert(0, str(ROOT))

import contracts  # noqa: E402
import driver  # noqa: E402
from windows_owner import Pin  # noqa: E402

PY = sys.executable
MANIFEST = ROOT / "manifests/s2a-lifecycle-v1.json"
FIXTURE = ROOT / "fixtures/result-not-measured-v1.json"
HELPER = REPO / "target/debug/examples/r0_owner.exe"
UNAPPROVED = Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32/where.exe"
SOURCE_FILES = (
    "apps/desktop/examples/r0_owner.rs",
    "apps/desktop/examples/r0_owner/sha256.rs",
    "apps/desktop/examples/r0_owner/ownership.rs",
)


def digest(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def record() -> dict:
    return json.loads((ROOT / "approved-executables.json").read_text(encoding="utf-8"))


def binding() -> dict:
    manifest = contracts.validate_manifest(contracts.load_document(str(MANIFEST), contracts.MAX_MANIFEST_BYTES))
    entries = [e for e in driver.approved_digests() if e["product_commit"] == manifest["identity"]["repository"]["sha"]]
    if len(entries) != 1:
        raise AssertionError("expected exactly one approved binding")
    return entries[0]


def workdir() -> str:
    return tempfile.mkdtemp(prefix="r0a-s2a-", dir="C:/tmp")


def profile_tree(base: Path) -> Path:
    profile = base / uuid.uuid4().hex
    (profile / "roaming").mkdir(parents=True)
    (profile / "temp").mkdir()
    return profile


def invoke(exe: Path, output: Path, fixture: Path = FIXTURE, profile: Path | None = None):
    output.mkdir(parents=True, exist_ok=True)
    profile = profile or profile_tree(output.parent)
    return subprocess.run(
        [
            PY, "-B", str(ROOT / "driver.py"),
            "--manifest", str(MANIFEST),
            "--executable", str(exe),
            "--fixture", str(fixture),
            "--output-dir", str(output),
            "--helper", str(HELPER),
            "--profile", str(profile),
        ],
        capture_output=True, text=True, check=False,
    )


class T02ApprovalRejection(unittest.TestCase):
    def base(self):
        return json.loads(json.dumps(record()))

    def test_unknown_field_duplicate_source_and_bad_digest_are_rejected(self):
        extra = self.base()
        extra["approved"][0]["extra"] = True
        duplicate = self.base()
        duplicate["approved"].append(dict(duplicate["approved"][0]))
        bad = self.base()
        bad["approved"][0]["helper_sha256"] = "sha256:" + "0" * 63
        unknown_protocol = self.base()
        unknown_protocol["approved"][0]["helper_protocol"] = 1
        for document in (extra, duplicate, bad, unknown_protocol):
            with self.assertRaises(RuntimeError):
                driver.validate_approval(document)


class T03LedgerSchema(unittest.TestCase):
    def valid(self):
        entry = binding()
        value = {
            "schema_version": 2, "execution_id": "a" * 32, "pid": 42,
            "exit_code": 1, "timed_out": True, "approved": True,
            "executable_sha256": entry["executable_sha256"], "executable_bytes": 10,
            "fixture_sha256": "sha256:" + "b" * 64, "fixture_bytes": 5,
            "elapsed_ms": 5000, "child_reaped": True, "termination_status": "requested",
            "cleanup": "complete", "publication": "driver-owned",
        }
        return value

    def check(self, value):
        entry = binding()
        return driver.validate_ledger(
            value, {"executable_sha256": entry["executable_sha256"]},
            "a" * 32, "sha256:" + "b" * 64, 5000,
        )

    def test_valid_and_each_contradiction_is_rejected(self):
        self.assertEqual(self.check(self.valid())["pid"], 42)
        mutations = [
            lambda v: v.pop("child_reaped"),
            lambda v: v.update(extra=1),
            lambda v: v.update(cleanup="maybe"),
            lambda v: v.update(publication="pending"),
            lambda v: v.update(execution_id="c" * 32),
            lambda v: v.update(timed_out=True, termination_status="not-requested"),
            lambda v: v.update(timed_out=False, termination_status="requested"),
            lambda v: v.update(exit_code="1"),
            lambda v: v.update(approved=False),
            lambda v: v.update(elapsed_ms=10001),
        ]
        for mutate in mutations:
            value = self.valid()
            mutate(value)
            with self.assertRaises(RuntimeError):
                self.check(value)

    def test_parse_rejects_duplicates_partial_and_oversized(self):
        for line in (b'{"a":1,"a":2}\n', b'{"a":1}', b"[" * 200 + b"\n", b"x" * (driver.MAX_LOG_BYTES + 1) + b"\n"):
            with self.assertRaises(RuntimeError):
                driver.parse_message(line)


class T04BoundedResponse(unittest.TestCase):
    def test_missing_response_is_bounded(self):
        release = threading.Event()

        class Blocking:
            def readline(self, _limit):
                release.wait(5)
                return b""

        started = time.monotonic()
        try:
            with self.assertRaisesRegex(RuntimeError, "deadline"):
                driver.read_message(Blocking(), 0.2)
        finally:
            release.set()
        self.assertLess(time.monotonic() - started, 2.0)


class T05Ownership(unittest.TestCase):
    def test_pin_denies_write_delete_and_rename(self):
        directory = tempfile.mkdtemp(prefix="r0a-s2a-pin-")
        try:
            owned = Path(directory) / "owned"
            owned.mkdir()
            target = owned / "fixture.bin"
            target.write_bytes(b"pinned")
            pin = Pin(target)
            try:
                with self.assertRaises(OSError):
                    target.open("ab").write(b"x")
                with self.assertRaises(OSError):
                    target.unlink()
                with self.assertRaises(OSError):
                    os.rename(owned, Path(directory) / "moved")
                pin.verify()
            finally:
                pin.close()
        finally:
            shutil.rmtree(directory, ignore_errors=True)

    def test_reparse_component_is_rejected(self):
        directory = tempfile.mkdtemp(prefix="r0a-s2a-link-")
        try:
            outside = Path(directory) / "outside"
            outside.mkdir()
            link = Path(directory) / "link"
            completed = subprocess.run(
                ["cmd", "/c", "mklink", "/J", str(link), str(outside)],
                capture_output=True, text=False, check=False,
            )
            if completed.returncode != 0:
                self.skipTest("junction creation unavailable: NOT_RUN")
            with self.assertRaises(RuntimeError):
                Pin(link, directory=True)
        finally:
            shutil.rmtree(directory, ignore_errors=True)


class T06LaunchRejection(unittest.TestCase):
    def test_unapproved_executable_is_rejected_before_creation(self):
        directory = workdir()
        try:
            output = Path(directory) / "out"
            completed = invoke(UNAPPROVED, output)
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(list(output.iterdir()), [])
            self.assertNotIn(directory, completed.stderr)
        finally:
            shutil.rmtree(directory, ignore_errors=True)

    def test_nonempty_output_is_rejected(self):
        directory = workdir()
        try:
            output = Path(directory) / "out"
            output.mkdir()
            (output / "keep").write_text("x", encoding="utf-8")
            self.assertNotEqual(invoke(UNAPPROVED, output).returncode, 0)
            self.assertTrue((output / "keep").exists())
        finally:
            shutil.rmtree(directory, ignore_errors=True)

    def test_fixture_digest_mismatch_is_rejected(self):
        directory = workdir()
        try:
            wrong = Path(directory) / "wrong.json"
            wrong.write_text("{}", encoding="utf-8")
            output = Path(directory) / "out"
            completed = invoke(REPO / "target/debug/cos-desktop.exe", output, fixture=wrong)
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(list(output.iterdir()), [])
        finally:
            shutil.rmtree(directory, ignore_errors=True)


class T07ContractsAndPrivacy(unittest.TestCase):
    def manifest(self):
        return contracts.validate_manifest(contracts.load_document(str(MANIFEST), contracts.MAX_MANIFEST_BYTES))

    def test_result_validates_without_private_paths(self):
        manifest = self.manifest()
        document = driver.result_document(manifest, ["ledger.json"], "NOT_MEASURED", "launched")
        contracts.check_identity(manifest, contracts.validate_result(document))
        text = json.dumps(document)
        self.assertNotIn(str(Path.home()), text)
        self.assertNotIn("SECRET", text)


    def test_exclusive_write_is_complete_and_non_overwriting(self):
        directory = workdir()
        try:
            target = Path(directory) / "ledger.json"
            driver.exclusive_write(target, {"ok": True})
            self.assertEqual(json.loads(target.read_text(encoding="utf-8")), {"ok": True})
            with self.assertRaises(FileExistsError):
                driver.exclusive_write(target, {"ok": False})
        finally:
            shutil.rmtree(directory, ignore_errors=True)


class T09Containment(unittest.TestCase):
    def test_job_close_terminates_owned_child(self):
        import subprocess
        from windows_owner import Job

        child = subprocess.Popen([PY, "-c", "import time; time.sleep(60)"])
        try:
            with Job() as job:
                job.assign(child)
                self.assertFalse(job.empty())
            child.wait(timeout=10)
            self.assertIsNotNone(child.poll())
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()


if __name__ == "__main__":
    unittest.main()
