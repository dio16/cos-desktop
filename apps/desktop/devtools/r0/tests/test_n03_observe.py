"""N03 independent observation tool: argument, truncation and closed-error coverage.

The OS-dependent path (Win32 window enumeration, UIAutomationClient, screen capture) needs
a desktop session and the UIAutomationClient assembly; those cases report NOT_RUN instead
of being simulated.
"""
from __future__ import annotations

import contextlib
import io
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import native_observe as observe  # noqa: E402

WINDOW = {"status": "OBSERVED", "title": "COS Desktop", "pid": 4242, "handle": 7,
          "left": 10, "top": 20, "width": 640, "height": 480}
CAPS = {"max_elements": 4, "max_depth": 6, "max_screenshot_bytes": 1024 * 1024}


def element(depth: int, name: str = "node") -> dict:
    return {"depth": depth, "name": name, "control_type": "ControlType.Pane",
            "automation_id": f"id-{depth}", "focused": False,
            "left": 0, "top": 0, "width": 1, "height": 1}


def probe_document(elements: list[dict] | None = None, *, truncated: bool = False,
                   uia_status: str = "OBSERVED", width: int = 640,
                   height: int = 480) -> dict:
    return {
        "ok": True,
        "window": WINDOW,
        "screenshot": {"status": "OBSERVED", "width": width, "height": height,
                       "bytes": 100},
        "uia": {"status": uia_status, "reason": "" if uia_status == "OBSERVED" else "UIA_UNAVAILABLE",
                "elements": elements or [], "focused": None, "truncated": truncated,
                "depth_limited": False, "elements_visited": len(elements or [])},
        "caps": CAPS,
    }


def png_bytes(width: int = 640, height: int = 480) -> bytes:
    """A minimal valid PNG, so the digest and the IHDR dimensions are real."""
    def chunk(kind: bytes, payload: bytes) -> bytes:
        body = kind + payload
        return (struct.pack(">I", len(payload)) + body
                + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF))
    raw = b"".join(b"\x00" + b"\x00\x00\x00" * width for _ in range(height))
    return (observe.PNG_SIGNATURE
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw))
            + chunk(b"IEND", b""))


class N03Observation(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)
        self.screenshot = self.base / "shot.png"
        self.record = self.base / "record.json"

    # ------------------------------------------------------------- arguments
    def invoke(self, *arguments: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-B", str(ROOT / "native_observe.py"), *arguments],
            capture_output=True, check=False, timeout=60)

    def test_argument_validation_is_closed(self) -> None:
        cases = {
            ("--out", str(self.record), "--screenshot", str(self.screenshot)): "ARGS_TARGET",
            ("--title", "x", "--pid", "1", "--out", str(self.record),
             "--screenshot", str(self.screenshot)): "ARGS_TARGET",
            ("--title", "x", "--out", str(self.record), "--screenshot", str(self.screenshot),
             "--max-elements", "0"): "ARGS_CAP",
            ("--title", "x", "--out", str(self.record), "--screenshot", str(self.screenshot),
             "--max-depth", "999"): "ARGS_CAP",
            ("--title", "x", "--out", str(self.record), "--screenshot", str(self.screenshot),
             "--max-screenshot-bytes", "1"): "ARGS_CAP",
        }
        for arguments, code in cases.items():
            result = self.invoke(*arguments)
            self.assertEqual(result.returncode, 1, arguments)
            self.assertEqual(json.loads(result.stdout)["error"], code, arguments)
            self.assertNotIn(b"Traceback", result.stderr, arguments)

    def test_help_exits_zero(self) -> None:
        result = self.invoke("--help")
        self.assertEqual(result.returncode, 0)
        self.assertIn(b"--max-elements", result.stdout)

    def test_missing_powershell_is_a_closed_error(self) -> None:
        result = self.invoke("--title", "COS Desktop", "--out", str(self.record),
                             "--screenshot", str(self.screenshot),
                             "--powershell", str(self.base / "no-such-powershell.exe"))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout)["error"], "PROBE_MISSING")
        self.assertNotIn(b"Traceback", result.stderr)

    # ------------------------------------------------------------- truncation
    def test_element_cap_truncation_is_recorded(self) -> None:
        self.screenshot.write_bytes(png_bytes())
        document = probe_document([element(index) for index in range(CAPS["max_elements"])],
                                  truncated=True)
        record = observe.build_record(document, self.screenshot, CAPS)
        self.assertEqual(len(record["uia"]["elements"]), CAPS["max_elements"])
        self.assertTrue(record["uia"]["truncated"])
        self.assertEqual(record["uia"]["status"], "OBSERVED")
        self.assertTrue(record["uia"]["digest"].startswith("sha256:"))

    def test_over_cap_document_is_refused(self) -> None:
        self.screenshot.write_bytes(png_bytes())
        document = probe_document([element(index) for index in range(CAPS["max_elements"] + 1)])
        with self.assertRaises(observe.ObservationError) as error:
            observe.build_record(document, self.screenshot, CAPS)
        self.assertEqual(error.exception.code, "SCHEMA")

    # ------------------------------------------------------------- screenshot
    def test_screenshot_digest_and_dimensions(self) -> None:
        payload = png_bytes(320, 240)
        self.screenshot.write_bytes(payload)
        document = probe_document(width=320, height=240)
        record = observe.build_record(document, self.screenshot, CAPS)
        self.assertEqual(record["screenshot"]["width"], 320)
        self.assertEqual(record["screenshot"]["height"], 240)
        self.assertEqual(record["screenshot"]["bytes"], len(payload))
        self.assertEqual(record["screenshot"]["digest"],
                         "sha256:" + __import__("hashlib").sha256(payload).hexdigest())

    def test_screenshot_dimension_mismatch_is_refused(self) -> None:
        self.screenshot.write_bytes(png_bytes(320, 240))
        document = probe_document(width=640, height=480)
        with self.assertRaises(observe.ObservationError) as error:
            observe.build_record(document, self.screenshot, CAPS)
        self.assertEqual(error.exception.code, "SCREENSHOT_MISMATCH")

    def test_refused_screenshot_is_not_measured(self) -> None:
        document = probe_document()
        document["screenshot"] = {"status": "NOT_MEASURED", "reason": "SCREENSHOT_FAILED"}
        record = observe.build_record(document, self.screenshot, CAPS)
        self.assertEqual(record["screenshot"],
                         {"status": "NOT_MEASURED", "reason": "SCREENSHOT_FAILED"})
        self.assertTrue(record["ok"], "a refused capability still yields a record")

    def test_refused_uia_is_not_measured(self) -> None:
        self.screenshot.write_bytes(png_bytes())
        record = observe.build_record(probe_document(uia_status="NOT_MEASURED"),
                                      self.screenshot, CAPS)
        self.assertEqual(record["uia"]["status"], "NOT_MEASURED")
        self.assertEqual(record["uia"]["reason"], "UIA_UNAVAILABLE")
        self.assertIsNone(record["uia"]["digest"])
        self.assertEqual(record["uia"]["elements"], [])

    # ------------------------------------------------------------- closed errors
    def run_with_probe(self, failure: str) -> tuple[int, str]:
        """Run main() with the probe replaced by a failure; returns (exit, stdout)."""
        original = observe.run_probe
        self.addCleanup(setattr, observe, "run_probe", original)
        observe.run_probe = lambda command: observe.fail(failure)
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = observe.main(["--title", "COS Desktop", "--out", str(self.record),
                                 "--screenshot", str(self.screenshot)])
        return code, buffer.getvalue()

    def test_probe_failure_codes_pass_through(self) -> None:
        for code in ("WINDOW_NOT_FOUND", "WINDOW_RECT_UNAVAILABLE", "WIN32_UNAVAILABLE"):
            exit_code, output = self.run_with_probe(code)
            self.assertEqual(exit_code, 1, code)
            self.assertEqual(json.loads(output)["error"], code, code)

    def test_malformed_probe_output_is_closed(self) -> None:
        exit_code, output = self.run_with_probe("PROBE_JSON")
        self.assertEqual(exit_code, 1)
        self.assertEqual(json.loads(output)["error"], "PROBE_JSON")

    def test_unknown_schema_is_refused(self) -> None:
        self.screenshot.write_bytes(png_bytes())
        broken = probe_document()
        broken["window"] = dict(WINDOW, extra=1)
        with self.assertRaises(observe.ObservationError) as error:
            observe.build_record(broken, self.screenshot, CAPS)
        self.assertEqual(error.exception.code, "SCHEMA")

    # ------------------------------------------------- probe contract regressions
    def fake_probe(self, *, returncode: int, stdout: bytes):
        """Replace subprocess.run inside the module; returns the CompletedProcess shape."""
        original = observe.subprocess.run
        self.addCleanup(setattr, observe.subprocess, "run", original)

        class Done:
            pass

        done = Done()
        done.returncode = returncode
        done.stdout = stdout
        done.stderr = b""
        observe.subprocess.run = lambda *args, **kwargs: done
        return done

    def test_nonzero_probe_exit_is_refused(self) -> None:
        """Valid `ok: true` JSON from a process that exits non-zero is not evidence."""
        self.fake_probe(returncode=3,
                        stdout=json.dumps(probe_document([element(0)])).encode())
        with self.assertRaises(observe.ObservationError) as error:
            observe.run_probe(["powershell", "-File", "probe.ps1"])
        self.assertEqual(error.exception.code, "PROBE_FAILED")

    def test_unknown_probe_code_is_closed(self) -> None:
        """An unregistered code must become closed JSON, never an assertion error."""
        self.fake_probe(returncode=0,
                        stdout=b'{"ok":false,"error":"UNKNOWN_ERROR"}')
        with self.assertRaises(observe.ObservationError) as error:
            observe.run_probe(["powershell", "-File", "probe.ps1"])
        self.assertEqual(error.exception.code, "PROBE_FAILED")
        # And through the CLI it is closed JSON with no traceback.
        result = self.invoke("--title", "COS Desktop", "--out", str(self.record),
                             "--screenshot", str(self.screenshot))
        self.assertNotIn(b"Traceback", result.stderr)

    def test_valid_png_with_exit_zero_is_observed(self) -> None:
        self.fake_probe(returncode=0,
                        stdout=json.dumps(probe_document([element(0)])).encode())
        self.screenshot.write_bytes(png_bytes())
        document = observe.run_probe(["powershell", "-File", "probe.ps1"])
        record = observe.build_record(document, self.screenshot, CAPS)
        self.assertEqual(record["screenshot"]["status"], "OBSERVED")
        self.assertEqual(record["uia"]["status"], "OBSERVED")

    def test_oversize_screenshot_is_refused(self) -> None:
        payload = png_bytes()
        self.screenshot.write_bytes(payload)
        caps = dict(CAPS, max_screenshot_bytes=len(payload) - 1)
        document = probe_document()
        document["caps"] = caps  # the probe reports the caps it was given
        with self.assertRaises(observe.ObservationError) as error:
            observe.build_record(document, self.screenshot, caps)
        self.assertEqual(error.exception.code, "SCREENSHOT_TOO_LARGE")

    # ------------------------------------------------- genuine CLI fail-closed
    def fake_probe_script(self, body: str) -> Path:
        """A real PowerShell probe, so the CLI runs it exactly as it runs the packaged one."""
        if shutil.which("powershell") is None:
            self.skipTest("NOT_RUN: Windows PowerShell is not on PATH")
        path = self.base / "fake_probe.ps1"
        path.write_text(body, encoding="utf-8")
        return path

    def run_cli_with_probe(self, probe: Path) -> tuple[subprocess.CompletedProcess, Path]:
        record = self.base / "never-written.json"
        result = subprocess.run(
            [sys.executable, "-B", str(ROOT / "native_observe.py"),
             "--title", "COS Desktop", "--probe", str(probe),
             "--out", str(record), "--screenshot", str(self.screenshot)],
            capture_output=True, check=False, timeout=120)
        return result, record

    def test_cli_refuses_a_probe_that_reports_success_then_fails(self) -> None:
        """Valid `ok: true` JSON followed by exit 3 must not become an observation."""
        document = json.dumps(probe_document([element(0)]))
        probe = self.fake_probe_script(
            "Write-Output '" + document + "'\nexit 3\n")
        result, record = self.run_cli_with_probe(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(result.stdout.decode().strip(), '{"ok":false,"error":"PROBE_FAILED"}',
                         "exactly one closed JSON object")
        self.assertEqual(result.stdout.count(b"\n"), 1, "a single line of output")
        self.assertNotIn(b"Traceback", result.stderr)
        self.assertFalse(record.exists(), "no observation file may be written")

    def test_cli_refuses_an_unregistered_probe_code(self) -> None:
        """An unknown probe code becomes closed JSON, not a traceback."""
        probe = self.fake_probe_script(
            "Write-Output '{\"ok\":false,\"error\":\"UNKNOWN_ERROR\"}'\nexit 0\n")
        result, record = self.run_cli_with_probe(probe)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual(result.stdout.decode().strip(), '{"ok":false,"error":"PROBE_FAILED"}')
        self.assertNotIn(b"Traceback", result.stderr)
        self.assertFalse(record.exists())

    # ------------------------------------------------------------- OS path
    def test_real_observation_of_a_window(self) -> None:
        """Needs a desktop session, PowerShell and a window that is not ours."""
        if os.name != "nt":
            self.skipTest("NOT_RUN: Win32 window enumeration requires Windows")
        probe = subprocess.run(["where", "powershell"], capture_output=True, check=False)
        if probe.returncode != 0:
            self.skipTest("NOT_RUN: Windows PowerShell is not on PATH")
        listing = subprocess.run(["tasklist"], capture_output=True, check=False)
        if b"cos-desktop" not in listing.stdout.lower():
            self.skipTest("NOT_RUN: no COS Desktop window is running in this session")
        result = self.invoke("--title", "COS Desktop", "--out", str(self.record),
                             "--screenshot", str(self.screenshot))
        self.assertEqual(result.returncode, 0, result.stdout)
        record = json.loads(self.record.read_text(encoding="utf-8"))
        self.assertTrue(record["ok"])
        self.assertIn("COS Desktop", record["window"]["title"])
        self.assertEqual(record["screenshot"]["status"], "OBSERVED")


if __name__ == "__main__":
    unittest.main()
