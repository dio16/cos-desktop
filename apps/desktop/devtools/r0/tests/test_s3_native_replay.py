"""Published Native journal arithmetic and refusal boundaries, not display proof."""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parents[3]
sys.path.insert(0, str(ROOT))
import s2b_contracts as s2b  # noqa: E402
import s3_native_replay as replay  # noqa: E402

PUBLISHED = REPO / "docs/development/evidence-published/r0c-2026-10-25"
ORACLE = REPO / "docs/development/evidence-published/r0a-s3-native-latest/independent-oracle.json"


def invoke(path: Path, *arguments: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, "-B", str(ROOT / "s3_native_replay.py"),
        "--input-dir", str(path), *arguments], capture_output=True, check=False, timeout=30)


@unittest.skipUnless(os.name == "nt", "published replay uses Windows pinned ownership reads")
class NativePublishedReplay(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.data, cls.index = replay.read_inputs(PUBLISHED)
        cls.report = replay.replay(PUBLISHED)
        cls.oracle = json.loads(ORACLE.read_bytes())["scenario_oracle"]

    def reject_scenario(self, path: str, mutate, code: str):
        data = dict(self.data)
        document = json.loads(data[path])
        mutate(document)
        data[path] = json.dumps(document).encode()
        with self.assertRaises(s2b.ObservationError) as caught:
            replay.validate_scenario(data, "keyboard")
        self.assertEqual(str(caught.exception), code)

    def test_six_real_journals_match_independently_retained_integer_oracle(self):
        self.assertEqual(len(self.report["inputs"]), 37)
        self.assertEqual([r["scenario"] for r in self.report["scenarios"]], list(replay.SCENARIOS))
        for scenario in self.report["scenarios"]:
            name = scenario["scenario"]
            expected = self.oracle[name]
            actual = scenario["aggregate"]
            with self.subTest(scenario=name):
                self.assertEqual(scenario["execution_id"], expected["execution_id"])
                self.assertEqual(actual["kind_counts"], expected["kind_counts"])
                self.assertEqual(actual["accounting"], expected["summary"])
                for kind in ("draw", "submission"):
                    for field in ("duration", "start_to_start", "end_to_start"):
                        metric, oracle = actual[kind][field], expected[kind][field]
                        self.assertEqual(metric["count"], oracle["count"])
                        for key in ("sum", "min", "max"):
                            value = oracle[key] if oracle["count"] else None
                            self.assertEqual(metric[key + "_ns"], value)
                        self.assertEqual(metric["population"], metric["count"])
                        self.assertEqual(metric["excluded"], 0)
                    inv, oracle = actual[kind]["invalidations"], expected["coalescing"][kind]
                    self.assertEqual(inv["count"], oracle["records"])
                    self.assertEqual(inv["sum"], oracle["invalidations_sum"])
                    self.assertEqual(len(inv["multiple_record_seqs"]), oracle["gt1"])
                    self.assertEqual(inv["max"], oracle["max_invalidations"] if oracle["records"] else None)
                dirty = actual["draw"]["dirty_to_start"]
                self.assertEqual(dirty["count"], expected["dirty_to_draw"]["count"])
                for key in ("sum", "min", "max"):
                    value = expected["dirty_to_draw"][key] if dirty["count"] else None
                    self.assertEqual(dirty[key + "_ns"], value)
                categories = {str(r["category"]): {"count": r["count"],
                    "caused_true": r["caused_invalidation"],
                    "caused_false": r["count"]-r["caused_invalidation"]} for r in actual["input_categories"]}
                self.assertEqual(categories, expected["input_categories"])

    def test_actual_record_references_and_explicit_pairing_are_recomputable(self):
        for scenario in self.report["scenarios"]:
            name = scenario["scenario"]
            source = json.loads(self.data[f"capture-run/{name}/s2b-capture.json"])
            records = {r["seq"]: r for r in source["records"]}
            for kind in ("draw", "submission"):
                value = scenario["aggregate"][kind]
                for seq, duration in value["duration"]["samples"]:
                    self.assertEqual(duration, records[seq]["end_ns"]-records[seq]["start_ns"])
                for before, after, gap in value["end_to_start"]["samples"]:
                    self.assertEqual(gap, records[after]["start_ns"]-records[before]["end_ns"])
                    self.assertEqual(records[after]["window"], records[before]["window"])
                    self.assertEqual(records[after]["lifetime"], records[before]["lifetime"])
            paired = scenario["aggregate"]["paired_draw_end_to_submission_start"]
            expected = [[r["seq"], r["start_ns"]-r["paired_draw_end_ns"]]
                        for r in source["records"] if r["kind"] == 3 and r["paired_draw_end_ns"] is not None]
            self.assertEqual(paired["samples"], expected)

    def test_retention_unknown_endpoints_and_scroll_zero_are_not_performance_zero(self):
        rows = {r["scenario"]: r for r in self.report["scenarios"]}
        for scenario in rows.values():
            value = scenario["aggregate"]
            accounting = value["accounting"]
            self.assertEqual(accounting["eligible"], sum(value["kind_counts"])+accounting["retention_dropped"])
            self.assertFalse(value["full_population"])
            self.assertEqual(accounting["unoffered_loss"], "UNKNOWN")
            for endpoint in s2b.MISSING:
                self.assertEqual(value["endpoints"][endpoint]["status"], "NOT_MEASURED")
            self.assertIn("executable-bytes", scenario["unavailable_verification_inputs"])
        scroll = rows["scroll"]["aggregate"]
        self.assertEqual(scroll["kind_counts"][2], 0)
        self.assertEqual(scroll["draw"]["duration"]["status"], "NOT_MEASURED")
        self.assertIsNone(scroll["draw"]["duration"]["sum_ns"])
        self.assertEqual(rows["combined"]["aggregate"]["submission"]["duration"]["count"], 0)
        nav = rows["navigation"]
        self.assertEqual(nav["aggregate"]["draw"]["end_to_start"]["max_ns"], 474_866_700)
        self.assertEqual(nav["readme_comparison"]["adjacent_gap_denominator"], 47)
        self.assertEqual(nav["readme_comparison"]["published_draw_record_denominator"], 48)
        self.assertFalse(nav["readme_comparison"]["gap_denominator_agrees"])
        self.assertEqual(nav["aggregate"]["visible_stall"], "UNVERIFIED")

    def test_public_cli_twice_has_identical_bytes_and_independently_verified_digest(self):
        first, second = invoke(PUBLISHED), invoke(PUBLISHED)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(first.stdout, second.stdout)
        document = json.loads(first.stdout)
        canonical = json.dumps(document["report"], sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
        self.assertEqual(document["report_sha256"], "sha256:"+hashlib.sha256(canonical).hexdigest())
        self.assertEqual(first.stdout, json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()+b"\n")

    def test_capture_identity_sequence_time_and_accounting_refusals(self):
        path = "capture-run/keyboard/s2b-capture.json"
        mutations = [
            (lambda d: d.update(execution_id="0"*32), "IDENTITY_MISMATCH"),
            (lambda d: d.update(pid=d["pid"]+1), "IDENTITY_MISMATCH"),
            (lambda d: d.update(executable_sha256="sha256:"+"0"*64), "IDENTITY_MISMATCH"),
            (lambda d: d["records"][0].update(seq=2), "SEQUENCE"),
            (lambda d: d["records"][0].update(start_ns=d["scope_end_ns"]+1), "TIMESTAMP"),
            (lambda d: d["summary"].update(retained=255), "COUNTS"),
            (lambda d: d["summary"].update(retention_dropped=0), "COUNTS"),
            (lambda d: d["clock"].update(domain="launcher-monotonic"), "CLOCK_DOMAIN"),
            (lambda d: d["records"].pop(), "COUNTS"),
        ]
        for mutate, code in mutations:
            with self.subTest(code=code):
                self.reject_scenario(path, mutate, code)

    def test_observation_binding_and_raw_metadata_refusals(self):
        path = "capture-run/keyboard/s2b-observation.json"
        for mutate, code in [
            (lambda d: d["identity"].update(source_commit="0"*40), "OBSERVATION_BINDING"),
            (lambda d: d["identity"].update(execution_id="0"*32), "OBSERVATION_BINDING"),
            (lambda d: d["capture"].update(sha256="sha256:"+"0"*64), "CAPTURE_DIGEST"),
            (lambda d: d["capture"].update(bytes=True), "INTEGER"),
            (lambda d: d["s2a"].update({"ledger.json":"sha256:"+"0"*64}), "LEDGER_DIGEST"),
        ]:
            with self.subTest(code=code):
                self.reject_scenario(path, mutate, code)
        self.reject_scenario("raw/keyboard-driver.json", lambda d: d.update(scenario="navigation"), "DRIVER_BINDING")
        self.reject_scenario("raw/keyboard-driver.json", lambda d: d["achieved_hz"].update(key="private/path"), "DRIVER_CADENCE")
        self.reject_scenario("raw/keyboard-driver.json", lambda d: d["achieved_hz"].update(key=10**500), "INTEGER")
        self.reject_scenario("raw/keyboard-driver.json", lambda d: d.update(requested_hz=10**500), "INTEGER")

    def test_corrupt_duplicate_and_nonfinite_json_are_refused_before_aggregation(self):
        for data, code in [(b'{"a":1,"a":2}', "DUPLICATE_KEY"),
                           (b'{"a":NaN}', "NON_FINITE_NUMBER"), (b'{', "MALFORMED_JSON")]:
            inputs = dict(self.data)
            inputs["capture-run/keyboard/s2b-capture.json"] = data
            with self.subTest(code=code), self.assertRaises(s2b.ObservationError) as caught:
                replay.validate_scenario(inputs, "keyboard")
            self.assertEqual(str(caught.exception), code)

    def copy_inputs(self, root: Path):
        for name, data in self.data.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (root / "SHA256SUMS").write_bytes(self.index)

    def test_byte_hash_missing_file_and_duplicate_index_are_closed_cli_failures(self):
        for mutation, expected in [("hash", "CHECKSUM_MISMATCH"),
                                   ("missing", "REPLAY_IO_OR_LAUNCHER_CONTRACT"),
                                   ("duplicate", "CHECKSUM_INDEX"),
                                   ("path", "CHECKSUM_INDEX")]:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.copy_inputs(root)
                if mutation == "hash":
                    (root / "README.md").write_bytes(self.data["README.md"]+b" ")
                elif mutation == "missing":
                    (root / "capture-run/scroll/s2b-capture.json").unlink()
                elif mutation == "duplicate":
                    (root / "SHA256SUMS").write_bytes(self.index+self.index.splitlines()[0]+b"\n")
                else:
                    (root / "SHA256SUMS").write_bytes(self.index.replace(b"README.md", b"../outside"))
                result = invoke(root)
                self.assertEqual(result.returncode, 1)
                self.assertEqual(json.loads(result.stdout), {"ok": False, "error": expected})
                self.assertEqual(result.stderr, b"")

    def test_output_bound_refuses_without_partial_output_or_new_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.copy_inputs(root)
            before = {p.relative_to(root).as_posix(): p.read_bytes() for p in root.rglob("*") if p.is_file()}
            result = invoke(root, "--max-output-bytes", "1024")
            self.assertEqual(result.returncode, 1)
            self.assertEqual(json.loads(result.stdout), {"ok": False, "error": "OUTPUT_LIMIT"})
            self.assertNotIn(b"report_sha256", result.stdout)
            after = {p.relative_to(root).as_posix(): p.read_bytes() for p in root.rglob("*") if p.is_file()}
            self.assertEqual(before, after)
            for cap in (0, 1023, replay.MAX_OUTPUT_BYTES+1):
                with self.assertRaises(s2b.ObservationError):
                    replay.encoded_report(self.report, cap)


class NativeGapBoundaries(unittest.TestCase):
    def record(self, seq: int, start: int, end: int, window: int = 1, lifetime: int = 1) -> dict:
        return {"seq":seq,"kind":2,"start_ns":start,"end_ns":end,"window":window,
                "lifetime":lifetime,"dirty_ns":None,"invalidations":1}

    def test_single_draw_cannot_be_formatted_as_an_observed_adjacent_gap(self):
        row = "| `keyboard` | r | a | m | 1/1 | 0/1 | input 0, dirty 0, draw 1, submit 0, surface-mutation 0, notify 0 | max 0.000 ms / 1 intervals | 0/1 (max 1) | n | `a` |"
        capture = {"execution_id":"a", "records":[{}], "summary":{
            "eligible":1, "retention_dropped":0, "input_unassociated":1}}
        value = {"kind_counts":[0,0,1,0,0,0,0,0,0,0],
                 "draw":{"end_to_start":{"count":0,"max_ns":None}}}
        with self.assertRaises(s2b.ObservationError) as caught:
            replay.readme_comparison(row, "keyboard", capture, value)
        self.assertEqual(str(caught.exception), "README_GAP")

    def test_window_lifetime_partition_and_loss_power_fences_exclude_pairs(self):
        records = [self.record(1,0,2), self.record(2,3,4,window=2,lifetime=2),
            {"seq":3,"kind":5}, self.record(4,5,7), self.record(5,8,9,window=2,lifetime=2),
            {"seq":6,"kind":7}, self.record(7,10,12), self.record(8,15,17)]
        value = replay.interval_metrics({"records":records}, 2)
        self.assertEqual(value["end_to_start"]["population"], 4)
        self.assertEqual(value["end_to_start"]["excluded"], 3)
        self.assertEqual(value["end_to_start"]["samples"], [[7,8,3]])
        self.assertEqual(value["start_to_start"]["samples"], [[7,8,5]])

    def test_overlap_is_signed_gap_not_clamped_or_counted_as_display_stall(self):
        value = replay.interval_metrics({"records":[self.record(1,10,30),self.record(2,20,40)]},2)
        self.assertEqual(value["end_to_start"]["samples"], [[1,2,-10]])
        self.assertEqual(value["start_to_start"]["samples"], [[1,2,10]])
        self.assertEqual(value["dirty_to_start"]["status"], "NOT_MEASURED")
        self.assertIsNone(value["dirty_to_start"]["sum_ns"])


if __name__ == "__main__":
    unittest.main()
