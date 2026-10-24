"""Deterministic N07 verifier tests: repeatability and named tamper mismatches."""
from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parents[3]
EVIDENCE = REPO / "docs/development/evidence/r0a-s2b/native-baseline-2026-10-25"
BUNDLE = REPO / "docs/development/evidence/r0a-s2b/native-bundles.zip"
SCRATCH = Path("C:/tmp/r0a-native-baseline-2026-10-25")

REQUIRED = (
    BUNDLE,
    REPO / "apps/desktop/devtools/r0/s2b-build.json",
    EVIDENCE / "source-identity.json",
    EVIDENCE / "s2b-default-a0-nb1.json",
    EVIDENCE / "s2b-feature-a1-nb1.json",
    REPO / "apps/desktop/devtools/r0/approved-executables.json",
    REPO / "target/debug/examples/r0_owner.exe",
    EVIDENCE / "record.json",
    SCRATCH / "a0/cos-desktop.exe",
    SCRATCH / "a1/cos-desktop.exe",
)


def args(bundle: Path, manifest_a0: Path, output: Path) -> list[str]:
    command = [
        sys.executable, "-B", str(ROOT / "n07_verify.py"),
        "--bundle", str(bundle),
        "--build-record", str(REPO / "apps/desktop/devtools/r0/s2b-build.json"),
        "--source-identity", str(EVIDENCE / "source-identity.json"),
        "--manifest-a0", str(manifest_a0),
        "--manifest-a1", str(EVIDENCE / "s2b-feature-a1-nb1.json"),
        "--approvals", str(REPO / "apps/desktop/devtools/r0/approved-executables.json"),
        "--helper", str(REPO / "target/debug/examples/r0_owner.exe"),
        "--a0", str(SCRATCH / "a0/cos-desktop.exe"),
        "--a1", str(SCRATCH / "a1/cos-desktop.exe"),
        "--baseline-record", str(EVIDENCE / "record.json"),
        "--output", str(output),
    ]
    helper_copy = SCRATCH / "helper/r0_owner.exe"
    if helper_copy.is_file():
        command.extend(["--helper-copy", str(helper_copy)])
    return command


def run(bundle: Path, manifest_a0: Path, output: Path) -> subprocess.CompletedProcess:
    return subprocess.run(args(bundle, manifest_a0, output), capture_output=True, text=True,
                          timeout=120, check=False)


@unittest.skipUnless(all(path.exists() for path in REQUIRED),
                     "NOT_RUN: the new-baseline N07 inputs are not present in this checkout")
class N07Verifier(unittest.TestCase):
    def test_same_inputs_produce_identical_record_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            first = Path(directory) / "first.json"
            second = Path(directory) / "second.json"
            self.assertIn(run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", first).returncode, (0, 1))
            self.assertIn(run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", second).returncode, (0, 1))
            self.assertEqual(first.read_bytes(), second.read_bytes())
            record = json.loads(first.read_text(encoding="utf-8"))
            self.assertEqual(record["record_kind"], "N07_NEW_BASELINE_IDENTITY")
            self.assertTrue(record["supersedes"].startswith("NOTHING"))

    def test_tampered_bundle_names_the_bundle_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            tampered = Path(directory) / "bundle.zip"
            shutil.copyfile(BUNDLE, tampered)
            data = bytearray(tampered.read_bytes())
            data[len(data) // 2] ^= 0xFF
            tampered.write_bytes(data)
            output = Path(directory) / "record.json"
            completed = run(tampered, EVIDENCE / "s2b-default-a0-nb1.json", output)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            record = json.loads(output.read_text(encoding="utf-8"))
            status = {entry["item"]: entry["status"] for entry in record["items"]}
            self.assertEqual(status["bundle.file_sha256"], "MISMATCH")

    def test_tampered_manifest_names_the_manifest_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            tampered = Path(directory) / "manifest-a0.json"
            document = json.loads((EVIDENCE / "s2b-default-a0-nb1.json").read_text(encoding="utf-8"))
            document["identity"]["executable_sha256"] = "sha256:" + "0" * 64
            tampered.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
            output = Path(directory) / "record.json"
            completed = run(BUNDLE, tampered, output)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            record = json.loads(output.read_text(encoding="utf-8"))
            status = {entry["item"]: entry["status"] for entry in record["items"]}
            self.assertEqual(status["manifest.a0.executable_sha256"], "MISMATCH")
