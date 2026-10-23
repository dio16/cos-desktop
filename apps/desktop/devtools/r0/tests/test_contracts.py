"""MP-R0A-S1 contract tests T01–T17 (25 executed cases). Standard library only."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import contracts  # noqa: E402


def fixture(name: str) -> Path:
    return ROOT / name


def load(path: Path, limit: int):
    return contracts.load_document(str(path), limit)


def shell_manifest():
    return load(fixture("manifests/shell-contract-v1.json"), contracts.MAX_MANIFEST_BYTES)


def media_manifest():
    return load(fixture("manifests/media-not-implemented-v1.json"), contracts.MAX_MANIFEST_BYTES)


def result_document():
    return load(fixture("fixtures/result-not-measured-v1.json"), contracts.MAX_RESULT_BYTES)


class T01ValidManifest(unittest.TestCase):
    def test_shell_manifest_is_accepted(self):
        document = shell_manifest()
        self.assertIs(contracts.validate_manifest(document), document)
        self.assertEqual(document["scenario"]["id"], "shell-startup")


class T02RejectsBadShape(unittest.TestCase):
    def test_missing_type_version(self):
        missing = copy.deepcopy(shell_manifest())
        del missing["identity"]["gpui_revision"]
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.validate_manifest(missing)
        self.assertEqual(caught.exception.errors[0]["code"], "MISSING_FIELD")

        wrong_type = copy.deepcopy(shell_manifest())
        wrong_type["machine"]["cpu"]["logical_count"] = "16"
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.validate_manifest(wrong_type)
        self.assertEqual(caught.exception.errors[0]["code"], "INVALID_TYPE")

        version = copy.deepcopy(shell_manifest())
        version["manifest_version"] = 2
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.validate_manifest(version)
        self.assertEqual(caught.exception.errors[0]["code"], "UNSUPPORTED_VERSION")


class T03BoundedLoad(unittest.TestCase):
    def test_duplicate_malformed_and_oversize_fail_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            duplicate = base / "duplicate.json"
            duplicate.write_text('{"a": 1, "a": 2}', encoding="utf-8")
            with self.assertRaises(contracts.ContractError) as caught:
                load(duplicate, contracts.MAX_MANIFEST_BYTES)
            self.assertEqual(caught.exception.errors[0]["code"], "DUPLICATE_KEY")

            malformed = base / "malformed.json"
            malformed.write_text("{not json", encoding="utf-8")
            with self.assertRaises(contracts.ContractError) as caught:
                load(malformed, contracts.MAX_MANIFEST_BYTES)
            self.assertEqual(caught.exception.errors[0]["code"], "MALFORMED_JSON")

            large = base / "large.json"
            large.write_bytes(b" " * (contracts.MAX_MANIFEST_BYTES + 1))
            with self.assertRaises(contracts.ContractError) as caught:
                load(large, contracts.MAX_MANIFEST_BYTES)
            self.assertEqual(caught.exception.errors[0]["code"], "TOO_LARGE")

            non_finite = base / "non-finite.json"
            non_finite.write_text('{"value": NaN}', encoding="utf-8")
            with self.assertRaises(contracts.ContractError) as caught:
                load(non_finite, contracts.MAX_MANIFEST_BYTES)
            self.assertEqual(caught.exception.errors[0]["code"], "NON_FINITE_NUMBER")

            huge = base / "huge.json"
            huge.write_text('{"value": ' + "9" * 5000 + "}", encoding="utf-8")
            with self.assertRaises(contracts.ContractError) as caught:
                load(huge, contracts.MAX_MANIFEST_BYTES)
            self.assertEqual(caught.exception.errors[0]["code"], "MALFORMED_JSON")


class T04UnmeasuredStaysUnmeasured(unittest.TestCase):
    def test_endpoint_and_metric_remain_unmeasured_without_fabrication(self):
        result = result_document()
        contracts.validate_result(result)
        run = result["runs"][0]
        self.assertEqual(run["status"], "NOT_MEASURED")
        self.assertEqual(run["endpoints"][0]["status"], "NOT_MEASURED")
        self.assertTrue(run["endpoints"][0]["reason"])
        self.assertEqual(run["metrics"][0]["counts"]["observed"], 0)
        self.assertTrue(all(value is None for value in run["metrics"][0]["percentiles"].values()))


def observed_result():
    result = result_document()
    run = result["runs"][0]
    run["status"] = "OBSERVED"
    run["reason"] = ""
    run["endpoints"][0]["status"] = "OBSERVED"
    run["endpoints"][0]["reason"] = ""
    metric = run["metrics"][0]
    metric["counts"]["eligible"] = 1
    metric["counts"]["observed"] = 1
    metric["percentiles"] = {"min": None, "max": None, "p50": None, "p95": None, "p99": None}
    metric["validity"] = "sample retained; no estimator is implemented in MP-R0A-S1"
    frames = run["frames"]
    for name in ("requested", "processed", "submitted", "present", "displayed"):
        frames[name] = 1
    for name in ("missed", "dropped", "stalled", "eligibility", "idle"):
        frames[name] = 0
    frames["reason"] = ""
    run["evidence"] = ["ledger-1"]
    return result


class T05NotImplementedCannotLookMeasured(unittest.TestCase):
    def test_not_implemented_surface_cannot_be_observed(self):
        document = media_manifest()
        contracts.validate_manifest(document)
        self.assertEqual(document["surface"]["status"], "NOT_IMPLEMENTED")

        fabricated = observed_result()
        fabricated["manifest"] = {
            "scenario_id": document["scenario"]["id"],
            "scenario_version": document["scenario"]["version"],
            "benchmark_revision": document["scenario"]["benchmark_revision"],
            "definition_hash": document["identity"]["definition_hash"],
        }
        run = fabricated["runs"][0]
        run["scenario_id"] = document["scenario"]["id"]
        run["run_id"] = document["workload"]["run_id"]
        run["pair_id"] = document["workload"]["pair_id"]
        run["variant"] = document["workload"]["variant"]
        contracts.validate_result(fabricated)
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.check_identity(document, fabricated)
        self.assertEqual(caught.exception.errors[0]["code"], "INVALID_VALUE")


class T06FailureCountsArePreserved(unittest.TestCase):
    def test_failed_censored_excluded_are_not_folded_into_success(self):
        result = result_document()
        result["runs"][0]["status"] = "FAILED"
        result["runs"][0]["reason"] = "partial failure retained"
        metric = result["runs"][0]["metrics"][0]
        metric["counts"] = {
            "eligible": 10,
            "observed": 4,
            "failed": 3,
            "censored": 2,
            "excluded": 1,
        }
        contracts.validate_result(result)
        self.assertEqual(result["runs"][0]["metrics"][0]["counts"]["observed"], 4)

        overflow = copy.deepcopy(result)
        overflow["runs"][0]["metrics"][0]["counts"]["failed"] = 9
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.validate_result(overflow)
        self.assertEqual(caught.exception.errors[0]["code"], "INVALID_VALUE")


class T07IdentityMustMatch(unittest.TestCase):
    def test_mismatched_definition_hash_is_rejected(self):
        manifest = shell_manifest()
        result = result_document()
        contracts.check_identity(manifest, result)
        result["manifest"]["definition_hash"] = "sha256:" + "9" * 64
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.check_identity(manifest, result)
        self.assertEqual(caught.exception.errors[0]["code"], "IDENTITY_MISMATCH")


class T08NoFabricatedPercentiles(unittest.TestCase):
    def test_unmeasured_result_cannot_carry_percentiles(self):
        result = result_document()
        contracts.validate_result(result)
        percentiles = result["runs"][0]["metrics"][0]["percentiles"]
        self.assertEqual(set(percentiles.values()), {None})

        fabricated = copy.deepcopy(result)
        fabricated["runs"][0]["metrics"][0]["percentiles"]["p99"] = 0.0
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.validate_result(fabricated)
        self.assertEqual(caught.exception.errors[0]["code"], "INVALID_VALUE")


class T09Privacy(unittest.TestCase):
    def test_error_output_does_not_reflect_input_text(self):
        with tempfile.TemporaryDirectory() as directory:
            secret = Path(directory) / "secret.json"
            secret.write_text('{"token": "SECRET-TOKEN-VALUE", "token": 1}', encoding="utf-8")
            completed = subprocess.run(
                [sys.executable, "-B", str(ROOT / "validate.py"), "manifest", str(secret)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(completed.returncode, 1)
            self.assertNotIn("SECRET-TOKEN-VALUE", completed.stdout)
            self.assertNotIn(directory, completed.stdout)
            self.assertIn("DUPLICATE_KEY", completed.stdout)

            unknown = Path(directory) / "unknown.json"
            unknown.write_text('{"SECRET-CREDENTIAL-IN-KEY": 1}', encoding="utf-8")
            completed = subprocess.run(
                [sys.executable, "-B", str(ROOT / "validate.py"), "manifest", str(unknown)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(completed.returncode, 1)
            self.assertNotIn("SECRET-CREDENTIAL-IN-KEY", completed.stdout + completed.stderr)
            self.assertIn("UNKNOWN_FIELD", completed.stdout)


class T10CliExitCodes(unittest.TestCase):
    def test_cli_distinguishes_success_and_failure(self):
        valid = subprocess.run(
            [
                sys.executable,
                "-B",
                str(ROOT / "validate.py"),
                "manifest",
                str(fixture("manifests/shell-contract-v1.json")),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(valid.returncode, 0)
        self.assertTrue(json.loads(valid.stdout)["ok"])

        pair = subprocess.run(
            [
                sys.executable,
                "-B",
                str(ROOT / "validate.py"),
                "pair",
                str(fixture("manifests/shell-contract-v1.json")),
                str(fixture("fixtures/result-not-measured-v1.json")),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(pair.returncode, 0)

        mismatch = subprocess.run(
            [
                sys.executable,
                "-B",
                str(ROOT / "validate.py"),
                "pair",
                str(fixture("manifests/media-not-implemented-v1.json")),
                str(fixture("fixtures/result-not-measured-v1.json")),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(mismatch.returncode, 1)
        self.assertEqual(json.loads(mismatch.stdout)["errors"][0]["code"], "IDENTITY_MISMATCH")


class T11DistinctDefinitions(unittest.TestCase):
    def test_different_definition_hash_is_a_distinct_manifest(self):
        shell = shell_manifest()
        media = media_manifest()
        self.assertNotEqual(
            shell["identity"]["definition_hash"], media["identity"]["definition_hash"]
        )
        contracts.validate_manifest(shell)
        contracts.validate_manifest(media)


class T12ResultStructuralIntegrity(unittest.TestCase):
    def test_resources_frames_duplicates_and_percentiles(self):
        base = observed_result()

        def rejects(mutate):
            candidate = copy.deepcopy(base)
            mutate(candidate)
            with self.assertRaises(contracts.ContractError):
                contracts.validate_result(candidate)

        rejects(lambda d: d["runs"][0]["resources"]["process"].__setitem__("private_bytes", "SECRET"))
        rejects(lambda d: d["runs"][0]["resources"]["gpu"].__setitem__("dedicated_bytes", -120))
        rejects(lambda d: d["runs"][0]["frames"].__setitem__("displayed", 999))
        rejects(lambda d: d["runs"][0]["metrics"].append(copy.deepcopy(d["runs"][0]["metrics"][0])))
        rejects(lambda d: d["runs"][0]["metrics"][0]["percentiles"].__setitem__("p50", 3.0))
        rejects(lambda d: d["runs"][0]["metrics"][0]["percentiles"].__setitem__("p99", 0.0))
        rejects(lambda d: d["runs"][0]["endpoints"].append(copy.deepcopy(d["runs"][0]["endpoints"][0])))

    def test_unavailable_frames_require_reason_and_nulls(self):
        result = result_document()
        result["runs"][0]["frames"]["present"] = None
        result["runs"][0]["frames"]["reason"] = ""
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.validate_result(result)
        self.assertEqual(caught.exception.errors[0]["code"], "MISSING_FIELD")
        result["runs"][0]["frames"]["reason"] = "present not sampled"
        contracts.validate_result(result)


class T13DigestTypes(unittest.TestCase):
    def test_git_revision_and_content_digest_are_distinct(self):
        manifest = shell_manifest()
        self.assertEqual(len(manifest["identity"]["repository"]["sha"]), 40)
        contracts.validate_manifest(manifest)

        bad_sha = copy.deepcopy(manifest)
        bad_sha["identity"]["repository"]["sha"] = "a" * 41
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(bad_sha)

        bad_digest = copy.deepcopy(manifest)
        bad_digest["identity"]["schema_hash"] = "a" * 64
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(bad_digest)



class T17ObservedPartialFrames(unittest.TestCase):
    def test_observed_run_allows_unavailable_counter_with_reason(self):
        result = observed_result()
        result["runs"][0]["frames"]["displayed"] = None
        result["runs"][0]["frames"]["reason"] = "displayed counter unavailable"
        contracts.validate_result(result)

        suspected = observed_result()
        suspected["runs"][0]["frames"]["displayed"] = None
        with self.assertRaises(contracts.ContractError):
            contracts.validate_result(suspected)

    def test_parameter_key_fullmatch(self):
        manifest = shell_manifest()
        manifest["workload"]["parameters"] = {"valid\n": 1}
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(manifest)


class T14StatusBidirectional(unittest.TestCase):
    def test_unmeasured_cannot_embed_observed_success(self):
        result = result_document()
        result["runs"][0]["metrics"][0]["counts"]["observed"] = 1
        with self.assertRaises(contracts.ContractError):
            contracts.validate_result(result)

        result = result_document()
        result["runs"][0]["endpoints"][0]["status"] = "OBSERVED"
        result["runs"][0]["endpoints"][0]["reason"] = ""
        with self.assertRaises(contracts.ContractError):
            contracts.validate_result(result)

        failed = result_document()
        failed["runs"][0]["status"] = "FAILED"
        failed["runs"][0]["reason"] = "partial failure"
        failed["runs"][0]["metrics"][0]["counts"]["eligible"] = 3
        failed["runs"][0]["metrics"][0]["counts"]["observed"] = 2
        failed["runs"][0]["metrics"][0]["counts"]["failed"] = 1
        contracts.validate_result(failed)

    def test_partial_frame_relation_is_still_checked(self):
        result = result_document()
        result["runs"][0]["frames"]["present"] = 2
        result["runs"][0]["frames"]["submitted"] = 1
        with self.assertRaises(contracts.ContractError):
            contracts.validate_result(result)


class T15DefinitionBinding(unittest.TestCase):
    def test_stale_definition_hash_is_rejected(self):
        manifest = shell_manifest()
        manifest["workload"]["generator"]["seed"] = 12345
        with self.assertRaises(contracts.ContractError) as caught:
            contracts.validate_manifest(manifest)
        self.assertEqual(caught.exception.errors[0]["code"], "INVALID_VALUE")

    def test_parameters_accept_bounded_scalars(self):
        manifest = shell_manifest()
        manifest["workload"]["parameters"] = {"entity_count": 10, "label": "x" * 128}
        import hashlib, json as jsonlib

        payload = jsonlib.dumps(
            {"scenario": manifest["scenario"], "workload": manifest["workload"]},
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
        manifest["identity"]["definition_hash"] = "sha256:" + hashlib.sha256(payload).hexdigest()
        contracts.validate_manifest(manifest)

        too_many = shell_manifest()
        too_many["workload"]["parameters"] = {f"p{i}": i for i in range(33)}
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(too_many)

        bad_key = shell_manifest()
        bad_key["workload"]["parameters"] = {"../escape": 1}
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(bad_key)

        long_utf8 = shell_manifest()
        long_utf8["workload"]["parameters"] = {"label": "あ" * 43}
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(long_utf8)



class T16UnicodeResourcesAndEndpoints(unittest.TestCase):
    def test_lone_surrogate_is_a_bounded_error(self):
        manifest = shell_manifest()
        manifest["workload"]["expected_state"] = "\ud800"
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(manifest)

    def test_unmeasured_resources_must_not_report_zero(self):
        result = result_document()
        result["runs"][0]["resources"]["process"]["private_bytes"] = 0
        with self.assertRaises(contracts.ContractError):
            contracts.validate_result(result)

    def test_failed_run_keeps_partial_frame_counts(self):
        result = result_document()
        run = result["runs"][0]
        run["status"] = "FAILED"
        run["reason"] = "partial failure"
        run["frames"]["submitted"] = 2
        run["frames"]["present"] = 1
        run["frames"]["displayed"] = 1
        contracts.validate_result(result)

    def test_observed_endpoint_must_be_declared(self):
        manifest = shell_manifest()
        result = observed_result()
        result["runs"][0]["endpoints"][0]["id"] = "not-in-manifest"
        contracts.validate_result(result)
        with self.assertRaises(contracts.ContractError):
            contracts.check_identity(manifest, result)

    def test_ids_and_unavailable_reasons_are_strict(self):
        result = result_document()
        result["runs"][0]["run_id"] = "run-001\n"
        with self.assertRaises(contracts.ContractError):
            contracts.validate_result(result)

        manifest = shell_manifest()
        manifest["identity"]["driver_hash"]["reason"] = ""
        with self.assertRaises(contracts.ContractError):
            contracts.validate_manifest(manifest)



if __name__ == "__main__":
    unittest.main()
