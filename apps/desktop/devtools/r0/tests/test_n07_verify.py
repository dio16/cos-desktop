"""Deterministic N07 verifier tests: delivery point, verdicts, repeatability and tampering."""
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
DELIVERY = REPO / "target/debug/examples/r0_owner.exe"
SCRATCH_HELPER = SCRATCH / "helper/r0_owner.exe"

REQUIRED = (
    BUNDLE,
    REPO / "apps/desktop/devtools/r0/s2b-build.json",
    EVIDENCE / "source-identity.json",
    EVIDENCE / "s2b-default-a0-nb1.json",
    EVIDENCE / "s2b-feature-a1-nb1.json",
    REPO / "apps/desktop/devtools/r0/approved-executables.json",
    DELIVERY,
    EVIDENCE / "record.json",
    SCRATCH / "a0/cos-desktop.exe",
    SCRATCH / "a1/cos-desktop.exe",
)


def args(bundle: Path, manifest_a0: Path, output: Path, record: Path,
         helper: Path = DELIVERY, repo_root: Path = REPO) -> list[str]:
    return [
        sys.executable, "-B", str(ROOT / "n07_verify.py"),
        "--bundle", str(bundle),
        "--build-record", str(REPO / "apps/desktop/devtools/r0/s2b-build.json"),
        "--source-identity", str(EVIDENCE / "source-identity.json"),
        "--manifest-a0", str(manifest_a0),
        "--manifest-a1", str(EVIDENCE / "s2b-feature-a1-nb1.json"),
        "--approvals", str(REPO / "apps/desktop/devtools/r0/approved-executables.json"),
        "--repo-root", str(repo_root),
        "--helper", str(helper),
        "--a0", str(SCRATCH / "a0/cos-desktop.exe"),
        "--a1", str(SCRATCH / "a1/cos-desktop.exe"),
        "--baseline-record", str(record),
        "--output", str(output),
    ]


def run(bundle: Path, manifest_a0: Path, output: Path, record: Path,
        helper: Path = DELIVERY, repo_root: Path = REPO) -> subprocess.CompletedProcess:
    command = args(bundle, manifest_a0, output, record, helper, repo_root)
    if SCRATCH_HELPER.is_file():
        command.extend(["--helper-copy", str(SCRATCH_HELPER)])
    return subprocess.run(command, capture_output=True, text=True, timeout=120, check=False)


@unittest.skipUnless(all(path.exists() for path in REQUIRED),
                     "NOT_RUN: the new-baseline N07 inputs are not present in this checkout")
class N07Verifier(unittest.TestCase):
    def test_complete_input_at_the_real_delivery_point_is_verified(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json")
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "VERIFIED")
            record = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(record["verdict"], {"status": "VERIFIED", "reason": ""})
            self.assertEqual(record["totals"], {"MATCH": 36, "MISMATCH": 0, "NOT_MEASURED": 0})
            item = {entry["item"]: entry for entry in record["items"]}
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["status"], "MATCH")

    def test_scratch_copy_only_is_not_verified_and_names_the_path_item(self) -> None:
        self.assertTrue(SCRATCH_HELPER.is_file(), "scratch copy required for this regression")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json", helper=SCRATCH_HELPER)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "MISMATCH")
            self.assertIn("helper.path_is_approved_delivery_point", payload["verdict"]["reason"])
            record = json.loads(output.read_text(encoding="utf-8"))
            item = {entry["item"]: entry for entry in record["items"]}
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["status"], "MISMATCH")
            # The bytes happen to match the approved identity, but that is only corroboration.
            self.assertEqual(item["helper.executable_sha256"]["status"], "MATCH")

    def test_missing_helper_file_is_unverified_not_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "r0_owner.exe"
            output = Path(directory) / "n07.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json", helper=missing)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "UNVERIFIED")
            self.assertIn("helper.path_is_approved_delivery_point", payload["verdict"]["reason"])
            record = json.loads(output.read_text(encoding="utf-8"))
            item = {entry["item"]: entry for entry in record["items"]}
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["status"], "NOT_MEASURED")
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["reason"], "DELIVERY_POINT_MISSING")
            self.assertEqual(item["helper.executable_sha256"]["status"], "NOT_MEASURED")
            self.assertEqual(item["helper.executable_sha256"]["reason"], "DELIVERY_POINT_MISSING")
            self.assertEqual(record["totals"]["MISMATCH"], 0)

    def test_missing_resolved_approved_path_is_unverified(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "n07.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json", repo_root=Path(directory))
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "UNVERIFIED")
            record = json.loads(output.read_text(encoding="utf-8"))
            item = {entry["item"]: entry for entry in record["items"]}
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["status"], "NOT_MEASURED")
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["reason"], "DELIVERY_POINT_MISSING")
            self.assertEqual(item["helper.executable_sha256"]["status"], "NOT_MEASURED")
            self.assertEqual(record["totals"]["MISMATCH"], 0)

    def test_missing_expected_value_is_unverified_and_names_the_item(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            record_path = Path(directory) / "record.json"
            document = json.loads((EVIDENCE / "record.json").read_text(encoding="utf-8"))
            document.pop("live_build_record")
            record_path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
            output = Path(directory) / "n07.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output, record_path)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "UNVERIFIED")
            self.assertIn("build_record.file_sha256", payload["verdict"]["reason"])

    def test_mismatch_still_exits_nonzero_and_is_named(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            tampered = Path(directory) / "bundle.zip"
            shutil.copyfile(BUNDLE, tampered)
            data = bytearray(tampered.read_bytes())
            data[len(data) // 2] ^= 0xFF
            tampered.write_bytes(data)
            output = Path(directory) / "record.json"
            completed = run(tampered, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json")
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "MISMATCH")
            self.assertIn("bundle.file_sha256", payload["verdict"]["reason"])

    def test_tampered_manifest_names_the_manifest_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            tampered = Path(directory) / "manifest-a0.json"
            document = json.loads((EVIDENCE / "s2b-default-a0-nb1.json").read_text(encoding="utf-8"))
            document["identity"]["executable_sha256"] = "sha256:" + "0" * 64
            tampered.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
            output = Path(directory) / "record.json"
            completed = run(BUNDLE, tampered, output, EVIDENCE / "record.json")
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "MISMATCH")
            self.assertIn("manifest.a0.executable_sha256", payload["verdict"]["reason"])

    def test_same_inputs_produce_identical_record_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            first = Path(directory) / "first.json"
            second = Path(directory) / "second.json"
            first_run = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", first,
                            EVIDENCE / "record.json")
            second_run = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", second,
                             EVIDENCE / "record.json")
            self.assertEqual(first_run.returncode, second_run.returncode)
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_verdict_precedence_names_both_kinds(self) -> None:
        sys.path.insert(0, str(ROOT))
        import n07_verify
        items = [
            {"item": "a", "status": "MATCH"},
            {"item": "b", "status": "NOT_MEASURED"},
            {"item": "c", "status": "MISMATCH"},
        ]
        verdict = n07_verify.verdict_for(items)
        self.assertEqual(verdict["status"], "MISMATCH")
        self.assertIn("c", verdict["reason"])
        self.assertEqual(n07_verify.verdict_for([{"item": "a", "status": "MATCH"}]),
                         {"status": "VERIFIED", "reason": ""})
        self.assertEqual(n07_verify.verdict_for([{"item": "a", "status": "NOT_MEASURED"}]),
                         {"status": "UNVERIFIED", "reason": "NOT_MEASURED items: a"})
