"""MP-VID1 capability-record tests: closed schema, fail-closed refusals and derivation.

Every test drives the real CLI path with a real PowerShell interpreter and a generated
probe script (`--probe` is the documented seam), so the runner's own spawn/hash/capture
logic is exercised rather than mocked. The probe's *host* observations are represented by
scripted documents; the real capability run is a separate, explicitly opted-in test.

Route honesty
-------------
A string scan of the product sources can never conclude that the pinned GPUI revision has
no presentation route, so these tests pin the rule that ``CONFIRMED_LIMITATION`` is
unreachable without an inspection of the pinned API, a fully bound complete scan and an
observed decode side; everything else must be ``UNEXAMINED`` with a closed reason and the
list of evidence a route decision still needs.
"""
from __future__ import annotations

import base64
import contextlib
import io
import json
import os
import shutil
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
REPO_ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE))
import vid1_capability as vid1  # noqa: E402

PWSH = shutil.which("pwsh") or shutil.which("pwsh.exe")
PROBE = HERE / "vid1_probe.ps1"
FFMPEG = Path(r"C:\ffmpeg\bin\ffmpeg.exe")
GIT = Path(r"C:\Program Files\Git\cmd\git.exe")
CAPS = {"timeout_ms": 1000, "max_output_bytes": 65536, "max_fixture_bytes": 1048576}
SURFACE_TOKENS = ["draw_surfaces", "IDXGISurface"]
HEAD_SHA = "3120a27467d580f5a0c22eb2f44e62314032e2d0"


def software_row(**overrides) -> dict:
    row = {
        "id": "sw_h264_first_frame",
        "kind": "decode",
        "container": "mp4",
        "codec": "h264",
        "pixel_format": "yuv420p",
        "hwaccel": "none",
        "argv": ["C:\\ffmpeg\\bin\\ffmpeg.exe", "-v", "error", "-i", "fixture.mp4", "-f", "null", "-"],
        "exit_code": 0,
        "wall_ms": 70,
        "status": "OBSERVED",
        "reason": "",
        "stdout_bytes": 0,
        "stderr_bytes": 0,
        "capped": False,
        "frames_decoded": 2,
        "decoder_pix_fmts": [],
        "hardware_pix_fmt": "",
        "hardware_frame_evidence": "NONE",
        "dxva_profile_guids": [],
        "output_bytes": None,
        "expected_output_bytes": None,
    }
    row.update(overrides)
    return row


def hardware_row(**overrides) -> dict:
    row = {
        "id": "hw_d3d11va_h264_decode",
        "kind": "hw_decode",
        "container": "mp4",
        "codec": "h264",
        "pixel_format": "yuv420p",
        "hwaccel": "d3d11va",
        "argv": [
            "C:\\ffmpeg\\bin\\ffmpeg.exe",
            "-hwaccel",
            "d3d11va",
            "-hwaccel_output_format",
            "d3d11",
            "-i",
            "fixture.mp4",
            "-f",
            "null",
            "-",
        ],
        "exit_code": 0,
        "wall_ms": 400,
        "status": "OBSERVED",
        "reason": "",
        "stdout_bytes": 0,
        "stderr_bytes": 1024,
        "capped": False,
        "frames_decoded": 5,
        "decoder_pix_fmts": ["yuv420p", "d3d11"],
        "hardware_pix_fmt": "d3d11",
        "hardware_frame_evidence": "PIX_FMT_OBSERVED",
        "dxva_profile_guids": [],
        "output_bytes": None,
        "expected_output_bytes": None,
    }
    row.update(overrides)
    return row


def scan(**overrides) -> dict:
    body = {
        "status": "OBSERVED",
        "reason": "",
        "root": "C:\\repo",
        "git_head_sha": HEAD_SHA,
        "git_tree_clean": True,
        "git_status_reason": "",
        "git_path": r"C:\Program Files\Git\cmd\git.exe",
        "file_cap": 3000,
        "file_size_cap": 1048576,
        "files_scanned": 80,
        "files_skipped_size": 0,
        "cap_hit": False,
        "bytes_scanned": 2073428,
        "tokens": list(SURFACE_TOKENS),
        "matches": [],
        "signal": "COMPLETE_BOUND",
    }
    body.update(overrides)
    return body


def not_run_scan() -> dict:
    return scan(
        status="NOT_MEASURED",
        reason="SOURCE_SCAN_NOT_RUN",
        root="",
        git_head_sha="",
        git_tree_clean=None,
        git_status_reason="GIT_UNAVAILABLE",
        git_path="",
        files_scanned=0,
        bytes_scanned=0,
        signal="NOT_RUN",
    )


def inspection(status: str = "NOT_RUN", **overrides) -> dict:
    body = {
        "status": status,
        "reason": "PIN_API_NOT_INSPECTED" if status == "NOT_RUN" else "",
        "pin": "" if status == "NOT_RUN" else "gpui 395def7a067b32ed598fcb3f048969ada9b54942",
        "api_surface": "" if status == "NOT_RUN" else "gpui::Window::draw_surfaces",
        "findings": [] if status == "NOT_RUN" else ["no surface import entry point exported by the pin"],
    }
    body.update(overrides)
    return body


def route(**overrides) -> dict:
    body = {
        "route_id": "mf-d3d11va-decode:d3d11-texture:gpui-surface-presentation",
        "status": "UNEXAMINED",
        "reason": "PIN_API_NOT_INSPECTED",
        "decode_side_observed": True,
        "scan_signal": "COMPLETE_BOUND",
        "scan_usable_as_support": True,
        "pin_api_inspection": "NOT_RUN",
        "grounds": [
            {"id": "decode_side_capability", "status": "OBSERVED", "detail": "hardware frames observed for h264"},
            {"id": "presentation_call_site_scan", "status": "OBSERVED", "detail": "signal=COMPLETE_BOUND; 80 files"},
            {
                "id": "pin_surface_documented_noop",
                "status": "DOCUMENTED",
                "detail": "DC-MP-VID1 records the pinned Windows surface path as a no-op",
            },
            {"id": "pin_api_inspection", "status": "NOT_MEASURED", "detail": "the pinned API was not inspected"},
        ],
        "missing_evidence": ["PIN_API_INSPECTION", "PIN_SURFACE_API", "COPY_COUNT"],
    }
    body.update(overrides)
    return body


def document(**overrides) -> dict:
    """A schema-valid probe document; each test overrides only the field it exercises."""
    body = {
        "ok": True,
        "probe_version": 3,
        "caps": dict(CAPS, max_experiments=24),
        "mft_flags": {"sync": "0x1", "async": "0x2", "hardware": "0x4", "all": "0x3F"},
        "host": {
            "status": "OBSERVED",
            "reason": "",
            "os_description": "Microsoft Windows 10.0.26300",
            "arch": "X64",
            "pwsh": "7.6.7",
            "interop_enabled": True,
            "scratch_retained": False,
            "scratch_path": "",
        },
        "media_foundation": {
            "status": "OBSERVED",
            "reason": "",
            "mft_guid_source": "Windows SDK 10.0.19041.0 um/mfapi.h",
            "components": [
                {"name": "mfplat.dll", "status": "OBSERVED", "reason": "", "version": "10.0.26100.9278"}
            ],
            "mft_enumeration": [
                {
                    "category": "video_decoder",
                    "guid": "d6c02d4b-6833-45b4-971a-05a4b04bab91",
                    "hresult": "0x00000000",
                    "all": 17,
                    "sync": 16,
                    "async": 0,
                    "hardware": 1,
                    "sync_status": "OBSERVED",
                    "async_status": "OBSERVED",
                    "hardware_status": "OBSERVED",
                    "reason": "",
                }
            ],
        },
        "d3d11": {
            "status": "OBSERVED",
            "reason": "",
            "hardware_hresult": "0x00000000",
            "hardware_feature_level": "0xB000",
            "warp_hresult": "0x00000000",
            "warp_feature_level": "0xB000",
            "adapters": [{"name": "Test Adapter", "driver_version": "1.2.3"}],
        },
        "tool": {"status": "NOT_MEASURED", "reason": "TOOL_NOT_SUPPLIED", "path": "", "sha256": "", "hwaccels": []},
        "fixtures": [
            {
                "id": "h264",
                "container": "mp4",
                "codec": "h264",
                "pixel_format": "yuv420p",
                "status": "OBSERVED",
                "reason": "",
                "bytes": 4096,
                "sha256": "a" * 64,
                "width": 640,
                "height": 360,
                "seconds": 1,
                "generator_argv": ["C:\\ffmpeg\\bin\\ffmpeg.exe", "-f", "lavfi", "-i", "testsrc2"],
            }
        ],
        "interop": [software_row(), hardware_row()],
        "source_scan": scan(),
        "pin_api_inspection": inspection(),
        "route": route(),
        "endpoints": [
            {"endpoint": name, "status": "NOT_MEASURED", "reason": why}
            for name, why in vid1.UNOBSERVABLE_ENDPOINTS.items()
        ],
        "corrections": [
            {
                "id": "mft_hardware_flag",
                "status": "CORRECTED_IN_THIS_RECORD",
                "detail": "the earlier hardware count used 0x2 (ASYNCMFT); hardware is 0x4",
            },
            {
                "id": "route_claim_from_scan_only",
                "status": "CORRECTED_IN_THIS_RECORD",
                "detail": "a scan alone no longer concludes a route limitation",
            },
        ],
    }
    for key, value in overrides.items():
        body[key] = value
    return body


def observed_tool() -> dict:
    return {
        "status": "OBSERVED",
        "reason": "",
        "path": r"C:\ffmpeg\bin\ffmpeg.exe",
        "sha256": "81b5832f6c548d64feba0ec7d643397e2351d9f3460a65c6c296956045eebaea",
        "hwaccels": ["d3d11va", "dxva2"],
    }


class TempProbes:
    """Writes throw-away PowerShell probes into one absolute temporary directory."""

    def __init__(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="vid1-test-"))

    def cleanup(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)

    def write(self, name: str, *, payload: str = "", exit_code: int = 0, touch_self: bool = False) -> str:
        encoded = base64.b64encode(payload.encode("utf-8")).decode("ascii")
        lines = ["$ErrorActionPreference = 'Stop'"]
        if touch_self:
            lines.append("Add-Content -LiteralPath $PSCommandPath -Value '# changed while running'")
        if encoded:
            lines.append(
                "$json = [System.Text.Encoding]::UTF8.GetString("
                f"[System.Convert]::FromBase64String('{encoded}'))"
            )
            lines.append("[Console]::Out.Write($json)")
        lines.append(f"exit {exit_code}")
        path = self.root / name
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return str(path)

    def write_script(self, name: str, lines: list[str]) -> str:
        path = self.root / name
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return str(path)

    def write_flooder(self, name: str) -> tuple[str, Path]:
        """A cmd tool that floods stderr and would write a marker after a delay."""
        marker = self.root / f"{name}.post-marker.txt"
        if marker.exists():
            marker.unlink()
        echo = "@echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        lines = [
            "@echo off",
            f"for /L %%i in (1,1,4000) do {echo} 1>&2",
            "ping -n 3 127.0.0.1 >nul",
            f'echo done> "{marker}"',
        ]
        return self.write_script(name, lines), marker


@unittest.skipUnless(PWSH, "PowerShell 7 is required for the documented --probe seam")
class CapabilityRecordTests(unittest.TestCase):
    def setUp(self) -> None:
        self.probes = TempProbes()
        self.addCleanup(self.probes.cleanup)
        self.out = self.probes.root / "record.json"

    # -- helpers -----------------------------------------------------------------
    def run_cli(self, probe: str, *extra: str, caps: dict | None = None) -> tuple[int, str, str]:
        self.out.unlink(missing_ok=True)
        chosen = dict(caps or CAPS)
        argv = [
            "--probe",
            probe,
            "--powershell",
            PWSH,
            "--out",
            str(self.out),
            "--timeout-ms",
            str(chosen["timeout_ms"]),
            "--max-output-bytes",
            str(chosen["max_output_bytes"]),
            "--max-fixture-bytes",
            str(chosen["max_fixture_bytes"]),
            *extra,
        ]
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = vid1.main(argv)
        return code, out.getvalue(), err.getvalue()

    def probe_for(self, body: dict, *, exit_code: int = 0, touch_self: bool = False) -> str:
        return self.probes.write(
            "probe.ps1", payload=json.dumps(body, ensure_ascii=False), exit_code=exit_code, touch_self=touch_self
        )

    def record(self) -> dict:
        return json.loads(self.out.read_text(encoding="utf-8"))

    def assert_refused(self, code: int, err: str, refusal: str) -> None:
        self.assertEqual(code, 2, err)
        self.assertIn(f"REFUSED {refusal}", err)
        self.assertFalse(self.out.exists(), "a refusal must not write a record")

    def with_cap(self, body: dict, cap: int | None = None) -> tuple[dict, int]:
        """Return the document and its byte length, echoing `cap` (default: its own length)."""
        body = json.loads(json.dumps(body))
        if cap is None:
            length = 0
            for _ in range(6):
                body["caps"] = dict(body["caps"], max_output_bytes=length)
                measured = len(json.dumps(body, ensure_ascii=False).encode("utf-8"))
                if measured == length:
                    break
                length = measured
            return body, length
        body["caps"] = dict(body["caps"], max_output_bytes=cap)
        return body, len(json.dumps(body, ensure_ascii=False).encode("utf-8"))

    def caps_with(self, cap: int) -> dict:
        return {"timeout_ms": 1000, "max_output_bytes": cap, "max_fixture_bytes": 1048576}

    # -- valid path --------------------------------------------------------------
    def test_valid_document_produces_a_bounded_record(self) -> None:
        code, out, err = self.run_cli(self.probe_for(document()))
        self.assertEqual(code, 0, err)
        record = self.record()
        self.assertEqual(
            sorted(record),
            sorted(
                {
                    "record_version",
                    "task",
                    "generated_utc",
                    "probe",
                    "caps",
                    "mft_flags",
                    "host",
                    "media_foundation",
                    "d3d11",
                    "tool",
                    "fixtures",
                    "interop",
                    "source_scan",
                    "pin_api_inspection",
                    "route",
                    "endpoints",
                    "corrections",
                    "summary",
                    "vid2_material",
                    "caps_requested",
                }
            ),
        )
        self.assertEqual(record["task"], "MP-VID1")
        self.assertEqual(record["record_version"], 3)
        self.assertEqual(record["mft_flags"], {"sync": "0x1", "async": "0x2", "hardware": "0x4", "all": "0x3F"})
        self.assertEqual(record["media_foundation"]["mft_enumeration"][0]["hardware"], 1)
        # The honest outcome: no route conclusion without inspecting the pinned API.
        self.assertEqual(record["route"]["status"], "UNEXAMINED")
        self.assertEqual(record["route"]["reason"], "PIN_API_NOT_INSPECTED")
        self.assertEqual(record["summary"]["route_status"], "UNEXAMINED")
        self.assertEqual(record["summary"]["route_reason"], "PIN_API_NOT_INSPECTED")
        self.assertEqual(record["summary"]["scan_signal"], "COMPLETE_BOUND")
        self.assertTrue(record["summary"]["scan_usable_as_support"])
        self.assertIn("PIN_API_INSPECTION", record["route"]["missing_evidence"])
        self.assertEqual(record["pin_api_inspection"]["status"], "NOT_RUN")
        self.assertEqual(record["corrections"][0]["id"], "mft_hardware_flag")
        self.assertEqual(record["fixtures"][0]["sha256"], "a" * 64)
        self.assertEqual(record["probe"]["exit_code"], 0)
        self.assertIn("route=UNEXAMINED", out)
        # No decision field may ever appear: VID1 reports, MP-VID2 decides.
        self.assertNotIn('"decision"', json.dumps(record))

    def test_absent_capability_is_carried_with_its_reason_and_no_invented_value(self) -> None:
        disabled = software_row(
            id="interop_disabled",
            container="",
            codec="",
            pixel_format="",
            hwaccel="",
            argv=[],
            exit_code=None,
            wall_ms=None,
            status="NOT_MEASURED",
            reason="INTEROP_DISABLED",
            frames_decoded=None,
        )
        body = document(
            interop=[disabled],
            source_scan=not_run_scan(),
            route=route(scan_signal="NOT_RUN", scan_usable_as_support=False, decode_side_observed=False),
        )
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assertEqual(code, 0, err)
        record = self.record()
        carried = record["interop"][0]
        self.assertEqual(carried["status"], "NOT_MEASURED")
        self.assertEqual(carried["reason"], "INTEROP_DISABLED")
        self.assertIsNone(carried["exit_code"])
        self.assertIsNone(carried["wall_ms"])
        self.assertIsNone(carried["frames_decoded"])
        self.assertEqual(carried["hardware_pix_fmt"], "")
        self.assertEqual(record["tool"]["path"], "")
        self.assertEqual(record["route"]["status"], "UNEXAMINED")
        self.assertFalse(record["summary"]["scan_usable_as_support"])
        self.assertIn(
            "endpoint:presentation_route NOT_MEASURED PRESENTATION_PATH_UNMEASURED",
            record["vid2_material"]["unavailable"],
        )

    def test_derivation_counts_and_tradeoffs_follow_only_observed_rows(self) -> None:
        code, _out, err = self.run_cli(self.probe_for(document(tool=observed_tool(), interop=[hardware_row()])))
        self.assertEqual(code, 0, err)
        record = self.record()
        self.assertEqual(record["summary"]["hardware_decode_observed"], 1)
        self.assertEqual(record["summary"]["hardware_download_observed"], 0)
        tradeoffs = " ".join(record["vid2_material"]["tradeoffs"])
        self.assertIn("no video route is selected", tradeoffs)
        self.assertIn("importing those frames into the pinned GPUI revision remains unverified", tradeoffs)
        self.assertIn("the route statement is UNEXAMINED", tradeoffs)
        self.assertIn(
            "hw_decode:hw_d3d11va_h264_decode codec=h264 hwaccel=d3d11va hardware_pix_fmt=d3d11",
            record["vid2_material"]["observed_paths"][0],
        )

    # -- BLOCKING (this round): the scan cannot conclude a route ------------------
    def test_scan_truncated_by_the_file_cap_cannot_yield_a_limitation(self) -> None:
        truncated = scan(cap_hit=True, files_skipped_size=3, signal="TRUNCATED")
        body = document(source_scan=truncated, route=route(scan_signal="TRUNCATED", scan_usable_as_support=False))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assertEqual(code, 0, err)
        record = self.record()
        self.assertEqual(record["summary"]["scan_signal"], "TRUNCATED")
        self.assertFalse(record["summary"]["scan_usable_as_support"])
        self.assertEqual(record["route"]["status"], "UNEXAMINED")

        body = document(source_scan=scan(cap_hit=True, files_skipped_size=3, signal="COMPLETE_BOUND"))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("COMPLETE_BOUND conflicts with a truncated scan", err)

        body = document(
            source_scan=truncated,
            route=route(status="CONFIRMED_LIMITATION", reason="", scan_signal="TRUNCATED", scan_usable_as_support=False),
        )
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")

    def test_unbound_scan_cannot_yield_a_limitation(self) -> None:
        unbound = scan(git_head_sha="", git_tree_clean=None, git_status_reason="GIT_STATUS_NOT_MEASURED", signal="UNBOUND")
        body = document(source_scan=unbound, route=route(scan_signal="UNBOUND", scan_usable_as_support=False))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assertEqual(code, 0, err)
        record = self.record()
        self.assertEqual(record["summary"]["scan_signal"], "UNBOUND")
        self.assertFalse(record["summary"]["scan_usable_as_support"])
        self.assertEqual(record["route"]["status"], "UNEXAMINED")

        body = document(
            source_scan=unbound,
            route=route(status="CONFIRMED_LIMITATION", reason="", scan_signal="UNBOUND", scan_usable_as_support=False),
        )
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")

        dirty = scan(git_tree_clean=False)
        body = document(source_scan=dirty)
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("COMPLETE_BOUND requires a clean tree", err)

    def test_confirmed_limitation_from_scan_only_is_refused(self) -> None:
        body = document(route=route(status="CONFIRMED_LIMITATION", reason="", pin_api_inspection="NOT_RUN"))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("requires an inspection of the pinned API", err)

    def test_unexamined_route_with_closed_reason_is_accepted_and_names_missing_evidence(self) -> None:
        body = document()
        body["route"] = route(missing_evidence=["PIN_API_INSPECTION", "PIN_SURFACE_API", "COPY_COUNT", "DEVICE_LOSS"])
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assertEqual(code, 0, err)
        record = self.record()
        self.assertEqual(record["route"]["status"], "UNEXAMINED")
        self.assertEqual(record["route"]["reason"], "PIN_API_NOT_INSPECTED")
        self.assertEqual(
            record["route"]["missing_evidence"],
            ["PIN_API_INSPECTION", "PIN_SURFACE_API", "COPY_COUNT", "DEVICE_LOSS"],
        )
        self.assertEqual(record["pin_api_inspection"]["status"], "NOT_RUN")

    def test_unexamined_route_needs_a_registered_reason(self) -> None:
        body = document(route=route(reason=""))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("UNEXAMINED route must carry a registered reason", err)
        body = document(route=route(reason="SOMETHING_NEW"))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")

    def test_route_and_inspection_must_agree_with_their_sections(self) -> None:
        body = document(route=route(scan_signal="UNBOUND", scan_usable_as_support=False))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("does not match source_scan.signal", err)

        body = document(route=route(pin_api_inspection="OBSERVED"))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("does not match pin_api_inspection.status", err)

    def test_pin_api_inspection_observed_requires_pin_and_findings(self) -> None:
        body = document(pin_api_inspection=inspection("OBSERVED", pin="", api_surface="", findings=[]))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("must name the pin, the API surface and findings", err)

    def test_pin_api_inspection_not_run_cannot_carry_findings(self) -> None:
        body = document(pin_api_inspection=inspection("NOT_RUN", findings=["something"]))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("cannot report findings", err)

    # -- other closed-schema checks -----------------------------------------
    def test_probe_nonzero_exit_is_refused_even_after_a_valid_document(self) -> None:
        code, _out, err = self.run_cli(self.probe_for(document(), exit_code=3))
        self.assert_refused(code, err, "PROBE_EXIT_NONZERO")

    def test_unregistered_reason_is_refused(self) -> None:
        body = document()
        body["interop"][0]["status"] = "NOT_MEASURED"
        body["interop"][0]["reason"] = "SOMETHING_NEW"
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("unregistered reason 'SOMETHING_NEW'", err)

    def test_unregistered_status_kind_and_endpoint_are_refused(self) -> None:
        mutations = (
            (lambda body: body["host"].update(status="PROBABLY_FINE"), "unregistered status"),
            (lambda body: body["interop"][0].update(kind="streaming"), "unregistered kind"),
            (
                lambda body: body["endpoints"].append(
                    {"endpoint": "user_visible_quality", "status": "NOT_MEASURED", "reason": "PROCESS_FAILED"}
                ),
                "unregistered endpoint",
            ),
        )
        for mutate, fragment in mutations:
            with self.subTest(fragment=fragment):
                body = document()
                mutate(body)
                code, _out, err = self.run_cli(self.probe_for(body))
                self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
                self.assertIn(fragment, err)

    def test_an_observed_entry_must_not_carry_a_reason(self) -> None:
        body = document()
        body["host"]["reason"] = "TOOL_NOT_SUPPLIED"
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("OBSERVED entry must carry no reason", err)

    def test_oversized_probe_output_is_refused(self) -> None:
        body, length = self.with_cap(document())
        code, _out, err = self.run_cli(self.probe_for(body), caps=self.caps_with(length - 16))
        self.assert_refused(code, err, "PROBE_OUTPUT_OVERSIZED")

    def test_truncated_document_is_refused(self) -> None:
        payload = json.dumps(document())
        probe = self.probes.write("truncated.ps1", payload=payload[: len(payload) // 2])
        code, _out, err = self.run_cli(probe)
        self.assert_refused(code, err, "PROBE_JSON_INVALID")

    def test_empty_output_is_refused(self) -> None:
        probe = self.probes.write("empty.ps1", payload="")
        code, _out, err = self.run_cli(probe)
        self.assert_refused(code, err, "PROBE_OUTPUT_EMPTY")

    def test_probe_level_refusal_is_carried_as_a_closed_code(self) -> None:
        refusal = self.probes.write("refusal.ps1", payload=json.dumps({"ok": False, "error": "CAP_RANGE"}))
        code, _out, err = self.run_cli(refusal)
        self.assert_refused(code, err, "PROBE_REFUSED")
        unregistered = self.probes.write("bad-refusal.ps1", payload=json.dumps({"ok": False, "error": "OOPS"}))
        code, _out, err = self.run_cli(unregistered)
        self.assert_refused(code, err, "PROBE_REFUSED")

    def test_caps_out_of_range_and_mismatch_are_refused(self) -> None:
        probe = self.probe_for(document())
        code, _out, err = self.run_cli(probe, caps={"timeout_ms": 10, "max_output_bytes": 4096, "max_fixture_bytes": 1048576})
        self.assert_refused(code, err, "CAPS_OUT_OF_RANGE")
        code, _out, err = self.run_cli(probe, caps={"timeout_ms": 1000, "max_output_bytes": 1, "max_fixture_bytes": 1048576})
        self.assert_refused(code, err, "CAPS_OUT_OF_RANGE")
        code, _out, err = self.run_cli(
            probe, caps={"timeout_ms": 2000, "max_output_bytes": CAPS["max_output_bytes"], "max_fixture_bytes": 1048576}
        )
        self.assert_refused(code, err, "CAPS_MISMATCH")

    # -- BLOCKING (previous round, unchanged): MFT flags ---------------------
    def test_mft_flags_are_pinned_to_the_sdk_values(self) -> None:
        body = document()
        body["mft_flags"] = dict(body["mft_flags"], hardware="0x2")
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("expected 0x4 (Windows SDK mfapi.h), got 0x2", err)

    def test_mft_counts_must_agree_with_their_status(self) -> None:
        body = document()
        row = dict(body["media_foundation"]["mft_enumeration"][0], hardware_status="NOT_MEASURED", hardware=1)
        body["media_foundation"] = dict(body["media_foundation"], mft_enumeration=[row])
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("a NOT_MEASURED count must not carry a value", err)

    # -- BLOCKING (previous round, unchanged): GUIDs are not evidence --------
    def test_hardware_decode_without_hardware_evidence_is_refused(self) -> None:
        body = document(interop=[hardware_row(decoder_pix_fmts=["yuv420p"], hardware_pix_fmt="", hardware_frame_evidence="NONE")])
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("requires hardware frames observed in the decoder log", err)

    def test_guid_list_alone_is_not_hardware_evidence(self) -> None:
        guid = "{86695f12-340e-4f04-9fd3-9253dd327460}"
        negative = hardware_row(
            decoder_pix_fmts=["yuv420p"],
            hardware_pix_fmt="",
            hardware_frame_evidence="NONE",
            status="UNSUPPORTED",
            reason="HWACCEL_SOFTWARE_FALLBACK",
            dxva_profile_guids=[guid],
        )
        body = document(interop=[negative])
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assertEqual(code, 0, err)
        self.assertEqual(self.record()["interop"][0]["status"], "UNSUPPORTED")
        claimed = json.loads(json.dumps(body))
        claimed["interop"][0].update(status="OBSERVED", reason="")
        code, _out, err = self.run_cli(self.probe_for(claimed))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")

    def test_hardware_pix_fmt_must_appear_in_the_decoder_log(self) -> None:
        body = document(interop=[hardware_row(decoder_pix_fmts=["yuv420p"])])
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("hardware pix_fmt must appear in the decoder log", err)

    # -- ACTIONABLE (previous round, unchanged): fixture identity ------------
    def test_fixture_without_digest_or_generator_is_refused(self) -> None:
        body = document()
        body["fixtures"][0]["sha256"] = ""
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("must carry its SHA-256", err)
        body = document()
        body["fixtures"][0]["generator_argv"] = []
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("must carry its generator argv", err)

    def test_corrections_are_required(self) -> None:
        body = document(corrections=[])
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("pre-fix observation must be preserved", err)

    def test_presentation_and_copy_endpoints_cannot_be_observed(self) -> None:
        body = document()
        body["endpoints"][0]["status"] = "OBSERVED"
        body["endpoints"][0]["reason"] = ""
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("cannot observe these endpoints", err)

    def test_duplicate_interop_ids_and_missing_endpoints_are_refused(self) -> None:
        body = document()
        body["interop"].append(dict(body["interop"][0]))
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("duplicate interop id", err)

        body = document()
        body["endpoints"] = body["endpoints"][:-1]
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("missing endpoint", err)

    def test_observed_row_needs_its_pixel_format_and_absolute_tool(self) -> None:
        body = document()
        body["interop"][0]["pixel_format"] = ""
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("must name the media pixel format", err)

        body = document()
        body["interop"][0]["argv"] = ["ffmpeg.exe", "-i", "fixture.mp4"]
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("absolute local tool path", err)

    def test_probe_path_must_be_absolute_and_present(self) -> None:
        code, _out, err = self.run_cli("vid1_probe.ps1")
        self.assertIn("REFUSED", err)
        self.assertEqual(code, 2)
        code, _out, err = self.run_cli(str(self.probes.root / "missing.ps1"))
        self.assert_refused(code, err, "PROBE_NOT_FOUND")

    def test_probe_changed_while_running_is_refused(self) -> None:
        probe = self.probe_for(document(), touch_self=True)
        code, _out, err = self.run_cli(probe)
        self.assert_refused(code, err, "PROBE_CHANGED")

    def test_out_path_must_be_absolute(self) -> None:
        probe = self.probe_for(document())
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = vid1.main(
                [
                    "--probe",
                    probe,
                    "--powershell",
                    PWSH,
                    "--out",
                    "relative-record.json",
                    "--timeout-ms",
                    str(CAPS["timeout_ms"]),
                    "--max-output-bytes",
                    str(CAPS["max_output_bytes"]),
                    "--max-fixture-bytes",
                    str(CAPS["max_fixture_bytes"]),
                ]
            )
        self.assertEqual(code, 2)
        self.assertIn("REFUSED OUT_NOT_ABSOLUTE", err.getvalue())
        self.assertFalse((HERE / "relative-record.json").exists())

    # -- BLOCKING (previous round, unchanged): bounded capture ---------------
    def test_stdout_capture_cap_is_enforced_while_reading(self) -> None:
        body, length = self.with_cap(document())
        code, _out, err = self.run_cli(self.probe_for(body), caps=self.caps_with(length))
        self.assertEqual(code, 0, f"a document exactly at the cap must be accepted: {err}")
        code, _out, err = self.run_cli(self.probe_for(body), caps=self.caps_with(length - 1))
        self.assert_refused(code, err, "PROBE_OUTPUT_OVERSIZED")

    def test_stdout_capture_cap_kills_a_flooding_probe(self) -> None:
        marker = self.probes.root / "stdout.post-marker.txt"
        probe = self.probes.write_script(
            "flood-out.ps1",
            [
                "$ErrorActionPreference = 'Stop'",
                "[Console]::Out.Write(('x' * 500000))",
                "Start-Sleep -Milliseconds 2000",
                f"Set-Content -LiteralPath '{marker}' -Value 'done'",
                "exit 0",
            ],
        )
        started = time.monotonic()
        code, _out, err = self.run_cli(
            probe, caps={"timeout_ms": 1000, "max_output_bytes": 4096, "max_fixture_bytes": 1048576}
        )
        elapsed = time.monotonic() - started
        self.assert_refused(code, err, "PROBE_OUTPUT_OVERSIZED")
        self.assertLess(elapsed, 4, "the child must be terminated on overflow, not waited out")
        self.assertFalse(marker.exists(), "the flooder reached its post-cap write, so it was not killed")

    def test_stderr_capture_cap_is_enforced_while_reading(self) -> None:
        body, length = self.with_cap(document())
        caps = self.caps_with(length)
        payload = base64.b64encode(json.dumps(body, ensure_ascii=False).encode("utf-8")).decode("ascii")
        under = self.probes.write_script(
            "stderr-under.ps1",
            [
                "$ErrorActionPreference = 'Stop'",
                f"[Console]::Error.Write(('e' * {length - 1}))",
                f"$json = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{payload}'))",
                "[Console]::Out.Write($json)",
                "exit 0",
            ],
        )
        code, _out, err = self.run_cli(under, caps=caps)
        self.assertEqual(code, 0, f"stderr just below the cap must be accepted: {err}")

        marker = self.probes.root / "stderr.post-marker.txt"
        over = self.probes.write_script(
            "stderr-over.ps1",
            [
                "$ErrorActionPreference = 'Stop'",
                "[Console]::Error.Write(('e' * 500000))",
                "Start-Sleep -Milliseconds 2000",
                f"Set-Content -LiteralPath '{marker}' -Value 'done'",
                "exit 0",
            ],
        )
        started = time.monotonic()
        code, _out, err = self.run_cli(over, caps=caps)
        elapsed = time.monotonic() - started
        self.assert_refused(code, err, "PROBE_STDERR_OVERSIZED")
        self.assertLess(elapsed, 4, "the child must be terminated on stderr overflow")
        self.assertFalse(marker.exists(), "the flooder reached its post-cap write, so it was not killed")

    @unittest.skipUnless(PROBE.is_file(), "the real probe script is required for the child-capture test")
    def test_probe_kills_a_flooding_child_and_records_the_cap(self) -> None:
        scratch = self.probes.root / "scratch"
        scratch.mkdir()
        flooder, marker = self.probes.write_flooder("flood.cmd")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = vid1.main(
                [
                    "--probe",
                    str(PROBE),
                    "--powershell",
                    PWSH,
                    "--ffmpeg",
                    flooder,
                    "--scratch",
                    str(scratch),
                    "--out",
                    str(self.out),
                    "--timeout-ms",
                    "15000",
                    "--max-output-bytes",
                    "65536",
                ]
            )
        self.assertEqual(code, 0, err.getvalue())
        record = self.record()
        reasons = {row["reason"] for row in record["fixtures"]}
        self.assertIn("PROCESS_OUTPUT_CAPPED", reasons)
        self.assertFalse(marker.exists(), "a capped child must be killed before it can finish")


class VocabularyTests(unittest.TestCase):
    def test_reason_vocabulary_is_closed_and_documented(self) -> None:
        self.assertIn("HWACCEL_SOFTWARE_FALLBACK", vid1.REASONS)
        self.assertIn("PRESENTATION_PATH_UNMEASURED", vid1.REASONS)
        self.assertIn("PIN_API_NOT_INSPECTED", vid1.REASONS)
        self.assertIn("SOURCE_SCAN_INSUFFICIENT", vid1.REASONS)
        self.assertNotIn("SOMETHING_NEW", vid1.REASONS)
        self.assertEqual(set(vid1.UNOBSERVABLE_ENDPOINTS.values()) - vid1.REASONS, set())
        self.assertEqual(set(vid1.MFT_FLAGS.values()), {"0x1", "0x2", "0x4", "0x3F"})
        self.assertIn("COMPLETE_BOUND", vid1.SCAN_SIGNALS)
        self.assertIn("PIN_API_INSPECTION", vid1.MISSING_EVIDENCE)

    def test_every_refusal_code_is_registered(self) -> None:
        for code in vid1.REFUSAL_CODES:
            self.assertRegex(code, r"^[A-Z][A-Z0-9_]+$")


@unittest.skipUnless(
    os.environ.get("COS_VID1_REAL_HOST") == "1" and PWSH and PROBE.is_file() and FFMPEG.is_file(),
    "real host probe run is opt-in via COS_VID1_REAL_HOST=1 (needs pwsh, the probe and FFmpeg)",
)
class RealHostProbeTests(unittest.TestCase):
    """One real capability run; opt-in so the default suite stays hermetic and fast."""

    def test_real_host_record(self) -> None:
        root = Path(tempfile.mkdtemp(prefix="vid1-real-"))
        self.addCleanup(shutil.rmtree, root, True)
        out = root / "record.json"
        err = io.StringIO()
        extra = ["--git", str(GIT)] if GIT.is_file() else []
        with contextlib.redirect_stderr(err):
            code = vid1.main(
                [
                    "--probe",
                    str(PROBE),
                    "--powershell",
                    PWSH,
                    "--ffmpeg",
                    str(FFMPEG),
                    "--scratch",
                    str(root),
                    "--source-root",
                    str(REPO_ROOT),
                    *extra,
                    "--out",
                    str(out),
                ]
            )
        self.assertEqual(code, 0, err.getvalue())
        record = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(record["task"], "MP-VID1")
        self.assertGreaterEqual(record["summary"]["interop_rows"], 10)
        self.assertGreaterEqual(record["summary"]["fixtures"], 1)
        self.assertEqual(record["mft_flags"]["hardware"], "0x4")
        self.assertGreaterEqual(record["source_scan"]["files_scanned"], 1)
        # The honest outcome: no route conclusion from a scan; the pin API was not inspected.
        self.assertEqual(record["route"]["status"], "UNEXAMINED")
        self.assertEqual(record["route"]["reason"], "PIN_API_NOT_INSPECTED")
        self.assertIn("PIN_API_INSPECTION", record["route"]["missing_evidence"])
        self.assertEqual(record["pin_api_inspection"]["status"], "NOT_RUN")
        self.assertIn(
            record["source_scan"]["signal"],
            {"COMPLETE_BOUND", "SHA_BOUND_TREE_STATE_UNKNOWN", "TRUNCATED", "UNBOUND"},
        )
        for row in record["interop"]:
            if row["status"] in {"NOT_MEASURED", "UNSUPPORTED"}:
                self.assertTrue(row["reason"], row["id"])
            if row["kind"] in {"hw_decode", "hw_download"} and row["status"] == "OBSERVED":
                self.assertEqual(row["hardware_frame_evidence"], "PIX_FMT_OBSERVED")
                self.assertIn(row["hardware_pix_fmt"], row["decoder_pix_fmts"])
        for fixture in record["fixtures"]:
            if fixture["status"] == "OBSERVED":
                self.assertRegex(fixture["sha256"], r"^[0-9a-f]{64}$")
                self.assertTrue(fixture["generator_argv"])
        self.assertEqual(record["summary"]["endpoints_not_measured"], len(vid1.UNOBSERVABLE_ENDPOINTS))


if __name__ == "__main__":
    unittest.main()
