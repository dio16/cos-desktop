"""Consumer-visible negative evidence cases against the retained real W2 run.

No process is started here. The positive record is actual Native evidence, not a
mock's echo of the verifier; byte rehashing is separately exercised by the CLI.
"""
from __future__ import annotations

import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w2_resource_verify import verify, verify_file

ROOT = Path(__file__).resolve().parents[5]
RECORD = ROOT / "docs/development/evidence-published/img2-w2-latest/native-matrix.json"


class W2EvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Missing mandatory retained evidence is an error, never a skipped PASS.
        cls.observed = json.loads(RECORD.read_text(encoding="utf-8"))

    def reject(self, mutate, token):
        record = copy.deepcopy(self.observed)
        mutate(record)
        result = verify(record)
        self.assertFalse(result["ok"], result)
        self.assertTrue(any(token in e for e in result["errors"]), result)

    def test_retained_real_record_and_current_source_replay(self):
        result = verify(self.observed, source_root=ROOT)
        self.assertEqual(result["errors"], [])
        self.assertTrue(result["ok"])
        self.assertEqual(result["mode"], "report_only")

    def test_record_missing(self):
        self.assertFalse(verify(None)["ok"])

    def test_empty_observation_population(self):
        self.reject(lambda r: r.update(workers=[]), "worker_set_missing")

    def test_duplicate_worker_identity(self):
        self.reject(lambda r: r["workers"].append(copy.deepcopy(r["workers"][0])), "worker_case_set_invalid")

    def test_missing_case_cannot_be_removed_from_expected_set(self):
        self.reject(lambda r: r["expected_case_ids"].pop(), "expected_case_set_invalid")

    def test_source_digest_mismatch(self):
        record = copy.deepcopy(self.observed)
        record["source_identity"]["files"]["probe"]["lf_sha256"] = "sha256:" + "0" * 64
        self.assertIn("source_digest_mismatch", verify(record, source_root=ROOT)["errors"])

    def test_process_lifetime_mismatch(self):
        self.reject(lambda r: r["workers"][0].update(start_utc_ticks=1), "process_identity_mismatch")

    def test_cross_process_clock_subtraction_rejected(self):
        self.reject(lambda r: r["workers"][0].update(cross_process_clock_subtraction=True), "cross_clock_comparison_invalid")

    def test_trace_clock_domain_mismatch(self):
        self.reject(lambda r: r["workers"][0]["stages"][0].update(clock_domain="wall_clock"), "trace_schema_or_clock_invalid")

    def test_trace_stage_reversal(self):
        def reverse(r):
            stages = r["workers"][0]["stages"]
            stages[1], stages[2] = stages[2], stages[1]
        self.reject(reverse, "trace_stage_order_invalid")

    def test_materialization_after_admission_refusal(self):
        def materialize(r):
            w = next(x for x in r["workers"] if x["case_id"] == "refuse_oversized_png")
            extra = dict(w["stages"][-1], stage="onload_entered")
            w["stages"].insert(-1, extra)
        self.reject(materialize, "materialization_after_refusal")

    def test_decoded_success_requires_present_output(self):
        self.reject(lambda r: r["workers"][0].update(output_present=False), "success_output_missing")

    def test_cancelled_work_cannot_publish_success(self):
        def publish(r):
            w = next(x for x in r["workers"] if x["kind"] == "cancel")
            w["published_success"] = True
        self.reject(publish, "cancelled_success_published")

    def test_missing_memory_sample_cannot_claim_value(self):
        self.reject(lambda r: r["memory"]["working_set_bytes"].update(sample_count=0), "memory_sample_summary_invalid")

    def test_independent_process_maxima_not_a_cohort_sum(self):
        self.reject(lambda r: r["cohorts"][-1]["totals"].update(working_set_bytes=1), "cohort_total_mismatch")

    def test_sampler_failure_not_measured_success(self):
        self.reject(lambda r: r["cohorts"][0].update(status="sampler_unavailable"), "cohort_incomplete_or_invalid")

    def test_incomplete_sample_population(self):
        self.reject(lambda r: r["cohorts"][-1]["samples"].pop(), "cohort_sample_missing")

    def test_report_and_work_caps(self):
        self.reject(lambda r: r["bounds"].update(max_report_bytes=1), "report_cap_exceeded")
        self.reject(lambda r: r.update(work_bytes=r["bounds"]["max_work_bytes"] + 1), "work_or_total_deadline_exceeded")

    def test_partial_output_or_trace_not_complete(self):
        self.reject(lambda r: r["workers"][0].update(trace_complete=False), "trace_completeness_invalid")

    def test_cancel_cleanup_must_be_confirmed(self):
        self.reject(lambda r: r["cleanup"].update(complete=False), "cleanup_incomplete")
        self.reject(lambda r: r["workers"][0].update(reaped=False), "worker_cleanup_invalid")

    def test_required_cancel_cannot_be_unreached(self):
        def unset(r):
            next(x for x in r["workers"] if x["kind"] == "cancel")["cancellation"]["requested_ns"] = None
        self.reject(unset, "required_cancellation_not_reached")

    def test_timeout_distinct_from_normal_cancellation(self):
        def change(r):
            next(x for x in r["workers"] if x["kind"] == "timeout")["outcome"] = "cancelled"
        self.reject(change, "worker_expected_outcome_mismatch")

    def test_sampled_counter_cannot_be_internal_scratch_peak(self):
        self.reject(lambda r: r["memory"]["decoder_internal_scratch_peak"].update(value=123), "unobserved_peak_claimed")

    def test_actual_in_decoder_reachability_not_inferred_from_poll(self):
        def claim(r):
            next(x for x in r["workers"] if x["kind"] == "onload_attempt")["cancellation"]["reachability"] = "reached"
        self.reject(claim, "in_decoder_cancel_falsely_claimed")

    def test_partial_duplicate_and_nonfinite_json_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "record.json"
            for data in ('{"schema_version":', '{"a":1,"a":2}', '{"a":NaN}'):
                with self.subTest(data=data):
                    p.write_text(data, encoding="utf-8")
                    self.assertFalse(verify_file(p)["ok"])


if __name__ == "__main__":
    unittest.main()
