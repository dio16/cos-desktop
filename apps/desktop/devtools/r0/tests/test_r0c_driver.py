"""MP-R0C-S1 driver regressions. Standard library only, no Windows required.

The real Windows probe is exercised separately as native smoke evidence. These
tests use the fake-probe seam and a fake clock so the deterministic plan, the
bounded runner, the queue/lag rules and the result contract are reproducible.
"""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import r0c_contracts as r0c  # noqa: E402
import r0c_driver  # noqa: E402
import r0c_input  # noqa: E402

MANIFEST = ROOT / "manifests/r0c-s1-keyboard.json"
GENERATION = 1234
LAUNCHER_PID = 9001
OBSERVATION_PID = 9002


def manifest() -> dict:
    return r0c.load_manifest(str(MANIFEST))


def probe_for(trace: dict, *, generation: int = GENERATION, pid: int = OBSERVATION_PID,
              launcher_pid: int = LAUNCHER_PID, title: str = "COS Desktop") -> dict:
    return {
        "generation": generation,
        "binding": {"bound": True, "launcher_pid": launcher_pid, "helper_pid": 9003,
                    "window_pid": pid},
        "window": {"pid": pid, "title": title},
        "actions": [
            {
                "sequence": action["sequence"],
                "kind": action["kind"],
                "planned_ns": action["planned_ns"],
                "actual_ns": action["planned_ns"] + 1_000_000,
                "delivered": True,
                "suppressed": False,
                "classification": "on-time",
            }
            for action in trace["actions"]
        ],
        "close": {"posted": True},
    }


def journal() -> dict:
    return {
        "capture_sha256": "sha256:" + "1" * 64,
        "counts": {"0": 140, "1": 140, "2": 51, "3": 51, "8": 2, "9": 1},
        "summary": {},
        "scope_ns": 30_000_000_000,
    }


def limits() -> dict:
    return {
        "max_actions": r0c.MAX_ACTIONS,
        "max_lag_ms": r0c.MAX_LAG_MS,
        "probe_timeout_ms": 44_000,
        "capture_timeout_ms": 120_000,
        "output_bytes": r0c_driver.MAX_PROBE_BYTES,
    }


_MISSING = object()


def result(probe: dict | None = None, journal_document=_MISSING,
           status: str = "COMPLETE", reason: str = "") -> dict:
    document = manifest()
    trace = r0c.plan_trace(document)
    probe = probe if probe is not None else probe_for(trace)
    journal_document = journal() if journal_document is _MISSING else journal_document
    delivered = sum(1 for action in probe["actions"]
                    if action["classification"] in ("on-time", "early", "late"))
    return r0c_driver.assemble_result(
        document, trace, probe, journal_document, 0,
        {"wrapper_exit": 0, "child_reaped": True, "close_posted": True,
         "timed_out": False, "cancelled": False},
        status, reason, limits(), delivered,
    )


class PlanTests(unittest.TestCase):
    def test_plan_command_writes_a_valid_trace(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "plan.json"
            code = r0c_driver.main(["plan", "--manifest", str(MANIFEST), "--out", str(out)])
            self.assertEqual(code, 0)
            trace = json.loads(out.read_text(encoding="utf-8"))
            r0c.validate_trace(trace, manifest())

    def test_held_key_plan_never_declares_focus_or_ime(self) -> None:
        document = copy.deepcopy(manifest())
        document["scenario"]["family"] = "held-key"
        document["scenario"]["order"] = ["held-key"]
        document["actions"] = [
            {"kind": "held-key", "requested_hz": 30.0, "burst": 1, "count": 20, "keys": ["TAB"]}
        ]
        trace = r0c.plan_trace(document)
        self.assertEqual({action["kind"] for action in trace["actions"]}, {"held-key"})


class ResultTests(unittest.TestCase):
    def test_requested_and_achieved_cadence_are_recorded_separately(self) -> None:
        document = result()
        r0c.validate_result(document)
        self.assertIn("keyboard", document["requested_cadence"])
        self.assertIn("keyboard", document["achieved_cadence"])
        self.assertNotEqual(document["requested_cadence"], document["achieved_cadence"])

    def test_a_single_sample_has_no_fabricated_cadence(self) -> None:
        document = copy.deepcopy(manifest())
        document["actions"][0]["count"] = 1
        document["scenario"]["active_end_ms"] = document["scenario"]["active_start_ms"] + 1000
        trace = r0c.plan_trace(document)
        probe = probe_for(trace)
        probe["actions"][0]["actual_ns"] = trace["actions"][0]["planned_ns"]
        document_result = r0c_driver.assemble_result(
            document, trace, probe, journal(), 0,
            {"wrapper_exit": 0, "child_reaped": True, "close_posted": True,
             "timed_out": False, "cancelled": False},
            "COMPLETE", "", limits(), 1,
        )
        self.assertIsNone(document_result["achieved_cadence"]["keyboard"])
        r0c.validate_result(document_result)

    def test_missing_kinds_are_not_reported_observed(self) -> None:
        document = result(journal_document={"capture_sha256": "sha256:" + "2" * 64,
                                            "counts": {"0": 140, "1": 140, "2": 51, "3": 51},
                                            "summary": {}, "scope_ns": 1})
        by_name = {item["name"]: item for item in document["endpoints"]}
        self.assertEqual(by_name["local-surface-mutation"]["status"], "NOT_MEASURED")
        self.assertEqual(by_name["local-notify"]["status"], "NOT_MEASURED")
        self.assertEqual(by_name["input-dispatch"]["status"], "OBSERVED")

    def test_a_missing_journal_marks_every_journal_endpoint_not_measured(self) -> None:
        document = result(journal_document=None)
        by_name = {item["name"]: item for item in document["endpoints"]}
        for name in r0c.JOURNAL_KINDS:
            self.assertEqual(by_name[name]["status"], "NOT_MEASURED")
            self.assertEqual(by_name[name]["reason"], "NO_RETAINED_VALID_RECORD")
        self.assertNotIn("capture_sha256", document["journal"])

    def test_promoting_a_missing_endpoint_is_rejected(self) -> None:
        document = copy.deepcopy(result())
        for item in document["endpoints"]:
            if item["name"] == "present":
                item["status"] = "OBSERVED"
                item.pop("reason", None)
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_result(document)
        self.assertEqual(caught.exception.code, "PROMOTED_ENDPOINT")

    def test_a_missing_endpoint_needs_a_reason(self) -> None:
        document = copy.deepcopy(result())
        for item in document["endpoints"]:
            if item["name"] == "scanout":
                item.pop("reason", None)
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_result(document)
        self.assertEqual(caught.exception.code, "MISSING_FIELD")

    def test_unknown_classification_is_rejected(self) -> None:
        document = copy.deepcopy(result())
        document["actions"][0]["classification"] = "fast"
        with self.assertRaises(r0c.ContractError):
            r0c.validate_result(document)

    def test_a_complete_result_requires_every_action_delivered(self) -> None:
        document = copy.deepcopy(result())
        document["delivered_actions"] = len(document["actions"]) - 1
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_result(document)
        self.assertEqual(caught.exception.code, "INVALID_VALUE")

    def test_result_identity_mismatch_is_rejected(self) -> None:
        document = result()
        changed = copy.deepcopy(manifest())
        changed["scenario"]["seed"] = 99
        with self.assertRaises(r0c.ContractError):
            r0c.check_identity(changed, document)


    def test_an_all_failed_probe_cannot_be_complete(self) -> None:
        document = manifest()
        trace = r0c.plan_trace(document)
        probe = probe_for(trace)
        for action in probe["actions"]:
            action["delivered"] = False
            action["suppressed"] = False
            action["classification"] = "failed"
        candidate = r0c_driver.assemble_result(
            document, trace, probe, journal(), 0,
            {"wrapper_exit": 0, "child_reaped": True, "close_posted": True,
             "timed_out": False, "cancelled": False},
            "COMPLETE", "", limits(), 0,
        )
        with self.assertRaises(r0c.ContractError) as caught:
            r0c.validate_result(candidate)
        self.assertEqual(caught.exception.code, "INVALID_VALUE")

    def test_verify_rejects_a_non_complete_result(self) -> None:
        document = result(status="FAILED", reason="WORKLOAD_NOT_DELIVERED")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            args = type("Args", (), {"manifest": str(MANIFEST), "result": str(path)})()
            with self.assertRaises(r0c.ContractError) as caught:
                r0c_driver.verify(args)
            self.assertEqual(caught.exception.code, "NOT_COMPLETE")


class ProbeVerificationTests(unittest.TestCase):
    def test_generation_identity_and_trace_are_checked(self) -> None:
        document = manifest()
        trace = r0c.plan_trace(document)
        delivered = r0c_driver.verify_probe(
            probe_for(trace), document, trace, set(), LAUNCHER_PID, OBSERVATION_PID, GENERATION
        )
        self.assertEqual(delivered, len(trace["actions"]))
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace, generation=2), document, trace, set(),
                                    LAUNCHER_PID, OBSERVATION_PID, GENERATION)
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace, title="Other"), document, trace, set(),
                                    LAUNCHER_PID, OBSERVATION_PID, GENERATION)
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace, pid=777), document, trace, {777},
                                    LAUNCHER_PID, OBSERVATION_PID, GENERATION)
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace), document, trace, set(),
                                    LAUNCHER_PID, 777, GENERATION)
        missing = probe_for(trace)
        missing["actions"].pop()
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(missing, document, trace, set(),
                                    LAUNCHER_PID, OBSERVATION_PID, GENERATION)

    def test_a_launcher_binding_mismatch_is_rejected(self) -> None:
        document = manifest()
        trace = r0c.plan_trace(document)
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace, launcher_pid=1), document, trace, set(),
                                    LAUNCHER_PID, OBSERVATION_PID, GENERATION)

    def test_a_tampered_classification_is_rejected(self) -> None:
        document = manifest()
        trace = r0c.plan_trace(document)
        probe = probe_for(trace)
        probe["actions"][0]["classification"] = "late"
        with self.assertRaises(r0c_driver.DriverError) as caught:
            r0c_driver.verify_probe(probe, document, trace, set(),
                                    LAUNCHER_PID, OBSERVATION_PID, GENERATION)
        self.assertEqual(caught.exception.code, "PROBE_CLASSIFICATION_MISMATCH")


class BoundedRunnerTests(unittest.TestCase):
    def test_timeout_returns_124_and_kills_the_child(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "slow.py"
            script.write_text("import time\ntime.sleep(30)\n", encoding="utf-8")
            started = time.monotonic()
            code, stdout, stderr = r0c_driver._run_bounded(
                [sys.executable, "-B", str(script)], timeout_s=0.3
            )
            elapsed = time.monotonic() - started
            self.assertEqual(code, 124)
            self.assertEqual(stderr, "TIMEOUT")
            self.assertLess(elapsed, 5.0)

    def test_oversized_output_is_bounded_while_reading(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "loud.py"
            script.write_text("import sys\nfor _ in range(200):\n    sys.stdout.write('x' * 100000)\n",
                              encoding="utf-8")
            code, stdout, _ = r0c_driver._run_bounded(
                [sys.executable, "-B", str(script)], timeout_s=30
            )
            self.assertEqual(code, 0)
            self.assertLessEqual(len(stdout), r0c_driver.MAX_PROBE_BYTES)


class InputBehaviourTests(unittest.TestCase):
    def test_message_plans_contain_only_the_declared_messages(self) -> None:
        keyboard = r0c_input.message_plan("keyboard", {"key": "TAB"}, 0)
        held = r0c_input.message_plan("held-key", {"key": "TAB"}, 0)
        saturation = r0c_input.message_plan("posted-saturation", {}, 0)
        scroll = r0c_input.message_plan("scroll", {"delta": 120}, 0)
        self.assertEqual([message for message, _, _ in keyboard],
                         [r0c_input.WM_KEYDOWN, r0c_input.WM_KEYUP])
        self.assertEqual([message for message, _, _ in held], [r0c_input.WM_KEYDOWN])
        self.assertEqual([message for message, _, _ in saturation], [r0c_input.WM_NULL])
        self.assertEqual([message for message, _, _ in scroll], [r0c_input.WM_MOUSEWHEEL])
        allowed = {r0c_input.WM_KEYDOWN, r0c_input.WM_KEYUP, r0c_input.WM_MOUSEMOVE,
                   r0c_input.WM_LBUTTONDOWN, r0c_input.WM_LBUTTONUP,
                   r0c_input.WM_MOUSEWHEEL, r0c_input.WM_NULL}
        for kind, payload in (("keyboard", {"key": "TAB"}), ("held-key", {"key": "TAB"}),
                              ("pointer", {"x": 1, "y": 2}), ("scroll", {"delta": 120}),
                              ("posted-saturation", {})):
            for message, _, _ in r0c_input.message_plan(kind, payload, 0):
                self.assertIn(message, allowed)

    def test_close_is_posted_once(self) -> None:
        close = {"attempts": 0, "posted": False}
        calls = []

        def post() -> bool:
            calls.append(1)
            return True

        self.assertTrue(r0c_input.close_once(close, post))
        self.assertTrue(r0c_input.close_once(close, post))
        self.assertEqual(close["attempts"], 1)
        self.assertEqual(len(calls), 1)

    def test_schedule_lag_is_suppressed_not_failed(self) -> None:
        record = r0c_input.action_record(
            0, r0c.MAX_LAG_MS * 1_000_000 + 1, False, True, 50_000_000,
            r0c.MAX_LAG_MS * 1_000_000,
        )
        self.assertEqual(record["classification"], "suppressed")


class IdentityTests(unittest.TestCase):
    def test_helper_and_build_identity_are_cross_checked(self) -> None:
        document = manifest()
        s2b_manifest = {
            "identity": {
                "repository": {"sha": document["identity"]["repository_sha"]},
                "executable_sha256": document["identity"]["executable_sha256"],
                "gpui_revision": document["identity"]["gpui_revision"],
                "build": {"profile": "dev", "features": ["r0-s2b-instrumentation"]},
            },
            "workload": {"source": {"sha256": document["scenario"]["fixture_sha256"]}},
        }
        binding = {
            "product_commit": document["identity"]["repository_sha"],
            "executable_sha256": document["identity"]["executable_sha256"],
            "helper_sha256": document["identity"]["helper_sha256"],
        }
        r0c_driver._require_identity_agreement(document, s2b_manifest, binding)
        broken = copy.deepcopy(binding)
        broken["helper_sha256"] = "sha256:" + "9" * 64
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver._require_identity_agreement(document, s2b_manifest, broken)


if __name__ == "__main__":
    unittest.main()
