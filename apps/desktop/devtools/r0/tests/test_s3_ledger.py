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

PROGRAM = "sha256:" + "1" * 64
WORKTREE = "sha256:" + "2" * 64


def body(run_id: str, *, outcome: str = "OBSERVED", git_sha: str | None = "a" * 40,
         counters: dict | None = None) -> dict:
    """One valid entry body (no digest fields): what an operator or producer supplies."""
    return {
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


def sealed_chain(bodies: list[dict]) -> list[str]:
    """Seal bodies into a chain: the header digest first, then each entry's digest."""
    previous = s3.header_digest()
    lines = []
    for item in bodies:
        entry = s3.seal(s3.validate_body(item), previous)
        previous = entry["entry_sha256"]
        lines.append(s3._encode(entry))
    return lines


def header_line() -> str:
    return s3._encode(s3.header())


def write_ledger(path: Path, bodies: list[dict], *, newline: bool = True) -> None:
    lines = [header_line(), *sealed_chain(bodies)]
    path.write_text("\n".join(lines) + ("\n" if newline else ""), encoding="utf-8")


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
        first = s3.append_entry(str(self.path), body("run-a"))
        before = self.path.read_bytes()
        second = s3.append_entry(
            str(self.path), body("run-b", counters={"frames_observed": 3}))
        after = self.path.read_bytes()
        self.assertTrue(after.startswith(before), "an append must not touch existing bytes")
        self.assertEqual(len(after.splitlines()), 3, "header plus two entries")
        # The chain links the header to the first entry and the entries to each other.
        self.assertEqual(first["previous_sha256"], s3.header_digest())
        self.assertEqual(second["previous_sha256"], first["entry_sha256"])
        self.assertEqual(second["ledger_digest"], second["entry_sha256"])

    def test_duplicate_run_id_is_refused(self) -> None:
        s3.append_entry(str(self.path), body("run-a"))
        with self.assertRaises(s3.LedgerError) as error:
            s3.append_entry(str(self.path), body("run-a"))
        self.assertEqual(error.exception.code, "DUPLICATE_RUN")
        write_ledger(self.path, [body("run-a"), body("run-a")])
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "DUPLICATE_RUN")

    def test_append_after_a_missing_newline_does_not_corrupt(self) -> None:
        """A2: a last line without its newline must not be concatenated onto."""
        write_ledger(self.path, [body("run-a")], newline=False)
        self.assertTrue(self.path.read_bytes().endswith(b"}"), "no trailing newline")
        self.assertEqual(len(s3.load_ledger(str(self.path))), 1)

        s3.append_entry(str(self.path), body("run-b"))
        entries = s3.load_ledger(str(self.path))
        self.assertEqual([entry["run_id"] for entry in entries], ["run-a", "run-b"])
        self.assertTrue(self.path.read_bytes().endswith(b"\n"))

    # ------------------------------------------------------- integrity / tamper

    def test_tampered_entry_is_detected(self) -> None:
        write_ledger(self.path, [body("run-a", counters={"frames_observed": 5})])
        text = self.path.read_text(encoding="utf-8")
        self.path.write_text(text.replace('"frames_observed":5', '"frames_observed":6'),
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.verify_ledger(str(self.path))
        self.assertEqual(error.exception.code, "DIGEST_MISMATCH")

    def test_deleted_interior_line_breaks_the_chain(self) -> None:
        write_ledger(self.path, [body("run-a"), body("run-b"), body("run-c")])
        lines = self.path.read_text(encoding="utf-8").split("\n")
        # Drop run-b: run-c still carries run-b's digest as its predecessor.
        self.path.write_text("\n".join([lines[0], lines[1], lines[3]]) + "\n",
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "LEDGER_CHAIN_BROKEN")

    def test_reordered_entries_break_the_chain(self) -> None:
        write_ledger(self.path, [body("run-a"), body("run-b")])
        lines = self.path.read_text(encoding="utf-8").split("\n")
        self.path.write_text("\n".join([lines[0], lines[2], lines[1]]) + "\n",
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "LEDGER_CHAIN_BROKEN")

    def test_truncated_tail_is_visible_through_the_ledger_digest(self) -> None:
        """Documented residual: a dropped last line leaves a valid chain, so the head
        digest is the anchor a consumer must record out of band."""
        write_ledger(self.path, [body("run-a"), body("run-b")])
        full = s3.verify_ledger(str(self.path))
        lines = self.path.read_text(encoding="utf-8").split("\n")
        self.path.write_text("\n".join(lines[:2]) + "\n", encoding="utf-8")
        truncated = s3.verify_ledger(str(self.path))
        self.assertEqual(truncated["entries"], 1)
        self.assertNotEqual(truncated["ledger_digest"], full["ledger_digest"])

    # -------------------------------------------------------------- provenance

    def test_exclusion_provenance_and_not_measured_counters(self) -> None:
        write_ledger(self.path, [
            body("run-observed", counters={"frames_observed": 4, "scope_ns": 10}),
            body("run-failed", outcome="FAILED", counters={"frames_observed": 9}),
            body("run-cancelled", outcome="CANCELLED", counters={"frames_observed": 9}),
            body("run-not-measured", outcome="NOT_MEASURED"),
            body("run-unbound", git_sha=None),
        ])
        entries = s3.load_ledger(str(self.path))
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
        self.assertEqual(summary["ledger_digest"], entries[-1]["entry_sha256"])

    def test_unbound_source_never_contributes(self) -> None:
        write_ledger(self.path, [body("run-unbound", git_sha=None)])
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
            "MALFORMED_JSON_BLANK": [header_line(), "", "{}"],
        }
        for code, lines in cases.items():
            self.path.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
            with self.assertRaises(s3.LedgerError) as error:
                s3.load_ledger(str(self.path))
            expected = "MALFORMED_JSON" if code == "MALFORMED_JSON_BLANK" else code
            self.assertEqual(error.exception.code, expected, code)

    def test_invalid_entry_schema_is_named(self) -> None:
        complete = body("run-a")
        broken = {**complete, "counters": {name: complete["counters"][name]
                                           for name in s3.COUNTERS if name != "scope_ns"}}
        entry = s3.seal(broken, s3.header_digest())
        self.path.write_text(header_line() + "\n" + s3._encode(entry) + "\n",
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "MISSING_FIELD")
        self.assertEqual(error.exception.line, 2, "the bad entry's line is reported")

    # -------------------------------------------------------------------- caps

    def test_caps_are_enforced(self) -> None:
        write_ledger(self.path, [body(f"run-{index:04d}") for index in
                                 range(s3.MAX_LEDGER_ENTRIES + 1)])
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "LEDGER_ENTRY_LIMIT")

        self.path.write_text(header_line() + "\n" + "x" * (s3.MAX_ENTRY_BYTES + 1) + "\n",
                             encoding="utf-8")
        with self.assertRaises(s3.LedgerError) as error:
            s3.load_ledger(str(self.path))
        self.assertEqual(error.exception.code, "LEDGER_ENTRY_TOO_LARGE")

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
        self.assertEqual(summary["ledger_digest"], s3.header_digest())

    def test_summary_is_deterministic(self) -> None:
        write_ledger(self.path, [body("run-b", counters={"frames_observed": 1}),
                                 body("run-a", counters={"frames_observed": 2})])
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
        entry_path.write_text(json.dumps(body("run-cli", counters={"frames_observed": 7})),
                              encoding="utf-8")
        appended = invoke("ledger", "append", "--ledger", str(self.path),
                          "--entry", str(entry_path))
        self.assertEqual(appended.returncode, 0, appended.stdout)
        self.assertEqual(json.loads(appended.stdout)["ok"], True)

        verified = invoke("ledger", "verify", "--ledger", str(self.path))
        self.assertEqual(verified.returncode, 0, verified.stdout)
        verified_payload = json.loads(verified.stdout)
        self.assertEqual(verified_payload["entries"], 1)
        self.assertEqual(verified_payload["ledger_digest"],
                         json.loads(appended.stdout)["entry_sha256"])

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

        missing = invoke("summary", "--ledger", str(self.path) + ".absent")
        self.assertEqual(missing.returncode, 1)
        self.assertEqual(json.loads(missing.stdout)["error"], "READ_FAILED")
        self.assertNotIn(b"Traceback", missing.stderr)


if __name__ == "__main__":
    unittest.main()
