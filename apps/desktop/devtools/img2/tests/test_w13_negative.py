"""Semantic W13 mutation tests; the retained real control is mandatory.

The source-bound mutation loop only counts a rejection after the unmodified
control has itself passed source-bound verification (W12 A-R1-01 premise).
"""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w13_full_verify import BOUNDS, compare_repeats, verify

EVIDENCE = Path(__file__).resolve().parents[5] / "docs/development/evidence-published/w13-latest"


def worker(record, index):
    return record["cases"][index]["workers"][0]


def retrace(item):
    for sequence, row in enumerate(item["stages"], 1):
        row["seq"] = sequence
    raw = "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in item["stages"])
    item.update(trace_utf8=raw, trace_bytes=len(raw.encode()),
                trace_sha256="sha256:" + hashlib.sha256(raw.encode()).hexdigest(),
                trace_records=len(item["stages"]), trace_partial_bytes=0)


def refresh(item):
    raw = json.dumps(item["probe_result"], separators=(",", ":"))
    item.update(stdout_utf8=raw, stdout_bytes=len(raw.encode()))
    for key in ("outcome", "reason_code", "admission_status", "materialization"):
        item["stages"][-1][key] = item["probe_result"][key]
    retrace(item)


def refresh_memory(item):
    window = item["sample"]
    native = json.loads(window["observer"]["stdout_utf8"])
    native.update(samples=window["samples"], qpc_start=window["qpc_start"],
                  qpc_end=window["qpc_end"], qpc_frequency=window["qpc_frequency"])
    raw = json.dumps(native, separators=(",", ":"))
    window["observer"].update(stdout_utf8=raw, stdout_bytes=len(raw.encode()))
    window["totals"] = {k: sum(s[k] for s in window["samples"])
                        for k in ("working_set_bytes", "private_bytes")}


def set_result(record, index, key, value):
    item = worker(record, index)
    item["probe_result"][key] = value
    refresh(item)


def rewrite_output(record, index, mutate):
    item = worker(record, index)
    data = bytearray(bytes.fromhex(item["output_hex"]))
    mutate(data)
    payload = bytes(data)
    item.update(output_hex=payload.hex(), output_bytes=len(payload),
                output_sha256="sha256:" + hashlib.sha256(payload).hexdigest())


def pixel_mutation(record):
    """Change one IDAT byte in place; the PNG CRC must expose it."""
    def mutate(data):
        start = data.find(b"IDAT") + 6
        data[start] ^= 0x01
    rewrite_output(record, 0, mutate)


def coherent_pixel_mutation(record):
    """Rewrite the retained oracle alongside a truncated output so both agree."""
    item = worker(record, 1)
    data = bytes.fromhex(item["output_hex"])
    item.update(output_hex=data[:-12].hex(), output_bytes=len(data) - 12,
                output_sha256="sha256:" + hashlib.sha256(data[:-12]).hexdigest(),
                oracle=dict(item["oracle"], ok=True))


def trailing_bytes(record):
    rewrite_output(record, 2, lambda data: data.extend(b"\x00\x00"))


def sample_outside_hold(record):
    item = worker(record, 0)
    index = [row["stage"] for row in item["stages"]].index("after_onload")
    later = item["stages"][index + 1]["qpc_ticks"] + 1
    window = item["sample"]
    window.update(qpc_start=later, qpc_end=later + 1)
    window["samples"][0].update(qpc_start=later, qpc_end=later + 1)
    refresh_memory(item)


def counter_over_cap(record):
    item = worker(record, 0)
    item["sample"]["samples"][0]["private_bytes"] = BOUNDS["max_observed_private_bytes"] + 1
    refresh_memory(item)


def overlap(record):
    worker(record, 1)["started_ns"] = worker(record, 0)["exited_ns"] - 1


MUTATIONS = (
    ("case_missing", lambda r: r["cases"].pop()),
    ("case_duplicated", lambda r: r["cases"].append(copy.deepcopy(r["cases"][-1]))),
    ("fixture_digest", lambda r: r["fixtures"][0].update(sha256="sha256:" + "0" * 64)),
    ("fixture_geometry", lambda r: r["fixtures"][1].update(height=4095)),
    ("fixture_codec", lambda r: r["fixtures"][1].update(codec="jpeg")),
    ("source_file_missing", lambda r: r["source_identity"]["files"].pop(
        next(iter(r["source_identity"]["files"])))),
    ("source_changed", lambda r: next(iter(r["source_identity"]["files"].values())).update(
        unchanged_after_run=False)),
    ("source_commit", lambda r: r["source_identity"].update(source_commit="0" * 40)),
    ("foreign_platform", lambda r: r["platform"].update(windows=False)),
    ("budget_override", lambda r: r["decode_limits"].update(max_pixels=1 << 40)),
    ("unexpected_roi", lambda r: set_result(r, 0, "roi_requested", True)),
    ("admission_charge", lambda r: set_result(r, 0, "decoded_bytes_required", 4096)),
    ("decoder_identity", lambda r: set_result(r, 1, "decoder", "JpegBitmapDecoder")),
    ("partial_claim", lambda r: set_result(r, 0, "true_partial_decode", "verified")),
    ("refusal_materialized", lambda r: set_result(r, 3, "materialization", "completed")),
    ("refusal_reason", lambda r: set_result(r, 2, "reason_code", "max_pixels_exceeded")),
    ("refusal_published", lambda r: worker(r, 3).update(
        output_present=True, published_success=True, output_bytes=8,
        output_sha256="sha256:" + "0" * 64, output_hex="00" * 8)),
    ("stage_order", lambda r: (worker(r, 0)["stages"].__setitem__(
        slice(7, 9), worker(r, 0)["stages"][8:6:-1]), retrace(worker(r, 0)))),
    ("stage_hold_missing", lambda r: (next(
        s for s in worker(r, 0)["stages"] if s["stage"] == "after_onload").update(
            artificial_instrumentation_hold=False), retrace(worker(r, 0)))),
    ("output_crc_mutation", pixel_mutation),
    ("output_truncated_coherent", coherent_pixel_mutation),
    ("output_trailing_bytes", trailing_bytes),
    ("output_digest", lambda r: worker(r, 1).update(output_sha256="sha256:" + "0" * 64)),
    ("worker_not_reaped", lambda r: worker(r, 0).update(reaped=False)),
    ("unrequested_stop", lambda r: worker(r, 0)["cancellation"].update(kill_issued=True)),
    ("cleanup_incomplete", lambda r: r["cleanup"].update(complete=False)),
    ("false_memory_class", lambda r: r["allocation_scope"].update(
        same_process_allocation_delta="MEASURED")),
    ("false_scratch_claim", lambda r: r["allocation_scope"].update(
        decoder_internal_scratch_peak="MEASURED")),
    ("codec_advantage_claim", lambda r: r["allocation_scope"].update(
        codec_memory_advantage="JPEG_LOWER")),
    ("counter_over_cap", counter_over_cap),
    ("memory_outside_hold", sample_outside_hold),
    ("observer_incomplete", lambda r: worker(r, 0)["sample"]["observer"].update(
        capture_complete=False)),
    ("sample_missing", lambda r: worker(r, 2).pop("sample")),
    ("workers_overlap", overlap),
    ("worker_deadline", lambda r: worker(r, 0).update(
        exited_ns=worker(r, 0)["started_ns"] + 46_000_000_000)),
    ("run_deadline", lambda r: r.update(duration_ns=301_000_000_000)),
    ("work_cap", lambda r: r.update(work_bytes=BOUNDS["max_work_bytes"] + 1)),
    ("report_cap", lambda r: r.update(work_bytes=BOUNDS["max_work_bytes"])),
    ("classification", lambda r: r["cases"][2].update(classification="FULL_FRAME_OUTPUT_VERIFIED")),
)


class W13SemanticTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        first = EVIDENCE / "native-full-matrix.json"
        second = EVIDENCE / "native-repeat-matrix.json"
        for path in (first, second):
            if not path.is_file():
                raise ValueError("native_control_missing:" + path.name)
            if path.stat().st_size > BOUNDS["max_report_bytes"]:
                raise ValueError("native_control_over_cap:" + path.name)
        cls.first = json.loads(first.read_bytes())
        cls.second = json.loads(second.read_bytes())
        cls.root = Path(__file__).resolve().parents[5]
        for name, record in (("first", cls.first), ("repeat", cls.second)):
            result = verify(record)
            if not result["ok"]:
                raise ValueError("native_control_invalid:" + name + ":" + result.get("reason", "?"))
        source = verify(cls.first, cls.root)
        if not (source["ok"] and source["closure_eligible"] and source["source_identity_verified"]):
            raise ValueError("source_bound_control_invalid:" + str(source.get("reason", source)))

    def test_portable_replay_cannot_grant_source_closure(self):
        result = verify(self.first)
        self.assertEqual((result["ok"], result["closure_eligible"],
                          result["source_identity_verified"]), (True, False, False))
        self.assertEqual((result["cases"], result["workers"], result["full_frame_outputs_verified"],
                          result["refused_before_onload"], result["output_pixels_verified"]),
                         (4, 4, 2, 2, 33554432))
        self.assertTrue(result["codec_rgba_identical"])
        self.assertEqual(result["true_partial_decode"], "NOT_VERIFIED")

    def test_source_bound_control_is_verified_before_mutations(self):
        source = verify(self.first, self.root)
        self.assertEqual((source["ok"], source["closure_eligible"],
                          source["source_identity_verified"]), (True, True, True))
        self.assertEqual(verify(self.second, self.root)["closure_eligible"], True)

    def test_repeat_comparison_keeps_counters_unforced(self):
        report = compare_repeats(self.first, self.second)
        self.assertTrue(report["source_identity_equal"] and report["fixtures_equal"])
        self.assertTrue(report["codec_pairs_identical"])
        self.assertEqual(report["cases"], 4)
        self.assertTrue(report["counters_not_forced_equal"])

    def test_semantic_mutations_fail_closed(self):
        git_only = {"source_commit"}
        for name, mutate in MUTATIONS:
            with self.subTest(mode="portable", name=name):
                record = copy.deepcopy(self.first)
                mutate(record)
                result = verify(record)
                if name in git_only:
                    self.assertEqual((result["ok"], result["closure_eligible"]), (True, False),
                                     msg=name)
                else:
                    self.assertEqual((result["ok"], result["closure_eligible"]), (False, False),
                                     msg=name)
            with self.subTest(mode="source-bound", name=name):
                record = copy.deepcopy(self.first)
                mutate(record)
                result = verify(record, self.root)
                self.assertEqual((result["ok"], result["closure_eligible"]), (False, False),
                                 msg=name)

    def test_repeat_mismatch_is_rejected(self):
        broken = copy.deepcopy(self.second)
        worker(broken, 1)["probe_result"]["pixel_format"] = "Bgr24"
        refresh(worker(broken, 1))
        with self.assertRaises(ValueError):
            compare_repeats(self.first, broken)


if __name__ == "__main__":
    unittest.main()
