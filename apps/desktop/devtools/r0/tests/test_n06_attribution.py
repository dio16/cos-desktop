"""N06 attribution: feasibility table, journal counting and honest CPU labelling."""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import n06_attribution as n06  # noqa: E402

REPO = ROOT.parents[3]
BUNDLE = REPO / "docs/development/evidence/r0a-s2b/native-bundles.zip"


# A contract-valid capture is the only accepted input, so the fixtures start from the
# repository's own S2B wire example and clone its records with unique sequence numbers and
# spans; nothing here invents a shape the contract would reject.
FIXTURE = ROOT / "fixtures/s2b-wire-v1.json"


def base_capture() -> dict:
    return json.loads(FIXTURE.read_text(encoding="utf-8"))


def capture(records: list[dict]) -> dict:
    """A capture whose summary agrees with the records it carries."""
    document = base_capture()
    document["records"] = records
    kinds = [0] * 10
    for item in records:
        kinds[item["kind"]] += 1
    local_received = kinds[8] + kinds[9]
    document["summary"].update({
        "kind_counts": kinds,
        "journal_seen": sum(kinds[:8]),
        "local_received": local_received,
        "local_enqueued": local_received,
        "local_undrained": 0,
        "eligible": len(records),
        "retained": len(records),
        "retention_dropped": 0,
        "excluded": 0,
        "pre_scope": 0,
        "post_scope": 0,
        "invalid_clock": 0,
        "barrier_spans": 0,
        "correlation_invalid": 0,
        "input_unassociated": 0,
        "unmatched_submission": sum(1 for item in records if item["kind"] == 3
                                    and item["paired_draw_start_ns"] is None),
        "journal_lost": sum(item["lost"] or 0 for item in records),
    })
    return document


def record(kind: int, window: int = 0, seq: int = 1, offset: int = 0) -> dict:
    """One contract-valid record of the requested kind."""
    item = {"seq": seq, "kind": kind, "window": None, "lifetime": None,
            "start_ns": 10 + offset, "end_ns": 10 + offset, "dirty_ns": None,
            "invalidations": None, "paired_draw_start_ns": None, "paired_draw_end_ns": None,
            "category": None, "caused_invalidation": None, "lost": None}
    if kind in (1, 2, 3, 4, 8):
        item.update({"window": window, "lifetime": 1})
    if kind == 2:
        item["invalidations"] = 0
    if kind == 8:
        item["category"] = 0
    if kind == 5:
        item["lost"] = 1
    return item


class N06Attribution(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)

    def write_capture(self, records: list[dict]) -> Path:
        path = self.base / "capture.json"
        path.write_text(json.dumps(capture(records)), encoding="utf-8")
        return path

    # ------------------------------------------------------------- feasibility
    def test_feasibility_separates_countable_from_unobtainable(self) -> None:
        table = n06.feasibility()
        self.assertEqual(table["gpui_pin"], "395def7a067b32ed598fcb3f048969ada9b54942")
        for kind in n06.COUNTABLE:
            self.assertEqual(table["kinds"][kind],
                             {"status": "COUNTABLE", "source": "journal", "reason": ""}, kind)
        # Presentation and scanout need a probe this repository does not have.
        for kind in ("present", "displayed"):
            self.assertEqual(table["kinds"][kind]["reason"], "DWM_PROBE_UNAVAILABLE", kind)
        self.assertEqual(table["kinds"]["scanout"]["reason"],
                         "DISPLAY_DRIVER_PROBE_UNAVAILABLE")
        self.assertEqual(table["kinds"]["gpu-submission"]["reason"],
                         "GPU_PROBE_UNAVAILABLE")
        self.assertEqual(table["kinds"]["input-to-photon"]["reason"],
                         "END_TO_END_PROBE_UNAVAILABLE")
        self.assertEqual(table["frames_displayed"]["status"], "NOT_MEASURED")
        self.assertEqual(table["per_frame_cpu"]["status"], "NOT_MEASURED")
        self.assertFalse(table["app_reported_stand_in"],
                         "an app-reported number is not an independent endpoint")

    # ------------------------------------------------------------- frames
    def test_journal_events_are_counted_and_labelled(self) -> None:
        records = [record(1, 1, seq=1, offset=1), record(1, 1, seq=2, offset=2),
                   record(1, 1, seq=3, offset=3), record(2, 1, seq=4, offset=4),
                   record(2, 1, seq=5, offset=5), record(6, seq=6, offset=6)]
        path = self.write_capture(records)
        result = n06.count_journal(n06.load_capture(str(path)))
        self.assertEqual(result["status"], "OBSERVED")
        self.assertEqual(result["unit"], "journal_events",
                         "journal events are not displayed frames")
        self.assertEqual(result["by_kind"]["gpui-dirty"], 3)
        self.assertEqual(result["by_kind"]["gpui-cpu-draw"], 2)
        self.assertEqual(result["by_kind"]["gpui-platform-submission"], 0)
        self.assertEqual(result["total"], 5)
        self.assertEqual(result["unclassified"], 1, "kind 6 is not a counted journal kind")
        self.assertEqual(result["by_window"], {"1": 5})
        self.assertEqual(result["frames_displayed"],
                         {"status": "NOT_MEASURED", "reason": "DWM_PROBE_UNAVAILABLE"})

    def test_unvalidated_capture_is_refused(self) -> None:
        """A document that is only shaped like a capture is not an observation."""
        path = self.base / "minimal.json"
        path.write_text('{"records":[{"kind":1}]}', encoding="utf-8")
        with self.assertRaises(n06.AttributionError) as error:
            n06.load_capture(str(path))
        self.assertEqual(error.exception.code, "CAPTURE_SCHEMA")
        # And through the CLI: closed JSON, no traceback, nothing counted.
        result = subprocess.run([sys.executable, "-B", str(ROOT / "n06_attribution.py"),
                                 "frames", "--capture", str(path)],
                                capture_output=True, check=False, timeout=30)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout)["error"], "CAPTURE_SCHEMA")
        self.assertNotIn(b"Traceback", result.stderr)

    def test_contract_capture_is_counted_and_not_claimed_source_bound(self) -> None:
        fixture = ROOT / "fixtures/s2b-wire-v1.json"
        if not fixture.is_file():
            self.skipTest("NOT_RUN: the S2B wire fixture is not present")
        result = n06.count_journal(n06.load_capture(str(fixture)))
        self.assertEqual(result["status"], "OBSERVED")
        self.assertEqual(result["unit"], "journal_events")
        self.assertGreater(result["total"], 0)
        # Contract-valid is not source-bound: the capture carries no source identity.
        self.assertFalse(result["provenance"]["source_bound"])
        self.assertEqual(result["provenance"]["status"], "NOT_MEASURED")
        self.assertEqual(result["provenance"]["reason"], "NO_SOURCE_IDENTITY_IN_CAPTURE")
        self.assertTrue(result["provenance"]["executable_sha256"].startswith("sha256:"))
        self.assertEqual(result["provenance"]["gpui_pin"], n06.s2b.GPUI_PIN)

    def test_empty_capture_is_not_measured(self) -> None:
        # A contract-valid capture with no records is still a capture: it reports the
        # explicit NOT_MEASURED status rather than a fabricated zero.
        path = self.write_capture([])
        result = n06.count_journal(n06.load_capture(str(path)))
        self.assertEqual(result["status"], "NOT_MEASURED")
        self.assertEqual(result["reason"], "NO_JOURNAL_RECORDS")
        self.assertEqual(result["total"], 0)
        self.assertEqual(result["frames_displayed"]["status"], "NOT_MEASURED")

    def test_record_and_window_caps_are_enforced(self) -> None:
        # The S2B contract caps a capture at 256 records; the tool cap sits above it, so an
        # over-long capture is refused by the contract and stays a closed failure.
        records = [record(1, index, seq=index + 1, offset=index)
                   for index in range(n06.s2b.MAX_RECORDS + 1)]
        path = self.write_capture(records)
        with self.assertRaises(n06.AttributionError) as error:
            n06.load_capture(str(path))
        self.assertEqual(error.exception.code, "CAPTURE_SCHEMA")

        # The window limit is the contract's too (16), so an over-windowed capture is
        # refused there first; the tool keeps its own bound as defence in depth.
        records = [record(1, window, seq=window + 1, offset=window)
                   for window in range(n06.MAX_WINDOWS + 1)]
        path = self.write_capture(records)
        with self.assertRaises(n06.AttributionError) as error:
            n06.load_capture(str(path))
        self.assertEqual(error.exception.code, "CAPTURE_SCHEMA")

    # ------------------------------------------------------------- cpu
    def test_cpu_labels_are_explicit(self) -> None:
        if shutil.which("powershell") is None:
            self.skipTest("NOT_RUN: Windows PowerShell is not on PATH")
        child = subprocess.Popen([sys.executable, "-c", "x = 0\nwhile True: x = x + 1"])
        self.addCleanup(child.kill)
        result = n06.sample_process_cpu(child.pid, 1)
        self.assertEqual(result["status"], "OBSERVED")
        self.assertEqual(result["attribution"], "process_over_window",
                         "process CPU over the window, never per-frame attribution")
        self.assertGreater(result["window_ms"], 0)
        self.assertGreaterEqual(result["cpu_ms"], 0)
        self.assertEqual(result["per_frame_cpu"],
                         {"status": "NOT_MEASURED", "reason": "NO_FRAME_ATTRIBUTION"})
        self.assertIn("not per-frame attribution", result["note"])

    def test_cpu_cap_and_closed_errors(self) -> None:
        with self.assertRaises(n06.AttributionError) as error:
            n06.sample_process_cpu(1, 0)
        self.assertEqual(error.exception.code, "ARGS_CAP")
        with self.assertRaises(n06.AttributionError) as error:
            n06.sample_process_cpu(1, n06.MAX_CPU_SECONDS + 1)
        self.assertEqual(error.exception.code, "ARGS_CAP")
        with self.assertRaises(n06.AttributionError) as error:
            n06.sample_process_cpu(1, 1, str(self.base / "no-powershell.exe"))
        self.assertEqual(error.exception.code, "CPU_PROBE_MISSING")

    # ------------------------------------------------------------- cli
    def test_cli_is_closed_and_has_help(self) -> None:
        helped = subprocess.run([sys.executable, "-B", str(ROOT / "n06_attribution.py"),
                                 "--help"], capture_output=True, check=False, timeout=30)
        self.assertEqual(helped.returncode, 0)
        missing = subprocess.run([sys.executable, "-B", str(ROOT / "n06_attribution.py"),
                                  "frames", "--capture", str(self.base / "absent.json")],
                                 capture_output=True, check=False, timeout=30)
        self.assertEqual(missing.returncode, 1)
        self.assertEqual(json.loads(missing.stdout)["error"], "READ_FAILED")
        self.assertNotIn(b"Traceback", missing.stderr)

        broken = self.base / "broken.json"
        broken.write_text("{not json", encoding="utf-8")
        refused = subprocess.run([sys.executable, "-B", str(ROOT / "n06_attribution.py"),
                                  "frames", "--capture", str(broken)],
                                 capture_output=True, check=False, timeout=30)
        self.assertEqual(refused.returncode, 1)
        self.assertEqual(json.loads(refused.stdout)["error"], "SCHEMA")
        self.assertNotIn(b"Traceback", refused.stderr)

    # ------------------------------------------------------------- real capture
    def test_retained_capture_reports_counters(self) -> None:
        if not BUNDLE.is_file():
            self.skipTest("NOT_RUN: the retained S2B bundle is not present in this checkout")
        with zipfile.ZipFile(BUNDLE) as archive:
            if "n02d/s2b-capture.json" not in archive.namelist():
                self.skipTest("NOT_RUN: the retained bundle has no s2b-capture.json")
            document = json.loads(archive.read("n02d/s2b-capture.json"))
        path = self.base / "real-capture.json"
        path.write_text(json.dumps(document), encoding="utf-8")
        result = n06.count_journal(n06.load_capture(str(path)))
        self.assertIn(result["status"], ("OBSERVED", "NOT_MEASURED"))
        self.assertEqual(result["unit"], "journal_events")
        self.assertEqual(result["frames_displayed"]["status"], "NOT_MEASURED")
        if result["status"] == "OBSERVED":
            self.assertGreater(result["total"], 0)


if __name__ == "__main__":
    unittest.main()
