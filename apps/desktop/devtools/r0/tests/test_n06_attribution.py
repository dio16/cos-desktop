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


def capture(records: list[dict]) -> dict:
    return {"records": records}


def record(kind: str, window: int = 0) -> dict:
    return {"seq": 0, "kind": kind, "window": window, "lifetime": 0,
            "start_ns": 0, "end_ns": 0, "dirty_ns": 0, "invalidations": 0,
            "paired_draw_start_ns": 0, "paired_draw_end_ns": 0, "category": 0,
            "caused_invalidation": 0, "lost": 0}


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
        path = self.write_capture([
            record("gpui-dirty", 1), record("gpui-dirty", 1), record("gpui-cpu-draw", 2),
            record("gpui-platform-submission", 2), record("present", 2),
            # The wire stores the numeric kind code; it must count the same way.
            record(1, 3), record(2, 3),
        ])
        result = n06.count_journal(n06.load_capture(str(path)))
        self.assertEqual(result["status"], "OBSERVED")
        self.assertEqual(result["unit"], "journal_events",
                         "journal events are not displayed frames")
        self.assertEqual(result["by_kind"]["gpui-dirty"], 3, "names and codes count alike")
        self.assertEqual(result["by_kind"]["gpui-cpu-draw"], 2)
        self.assertEqual(result["total"], 6)
        self.assertEqual(result["unclassified"], 1, "present is not a journal kind here")
        self.assertEqual(result["by_window"], {"1": 2, "2": 2, "3": 2})
        self.assertEqual(result["frames_displayed"],
                         {"status": "NOT_MEASURED", "reason": "DWM_PROBE_UNAVAILABLE"})

    def test_empty_capture_is_not_measured(self) -> None:
        path = self.write_capture([])
        result = n06.count_journal(n06.load_capture(str(path)))
        self.assertEqual(result["status"], "NOT_MEASURED")
        self.assertEqual(result["reason"], "NO_JOURNAL_RECORDS")
        self.assertEqual(result["total"], 0)
        self.assertEqual(result["frames_displayed"]["status"], "NOT_MEASURED")

    def test_record_and_window_caps_are_enforced(self) -> None:
        path = self.write_capture([record("gpui-dirty") for _ in
                                   range(n06.MAX_RECORDS + 1)])
        with self.assertRaises(n06.AttributionError) as error:
            n06.count_journal(n06.load_capture(str(path)))
        self.assertEqual(error.exception.code, "CAPTURE_RECORD_LIMIT")

        path = self.write_capture([record("gpui-dirty", window)
                                   for window in range(n06.MAX_WINDOWS + 1)])
        with self.assertRaises(n06.AttributionError) as error:
            n06.count_journal(n06.load_capture(str(path)))
        self.assertEqual(error.exception.code, "CAPTURE_RECORD_LIMIT")

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
