"""W5 metadata false-green negatives over the retained actual WIC record."""
from __future__ import annotations

import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w2_resource_verify import sha
from w5_metadata_verify import verify

ROOT = Path(__file__).resolve().parents[5]
RECORD = ROOT / "docs/development/evidence-published/img2-w5-latest/metadata-native-matrix.json"


class W5RecordTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = json.loads(RECORD.read_bytes())

    def worker(self, record, cid):
        return next(w for w in record["workers"] if w["case_id"] == cid)

    def set_result(self, worker, **fields):
        worker["probe_result"].update(fields)
        worker["stdout_utf8"] = json.dumps(worker["probe_result"]) + "\n"

    def reject(self, mutation, error=None, source=False):
        record = copy.deepcopy(self.native)
        mutation(record)
        result = verify(record, ROOT if source else None)
        self.assertFalse(result["ok"], result)
        if error:
            self.assertEqual(result["error"], error)

    def test_retained_record_is_valid(self):
        result = verify(self.native)
        self.assertTrue(result["ok"], result)

    def test_orientation_value_cannot_be_relabelled(self):
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_ori6"), orientation_raw=1),
                    "orientation_value_mismatch")
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_ori3"), orientation_raw=8),
                    "orientation_value_mismatch")

    def test_transform_cannot_be_claimed_without_effect(self):
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_ori6"),
                                              transform_applied="by_probe"),
                    "transform_application_mismatch")

    def test_double_rotation_is_not_the_expected_raster(self):
        def change(record):
            worker = self.worker(record, "tiff_ori6_apply")
            output = bytes.fromhex(worker["output_hex"])
            worker["output_hex"] = output.hex()
            worker["oracle"]["actual_rgba_sha256"] = "0" * 64
        self.reject(change, "orientation_pixel_mismatch")

    def test_source_dimensions(self):
        def change(record):
            next(f for f in record["fixtures"] if f["id"] == "tiff_ori6")["width"] += 1
        self.reject(change, "full_frame_requirement_invalid")

    def test_output_dimensions(self):
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_ori6_apply"), output_width=3),
                    "output_dimensions_mismatch")

    def test_icc_digest(self):
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_icc_valid"),
                                              icc_sha256="sha256:" + "0" * 64),
                    "icc_digest_mismatch")

    def test_icc_absent_cannot_become_present(self):
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_icc_absent"), icc_present=True),
                    "icc_presence_mismatch")

    def test_profile_readability_is_not_colour_conversion(self):
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_icc_valid"),
                                              colour_transform_verified="VERIFIED"),
                    "false_colour_claim")
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_icc_valid"),
                                              icc_application="VERIFIED"),
                    "false_colour_claim")

    def test_metadata_unreadable_cannot_be_verified(self):
        self.reject(lambda r: self.set_result(self.worker(r, "png_plain"),
                                              orientation_present=True, metadata_available=True),
                    "orientation_presence_mismatch")

    def test_metadata_cap_cannot_be_admitted(self):
        def change(record):
            worker = self.worker(record, "tiff_icc_oversized")
            worker.update(outcome="decoded", exit_code=0)
            self.set_result(worker, outcome="decoded", admission_status="admitted",
                            materialization="completed")
        self.reject(change, "probe_exit_result_mismatch")

    def test_materialization_after_metadata_refusal(self):
        def change(record):
            worker = self.worker(record, "tiff_icc_oversized")
            worker["stages"].insert(-1, {"stage": "onload_entered", "seq": 0, "pid": worker["pid"],
                                         "process_start_utc_ticks": worker["start_utc_ticks"]})
            for seq, stage in enumerate(worker["stages"], 1):
                stage["seq"] = seq
            raw = "".join(json.dumps(s, separators=(",", ":")) + "\n" for s in worker["stages"])
            worker.update(trace_utf8=raw, trace_sha256=sha(raw.encode()),
                          trace_bytes=len(raw.encode()), trace_records=len(worker["stages"]))
        self.reject(change)

    def test_missing_terminal_stage(self):
        def change(record):
            worker = self.worker(record, "tiff_ori6")
            worker["stages"].pop()
            raw = "".join(json.dumps(s, separators=(",", ":")) + "\n" for s in worker["stages"])
            worker.update(trace_utf8=raw, trace_sha256=sha(raw.encode()),
                          trace_bytes=len(raw.encode()), trace_records=len(worker["stages"]))
        self.reject(change, "native_incomplete")

    def test_source_identity_mismatch(self):
        def change(record):
            name = next(iter(record["source_identity"]["files"]))
            record["source_identity"]["files"][name]["lf_sha256"] = "sha256:" + "0" * 64
        self.reject(change, "source_digest_mismatch", source=True)

    def test_unsupported_cannot_be_decoded(self):
        def change(record):
            worker = self.worker(record, "unsupported_source")
            worker.update(outcome="decoded", exit_code=0)
            self.set_result(worker, outcome="decoded", admission_status="admitted",
                            materialization="completed")
        self.reject(change, "probe_exit_result_mismatch")

    def test_stop_time_and_outcome_contract(self):
        def req(record):
            self.worker(record, "cancel_metadata")["cancellation"]["requested_ns"] = None
        self.reject(req, "success_after_stop")
        def obs(record):
            self.worker(record, "timeout_metadata")["cancellation"]["observed_ns"] = None
        self.reject(obs, "success_after_stop")
        def swap(record):
            self.worker(record, "cancel_metadata")["outcome"] = "timeout"
        self.reject(swap, "success_after_stop")
        def alive(record):
            self.worker(record, "cancel_metadata")["alive_after_wait"] = True
        self.reject(alive, "success_after_stop")
        def published(record):
            self.worker(record, "cancel_metadata")["published_success"] = True
        self.reject(published, "success_after_stop")

    def test_metadata_availability_and_presence_must_be_consistent(self):
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_ori6"), metadata_available=False),
                    "metadata_availability_mismatch")
        self.reject(lambda r: self.set_result(self.worker(r, "bmp24_plain"), metadata_available=True),
                    "metadata_availability_mismatch")
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_icc_valid"), colour_contexts=0),
                    "icc_digest_mismatch")
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_icc_valid"), metadata_available=False),
                    "metadata_availability_mismatch")
        self.reject(lambda r: self.set_result(self.worker(r, "tiff_icc_oversized"), icc_present=False),
                    "metadata_presence_mismatch")

    def test_full_frame_requirement_cannot_be_zeroed(self):
        def change(record):
            worker = self.worker(record, "tiff_ori6")
            self.set_result(worker, decoded_bytes_required=0)
            admitted = next(s for s in worker["stages"] if s["stage"] == "admitted")
            admitted["decoded_bytes_required"] = 0
            raw = "".join(json.dumps(st, separators=(",", ":")) + "\n" for st in worker["stages"])
            worker.update(trace_utf8=raw, trace_sha256=sha(raw.encode()),
                          trace_bytes=len(raw.encode()), trace_records=len(worker["stages"]))
        self.reject(change, "full_frame_requirement_invalid")

    def test_conflicting_orientation_is_not_reported_as_clean(self):
        def change(record):
            self.set_result(self.worker(record, "tiff_ori_conflict"), orientation_raw=8)
        self.reject(change, "orientation_value_mismatch")

    def test_cancellation_cannot_be_decoded(self):
        def change(record):
            worker = self.worker(record, "cancel_metadata")
            worker.update(outcome="decoded", published_success=True, output_present=True)
        self.reject(change, "success_after_stop")


if __name__ == "__main__":
    unittest.main()
