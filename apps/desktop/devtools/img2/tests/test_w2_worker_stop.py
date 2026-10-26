"""Stop-lifecycle regressions: a request time never proves a cancellation.

The fake processes below exercise the real ``Worker.stop``/``Worker.finish``
code without spawning PowerShell or WIC.
"""
from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w2_resource_matrix import Worker, cancel_record, stop_outcome  # noqa: E402


class FakeProc:
    """Minimal stand-in for subprocess.Popen with an observable kill count."""

    def __init__(self, pre_exit=None, kill_error=None, exit_code=1):
        self.pid = 4242
        self.returncode = pre_exit
        self.pre_exit = pre_exit
        self.kill_error = kill_error
        self.exit_code = exit_code
        self.kills = 0
        self.waits = 0

    def poll(self):
        return self.returncode

    def kill(self):
        if self.kill_error is not None:
            # The child completed between poll() and kill(); only the natural code is observable.
            self.returncode = self.exit_code
            raise self.kill_error
        self.kills += 1
        self.returncode = self.exit_code

    def wait(self, timeout=None):
        self.waits += 1
        return self.returncode


HOLD_ROW = {"stage": "before_onload", "pid": 4242, "process_start_utc_ticks": 7,
            "artificial_instrumentation_hold": True, "pause_timeout_ms": 10000}


def make_worker(proc, kind="cancel", rows=None):
    w = object.__new__(Worker)
    w.cid, w.fid, w.kind, w.hold = "cancel_before_onload", "detail_png", kind, "before_onload"
    w.proc = proc
    w.cancel = cancel_record(kind)
    w.rows = [HOLD_ROW] if rows is None else rows
    w.fixture = {"sha256": "sha256:" + "1" * 64}
    w.started = 1
    w.read_trace = lambda: w.rows
    return w


class StopLifecycleTests(unittest.TestCase):
    def test_exit_zero_before_request_keeps_real_result(self):
        proc = FakeProc(pre_exit=0)
        w = make_worker(proc)
        w.stop("cancel")
        self.assertEqual(w.cancel["state"], "already_exited")
        self.assertIsNone(w.cancel["requested_ns"])
        self.assertFalse(w.cancel["kill_issued"])
        self.assertEqual(proc.kills, 0)
        self.assertEqual(w.cancel["returncode_before_request"], 0)
        self.assertEqual(stop_outcome(w.cancel, {"outcome": "decoded"}), "decoded")
        self.assertNotIn(stop_outcome(w.cancel, {"outcome": "decoded"}), ("cancelled", "timeout"))

    def test_exit_one_before_request_is_not_a_cancellation(self):
        proc = FakeProc(pre_exit=1)
        w = make_worker(proc)
        w.stop("cancel")
        self.assertEqual(w.cancel["state"], "already_exited")
        self.assertEqual(proc.kills, 0)
        self.assertEqual(stop_outcome(w.cancel, None), "error")

    def test_confirmed_kill_requires_issued_terminate_and_nonzero_exit(self):
        proc = FakeProc(exit_code=1)
        w = make_worker(proc, rows=[HOLD_ROW])
        w.stop("cancel")
        self.assertEqual(w.cancel["state"], "stop_confirmed")
        self.assertTrue(w.cancel["kill_issued"])
        self.assertTrue(w.cancel["alive_at_request"])
        self.assertTrue(w.cancel["reap_confirmed"])
        self.assertEqual(proc.kills, 1)
        self.assertEqual(w.cancel["exit_after_request"], 1)
        self.assertEqual(w.cancel["reachability"], "reached")
        self.assertEqual(stop_outcome(w.cancel, None), "cancelled")

    def test_timeout_reason_is_distinct_from_cancel(self):
        w = make_worker(FakeProc(exit_code=1), kind="timeout", rows=[HOLD_ROW])
        w.hold = "before_onload"
        w.stop("timeout")
        self.assertEqual(stop_outcome(w.cancel, None), "timeout")

    def test_kill_natural_exit_race_is_not_verified(self):
        proc = FakeProc(kill_error=OSError("gone"), exit_code=0)
        w = make_worker(proc)
        w.stop("cancel")
        self.assertEqual(w.cancel["state"], "unverified_race")
        self.assertIsNotNone(w.cancel["requested_ns"])
        self.assertFalse(w.cancel["kill_issued"])
        self.assertEqual(w.cancel["kill_error"], "OSError")
        self.assertEqual(w.cancel["exit_after_request"], 0)
        self.assertEqual(stop_outcome(w.cancel, {"outcome": "decoded"}), "not_verified")

    def test_stop_is_idempotent(self):
        proc = FakeProc(exit_code=1)
        w = make_worker(proc, rows=[HOLD_ROW])
        w.stop("cancel")
        w.stop("timeout")
        self.assertEqual(proc.kills, 1)
        self.assertEqual(w.cancel["reason"], "cancel")

    def test_finish_preserves_output_of_natural_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "observe").mkdir()
            w = make_worker(FakeProc(pre_exit=0))
            w.work = work
            w.observe = work / "observe"
            w.output = work / "output.png"
            w.output.write_bytes(b"real-output")
            w.stdout_path, w.stderr_path = work / "stdout.json", work / "stderr.txt"
            w.stdout_path.write_text(json.dumps({"outcome": "decoded"}), encoding="utf-8")
            w.stderr_path.write_text("", encoding="utf-8")
            w.stdout, w.stderr = w.stdout_path.open("ab"), w.stderr_path.open("ab")
            w.identity_verified = False
            w.fixture = {"sha256": "sha256:" + "1" * 64}
            w.rows = [{"stage": "before_onload", "pid": 4242, "process_start_utc_ticks": 7}]
            w.stop("cancel")
            record = w.finish("sha256:" + "0" * 64)
            self.assertEqual(record["outcome"], "decoded")
            self.assertTrue(record["output_present"])
            self.assertTrue(record["published_success"])
            self.assertEqual(record["removed_owned_output_paths"], [])
            self.assertTrue(w.output.is_file())

    def test_kill_without_hold_evidence_is_not_confirmed(self):
        proc = FakeProc(exit_code=1)
        w = make_worker(proc, kind="onload_attempt", rows=[{"stage": "onload_entered"}])
        w.hold = "before_onload"
        w.stop("cancel")
        self.assertEqual(w.cancel["state"], "unverified_race")
        self.assertTrue(w.cancel["kill_issued"])
        self.assertFalse(w.cancel["hold_established"])
        self.assertEqual(w.cancel["cause"], "unattributable")
        self.assertEqual(proc.kills, 1)
        self.assertEqual(stop_outcome(w.cancel, None), "not_verified")

    def test_hold_row_without_marker_is_not_a_hold(self):
        proc = FakeProc(exit_code=1)
        w = make_worker(proc, rows=[{"stage": "before_onload"}])
        w.stop("cancel")
        self.assertEqual(w.cancel["state"], "unverified_race")
        self.assertFalse(w.cancel["hold_established"])
        self.assertEqual(stop_outcome(w.cancel, None), "not_verified")

    def test_finish_removes_output_only_after_confirmed_stop(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "observe").mkdir()
            w = make_worker(FakeProc(exit_code=1))
            w.work = work
            w.observe = work / "observe"
            w.output = work / "output.png"
            w.output.write_bytes(b"partial")
            w.stdout_path, w.stderr_path = work / "stdout.json", work / "stderr.txt"
            w.stdout_path.write_text("", encoding="utf-8")
            w.stderr_path.write_text("", encoding="utf-8")
            w.stdout, w.stderr = w.stdout_path.open("ab"), w.stderr_path.open("ab")
            w.identity_verified = False
            w.rows = [HOLD_ROW]
            w.stop("cancel")
            record = w.finish("sha256:" + "0" * 64)
            self.assertEqual(record["outcome"], "cancelled")
            self.assertFalse(record["output_present"])
            self.assertEqual(len(record["removed_owned_output_paths"]), 1)
            self.assertFalse(w.output.exists())


if __name__ == "__main__":
    unittest.main()
