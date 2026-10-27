"""Consumer-visible false-green negatives over retained actual W3 Native bytes."""
from __future__ import annotations

import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w3_verify import verify
from w3_oracle import compare
from w2_resource_verify import sha
from test_w3_oracle import png

ROOT = Path(__file__).resolve().parents[5]


class W3RecordTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = json.loads((ROOT / "docs/development/evidence-published/img2-w3-latest/real-native-matrix.json").read_bytes())

    def reject(self, mutation, error=None, *, source=False):
        record = copy.deepcopy(self.native)
        mutation(record)
        result = verify(record, ROOT if source else None)
        self.assertFalse(result["ok"], result)
        if error:
            self.assertEqual(result["error"], error)

    def change_result(self, **fields):
        def change(record):
            worker = record["workers"][0]
            worker["probe_result"].update(fields)
            worker["stdout_utf8"] = json.dumps(worker["probe_result"]) + "\n"
        return change

    def test_retained_raw_bytes_replay(self):
        result = verify(self.native)
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["consumer_pixels_verified"], 5)

    def test_wrong_format_cannot_be_certified(self):
        self.reject(self.change_result(pixel_format="Bgr32"), "source_format_invalid")

    def test_wrong_depth_cannot_be_certified(self):
        self.reject(self.change_result(bits_per_channel=5), "source_format_invalid")

    def test_missing_and_duplicate_cases(self):
        self.reject(lambda r: r["workers"].pop(), "worker_case_set_invalid")
        self.reject(lambda r: r["workers"].append(copy.deepcopy(r["workers"][0])), "worker_case_set_invalid")
        self.reject(lambda r: r["fixtures"].pop(), "fixture_set_invalid")
        self.reject(lambda r: r["consumer"]["record"].update(observations=[]), "consumer_case_set_invalid")

    def test_partial_report_and_trace(self):
        self.reject(lambda r: r.update(complete=False), "incomplete_report")
        self.reject(lambda r: r["workers"][0].update(trace_complete=False), "native_incomplete")
        self.reject(lambda r: r["workers"][0].update(trace_utf8=r["workers"][0]["trace_utf8"][:-1]), "trace_identity_invalid")

    def test_source_and_input_identity(self):
        self.reject(lambda r: r["source_identity"]["files"][next(iter(r["source_identity"]["files"]))].update(lf_sha256="sha256:" + "0" * 64), "source_digest_mismatch", source=True)
        self.reject(lambda r: r["workers"][0].update(input_sha256="sha256:" + "0" * 64), "worker_identity_mismatch")

    def test_row_padding_and_arithmetic_overflow(self):
        self.reject(lambda r: r["fixtures"][0].update(logical_row_bytes=12), "fixture_accounting_mismatch")
        self.reject(lambda r: r["fixtures"][0].update(logical_source_bytes=2**64), "fixture_accounting_mismatch")
        self.reject(lambda r: r["fixtures"][0].update(encoded_row_stride=10), "fixture_accounting_mismatch")

    def test_row_order_is_not_ignored(self):
        self.reject(lambda r: r["fixtures"][0].update(row_order="top-down"), "fixture_row_order_mismatch")

    def test_empty_missing_and_changed_output(self):
        self.reject(lambda r: r["workers"][0].update(output_present=False), "decoded_output_missing")
        self.reject(lambda r: r["workers"][0].update(output_hex=""), "decoded_output_missing")
        self.reject(lambda r: r["workers"][0].update(output_hex="00"), "decoded_output_missing")

    def test_pixel_mismatch_cannot_be_reported_as_success(self):
        self.reject(lambda r: r["workers"][0]["oracle"].update(actual_rgba_sha256="0" * 64), "pixel_mismatch")
        self.reject(lambda r: r["consumer"]["record"]["observations"][0].update(pixel_digest="0" * 64), "consumer_pixel_mismatch")
        def different_raster(record):
            worker = record["workers"][0]
            output = png(5, 1, bytes((0, 0, 0, 255)) * 5)
            worker.update(output_hex=output.hex(), output_bytes=len(output), output_sha256=sha(output))
            source = next(f for f in record["fixtures"] if f["id"] == worker["fixture_id"])
            worker["oracle"] = compare(bytes.fromhex(source["input_hex"]), output)
        self.reject(different_raster, "pixel_mismatch")

    def test_stop_cannot_publish_success(self):
        for kind in ("cancel", "timeout"):
            with self.subTest(kind=kind):
                def publish(r):
                    next(w for w in r["workers"] if w["kind"] == kind)["published_success"] = True
                self.reject(publish, "success_after_stop")

    def test_nonhold_kill_cannot_be_confirmed(self):
        def unheld(r):
            next(w for w in r["workers"] if w["kind"] == "cancel")["cancellation"]["hold_established"] = False
        self.reject(unheld, "stop_not_confirmed")

    def test_timeout_requires_elapsed_deadline(self):
        def early(record):
            worker = next(w for w in record["workers"] if w["kind"] == "timeout")
            worker["cancellation"]["requested_ns"] = worker["started_ns"] + 1
        self.reject(early, "timeout_not_elapsed")

    def test_refusal_cannot_publish_output(self):
        def publish(r):
            next(w for w in r["workers"] if w["case_id"] == "max_pixels_over")["output_present"] = True
        self.reject(publish, "output_after_refusal")

    def test_oversize_bounds_and_unreaped_child(self):
        self.reject(lambda r: r["bounds"].update(max_workers=2), "bounds_invalid")
        self.reject(lambda r: r["workers"][0].update(reaped=False), "worker_not_reaped")
        self.reject(lambda r: r["cleanup"].update(complete=False), "cleanup_incomplete")

    def test_missing_materialization_stage_is_incomplete(self):
        def omit(record):
            worker = record["workers"][0]
            worker["stages"] = [s for s in worker["stages"] if s["stage"] != "onload_returned"]
            for seq, stage in enumerate(worker["stages"], 1):
                stage["seq"] = seq
            raw = "".join(json.dumps(s, separators=(",", ":")) + "\n" for s in worker["stages"])
            worker.update(trace_utf8=raw, trace_sha256=sha(raw.encode()),
                          trace_bytes=len(raw.encode()), trace_records=len(worker["stages"]))
        self.reject(omit, "native_stage_set_incomplete")

    def test_source_dimensions_must_match_input(self):
        self.reject(self.change_result(width=99), "source_dimensions_invalid")

    def test_budget_refusal_keeps_its_own_reason(self):
        def substitute(record):
            worker = next(w for w in record["workers"] if w["case_id"] == "max_pixels_over")
            worker["probe_result"]["reason_code"] = "max_dimension_exceeded"
            worker["stdout_utf8"] = json.dumps(worker["probe_result"]) + "\n"
        self.reject(substitute, "budget_refusal_reason_invalid")

    def test_bottom_up_cannot_replace_required_top_down_case(self):
        def replace(record):
            bottom = next(f for f in record["fixtures"] if f["id"] == "odd_bottom")
            top = next(f for f in record["fixtures"] if f["id"] == "odd_top")
            top.update({k: v for k, v in bottom.items() if k not in ("id", "path")})
            for worker in record["workers"]:
                if worker["fixture_id"] == "odd_top":
                    worker["input_sha256"] = "sha256:" + top["sha256"]
        self.reject(replace, "required_fixture_coverage_missing")


if __name__ == "__main__":
    unittest.main()
