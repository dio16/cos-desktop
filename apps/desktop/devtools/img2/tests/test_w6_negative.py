"""The24 W6 failure classes mutate retained real Windows observations."""
from __future__ import annotations

import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w6_memory_verify import BOUNDS, verify

ROOT = Path(__file__).resolve().parents[5]
REPORT = ROOT / "docs/development/evidence-published/img2-w6-latest/native-memory-matrix.json"


def sample(record):
    return record["experiments"][6]["windows"][0]["samples"][0]


def first_window(record):
    return record["experiments"][6]["windows"][0]


def sync_sampler(record):
    for group in record["experiments"]:
        for window in group["windows"]:
            raw = json.loads(window["observer"]["stdout_utf8"])
            raw["samples"] = window["samples"]
            window["observer"]["stdout_utf8"] = json.dumps(raw, separators=(",", ":")) + "\n"
            window["observer"]["stdout_bytes"] = len(window["observer"]["stdout_utf8"].encode("utf-8"))


def mutation(record, number):
    group = record["experiments"][6]
    window = group["windows"][0]
    worker = group["workers"][0]
    if number == 1:
        sample(record)["pid"] = group["workers"][1]["pid"]
    elif number == 2:
        sample(record)["start_utc_ticks"] += 1
    elif number == 3:
        worker["exe_sha256"] = "sha256:" + "0" * 64
    elif number == 4:
        group["workers"].pop()
    elif number == 5:
        group["windows"].pop()
    elif number == 6:
        window["samples"].reverse()
    elif number == 7:
        sample(record)["private_bytes"] = -1
    elif number == 8:
        sample(record)["working_set_bytes"] = 0
    elif number == 9:
        window["totals"]["private_bytes"] += 1
    elif number == 10:
        group["metrics"]["max_observed_window_private"] += 1
    elif number == 11:
        worker["exited_ns"] = window["end_ns"] - 1
    elif number == 12:
        window["observer"]["exit_code"] = 1
    elif number == 13:
        window["observer"]["stdout_bytes"] -= 1
    elif number == 14:
        record["allocation_scope"]["maximum"] = "TRUE_CONCURRENT_ALLOCATION_PEAK"
    elif number == 15:
        record["allocation_scope"]["decoder_internal_scratch_peak"] = window["totals"]["private_bytes"]
    elif number == 16:
        record["allocation_scope"]["explicit_known_reservations_bytes"] = group["requirements"]["decoded_bytes_required"]
    elif number == 17:
        record["matched_run_differences"][0]["same_instant"] = True
    elif number == 18:
        record["allocation_scope"]["wic_decoder_latency_ns"] = group["end_ns"] - group["start_ns"]
    elif number == 19:
        window["samples"].pop()
    elif number == 20:
        stopped = record["experiments"][10]["workers"][0]
        stopped.update(outcome="decoded", published_success=True, output_present=True)
    elif number == 21:
        # The native terminal still says refusal: an outer decoded flag must not win.
        worker["probe_result"]["outcome"] = "oversize"
        worker["probe_result"]["admission_status"] = "refused"
        worker["probe_result"]["materialization"] = "not_requested"
        worker["stdout_utf8"] = json.dumps(worker["probe_result"])
        worker["stdout_bytes"] = len(worker["stdout_utf8"].encode("utf-8"))
    elif number == 22:
        record["source_identity"]["source_commit"] = "d75df9d0fef8d9eca39c41c563ada11252ed21cb"
    elif number == 23:
        group["workers"].append(copy.deepcopy(worker))
    elif number == 24:
        record["work_bytes"] = BOUNDS["max_work_bytes"] + 1
    else:
        raise ValueError("unknown negative class")
    if number in (1, 2, 6, 7, 8, 19):
        sync_sampler(record)


NEGATIVE_NAMES = (
    "pid_swap", "start_ticks", "executable_identity", "worker_missing", "window_missing",
    "sampling_order", "negative_counter", "zero_counter", "window_totals", "observed_maximum",
    "exit_during_window", "observer_nonzero", "truncated_capture_as_complete", "false_concurrent_peak",
    "private_as_scratch", "requirement_as_reservation", "matched_runs_as_same_instant",
    "hold_as_decoder_latency", "missing_sample_as_success", "cancel_as_published_decode",
    "refusal_as_decode", "old_source_identity", "worker_count_bound", "resource_bounds",
)


class NativeMutationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = json.loads(REPORT.read_bytes())
        result = verify(cls.native, ROOT)
        if not result["ok"] or not result["closure_eligible"]:
            raise AssertionError("Real W6 control record is not eligible: " + str(result))

    def test_all24_independent_mutation_classes_are_rejected(self):
        for number, name in enumerate(NEGATIVE_NAMES, 1):
            with self.subTest(number=number, mutation=name):
                record = copy.deepcopy(self.native)
                mutation(record, number)
                self.assertFalse(verify(record, ROOT)["ok"], name)

    def test_unsupported_terminal_cannot_be_promoted_by_outer_decode(self):
        record = copy.deepcopy(self.native)
        mutation(record, 21)
        worker = record["experiments"][6]["workers"][0]
        worker["probe_result"]["outcome"] = "unsupported"
        worker["stdout_utf8"] = json.dumps(worker["probe_result"])
        worker["stdout_bytes"] = len(worker["stdout_utf8"].encode("utf-8"))
        self.assertFalse(verify(record, ROOT)["ok"])

    def test_extra_decode_phase_cannot_expand_total_case_budget(self):
        record = copy.deepcopy(self.native)
        record["consumer"] = {"record": {"observations": [{"case_id": "extra"}]}}
        self.assertEqual(verify(record, ROOT).get("error"), "undeclared_decode_phase")

    def test_failed_group_cannot_be_claimed_complete(self):
        record = copy.deepcopy(self.native)
        record["experiments"][0]["failure"] = "observer failed"
        self.assertEqual(verify(record, ROOT).get("error"), "group_failure_not_closed")

    def test_structural_replay_does_not_claim_fixed_source_identity(self):
        result = verify(self.native)
        self.assertTrue(result["ok"])
        self.assertFalse(result["source_identity_verified"])
        self.assertFalse(result["closure_eligible"])

    def test_sample_and_report_limits_are_also_enforced(self):
        for field in ("samples", "report"):
            with self.subTest(bound=field):
                record = copy.deepcopy(self.native)
                if field == "samples":
                    record["bounds"]["max_samples"] += 1
                else:
                    record["unbounded_payload"] = "x" * BOUNDS["max_report_bytes"]
                self.assertFalse(verify(record, ROOT)["ok"])


if __name__ == "__main__":
    unittest.main()
