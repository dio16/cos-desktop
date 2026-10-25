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
         helper: Path = DELIVERY, repo_root: Path | None = REPO) -> list[str]:
    command = [
        sys.executable, "-B", str(ROOT / "n07_verify.py"),
        "--bundle", str(bundle),
        "--build-record", str(REPO / "apps/desktop/devtools/r0/s2b-build.json"),
        "--source-identity", str(EVIDENCE / "source-identity.json"),
        "--manifest-a0", str(manifest_a0),
        "--manifest-a1", str(EVIDENCE / "s2b-feature-a1-nb1.json"),
        "--approvals", str(REPO / "apps/desktop/devtools/r0/approved-executables.json"),
        "--helper", str(helper),
        "--a0", str(SCRATCH / "a0/cos-desktop.exe"),
        "--a1", str(SCRATCH / "a1/cos-desktop.exe"),
        "--baseline-record", str(record),
        "--output", str(output),
    ]
    if repo_root is not None:
        command.extend(["--repo-root", str(repo_root)])
    return command


def run(bundle: Path, manifest_a0: Path, output: Path, record: Path,
        helper: Path = DELIVERY, repo_root: Path | None = REPO) -> subprocess.CompletedProcess:
    command = args(bundle, manifest_a0, output, record, helper, repo_root)
    if SCRATCH_HELPER.is_file():
        command.extend(["--helper-copy", str(SCRATCH_HELPER)])
    return subprocess.run(command, capture_output=True, text=True, timeout=120, check=False)


# The archived record pins the 2026-10-25 baseline (28a68e6c). These items compare that
# record's expected identity/hashes with the LIVE tree (helper binary, live build record,
# source identity, approvals), so any later candidate commit legitimately moves them.
# They are the explicit "is the live tree the baseline?" check, asserted separately (and
# expected to fail while the tree is not that baseline). The record's recorded claims are
# still compared with the archived bundle and the archived manifests, which do not move.
LIVE_BASELINE_MISMATCH = frozenset({
    "helper.executable_sha256",
    "build_record.file_sha256",
    "build_record.executable_sha256",
    "build_record.baseline_executable_sha256",
    "build_record.source_commit",
    "build_record.baseline_source_commit",
    "source_identity.observer_source_sha256",
    "source_identity.content_matches_current_source",
    "manifest.a0.driver_hash",
    "manifest.a1.driver_hash",
    "approvals.file_sha256",
})


def items_by_name(record: dict) -> dict[str, dict]:
    return {entry["item"]: entry for entry in record["items"]}


def mismatch_names(record: dict) -> set[str]:
    return {entry["item"] for entry in record["items"] if entry["status"] == "MISMATCH"}


@unittest.skipUnless(all(path.exists() for path in REQUIRED),
                     "NOT_RUN: the new-baseline N07 inputs are not present in this checkout")
class N07Verifier(unittest.TestCase):
    def test_historical_record_matches_its_recorded_archive_and_names_live_drift(self) -> None:
        # Historical verification: every recorded claim is compared with the archived
        # bundle and the archived manifests. The only mismatches are the live-tree items
        # below, which are asserted separately and expected to fail while the tree is not
        # the recorded baseline.
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json")
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "MISMATCH")
            record = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(mismatch_names(record), LIVE_BASELINE_MISMATCH)
            self.assertEqual(record["totals"], {"MATCH": 25, "MISMATCH": 11, "NOT_MEASURED": 0})
            item = items_by_name(record)
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["status"], "MATCH")
            self.assertEqual(item["bundle.file_sha256"]["status"], "MATCH")
            self.assertEqual(item["baseline_a0.executable_sha256"]["status"], "MATCH")
            self.assertEqual(item["manifest.a0.executable_sha256"]["status"], "MATCH")
            self.assertEqual(item["manifest.a1.executable_sha256"]["status"], "MATCH")

    @unittest.expectedFailure
    def test_live_tree_matches_the_recorded_baseline(self) -> None:
        """Explicit "is the live tree the recorded baseline?" check.

        Expected to fail while later commits have moved the live helper, build record,
        source identity or approvals away from baseline 28a68e6c. If the tree is ever
        that baseline again this passes and the marker must be removed; the historical
        record is never rewritten to make it pass.
        """
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json")
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertEqual(json.loads(completed.stdout)["verdict"]["status"], "VERIFIED")

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

    def test_missing_helper_is_not_measured_while_live_drift_stays_named(self) -> None:
        # The missing delivery point is still reported as NOT_MEASURED, never as a
        # mismatch. The remaining mismatches are exactly the live-tree items.
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "r0_owner.exe"
            output = Path(directory) / "n07.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json", helper=missing)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "MISMATCH")
            record = json.loads(output.read_text(encoding="utf-8"))
            item = items_by_name(record)
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["status"], "NOT_MEASURED")
            self.assertEqual(item["helper.path_is_approved_delivery_point"]["reason"], "DELIVERY_POINT_MISSING")
            self.assertEqual(item["helper.executable_sha256"]["status"], "NOT_MEASURED")
            self.assertEqual(item["helper.executable_sha256"]["reason"], "DELIVERY_POINT_MISSING")
            self.assertEqual(mismatch_names(record),
                             LIVE_BASELINE_MISMATCH - {"helper.executable_sha256"})
            self.assertEqual(record["totals"], {"MATCH": 24, "MISMATCH": 10, "NOT_MEASURED": 2})

    def test_arbitrary_repo_root_is_refused_with_a_closed_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "n07.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json", repo_root=Path(directory))
            self.assertEqual(completed.returncode, 2, completed.stdout + completed.stderr)
            self.assertEqual(json.loads(completed.stdout),
                             {"ok": False, "error": "REPO_ROOT_NOT_CANONICAL"})
            self.assertFalse(output.exists(), "a refused root must not publish a record")

    def test_omitted_repo_root_uses_the_canonical_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "n07.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output,
                            EVIDENCE / "record.json", repo_root=None)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["verdict"]["status"], "MISMATCH")
            record = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(record["inputs"]["repo_root"], str(REPO))
            self.assertEqual(mismatch_names(record), LIVE_BASELINE_MISMATCH)

    def test_missing_expected_value_is_not_measured_while_live_drift_stays_named(self) -> None:
        # A recorded expectation that the archive omits is NOT_MEASURED with CLAIM_ABSENT;
        # it is never converted into a mismatch, and the live-drift set stays explicit.
        with tempfile.TemporaryDirectory() as directory:
            record_path = Path(directory) / "record.json"
            document = json.loads((EVIDENCE / "record.json").read_text(encoding="utf-8"))
            document.pop("live_build_record")
            record_path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
            output = Path(directory) / "n07.json"
            completed = run(BUNDLE, EVIDENCE / "s2b-default-a0-nb1.json", output, record_path)
            self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
            record = json.loads(output.read_text(encoding="utf-8"))
            item = items_by_name(record)
            self.assertEqual(item["build_record.file_sha256"]["status"], "NOT_MEASURED")
            self.assertEqual(item["build_record.file_sha256"]["reason"], "CLAIM_ABSENT")
            self.assertEqual(mismatch_names(record),
                             LIVE_BASELINE_MISMATCH - {"build_record.file_sha256"})
            self.assertEqual(record["totals"], {"MATCH": 25, "MISMATCH": 10, "NOT_MEASURED": 1})

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
