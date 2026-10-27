"""Consumer-visible false-green negatives over the retained actual W4 record."""
from __future__ import annotations

import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w2_resource_verify import sha
from w4_region_verify import verify

ROOT = Path(__file__).resolve().parents[5]
RECORD = ROOT / "docs/development/evidence-published/img2-w4-latest/real-native-matrix.json"


class W4RecordTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = json.loads(RECORD.read_bytes())

    def reject(self, mutation, error=None, source=False):
        record = copy.deepcopy(self.native)
        mutation(record)
        result = verify(record, ROOT if source else None)
        self.assertFalse(result["ok"], result)
        if error:
            self.assertEqual(result["error"], error)

    def worker(self, record, case_id):
        return next(w for w in record["workers"] if w["case_id"] == case_id)

    def set_result(self, worker, **fields):
        worker["probe_result"].update(fields)
        worker["stdout_utf8"] = json.dumps(worker["probe_result"]) + "\n"

    def retrace(self, worker):
        raw = "".join(json.dumps(s, separators=(",", ":")) + "\n" for s in worker["stages"])
        worker.update(trace_utf8=raw, trace_sha256=sha(raw.encode()),
                      trace_bytes=len(raw.encode()), trace_records=len(worker["stages"]),
                      trace_partial_bytes=0)

    def test_retained_record_is_valid(self):
        result = verify(self.native)
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["roi_pixels_verified"], 11)

    def test_changed_output_digest(self):
        def change(record):
            self.worker(record, "roi_center_png")["output_sha256"] = "sha256:" + "0" * 64
        self.reject(change, "roi_output_identity_invalid")

    def test_changed_roi_dimensions(self):
        def change(record):
            worker = self.worker(record, "roi_center_png")
            self.set_result(worker, roi_width=worker["probe_result"]["roi_width"] + 1)
        self.reject(change, "roi_identity_invalid")

    def test_changed_source_dimensions(self):
        def change(record):
            next(f for f in record["fixtures"] if f["id"] == "png_rgb")["width"] += 1
        self.reject(change, "full_frame_requirement_invalid")

    def test_changed_expected_pixel(self):
        def change(record):
            self.worker(record, "roi_center_png")["oracle"]["expected_rgba_sha256"] = "0" * 64
        self.reject(change, "roi_pixel_mismatch")

    def test_out_of_bounds_accepted(self):
        def change(record):
            worker = self.worker(record, "roi_x_outside")
            worker.update(outcome="decoded", exit_code=0, output_present=False)
            self.set_result(worker, outcome="decoded", admission_status="admitted",
                            materialization="completed")
        self.reject(change, "probe_exit_result_mismatch")

    def test_full_frame_over_budget_admitted(self):
        def change(record):
            worker = self.worker(record, "full_frame_budget_small_roi")
            worker.update(outcome="decoded", exit_code=0)
            self.set_result(worker, outcome="decoded", admission_status="admitted",
                            materialization="completed")
        self.reject(change, "probe_exit_result_mismatch")

    def test_roi_bytes_impersonate_full_frame_requirement(self):
        def change(record):
            worker = self.worker(record, "full_frame_budget_small_roi")
            self.set_result(worker, full_frame_decoded_bytes=worker["probe_result"]["roi_output_bytes_required"])
        self.reject(change, "full_frame_requirement_invalid")

    def test_onload_reached_after_refusal(self):
        def change(record):
            worker = self.worker(record, "roi_x_outside")
            worker["stages"].insert(-1, {"stage": "onload_entered", "seq": 0, "pid": worker["pid"],
                                         "process_start_utc_ticks": worker["start_utc_ticks"]})
            for seq, stage in enumerate(worker["stages"], 1):
                stage["seq"] = seq
            self.retrace(worker)
        self.reject(change)

    def test_missing_refused_stage(self):
        def change(record):
            worker = self.worker(record, "roi_zero_width")
            worker["stages"] = [s for s in worker["stages"] if s["stage"] != "refused"]
            for seq, stage in enumerate(worker["stages"], 1):
                stage["seq"] = seq
            self.retrace(worker)
        self.reject(change, "native_stage_set_incomplete")

    def test_missing_probe_exit(self):
        def change(record):
            worker = self.worker(record, "roi_center_png")
            worker["stages"].pop()
            self.retrace(worker)
        self.reject(change, "native_incomplete")

    def test_trace_bytes_do_not_match_sha(self):
        def change(record):
            worker = self.worker(record, "roi_center_png")
            worker["trace_utf8"] += " "
        self.reject(change, "trace_identity_invalid")

    def test_unsupported_promoted_to_decoded(self):
        def change(record):
            worker = self.worker(record, "unsupported_source")
            worker.update(outcome="decoded", exit_code=0, output_present=True,
                          output_hex="00", output_bytes=1, published_success=True)
            worker["probe_result"].update(outcome="decoded", admission_status="admitted",
                                          materialization="completed")
            worker["stages"] = copy.deepcopy(self.worker(self.native, "roi_center_png")["stages"])
            for seq, stage in enumerate(worker["stages"], 1):
                stage["seq"] = seq
                stage["pid"] = worker["pid"]
                stage["process_start_utc_ticks"] = worker["start_utc_ticks"]
            self.retrace(worker)
        self.reject(change)

    def test_roi_success_cannot_claim_partial_decode(self):
        def change(record):
            self.set_result(self.worker(record, "roi_center_png"), true_partial_decode="verified")
        self.reject(change, "false_partial_decode_claim")

    def test_sequential_or_tiled_claim_without_evidence(self):
        def change(record):
            self.set_result(self.worker(record, "roi_center_png"), sequential_decode="verified")
        self.reject(change, "false_partial_decode_claim")

    def test_cancellation_promoted_to_decoded(self):
        def change(record):
            worker = self.worker(record, "cancel_roi")
            worker.update(outcome="decoded", published_success=True, output_present=True)
        self.reject(change, "output_after_stop")

    def test_old_source_digest_is_rejected(self):
        def change(record):
            name = next(iter(record["source_identity"]["files"]))
            record["source_identity"]["files"][name]["lf_sha256"] = "sha256:" + "0" * 64
        self.reject(change, "source_digest_mismatch", source=True)

    def test_bgr555_control_cannot_be_reclassified_as_bgr565(self):
        def change(record):
            self.set_result(self.worker(record, "roi_full_bgr555_control"),
                            pixel_format="Bgr565", bits_per_channel=6)
        self.reject(change, "source_format_mismatch")

    def test_bgr565_cannot_be_reclassified_as_bgr555(self):
        def change(record):
            self.set_result(self.worker(record, "roi_full_bgr565"),
                            pixel_format="Bgr555", bits_per_channel=5)
        self.reject(change, "source_format_mismatch")

    def test_cancel_cause_is_required(self):
        def change(record):
            self.worker(record, "cancel_roi")["cancellation"]["cause"] = "unattributable"
        self.reject(change, "stop_not_confirmed")

    def test_cancel_method_is_required(self):
        def change(record):
            self.worker(record, "timeout_roi")["cancellation"]["method"] = "never_issued"
        self.reject(change, "stop_not_confirmed")

    def test_cancel_request_time_is_required(self):
        def change(record):
            self.worker(record, "cancel_roi")["cancellation"]["requested_ns"] = None
        self.reject(change, "stop_not_confirmed")

    def test_stop_exit_code_must_match_reap(self):
        def change(record):
            worker = self.worker(record, "cancel_roi")
            worker["exit_code"] = 0
        self.reject(change, "stop_not_confirmed")

    def test_exit_before_request_must_be_false(self):
        def change(record):
            self.worker(record, "cancel_roi")["cancellation"]["exit_before_request"] = True
        self.reject(change, "stop_not_confirmed")

    def test_returncode_before_request_must_be_absent(self):
        def change(record):
            self.worker(record, "timeout_roi")["cancellation"]["returncode_before_request"] = 0
        self.reject(change, "stop_not_confirmed")

    def test_last_observed_stage_must_be_the_hold(self):
        def change(record):
            self.worker(record, "cancel_roi")["cancellation"]["last_observed_stage"] = "onload_entered"
        self.reject(change, "stop_not_confirmed")

    def test_stop_reason_must_match_case(self):
        def change(record):
            self.worker(record, "cancel_roi")["cancellation"]["reason"] = "timeout"
        self.reject(change, "stop_not_confirmed")

    def test_stop_reachability_must_be_reached(self):
        def change(record):
            self.worker(record, "timeout_roi")["cancellation"]["reachability"] = "not_reached"
        self.reject(change, "stop_not_confirmed")

    def test_duplicate_and_missing_case(self):
        self.reject(lambda r: r["workers"].pop(), "worker_count_invalid")
        self.reject(lambda r: r["workers"].append(copy.deepcopy(r["workers"][0])), "worker_count_invalid")


if __name__ == "__main__":
    unittest.main()
