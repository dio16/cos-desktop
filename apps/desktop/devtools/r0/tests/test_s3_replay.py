"""S3 operational replay: S2 result import, provenance and deterministic replay.

The ten replay cases from the task card, plus the real retained bundle when it is
present. A bundle that is not on disk is reported as NOT_RUN with its reason instead of
being simulated.
"""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import contracts  # noqa: E402
import s3_import as imp  # noqa: E402
import s3_ledger as s3  # noqa: E402

REPO = ROOT.parents[3]
BUNDLE = REPO / "docs/development/evidence/r0a-s2b/native-bundles.zip"

MANIFEST = {
    "scenario_id": "shell-startup",
    "scenario_version": "v2",
    "benchmark_revision": "r0a-s2b",
    "definition_hash": "sha256:" + "3" * 64,
}
SOURCE = {"git_sha": "a" * 40, "worktree_digest": "sha256:" + "4" * 64}
PROGRAM = "sha256:" + "5" * 64


def run_document(*, status: str = "OBSERVED", processed: int | None = 12,
                 present: int | None = 11, run_id: str = "run-1",
                 variant: str = "instrumented", manifest: dict | None = None) -> dict:
    """A minimal but fully valid canonical S2 result."""
    manifest = manifest or MANIFEST
    frames = {name: None for name in (
        "requested", "processed", "submitted", "present", "displayed", "missed",
        "dropped", "stalled", "eligibility", "idle")}
    frames["processed"] = processed
    frames["present"] = present
    frames["rule"] = "counted by the fixture"
    # A missing counter needs its reason even when the run itself is OBSERVED.
    frames["reason"] = ("some frame counters are not measured by this fixture"
                        if any(value is None for value in frames.values())
                        else "")
    def group(numbers: tuple[str, ...]) -> dict:
        measured = {name: None for name in numbers}
        measured.update({"status": "NOT_MEASURED", "reason": "no probe"})
        return measured

    return {
        "result_version": contracts.RESULT_VERSION,
        "manifest": dict(manifest),
        "runs": [{
            "scenario_id": manifest["scenario_id"],
            "run_id": run_id,
            "pair_id": "pair-1",
            "variant": variant,
            "status": status,
            "reason": "" if status == "OBSERVED" else "fixture status",
            "endpoints": ([{
                "id": "process-launch",
                "status": "OBSERVED",
                "reason": "",
                "provider": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
                "version": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
                "unit": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
                "clock": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
                "correlation": {"status": "NOT_IMPLEMENTED", "reason": "no visual probe"},
            }] if status == "OBSERVED" else []),
            # An OBSERVED run must carry at least one observed metric sample.
            "metrics": [{
                "id": "startup-launch",
                "unit": "ms",
                "method": "launcher-monotonic",
                "sample_unit": "launch",
                "counts": {"eligible": 1, "observed": 1 if status == "OBSERVED" else 0,
                           "failed": 0, "censored": 0, "excluded": 0},
                "percentiles": {"min": None, "max": None, "p50": None, "p95": None,
                                "p99": None},
                "estimator": "none",
                "validity": "fixture metric; no estimator",
                "spread": "not-estimated",
            }],
            "resources": {
                "process": group(("private_bytes", "working_set_bytes")),
                "media": group(("decoded_bytes", "scratch_bytes", "staging_bytes",
                                "cache_bytes")),
                "gpu": group(("dedicated_bytes", "shared_bytes", "committed_bytes")),
                "devices": group(("count", "generations")),
                "sampling": "not_measured",
            },
            "frames": frames,
            "instrumentation": {"tool": "fixture", "revision": "1", "config": "none",
                                "probes": {"supported": [], "missing": []},
                                "overhead": "none", "loss": "none", "floor": "none"},
            "failures": [],
            "outliers": [],
            "evidence": ["ledger.json"] if status == "OBSERVED" else [],
        }],
        "evidence": ["ledger.json"],
    }


def import_document(document: dict, ledger_path: Path, **kwargs) -> dict:
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False,
                                     encoding="utf-8") as handle:
        json.dump(document, handle)
        path = handle.name
    return imp.import_result(path, str(ledger_path), **kwargs)


class S3Replay(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.ledger = Path(self.directory.name) / "s3-ledger.jsonl"

    # ---------------------------------------------------------------- case 1
    def test_canonical_result_round_trip(self) -> None:
        imported = import_document(run_document(), self.ledger,
                                   source=SOURCE, program_digest=PROGRAM)
        self.assertEqual(imported["runs"][0]["entries"], 1)
        entries = s3.load_ledger(str(self.ledger))
        self.assertEqual(len(entries), 1)
        self.assertEqual(entries[0]["outcome"], "OBSERVED")
        self.assertEqual(entries[0]["counters"],
                         {"frames_observed": 12, "dirty_frames": None,
                          "present_calls": 11, "scope_ns": None})
        summary = s3.summarise(entries)
        self.assertEqual(summary["contributing"], ["run-1.instrumented"])
        self.assertEqual(summary["counters"]["frames_observed"],
                         {"status": "OBSERVED", "value": 12, "runs": 1})
        self.assertEqual(summary["counters"]["dirty_frames"],
                         {"status": "NOT_MEASURED", "reason": "NO_RUN_MEASURED_THIS_COUNTER"})
        self.assertEqual(summary["ledger_digest"], imported["ledger_digest"])

    # ---------------------------------------------------------------- case 2
    def test_reimport_is_refused_as_a_duplicate(self) -> None:
        import_document(run_document(), self.ledger, source=SOURCE, program_digest=PROGRAM)
        with self.assertRaises(s3.LedgerError) as error:
            import_document(run_document(), self.ledger, source=SOURCE,
                            program_digest=PROGRAM)
        self.assertEqual(error.exception.code, "DUPLICATE_RUN")
        self.assertEqual(len(s3.load_ledger(str(self.ledger))), 1)

    # ---------------------------------------------------------------- case 3
    def test_missing_source_identity_is_excluded(self) -> None:
        imported = import_document(run_document(), self.ledger, source=None,
                                   program_digest=PROGRAM)
        summary = s3.summarise(s3.load_ledger(str(self.ledger)))
        self.assertEqual(summary["contributing"], [])
        self.assertEqual(summary["excluded"],
                         [{"run_id": "run-1.instrumented", "reason": "SOURCE_NOT_BOUND"}])
        self.assertEqual(summary["runs"],
                         {"status": "NOT_MEASURED", "reason": "NO_CONTRIBUTING_RUNS"})
        self.assertEqual(imported["ledger_digest"], summary["ledger_digest"])

    # ---------------------------------------------------------------- case 4
    def test_missing_candidate_digest_is_excluded(self) -> None:
        import_document(run_document(), self.ledger, source=SOURCE, program_digest=None)
        summary = s3.summarise(s3.load_ledger(str(self.ledger)))
        self.assertEqual(summary["excluded"],
                         [{"run_id": "run-1.instrumented", "reason": "CANDIDATE_NOT_BOUND"}])

    # ---------------------------------------------------------------- case 5
    def test_not_measured_contributes_nothing(self) -> None:
        import_document(run_document(status="NOT_MEASURED", processed=None, present=None),
                        self.ledger, source=SOURCE, program_digest=PROGRAM)
        entries = s3.load_ledger(str(self.ledger))
        self.assertEqual(entries[0]["outcome"], "NOT_MEASURED")
        self.assertEqual(entries[0]["counters"]["frames_observed"], None)
        summary = s3.summarise(entries)
        self.assertEqual(summary["contributing"], [])
        self.assertEqual(summary["excluded"][0]["reason"], "OUTCOME_NOT_MEASURED")

    # ---------------------------------------------------------------- case 6
    def test_failed_and_cancelled_contribute_nothing(self) -> None:
        import_document(run_document(status="FAILED"), self.ledger, source=SOURCE,
                        program_digest=PROGRAM)
        summary = s3.summarise(s3.load_ledger(str(self.ledger)))
        self.assertEqual(summary["contributing"], [])
        self.assertEqual(summary["excluded"][0]["reason"], "OUTCOME_FAILED")
        # CANCELLED has no S2 producer; it is a ledger-level outcome and must be excluded
        # the same way (the aggregation is shared).
        body = {
            "record": s3.RECORD_RUN, "entry_version": s3.ENTRY_VERSION,
            "run_id": "run-cancelled", "source": SOURCE,
            "candidate": {"id": "s2-candidate", "program_digest": PROGRAM},
            "host": {"os": "windows", "arch": "x86_64"},
            "interpreter": {"name": "cpython", "version": "3.13.14"},
            "start_utc": "1970-01-01T00:00:00Z", "end_utc": "1970-01-01T00:00:00Z",
            "outcome": "CANCELLED",
            "counters": {name: None for name in s3.COUNTERS},
        }
        s3.append_entry(str(self.ledger), body)
        summary = s3.summarise(s3.load_ledger(str(self.ledger)))
        self.assertEqual([row["reason"] for row in summary["excluded"]],
                         ["OUTCOME_FAILED", "OUTCOME_CANCELLED"])

    # ---------------------------------------------------------------- case 7
    def test_chain_refuses_deletion_and_reorder(self) -> None:
        for index in range(3):
            import_document(run_document(run_id=f"run-{index}"), self.ledger,
                            source=SOURCE, program_digest=PROGRAM)
        lines = self.ledger.read_text(encoding="utf-8").split("\n")
        self.ledger.write_text("\n".join([lines[0], lines[1], lines[3]]) + "\n",
                               encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.ledger))
        self.assertEqual(error.exception.code, "LEDGER_CHAIN_BROKEN")

        for index in range(3):
            self.ledger.unlink(missing_ok=True)
            for position in range(3):
                import_document(run_document(run_id=f"run-{position}"), self.ledger,
                                source=SOURCE, program_digest=PROGRAM)
            lines = self.ledger.read_text(encoding="utf-8").split("\n")
            self.ledger.write_text("\n".join([lines[0], lines[2], lines[1]]) + "\n",
                                   encoding="utf-8")
            with self.assertRaises(s3.LedgerError) as error:
                s3.load_ledger(str(self.ledger))
            self.assertEqual(error.exception.code, "LEDGER_CHAIN_BROKEN")
            break

    # ---------------------------------------------------------------- case 8
    def test_ledger_cap_refuses_the_import(self) -> None:
        # The cap is reached by writing the full ledger directly; appending 1024 times
        # would re-validate the whole file each time and prove the same boundary.
        lines = [s3._encode(s3.header())]
        previous = s3.header_digest()
        for index in range(s3.MAX_LEDGER_ENTRIES):
            entry = s3.seal({
                "record": s3.RECORD_RUN, "entry_version": s3.ENTRY_VERSION,
                "run_id": f"filler-{index:04d}", "source": SOURCE,
                "candidate": {"id": "s2-candidate", "program_digest": PROGRAM},
                "host": {"os": "windows", "arch": "x86_64"},
                "interpreter": {"name": "cpython", "version": "3.13.14"},
                "start_utc": "1970-01-01T00:00:00Z", "end_utc": "1970-01-01T00:00:00Z",
                "outcome": "OBSERVED",
                "counters": {name: None for name in s3.COUNTERS},
            }, previous)
            previous = entry["entry_sha256"]
            lines.append(s3._encode(entry))
        self.ledger.write_text(chr(10).join(lines) + chr(10), encoding="utf-8")
        self.assertEqual(len(s3.load_ledger(str(self.ledger))), s3.MAX_LEDGER_ENTRIES)
        with self.assertRaises(s3.LedgerError) as error:
            import_document(run_document(), self.ledger, source=SOURCE,
                            program_digest=PROGRAM)
        self.assertEqual(error.exception.code, "LEDGER_ENTRY_LIMIT")

    # ---------------------------------------------------------------- case 9
    def test_same_input_reproduces_the_same_summary(self) -> None:
        first = import_document(run_document(), self.ledger, source=SOURCE,
                                program_digest=PROGRAM)
        first_summary = s3.summarise(s3.load_ledger(str(self.ledger)))
        second_ledger = Path(self.directory.name) / "second.jsonl"
        second = import_document(run_document(), second_ledger, source=SOURCE,
                                 program_digest=PROGRAM)
        second_summary = s3.summarise(s3.load_ledger(str(second_ledger)))
        self.assertEqual(first["ledger_digest"], second["ledger_digest"])
        self.assertEqual(first_summary, second_summary)

    # --------------------------------------------------------------- case 10
    def test_aggregate_matches_an_independent_expected_value(self) -> None:
        import_document(run_document(processed=7, present=5), self.ledger,
                        source=SOURCE, program_digest=PROGRAM)
        import_document(run_document(run_id="run-2", processed=3, present=2),
                        self.ledger, source=SOURCE, program_digest=PROGRAM)
        summary = s3.summarise(s3.load_ledger(str(self.ledger)))
        # Computed by hand from the two fixtures: 7 + 3 frames processed, 5 + 2 presents.
        self.assertEqual(summary["counters"]["frames_observed"]["value"], 10)
        self.assertEqual(summary["counters"]["present_calls"]["value"], 7)
        self.assertEqual(summary["counters"]["frames_observed"]["runs"], 2)
        self.assertEqual(summary["runs"], {"status": "OBSERVED", "value": 2})
        self.assertEqual(summary["counters"]["dirty_frames"]["status"], "NOT_MEASURED")
        self.assertEqual(summary["counters"]["scope_ns"]["status"], "NOT_MEASURED")

    # ------------------------------------------------------------- identity
    def test_manifest_mismatch_is_refused_before_any_write(self) -> None:
        wrong = dict(MANIFEST, definition_hash="sha256:" + "9" * 64)
        with self.assertRaises(imp.ImportError_) as error:
            import_document(run_document(), self.ledger, source=SOURCE,
                            program_digest=PROGRAM, expected_manifest=wrong)
        self.assertEqual(error.exception.code, "IMPORT_IDENTITY")
        self.assertFalse(self.ledger.exists(), "nothing may be written on a refusal")

    def test_invalid_result_is_refused_before_any_write(self) -> None:
        broken = run_document()
        broken["result_version"] = 99
        with self.assertRaises(imp.ImportError_) as error:
            import_document(broken, self.ledger, source=SOURCE, program_digest=PROGRAM)
        self.assertEqual(error.exception.code, "IMPORT_RESULT_VERSION")
        self.assertFalse(self.ledger.exists())

    def test_reconciliation_table_is_explicit(self) -> None:
        report = imp.reconciliation_report()
        self.assertEqual(report["counters"]["frames_observed"],
                         {"source": "frames.processed", "state": "matched"})
        self.assertEqual(report["counters"]["present_calls"],
                         {"source": "frames.present", "state": "matched"})
        self.assertEqual(report["counters"]["dirty_frames"],
                         {"source": None, "state": "UNMATCHED"})
        self.assertEqual(report["counters"]["scope_ns"],
                         {"source": None, "state": "UNMATCHED"})
        self.assertIn("resources.process", report["unmatched_s2_fields"])

    # --------------------------------------------------------- real replay
    def test_retained_s2_bundle_replays_deterministically(self) -> None:
        if not BUNDLE.is_file():
            self.skipTest("NOT_RUN: the retained S2 bundle is not present in this "
                          "checkout; no simulated replay is substituted")
        with zipfile.ZipFile(BUNDLE) as archive:
            result = json.loads(archive.read("n02d/result.json"))
            build = json.loads(archive.read("n02d/build-record.json"))
            identity = json.loads(archive.read("n02d/source-identity.json"))
        # The real result must pass the real contract before it is imported.
        contracts.validate_result(result)
        source = imp.source_identity_from_build_record({**build, "files": identity["files"]})
        program = imp.program_digest_from_build_record(build)
        self.assertIsNotNone(source["git_sha"], "the bundle records a baseline commit")
        self.assertIsNotNone(program, "the bundle records an executable digest")

        first = import_document(result, self.ledger, source=source, program_digest=program)
        entries = s3.load_ledger(str(self.ledger))
        summary = s3.summarise(entries)
        self.assertEqual(len(entries), 1)
        self.assertEqual(entries[0]["outcome"], result["runs"][0]["status"])
        self.assertEqual(entries[0]["source"], source)
        self.assertEqual(entries[0]["candidate"]["program_digest"], program)
        # The retained run is NOT_MEASURED, so it is traceable and excluded.
        self.assertEqual(summary["contributing"], [])
        self.assertEqual(summary["excluded"][0]["reason"], "OUTCOME_NOT_MEASURED")
        for name in s3.COUNTERS:
            self.assertEqual(summary["counters"][name]["status"], "NOT_MEASURED")

        # Independent replay: a fresh ledger from the same fixed input reproduces both the
        # chain head and the aggregate.
        second_ledger = Path(self.directory.name) / "replay.jsonl"
        second = import_document(result, second_ledger, source=source, program_digest=program)
        self.assertEqual(first["ledger_digest"], second["ledger_digest"])
        self.assertEqual(summary, s3.summarise(s3.load_ledger(str(second_ledger))))

    # ------------------------------------------------------------------- cli
    def test_cli_import_reports_closed_json(self) -> None:
        path = Path(self.directory.name) / "result.json"
        path.write_text(json.dumps(run_document()), encoding="utf-8")
        result = subprocess.run(
            [sys.executable, "-B", str(ROOT / "s3_import.py"), "import",
             "--result", str(path), "--ledger", str(self.ledger),
             "--git-sha", SOURCE["git_sha"],
             "--worktree-digest", SOURCE["worktree_digest"],
             "--program-digest", PROGRAM],
            capture_output=True, check=False, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["ok"], True)
        self.assertIn("ledger_digest", payload)
        self.assertNotIn(b"Traceback", result.stderr)

        broken = Path(self.directory.name) / "broken.json"
        broken.write_text("{}", encoding="utf-8")
        refused = subprocess.run(
            [sys.executable, "-B", str(ROOT / "s3_import.py"), "import",
             "--result", str(broken), "--ledger", str(self.ledger)],
            capture_output=True, check=False, timeout=20)
        self.assertEqual(refused.returncode, 1)
        self.assertEqual(json.loads(refused.stdout)["error"], "IMPORT_SCHEMA")
        self.assertNotIn(b"Traceback", refused.stderr)


if __name__ == "__main__":
    unittest.main()
