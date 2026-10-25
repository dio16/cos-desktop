"""MP-R0C-S1 contract regressions. Standard library only, no Windows required."""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import r0c_contracts as r0c  # noqa: E402

MANIFEST = ROOT / "manifests/r0c-s1-keyboard.json"


def manifest() -> dict:
    return r0c.load_manifest(str(MANIFEST))


class ManifestTests(unittest.TestCase):
    def test_the_committed_manifest_is_valid(self) -> None:
        document = manifest()
        self.assertEqual(document["manifest_version"], r0c.MANIFEST_VERSION)
        self.assertEqual(document["scenario"]["family"], "keyboard")

    def test_unknown_fields_are_rejected(self) -> None:
        document = copy.deepcopy(manifest())
        document["unexpected"] = 1
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "UNKNOWN_FIELD")

    def test_bad_digest_is_rejected(self) -> None:
        document = copy.deepcopy(manifest())
        document["identity"]["executable_sha256"] = "sha256:not-a-digest"
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "INVALID_DIGEST")

    def test_not_implemented_family_is_refused(self) -> None:
        document = copy.deepcopy(manifest())
        document["scenario"]["family"] = "popup"
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "NOT_IMPLEMENTED")

    def test_out_of_bounds_are_rejected(self) -> None:
        for path, value in (
            (("scenario", "duration_ms"), 10),
            (("scenario", "active_end_ms"), 999999),
            (("actions", 0, "requested_hz"), 0.0),
            (("actions", 0, "count"), 0),
        ):
            document = copy.deepcopy(manifest())
            target = document
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = value
            with self.assertRaises(r0c.ContractError):
                r0c.validate_manifest(document)

    def test_unknown_action_kind_is_rejected(self) -> None:
        document = copy.deepcopy(manifest())
        document["actions"][0]["kind"] = "focus"
        with self.assertRaises(r0c.ContractError):
            r0c.validate_manifest(document)

    def test_repeats_above_one_are_refused(self) -> None:
        document = copy.deepcopy(manifest())
        document["scenario"]["repeats"] = 2
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "NOT_IMPLEMENTED")

    def test_burst_above_one_is_refused(self) -> None:
        document = copy.deepcopy(manifest())
        document["actions"][0]["burst"] = 4
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "NOT_IMPLEMENTED")

    def test_order_must_match_the_actions(self) -> None:
        document = copy.deepcopy(manifest())
        document["scenario"]["order"] = ["pointer"]
        with self.assertRaises(r0c.ContractError):
            r0c.validate_manifest(document)

    def test_a_single_family_must_declare_its_own_actions(self) -> None:
        document = copy.deepcopy(manifest())
        document["actions"] = [
            {"kind": "pointer", "requested_hz": 20.0, "burst": 1, "count": 10}
        ]
        document["scenario"]["order"] = ["pointer"]
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "INVALID_VALUE")

    def test_the_total_action_count_is_bounded(self) -> None:
        document = copy.deepcopy(manifest())
        document["actions"] = [
            {"kind": "keyboard", "requested_hz": 200.0, "burst": 1, "count": 60_000,
             "keys": ["TAB"]},
            {"kind": "pointer", "requested_hz": 200.0, "burst": 1, "count": 60_000},
        ]
        document["scenario"]["order"] = ["keyboard", "pointer"]
        document["scenario"]["family"] = "combined"
        document["scenario"]["active_start_ms"] = 0
        document["scenario"]["active_end_ms"] = 300_000
        document["scenario"]["duration_ms"] = 300_000
        document["scenario"]["warmup_ms"] = 0
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "OUT_OF_RANGE")


class TraceTests(unittest.TestCase):
    def test_the_same_seed_reproduces_the_same_trace(self) -> None:
        document = manifest()
        first = r0c.plan_trace(document)
        second = r0c.plan_trace(copy.deepcopy(document))
        self.assertEqual(first["trace_sha256"], second["trace_sha256"])
        self.assertEqual([a["sequence"] for a in first["actions"]], [a["sequence"] for a in second["actions"]])

    def test_a_different_seed_changes_the_trace(self) -> None:
        document = manifest()
        first = r0c.plan_trace(document)
        document["scenario"]["seed"] = 8
        second = r0c.plan_trace(document)
        self.assertNotEqual(first["trace_sha256"], second["trace_sha256"])

    def test_trace_identity_mismatch_is_rejected(self) -> None:
        document = manifest()
        trace = r0c.plan_trace(document)
        changed = copy.deepcopy(document)
        changed["scenario"]["seed"] = 9
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_trace(trace, changed)
        self.assertEqual(caught.exception.code, "TRACE_MISMATCH")

    def test_trace_validation_round_trips(self) -> None:
        document = manifest()
        trace = r0c.plan_trace(document)
        r0c.validate_trace(trace, document)

    def test_action_count_is_bounded(self) -> None:
        document = copy.deepcopy(manifest())
        document["actions"][0]["count"] = r0c.MAX_ACTIONS + 1
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_manifest(document)
        self.assertEqual(caught.exception.code, "OUT_OF_RANGE")

    def test_a_trace_that_leaves_the_active_window_is_refused(self) -> None:
        document = copy.deepcopy(manifest())
        document["actions"][0]["count"] = 400
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.plan_trace(document)
        self.assertEqual(caught.exception.code, "OUT_OF_RANGE")

    def test_the_trace_write_cap_is_enforced(self) -> None:
        document = copy.deepcopy(manifest())
        document["scenario"]["duration_ms"] = 300_000
        document["scenario"]["warmup_ms"] = 0
        document["scenario"]["active_start_ms"] = 0
        document["scenario"]["active_end_ms"] = 300_000
        document["actions"][0] = {
            "kind": "posted-saturation", "requested_hz": 200.0, "burst": 1, "count": 60_000,
        }
        document["scenario"]["order"] = ["posted-saturation"]
        document["scenario"]["family"] = "posted-saturation"
        trace = r0c.plan_trace(document)
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_trace(trace, document)
        self.assertEqual(caught.exception.code, "OUT_OF_RANGE")
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.write_trace(trace, str(Path(__file__).resolve().parent / "should-not-exist.json"))
        self.assertEqual(caught.exception.code, "OUT_OF_RANGE")


class ClassificationTests(unittest.TestCase):
    def test_classification_is_closed(self) -> None:
        interval = 50_000_000
        self.assertEqual(r0c.classify(0, 10, True, interval_ns=interval), "on-time")
        self.assertEqual(r0c.classify(0, interval + 1, True, interval_ns=interval), "late")
        self.assertEqual(r0c.classify(0, r0c.MAX_LAG_MS * 1_000_000 + 1, True, interval_ns=interval), "dropped")
        self.assertEqual(r0c.classify(0, 10, False, interval_ns=interval), "failed")
        self.assertEqual(r0c.classify(0, None, False, interval_ns=interval, cancelled=True), "cancelled")
        self.assertEqual(r0c.classify(0, None, False, interval_ns=interval, suppressed=True), "suppressed")
        self.assertEqual(r0c.classify(100, 10, True, interval_ns=interval), "early")


class EndpointTests(unittest.TestCase):
    def test_an_implemented_endpoint_without_records_is_not_observed(self) -> None:
        matrix = r0c.endpoint_matrix({})
        journal = [item for item in matrix if item["name"] in r0c.JOURNAL_KINDS]
        self.assertEqual(len(journal), 6)
        for item in journal:
            self.assertEqual(item["status"], "NOT_MEASURED")
            self.assertEqual(item["reason"], "NO_RETAINED_VALID_RECORD")

    def test_only_endpoints_with_retained_records_are_observed(self) -> None:
        matrix = r0c.endpoint_matrix({0: 3, 2: 1})
        by_name = {item["name"]: item for item in matrix}
        self.assertEqual(by_name["input-dispatch"]["status"], "OBSERVED")
        self.assertEqual(by_name["input-dispatch"]["records"], 3)
        self.assertEqual(by_name["gpui-cpu-draw"]["status"], "OBSERVED")
        self.assertEqual(by_name["local-surface-mutation"]["status"], "NOT_MEASURED")
        self.assertEqual(by_name["local-notify"]["status"], "NOT_MEASURED")

    def test_every_missing_endpoint_carries_a_reason(self) -> None:
        matrix = r0c.endpoint_matrix({})
        missing = [item for item in matrix if item["status"] == "NOT_MEASURED"]
        self.assertTrue(missing)
        for item in missing:
            self.assertTrue(item.get("reason"))

    def test_a_missing_endpoint_cannot_be_promoted(self) -> None:
        self.assertIn("present", r0c.ENDPOINTS_NOT_MEASURED)


if __name__ == "__main__":
    unittest.main()
