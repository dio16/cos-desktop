#!/usr/bin/env python3
"""Test-only UIA focus-event audit harness (AX F1 evidence gap).

The native AX audit could not observe the focused control: ``accevent.exe``
needs elevation and the probe's ``AutomationElement.FocusedElement`` reads came
back empty once the window was foregrounded. This driver measures the same
question from an ordinary same-integrity UIA *client* instead of the SDK tool:

* ``uia_focus_listener.cs`` is compiled with the .NET Framework ``csc`` and
  subscribes to ``Automation.AddAutomationFocusChangedEventHandler``
  (``UIA_AutomationFocusChangedEventId``). It records every event with a UTC
  timestamp, runtime id, name, control type, class, automation id, process id
  and keyboard-focus facts, and independently polls
  ``AutomationElement.FocusedElement`` so the focused element after each input
  step is observable even when no event fires.
* ``uia_focus_control.cs`` is a WinForms window that moves focus between three
  named controls on a timer. It runs first in the same session and proves the
  listener sees real focus transitions on this host, so a negative result
  against the product window is attributable to the product provider and not to
  the measurement path.
* The release Desktop then runs in the MP-ACC1 native lane (fresh DB copy, own
  secrets, port 8166, discovery only, no generation) and the harness sends
  real ``SendInput`` Tab / Shift+Tab / Ctrl+N / Ctrl+K sequences while the
  listener records. ``probe_window.ps1`` supplies the UIA tree dumps,
  screenshots and per-step wall-clock boundaries.

The build outputs stay under ``target/`` (gitignored); the evidence directory
keeps the exact sources, the build manifest with hashes and the raw logs.

It is a development/integration helper, not a product entry point.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
REPO_ROOT = HARNESS.parents[3]
sys.path.insert(0, str(HARNESS))

import run_integration as base  # noqa: E402 - local test helper, stdlib only


def _load_acc1():
    """Load the MP-ACC1 native lane, falling back to the committed version.

    A concurrent editor can leave the shared helper unparsable in the worktree;
    this measurement then runs against the same file at HEAD instead of failing
    on someone else's in-flight edit.
    """
    try:
        import run_acc1_native as module
        return module, "worktree"
    except SyntaxError:
        import importlib.util
        import tempfile
        head = subprocess.run(
            ["git", "show",
             "HEAD:apps/desktop/devtools/runtime_integration/run_acc1_native.py"],
            cwd=str(REPO_ROOT), capture_output=True, text=True, encoding="utf-8",
            errors="replace")
        if head.returncode != 0 or not head.stdout:
            raise
        path = Path(tempfile.gettempdir()) / f"run_acc1_native_head_{os.getpid()}.py"
        path.write_text(head.stdout, encoding="utf-8")
        spec = importlib.util.spec_from_file_location("run_acc1_native_head", path)
        module = importlib.util.module_from_spec(spec)
        sys.modules["run_acc1_native_head"] = module
        spec.loader.exec_module(module)
        return module, "head"


acc1, ACC1_SOURCE = _load_acc1()

EVIDENCE_DEFAULT = (
    acc1.ROOT / "docs" / "development" / "evidence-published" / "uia-focus-events-latest"
)
PORT = 8166
LISTENER_SOURCE = HARNESS / "uia_focus_listener.cs"
CONTROL_SOURCE = HARNESS / "uia_focus_control.cs"
CSC = Path("C:/Windows/Microsoft.NET/Framework64/v4.0.30319/csc.exe")
GAC_MSIL = Path("C:/Windows/Microsoft.NET/assembly/GAC_MSIL")
UIA_REFERENCES = {
    "UIAutomationClient": GAC_MSIL / "UIAutomationClient/v4.0_4.0.0.0__31bf3856ad364e35/UIAutomationClient.dll",
    "UIAutomationTypes": GAC_MSIL / "UIAutomationTypes/v4.0_4.0.0.0__31bf3856ad364e35/UIAutomationTypes.dll",
    "WindowsBase": GAC_MSIL / "WindowsBase/v4.0_4.0.0.0__31bf3856ad364e35/WindowsBase.dll",
}


def now_utc() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"


def parse_utc(value: str) -> datetime:
    return datetime.fromisoformat(str(value).replace("Z", "+00:00"))


def read_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}


def read_jsonl(path: Path) -> list[dict]:
    records: list[dict] = []
    if not path.is_file():
        return records
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            records.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return records


class FocusAudit:
    """One bounded focus-event session over the native window."""

    evidence_dir: Path

    def __init__(self, evidence: Path) -> None:
        self.evidence = evidence
        self.uia_dir = evidence / "uia"
        self.shot_dir = evidence / "shots"
        self.source_dir = evidence / "listener"
        self.build_dir = acc1.ROOT / "target" / "ax-focus-build" / time.strftime("%Y%m%dT%H%M%S")
        self.uia_dir.mkdir(parents=True, exist_ok=True)
        self.shot_dir.mkdir(parents=True, exist_ok=True)
        self.source_dir.mkdir(parents=True, exist_ok=True)
        self.build_dir.mkdir(parents=True, exist_ok=True)
        FocusAudit.evidence_dir = evidence
        self.calls = 0
        self.pid = 0
        self.hwnd = 0
        self.listener_log: Path | None = None
        self.app_names: set[str] = set()
        self.listener_exe = self.build_dir / "uia_focus_listener.exe"
        self.control_exe = self.build_dir / "uia_focus_control.exe"
        self.build: dict = {}

    # -- build -----------------------------------------------------------------
    def compile(self) -> dict:
        """Compile the listener and control target with the .NET Framework csc."""
        commands = []
        for source, output, references in (
            (LISTENER_SOURCE, self.listener_exe, list(UIA_REFERENCES.values())),
            (CONTROL_SOURCE, self.control_exe, []),
        ):
            command = [str(CSC), "-nologo", f"-out:{output}", str(source)]
            command += [f"-r:{reference}" for reference in references]
            completed = subprocess.run(command, capture_output=True, text=True,
                                       timeout=180, encoding="utf-8", errors="replace")
            commands.append({
                "command": [str(part) for part in command],
                "exit": completed.returncode,
                "stdout": completed.stdout[-800:],
                "stderr": completed.stderr[-800:],
            })
            if completed.returncode != 0 or not output.is_file():
                raise RuntimeError(f"csc failed for {source.name}: {completed.stderr[-400:]}")
        self.build = {
            "csc": {"path": str(CSC), "bytes": CSC.stat().st_size,
                    "sha256": acc1.sha256_file(CSC), "file_version": _file_version(CSC)},
            "references": {name: {"path": str(path), "sha256": acc1.sha256_file(path)}
                           for name, path in UIA_REFERENCES.items()},
            "listener": {"source": str(LISTENER_SOURCE.relative_to(acc1.ROOT)),
                         "source_sha256": acc1.sha256_file(LISTENER_SOURCE),
                         "exe": str(self.listener_exe),
                         "exe_bytes": self.listener_exe.stat().st_size,
                         "exe_sha256": acc1.sha256_file(self.listener_exe)},
            "control": {"source": str(CONTROL_SOURCE.relative_to(acc1.ROOT)),
                        "source_sha256": acc1.sha256_file(CONTROL_SOURCE),
                        "exe": str(self.control_exe),
                        "exe_bytes": self.control_exe.stat().st_size,
                        "exe_sha256": acc1.sha256_file(self.control_exe)},
            "commands": commands,
        }
        # The evidence directory keeps the exact sources and the build manifest.
        for source in (LISTENER_SOURCE, CONTROL_SOURCE):
            target = self.source_dir / source.name
            target.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
        (self.source_dir / "build.json").write_text(
            json.dumps(self.build, ensure_ascii=False, indent=2), encoding="utf-8")
        return self.build

    # -- listener --------------------------------------------------------------
    def start_listener(self, *, label: str, target_pid: int, seconds: int) -> dict:
        # The compiled listener writes under target/ (the sandbox permits that
        # child to write there); the driver copies the raw bytes into the
        # evidence directory afterwards and records their hashes.
        out = self.build_dir / f"{label}-focus-events.jsonl"
        summary = self.build_dir / f"{label}-focus-events-summary.json"
        status = self.build_dir / f"{label}-focus-events-status.json"
        stop_file = self.build_dir / f"{label}.stop"
        for path in (out, summary, status, stop_file):
            if path.exists():
                path.unlink()
        command = [
            str(self.listener_exe), "--out", str(out), "--summary", str(summary),
            "--status", str(status), "--target-pid", str(target_pid),
            "--seconds", str(seconds), "--poll-ms", "100",
            "--stop-file", str(stop_file), "--label", label,
        ]
        started_utc = now_utc()
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, encoding="utf-8", errors="replace")
        ready: dict = {}
        deadline = time.monotonic() + 20.0
        while time.monotonic() < deadline:
            if process.poll() is not None:
                break
            if status.is_file():
                ready = read_json(status)
                if ready.get("state") in {"subscribed", "done"}:
                    break
            time.sleep(0.1)
        if not ready and process.poll() is not None:
            # The listener died before it could announce itself: keep its exit
            # code and streams so the failure is diagnosable, never silent.
            try:
                stdout_text, stderr_text = process.communicate(timeout=10)
            except (subprocess.TimeoutExpired, ValueError):
                stdout_text, stderr_text = "", ""
            ready = {"state": "exited", "exit": process.returncode,
                     "stdout": (stdout_text or "")[-800:],
                     "stderr": (stderr_text or "")[-800:]}
        self.listener_log = out
        return {
            "command": command, "pid": process.pid, "started_utc": started_utc,
            "status": ready, "log": out.name, "summary_path": summary.name,
            "stop_file": str(stop_file), "_process": process,
            "_out": out, "_summary": summary, "_status": status,
        }

    @staticmethod
    def stop_listener(listener: dict, timeout: float = 20.0) -> dict:
        process = listener.get("_process")
        Path(str(listener["stop_file"])).write_text("stop", encoding="utf-8")
        if process is not None:
            try:
                process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        summary = read_json(Path(str(listener["_summary"])))
        copies = FocusAudit.copy_into_evidence(
            Path(str(listener["_out"])), Path(str(listener["_summary"])),
            Path(str(listener["_status"])))
        return {
            "exit": process.returncode if process is not None else None,
            "stopped_utc": now_utc(),
            "summary": summary,
            "evidence_copies": copies,
        }

    @staticmethod
    def copy_into_evidence(*sources: Path) -> list[dict]:
        """Copy the raw listener outputs next to the session record, byte-exact."""
        copies: list[dict] = []
        evidence = FocusAudit.evidence_dir
        for source in sources:
            if not source.is_file():
                copies.append({"source": str(source), "copied": False})
                continue
            target = evidence / source.name
            target.write_bytes(source.read_bytes())
            copies.append({
                "source": str(source), "target": target.name, "copied": True,
                "bytes": target.stat().st_size, "sha256": acc1.sha256_file(target),
                "source_sha256": acc1.sha256_file(source),
            })
        return copies

    # -- probe -----------------------------------------------------------------
    def probe(self, label: str, action: str, *, match: str = "", text: str = "",
              shot: str = "", pid: int | None = None) -> dict:
        self.calls += 1
        out = self.uia_dir / f"{self.calls:03d}-{label}-{action}.json"
        command = [
            "powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass",
            "-File", str(acc1.PROBE), "-ProcessId", str(pid or self.pid), "-Action", action,
            "-Out", str(out),
        ]
        image = self.shot_dir / f"{self.calls:03d}-{label}.png" if shot else Path()
        if shot:
            command += ["-ImageOut", str(image)]
        if match:
            command += ["-NameMatch", match]
        if text:
            command += ["-Text", text]
        started_utc = now_utc()
        completed = subprocess.run(command, capture_output=True, text=True, timeout=240,
                                   encoding="utf-8", errors="replace")
        finished_utc = now_utc()
        if not out.is_file():
            return {"action": action, "found": False,
                    "error": f"probe produced no output (exit {completed.returncode})",
                    "stderr": completed.stderr[-400:],
                    "started_utc": started_utc, "finished_utc": finished_utc}
        result = json.loads(out.read_text(encoding="utf-8"))
        result["_out"] = str(out.relative_to(self.evidence))
        result["started_utc"] = started_utc
        result["finished_utc"] = finished_utc
        if shot and image.is_file():
            result["_image"] = str(image.relative_to(self.evidence))
            result["_image_sha256"] = acc1.sha256_file(image)
            result["_image_bytes"] = image.stat().st_size
        return result

    @staticmethod
    def focused_rows(tree: dict) -> list[dict]:
        return [
            {"name": row.get("name"), "control_type": row.get("control_type"),
             "id": row.get("id"), "value": row.get("value")}
            for row in tree.get("rows") or [] if row.get("focused")
        ]

    # -- focus attribution -----------------------------------------------------
    def listener_records(self) -> list[dict]:
        path = getattr(self, "listener_log", None)
        return read_jsonl(path) if path else []

    def other_instances(self, exclude: int = 0) -> list[dict]:
        """Record other cos-desktop processes so concurrent instances are visible."""
        try:
            completed = subprocess.run(
                ["powershell.exe", "-NoProfile", "-Command",
                 "Get-Process cos-desktop -ErrorAction SilentlyContinue | "
                 "ForEach-Object { [pscustomobject]@{ id=$_.Id; "
                 "start=$_.StartTime.ToString('yyyy-MM-ddTHH:mm:ss'); "
                 "title=$_.MainWindowTitle; path=$_.Path } } | ConvertTo-Json -Compress"],
                capture_output=True, text=True, timeout=60, encoding="utf-8",
                errors="replace")
            text = completed.stdout.strip()
            if not text:
                return []
            data = json.loads(text)
            rows = data if isinstance(data, list) else [data]
            return [row for row in rows if row.get("id") != exclude]
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError):
            return []

    def attribution(self, started_utc: str, finished_utc: str) -> dict:
        """Classify the listener records inside one probe call's UTC window."""
        start = parse_utc(started_utc)
        end = parse_utc(finished_utc)
        rows: list[dict] = []
        for row in self.listener_records():
            try:
                moment = parse_utc(row.get("time_utc", ""))
            except ValueError:
                continue
            if start <= moment <= end:
                rows.append(row)
        target = [row for row in rows if row.get("process_id") == self.pid]
        foreign = [row for row in rows
                   if row.get("process_id") not in (0, self.pid, None)]
        app_names = getattr(self, "app_names", set())
        foreign_app = [row for row in foreign if row.get("name") in app_names]
        prefix = f"42.{self.hwnd}."
        return {
            "target_records": len(target),
            "target_events": sum(1 for row in target if row.get("kind") == "event"),
            "target_names": sorted({str(row.get("name")) for row in target if row.get("name")}),
            "records_with_our_hwnd": sum(
                1 for row in rows if str(row.get("runtime_id", "")).startswith(prefix)),
            "foreign_processes": sorted({row.get("process_id") for row in foreign}),
            "foreign_names": sorted({str(row.get("name")) for row in foreign
                                     if row.get("name")})[:12],
            "foreign_app_events": sum(1 for row in foreign_app if row.get("kind") == "event"),
            "foreign_app_names": sorted({str(row.get("name")) for row in foreign_app
                                         if row.get("name")})[:12],
        }

    def activate(self, label: str) -> dict:
        """Foreground the window and confirm the listener sees our process focused."""
        probe = self.probe(label, "raise")
        time.sleep(0.4)
        tail = self.listener_records()[-4:]
        return {
            "probe": probe,
            "is_foreground": probe.get("is_foreground"),
            "listener_tail": [
                {key: row.get(key) for key in
                 ("time_utc", "kind", "process_id", "name", "runtime_id")}
                for row in tail
            ],
            "focus_is_target": any(row.get("process_id") == self.pid for row in tail),
        }

    def focus_cycle(self, label: str, attempts: int = 3) -> dict:
        """Give the window a real deactivate/reactivate cycle.

        GPUI updates its accessibility host-focus state from WM_ACTIVATE. The
        accesskit adapter is created lazily on the first UIA request, so a
        window that was already active when the adapter appeared can stay
        "unfocused" for the provider: no focus events are raised and
        FocusedElement falls back to the top-level window. Parking focus on a
        helper window and raising the application again reproduces the
        activation a person produces when they click back into the window.
        """
        tries: list[dict] = []
        for attempt in range(1, attempts + 1):
            park = subprocess.Popen([str(self.control_exe), "240"],
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    text=True, encoding="utf-8", errors="replace")
            time.sleep(1.2)
            parked = self.probe(f"{label}-park{attempt}", "foreground", pid=park.pid)
            time.sleep(0.6)
            started = now_utc()
            raised = self.probe(f"{label}-raise{attempt}", "raise")
            time.sleep(1.2)
            finished = now_utc()
            attribution = self.attribution(started, finished)
            try:
                park.terminate()
                park.wait(timeout=10)
            except (subprocess.TimeoutExpired, OSError):
                park.kill()
            tries.append({
                "attempt": attempt, "park_pid": park.pid, "parked": parked,
                "raised": raised, "started_utc": started, "finished_utc": finished,
                "attribution": attribution,
            })
            if attribution["target_records"] > 0:
                break
        return {
            "attempts_used": len(tries),
            "attempts": tries,
            "attribution": tries[-1]["attribution"],
            "activated": tries[-1]["attribution"]["target_records"] > 0,
        }

    def run_sequence(self, label: str, text: str, *, attempts: int = 2,
                     minimum_target_records: int = 1) -> dict:
        """Run one input sequence and verify that the listener saw our instance.

        A concurrent COS Desktop instance can steal keyboard focus; when that
        happens the recorded events carry another process id and the attempt is
        repeated (bounded) instead of being reported as this instance's result.
        """
        tries: list[dict] = []
        for attempt in range(1, attempts + 1):
            activation = self.activate(f"{label}-act{attempt}")
            started = now_utc()
            result = self.probe(f"{label}-{attempt}", "sequence", text=text)
            finished = now_utc()
            attribution = self.attribution(started, finished)
            tries.append({
                "attempt": attempt, "activation": activation,
                "started_utc": started, "finished_utc": finished,
                "attribution": attribution, "probe": result,
            })
            if (attribution["target_records"] >= minimum_target_records
                    and attribution["foreign_app_events"] == 0):
                break
        final = tries[-1]
        return {
            "attempts_used": len(tries),
            "attempts": [{key: value for key, value in attempt.items()
                          if key != "probe"} | {"probe_out": attempt["probe"].get("_out")}
                         for attempt in tries],
            "attribution": final["attribution"],
            "steps": final["probe"].get("steps") or [],
            "probe": final["probe"],
        }

    # -- control pass ----------------------------------------------------------
    def control_pass(self) -> dict:
        """Prove the listener sees real focus transitions on this host."""
        record: dict = {"purpose": "instrument validation: WinForms focus target"}
        control = subprocess.Popen([str(self.control_exe), "30"], stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True, encoding="utf-8",
                                   errors="replace")
        record["target_pid"] = control.pid
        time.sleep(1.5)
        listener = self.start_listener(label="control", target_pid=control.pid, seconds=15)
        record["listener_start"] = {key: value for key, value in listener.items()
                                    if not key.startswith("_")}
        time.sleep(1.0)
        record["foreground"] = self.probe("control", "foreground", pid=control.pid)
        time.sleep(7.0)
        record["listener_stop"] = self.stop_listener(listener)
        try:
            control.wait(timeout=10)
        except subprocess.TimeoutExpired:
            control.kill()
        record["control_exit"] = control.returncode
        events = read_jsonl(listener["_out"])
        record["records"] = len(events)
        record["events"] = [row for row in events if row.get("kind") == "event"][:20]
        record["event_count"] = sum(1 for row in events if row.get("kind") == "event")
        record["poll_count"] = sum(1 for row in events if row.get("kind") == "poll")
        record["target_names"] = sorted({
            str(row.get("name")) for row in events
            if row.get("process_id") == control.pid and row.get("name")
        })
        record["verdict"] = (
            "PASS" if record["event_count"] > 0 and len(record["target_names"]) >= 2
            else "FAIL")
        return record

    # -- main measurement ------------------------------------------------------
    def sequences(self) -> dict:
        """Run the input sequences while the listener records focus events.

        A freshly activated window reports no focused control: the provider has
        no focused node and Tab traversal does nothing until one exists. The
        keyboard-only prologue (Ctrl+K, Escape) opens and dismisses the command
        palette, which gives the app a focused element and lets the dismissal
        restore focus to the work surface. Every sequence is repeated (bounded)
        when a concurrent COS Desktop instance steals keyboard focus, and the
        attribution is recorded for every attempt.
        """
        record: dict = {}
        record["tree_before"] = self.probe("m0", "tree", shot="m0-before")
        self.hwnd = int(record["tree_before"].get("hwnd") or 0)
        record["hwnd"] = self.hwnd
        record["other_instances"] = self.other_instances(exclude=self.pid)
        record["focused_before"] = self.focused_rows(record["tree_before"])
        self.app_names = {str(row.get("name")) for row in record["tree_before"].get("rows") or []
                          if row.get("name")}
        record["initial_focus"] = self.activate("m0-act")
        record["focus_cycle"] = self.focus_cycle("m0-cycle")

        # 0. Keyboard-only focus prologue: open the palette (its search field
        #    takes focus) and dismiss it (focus returns to the work surface).
        record["focus_prologue"] = self.run_sequence(
            "m1-prologue",
            "fg2|escape|wait:600|ctrl+k|wait:900|names|escape|wait:900|names",
            attempts=3, minimum_target_records=2)
        record["tree_after_prologue"] = self.probe("m1", "tree")
        record["shot_main"] = self.probe("m1", "shot", shot="m1-main")
        record["focused_after_prologue"] = self.focused_rows(record["tree_after_prologue"])

        # 1. Tab traversal from the restored focus. The window is raised again
        #    without SetFocus, so the application keeps its own focused control.
        record["tab_order"] = self.run_sequence(
            "m2-tab",
            "fg2|wait:500|names|" + "|".join(["tab|wait:400"] * 12) + "|names",
            attempts=3, minimum_target_records=6)
        record["tree_after_tab"] = self.probe("m2", "tree")
        record["shot_after_tab"] = self.probe("m2", "shot", shot="m2-after-tab")

        # 2. Shift+Tab from wherever Tab traversal ended.
        record["shift_tab"] = self.run_sequence(
            "m3-shifttab",
            "fg2|wait:300|names|" + "|".join(["shift+tab|wait:450"] * 4) + "|names",
            attempts=3, minimum_target_records=3)

        # 3. Brief (Ctrl+N): open, traverse, close and observe focus restoration.
        record["foreground_check"] = self.probe("m3", "raise")
        if record["foreground_check"].get("is_foreground"):
            record["brief"] = self.run_sequence(
                "m4-brief",
                "fg2|ctrl+n|wait:1000|names|tab|wait:450|tab|wait:450|escape|"
                "wait:800|names", attempts=3, minimum_target_records=2)
            record["tree_brief"] = self.probe("m4", "tree")
            record["shot_brief"] = self.probe("m4", "shot", shot="m4-brief")
        else:
            record["brief"] = {"skipped": "window_not_foreground"}
            record["tree_brief"] = {}

        # 4. Palette (Ctrl+K): open, traverse, close.
        record["foreground_check2"] = self.probe("m5", "raise")
        if record["foreground_check2"].get("is_foreground"):
            record["palette"] = self.run_sequence(
                "m5-palette",
                "fg2|ctrl+k|wait:900|names|tab|wait:450|shift+tab|wait:450|"
                "escape|wait:700|names", attempts=3, minimum_target_records=2)
            record["tree_palette"] = self.probe("m5", "tree")
            record["shot_palette"] = self.probe("m5", "shot", shot="m5-palette")
        else:
            record["palette"] = {"skipped": "window_not_foreground"}
            record["tree_palette"] = {}

        # 5. Focused element after the palette closed (listener poll covers this,
        #    the probe read is recorded for comparison with F1).
        record["tree_after_palette"] = self.probe("m6", "tree")
        record["shot_after_palette"] = self.probe("m6", "shot", shot="m6-after-palette")
        record["focused_after_palette"] = self.focused_rows(record["tree_after_palette"])
        record["other_instances_after"] = self.other_instances(exclude=self.pid)
        return record


def _file_version(path: Path) -> str:
    try:
        completed = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command",
             f"(Get-Item '{path}').VersionInfo.FileVersion"],
            capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace")
        return completed.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def analyse_steps(records: list[dict], steps: list[dict], target_pid: int) -> dict:
    """Attribute listener records to each probe step by its UTC window."""
    timed: list[dict] = []
    for row in records:
        try:
            row["_t"] = parse_utc(row.get("time_utc", ""))
        except ValueError:
            continue
        timed.append(row)
    timed.sort(key=lambda row: row["_t"])

    attributed: list[dict] = []
    for step in steps:
        try:
            start = parse_utc(step.get("start_utc", ""))
            end = parse_utc(step.get("end_utc", ""))
        except ValueError:
            continue
        in_window = [row for row in timed if start <= row["_t"] <= end]
        target = [row for row in in_window if row.get("process_id") == target_pid]
        target_events = [row for row in target if row.get("kind") == "event"]
        before_end = [row for row in timed if row["_t"] <= end]
        last_any = before_end[-1] if before_end else None
        last_target = next((row for row in reversed(before_end)
                            if row.get("process_id") == target_pid), None)
        attributed.append({
            "sequence": step.get("_sequence"),
            "index": step.get("_index"),
            "step": step.get("step"),
            "start_utc": step.get("start_utc"),
            "end_utc": step.get("end_utc"),
            "probe_focus_after": step.get("focus_after"),
            "probe_focus_after_rid": step.get("focus_after_rid"),
            "probe_focus_after_type": step.get("focus_after_type"),
            "probe_error": step.get("error"),
            "is_foreground": step.get("is_foreground"),
            "listener_target_records": len(target),
            "listener_target_events": [
                {"time_utc": row.get("time_utc"), "kind": row.get("kind"),
                 "event_id": row.get("event_id"), "runtime_id": row.get("runtime_id"),
                 "name": row.get("name"), "control_type": row.get("control_type"),
                 "class_name": row.get("class_name"),
                 "keyboard_focusable": row.get("keyboard_focusable"),
                 "has_keyboard_focus": row.get("has_keyboard_focus")}
                for row in target_events
            ],
            "listener_foreign_events": [
                {"time_utc": row.get("time_utc"), "process_id": row.get("process_id"),
                 "runtime_id": row.get("runtime_id"), "name": row.get("name"),
                 "control_type": row.get("control_type")}
                for row in in_window
                if row.get("kind") == "event"
                and row.get("process_id") not in (0, target_pid, None)
            ],
            "listener_focused_at_end": _focus_row(last_any),
            "listener_target_focused_at_end": _focus_row(last_target),
        })
    return {"steps": attributed}


def _focus_row(row: dict | None) -> dict | None:
    if row is None:
        return None
    return {"time_utc": row.get("time_utc"), "kind": row.get("kind"),
            "process_id": row.get("process_id"), "name": row.get("name"),
            "runtime_id": row.get("runtime_id"),
            "control_type": row.get("control_type")}


def collect_steps(sequence_results: dict) -> list[dict]:
    steps: list[dict] = []
    for name, result in sequence_results.items():
        if not isinstance(result, dict) or "steps" not in result:
            continue
        for index, step in enumerate(result.get("steps") or []):
            entry = dict(step)
            entry["_sequence"] = name
            entry["_index"] = index
            steps.append(entry)
    return steps


def write(path: Path, document: object) -> None:
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", default=str(EVIDENCE_DEFAULT))
    parser.add_argument("--port", type=int, default=PORT)
    parser.add_argument("--skip-control", action="store_true")
    parser.add_argument("--seconds", type=int, default=240,
                        help="listener bound for the Desktop session")
    args = parser.parse_args(argv)
    evidence = Path(args.evidence_dir).resolve()
    evidence.mkdir(parents=True, exist_ok=True)

    session: dict = {
        "measurement": "UIA focus-event stream from a same-integrity UIA client (AX F1)",
        "task": "AX F1 evidence gap: focused-control observation without accevent.exe",
        "started_at": now_utc(),
        "harness": {
            "command": "python apps/desktop/devtools/runtime_integration/run_ax_focus_events.py",
            "acc1_source": ACC1_SOURCE,
            "port": args.port,
            "listener": "Automation.AddAutomationFocusChangedEventHandler "
                        "(UIA_AutomationFocusChangedEventId) + FocusedElement polling",
            "control": "WinForms focus target proving the instrument sees real focus transitions",
            "probe": str(acc1.PROBE),
            "not_measured": "screen-reader pronouncement (NVDA absent); this is a UIA client "
                            "event stream, not an AT utterance",
        },
        "environment": {
            "os": platform.version(),
            "platform": platform.platform(),
            "desktop_root": str(acc1.ROOT),
            "head": base.git_value(acc1.ROOT, "rev-parse", "HEAD"),
            "binary": str(acc1.DESKTOP_EXE),
            "binary_sha256": acc1.sha256_file(acc1.DESKTOP_EXE)
            if acc1.DESKTOP_EXE.is_file() else "missing",
            "elevated": _elevated(),
            "session_id": _session_id(),
        },
        "control": {}, "runtime": {}, "window": {}, "steps": {},
        "listener": {}, "analysis": {}, "verdict": "NOT_RUN",
        "residuals": [], "cleanup": {}, "leak_check": {},
    }
    audit = FocusAudit(evidence)
    ctx = None
    secrets: list[str] = []
    try:
        session["build"] = audit.compile()
        if not args.skip_control:
            session["control"] = audit.control_pass()
            if session["control"].get("verdict") != "PASS":
                session["residuals"].append(
                    "control target produced no focus events; a negative Desktop result "
                    "cannot be attributed to the product provider")

        session["other_cos_desktop_instances_before"] = audit.other_instances()
        ctx = acc1.Ctx("ax-focus", args.port)
        secrets = list(ctx.collect)
        session["runtime"] = {
            "work_dir": str(ctx.work),
            "start": ctx.start_runtime(wait_generation=False),
        }
        session["window"] = ctx.launch_desktop("axfocus")
        audit.pid = ctx.app.ui.pid if ctx.app and ctx.app.ui else 0
        session["window_pid"] = audit.pid

        listener = audit.start_listener(label="desktop", target_pid=audit.pid,
                                        seconds=args.seconds)
        session["listener"]["start"] = {key: value for key, value in listener.items()
                                        if not key.startswith("_")}
        time.sleep(0.8)
        session["steps"] = audit.sequences()
        session["listener"]["stop"] = audit.stop_listener(listener)

        records = read_jsonl(listener["_out"])
        steps = collect_steps(session["steps"])
        session["analysis"] = analyse_steps(records, steps, audit.pid)
        session["analysis"]["summary"] = session["listener"]["stop"].get("summary", {})
        session["analysis"]["record_count"] = len(records)
        target_events = sum(1 for row in records
                            if row.get("kind") == "event" and row.get("process_id") == audit.pid)
        target_polls = sum(1 for row in records
                           if row.get("kind") == "poll" and row.get("process_id") == audit.pid)
        distinct_targets = {
            str(row.get("runtime_id")) for row in records
            if row.get("process_id") == audit.pid and row.get("runtime_id")
        }
        session["analysis"]["target_event_count"] = target_events
        session["analysis"]["target_poll_count"] = target_polls
        session["analysis"]["distinct_target_runtime_ids"] = sorted(distinct_targets)
        session["analysis"]["event_ids_seen"] = sorted({
            int(row.get("event_id") or 0) for row in records if row.get("kind") == "event"
        })
        if target_events > 0 and len(distinct_targets) >= 2:
            session["verdict"] = "PASS"
        elif target_polls > 0 and len(distinct_targets) >= 1:
            session["verdict"] = "PARTIAL"
        else:
            session["verdict"] = "NOT_RUN"
        session["finished_at"] = now_utc()
    except Exception as error:  # noqa: BLE001 - record the failure, never hide it
        import traceback
        session["error"] = f"{type(error).__name__}: {error}"
        session["traceback"] = traceback.format_exc()[-4000:]
    finally:
        if ctx is not None:
            session["cleanup"] = {
                "desktop": ctx.app.close() if ctx.app else None,
                "runtime": ctx.runtime.stop(),
                "ports": {str(args.port): acc1.port_open(args.port),
                          "8188": acc1.port_open(8188)},
            }
            base.cleanup_credential_store(ctx.runtime.endpoint)
        document = acc1.redact_document(session)
        document["leak_check"] = acc1.leak_scan(evidence, list(secrets))
        write(evidence / "session.json", document)
        print(json.dumps({"verdict": session.get("verdict"),
                          "error": session.get("error", ""),
                          "evidence": str(evidence)}, ensure_ascii=False))
    return 0 if "error" not in session else 1


def _elevated() -> bool:
    try:
        import ctypes
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except (AttributeError, OSError):
        return False


def _session_id() -> str:
    try:
        completed = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command",
             "(Get-Process -Id $PID).SessionId"],
            capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace")
        return completed.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


if __name__ == "__main__":
    raise SystemExit(main())
