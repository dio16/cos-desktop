"""MP-VID1 capability-record tests: closed schema, fail-closed refusals and derivation.

Every test drives the real CLI path with a real PowerShell interpreter and a generated
probe script (`--probe` is the documented seam), so the runner's own spawn/hash/cap logic
is exercised rather than mocked. The probe's *host* observations are represented by the
scripted documents; the real capability run is a separate, explicitly opted-in test.
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
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
import vid1_capability as vid1  # noqa: E402

PWSH = shutil.which("pwsh") or shutil.which("pwsh.exe")
PROBE = HERE / "vid1_probe.ps1"
FFMPEG = Path(r"C:\ffmpeg\bin\ffmpeg.exe")
CAPS = {"timeout_ms": 1000, "max_output_bytes": 4096, "max_fixture_bytes": 1048576}


def document(**overrides) -> dict:
    """A schema-valid probe document; each test overrides only the field it exercises."""
    body = {
        "ok": True,
        "probe_version": 1,
        "caps": dict(CAPS, max_experiments=24),
        "host": {
            "status": "OBSERVED",
            "reason": "",
            "os_description": "Microsoft Windows 10.0.26300",
            "arch": "X64",
            "pwsh": "7.6.7",
            "interop_enabled": True,
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
                    "all_status": "OBSERVED",
                    "all": 17,
                    "hardware_status": "OBSERVED",
                    "hardware": 0,
                    "sync": 16,
                    "reason": "",
                    "hresult": "0x00000000",
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
                "width": 640,
                "height": 360,
                "seconds": 1,
            }
        ],
        "interop": [
            {
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
                "truncated": False,
                "frames_decoded": 2,
                "decoder_pix_fmts": [],
                "dxva_profile_guids": [],
                "output_bytes": None,
                "expected_output_bytes": None,
            }
        ],
        "endpoints": [
            {"endpoint": name, "status": "NOT_MEASURED", "reason": why}
            for name, why in vid1.UNOBSERVABLE_ENDPOINTS.items()
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


def hw_row(**overrides) -> dict:
    row = {
        "id": "hw_d3d11va_h264_decode",
        "kind": "hw_decode",
        "container": "mp4",
        "codec": "h264",
        "pixel_format": "yuv420p",
        "hwaccel": "d3d11va",
        "argv": ["C:\\ffmpeg\\bin\\ffmpeg.exe", "-hwaccel", "d3d11va", "-i", "fixture.mp4", "-f", "null", "-"],
        "exit_code": 0,
        "wall_ms": 400,
        "status": "OBSERVED",
        "reason": "",
        "stdout_bytes": 0,
        "stderr_bytes": 1024,
        "truncated": False,
        "frames_decoded": 5,
        "decoder_pix_fmts": ["yuv420p", "d3d11"],
        "dxva_profile_guids": [],
        "output_bytes": None,
        "expected_output_bytes": None,
    }
    row.update(overrides)
    return row


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


@unittest.skipUnless(PWSH, "PowerShell 7 is required for the documented --probe seam")
class CapabilityRecordTests(unittest.TestCase):
    def setUp(self) -> None:
        self.probes = TempProbes()
        self.addCleanup(self.probes.cleanup)
        self.out = self.probes.root / "record.json"

    # -- helpers -----------------------------------------------------------------
    def run_cli(self, probe: str, *extra: str, caps: dict | None = None) -> tuple[int, str, str]:
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
                    "host",
                    "media_foundation",
                    "d3d11",
                    "tool",
                    "fixtures",
                    "interop",
                    "endpoints",
                    "summary",
                    "vid2_material",
                    "caps_requested",
                }
            ),
        )
        self.assertEqual(record["task"], "MP-VID1")
        self.assertEqual(record["record_version"], 1)
        self.assertEqual(record["probe"]["exit_code"], 0)
        self.assertRegex(record["probe"]["probe_sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(record["probe"]["powershell"], PWSH)
        self.assertEqual(record["media_foundation"]["mft_enumeration"][0]["all"], 17)
        self.assertEqual(record["summary"]["interop_rows"], 1)
        self.assertEqual(record["summary"]["interop_observed"], 1)
        self.assertEqual(record["summary"]["interop_not_measured"], 0)
        self.assertEqual(record["summary"]["interop_unsupported"], 0)
        self.assertEqual(record["summary"]["fixtures_observed"], 1)
        self.assertIn("record=", out)
        # No decision field may ever appear: VID1 reports, MP-VID2 decides.
        self.assertNotIn("decision", json.dumps(record))

    def test_absent_capability_is_carried_with_its_reason_and_no_invented_value(self) -> None:
        body = document(
            tool={"status": "NOT_MEASURED", "reason": "TOOL_NOT_SUPPLIED", "path": "", "sha256": "", "hwaccels": []},
            interop=[
                {
                    "id": "interop_disabled",
                    "kind": "decode",
                    "container": "",
                    "codec": "",
                    "pixel_format": "",
                    "hwaccel": "",
                    "argv": [],
                    "exit_code": None,
                    "wall_ms": None,
                    "status": "NOT_MEASURED",
                    "reason": "INTEROP_DISABLED",
                    "stdout_bytes": 0,
                    "stderr_bytes": 0,
                    "truncated": False,
                    "frames_decoded": None,
                    "decoder_pix_fmts": [],
                    "dxva_profile_guids": [],
                    "output_bytes": None,
                    "expected_output_bytes": None,
                }
            ],
        )
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assertEqual(code, 0, err)
        record = self.record()
        row = record["interop"][0]
        self.assertEqual(row["status"], "NOT_MEASURED")
        self.assertEqual(row["reason"], "INTEROP_DISABLED")
        self.assertIsNone(row["exit_code"])
        self.assertIsNone(row["wall_ms"])
        self.assertIsNone(row["frames_decoded"])
        self.assertEqual(record["tool"]["path"], "")
        self.assertIn(
            "endpoint:presentation_route NOT_MEASURED PRESENTATION_PATH_UNMEASURED",
            record["vid2_material"]["unavailable"],
        )

    def test_derivation_counts_and_tradeoffs_follow_only_observed_rows(self) -> None:
        code, _out, err = self.run_cli(self.probe_for(document(tool=observed_tool(), interop=[hw_row()])))
        self.assertEqual(code, 0, err)
        record = self.record()
        self.assertEqual(record["summary"]["hardware_decode_observed"], 1)
        self.assertEqual(record["summary"]["hardware_download_observed"], 0)
        tradeoffs = " ".join(record["vid2_material"]["tradeoffs"])
        self.assertIn("no video route is selected", tradeoffs)
        self.assertIn("importing those frames into the pinned GPUI revision remains unverified", tradeoffs)
        self.assertIn("hw_decode:hw_d3d11va_h264_decode codec=h264 hwaccel=d3d11va", record["vid2_material"]["observed_paths"][0])

    # -- fail-closed refusals ----------------------------------------------------
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
        for mutate, fragment in (
            (lambda body: body["host"].update(status="PROBABLY_FINE"), "unregistered status"),
            (lambda body: body["interop"][0].update(kind="streaming"), "unregistered kind"),
            (lambda body: body["endpoints"].append({"endpoint": "user_visible_quality", "status": "NOT_MEASURED", "reason": "PROCESS_FAILED"}), "unregistered endpoint"),
        ):
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
        body = document(host=dict(document()["host"], os_description="x" * 8000))
        code, _out, err = self.run_cli(self.probe_for(body))
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
        code, _out, err = self.run_cli(probe, caps={"timeout_ms": 2000, "max_output_bytes": 4096, "max_fixture_bytes": 1048576})
        self.assert_refused(code, err, "CAPS_MISMATCH")

    def test_hardware_decode_without_hardware_evidence_is_refused(self) -> None:
        body = document(interop=[hw_row(decoder_pix_fmts=["yuv420p"], dxva_profile_guids=[])])
        code, _out, err = self.run_cli(self.probe_for(body))
        self.assert_refused(code, err, "PROBE_DOCUMENT_INVALID")
        self.assertIn("requires hardware frames or DXVA GUIDs", err)

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


class VocabularyTests(unittest.TestCase):
    def test_reason_vocabulary_is_closed_and_documented(self) -> None:
        self.assertIn("HWACCEL_SOFTWARE_FALLBACK", vid1.REASONS)
        self.assertIn("PRESENTATION_PATH_UNMEASURED", vid1.REASONS)
        self.assertNotIn("SOMETHING_NEW", vid1.REASONS)
        self.assertEqual(set(vid1.UNOBSERVABLE_ENDPOINTS.values()) - vid1.REASONS, set())

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
                    "--out",
                    str(out),
                ]
            )
        self.assertEqual(code, 0, err.getvalue())
        record = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(record["task"], "MP-VID1")
        self.assertGreaterEqual(record["summary"]["interop_rows"], 10)
        self.assertGreaterEqual(record["summary"]["fixtures"], 1)
        for row in record["interop"]:
            if row["status"] == "NOT_MEASURED" or row["status"] == "UNSUPPORTED":
                self.assertTrue(row["reason"], row["id"])
        self.assertEqual(record["summary"]["endpoints_not_measured"], len(vid1.UNOBSERVABLE_ENDPOINTS))


if __name__ == "__main__":
    unittest.main()
