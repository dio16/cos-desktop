"""Independent wire examples: never evidence that GPUI or hardware was observed."""
from __future__ import annotations

import copy
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parents[3]
sys.path.insert(0, str(ROOT))
import s2b_contracts as s2b  # noqa: E402

FIXTURE = ROOT / "fixtures/s2b-wire-v1.json"


def example() -> dict:
    return json.loads(FIXTURE.read_bytes())


def invoke(data: bytes) -> subprocess.CompletedProcess:
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "capture.json"
        path.write_bytes(data)
        return subprocess.run(
            [sys.executable, "-B", str(ROOT / "s2b_validate.py"), "capture", "--capture", str(path)],
            capture_output=True, check=False, timeout=10,
        )


class S2BWireContract(unittest.TestCase):
    def reject(self, value: dict, code: str) -> None:
        with self.assertRaises(s2b.ObservationError) as error:
            s2b.validate_capture(value)
        self.assertEqual(str(error.exception), code)

    def test_nested_input_coalescing_and_explicit_draw_pair_are_not_one_to_one(self):
        value = s2b.validate_capture(example())
        self.assertEqual(value["summary"]["kind_counts"][2:4], [2, 1])
        self.assertEqual(value["records"][1]["invalidations"], 3)
        self.assertEqual(value["records"][3]["window"], None)
        self.assertEqual(value["records"][7]["lifetime"], 1)  # Local queue delivered before-close event later.
        outcome = invoke(FIXTURE.read_bytes())
        self.assertEqual(outcome.returncode, 0, outcome.stderr)
        result = json.loads(outcome.stdout)
        self.assertEqual(result["scope"], "WIRE_CONTRACT_ONLY")
        self.assertIs(result["native_proven"], False)
        self.assertIs(result["full_coverage"], False)
        self.assertEqual(result["endpoints"]["gpui-platform-submission"]["status"], "OBSERVED")
        for name in ("present-call", "present-return", "present", "displayed", "scanout", "input-to-photon", "first-frame"):
            self.assertEqual(result["endpoints"][name]["status"], "NOT_MEASURED")

    def test_wrong_domain_preorigin_reversed_overflow_and_unknown_accuracy_are_rejected(self):
        mutations = [
            (lambda v: v["clock"].update(domain="wall-clock"), "CLOCK_DOMAIN"),
            (lambda v: v["clock"].update(mapping="QPC-to-ETW"), "CLOCK_DOMAIN"),
            (lambda v: v["clock"].update(resolution=1), "CLOCK_DOMAIN"),
            (lambda v: v["clock"].update(min_positive_delta_ns=0), "INTEGER"),
            (lambda v: v["records"][1].update(start_ns=-1), "INTEGER"),
            (lambda v: v["records"][1].update(start_ns=4), "TIMESTAMP"),
            (lambda v: v["records"][1].update(end_ns=11), "TIMESTAMP"),
            (lambda v: v["records"][1].update(end_ns=1 << 64), "INTEGER"),
            (lambda v: v["records"][1].update(start_ns=True), "INTEGER"),
            (lambda v: v["records"][1].update(dirty_ns=13), "DIRTY_ORDER"),
            (lambda v: v.update(scope_end_ns=30_000_000_006), "CAPTURE_SCOPE"),
        ]
        for mutate, code in mutations:
            with self.subTest(code=code, mutation=mutate):
                value = example()
                mutate(value)
                self.reject(value, code)

    def test_source_association_cannot_invent_a_draw_or_input_window(self):
        for change in ({"paired_draw_end_ns": 23}, {"paired_draw_start_ns": None}, {"invalidations": None}):
            value = example()
            value["records"][2].update(change)
            self.reject(value, "PAIR_ORDER" if "paired_draw_end_ns" in change else "PAIR")
        value = example()
        value["records"][3].update(window=10, lifetime=1)
        self.reject(value, "RECORD_SEMANTICS")

    def test_window_lifetime_reuse_and_events_after_close_are_rejected(self):
        value = example()
        value["records"][5]["lifetime"] = 1
        self.reject(value, "WINDOW_LIFETIME")
        value = example()
        value["records"][8].update(start_ns=41, end_ns=41)
        self.reject(value, "CLOSED_WINDOW")
        value = example()
        value["records"][6]["lifetime"] = 3
        self.reject(value, "WINDOW_LIFETIME")

    def test_stale_execution_pid_or_executable_is_rejected(self):
        cases = (("b" * 32, None, None), (None, 43, None), (None, None, "sha256:" + "b" * 64))
        for execution, pid, executable in cases:
            with self.subTest(execution=execution, pid=pid), self.assertRaises(s2b.ObservationError) as error:
                s2b.validate_capture(example(), execution=execution, pid=pid, executable=executable)
            self.assertEqual(str(error.exception), "IDENTITY_MISMATCH")

    def test_source_loss_scope_and_unfinalized_tail_cannot_be_claimed_complete(self):
        for change in ({"source_coverage": "ALL_NATIVE_MESSAGES"}, {"unoffered_loss": 0}, {"unfinalized_tail": 0}):
            value = example()
            value["summary"].update(change)
            self.reject(value, "COVERAGE")

    def test_retention_queue_and_observation_counts_cannot_hide_loss(self):
        for change in ({"retained": 8}, {"eligible": 10}, {"retention_dropped": 1},
                       {"local_undrained": 1}, {"local_enqueued": 3}, {"input_unassociated": 0},
                       {"kind_counts": [1, 2, 3, 1, 1, 0, 0, 0, 1, 1]}):
            value = example()
            value["summary"].update(change)
            self.reject(value, "COUNTS")
        value = example()
        value["summary"]["retained"] = True
        self.reject(value, "INTEGER")

    def test_duplicate_draw_and_sequence_do_not_manufacture_samples(self):
        value = example()
        value["records"][6] = copy.deepcopy(value["records"][1])
        value["records"][6]["seq"] = 7
        self.reject(value, "DUPLICATE_EVENT")
        value = example()
        value["records"][1]["seq"] = 1
        self.reject(value, "SEQUENCE")

    def test_missing_private_unknown_and_unsupported_physical_fields_fail_closed(self):
        for mutate in (
            lambda v: v.pop("summary"),
            lambda v: v.update(private_path="SECRET-PRIVATE-PATH"),
            lambda v: v["records"][0].update(task_id="SECRET-TASK"),
            lambda v: v.update(endpoints={"displayed": "OBSERVED"}),
        ):
            value = example()
            mutate(value)
            outcome = invoke(json.dumps(value).encode())
            self.assertEqual(outcome.returncode, 1)
            self.assertEqual(json.loads(outcome.stdout)["error"], "SCHEMA")
            self.assertNotIn(b"SECRET", outcome.stdout + outcome.stderr)

    def test_bounded_utf8_json_duplicate_nonfinite_and_partial_input_fail_closed(self):
        cases = [
            (b"\xff", "INVALID_UTF8"),
            (b'{"capture_version":1,"capture_version":1}', "DUPLICATE_KEY"),
            (b'{"x":NaN}', "NON_FINITE_NUMBER"),
            (b'{"x":Infinity}', "NON_FINITE_NUMBER"),
            (b'{"capture_version":', "MALFORMED_JSON"),
            (b" " * (1024 * 1024 + 1), "TOO_LARGE"),
        ]
        for raw, code in cases:
            with self.subTest(code=code):
                outcome = invoke(raw)
                self.assertEqual(outcome.returncode, 1)
                self.assertEqual(json.loads(outcome.stdout)["error"], code)

    def test_record_and_window_capacity_are_enforced(self):
        value = example()
        value["scope_end_ns"] = 300
        value["records"] = [
            {**value["records"][0], "seq": i + 1, "start_ns": i + 5, "end_ns": i + 5}
            for i in range(257)
        ]
        self.reject(value, "RECORD_LIMIT")
        value = example()
        value["records"][0]["lifetime"] = 17
        self.reject(value, "INTEGER")

    def test_no_supported_observation_and_failed_run_cannot_be_success(self):
        value = s2b.validate_capture(example())
        self.assertEqual(s2b.disposition(value, {"timed_out": False, "exit_code": 0}), "OBSERVED")
        for ledger in ({"timed_out": True, "exit_code": 1}, {"timed_out": False, "exit_code": 2}):
            self.assertEqual(s2b.disposition(value, ledger), "FAILED")
        value["records"] = []
        for key, count in value["summary"].items():
            if type(count) is int:
                value["summary"][key] = 0
        value["summary"]["kind_counts"] = [0] * 10
        value = s2b.validate_capture(value)
        self.assertEqual(s2b.disposition(value, {"timed_out": False, "exit_code": 0}), "NOT_MEASURED")

    def test_early_close_is_cancelled_with_a_closed_reason(self) -> None:
        value = s2b.validate_capture(example())
        self.assertTrue(value["summary"]["cancelled"])
        self.assertEqual(
            s2b.disposition_detail(
                value, {"timed_out": False, "exit_code": 0, "elapsed_ms": 5,
                        "cleanup": "complete"}, duration_ms=1000),
            {"status": "CANCELLED", "reason": "CLOSED_BEFORE_DECLARED_DURATION"})
        self.assertEqual(
            s2b.disposition_detail(
                value, {"timed_out": False, "exit_code": 0, "elapsed_ms": 5000,
                        "cleanup": "complete"}, duration_ms=1000),
            {"status": "OBSERVED", "reason": "RETAINED_SOURCE_OBSERVATION"})
        self.assertEqual(
            s2b.disposition_detail(value, {"timed_out": True, "exit_code": 1},
                                   duration_ms=1000),
            {"status": "FAILED", "reason": "LAUNCHER_FAILED"})
        self.assertEqual(
            s2b.disposition_detail(
                value, {"timed_out": False, "exit_code": 0, "elapsed_ms": 5000,
                        "cleanup": "incomplete"}, duration_ms=1000),
            {"status": "FAILED", "reason": "CLEANUP_UNCONFIRMED"})

    def test_refused_publication_removes_only_its_own_partial_artifacts(self) -> None:
        import s2b_capture
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            owned_file = root / "s2b-capture.json"
            owned_file.write_text("partial", encoding="utf-8")
            owned_dir = root / "s2b-committed-partial"
            owned_dir.mkdir()
            driver_owned = root / "ledger.json"
            driver_owned.write_text("driver", encoding="utf-8")
            s2b_capture.discard_partial([owned_file, owned_dir])
            self.assertFalse(owned_file.exists())
            self.assertFalse(owned_dir.exists())
            self.assertTrue(driver_owned.exists(), "the S2A driver publication is not ours")

    def test_local_surface_and_notify_endpoints_are_produced_from_retained_records(self) -> None:
        value = s2b.validate_capture(example())
        self.assertGreaterEqual(value["summary"]["kind_counts"][8], 1)
        self.assertGreaterEqual(value["summary"]["kind_counts"][9], 1)
        matrix = s2b.endpoint_matrix(value)
        self.assertEqual(matrix["local-surface-mutation"],
                         {"status": "OBSERVED", "reason": "RETAINED_SOURCE_RECORD"})
        self.assertEqual(matrix["local-notify"],
                         {"status": "OBSERVED", "reason": "RETAINED_SOURCE_RECORD"})
        for name in ("app-main-entry", "frame-request", "first-frame", "useful-shell",
                     "present-call", "present-return", "present", "displayed",
                     "scanout", "input-to-photon"):
            self.assertEqual(
                matrix[name],
                {"status": "NOT_MEASURED", "reason": "NOT_EXPOSED_BY_SELECTED_PROBE"}, name)
        without_local = copy.deepcopy(value)
        without_local["summary"]["kind_counts"][8] = 0
        without_local["summary"]["kind_counts"][9] = 0
        without_local = s2b.endpoint_matrix(without_local)
        self.assertEqual(without_local["local-surface-mutation"],
                         {"status": "NOT_MEASURED", "reason": "NO_RETAINED_VALID_RECORD"})
        self.assertEqual(without_local["local-notify"],
                         {"status": "NOT_MEASURED", "reason": "NO_RETAINED_VALID_RECORD"})

    def test_retained_bundles_are_classified_cancelled_and_observed(self) -> None:
        bundle = REPO / "docs/development/evidence/r0a-s2b/native-bundles.zip"
        if not bundle.is_file():
            self.skipTest("NOT_RUN: the retained S2B bundle is not present in this checkout")
        for tag, status, reason, retained in (
                ("n02d", "OBSERVED", "RETAINED_SOURCE_OBSERVATION", 256),
                ("n04b", "CANCELLED", "CLOSED_BEFORE_DECLARED_DURATION", 34)):
            completed = subprocess.run(
                [sys.executable, "-B", str(ROOT / "s2b_validate.py"), "bundle",
                 "--bundle-zip", str(bundle), "--bundle-tag", tag, "--timeout-ms", "60000"],
                capture_output=True, text=True, timeout=120)
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            payload = json.loads(completed.stdout)
            self.assertEqual(payload["disposition"], status, tag)
            self.assertEqual(payload["disposition_reason"], reason, tag)
            self.assertEqual(payload["retained"], retained, tag)

    def test_power_boundary_cannot_be_crossed_by_a_valid_duration(self):
        value = example()
        value["records"][5].update(kind=7, window=None, lifetime=None)
        value["records"][6].update(start_ns=41, dirty_ns=40)
        self.reject(value, "POWER_BOUNDARY")

    def test_positive_offered_position_loss_cannot_exceed_the_reported_total(self):
        value = example()
        value["records"].append({
            "seq":10,"kind":5,"window":None,"lifetime":None,"start_ns":None,"end_ns":None,
            "dirty_ns":None,"invalidations":None,"paired_draw_start_ns":None,
            "paired_draw_end_ns":None,"category":None,"caused_invalidation":None,"lost":3,
        })
        value["summary"].update(journal_lost=3, journal_seen=8, eligible=10, retained=10,
                                kind_counts=[1,2,2,1,1,1,0,0,1,1])
        self.assertEqual(s2b.validate_capture(value)["summary"]["unoffered_loss"], "UNKNOWN")
        value["summary"]["journal_lost"] = 2
        self.reject(value, "COUNTS")

    def test_single_handle_parser_rejects_duplicate_nonfinite_utf8_and_oversize(self):
        self.assertEqual(s2b.parse_bytes(b'{"a":1}', 16), {"a": 1})
        for raw, code in ((b'{"a":1,"a":2}', "DUPLICATE_KEY"), (b'{"a":NaN}', "NON_FINITE_NUMBER"),
                          (b"\xff", "INVALID_UTF8"), (b'{"a":', "MALFORMED_JSON"),
                          (b'{"a":' + b"1" * 32 + b'}', "TOO_LARGE")):
            with self.subTest(code=code), self.assertRaises(s2b.ObservationError) as error:
                s2b.parse_bytes(raw, 16)
            self.assertEqual(str(error.exception), code)

    @unittest.skipUnless(os.name == "nt", "Windows closed copy boundary")
    def test_replay_copy_rejects_unknown_entries_oversize_and_marker_shapes(self):
        import s2b_capture
        from windows_owner import Pin
        allowed = {"s2b-committed-" + "a" * 64}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "s2b-capture.json").write_bytes(FIXTURE.read_bytes())
            (root / "private.bin").write_bytes(b"x")
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_capture.materialized_copy(root, allowed)
            self.assertEqual(str(error.exception), "UNEXPECTED_ENTRY")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "s2b-capture.json").write_bytes(b"x" * (s2b.MAX_BYTES + 1))
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_capture.materialized_copy(root, allowed)
            self.assertEqual(str(error.exception), "ARTIFACT_SIZE")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ("committed-" + "0" * 63)).mkdir()
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_capture.materialized_copy(root, allowed)
            self.assertEqual(str(error.exception), "UNEXPECTED_ENTRY")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / ("s2b-committed-" + "a" * 64)
            marker.write_bytes(b"x")
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_capture.materialized_copy(root, allowed)
            self.assertEqual(str(error.exception), "COMMIT_INVALID")
            marker.write_bytes(b"")
            created = None
            try:
                # The successful copy owns a temporary directory; a test must not
                # discard that owner and leak it into the shared temp root.
                _, created = s2b_capture.materialized_copy(root, allowed)
                with self.assertRaises(s2b.ObservationError) as error:
                    s2b_capture.materialized_copy(root, {"s2b-committed-" + "c" * 64})
                self.assertEqual(str(error.exception), "MARKER_UNEXPECTED")
            finally:
                if created is not None:
                    shutil.rmtree(created, ignore_errors=True)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index in range(9):
                (root / ("s2b-committed-" + f"{index:064x}")).write_bytes(b"")
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_capture.materialized_copy(root, {f"s2b-committed-{i:064x}" for i in range(9)})
            self.assertEqual(str(error.exception), "MARKER_LIMIT")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index in range(33):
                (root / f"f{index}").write_bytes(b"")
            with self.assertRaises(s2b.ObservationError) as error:
                s2b_capture.materialized_copy(root, allowed)
            self.assertEqual(str(error.exception), "ENTRY_LIMIT")

    def test_zip_replay_rejects_traversal_duplicates_unexpected_symlink_and_size(self):
        import s2b_validate, zipfile
        def archive(entries):
            directory = tempfile.mkdtemp()
            path = Path(directory) / "bundle.zip"
            with zipfile.ZipFile(path, "w") as z:
                for name, data in entries:
                    info = zipfile.ZipInfo(name)
                    info.external_attr = 0o120777 << 16 if name.endswith("link") else 0
                    z.writestr(info, data)
            return path, Path(directory)
        cases = [
            ([("../escape.json", b"x")], "ZIP_MEMBER"),
            ([("n02d/a", b"1"), ("n02d/a", b"2")], "ZIP_DUPLICATE"),
            ([("elsewhere/a", b"1")], "ZIP_UNEXPECTED"),
            ([("n02d/link", b"/etc/passwd")], "ZIP_MEMBER"),
            ([("feature-manifest.json", b"x" * (1024 * 1024 + 1))], "ZIP_SIZE"),
            ([(f"n02d/f{i}", b"x") for i in range(65)], "ZIP_ENTRY_LIMIT"),
            ([("n02d/./ledger.json", b"1")], "ZIP_MEMBER"),
            ([("n02d/ledger.json", b"1"), ("n02d/Ledger.json", b"2")], "ZIP_DUPLICATE"),
            ([("other/", b"")], "ZIP_UNEXPECTED"),
            ([("n02d//ledger.json", b"1")], "ZIP_MEMBER"),
            ([("n02d//", b"")], "ZIP_MEMBER"),
            ([("/n02d/ledger.json", b"1")], "ZIP_MEMBER"),
            ([("n02d/", b"directory-payload")], "ZIP_SIZE"),
            ([("n02d/NUL", b"x")], "ZIP_MEMBER"),
            ([("n02d/nul", b"x")], "ZIP_MEMBER"),
            ([("n02d/Con.txt", b"x")], "ZIP_MEMBER"),
            ([("n02d/COM1", b"x")], "ZIP_MEMBER"),
            ([("n02d/lpt9.log", b"x")], "ZIP_MEMBER"),
        ]
        for entries, code in cases:
            path, directory = archive(entries)
            with self.subTest(code=code), tempfile.TemporaryDirectory() as target, \
                    self.assertRaises(s2b.ObservationError) as error:
                s2b_validate.safe_extract(str(path), Path(target))
            self.assertEqual(str(error.exception), code)
            shutil.rmtree(directory, ignore_errors=True)

    def test_terminal_directory_slash_and_zero_size_directories_remain_valid(self):
        import s2b_validate, zipfile
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / "bundle.zip"
            with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
                # Matches the retained Native archive: an empty deflated directory
                # entry has zero file data but a nonzero compressed stream.
                archive.writestr("n02d/", b"")
                archive.writestr("n02d/ledger.json", b'{"ok":true}')
            with tempfile.TemporaryDirectory() as target:
                s2b_validate.safe_extract(str(path), Path(target))
                self.assertEqual((Path(target) / "n02d/ledger.json").read_bytes(), b'{"ok":true}')

    def test_malformed_and_crc_invalid_archives_use_one_closed_code(self):
        import s2b_validate, zipfile
        with tempfile.TemporaryDirectory() as work:
            broken = Path(work) / "broken.zip"
            broken.write_bytes(b"PK\x03\x04 truncated archive")
            with tempfile.TemporaryDirectory() as target, self.assertRaises(s2b.ObservationError) as error:
                s2b_validate.safe_extract(str(broken), Path(target))
            self.assertEqual(str(error.exception), "ZIP_INVALID")
            payload = b"crc-invalid-payload"
            corrupt = Path(work) / "crc.zip"
            with zipfile.ZipFile(corrupt, "w", zipfile.ZIP_STORED) as archive:
                archive.writestr("n02d/ledger.json", payload)
            raw = bytearray(corrupt.read_bytes())
            offset = raw.find(payload)
            self.assertGreaterEqual(offset, 0)
            raw[offset] ^= 0xFF
            corrupt.write_bytes(bytes(raw))
            with tempfile.TemporaryDirectory() as target, self.assertRaises(s2b.ObservationError) as error:
                s2b_validate.safe_extract(str(corrupt), Path(target))
            self.assertEqual(str(error.exception), "ZIP_INVALID")

    def test_bundle_replay_reports_a_broken_zip_without_a_traceback(self):
        with tempfile.TemporaryDirectory() as work:
            broken = Path(work) / "broken.zip"
            broken.write_bytes(b"not a zip archive")
            owned = Path(tempfile.gettempdir())
            before = {path.name for path in owned.glob("s2b-bundle-*")}
            outcome = subprocess.run(
                [sys.executable, "-B", str(ROOT / "s2b_validate.py"), "bundle",
                 "--bundle-zip", str(broken), "--bundle-tag", "n02d", "--timeout-ms", "1000"],
                capture_output=True, check=False, timeout=10,
            )
            after = {path.name for path in owned.glob("s2b-bundle-*")}
        self.assertEqual(outcome.returncode, 1)
        self.assertEqual(json.loads(outcome.stdout), {"ok": False, "error": "ZIP_INVALID"})
        self.assertNotIn(b"Traceback", outcome.stderr)
        self.assertEqual(outcome.stderr, b"")
        # A refused replay must not leave its owned extraction directory behind.
        self.assertEqual(after - before, set())

    def test_refused_archive_is_not_partially_expanded_or_overwritten(self):
        import s2b_validate, struct, zipfile
        with tempfile.TemporaryDirectory() as work:
            collision = Path(work) / "collision.zip"
            with zipfile.ZipFile(collision, "w") as archive:
                archive.writestr("n02d/ledger.json", b"original")
                archive.writestr("n02d//ledger.json", b"replacement")
            with tempfile.TemporaryDirectory() as target:
                with self.assertRaises(s2b.ObservationError) as error:
                    s2b_validate.safe_extract(str(collision), Path(target))
                self.assertEqual(str(error.exception), "ZIP_MEMBER")
                self.assertEqual(list(Path(target).iterdir()), [])
            hidden = Path(work) / "hidden.zip"
            with zipfile.ZipFile(hidden, "w", zipfile.ZIP_DEFLATED) as archive:
                archive.writestr("n02d/", b"payload-hidden")
            # A directory entry can declare empty data while keeping a compressed
            # payload; the declared sizes alone cannot prove the member is empty.
            raw = bytearray(hidden.read_bytes())
            central = raw.rfind(b"PK\x01\x02")
            self.assertGreaterEqual(central, 0)
            struct.pack_into("<I", raw, central + 16, 0)  # CRC of empty data
            struct.pack_into("<I", raw, central + 24, 0)  # declared uncompressed size
            hidden.write_bytes(bytes(raw))
            with tempfile.TemporaryDirectory() as target, self.assertRaises(s2b.ObservationError) as error:
                s2b_validate.safe_extract(str(hidden), Path(target))
            self.assertEqual(str(error.exception), "ZIP_SIZE")

    def test_windows_device_names_cannot_be_silently_dropped(self):
        # Observed on this Windows host: writing a member literally named NUL through
        # the OS succeeds without creating a file, so extraction would report a
        # materialised member that does not exist. Refuse the device alias first.
        import s2b_validate, zipfile
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / "device.zip"
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr("n02d/ledger.json", b"kept")
                archive.writestr("n02d/NUL", b"discarded-by-device")
            with tempfile.TemporaryDirectory() as target:
                with self.assertRaises(s2b.ObservationError) as error:
                    s2b_validate.safe_extract(str(path), Path(target))
                self.assertEqual(str(error.exception), "ZIP_MEMBER")
                self.assertEqual(list(Path(target).iterdir()), [])

    def test_live_manifest_and_build_metadata_name_the_current_consumer_source(self):
        import contracts, s2b_capture
        current = s2b.source_digest()
        for name in ("s2b-default-a0.json", "s2b-feature-a1.json"):
            document = contracts.validate_manifest(
                contracts.load_document(str(ROOT / "manifests" / name), contracts.MAX_MANIFEST_BYTES))
            self.assertEqual(document["identity"]["driver_hash"], current, name)
        feature = contracts.load_document(
            str(ROOT / "manifests/s2b-feature-a1.json"), contracts.MAX_MANIFEST_BYTES)
        self.assertEqual(s2b.validate_manifest(feature)["identity"]["driver_hash"], current)
        build = contracts.load_document(str(ROOT / "s2b-build.json"), contracts.MAX_MANIFEST_BYTES)
        self.assertEqual(
            s2b_capture.validate_build_record(build, live=True)["observer_source_sha256"], current)

    def test_recorded_build_and_source_identity_must_agree(self):
        import s2b_capture
        recorded = {"observer_source_sha256": "sha256:" + "a" * 64}
        identity = {"observer_source_sha256": "sha256:" + "a" * 64}
        s2b_capture.require_source_agreement(recorded, identity)
        identity["observer_source_sha256"] = "sha256:" + "b" * 64
        with self.assertRaises(s2b.ObservationError) as error:
            s2b_capture.require_source_agreement(recorded, identity)
        self.assertEqual(str(error.exception), "SOURCE_IDENTITY")

    @unittest.skipUnless(os.name == "nt", "Windows directory visibility and Pin boundary")
    def test_absent_or_nonempty_completion_directory_never_commits(self):
        import s2b_capture
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(OSError):
                s2b_capture.require_marker(root, "committed-test")
            marker = root / "committed-test"
            marker.mkdir()
            (marker / "partial").write_text("not committed", encoding="ascii")
            with self.assertRaises(s2b.ObservationError):
                s2b_capture.require_marker(root, "committed-test")
            (marker / "partial").unlink()
            marker.rmdir()
            marker.write_bytes(b"")  # byte-bound archive representation
            s2b_capture.require_marker(root, "committed-test")
            marker.write_bytes(b"partial")
            with self.assertRaises(s2b.ObservationError):
                s2b_capture.require_marker(root, "committed-test")


if __name__ == "__main__":
    unittest.main()
