#!/usr/bin/env python3
"""Reproduce the MP-ACC1 Brief input abort through the native release window.

The ACC1 native harness entered Brief text with a synthetic click first. During
that work a UIA-focus-only attempt (UIA ``SetFocus`` on a Brief field, then
``WM_CHAR`` with no mouse input) aborted the release binary with
``thread caused non-unwinding panic. aborting.`` This script keeps that sequence
deterministic and bounded:

1. start the isolated Runtime (fresh DB copy, own secrets, own discovery) and
   launch the Desktop with discovery only;
2. open the Brief through the UIA Invoke pattern;
3. type into one Brief text field, move focus to the *other* Brief text field
   with UIA ``SetFocus`` (no click, exactly the reported path) and type again;
4. record the window/UIA/screenshot state of every step, the app exit code, the
   captured stderr and the product crash reports.

It never submits a Brief, never starts generation, and never foregrounds the
window. Run it with the repository's venv Python; the probe is
``probe_window.ps1`` in this directory.

Usage (from the repository root):

    C:/AI/orchestrator/.venv/Scripts/python.exe -B \
      apps/desktop/devtools/runtime_integration/run_acc1_input_panic.py \
      --mode direction-then-goal \
      --evidence-dir docs/development/evidence-published/acc1-input-panic-latest/prefix

Modes:
    direction-then-goal  type into Direction, UIA-focus Goal, type (the abort path)
    click-first          click Goal, type (the harness path that never aborted)
    focus-only           UIA-focus the empty Goal on a fresh Brief, type
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run_acc1 as acc1  # noqa: E402 - local harness helpers, stdlib only
import run_acc1_native as native  # noqa: E402
import run_integration as base  # noqa: E402

ROOT = base.DESKTOP_ROOT
PORT = 8132
PANIC_MARKER = "thread caused non-unwinding panic. aborting."
EXIT_ABORT = 3221226505  # 0xC0000409, the abort() seen from the window procedure
GOAL = "目的"
DIRECTION = "方向性"


def now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return f"sha256:{digest.hexdigest()}"


def sha256_text(text: str) -> str:
    return f"sha256:{hashlib.sha256(text.encode('utf-8')).hexdigest()}"


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def port_open(port: int) -> bool:
    probe = socket.socket()
    probe.settimeout(0.5)
    try:
        return probe.connect_ex(("127.0.0.1", port)) == 0
    finally:
        probe.close()


def wait_port_free(port: int, timeout: float = 30.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not port_open(port):
            return True
        time.sleep(0.5)
    return False


def pid_alive(pid: int) -> bool:
    try:
        listed = subprocess.run(
            ["tasklist", "/FI", f"PID eq {pid}", "/NH"],
            capture_output=True, text=True, timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return str(pid) in listed.stdout


class Run:
    """One bounded reproduction run and its retained record."""

    def __init__(self, *, label: str, exe: Path, evidence: Path, keep_work: bool):
        stamp = time.strftime("%Y%m%dT%H%M%S")
        self.label = label
        self.exe = exe
        self.evidence = evidence
        self.work = ROOT / "target" / "acc1-input-panic" / f"{stamp}-{label}"
        self.work.mkdir(parents=True, exist_ok=True)
        self.keep_work = keep_work
        self.evidence.mkdir(parents=True, exist_ok=True)
        self.secret = acc1.secrets.token_hex(24)
        self.handle_secret = acc1.secrets.token_hex(24)
        self.runtime = native.NativeRuntime(
            port=PORT,
            work=self.work,
            db_copy=self.work / "generation.sqlite",
            presets=self.work / "cos_presets.json",
            secret=self.secret,
            handle_secret=self.handle_secret,
        )
        self.app: native.NativeApp | None = None
        self.record: dict = {
            "label": label,
            "started_at": now(),
            "mode": "",
            "exe": str(exe),
            "exe_sha256": sha256_file(exe) if exe.is_file() else "missing",
            "exe_bytes": exe.stat().st_size if exe.is_file() else 0,
            "steps": [],
            "typing": [],
            "panic": {"observed": False},
            "cleanup": {},
            "residuals": [],
        }

    # -- setup / teardown -------------------------------------------------

    def start_runtime(self) -> dict:
        base.copy_database(acc1.SOURCE_DB, self.runtime.db_copy)
        shutil.copyfile(acc1.SOURCE_PRESETS, self.runtime.presets)
        started = self.runtime.start()
        self.record["runtime"] = {
            "pid": started["pid"],
            "endpoint": started["endpoint"],
            "epoch": started["epoch"],
            "server_identity": started["server_identity"],
            "server_build": started["server_build"],
            "db_copy_sha256": sha256_file(self.runtime.db_copy),
            "db_copy_bytes": self.runtime.db_copy.stat().st_size,
        }
        return started

    def start_app(self) -> int:
        self.app = native.NativeApp(
            label=self.label, work=self.work, secret=self.secret,
            discovery=self.runtime.discovery_path,
        )
        previous = native.DESKTOP_EXE
        native.DESKTOP_EXE = self.exe
        try:
            pid = self.app.launch()
        finally:
            native.DESKTOP_EXE = previous
        window = self.app.wait_window(timeout=60.0)
        self.step("window", {"window": window})
        return pid

    def close(self) -> None:
        cleanup: dict = {}
        if self.app is not None:
            cleanup["app"] = self.app.close()
        if self.runtime is not None:
            cleanup["runtime"] = self.runtime.stop()
        cleanup["port_free"] = wait_port_free(PORT)
        cleanup["pid_alive"] = {
            str(pid): pid_alive(pid)
            for pid in (
                (self.app.process.pid if self.app and self.app.process else None),
                (self.runtime.process.pid if self.runtime.process else None),
            )
            if pid
        }
        self.record["cleanup"] = cleanup
        if self.app is not None and not cleanup["app"].get("exited"):
            self.record["residuals"].append("desktop process did not exit")
        if not cleanup["port_free"]:
            self.record["residuals"].append(f"port {PORT} still has a listener")

    # -- recording --------------------------------------------------------

    def step(self, label: str, data: dict) -> dict:
        entry = {**data, "label": label, "at": now()}
        self.record["steps"].append(entry)
        return entry

    def snapshot(self, ui: native.Ui, label: str) -> dict:
        dump = ui.probe("dump")
        shot = ui.shot(label)
        names = list(dump.get("names") or [])
        entry = {
            "label": label,
            "at": now(),
            "elements": dump.get("elements"),
            "names": names[:120],
            "names_sha256": sha256_text("\n".join(names)),
            "window": {
                "title": dump.get("title"), "class": dump.get("class"),
                "width": dump.get("width"), "height": dump.get("height"),
            },
            "screenshot": {
                "file": Path(str(shot.get("_image") or "")).name or "",
                "sha256": shot.get("_image_sha256"),
                "bytes": shot.get("_image_bytes"),
                "printed": shot.get("ok"),
            },
        }
        if shot.get("_image"):
            source = Path(str(shot["_image"]))
            target = self.evidence / "shots" / source.name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            with contextlib.suppress(OSError):
                source.unlink()
        return self.step(f"snapshot:{label}", entry)

    def dump_uia(self, ui: native.Ui, label: str) -> None:
        source = Path(str((ui.probe("dump").get("_out")) or ""))
        if source.is_file():
            target = self.evidence / "uia" / f"{label}.json"
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)

    def type_char(self, ui: native.Ui, match: str, character: str, tag: str) -> dict:
        result = ui.probe("type", match=match, text=character)
        entry = {
            "tag": tag,
            "at": now(),
            "character": character,
            "matched": result.get("matched"),
            "set_focus_called": result.get("set_focus_called"),
            "focused": result.get("focused"),
            "focused_name": result.get("focused_name"),
            "code_units_sent": result.get("code_units_sent"),
            "value_after": result.get("value_after"),
            "error": result.get("error"),
        }
        self.record["typing"].append(entry)
        return entry

    def classify(self, ui: native.Ui) -> None:
        pid = self.app.process.pid if self.app and self.app.process else 0
        try:
            code = self.app.process.wait(timeout=20) if self.app and self.app.process else None
        except subprocess.TimeoutExpired:
            code = "running"
        log_path = self.work / f"desktop-{self.label}.log"
        log_text = log_path.read_text(encoding="utf-8", errors="replace") if log_path.is_file() else ""
        panic_line = next((line for line in log_text.splitlines() if PANIC_MARKER in line), "")
        crash_dir = self.work / f"localappdata-{self.label}" / "cos-desktop" / "crash-reports"
        reports = []
        if crash_dir.is_dir():
            for report in sorted(crash_dir.glob("report-*.txt")):
                target = self.evidence / "crash-reports" / report.name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(report, target)
                reports.append({"file": target.name, "sha256": sha256_file(target),
                                "text": report.read_text(encoding="utf-8", errors="replace")})
        self.record["panic"] = {
            "observed": bool(panic_line) or code == EXIT_ABORT,
            "exit_code": code,
            "exit_code_hex": f"0x{code:08X}" if isinstance(code, int) and code > 0x10000 else None,
            "log_line": panic_line,
            "stderr_message_printed_by_product_hook": False,
            "crash_reports": reports,
            "process_alive": pid_alive(pid) if pid else False,
            "note": (
                "The product installs its own panic hook, so the default hook's "
                "'thread panicked at ...' text is not printed; only the standard "
                "library's non-unwinding abort line reaches stderr."
            ),
        }
        if panic_line or code == EXIT_ABORT:
            log_file = self.evidence / "desktop.log"
            log_file.write_text(log_text, encoding="utf-8")
            self.record["captured_log_file"] = log_file.name
        if pid and pid_alive(pid):
            with contextlib.suppress(RuntimeError):
                ui.probe("close")
            try:
                code = self.app.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                code = "running"
            self.record["panic"]["post_close_exit_code"] = code


def run_mode(mode: str, ui: native.Ui, record: Run) -> None:
    opened = ui.probe("invoke", match="新しいタスクを作成")
    record.step("brief_open", {"invoked": opened.get("invoked"),
                               "pattern": opened.get("pattern"),
                               "error": opened.get("error")})
    time.sleep(1.5)
    record.snapshot(ui, "brief-open")
    record.dump_uia(ui, "brief-open")
    names = list((ui.probe("dump").get("names") or []))
    goal = GOAL if GOAL in names else ""
    direction = DIRECTION if DIRECTION in names else ""
    record.step("brief_fields", {"goal": goal, "direction": direction})
    if not goal or not direction:
        raise RuntimeError("the Brief text fields did not render; catalog unavailable?")

    if mode == "click-first":
        clicked = ui.probe("click", match=goal)
        record.step("click_goal", {"clicked": clicked.get("clicked"), "error": clicked.get("error")})
        time.sleep(0.4)
        for index, character in enumerate("ACC1NATIVE"):
            entry = record.type_char(ui, goal, character, "goal")
            if entry["code_units_sent"] is None:
                break
            if record.app and not pid_alive(record.app.process.pid):
                break
            time.sleep(0.15)
    elif mode == "focus-only":
        focused = ui.probe("focus", match=goal)
        record.step("uia_focus_goal", {"set_focus_called": focused.get("set_focus_called"),
                                       "focused": focused.get("focused"),
                                       "focus_error": focused.get("focus_error")})
        time.sleep(0.4)
        for index, character in enumerate("ACC1NATIVE"):
            entry = record.type_char(ui, goal, character, "goal")
            if entry["code_units_sent"] is None:
                break
            if record.app and not pid_alive(record.app.process.pid):
                break
            time.sleep(0.15)
    else:
        direction_click = ui.probe("click", match=direction)
        record.step("click_direction", {"clicked": direction_click.get("clicked"),
                                        "error": direction_click.get("error")})
        time.sleep(0.4)
        for character in "abcdefghij":
            entry = record.type_char(ui, direction, character, "direction")
            if entry["code_units_sent"] is None:
                break
            if record.app and not pid_alive(record.app.process.pid):
                break
            time.sleep(0.15)
        record.snapshot(ui, "direction-filled")
        focus = ui.probe("focus", match=goal)
        record.step("uia_focus_goal", {"set_focus_called": focus.get("set_focus_called"),
                                       "focused": focus.get("focused"),
                                       "focus_error": focus.get("focus_error")})
        time.sleep(0.4)
        for index, character in enumerate("ACC1PANIC"):
            entry = record.type_char(ui, goal, character, "goal")
            if entry["code_units_sent"] is None:
                break
            if record.app and not pid_alive(record.app.process.pid):
                break
            time.sleep(0.15)
    if record.app and pid_alive(record.app.process.pid):
        record.snapshot(ui, "after-typing")
        record.dump_uia(ui, "after-typing")
        for match, tag in ((goal, "goal"), (direction, "direction")):
            value = ui.probe("valueread", match=match)
            record.step(f"value:{tag}", {"value": value.get("value"), "error": value.get("error")})


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", default="direction-then-goal",
                        choices=["direction-then-goal", "click-first", "focus-only"])
    parser.add_argument("--exe", default=str(base.DESKTOP_EXE))
    parser.add_argument("--evidence-dir", default=str(
        ROOT / "docs" / "development" / "evidence-published" / "acc1-input-panic-latest"))
    parser.add_argument("--keep-work", action="store_true")
    args = parser.parse_args(argv)

    if port_open(PORT):
        print(f"port {PORT} already has a listener; refusing to start", file=sys.stderr)
        return 2
    exe = Path(args.exe)
    if not exe.is_file():
        print(f"missing Desktop binary: {exe}", file=sys.stderr)
        return 2
    if not acc1.RUNTIME_PYTHON.is_file():
        print(f"missing Runtime Python: {acc1.RUNTIME_PYTHON}", file=sys.stderr)
        return 2

    run = Run(label=args.mode.replace("-", "_"), exe=exe,
              evidence=Path(args.evidence_dir), keep_work=args.keep_work)
    run.record["mode"] = args.mode
    exit_code = 3
    try:
        run.start_runtime()
        pid = run.start_app()
        run.record["desktop_pid"] = pid
        if run.app is None or not pid_alive(pid):
            raise RuntimeError("the Desktop window did not stay alive")
        if run.app.wait_connected(timeout=90.0)[2] is not True:
            raise RuntimeError("the Desktop did not render the connected rail")
        run.snapshot(run.app.ui, "connected")
        run_mode(args.mode, run.app.ui, run)
        run.classify(run.app.ui)
        exit_code = 0
    except Exception as error:  # noqa: BLE001 - the record must survive any failure
        run.record["residuals"].append(f"run error: {type(error).__name__}: {error}")
        exit_code = 1
    finally:
        run.close()
        run.record["verdict"] = "PANIC" if run.record["panic"]["observed"] else "NO_PANIC"
        run.record["finished_at"] = now()
        run.record["residuals"].append(
            "work directory retained" if args.keep_work else "work directory retained for inspection"
        )
        write_json(run.evidence / "run.json", run.record)
        print(json.dumps({
            "mode": args.mode,
            "verdict": run.record["verdict"],
            "exit_code": run.record["panic"]["exit_code"],
            "panic_line": run.record["panic"]["log_line"],
            "evidence": str(run.evidence),
            "residuals": run.record["residuals"],
        }, ensure_ascii=False, indent=2))
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
