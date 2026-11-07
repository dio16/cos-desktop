"""REC2 evidence boundaries and correlated gauge behavior."""
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from check_cache_lifetime import analyze_campaign, expected_action, gauge_trend


class ResourceTrendTests(unittest.TestCase):
    def test_growth_after_an_initial_flat_span_is_not_a_plateau(self):
        result = gauge_trend([(0.0, 100), (1.0, 100), (2.0, 200), (3.0, 300)])
        self.assertTrue(result["nondecreasing_growth"])
        self.assertFalse(result["strictly_increasing"])
        self.assertEqual(result["last_minus_first"], 200)
        self.assertEqual(result["least_squares_bytes_per_second"], 70.0)
        self.assertEqual(result["final_third_sampled_range"], 100)

    def test_release_is_kept_in_the_sample_distribution(self):
        result = gauge_trend([(0.0, 400), (1.0, 300), (2.0, 100), (3.0, 100)])
        self.assertFalse(result["nondecreasing_growth"])
        self.assertEqual(result["last_minus_first"], -300)
        self.assertEqual(result["p50"], 200.0)
        self.assertEqual(result["max"], 400)
        self.assertIsNone(result["p99"])

    def test_constant_retention_reports_gauge_range_not_release(self):
        result = gauge_trend([(0.0, 100), (2.0, 100), (4.0, 100)])
        self.assertEqual(result["final_third_sampled_range"], 0)
        self.assertEqual(result["least_squares_bytes_per_second"], 0)
        self.assertEqual(result["last"], 100)
        self.assertFalse(result["nondecreasing_growth"])
        self.assertIsNone(result["p95"])

    def test_missing_gpu_samples_are_not_zero_usage(self):
        self.assertEqual(gauge_trend([])["status"], "NOT_MEASURED")
        self.assertNotIn("max", gauge_trend([]))

    def test_one_sample_has_no_growth_slope(self):
        result = gauge_trend([(3.0, 200)])
        self.assertIsNone(result["least_squares_bytes_per_second"])
        self.assertFalse(result["strictly_increasing"])

    def test_duplicate_or_reversed_clocks_and_negative_gauges_are_rejected(self):
        for samples in ([(1.0, 1), (1.0, 2)], [(2.0, 1), (1.0, 2)], [(0.0, -1)], [(float("nan"), 1)]):
            with self.subTest(samples=samples), self.assertRaises(ValueError):
                gauge_trend(samples)


class CampaignEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.directory = Path(self.temp.name)
        self.manifest = {
            "manifest_version": 1, "task": "MP-REC2", "contract": "DC-MP-REC2",
            "variant": "shared", "recovery": False, "tracked_dirty": False,
            "tracked_dirty_after": False, "source_commit": "a" * 40,
            "source_commit_after": "a" * 40, "executable_sha256": "b" * 64,
            "executable_sha256_after": "b" * 64, "driver_sha256": "c" * 64,
            "driver_sha256_after": "c" * 64, "campaign_status": "OBSERVED",
            "integrity_failures": [],
            "qpc_frequency": 1_000_000,
            "endpoints": {"atlas_allocations": {"status": "NOT_MEASURED"}}, "runs": [],
        }
        self.native_samples = [self.sample(0, 500_000, "pre_stress", 0, "baseline", "observed")]
        self.native_samples.extend(
            self.sample(i + 1, (i + 1) * 1_000_000, "stress", i, expected_action(i, False),
                        "scene_complete" if i == 0 else (
                            "requested" if i % 12 in (3, 4, 5, 6, 7) else "pending_or_unavailable"))
            for i in range(60)
        )
        self.native_samples.append(
            self.sample(61, 61_000_000, "settle", 60, "observe_settle", "observed"))
        self.native_samples.append(
            self.sample(62, 62_000_000, "peer_closed", 60, "close_other_window", "observed"))
        final = self.sample(63, 63_000_000, "post_close_teardown", 60, "close_and_evict", "observed")
        final["viewer_open"] = False
        final["compare_open"] = False
        final["views"] = [{**view, "complete_in_scene": False} for view in self.observed_views()]
        self.native_samples.append(final)
        self._write_campaign()

    def sample(self, order, elapsed_us, phase, step, action, result):
        return {"elapsed_us": elapsed_us, "phase": phase, "step": step,
                "action": action, "result": result, "windows": 1,
                "viewer_open": phase != "post_close_teardown",
                "compare_open": phase != "post_close_teardown",
                "device_generation": 1, "device_state": "ready",
                "counters": self.counters(), "views": self.observed_views(),
                "identities": [None] * 6}

    @staticmethod
    def observed_views():
        return [{"scene_serial": 0, "view_serial": 0, "phase": 2,
                 "complete_in_scene": True, "current_origin": None,
                 "retained_origin": None, "retiring_frames": 0}
                for _ in range(3)]

    def tearDown(self):
        self.temp.cleanup()

    @staticmethod
    def counters():
        return {"retained_frames": 0, "backup_claims": 0, "backup_origins": 0,
                "pool_count": 0, "staging_reservation_bytes": 0,
                "in_flight_reservation_bytes": 0, "resident_reservation_bytes": 0,
                "quarantined_payload_bytes": 0, "quarantine_capacity_bytes": 64 * 1024 * 1024,
                "quarantined_frames": 0,
                "consumers": 0, "consumer_unknown_frames": 0,
                "decoded_retained_bytes": 0, "packed_retained_bytes": 0, "cpu_retained_bytes": 0}

    def _write_campaign(self):
        self.manifest["runs"] = []
        for n in range(5):
            run_dir = self.directory / f"run-{n + 1}"
            run_dir.mkdir(exist_ok=True)
            native = {"schema_version": 1, "task": "MP-REC2", "contract": "DC-MP-REC2",
                      "pid": 1000 + n, "run_start_unix_ms": 100_000, "variant": "shared",
                      "recovery": False, "stress_start_elapsed_us": 0,
                      "stress_end_elapsed_us": 60_000_000, "samples": self.native_samples}
            stdout = "REC2_RESULT " + json.dumps(native) + "\n"
            stderr = ""
            (run_dir / "native.stdout").write_bytes(stdout.encode("utf-8"))
            (run_dir / "native.stderr").write_bytes(stderr.encode("utf-8"))
            process_samples = [{"pid": 1000 + n, "start_utc_ticks": 10000 + n,
                                "alive_after": True, "identity_after_ticks": 10000 + n,
                                "executable_path": "C:/dev/cos-desktop/target/release/examples/compare.exe",
                                "qpc_start": i * 1_000_000, "qpc_end": i * 1_000_000 + 1,
                                "private_bytes": 100, "working_set_bytes": 200,
                                "gpu": {"status": "NOT_MEASURED"}}
                               for i in range(1, 61)]
            record = {"status": "OBSERVED", "timed_out": False, "exit_code": 0,
                      "process_started": True, "native_stdout_truncated": False,
                      "native_stderr_truncated": False,
                      "observed_executable_path": "C:/dev/cos-desktop/target/release/examples/compare.exe",
                      "observed_path_matches_requested": True,
                      "variant": "shared", "pid": 1000 + n, "start_utc_ticks": 10000 + n,
                      "pair_id": "p", "scenario_id": "s", "launch_utc": "1970-01-01T00:01:40+00:00",
                      "launch_qpc": 0, "qpc_frequency": 1_000_000, "samples": process_samples,
                      "native_stdout_sha256": hashlib.sha256(stdout.encode()).hexdigest(),
                      "native_stderr_sha256": hashlib.sha256(stderr.encode()).hexdigest()}
            obs = f"observations-{n}.json"
            (run_dir / obs).write_text(json.dumps(record))
            self.manifest["runs"].append({"run_id": f"run-{n}", "observations": f"run-{n + 1}/{obs}",
                                           "stdout": f"run-{n + 1}/native.stdout",
                                           "stderr": f"run-{n + 1}/native.stderr"})
        (self.directory / "manifest.json").write_text(json.dumps(self.manifest))

    def test_valid_campaign_separates_dispatch_from_scene_and_physical_completion(self):
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "OBSERVED")
        self.assertEqual(result["acceptance"], "HOLD")
        run = result["runs"][0]
        self.assertEqual(run["actions"]["load_same_pair"]["scene_complete_observations"], 1)
        self.assertEqual(run["actions"]["load_same_pair"]["pending_observations"], 4)
        self.assertEqual(run["actions"]["swap"]["dispatch_only_observations"], 5)
        self.assertEqual(run["actions"]["viewer_reload"]["pending_observations"], 5)
        self.assertTrue(run["post_close_reconciliation"]["consumer_release_observed"])
        self.assertEqual(run["post_close_reconciliation"]["physical_atlas_release"], "NOT_MEASURED")

    def test_a_ledger_without_the_declared_workload_is_rejected(self):
        for sample in self.native_samples[1:61]:
            sample["action"] = "load_same_pair"
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "FAILED")
        self.assertTrue(any("instead of the declared" in failure
                            for failure in result["runs"][0]["failures"]))

    def test_misnumbered_or_duplicated_steps_are_rejected(self):
        self.native_samples[11]["step"] = 9
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "FAILED")
        self.assertTrue(any("misnumbered" in failure for failure in result["runs"][0]["failures"]))

    def test_a_recovery_campaign_must_observe_failure_and_recovery(self):
        self.manifest["recovery"] = True
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "FAILED")
        self.assertTrue(any("instead of the declared 'recovery_fail'" in failure
                            for failure in result["runs"][0]["failures"]))

    def test_two_window_workload_must_observe_and_withdraw_the_peer(self):
        self.manifest["variant"] = "windows2"
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "FAILED")
        self.assertTrue(any("both windows" in failure for failure in result["runs"][0]["failures"]))
        self._write_campaign()
        for sample in self.native_samples[1:61]:
            sample["windows"] = 2
        self.native_samples[-1]["windows"] = 2
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertTrue(any("peer window was not withdrawn" in failure
                            for failure in result["runs"][0]["failures"]))

    def test_recorded_gpu_total_must_recompute_from_its_instances(self):
        record_path = self.directory / "run-1/observations-0.json"
        record = json.loads(record_path.read_text())
        record["samples"][0]["gpu"] = {
            "status": "OBSERVED", "dedicated_bytes": 10, "shared_bytes": 5, "committed_bytes": 15,
            "instances": [
                {"counter": "dedicated usage", "status": 0, "bytes": 4},
                {"counter": "shared usage", "status": 0, "bytes": 5},
                {"counter": "total committed", "status": 0, "bytes": 9},
            ],
        }
        record_path.write_text(json.dumps(record))
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "FAILED")
        self.assertTrue(any("does not match its own instances" in failure
                            for failure in result["runs"][0]["failures"]))

    def test_empty_campaign_and_native_samples_fail_closed(self):
        self.manifest["runs"] = []
        (self.directory / "manifest.json").write_text(json.dumps(self.manifest))
        self.assertEqual(analyze_campaign(self.directory)["status"], "FAILED")
        self._write_campaign()
        self.native_samples = []
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "FAILED")
        self.assertTrue(any("empty" in failure for failure in result["runs"][0]["failures"]))

    def test_schema_errors_and_undersampled_stress_do_not_pass(self):
        self.native_samples = self.native_samples[:59] + [
            {"elapsed_us": 60_000_000, "phase": "settle", "step": 60,
             "action": "settle", "result": "observed", "windows": 1,
             "viewer_open": False, "compare_open": False, "device_generation": 1,
             "device_state": "ready", "counters": self.counters(),
             "views": self.observed_views(), "identities": [None] * 6},
            self.native_samples[-1],
        ]
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "FAILED")
        self.assertTrue(any("Fewer than 60" in failure for failure in result["runs"][0]["failures"]))
        self._write_campaign()
        output = self.directory / "run-1/native.stdout"
        native_line = json.loads(output.read_text().removeprefix("REC2_RESULT "))
        native_line["schema_version"] = 99
        output.write_text("REC2_RESULT " + json.dumps(native_line) + "\n")
        self._refresh_digest(0)
        self.assertEqual(analyze_campaign(self.directory)["runs"][0]["status"], "FAILED")

    def test_reused_process_lifetime_is_rejected(self):
        self._write_campaign()
        self._refresh_record_identity(1, 1000, 10000)
        result = analyze_campaign(self.directory)
        self.assertTrue(any("reused" in failure for failure in result["runs"][1]["failures"]))

    def test_unknown_consumers_and_missing_final_counts_are_never_release(self):
        self.native_samples[-1]["counters"]["consumer_unknown_frames"] = 1
        self._write_campaign()
        run = analyze_campaign(self.directory)["runs"][0]
        self.assertFalse(run["post_close_reconciliation"]["consumer_release_observed"])
        del self.native_samples[-1]["counters"]["backup_claims"]
        self._write_campaign()
        run = analyze_campaign(self.directory)["runs"][0]
        self.assertFalse(run["post_close_reconciliation"]["consumer_release_observed"])

    def test_incomplete_view_and_identity_ledgers_fail_closed(self):
        self.native_samples[-1]["views"] = self.native_samples[-1]["views"][:1]
        self.native_samples[-1]["identities"][0] = "not-a-sha256"
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["runs"][0]["status"], "FAILED")
        self.assertFalse(result["runs"][0]["post_close_reconciliation"]["consumer_release_observed"])

    def test_unavailable_final_device_is_not_a_release_observation(self):
        self.native_samples[-1]["device_state"] = "unavailable"
        self._write_campaign()
        reconciliation = analyze_campaign(self.directory)["runs"][0]["post_close_reconciliation"]
        self.assertFalse(reconciliation["consumer_release_observed"])
        self.assertEqual(reconciliation["final_device_state"], "unavailable")
        self.assertEqual(reconciliation["status"], "HOLD")
        self.assertEqual(reconciliation["gpu_completion"], "NOT_MEASURED")

    def test_capture_truncation_and_observer_changes_are_not_intact_evidence(self):
        record_path = self.directory / "run-1/observations-0.json"
        record = json.loads(record_path.read_text())
        record["native_stderr_truncated"] = True
        record_path.write_text(json.dumps(record))
        self.assertEqual(analyze_campaign(self.directory)["status"], "FAILED")
        self._write_campaign()
        self.manifest["driver_sha256_after"] = "d" * 64
        (self.directory / "manifest.json").write_text(json.dumps(self.manifest))
        self.assertEqual(analyze_campaign(self.directory)["status"], "FAILED")

    def test_sampled_executable_path_must_bind_to_the_record(self):
        record_path = self.directory / "run-1/observations-0.json"
        record = json.loads(record_path.read_text())
        record["observed_executable_path"] = "C:/elsewhere/compare.exe"
        record_path.write_text(json.dumps(record))
        result = analyze_campaign(self.directory)
        self.assertEqual(result["runs"][0]["status"], "FAILED")
        self.assertTrue(any("does not match its samples" in failure for failure in result["runs"][0]["failures"]))
        self._write_campaign()
        record = json.loads(record_path.read_text())
        record["observed_executable_path"] = None
        for sample in record["samples"]:
            sample["executable_path"] = None
        record_path.write_text(json.dumps(record))
        result = analyze_campaign(self.directory)
        self.assertTrue(any("readable executable path" in failure for failure in result["runs"][0]["failures"]))

    def test_final_consumer_withdrawal_does_not_erase_unproven_reservations(self):
        final = self.native_samples[-1]["counters"]
        final.update(pool_count=1, quarantined_frames=1, quarantined_payload_bytes=1024,
                     decoded_retained_bytes=512, packed_retained_bytes=512, cpu_retained_bytes=1024)
        self._write_campaign()
        result = analyze_campaign(self.directory)
        self.assertEqual(result["status"], "OBSERVED")
        reconciliation = result["runs"][0]["post_close_reconciliation"]
        self.assertTrue(reconciliation["consumer_release_observed"])
        self.assertFalse(reconciliation["application_resources_empty"])
        self.assertEqual(reconciliation["quarantined_payload_bytes"], 1024)
        self.assertEqual(reconciliation["cpu_retained_bytes"], 1024)
        self.assertEqual(result["acceptance"], "HOLD")

    def _refresh_digest(self, n):
        run_dir = self.directory / f"run-{n + 1}"
        record_path = run_dir / f"observations-{n}.json"
        record = json.loads(record_path.read_text())
        record["native_stdout_sha256"] = hashlib.sha256((run_dir / "native.stdout").read_bytes()).hexdigest()
        record_path.write_text(json.dumps(record))

    def _refresh_record_identity(self, n, pid, ticks):
        run_dir = self.directory / f"run-{n + 1}"
        record_path = run_dir / f"observations-{n}.json"
        record = json.loads(record_path.read_text())
        record.update(pid=pid, start_utc_ticks=ticks)
        for sample in record["samples"]:
            sample.update(pid=pid, start_utc_ticks=ticks, identity_after_ticks=ticks)
        record_path.write_text(json.dumps(record))
        output = run_dir / "native.stdout"
        native = json.loads(output.read_text().removeprefix("REC2_RESULT "))
        native["pid"] = pid
        output.write_text("REC2_RESULT " + json.dumps(native) + "\n")
        record["native_stdout_sha256"] = hashlib.sha256(output.read_bytes()).hexdigest()
        record_path.write_text(json.dumps(record))


if __name__ == "__main__":
    unittest.main()
