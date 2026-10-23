"""R9 consumer, single-use profile and publication-failure regressions.

All generated ledgers are contract fixtures, not process observations. Real process
ownership and shutdown are exercised separately with the approved Desktop binary.
"""
from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import contracts
import driver
from windows_owner import Pin, known_folder


def manifest():
    return contracts.validate_manifest(contracts.load_document(
        str(ROOT / "manifests/s2a-lifecycle-v1.json"), contracts.MAX_MANIFEST_BYTES))


def ledger(m, *, timeout=False, exit_code=0):
    return {
        "schema_version": 2, "execution_id": "a" * 32, "pid": 42,
        "exit_code": exit_code, "timed_out": timeout, "approved": True,
        "executable_sha256": m["identity"]["executable_sha256"], "executable_bytes": 10,
        "fixture_sha256": m["workload"]["source"]["sha256"], "fixture_bytes": 5,
        "elapsed_ms": 5000 if timeout else 10, "child_reaped": True,
        "termination_status": "requested" if timeout else "not-requested",
        "cleanup": "complete", "publication": "driver-owned",
    }


class Publication(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="r9-consumer-", dir="C:/tmp")
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name)
        self.manifest = manifest()

    def publish(self, *, timeout=False, exit_code=0):
        value = ledger(self.manifest, timeout=timeout, exit_code=exit_code)
        status = "FAILED" if timeout or exit_code else "NOT_MEASURED"
        result = driver.result_document(self.manifest, ["ledger.json"], status, "contract fixture")
        driver.publish(self.output, self.manifest, 5000, value, result)
        return value, result

    def rebind(self, name, content):
        # Re-sign the mutated artifact so rejection must be semantic, not just hash.
        for marker in self.output.glob("committed-*"):
            marker.rmdir()
        target = self.output / name
        target.write_text(json.dumps(content) + "\n", encoding="utf-8")
        completion = json.loads((self.output / "completion.json").read_bytes())
        completion["artifacts"][name] = "sha256:" + hashlib.sha256(target.read_bytes()).hexdigest()
        raw = (json.dumps(completion) + "\n").encode()
        (self.output / "completion.json").write_bytes(raw)
        (self.output / ("committed-" + hashlib.sha256(raw).hexdigest())).mkdir()

    def reject(self):
        with self.assertRaises((RuntimeError, OSError, contracts.ContractError)):
            driver.verify_publication(self.output, self.manifest, 5000)

    def test_complete_normal_run_is_accepted_and_missing_commit_is_incomplete(self):
        self.publish()
        accepted = driver.verify_publication(self.output, self.manifest, 5000)
        self.assertEqual(accepted["execution_id"], "a" * 32)
        next(self.output.glob("committed-*")).rmdir()
        # The superseded JSON marker is never an alternate success path.
        driver.exclusive_write(self.output / "commit.json", {"schema_version": 1, "execution_id": "a" * 32})
        self.reject()

    def test_timeout_and_nonzero_exit_require_failed_result(self):
        for timeout, exit_code in ((True, 1), (False, 2)):
            with self.subTest(timeout=timeout, exit_code=exit_code):
                with tempfile.TemporaryDirectory(dir="C:/tmp") as base:
                    original, self.output = self.output, Path(base)
                    try:
                        _, result = self.publish(timeout=timeout, exit_code=exit_code)
                        driver.verify_publication(self.output, self.manifest, 5000)
                        result["runs"][0]["status"] = "NOT_MEASURED"
                        result["runs"][0]["failures"] = []
                        self.rebind("result.json", result)
                        self.reject()
                    finally:
                        self.output = original

    def test_normal_exit_cannot_be_relabelled_failed(self):
        _, result = self.publish()
        result["runs"][0]["status"] = "FAILED"
        result["runs"][0]["failures"] = ["invented failure"]
        self.rebind("result.json", result)
        self.reject()

    def test_matrix_missing_duplicate_unknown_observed_or_reasonless_is_rejected(self):
        self.publish()
        valid = json.loads((self.output / "endpoint-availability.json").read_bytes())
        mutations = [
            lambda v: v.pop(),
            lambda v: v.__setitem__(-1, copy.deepcopy(v[0])),
            lambda v: v[-1].update(id="unknown-endpoint"),
            lambda v: next(item for item in v if item["id"] == "present-call").update(status="OBSERVED"),
            lambda v: v[-1].update(reason=" "),
            lambda v: v[0].update(status="NOT_MEASURED"),
            lambda v: v[1].update(extra=True),
        ]
        for index, mutate in enumerate(mutations):
            with self.subTest(case=index):
                changed = copy.deepcopy(valid)
                mutate(changed)
                self.rebind("endpoint-availability.json", changed)
                self.reject()

    def test_ledger_execution_or_closed_schema_cannot_be_forged(self):
        original, _ = self.publish()
        for change in ({"execution_id": "b" * 32}, {"extra": 1}, {"timed_out": True}):
            with self.subTest(change=change):
                self.rebind("ledger.json", {**original, **change})
                self.reject()

    def test_every_completion_file_error_precedes_commit(self):
        real_open, real_sync = Path.open, os.fsync
        for stage in ("create", "write", "flush", "fsync", "close"):
            with self.subTest(stage=stage), tempfile.TemporaryDirectory(dir="C:/tmp") as base:
                original, self.output = self.output, Path(base)
                completion_fd = []
                class Fault:
                    def __init__(self, file):
                        self.file = file
                        completion_fd.append(file.fileno())
                    def __enter__(self):
                        return self
                    def __exit__(self, *_args):
                        self.file.close()
                        if stage == "close":
                            raise OSError("injected close failure")
                    def write(self, value):
                        result = self.file.write(value)
                        if stage == "write":
                            raise OSError("injected write failure")
                        return result
                    def flush(self):
                        self.file.flush()
                        if stage == "flush":
                            raise OSError("injected flush failure")
                    def fileno(self):
                        return self.file.fileno()
                def opening(path, *args, **kwargs):
                    if path.name != "completion.json" or not args or args[0] != "x":
                        return real_open(path, *args, **kwargs)
                    if stage == "create":
                        raise OSError("injected create failure")
                    return Fault(real_open(path, *args, **kwargs))
                def syncing(fd):
                    real_sync(fd)
                    if stage == "fsync" and fd in completion_fd:
                        raise OSError("injected sync failure")
                try:
                    with patch.object(Path, "open", opening), patch.object(os, "fsync", syncing):
                        with self.assertRaises(OSError):
                            self.publish()
                    self.assertEqual(list(self.output.glob("committed-*")), [])
                    self.reject()
                finally:
                    self.output = original

    def test_commit_create_failure_and_interruption_have_defined_visibility(self):
        real_mkdir = Path.mkdir
        for after_create in (False, True):
            with self.subTest(after_create=after_create), tempfile.TemporaryDirectory(dir="C:/tmp") as base:
                original, self.output = self.output, Path(base)
                def interrupted(path, *args, **kwargs):
                    if path.name.startswith("committed-"):
                        if after_create:
                            real_mkdir(path, *args, **kwargs)
                        raise SystemExit("interruption at commit point")
                    return real_mkdir(path, *args, **kwargs)
                try:
                    with patch.object(Path, "mkdir", interrupted):
                        with self.assertRaises(SystemExit):
                            self.publish()
                    if after_create:
                        self.assertEqual(driver.verify_publication(self.output, self.manifest, 5000)["execution_id"], "a" * 32)
                    else:
                        self.reject()
                finally:
                    self.output = original

    def test_nonempty_or_wrong_digest_commit_is_rejected(self):
        self.publish()
        marker = next(self.output.glob("committed-*"))
        (marker / "unexpected").write_bytes(b"x")
        self.reject()
        (marker / "unexpected").unlink()
        marker.rename(marker.with_name("committed-" + "0" * 64))
        self.reject()


class ProfileIsolation(unittest.TestCase):
    def test_shell_profile_rejection_does_not_depend_on_userprofile(self):
        home = known_folder("profile")
        execution = uuid.uuid4().hex
        with patch.dict(os.environ, {}, clear=True):
            for path in (home, home / execution, home.parent):
                with self.subTest(path=str(path)), self.assertRaises(RuntimeError):
                    driver.require_per_run_profile(path, execution)
            with tempfile.TemporaryDirectory(dir="C:/tmp") as base:
                profile = Path(base) / execution
                (profile / "roaming").mkdir(parents=True)
                (profile / "temp").mkdir()
                driver.require_per_run_profile(profile, execution)
                (profile / "roaming" / "prior-state").write_bytes(b"x")
                with self.assertRaises(RuntimeError):
                    driver.require_per_run_profile(profile, execution)
                with self.assertRaises(RuntimeError):
                    driver.require_per_run_profile(profile, "b" * 32)

    def test_unavailable_os_profile_fails_closed(self):
        with patch.object(driver, "known_folder", side_effect=RuntimeError("unavailable")):
            with self.assertRaises(RuntimeError):
                driver.require_per_run_profile(Path("C:/tmp") / ("a" * 32), "a" * 32)

    def test_cleared_or_renamed_profile_and_consumed_run_id_cannot_be_reused(self):
        with tempfile.TemporaryDirectory(dir="C:/tmp") as base:
            root = Path(base)
            registry = root / "registry"
            registry.mkdir()
            profile = root / uuid.uuid4().hex
            profile.mkdir()
            with Pin(profile, directory=True) as owned:
                identity = owned.identity()
                driver.consume_identity(registry, profile.name, identity)
            # Even after contents are gone and the directory has a new run name,
            # its opened filesystem identity still has an independent claim.
            old_id = profile.name
            profile = profile.rename(root / uuid.uuid4().hex)
            with Pin(profile, directory=True) as owned:
                with self.assertRaises(FileExistsError):
                    driver.consume_identity(registry, profile.name, owned.identity())
            fresh = root / uuid.uuid4().hex
            fresh.mkdir()
            with Pin(fresh, directory=True) as owned:
                with self.assertRaises(FileExistsError):
                    driver.consume_identity(registry, old_id, owned.identity())


if __name__ == "__main__":
    unittest.main()
