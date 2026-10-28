"""Semantic W12 evidence and matched-raster mutations; the real control is mandatory."""
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
from w12_jpeg_verify import BOUNDS, compare_repeats, verify

EVIDENCE = Path(__file__).resolve().parents[5] / "docs/development/evidence-published/w12-latest"


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


def png_chunks(data):
    offset, chunks = 8, []
    while offset < len(data):
        length = int.from_bytes(data[offset:offset + 4], "big")
        chunk = data[offset:offset + 12 + length]
        chunks.append(chunk)
        offset += 12 + length
    return chunks


def rebuild(width, height, colour, rows):
    def chunk(tag, body):
        return (struct.pack(">I", len(body)) + tag + body
                + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, colour, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b""))


def replace_output(record, index, data):
    item = worker(record, index)
    item.update(output_hex=data.hex(), output_bytes=len(data),
                output_sha256="sha256:" + hashlib.sha256(data).hexdigest())


def swap_png_with_jpeg_shape(record):
    """Give the PNG case a JPEG-shaped record while keeping the producer coherent."""
    item = worker(record, 1)
    item["probe_result"].update(decoder="JpegBitmapDecoder")
    refresh(item)


def tamper_png_pixels(record):
    item = worker(record, 1)
    width, height, rgba = png_pixels(bytes.fromhex(item["output_hex"]))
    pixels = bytearray(rgba)
    pixels[0] ^= 0x01
    rows = b"".join(b"\x00" + bytes(pixels[y * width * 4:(y + 1) * width * 4])
                    for y in range(height))
    data = rebuild(width, height, 6, rows)
    replace_output(record, 1, data)
    changed = "sha256:" + hashlib.sha256(pixels).hexdigest()
    item["oracle"].update(ok=True, first_mismatch=None, expected_sha256=changed, actual_sha256=changed)


def break_pair(record):
    """Make the JPEG/PNG pair genuinely different while both self-reports stay coherent."""
    tamper_png_pixels(record)


def stale_output_after_refusal(record):
    item = worker(record, 5)
    item.update(output_present=True, published_success=True,
                output_bytes=16, output_sha256="sha256:" + "0" * 64, output_hex="00" * 16)


def memory_outside_hold(record):
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


def concurrent_lifetimes(record):
    first = worker(record, 0)
    second = worker(record, 1)
    second["started_ns"] = first["exited_ns"] - 1


def duplicate_lifetime(record):
    second = worker(record, 1)
    first = worker(record, 0)
    second["pid"] = first["pid"]
    second["start_utc_ticks"] = first["start_utc_ticks"]
    second["sample"]["samples"][0].update(pid=first["pid"], start_utc_ticks=first["start_utc_ticks"])
    second["sample"]["request_identities"][0].update(pid=first["pid"],
                                                    start_utc_ticks=first["start_utc_ticks"])
    refresh_memory(second)


MUTATIONS = (
    ("case_missing", lambda r: r["cases"].pop()),
    ("case_duplicated", lambda r: r["cases"].append(copy.deepcopy(r["cases"][-1]))),
    ("fixture_digest", lambda r: r["fixtures"][1].update(sha256="sha256:" + "0" * 64)),
    ("fixture_dimension", lambda r: r["fixtures"][0].update(width=4095)),
    ("fixture_codec", lambda r: r["fixtures"][1].update(codec="jpeg")),
    ("source_missing_file", lambda r: r["source_identity"]["files"].pop(
        next(iter(r["source_identity"]["files"])))),
    ("source_changed", lambda r: next(iter(r["source_identity"]["files"].values())).update(
        unchanged_after_run=False)),
    ("source_commit", lambda r: r["source_identity"].update(source_commit="0" * 40)),
    ("foreign_platform", lambda r: r["platform"].update(windows=False)),
    ("budget_override", lambda r: r["decode_limits"].update(max_dimension=8192)),
    ("roi_coordinate", lambda r: r["cases"][0].update(roi=[18, 23, 1, 1])),
    ("roi_reduced_full_frame", lambda r: set_result(r, 0, "full_frame_decoded_bytes", 4)),
    ("decoder_identity", swap_png_with_jpeg_shape),
    ("admission_reduced_charge", lambda r: set_result(r, 0, "decoded_bytes_required", 4)),
    ("refusal_materialized", lambda r: set_result(r, 4, "materialization", "completed")),
    ("refusal_reason", lambda r: set_result(r, 5, "reason_code", "max_pixels_exceeded")),
    ("refusal_published", stale_output_after_refusal),
    ("stage_order", lambda r: (worker(r, 0)["stages"].__setitem__(slice(7, 9),
        worker(r, 0)["stages"][8:6:-1]), retrace(worker(r, 0)))),
    ("stage_hold_missing", lambda r: (next(s for s in worker(r, 0)["stages"]
        if s["stage"] == "after_onload").update(artificial_instrumentation_hold=False),
        retrace(worker(r, 0)))),
    ("output_digest", lambda r: worker(r, 2).update(output_sha256="sha256:" + "0" * 64)),
    ("png_pixels_tampered_coherent", tamper_png_pixels),
    ("matched_pair_inequality", break_pair),
    ("worker_not_reaped", lambda r: worker(r, 0).update(reaped=False)),
    ("unrequested_stop", lambda r: worker(r, 0)["cancellation"].update(kill_issued=True)),
    ("cleanup_incomplete", lambda r: r["cleanup"].update(complete=False)),
    ("false_partial_decode", lambda r: set_result(r, 0, "true_partial_decode", "verified")),
    ("false_memory_class", lambda r: r["allocation_scope"].update(
        same_process_allocation_delta="MEASURED")),
    ("codec_advantage_claim", lambda r: r["allocation_scope"].update(
        codec_memory_advantage="JPEG_LOWER")),
    ("counter_over_cap", counter_over_cap),
    ("memory_outside_hold", memory_outside_hold),
    ("observer_incomplete", lambda r: worker(r, 0)["sample"]["observer"].update(
        capture_complete=False)),
    ("sample_atomic_claim", lambda r: worker(r, 1)["sample"].update(atomic_snapshot=True)),
    ("sample_missing", lambda r: worker(r, 2).pop("sample")),
    ("workers_overlap", concurrent_lifetimes),
    ("worker_lifetime_reused", duplicate_lifetime),
    ("worker_deadline", lambda r: worker(r, 3).update(
        exited_ns=worker(r, 3)["started_ns"] + 46_000_000_000)),
    ("run_deadline", lambda r: r.update(duration_ns=301_000_000_000)),
    ("work_cap", lambda r: r.update(work_bytes=BOUNDS["max_work_bytes"] + 1)),
    ("classification", lambda r: r["cases"][4].update(classification="ROI_OUTPUT_VERIFIED")),
)


class W12SemanticTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        first = EVIDENCE / "native-matched-matrix.json"
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
        # The unmodified control must pass the *source-bound* verification before any
        # source-bound mutation can be counted. Without this premise a repository
        # without the pinned Git objects would fail every mutant for an unrelated
        # reason and the suite would report 39 successes it never earned.
        source = verify(cls.first, cls.root)
        if not (source["ok"] and source["closure_eligible"] and source["source_identity_verified"]):
            raise ValueError("source_bound_control_invalid:" + str(source.get("reason", source)))

    def test_portable_replay_cannot_grant_source_closure(self):
        result = verify(self.first)
        self.assertEqual((result["ok"], result["closure_eligible"],
                          result["source_identity_verified"]), (True, False, False))
        self.assertEqual((result["cases"], result["workers"], result["roi_outputs_verified"],
                          result["matched_pairs_byte_identical"], result["refused_before_onload"]),
                         (6, 6, 4, 2, 2))
        self.assertEqual(result["memory_comparison"], "OBSERVED_WHOLE_PROCESS_DIFFERENTIAL")
        self.assertEqual(result["codec_memory_advantage"], "NOT_CLAIMED")

    def test_source_bound_control_is_verified_before_mutations(self):
        source = verify(self.first, self.root)
        self.assertEqual((source["ok"], source["closure_eligible"],
                          source["source_identity_verified"]), (True, True, True))
        repeat = verify(self.second, self.root)
        self.assertEqual((repeat["ok"], repeat["closure_eligible"]), (True, True))

    def test_repeat_comparison_keeps_counters_unforced(self):
        report = compare_repeats(self.first, self.second)
        self.assertTrue(report["source_identity_equal"] and report["fixtures_equal"])
        self.assertEqual(report["cases"], 6)
        self.assertTrue(report["counters_not_forced_equal"])

    def test_semantic_mutations_fail_closed(self):
        # Portable verification must reject every mutant except the one that only a
        # real Git lookup can contradict; that single case is asserted explicitly so
        # the distinction is proven rather than hidden inside a shared failure.
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
