"""Semantic W11 evidence mutations; the real Native control is mandatory."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import struct
import sys
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w2_independent_oracle import png_pixels
from w11_jpeg_verify import BOUNDS, verify

RECORD = Path(__file__).resolve().parents[5] / "docs/development/evidence-published/w11-latest/native-jpeg-matrix.json"


def _worker(record, case=0, leg=0):
    return record["cases"][case]["workers"][leg]


def _trace(worker):
    for sequence, row in enumerate(worker["stages"], 1):
        row["seq"] = sequence
    raw = "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in worker["stages"])
    worker.update(trace_utf8=raw, trace_bytes=len(raw.encode()),
                  trace_sha256="sha256:" + hashlib.sha256(raw.encode()).hexdigest(),
                  trace_records=len(worker["stages"]), trace_partial_bytes=0)


def _result(worker):
    raw = json.dumps(worker["probe_result"], separators=(",", ":"))
    worker.update(stdout_utf8=raw, stdout_bytes=len(raw.encode()))
    terminal = worker["stages"][-1]
    for key in ("outcome", "reason_code", "admission_status", "materialization"):
        terminal[key] = worker["probe_result"][key]
    _trace(worker)


def _memory(worker):
    window = worker["sample"]
    native = json.loads(window["observer"]["stdout_utf8"])
    native.update(samples=window["samples"], qpc_start=window["qpc_start"],
                  qpc_end=window["qpc_end"], qpc_frequency=window["qpc_frequency"])
    raw = json.dumps(native, separators=(",", ":"))
    window["observer"].update(stdout_utf8=raw, stdout_bytes=len(raw.encode()))
    window["totals"] = {k: sum(s[k] for s in window["samples"])
                        for k in ("working_set_bytes", "private_bytes")}


def _set_result(record, case, key, value):
    worker = _worker(record, case)
    worker["probe_result"][key] = value
    _result(worker)


def _fixture_identity(record):
    record["fixtures"][0]["sha256"] = "sha256:" + "0" * 64
    for row in record["cases"]:
        if row["fixture_id"] == "jpeg_gray_4k":
            for worker in row["workers"]:
                worker["input_sha256"] = record["fixtures"][0]["sha256"]


def _stage_order(record):
    worker = _worker(record)
    worker["stages"][7], worker["stages"][8] = worker["stages"][8], worker["stages"][7]
    _trace(worker)


def _counter(record, value):
    worker = _worker(record)
    worker["sample"]["samples"][0]["private_bytes"] = value
    _memory(worker)


def _sample_outside_hold(record):
    worker = _worker(record)
    window = worker["sample"]
    index = [r["stage"] for r in worker["stages"]].index(worker["hold_stage"])
    later = worker["stages"][index + 1]["qpc_ticks"] + 1
    window.update(qpc_start=later, qpc_end=later + 1)
    window["samples"][0].update(qpc_start=later, qpc_end=later + 1)
    _memory(worker)


def _pixel_tamper(record):
    worker = _worker(record)
    width, height, rgba = png_pixels(bytes.fromhex(worker["output_hex"]))
    pixels = bytearray(rgba)
    pixels[0] ^= 1

    def chunk(tag, body):
        return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xffffffff)

    scanlines = b"".join(b"\0" + pixels[y * width * 4:(y + 1) * width * 4] for y in range(height))
    output = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
              + chunk(b"IDAT", zlib.compress(scanlines)) + chunk(b"IEND", b""))
    worker.update(output_hex=output.hex(), output_bytes=len(output),
                  output_sha256="sha256:" + hashlib.sha256(output).hexdigest())
    # A dishonest producer updates its own expected/actual hashes consistently.
    changed = "sha256:" + hashlib.sha256(pixels).hexdigest()
    worker["oracle"].update(expected_rgba_sha256=changed, actual_rgba_sha256=changed,
                            first_mismatch=None, ok=True)


def _stage_clock(record):
    worker = _worker(record)
    worker["stages"][2]["qpc_ticks"] = worker["stages"][1]["qpc_ticks"] - 1
    _trace(worker)


def _missing_hold(record):
    worker = _worker(record)
    next(s for s in worker["stages"] if s["stage"] == worker["hold_stage"])["artificial_instrumentation_hold"] = False
    _trace(worker)


def _overlap(record):
    workers = record["cases"][0]["workers"]
    workers[1]["started_ns"] = workers[0]["exited_ns"] - 1


def _admission_charge(record):
    worker = _worker(record)
    worker["probe_result"]["decoded_bytes_required"] = 4
    next(s for s in worker["stages"] if s["stage"] == "admitted")["decoded_bytes_required"] = 4
    _result(worker)


MUTATIONS = (
    ("case_missing", lambda r: r["cases"].pop()),
    ("fixture_identity_coherent", _fixture_identity),
    ("fixture_geometry", lambda r: r["fixtures"][0].update(width=4095)),
    ("source_file_missing", lambda r: r["source_identity"]["files"].pop(next(iter(r["source_identity"]["files"])))),
    ("source_changed", lambda r: next(iter(r["source_identity"]["files"].values())).update(unchanged_after_run=False)),
    ("source_digest_invalid", lambda r: next(iter(r["source_identity"]["files"].values())).update(git_blob_sha256="not-a-digest")),
    ("source_commit_invalid", lambda r: r["source_identity"].update(source_commit="not-a-commit")),
    ("foreign_platform", lambda r: r["platform"].update(windows=False)),
    ("budget_override", lambda r: r["decode_limits"].update(max_dimension=16384)),
    ("roi_coordinate", lambda r: r["cases"][0].update(roi=[18, 23, 1, 1])),
    ("roi_reduced_full_frame_charge", lambda r: _set_result(r, 3, "full_frame_decoded_bytes", 4)),
    ("admission_reduced_charge_coherent", _admission_charge),
    ("refusal_materialized", lambda r: _set_result(r, 2, "materialization", "completed")),
    ("refusal_reason", lambda r: _set_result(r, 2, "reason_code", "roi_output_exceeded")),
    ("stage_order", _stage_order),
    ("stage_clock_reversed", _stage_clock),
    ("stage_hold_missing", _missing_hold),
    ("pixel_tamper_coherent_oracle", _pixel_tamper),
    ("output_digest", lambda r: _worker(r).update(output_sha256="sha256:" + "0" * 64)),
    ("refusal_published", lambda r: _worker(r, 3).update(output_present=True, published_success=True)),
    ("worker_not_reaped", lambda r: _worker(r).update(reaped=False)),
    ("unrequested_stop_contradiction", lambda r: _worker(r)["cancellation"].update(kill_issued=True)),
    ("cleanup_incomplete", lambda r: r["cleanup"].update(complete=False)),
    ("false_partial_decode", lambda r: _set_result(r, 0, "true_partial_decode", "verified")),
    ("false_memory_peak", lambda r: r["allocation_scope"].update(true_concurrent_allocation_peak="MEASURED")),
    ("counter_negative_coherent", lambda r: _counter(r, -1)),
    ("counter_over_cap_coherent", lambda r: _counter(r, BOUNDS["max_observed_private_bytes"] + 1)),
    ("counter_outside_hold_coherent", _sample_outside_hold),
    ("observer_incomplete", lambda r: _worker(r)["sample"]["observer"].update(capture_complete=False)),
    ("observer_identity", lambda r: _worker(r)["sample"]["observer"].update(script_sha256="sha256:" + "0" * 64)),
    ("atomic_counter_claim", lambda r: _worker(r)["sample"].update(atomic_snapshot=True)),
    ("workers_overlap", _overlap),
    ("worker_deadline", lambda r: _worker(r).update(exited_ns=_worker(r)["started_ns"] + 46_000_000_000)),
    ("run_deadline", lambda r: r.update(duration_ns=301_000_000_000)),
    ("work_cap", lambda r: r.update(work_bytes=BOUNDS["max_work_bytes"] + 1)),
    ("classification", lambda r: r["cases"][3].update(classification="ROI_OUTPUT_VERIFIED")),
)


class W11SemanticEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # An absent or invalid real control is an error, never a skipped success.
        if RECORD.stat().st_size > BOUNDS["max_report_bytes"]:
            raise ValueError("native_record_over_cap")
        cls.control = json.loads(RECORD.read_bytes())
        result = verify(cls.control)
        if not result["ok"]:
            raise ValueError("native_control_invalid:" + result.get("reason", "unknown"))

    def test_portable_replay_cannot_grant_source_closure(self):
        result = verify(self.control)
        self.assertEqual((result["ok"], result["closure_eligible"], result["source_identity_verified"]),
                         (True, False, False))
        self.assertEqual((result["cases"], result["workers"], result["roi_outputs_verified"],
                          result["refused_before_onload"], result["true_partial_decode"]),
                         (5, 9, 6, 3, "NOT_VERIFIED"))

    def test_semantic_mutations_fail_closed(self):
        for name, mutate in MUTATIONS:
            with self.subTest(name=name):
                record = copy.deepcopy(self.control)
                mutate(record)
                result = verify(record)
                self.assertEqual((result["ok"], result["closure_eligible"]), (False, False),
                                 msg=name)


if __name__ == "__main__":
    unittest.main()
