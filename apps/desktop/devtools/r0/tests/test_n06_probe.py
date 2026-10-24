"""N06 independent probe: OS endpoints, closed vocabulary, caps and derived labels.

The fake probe is a real PowerShell script driven by the real CLI (`--probe` is the
documented test seam), so these tests exercise the same command path as production rather
than monkeypatching the CLI's own internals. Every value the tool cannot obtain must stay
NOT_MEASURED with a closed reason, and an app-reported number must never reach an
independent field.
"""
from __future__ import annotations

import copy
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import n06_attribution as n06  # noqa: E402

CAPS = {"seconds": 10, "interval_ms": 20, "max_samples": 2000,
        "max_window_pixels": 4_000_000}
PROBE_PARAMS = ("param([int]$ProcessId, [int]$Seconds = 10, [int]$IntervalMs = 20, "
                "[int]$MaxSamples = 2000, [int]$MaxWindowPixels = 4000000)\n")


def probe_document(**overrides) -> dict:
    """A schema-valid probe document; each test overrides only the field it is testing."""
    document = {
        "ok": True,
        "probe_version": 1,
        "pid": 4242,
        "window_ms": 10_000.0,
        "caps": dict(CAPS),
        "window": {"status": "OBSERVED", "reason": "", "title": "COS Desktop",
                   "handle": 1234, "left": 10, "top": 10, "width": 1280, "height": 832,
                   "visible": True, "minimized": False},
        "cpu": {"status": "OBSERVED", "reason": "", "cpu_ms": 125.5,
                "source": "GetProcessTimes", "attribution": "process_over_window"},
        "dwm": {"status": "NOT_MEASURED", "reason": "DWM_TIMING_UNAVAILABLE",
                "error": "hr=0x88980090", "composed_delta": 0, "displayed_delta": 0,
                "dropped_delta": 0, "missed_delta": 0, "late_delta": 0},
        "frames": {"status": "OBSERVED", "reason": "", "method": "PrintWindow(PW_RENDERFULLCONTENT)",
                   "samples": 400, "changes": 120, "uniform_captures": 0,
                   "failed_captures": 0, "interval_ms": 20},
        "gpu": {"status": "OBSERVED", "reason": "", "error": "", "samples": 9,
                "engine_instances": 4, "mean_percent_summed_over_engines": 7.5,
                "max_percent_summed_over_engines": 22.0, "engine_types": "3d,copy"},
    }
    for key, value in overrides.items():
        document[key] = value
    return document


def windowless_document() -> dict:
    """The shape this host produces for a live process that owns no visible window."""
    document = probe_document()
    document["window"] = {"status": "NOT_MEASURED", "reason": "WINDOW_NOT_FOUND",
                          "title": "", "handle": 0, "left": 0, "top": 0, "width": 0,
                          "height": 0, "visible": False, "minimized": False}
    document["frames"] = {"status": "NOT_MEASURED", "reason": "WINDOW_NOT_FOUND",
                          "method": "PrintWindow(PW_RENDERFULLCONTENT)", "samples": 0,
                          "changes": 0, "uniform_captures": 0, "failed_captures": 0,
                          "interval_ms": 20}
    document["gpu"] = {"status": "NOT_MEASURED", "reason": "GPU_NO_PID_INSTANCES",
                       "error": "", "samples": 0, "engine_instances": 0,
                       "mean_percent_summed_over_engines": 0.0,
                       "max_percent_summed_over_engines": 0.0, "engine_types": ""}
    return document


def build(document: dict, executable: str | None = "sha256:" + "1" * 64) -> dict:
    return n06.build_record(document, probe=Path("fake_probe.ps1"),
                            probe_sha256="sha256:" + "0" * 64,
                            executable_sha256=executable)


class ProbeDocumentTests(unittest.TestCase):
    """Pure validation and record building: no subprocess, no clock."""

    def validate(self, document: dict) -> dict:
        return n06.validate_probe(document, {"pid": document["pid"], "caps": dict(CAPS)})

    def test_feasibility_states_capability_not_a_measurement(self) -> None:
        table = n06.feasibility()
        probe = table["independent_probe"]
        self.assertFalse(probe["app_reported_stand_in"])
        self.assertEqual(probe["endpoints"]["app_displayed_frames"],
                         {"status": "NOT_MEASURED", "reason": "NO_PER_WINDOW_DISPLAY_COUNTER"})
        self.assertFalse(probe["endpoints"]["compositor_desktop_wide"]["per_app"])
        self.assertIn("capability", probe["capability_note"])
        # The journal table's own claims are unchanged: an idle baseline is not a present
        # baseline and the probe availability does not turn journal events into frames.
        self.assertEqual(table["frames_displayed"]["status"], "NOT_MEASURED")
        self.assertEqual(table["per_frame_cpu"]["status"], "NOT_MEASURED")
        self.assertFalse(table["app_reported_stand_in"])

    def test_executable_identity_is_its_own_named_block(self) -> None:
        record = build(self.validate(probe_document()))
        executable = record["target"]["executable"]
        self.assertEqual(executable["status"], "ASSERTED",
                         "a caller-supplied digest is not a probe measurement")
        self.assertEqual(executable["sha256"], "sha256:" + "1" * 64)
        self.assertEqual(executable["identity_source"],
                         "calling_runner_prelaunch_digest_verification")
        self.assertIn("not a probe measurement", executable["observation"])
        self.assertIn("run-summary.json", executable["observation"])

        absent = build(self.validate(probe_document()), executable=None)
        self.assertEqual(absent["target"]["executable"]["status"], "NOT_MEASURED")
        self.assertEqual(absent["target"]["executable"]["reason"],
                         "EXECUTABLE_IDENTITY_NOT_SUPPLIED")
        self.assertIsNone(absent["target"]["executable"]["sha256"])
        n06.validate_record(absent)

    # ------------------------------------------------------------- value present
    def test_present_values_become_labelled_endpoints(self) -> None:
        document = self.validate(probe_document())
        record = build(document)
        self.assertFalse(record["app_reported_stand_in"])
        frames = record["frames"]["window_content_changes"]
        self.assertEqual(frames["status"], "OBSERVED")
        self.assertEqual(frames["unit"], "sampled_window_content_changes",
                         "content changes are not a frame counter")
        self.assertEqual(frames["changes"], 120)
        self.assertEqual(frames["samples"], 400)
        self.assertEqual(frames["achieved_mean_interval_ms"], 25.0,
                         "the requested interval is a request; the achieved cadence is derived")
        self.assertEqual(record["frames"]["compositor_desktop_wide"]["per_app"], False,
                         "DWM counters are desktop-wide")
        self.assertEqual(record["frames"]["app_displayed_frames"]["status"], "NOT_MEASURED")
        self.assertEqual(record["frames"]["app_displayed_frames"]["reason"],
                         "NO_PER_WINDOW_DISPLAY_COUNTER")
        self.assertEqual(record["cpu"]["attribution"], "process_over_window")
        self.assertEqual(record["cpu"]["cpu_ms"], 125.5)
        self.assertEqual(record["gpu"]["mean_percent_summed_over_engines"], 7.5)
        ratio = record["per_frame_cpu"]
        self.assertEqual(ratio["status"], "DERIVED")
        self.assertEqual(ratio["kind"], "ratio")
        self.assertEqual(ratio["denominator"], 120)
        self.assertAlmostEqual(ratio["value_ms_per_observed_change"], 125.5 / 120, places=6)
        self.assertIn("not a per-frame measurement", ratio["note"])
        self.assertIn("sampled lower bound", ratio["direction"])

    # ------------------------------------------------------------- value absent
    def test_absent_values_stay_not_measured_with_reasons(self) -> None:
        record = build(self.validate(windowless_document()))
        frames = record["frames"]["window_content_changes"]
        self.assertEqual(frames["status"], "NOT_MEASURED")
        self.assertEqual(frames["reason"], "WINDOW_NOT_FOUND")
        self.assertEqual(frames["changes"], 0)
        self.assertEqual(record["frames"]["compositor_desktop_wide"]["status"], "NOT_MEASURED")
        self.assertEqual(record["frames"]["compositor_desktop_wide"]["reason"],
                         "DWM_TIMING_UNAVAILABLE")
        self.assertEqual(record["gpu"]["status"], "NOT_MEASURED")
        self.assertEqual(record["gpu"]["reason"], "GPU_NO_PID_INSTANCES")
        self.assertEqual(record["per_frame_cpu"],
                         {"status": "NOT_MEASURED", "reason": "FRAMES_NOT_MEASURED"})
        self.assertFalse(record["app_reported_stand_in"])
        # An unmeasured endpoint carries no smuggled number.
        self.assertEqual(record["gpu"]["mean_percent_summed_over_engines"], 0)
        self.assertEqual(record["frames"]["compositor_desktop_wide"]["composed_delta"], 0)

    def test_zero_changes_is_not_measured_not_a_zero_ratio(self) -> None:
        document = probe_document(frames={
            "status": "OBSERVED", "reason": "",
            "method": "PrintWindow(PW_RENDERFULLCONTENT)", "samples": 400, "changes": 0,
            "uniform_captures": 0, "failed_captures": 0, "interval_ms": 20})
        record = build(self.validate(document))
        self.assertEqual(record["frames"]["window_content_changes"]["status"], "OBSERVED",
                         "an idle window is observed, it just changed zero times")
        self.assertEqual(record["per_frame_cpu"],
                         {"status": "NOT_MEASURED", "reason": "NO_FRAME_CHANGES"})

    def test_cpu_absent_explains_the_missing_ratio(self) -> None:
        document = probe_document(cpu={
            "status": "NOT_MEASURED", "reason": "PROCESS_ENDED_DURING_WINDOW", "cpu_ms": 0,
            "source": "GetProcessTimes", "attribution": "process_over_window"})
        record = build(self.validate(document))
        self.assertEqual(record["per_frame_cpu"],
                         {"status": "NOT_MEASURED", "reason": "CPU_NOT_MEASURED"})

    def test_zero_cpu_delta_never_yields_a_derived_zero_ratio(self) -> None:
        """C-1: 0 ms is the scheduler-clock floor, not a measured zero ratio."""
        document = probe_document(cpu={
            "status": "OBSERVED", "reason": "", "cpu_ms": 0.0,
            "source": "GetProcessTimes", "attribution": "process_over_window"})
        record = build(self.validate(document))
        self.assertEqual(record["frames"]["window_content_changes"]["changes"], 120,
                         "the denominator is present, so only the numerator blocks the ratio")
        self.assertEqual(record["per_frame_cpu"],
                         {"status": "NOT_MEASURED", "reason": "CPU_BELOW_CLOCK_RESOLUTION"})
        self.assertIn("CPU_BELOW_CLOCK_RESOLUTION", n06.PER_FRAME_NOT_MEASURED_REASONS)
        n06.validate_record(record)

        # The same refusal is enforced on comparison input, so a tampered DERIVED record with
        # a zero numerator cannot be replayed as evidence.
        tampered = copy.deepcopy(record)
        tampered["per_frame_cpu"] = {
            "status": "DERIVED", "kind": "ratio", "numerator_ms": 0.0, "denominator": 120,
            "denominator_unit": "sampled_window_content_changes",
            "value_ms_per_observed_change": 0.0, "window_ms": 10_000.0,
            "direction": "", "reason": "", "note": ""}
        with self.assertRaises(n06.AttributionError) as error:
            n06.validate_record(tampered)
        self.assertEqual(error.exception.code, "COMPARE_INPUT")

    def test_frame_sampler_caveat_discloses_subsampling(self) -> None:
        record = build(self.validate(probe_document()))
        caveat = record["frames"]["window_content_changes"]["caveat"]
        self.assertIn("stride", caveat)
        self.assertIn("blue channel", caveat)
        self.assertIn("un-sampled pixels are invisible", caveat)

    # ------------------------------------------------------------- closed codes
    def test_unregistered_reason_code_is_refused(self) -> None:
        document = probe_document(frames={
            "status": "NOT_MEASURED", "reason": "MADE_UP_REASON",
            "method": "PrintWindow(PW_RENDERFULLCONTENT)", "samples": 0, "changes": 0,
            "uniform_captures": 0, "failed_captures": 0, "interval_ms": 20})
        with self.assertRaises(n06.AttributionError) as error:
            self.validate(document)
        self.assertEqual(error.exception.code, "PROBE_SCHEMA")

    def test_values_are_bounded_and_cannot_ride_on_not_measured(self) -> None:
        # An unmeasured endpoint with a non-zero delta is a schema violation, not a value.
        document = probe_document()
        document["dwm"]["displayed_delta"] = 7
        with self.assertRaises(n06.AttributionError):
            self.validate(document)
        # A window larger than the cap is refused.
        document = probe_document()
        document["window"]["width"] = 100_000
        with self.assertRaises(n06.AttributionError):
            self.validate(document)
        # Too many samples for the declared cap.
        document = probe_document()
        document["frames"]["samples"] = CAPS["max_samples"] + 1
        with self.assertRaises(n06.AttributionError):
            self.validate(document)
        # A caps disagreement is never silently accepted.
        document = probe_document()
        document["caps"]["interval_ms"] = 50
        with self.assertRaises(n06.AttributionError):
            self.validate(document)
        # An observed GPU endpoint with no attributed instance is not an observation.
        document = probe_document()
        document["gpu"]["engine_instances"] = 0
        with self.assertRaises(n06.AttributionError):
            self.validate(document)

    # ------------------------------------------------------------- no stand-in
    def test_app_reported_value_cannot_reach_an_independent_field(self) -> None:
        document = probe_document()
        document["app_reported_changes"] = 999
        with self.assertRaises(n06.AttributionError) as error:
            self.validate(document)
        self.assertEqual(error.exception.code, "PROBE_SCHEMA")

        document = probe_document()
        document["frames"]["app_reported_changes"] = 999
        with self.assertRaises(n06.AttributionError) as error:
            self.validate(document)
        self.assertEqual(error.exception.code, "PROBE_SCHEMA")

        # And the record this tool writes never contains an app-reported number.
        record = build(self.validate(probe_document()))
        self.assertFalse(record["app_reported_stand_in"])
        serialised = json.dumps(record)
        self.assertNotIn("app_reported_changes", serialised)


class RecordAndCompareTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)

    def write(self, record: dict, name: str) -> Path:
        path = self.base / name
        n06.write_record(record, path)
        return path

    def test_record_roundtrip_is_closed(self) -> None:
        record = build(n06.validate_probe(probe_document(),
                                          {"pid": 4242, "caps": dict(CAPS)}))
        path = self.write(record, "a0.json")
        loaded = n06.load_record(path)
        self.assertEqual(loaded["record_kind"], "n06_independent_attribution")
        tampered = copy.deepcopy(loaded)
        tampered["extra"] = 1
        with self.assertRaises(n06.AttributionError) as error:
            n06.validate_record(tampered)
        self.assertEqual(error.exception.code, "PROBE_SCHEMA")

    def test_compare_reports_deltas_and_residuals(self) -> None:
        slow = build(n06.validate_probe(
            probe_document(cpu={"status": "OBSERVED", "reason": "", "cpu_ms": 400.0,
                                "source": "GetProcessTimes",
                                "attribution": "process_over_window"}),
            {"pid": 4242, "caps": dict(CAPS)}))
        fast = build(n06.validate_probe(
            probe_document(cpu={"status": "OBSERVED", "reason": "", "cpu_ms": 100.0,
                                "source": "GetProcessTimes",
                                "attribution": "process_over_window"}),
            {"pid": 4242, "caps": dict(CAPS)}))
        comparison = n06.compare_records(slow, fast)
        self.assertEqual(comparison["deltas"]["process_cpu_ms"]["a1_minus_a0"], -300.0)
        self.assertEqual(comparison["deltas"]["sampled_window_content_changes"]["a1_minus_a0"], 0)
        self.assertEqual(comparison["deltas"]["gpu_mean_percent_summed_over_engines"]
                         ["a1_minus_a0"], 0.0)
        ratio_delta = comparison["deltas"]["cpu_ms_per_observed_change"]
        self.assertEqual(ratio_delta["status"], "DERIVED")
        self.assertAlmostEqual(ratio_delta["a1_minus_a0"], round(100.0 / 120 - 400.0 / 120, 6))
        residual = {item["endpoint"]: item for item in comparison["residual_not_measured"]}
        self.assertIn("frames.app_displayed_frames", residual)
        self.assertIn("frames.compositor_desktop_wide", residual)
        self.assertNotIn("frames.window_content_changes", residual)
        self.assertNotIn("per_frame_cpu.derived_ratio", residual)
        self.assertFalse(comparison["app_reported_stand_in"])
        self.assertIn("not a controlled estimator", comparison["note"])
        a0_inputs = comparison["inputs"]["a0"]
        self.assertEqual(a0_inputs["executable_sha256"], "sha256:" + "1" * 64)
        self.assertEqual(a0_inputs["executable_identity_status"], "ASSERTED")
        self.assertIn("input_record_sha256", a0_inputs,
                      "the record digest must not be called `sha256` next to an executable")
        self.assertNotIn("sha256", a0_inputs)
        self.assertIn("caller-verified binary digest", comparison["input_naming"])
        self.assertIn("ASSERTED", comparison["input_naming"])
        self.assertIn("documented legacy status", comparison["input_naming"])

    def test_legacy_observed_executable_identity_still_replays(self) -> None:
        """C-4: retained pre-fix records carry OBSERVED; tolerate that one field only."""
        record = build(n06.validate_probe(probe_document(),
                                          {"pid": 4242, "caps": dict(CAPS)}))
        legacy = copy.deepcopy(record)
        legacy["target"]["executable"]["status"] = "OBSERVED"
        n06.validate_record(legacy)  # documented legacy tolerance, no record edit
        comparison = n06.compare_records(legacy, record)
        self.assertEqual(comparison["inputs"]["a0"]["executable_identity_status"], "OBSERVED")
        self.assertEqual(comparison["inputs"]["a1"]["executable_identity_status"], "ASSERTED")
        # The tolerance is only for that identity field: a malformed assertion still fails,
        # and every measurement check is unchanged.
        broken = copy.deepcopy(record)
        broken["target"]["executable"]["sha256"] = "sha256:not-hex"
        with self.assertRaises(n06.AttributionError) as error:
            n06.validate_record(broken)
        self.assertEqual(error.exception.code, "COMPARE_INPUT")

    def test_compare_records_caller_qualification(self) -> None:
        """C-5: a caller-declared run fact (capture disposition) is carried, not hidden."""
        record = build(n06.validate_probe(probe_document(),
                                          {"pid": 4242, "caps": dict(CAPS)}))
        text = ("A1 capture disposition CANCELLED / CLOSED_BEFORE_DECLARED_DURATION: the "
                "declared-input probe window sat inside the app lifetime; no settled-cost "
                "conclusion follows")
        qualified = n06.compare_records(record, record, qualifications=[text])
        plain = n06.compare_records(record, record)
        self.assertEqual(qualified["qualifications"], [text])
        self.assertEqual(qualified["deltas"], plain["deltas"],
                         "qualifications add a stated fact; they never change the numbers")
        self.assertEqual(qualified["note"], plain["note"])

    def test_qualification_caps_are_closed(self) -> None:
        record = build(n06.validate_probe(probe_document(),
                                          {"pid": 4242, "caps": dict(CAPS)}))
        for qualifications in (
                ["x" * (n06.MAX_QUALIFICATION_CHARS + 1)],
                ["line\x07break"],
                [str(index) for index in range(n06.MAX_QUALIFICATIONS + 1)],
        ):
            with self.assertRaises(n06.AttributionError) as error:
                n06.compare_records(record, record, qualifications=qualifications)
            self.assertEqual(error.exception.code, "ARGS_CAP")

    def test_compare_propagates_not_measured_without_a_number(self) -> None:
        measured = build(n06.validate_probe(probe_document(),
                                            {"pid": 4242, "caps": dict(CAPS)}))
        absent = build(n06.validate_probe(windowless_document(),
                                          {"pid": 4242, "caps": dict(CAPS)}))
        comparison = n06.compare_records(measured, absent)
        delta = comparison["deltas"]["sampled_window_content_changes"]
        self.assertEqual(delta["status"], "NOT_MEASURED")
        self.assertEqual(delta["reason"], "INPUT_NOT_MEASURED")
        self.assertEqual(delta["a1_reason"], "WINDOW_NOT_FOUND")
        self.assertNotIn("a1_minus_a0", delta)
        residual = {item["endpoint"]: item for item in comparison["residual_not_measured"]}
        self.assertIn("frames.window_content_changes", residual)
        self.assertIn("gpu.engine_utilization", residual)
        self.assertIn("per_frame_cpu.derived_ratio", residual)

    def test_compare_refuses_a_record_claiming_an_app_stand_in(self) -> None:
        record = build(n06.validate_probe(probe_document(),
                                         {"pid": 4242, "caps": dict(CAPS)}))
        record["app_reported_stand_in"] = True
        with self.assertRaises(n06.AttributionError) as error:
            n06.validate_record(record)
        self.assertEqual(error.exception.code, "COMPARE_INPUT")

    def test_oversized_record_is_refused(self) -> None:
        record = build(n06.validate_probe(probe_document(),
                                         {"pid": 4242, "caps": dict(CAPS)}))
        record["probe"]["path"] = "x" * n06.MAX_RECORD_BYTES
        with self.assertRaises(n06.AttributionError) as error:
            n06.write_record(record, self.base / "huge.json")
        self.assertEqual(error.exception.code, "RECORD_TOO_LARGE")
        self.assertFalse((self.base / "huge.json").exists())


class CliFailClosedTests(unittest.TestCase):
    """The real CLI is driven against a real fake PowerShell probe through `--probe`."""

    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)
        self.record = self.base / "record.json"

    def powershell(self) -> str:
        found = shutil.which("powershell")
        if found is None:
            self.skipTest("NOT_RUN: Windows PowerShell is not on PATH")
        return found

    def fake_probe(self, body: str, name: str = "fake_probe.ps1") -> Path:
        path = self.base / name
        path.write_text(PROBE_PARAMS + body, encoding="utf-8")
        return path

    def emit_document(self, document: dict, exit_code: int = 0) -> Path:
        payload = json.dumps(document)
        self.assertNotIn("'@", payload)
        return self.fake_probe(
            "$json = @'\n" + payload + "\n'@\nWrite-Output $json\nexit " + str(exit_code) + "\n")

    def run_cli(self, *arguments: str, timeout: float = 120) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-B", str(ROOT / "n06_attribution.py"), *arguments],
            capture_output=True, check=False, timeout=timeout)

    def run_attribute(self, probe: Path,
                      extra: tuple[str, ...] = ()) -> tuple[subprocess.CompletedProcess, Path]:
        result = self.run_cli("attribute", "--pid", "4242", "--out", str(self.record),
                              *extra, "--probe", str(probe))
        return result, self.record

    # ------------------------------------------------------------- value present
    def test_real_cli_records_a_present_value_from_the_fake_probe(self) -> None:
        self.powershell()
        probe = self.emit_document(probe_document())
        result, record_path = self.run_attribute(
            probe, ("--executable-sha256", "sha256:" + "a" * 64))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertNotIn(b"Traceback", result.stderr)
        summary = json.loads(result.stdout)
        self.assertTrue(summary["ok"])
        record = json.loads(record_path.read_text(encoding="utf-8"))
        self.assertEqual(record["frames"]["window_content_changes"]["changes"], 120)
        self.assertEqual(record["per_frame_cpu"]["status"], "DERIVED")
        self.assertEqual(record["frames"]["app_displayed_frames"]["status"], "NOT_MEASURED")
        self.assertFalse(record["app_reported_stand_in"])
        self.assertEqual(record["target"]["executable"]["sha256"], "sha256:" + "a" * 64)
        self.assertEqual(record["target"]["executable"]["status"], "ASSERTED")

    # ------------------------------------------------------------- failure modes
    def test_nonzero_exit_after_valid_json_is_refused(self) -> None:
        self.powershell()
        probe = self.emit_document(probe_document(), exit_code=3)
        result, record_path = self.run_attribute(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_EXIT")
        self.assertNotIn(b"Traceback", result.stderr)
        self.assertFalse(record_path.exists(), "valid JSON after a non-zero exit is not evidence")

    def test_truncated_output_is_refused(self) -> None:
        self.powershell()
        probe = self.fake_probe("Write-Output '{\"ok\":true,\"probe_version\":1'\nexit 0\n")
        result, record_path = self.run_attribute(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_JSON")
        self.assertFalse(record_path.exists())

    def test_oversized_output_is_refused(self) -> None:
        self.powershell()
        probe = self.fake_probe("Write-Output ('x' * 1100000)\nexit 0\n")
        result, record_path = self.run_attribute(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_OVERSIZE")
        self.assertFalse(record_path.exists())

    def test_unregistered_probe_error_code_is_closed(self) -> None:
        self.powershell()
        probe = self.fake_probe("Write-Output '{\"ok\":false,\"error\":\"NOPE\"}'\nexit 0\n")
        result, record_path = self.run_attribute(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_FAILED")
        self.assertFalse(record_path.exists())

    def test_registered_probe_error_passes_through(self) -> None:
        self.powershell()
        probe = self.fake_probe(
            "Write-Output '{\"ok\":false,\"error\":\"PROCESS_NOT_FOUND\"}'\nexit 0\n")
        result, record_path = self.run_attribute(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(json.loads(result.stdout)["error"], "PROCESS_NOT_FOUND")
        self.assertFalse(record_path.exists())

    def test_unregistered_reason_code_from_the_probe_is_refused(self) -> None:
        self.powershell()
        document = windowless_document()
        document["frames"]["reason"] = "MADE_UP_REASON"
        probe = self.emit_document(document)
        result, record_path = self.run_attribute(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_SCHEMA")
        self.assertFalse(record_path.exists())

    def test_extra_app_reported_field_is_refused(self) -> None:
        self.powershell()
        document = probe_document()
        document["app_reported_frames"] = 999
        probe = self.emit_document(document)
        result, record_path = self.run_attribute(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_SCHEMA")
        self.assertFalse(record_path.exists())

    # ------------------------------------------------------------- caps and seams
    def test_argument_caps_are_closed(self) -> None:
        cases = (
            ("attribute", "--pid", "0"),
            ("attribute", "--pid", "4242", "--seconds", "0"),
            ("attribute", "--pid", "4242", "--seconds", "121"),
            ("attribute", "--pid", "4242", "--interval-ms", "5"),
            ("attribute", "--pid", "4242", "--max-samples", "5"),
            ("attribute", "--pid", "4242", "--max-window-pixels", "100"),
            # 1000 requested samples cannot fit a cap of 10.
            ("attribute", "--pid", "4242", "--seconds", "10", "--interval-ms", "10",
             "--max-samples", "10"),
            # An executable identity is a full sha256 or it is refused.
            ("attribute", "--pid", "4242", "--executable-sha256", "deadbeef"),
        )
        expected = ("ARGS_PID", "ARGS_CAP", "ARGS_CAP", "ARGS_CAP", "ARGS_CAP", "ARGS_CAP",
                    "ARGS_CAP", "ARGS_IDENTITY")
        for arguments, code in zip(cases, expected):
            result = self.run_cli(*arguments)
            self.assertEqual(result.returncode, 1, (arguments, result.stdout))
            self.assertEqual(json.loads(result.stdout)["error"], code, arguments)
            self.assertNotIn(b"Traceback", result.stderr)

    def test_missing_shell_or_probe_is_closed(self) -> None:
        result = self.run_cli("attribute", "--pid", "4242",
                              "--powershell", str(self.base / "no-such-shell.exe"))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_MISSING")
        result = self.run_cli("attribute", "--pid", "4242",
                              "--probe", str(self.base / "no-such-probe.ps1"))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_MISSING")
        self.assertNotIn(b"Traceback", result.stderr)

    def test_probe_timeout_is_bounded(self) -> None:
        powershell = self.powershell()
        sleeper = self.fake_probe("Start-Sleep -Seconds 10\nexit 0\n", name="slow.ps1")
        started = time.monotonic()
        with self.assertRaises(n06.AttributionError) as error:
            n06.run_probe([powershell, "-NoProfile", "-NonInteractive",
                           "-ExecutionPolicy", "Bypass", "-File", str(sleeper)],
                          timeout_s=1.5)
        self.assertEqual(error.exception.code, "PROBE_TIMEOUT")
        self.assertLess(time.monotonic() - started, 8.0)


class LegacyCpuSpawnTests(unittest.TestCase):
    """C-3: the legacy `cpu` path must spawn an absolute interpreter, never a PATH shim."""

    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)
        self.shim = self.base / "shim"
        self.shim.mkdir()
        (self.shim / "powershell.bat").write_text(
            "@echo off\r\necho SHIM-USED > \""
            + str(self.base / "shim-used.txt") + "\"\r\nexit 0\r\n",
            encoding="utf-8")
        self.original_path = os.environ.get("PATH", "")
        self.addCleanup(self.restore_path)

    def restore_path(self) -> None:
        os.environ["PATH"] = self.original_path

    def test_shim_ahead_on_path_is_never_the_interpreter(self) -> None:
        if os.name != "nt":
            self.skipTest("NOT_RUN: absolute interpreter resolution is Windows-specific")
        if shutil.which("powershell.exe") is None:
            self.skipTest("NOT_RUN: no powershell.exe for an absolute-path check")
        os.environ["PATH"] = str(self.shim) + os.pathsep + self.original_path
        resolved = n06._resolve_powershell("powershell")
        self.assertTrue(Path(resolved).is_absolute())
        self.assertEqual(Path(resolved).suffix.lower(), ".exe")
        self.assertNotEqual(Path(resolved).parent.resolve(), self.shim.resolve())

        captured = {}
        original_run = n06.subprocess.run

        class Done:
            returncode = 0
            stdout = b'{"cpu_ms":1.0,"window_ms":1000.0}'

        def fake_run(command, **kwargs):
            captured["command"] = command
            return Done()

        try:
            n06.subprocess.run = fake_run
            result = n06.sample_process_cpu(4242, 1)
        finally:
            n06.subprocess.run = original_run
        self.assertEqual(captured["command"][0], resolved,
                         "the legacy cpu path must spawn the resolved absolute image")
        self.assertTrue(Path(captured["command"][0]).is_absolute())
        self.assertNotEqual(captured["command"][0], "powershell")
        self.assertEqual(result["status"], "OBSERVED")
        self.assertFalse((self.base / "shim-used.txt").exists(), "the shim must never run")

    def test_shim_only_path_is_a_closed_missing_interpreter(self) -> None:
        if os.name != "nt":
            self.skipTest("NOT_RUN: absolute interpreter resolution is Windows-specific")
        os.environ["PATH"] = str(self.shim)
        with self.assertRaises(n06.AttributionError) as error:
            n06.sample_process_cpu(4242, 1)
        self.assertEqual(error.exception.code, "CPU_PROBE_MISSING")


class RealHostProbeTests(unittest.TestCase):
    """One real probe run: the packaged script on this OS, no fake document."""

    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)

    def test_real_probe_degrades_for_a_live_windowless_process(self) -> None:
        if shutil.which("powershell") is None:
            self.skipTest("NOT_RUN: Windows PowerShell is not on PATH")
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        self.addCleanup(child.kill)
        record_path = self.base / "record.json"
        result = subprocess.run(
            [sys.executable, "-B", str(ROOT / "n06_attribution.py"), "attribute",
             "--pid", str(child.pid), "--seconds", "1", "--interval-ms", "50",
             "--max-samples", "100", "--out", str(record_path)],
            capture_output=True, check=False, timeout=180)
        if result.returncode == 1:
            code = json.loads(result.stdout).get("error")
            if code == "PROBE_UNAVAILABLE":
                self.skipTest("NOT_RUN: this host cannot compile the probe's Win32 layer")
            self.fail("the packaged probe failed closed: " + result.stdout.decode())
        record = json.loads(record_path.read_text(encoding="utf-8"))
        self.assertFalse(record["app_reported_stand_in"])
        frames = record["frames"]["window_content_changes"]
        self.assertEqual(frames["status"], "NOT_MEASURED")
        self.assertEqual(frames["reason"], "WINDOW_NOT_FOUND",
                         "a live process without a visible window has no frame endpoint")
        self.assertIn(record["per_frame_cpu"],
                      ({"status": "NOT_MEASURED", "reason": "FRAMES_NOT_MEASURED"},
                       {"status": "NOT_MEASURED", "reason": "CPU_NOT_MEASURED"}))
        self.assertEqual(record["frames"]["app_displayed_frames"]["status"], "NOT_MEASURED")
        self.assertEqual(record["frames"]["app_displayed_frames"]["reason"],
                         "NO_PER_WINDOW_DISPLAY_COUNTER")
        if record["frames"]["compositor_desktop_wide"]["status"] == "NOT_MEASURED":
            self.assertEqual(record["frames"]["compositor_desktop_wide"]["reason"],
                             "DWM_TIMING_UNAVAILABLE")
        if record["gpu"]["status"] == "NOT_MEASURED":
            self.assertIn(record["gpu"]["reason"],
                          ("GPU_COUNTER_UNAVAILABLE", "GPU_NO_SAMPLES", "GPU_NO_PID_INSTANCES"))


if __name__ == "__main__":
    unittest.main()
