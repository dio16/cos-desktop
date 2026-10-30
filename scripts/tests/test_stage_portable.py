"""Consumer-facing regression cases for the internal portable preflight CLI."""
from __future__ import annotations

import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "stage_portable.py"
sys.path.insert(0, str(SCRIPT.parent))
import stage_portable


def minimal_pe(import_names=("KERNEL32.dll",)) -> bytes:
    """A deterministic PE32+ fixture with independently encoded DLL descriptors."""
    data = bytearray(0x400)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", data, 0x84, 0x8664, 1, 0, 0, 0, 0xF0, 0x22)
    optional = 0x98
    struct.pack_into("<H", data, optional, 0x20B)
    struct.pack_into("<II", data, optional + 112 + 8, 0x1000, (len(import_names) + 1) * 20)
    section = optional + 0xF0
    struct.pack_into("<IIII", data, section + 8, 0x200, 0x1000, 0x200, 0x200)
    cursor = 0x280
    for index, name in enumerate(import_names):
        encoded = name.encode("ascii") + b"\0"
        struct.pack_into("<IIIII", data, 0x200 + 20 * index, 0, 0, 0, 0x1000 + cursor - 0x200, 0)
        data[cursor:cursor + len(encoded)] = encoded
        cursor += len(encoded)
    return bytes(data)


class StagePortableCliTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="portable-preflight-")
        self.package = Path(self.temporary.name) / "package with 日本語"
        self.package.mkdir()
        self.write_valid_package()

    def tearDown(self):
        self.temporary.cleanup()

    def write_valid_package(self):
        payload = {
            "cos-desktop.exe": minimal_pe(),
            "LICENSE": b"Apache License 2.0\n",
            "NOTICE": b"COS Desktop notice\n",
        }
        for name, content in payload.items():
            (self.package / name).write_bytes(content)
        manifest = {
            "format": "cos-desktop-internal-portable-v1",
            "desktop_version": "0.1.0",
            "protocol_version": {"status": "UNKNOWN_NOT_NEGOTIATED", "version": None},
            "schema_version": {"status": "UNKNOWN_NOT_NEGOTIATED", "version": None},
            "platform": "windows",
            "architecture": "x86_64",
            "binary": {"path": "cos-desktop.exe", "sha256": hashlib.sha256(payload["cos-desktop.exe"]).hexdigest()},
            "files": [{"path": name, "sha256": hashlib.sha256(content).hexdigest()} for name, content in sorted(payload.items())],
            "source": {"commit": "a" * 40, "tree": "b" * 40, "worktree": "CLEAN"},
            "build": {"profile": "dev", "command": "cargo build --locked --package cos-desktop --bin cos-desktop --message-format=json-render-diagnostics", "cargo_version": "cargo 1.90", "rustc_version": "rustc 1.90"},
            "licenses": {"LICENSE": "INCLUDED", "NOTICE": "INCLUDED"},
            "external_runtime_dependencies": {
                "observed_direct_pe_imports": ["KERNEL32.dll"],
                "delay_load_imports": "NOT_INSPECTED",
                "transitive_runtime_dependencies": "UNKNOWN_NOT_INFERRED",
                "redistributable_supply_policy": "UNRESOLVED_DD21",
                "dependency_classification": "NOT_CLASSIFIED_SYSTEM_VS_REDISTRIBUTABLE",
            },
            "signing_status": "UNSIGNED_INTERNAL_STAGING",
            "packaging_status": "UNSIGNED_INTERNAL_STAGING",
            "verification_status": "VERIFIED_INTERNAL_ONLY",
            "distribution_ready": False,
            "limitations": ["UNSIGNED", "INSTALLER_NOT_SELECTED", "PUBLISHER_NOT_SELECTED", "DISTRIBUTION_CHANNEL_NOT_SELECTED", "UNINSTALL_POLICY_UNRESOLVED", "CLEAN_USER_NOT_MEASURED", "TRANSITIVE_DLL_CLOSURE_UNKNOWN", "THIRD_PARTY_LICENSE_CLOSURE_UNASSESSED", "DELAY_LOAD_IMPORTS_NOT_INSPECTED", "DYNAMIC_LOAD_DEPENDENCIES_NOT_ENUMERATED"],
        }
        self.write_manifest(manifest)

    def read_manifest(self):
        return json.loads((self.package / "manifest.json").read_text(encoding="utf-8"))

    def write_manifest(self, manifest):
        (self.package / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")

    def invoke(self, *arguments):
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), *map(str, arguments)],
            capture_output=True,
            text=True,
            encoding="utf-8",
        )

    def assert_rejected(self, *arguments):
        result = self.invoke("verify", self.package, *arguments)
        self.assertNotEqual(0, result.returncode, result.stdout)
        self.assertIn("preflight error", result.stderr)

    def test_valid_package_verifies_from_unicode_path(self):
        result = self.invoke("verify", self.package)
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertIn("UNSIGNED_INTERNAL_STAGING", result.stdout)

    def test_case_variant_dll_imports_verify_across_independent_processes(self):
        binary = minimal_pe(("kernel32.dll", "KERNEL32.dll"))
        (self.package / "cos-desktop.exe").write_bytes(binary)
        manifest = self.read_manifest()
        digest = hashlib.sha256(binary).hexdigest()
        manifest["binary"]["sha256"] = digest
        for entry in manifest["files"]:
            if entry["path"] == "cos-desktop.exe":
                entry["sha256"] = digest
        manifest["external_runtime_dependencies"]["observed_direct_pe_imports"] = ["KERNEL32.dll", "kernel32.dll"]
        self.write_manifest(manifest)
        for seed in ("1", "2", "3", "4"):
            with self.subTest(hash_seed=seed):
                result = subprocess.run(
                    [sys.executable, "-B", str(SCRIPT), "verify", str(self.package)],
                    env={**os.environ, "PYTHONHASHSEED": seed},
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                )
                self.assertEqual(0, result.returncode, result.stderr)

    def test_missing_or_modified_binary_is_rejected(self):
        binary = self.package / "cos-desktop.exe"
        original = binary.read_bytes()
        binary.unlink()
        self.assert_rejected()
        binary.write_bytes(original + b"changed")
        self.assert_rejected()

    def test_missing_or_modified_license_is_rejected(self):
        for name in ("LICENSE", "NOTICE"):
            with self.subTest(name=name):
                license_path = self.package / name
                original = license_path.read_bytes()
                license_path.unlink()
                self.assert_rejected()
                license_path.write_bytes(original + b"changed")
                self.assert_rejected()
                license_path.write_bytes(original)

    def test_extra_and_secret_files_are_rejected(self):
        (self.package / "private-token.txt").write_text("not for distribution", encoding="utf-8")
        self.assert_rejected()

    def test_hard_link_payload_is_rejected(self):
        alias = Path(self.temporary.name) / "outside-license-alias"
        alias.hardlink_to(self.package / "LICENSE")
        self.assert_rejected()

    def test_symbolic_link_payload_is_rejected(self):
        license_path = self.package / "LICENSE"
        license_path.unlink()
        target = Path(self.temporary.name) / "outside-license"
        target.write_text("license", encoding="utf-8")
        try:
            license_path.symlink_to(target)
        except (OSError, NotImplementedError) as exc:
            self.skipTest(f"symbolic links unavailable: {exc}")
        self.assert_rejected()

    @unittest.skipUnless(sys.platform == "win32", "Windows junction boundary")
    def test_junction_package_directory_is_rejected(self):
        junction = Path(self.temporary.name) / "redirected package"
        created = subprocess.run(
            ["cmd.exe", "/c", "mklink", "/J", str(junction), str(self.package)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(0, created.returncode, created.stderr)
        try:
            result = self.invoke("verify", junction)
            self.assertNotEqual(0, result.returncode, result.stdout)
            self.assertIn("preflight error", result.stderr)
        finally:
            junction.rmdir()

    def test_modified_manifest_cannot_claim_signed_or_release_ready(self):
        manifest = self.read_manifest()
        manifest["signing_status"] = "SIGNED"
        manifest["distribution_ready"] = True
        self.write_manifest(manifest)
        self.assert_rejected()

    def test_manifest_cannot_expand_payload_allowlist_or_use_unsafe_path(self):
        manifest = self.read_manifest()
        manifest["files"][0]["path"] = "../private-token.txt"
        self.write_manifest(manifest)
        self.assert_rejected()

    def test_protocol_version_cannot_be_fabricated_by_manifest_edit(self):
        manifest = self.read_manifest()
        manifest["protocol_version"] = {"status": "NEGOTIATED", "version": "0.1.0"}
        self.write_manifest(manifest)
        self.assert_rejected()

    def test_stage_require_signed_refuses_before_building(self):
        result = self.invoke("stage", "--source-root", self.package / "missing-source", "--output", self.package.parent / "out", "--require-signed")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("unsupported", result.stderr)
    def test_manifest_digest_tampering_is_rejected(self):
        manifest = self.read_manifest()
        manifest["files"][0]["sha256"] = "0" * 64
        self.write_manifest(manifest)
        self.assert_rejected()

    def test_require_signed_distribution_preflight_fails_closed(self):
        result = self.invoke("verify", self.package, "--require-signed")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("unsupported", result.stderr)

    def update_binary_digest(self):
        with (self.package / "cos-desktop.exe").open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        manifest = self.read_manifest()
        manifest["binary"]["sha256"] = digest
        for entry in manifest["files"]:
            if entry["path"] == "cos-desktop.exe":
                entry["sha256"] = digest
        self.write_manifest(manifest)

    def test_pe_exact_size_limit_is_admitted(self):
        with (self.package / "cos-desktop.exe").open("r+b") as stream:
            stream.truncate(stage_portable.MAX_PE_BYTES)
        self.update_binary_digest()
        result = self.invoke("verify", self.package)
        self.assertEqual(0, result.returncode, result.stderr)

    def test_pe_one_byte_above_size_limit_is_refused(self):
        with (self.package / "cos-desktop.exe").open("r+b") as stream:
            stream.truncate(stage_portable.MAX_PE_BYTES + 1)
        self.update_binary_digest()
        self.assert_rejected()

    def test_manifest_exact_size_limit_is_admitted(self):
        manifest = self.package / "manifest.json"
        content = manifest.read_bytes()
        manifest.write_bytes(content + b" " * (stage_portable.MAX_MANIFEST_BYTES - len(content)))
        result = self.invoke("verify", self.package)
        self.assertEqual(0, result.returncode, result.stderr)

    def test_manifest_one_byte_above_size_limit_is_refused(self):
        manifest = self.package / "manifest.json"
        content = manifest.read_bytes()
        manifest.write_bytes(content + b" " * (stage_portable.MAX_MANIFEST_BYTES + 1 - len(content)))
        self.assert_rejected()

    def test_payload_replaced_with_a_hard_link_during_verification_is_refused(self):
        license_path = self.package / "LICENSE"
        outside = Path(self.temporary.name) / "external-license"
        outside.write_bytes(license_path.read_bytes())
        real_inspect = stage_portable.inspect_pe

        def replace_after_inspection(data):
            result = real_inspect(data)
            license_path.unlink()
            license_path.hardlink_to(outside)
            return result

        with patch.object(stage_portable, "inspect_pe", side_effect=replace_after_inspection):
            with self.assertRaises(stage_portable.StageError):
                stage_portable.verify_directory(self.package)
        self.assertEqual(2, outside.stat().st_nlink)

    def test_manifest_replaced_by_identical_bytes_during_verification_is_refused(self):
        replacement = Path(self.temporary.name) / "replacement-manifest"
        replacement.write_bytes((self.package / "manifest.json").read_bytes())
        real_inspect = stage_portable.inspect_pe

        def replace_after_inspection(data):
            result = real_inspect(data)
            os.replace(replacement, self.package / "manifest.json")
            return result

        with patch.object(stage_portable, "inspect_pe", side_effect=replace_after_inspection):
            with self.assertRaises(stage_portable.StageError):
                stage_portable.verify_directory(self.package)

    def test_file_added_after_initial_inventory_is_refused(self):
        real_inspect = stage_portable.inspect_pe

        def add_after_inspection(data):
            result = real_inspect(data)
            (self.package / "unexpected.txt").write_text("not approved", encoding="utf-8")
            return result

        with patch.object(stage_portable, "inspect_pe", side_effect=add_after_inspection):
            with self.assertRaises(stage_portable.StageError):
                stage_portable.verify_directory(self.package)

    def test_binary_written_in_place_during_snapshot_hashing_is_refused(self):
        binary = self.package / "cos-desktop.exe"
        original = binary.read_bytes()
        real_sha256 = hashlib.sha256

        def hash_then_change(data=b""):
            digest = real_sha256(data)
            if data == original:
                with binary.open("r+b") as stream:
                    stream.seek(-1, os.SEEK_END)
                    stream.write(b"\xff")
            return digest

        with patch.object(hashlib, "sha256", side_effect=hash_then_change):
            with self.assertRaises(stage_portable.StageError):
                stage_portable.verify_directory(self.package)

    def stage_failure(self, change):
        """Run the patched stage() and return the StageError it raises."""
        with self.assertRaises(stage_portable.StageError) as raised:
            self.stage_with_final_change(change, expect_failure=False)
        return raised.exception

    def stage_with_final_change(self, change, expect_failure=True):
        root = Path(self.temporary.name) / "source"
        root.mkdir()
        for name in ("LICENSE", "NOTICE"):
            (root / name).write_bytes((self.package / name).read_bytes())
        output = Path(self.temporary.name) / "published"
        real_verify = stage_portable.verify_directory

        def change_before_final_verification(directory):
            if directory == output:
                change(output)
            return real_verify(directory)

        # Isolate the filesystem transaction; the real Cargo path has separate smoke evidence.
        with (
            patch.object(stage_portable, "read_source_identity", return_value=("a" * 40, "b" * 40)),
            patch.object(stage_portable, "cargo_package", return_value=({"version": "0.1.0"}, "fixture-package")),
            patch.object(stage_portable, "build_executable", return_value=(
                self.package / "cos-desktop.exe", "cargo fixture", "rustc fixture",
                "cargo build --locked --package cos-desktop --bin cos-desktop --message-format=json-render-diagnostics")),
            patch.object(stage_portable, "verify_directory", side_effect=change_before_final_verification),
        ):
            if expect_failure:
                with self.assertRaises(stage_portable.StageError):
                    stage_portable.stage(root, output)
            else:
                stage_portable.stage(root, output)
        return output

    def test_failed_post_rename_verification_withdraws_own_completed_name(self):
        def corrupt(output):
            (output / "NOTICE").write_text("modified after rename", encoding="utf-8")

        output = self.stage_with_final_change(corrupt)
        self.assertFalse(output.exists())
        # The tool never deletes a directory by path: the withdrawn package is
        # retained under its staging name instead of being removed.
        self.assertEqual(1, len(list(output.parent.glob(".published.staging-*"))))

    def test_post_rename_failure_does_not_remove_a_foreign_replacement(self):
        saved = Path(self.temporary.name) / "moved-owned-package"

        def replace(output):
            output.rename(saved)
            output.mkdir()
            (output / "user-data.txt").write_text("preserve me", encoding="utf-8")

        output = self.stage_with_final_change(replace)
        self.assertEqual("preserve me", (output / "user-data.txt").read_text(encoding="utf-8"))
        self.assertEqual(b"COS Desktop notice\n", (saved / "NOTICE").read_bytes())

    def test_retained_staging_directory_is_named_in_the_printed_error(self):
        def corrupt(output):
            (output / "NOTICE").write_text("modified after rename", encoding="utf-8")

        output = Path(self.temporary.name) / "published"
        message = str(self.stage_failure(corrupt))
        self.assertIn("retained staging directory", message)
        # The name in the message is the directory that actually survives, and it is
        # part of the message text the CLI prints, not a note only repr() shows.
        retained = message.split("retained staging directory: ", 1)[1].rstrip(")")
        self.assertTrue(Path(retained).is_dir())
        self.assertEqual(1, len(list(output.parent.glob(".published.staging-*"))))

    def test_cli_prints_the_error_message_it_reports(self):
        result = self.invoke("verify", Path(self.temporary.name) / "absent-package")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("preflight error", result.stderr)
        self.assertIn("real directory", result.stderr)

    def test_stage_removes_nothing_so_a_replacement_can_never_be_deleted(self):
        # No cleanup deletion exists to race with: a failing stage leaves both its
        # own staging directory and whatever else sits in the parent untouched.
        bystander = Path(self.temporary.name) / "bystander"
        bystander.mkdir()
        (bystander / "user-data.txt").write_text("preserve me", encoding="utf-8")

        def corrupt(output):
            (output / "NOTICE").write_text("modified after rename", encoding="utf-8")

        output = self.stage_with_final_change(corrupt)
        self.assertFalse(output.exists())
        self.assertEqual("preserve me", (bystander / "user-data.txt").read_text(encoding="utf-8"))
        self.assertEqual(1, len(list(output.parent.glob(".published.staging-*"))))

    def test_replacement_between_ownership_check_and_withdrawal_is_preserved(self):
        saved = Path(self.temporary.name) / "moved-owned-package"
        output = Path(self.temporary.name) / "published"
        real_rename = os.rename

        def racing_rename(source, target):
            # Inject the replacement in the window after the ownership check and
            # before the withdrawal rename runs.
            if Path(source) == output and not saved.exists():
                real_rename(output, saved)
                output.mkdir()
                (output / "user-data.txt").write_text("preserve me", encoding="utf-8")
            return real_rename(source, target)

        def corrupt(directory):
            (directory / "NOTICE").write_text("modified after rename", encoding="utf-8")

        root = Path(self.temporary.name) / "source"
        root.mkdir()
        for name in ("LICENSE", "NOTICE"):
            (root / name).write_bytes((self.package / name).read_bytes())
        real_verify = stage_portable.verify_directory

        def corrupt_published(directory):
            if directory == output:
                corrupt(directory)
            return real_verify(directory)

        with (
            patch.object(stage_portable, "read_source_identity", return_value=("a" * 40, "b" * 40)),
            patch.object(stage_portable, "cargo_package", return_value=({"version": "0.1.0"}, "fixture-package")),
            patch.object(stage_portable, "build_executable", return_value=(
                self.package / "cos-desktop.exe", "cargo fixture", "rustc fixture",
                "cargo build --locked --package cos-desktop --bin cos-desktop --message-format=json-render-diagnostics")),
            patch.object(stage_portable, "verify_directory", side_effect=corrupt_published),
            patch.object(stage_portable.os, "rename", side_effect=racing_rename),
        ):
            with self.assertRaises(stage_portable.StageError):
                stage_portable.stage(root, output)
        self.assertEqual("preserve me", (output / "user-data.txt").read_text(encoding="utf-8"))
        # The foreign replacement is back at its path; this run's own package is
        # still parked where the race moved it, never deleted.
        self.assertTrue((saved / "NOTICE").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
