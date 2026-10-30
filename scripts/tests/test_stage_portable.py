"""Consumer-facing regression cases for the internal portable preflight CLI."""
from __future__ import annotations

import hashlib
import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "stage_portable.py"


def minimal_pe() -> bytes:
    """A deterministic PE32+ fixture with one direct KERNEL32.dll import."""
    data = bytearray(0x300)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", data, 0x84, 0x8664, 1, 0, 0, 0, 0xF0, 0x22)
    optional = 0x98
    struct.pack_into("<H", data, optional, 0x20B)
    struct.pack_into("<II", data, optional + 112 + 8, 0x1000, 40)
    section = optional + 0xF0
    struct.pack_into("<IIII", data, section + 8, 0x80, 0x1000, 0x80, 0x200)
    struct.pack_into("<IIIII", data, 0x200, 0, 0, 0, 0x1028, 0)
    data[0x228:0x238] = b"KERNEL32.dll\0"
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
