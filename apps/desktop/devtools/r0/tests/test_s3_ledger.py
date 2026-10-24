"""S3 ledger, bounded loading and aggregation. Wire records are not Native evidence."""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import s3_ledger as s3  # noqa: E402

DIGEST = "sha256:" + "0" * 64
PROGRAM = "sha256:" + "1" * 64
WORKTREE = "sha256:" + "2" * 64


def entry(run_id: str, *, outcome: str = "OBSERVED", git_sha: str | None = "a" * 40,
          counters: dict | None = None) -> dict:
    """One valid run entry, digest included."""
    body = {
        "record": s3.RECORD_RUN,
        "entry_version": s3.ENTRY_VERSION,
        "run_id": run_id,
        "source": {"git_sha": git_sha, "worktree_digest": WORKTREE},
        "candidate": {"id": "candidate-a", "program_digest": PROGRAM},
        "host": {"os": "windows", "arch": "x86_64"},
        "interpreter": {"name": "cpython", "version": "3.13.14"},
        "start_utc": "2026-10-24T00:00:00Z",
        "end_utc": "2026-10-24T00:00:05Z",
        "outcome": outcome,
        "counters": {name: (counters or {}).get(name) for name in s3.COUNTERS},
    }
    body["entry_sha256"] = s3.entry_digest(body)
    return body


def header_line() -> str:
    return s3._encode(s3._header())


def write_ledger(path: Path, entries: list[dict], *, header: str | None = None) -> None:
    lines = [header_line() if header is None else header]
    lines.extend(s3._encode(entry) for entry in entries)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def invoke(*arguments: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, "-B", str(ROOT / "s3_ledger.py"), *arguments],
        capture_output=True, check=False, timeout=20,
    )


class S3LedgerContract(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "s3-ledger.jsonl"

    # ------------------------------------------------------------------ append

    def test_append_never_rewrites_an_existing_record(self) -> None:
        first = entry("run-a")
        s3.append_entry(str(self.path), first)
        before = self.path.read_bytes()
        s3.append_entry(str(self.path), entry("run-b", counters={"frames_observed": 3}))
        after = self.path.read_bytes()
        self.assertTrue(after.startswith(before), "an append must not touch existing bytes")
        self.assertEqual(len(after.splitlines()), 3, "header plus two entries")

    def test_duplicate_run_id_is_refused(self) -> None:
        s3.append_entry(str(self.path), entry("run-a"))
        with self.assertRaises(s3.LedgerError) as error:
            s3.append_entry(str(self.path), entry("run-a"))
        self.assertEqual(error.exception.code, "DUPLICATE_RUN")
        # A ledger assembled by hand with a duplicate is refused as well.
        write_ledger(self.path, [entry("run-a"), entry("run-a")])
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "DUPLICATE_RUN")

    # -------------------------------------------------------------- tampering

    def test_tampered_entry_is_detected(self) -> None:
        write_ledger(self.path, [entry("run-a", counters={"frames_observed": 5})])
        text = self.path.read_text(encoding="utf-8")
        self.path.write_text(text.replace('"frames_observed":5', '"frames_observed":6'),
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.verify_ledger(str(self.path))
        self.assertEqual(error.exception.code, "DIGEST_MISMATCH")

    # -------------------------------------------------------------- provenance

    def test_exclusion_provenance_and_not_measured_counters(self) -> None:
        entries = [
            entry("run-observed", counters={"frames_observed": 4, "scope_ns": 10}),
            entry("run-failed", outcome="FAILED", counters={"frames_observed": 9}),
            entry("run-cancelled", outcome="CANCELLED", counters={"frames_observed": 9}),
            entry("run-not-measured", outcome="NOT_MEASURED"),
            entry("run-unbound", git_sha=None),
        ]
        summary = s3.summarise(entries)
        self.assertEqual(summary["contributing"], ["run-observed"])
        self.assertEqual(
            [(row["run_id"], row["reason"]) for row in summary["excluded"]],
            [
                ("run-failed", "OUTCOME_FAILED"),
                ("run-cancelled", "OUTCOME_CANCELLED"),
                ("run-not-measured", "OUTCOME_NOT_MEASURED"),
                ("run-unbound", "SOURCE_NOT_BOUND"),
            ],
        )
        self.assertEqual(summary["counters"]["frames_observed"],
                         {"status": "OBSERVED", "value": 4, "runs": 1})
        self.assertEqual(summary["counters"]["dirty_frames"],
                         {"status": "NOT_MEASURED", "reason": "NO_RUN_MEASURED_THIS_COUNTER"})
        self.assertEqual(summary["runs"], {"status": "OBSERVED", "value": 1})

    def test_unbound_source_never_contributes(self) -> None:
        # A run may declare itself unbound (`null`), and the loader accepts it; the
        # aggregation is what excludes it, with an explicit reason.
        write_ledger(self.path, [entry("run-unbound", git_sha=None)])
        entries = s3.load_ledger(str(self.path))
        self.assertEqual(len(entries), 1)
        summary = s3.summarise(entries)
        self.assertEqual(summary["contributing"], [])
        self.assertEqual(summary["excluded"],
                         [{"run_id": "run-unbound", "reason": "SOURCE_NOT_BOUND"}])
        self.assertEqual(summary["runs"],
                         {"status": "NOT_MEASURED", "reason": "NO_CONTRIBUTING_RUNS"})

    # -------------------------------------------------------------- malformed

    def test_malformed_ledger_fails_closed(self) -> None:
        cases = {
            "MALFORMED_JSON": [header_line(), "{not json"],
            "DUPLICATE_KEY": [header_line(), '{"record":"run_entry","record":"run_entry"}'],
            "NON_FINITE_NUMBER": [header_line(), '{"record":"run_entry","x":NaN}'],
            "LEDGER_HEADER_INVALID": ['{"record":"something_else","ledger_version":1}'],
            "LEDGER_VERSION": ['{"record":"ledger_header","ledger_version":9}'],
            "LEDGER_EMPTY": [],
        }
        # A blank line inside the ledger is malformed, not skipped.
        cases["MALFORMED_JSON_BLANK"] = [header_line(), "", "{}"]
        for code, lines in cases.items():
            self.path.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
            with self.assertRaises(s3.LedgerError) as error:
                s3.load_ledger(str(self.path))
            expected = "MALFORMED_JSON" if code == "MALFORMED_JSON_BLANK" else code
            self.assertEqual(error.exception.code, expected, code)

    def test_invalid_entry_schema_is_named(self) -> None:
        broken = entry("run-a")
        broken["counters"].pop("scope_ns")
        broken["entry_sha256"] = s3.entry_digest(broken)
        self.path.write_text(header_line() + "\n" + s3._encode(broken) + "\n",
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "MISSING_FIELD")
        self.assertEqual(error.exception.line, 2, "the bad entry's line is reported")

    # -------------------------------------------------------------------- caps

    def test_caps_are_enforced(self) -> None:
        # Entry count.
        write_ledger(self.path, [entry(f"run-{index:04d}") for index in
                                 range(s3.MAX_LEDGER_ENTRIES + 1)])
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "LEDGER_ENTRY_LIMIT")

        # Entry size: one oversized line is refused before parsing.
        self.path.write_text(header_line() + "\n" + "x" * (s3.MAX_ENTRY_BYTES + 1) + "\n",
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "LEDGER_ENTRY_TOO_LARGE")

        # Ledger size.
        self.path.write_bytes(b"x" * (s3.MAX_LEDGER_BYTES + 1))
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "LEDGER_TOO_LARGE")

    # ----------------------------------------------------------------- summary

    def test_empty_ledger_summary_says_not_measured(self) -> None:
        write_ledger(self.path, [])
        summary = s3.summarise(s3.load_ledger(str(self.path)))
        self.assertEqual(summary["entries_total"], 0)
        self.assertEqual(summary["contributing"], [])
        self.assertEqual(summary["runs"],
                         {"status": "NOT_MEASURED", "reason": "NO_CONTRIBUTING_RUNS"})
        for name in s3.COUNTERS:
            self.assertEqual(summary["counters"][name],
                             {"status": "NOT_MEASURED", "reason": "NO_CONTRIBUTING_RUNS"})

    def test_summary_is_deterministic(self) -> None:
        write_ledger(self.path, [entry("run-b", counters={"frames_observed": 1}),
                                 entry("run-a", counters={"frames_observed": 2})])
        first = invoke("summary", "--ledger", str(self.path))
        second = invoke("summary", "--ledger", str(self.path))
        self.assertEqual(first.returncode, 0, first.stdout)
        self.assertEqual(first.stdout, second.stdout)
        payload = json.loads(first.stdout)
        self.assertEqual(payload["ok"], True)
        self.assertEqual(payload["counters"]["frames_observed"]["value"], 3)

    # --------------------------------------------------------------------- cli

    def test_cli_round_trip_and_help(self) -> None:
        entry_path = Path(self.directory.name) / "entry.json"
        entry_path.write_text(json.dumps(entry("run-cli", counters={"frames_observed": 7})),
                              encoding="utf-8")
        appended = invoke("ledger", "append", "--ledger", str(self.path),
                          "--entry", str(entry_path))
        self.assertEqual(appended.returncode, 0, appended.stdout)
        self.assertEqual(json.loads(appended.stdout)["ok"], True)

        verified = invoke("ledger", "verify", "--ledger", str(self.path))
        self.assertEqual(verified.returncode, 0, verified.stdout)
        self.assertEqual(json.loads(verified.stdout)["entries"], 1)

        summary = invoke("summary", "--ledger", str(self.path))
        self.assertEqual(summary.returncode, 0, summary.stdout)
        self.assertEqual(json.loads(summary.stdout)["contributing"], ["run-cli"])

        for arguments in (["--help"], ["ledger", "--help"], ["ledger", "append", "--help"],
                          ["ledger", "verify", "--help"], ["summary", "--help"]):
            helped = invoke(*arguments)
            self.assertEqual(helped.returncode, 0, arguments)
            self.assertNotIn(b"Traceback", helped.stderr)

    def test_cli_reports_a_closed_error_without_a_traceback(self) -> None:
        self.path.write_text(header_line() + "\n{not json\n", encoding="utf-8")
        result = invoke("summary", "--ledger", str(self.path))
        self.assertEqual(result.returncode, 1)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["ok"], False)
        self.assertEqual(payload["error"], "MALFORMED_JSON")
        self.assertEqual(payload["line"], 2)
        self.assertNotIn(b"Traceback", result.stderr)

        # A missing ledger is a closed read failure, not a traceback.
        missing = invoke("summary", "--ledger", str(self.path) + ".absent")
        self.assertEqual(missing.returncode, 1)
        self.assertEqual(json.loads(missing.stdout)["error"], "READ_FAILED")
        self.assertNotIn(b"Traceback", missing.stderr)


if __name__ == "__main__":
    unittest.main()
