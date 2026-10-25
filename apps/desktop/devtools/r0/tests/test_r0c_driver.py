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


def manifest() -> dict:
    return r0c.load_manifest(str(MANIFEST))


def probe_for(trace: dict, *, generation: int = 1, pid: int = 4321, title: str = "COS Desktop") -> dict:
    return {
        "generation": generation,
        "window": {"pid": pid, "title": title},
        "actions": [
            {
                "sequence": action["sequence"],
                "kind": action["kind"],
                "planned_ns": action["planned_ns"],
                "actual_ns": action["planned_ns"] + 1_000_000,
                "classification": "on-time",
            }
            for action in trace["actions"]
        ],
        "close": {"posted": True},
    }


def journal() -> dict:
    return {
        "capture_sha256": "sha256:" + "1" * 64,
        "counts": {"0": 140, "1": 140, "2": 51, "3": 51},
        "summary": {},
        "scope_ns": 30_000_000_000,
    }


def result() -> dict:
    document = manifest()
    trace = r0c.plan_trace(document)
    return r0c_driver.assemble_result(
        document,
        trace,
        probe_for(trace),
        journal(),
        0,
        {"wrapper_exit": 0, "child_reaped": True, "close_posted": True, "timed_out": False, "cancelled": False},
        "COMPLETE",
        "",
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
        document["actions"] = [
            {"kind": "held-key", "requested_hz": 30.0, "burst": 1, "count": 20, "keys": ["TAB"]}
        ]
        trace = r0c.plan_trace(document)
        text = json.dumps(trace)
        for forbidden in ("focus", "ime", "composition"):
            self.assertNotIn(forbidden, text.lower())
        self.assertEqual({action["kind"] for action in trace["actions"]}, {"held-key"})


class ResultTests(unittest.TestCase):
    def test_requested_and_achieved_cadence_are_recorded_separately(self) -> None:
        document = result()
        r0c.validate_result(document)
        self.assertIn("keyboard", document["requested_cadence"])
        self.assertIn("keyboard", document["achieved_cadence"])
        self.assertNotEqual(document["requested_cadence"], document["achieved_cadence"])

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

    def test_result_identity_mismatch_is_rejected(self) -> None:
        document = result()
        changed = copy.deepcopy(manifest())
        changed["scenario"]["seed"] = 99
        with self.assertRaises(r0c.ContractError):
            r0c.check_identity(changed, document)


class ProbeVerificationTests(unittest.TestCase):
    def test_generation_identity_and_trace_are_checked(self) -> None:
        document = manifest()
        trace = r0c.plan_trace(document)
        r0c_driver.verify_probe(probe_for(trace), document, trace, set())
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace, generation=2), document, trace, set())
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace, title="Other"), document, trace, set())
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(probe_for(trace, pid=777), document, trace, {777})
        missing = probe_for(trace)
        missing["actions"].pop()
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(missing, document, trace, set())
        duplicate = probe_for(trace)
        duplicate["actions"].append(dict(duplicate["actions"][0]))
        with self.assertRaises(r0c_driver.DriverError):
            r0c_driver.verify_probe(duplicate, document, trace, set())


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

    def test_oversized_output_is_bounded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "loud.py"
            script.write_text("print('x' * 100000)\n", encoding="utf-8")
            code, stdout, _ = r0c_driver._run_bounded(
                [sys.executable, "-B", str(script)], timeout_s=10
            )
            self.assertEqual(code, 0)
            self.assertLessEqual(len(stdout), r0c_driver.MAX_PROBE_BYTES)


class InputMessageTests(unittest.TestCase):
    def test_held_key_posts_key_down_only_and_keyboard_pairs(self) -> None:
        self.assertEqual(len(r0c_input._send(None, 1, {"kind": "keyboard", "sequence": 0,
                                                       "payload": {"key": "TAB"}})), 2)
        self.assertEqual(len(r0c_input._send(None, 1, {"kind": "held-key", "sequence": 0,
                                                       "payload": {"key": "TAB"}})), 1)
        self.assertEqual(len(r0c_input._send(None, 1, {"kind": "posted-saturation", "sequence": 0,
                                                       "payload": {}})), 1)
        self.assertEqual(len(r0c_input._send(None, 1, {"kind": "scroll", "sequence": 0,
                                                       "payload": {"delta": 120}})), 1)

    def test_close_is_guarded_against_a_second_attempt(self) -> None:
        source = (ROOT / "r0c_input.py").read_text(encoding="utf-8")
        self.assertIn('if not close["posted"]', source)
        self.assertIn('close["attempts"] += 1', source)

    def test_the_probe_never_changes_focus_or_ime(self) -> None:
        source = (ROOT / "r0c_input.py").read_text(encoding="utf-8")
        for forbidden in ("SetForegroundWindow", "SetFocus", "ImmGetContext", "ImmSetComposition"):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
