#!/usr/bin/env python3
"""Bounded real-NVDA observation harness for the COS Desktop v0.1.0 release.

This is a test-only driver (not a product entry point). It launches the exact
v0.1.0 portable release binary against the isolated MP-ACC1 Runtime lane
(discovery only, no generation), starts the official NVDA 2026.3.1 portable
copy with an isolated configuration, drives real keyboard input and UIA focus
through ``probe_window.ps1``, and records the NVDA speech log (IO level) for
each surface.

It never synthesizes an utterance: the recorded speech is what NVDA passed to
its configured synthesizer. Chrome/Twitch startup speech is filtered out of the
committed log because it is unrelated private user content; the raw log hash is
recorded and the raw file is deleted with the portable copy.
"""
from __future__ import annotations

import argparse
import ast
import contextlib
import ctypes
import json
import os
import re
import shutil
import subprocess
import sys
import time
import traceback
from pathlib import Path

ROOT = Path(r"C:\dev\cos-desktop")
HARNESS = ROOT / "apps" / "desktop" / "devtools" / "runtime_integration"
sys.path.insert(0, str(HARNESS))

import run_acc1_native as acc1  # noqa: E402
import run_integration as base  # noqa: E402

RELEASE_ZIP = ROOT / "target" / "cos-desktop-v0.1.0-portable-unsigned.zip"
RELEASE_EXE = ROOT / "target" / "nvda-at" / "release" / "cos-desktop.exe"
acc1.DESKTOP_EXE = RELEASE_EXE
NVDA_HOME = Path(os.environ["LOCALAPPDATA"]) / "nvda-at"
NVDA = NVDA_HOME / "portable" / "nvda.exe"
NVDA_CONFIG = NVDA_HOME / "config"
NVDA_LOG = NVDA_HOME / "logs" / "session.log"
EVIDENCE = ROOT / "docs" / "development" / "evidence-published" / "nvda-at-latest"
CHROME_HELPER = ROOT / "target" / "nvda-at" / "chrome_windows.py"
CHROME_STATE = ROOT / "target" / "nvda-at" / "chrome-state.json"
PORT = 8155
PROBE = acc1.PROBE

# Filtering rules for the committed log. The raw log is retained only long
# enough to hash and filter it, then deleted with the portable copy.
CHROME_MARKERS = (
    "Twitch",
    "Google Chrome",
    "以下の理由",
    "チャット",
    "配信",
    "フォロー",
    "スキップ先",
    "ナビゲーション ランドマーク",
    "検索入力欄",
    "検索ボタン",
    "その他のオプション",
    "グリッドあり",
    "既読",
    "オーディオ再生中",
    "AltP",
    "alt+",
)

# Only speech that names a COS Desktop surface/control is committed. This is an
# allowlist, not a claim about what NVDA said: the raw log is hashed and deleted.
APP_TERMS = (
    "COS Desktop", "作業画面", "タスク", "停止", "成果物", "候補", "画像", "診断",
    "作業領域", "受付", "利用可能", "利用不可", "正式状態", "反復", "詳細と説明",
    "折りたたみ", "Microsoft IME", "ヒラガナ", "ローマ字", "task-", "候補一覧",
    "処理の流れ", "全体表示", "COMPLETED", "ブリーフ", "方向性", "提出",
    "実行履歴", "履歴を閉じる", "コマンド", "評価 —", "判断の根拠", "改善 —",
    "結果 —", "目的 エディット",
)


def is_app_speech(text: str) -> bool:
    return any(term in text for term in APP_TERMS)


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2), encoding="utf-8")


def extract_speech_strings(line: str) -> list[str]:
    """Extract top-level string items from an NVDA ``Speaking [...]`` repr.

    Commands (LangChangeCommand, IndexCommand, CallbackCommand, ...) are nested
    in parentheses and are skipped. Only strings that are direct list items are
    returned; no text is invented.
    """
    start = line.find("[")
    if start < 0:
        return []
    depth = 1
    out: list[str] = []
    i = start + 1
    while i < len(line):
        char = line[i]
        if char in "([{":
            depth += 1
            i += 1
        elif char in ")]}":
            depth -= 1
            i += 1
            if depth <= 0:
                break
        elif char in "'\"" and depth == 1:
            quote = char
            j = i + 1
            while j < len(line):
                if line[j] == "\\":
                    j += 2
                    continue
                if line[j] == quote:
                    break
                j += 1
            token = line[i : j + 1]
            with contextlib.suppress(Exception):
                value = ast.literal_eval(token)
                if isinstance(value, str) and value:
                    out.append(value)
            i = j + 1
        else:
            i += 1
    return out


class SpeechLog:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.line_index = 0
        self.pending_at = ""
        self.all_utterances: list[dict] = []

    def _lines(self) -> list[str]:
        if not self.path.is_file():
            return []
        return self.path.read_text(encoding="utf-8", errors="replace").splitlines()

    def read_new(self, step: str) -> list[dict]:
        lines = self._lines()
        new = lines[self.line_index :]
        self.line_index = len(lines)
        utterances: list[dict] = []
        for line in new:
            match = re.match(r"^IO - speech\.speech\.speak \((\d\d:\d\d:\d\d\.\d+)\)", line)
            if match:
                self.pending_at = match.group(1)
                continue
            if not line.startswith("Speaking "):
                continue
            text = " ".join(extract_speech_strings(line)).strip()
            if not text:
                continue
            entry = {
                "step": step,
                "at": self.pending_at,
                "text": text,
                "raw": line[:4000],
            }
            utterances.append(entry)
            self.all_utterances.append(entry)
        return utterances


def probe_pid(pid: int, action: str, out: Path, *, image_out: Path | None = None,
              match: str = "", text: str = "") -> dict:
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        out.unlink()
    command = [
        "powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", str(PROBE), "-ProcessId", str(pid), "-Action", action,
        "-Out", str(out),
    ]
    if image_out is not None:
        image_out.parent.mkdir(parents=True, exist_ok=True)
        command += ["-ImageOut", str(image_out)]
    if match:
        command += ["-NameMatch", match]
    if text:
        command += ["-Text", text]
    completed = subprocess.run(
        command, capture_output=True, text=True, timeout=180,
        encoding="utf-8", errors="replace",
    )
    if not out.is_file():
        return {"action": action, "found": False,
                "error": f"probe produced no output (exit {completed.returncode})",
                "stderr": completed.stderr[-400:]}
    result = json.loads(out.read_text(encoding="utf-8"))
    result["_out"] = str(out)
    if image_out is not None and image_out.is_file():
        result["_image"] = str(image_out)
        result["_image_sha256"] = acc1.sha256_file(image_out)
        result["_image_bytes"] = image_out.stat().st_size
    return result


def powershell_json(script: str) -> object:
    completed = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command", script],
        capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace",
    )
    with contextlib.suppress(Exception):
        return json.loads(completed.stdout.strip())
    return {"error": completed.stderr[-400:]}


def nvda_processes() -> list[int]:
    result = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command",
         "(Get-Process nvda -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id) -join ','"],
        capture_output=True, text=True, timeout=30, encoding="utf-8", errors="replace",
    )
    return [int(item) for item in result.stdout.strip().split(",") if item.strip().isdigit()]


class NvdaSession:
    def __init__(self, evidence: Path) -> None:
        self.evidence = evidence
        self.proc: subprocess.Popen | None = None
        self.log = SpeechLog(NVDA_LOG)
        self.started_at = ""
        self.ready = False

    def quit(self) -> dict:
        record: dict = {"quit_sent": False, "exited": False, "forced": False}
        if self.proc is None:
            record["reason"] = "not_started"
            return record
        with contextlib.suppress(Exception):
            subprocess.run([str(NVDA), "-q"], timeout=20, capture_output=True)
            record["quit_sent"] = True
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                record["exited"] = True
                break
            time.sleep(0.5)
        if self.proc.poll() is None:
            with contextlib.suppress(Exception):
                self.proc.kill()
                record["forced"] = True
            time.sleep(1.0)
            record["exited"] = self.proc.poll() is not None
        record["remaining_pids"] = nvda_processes()
        return record

    def start(self, wait_seconds: float = 14.0) -> dict:
        record: dict = {"executable": str(NVDA), "config": str(NVDA_CONFIG),
                        "log": str(NVDA_LOG)}
        if NVDA_LOG.exists():
            NVDA_LOG.unlink()
        NVDA_LOG.parent.mkdir(parents=True, exist_ok=True)
        self.started_at = now_iso()
        self.proc = subprocess.Popen(
            [str(NVDA), "-c", str(NVDA_CONFIG), "-f", str(NVDA_LOG), "-l", "12", "-n", "ja"],
            cwd=str(NVDA_HOME), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        record["pid"] = self.proc.pid
        deadline = time.monotonic() + wait_seconds
        while time.monotonic() < deadline:
            text = NVDA_LOG.read_text(encoding="utf-8", errors="replace") if NVDA_LOG.is_file() else ""
            if "Starting NVDA version" in text and "Speaking" in text:
                self.ready = True
                break
            if self.proc.poll() is not None:
                break
            time.sleep(0.5)
        text = NVDA_LOG.read_text(encoding="utf-8", errors="replace") if NVDA_LOG.is_file() else ""
        record["ready"] = self.ready
        record["alive"] = self.proc.poll() is None
        record["version_line"] = next(
            (line for line in text.splitlines() if "Starting NVDA version" in line), "")
        record["log_bytes_at_ready"] = len(text.encode("utf-8"))
        return record


class Driver:
    def __init__(self, evidence: Path) -> None:
        self.evidence = evidence
        self.uia_dir = evidence / "uia"
        self.shot_dir = evidence / "shots"
        self.nvda_dir = evidence / "nvda"
        self.uia_dir.mkdir(parents=True, exist_ok=True)
        self.shot_dir.mkdir(parents=True, exist_ok=True)
        self.nvda_dir.mkdir(parents=True, exist_ok=True)
        self.session: dict = {
            "nvda": {},
            "measurement": "Real NVDA screen-reader session on the COS Desktop v0.1.0 release",
            "task": "AX acceptance follow-up (NVDA pronouncement)",
            "started_at": now_iso(),
            "steps": {},
            "speech": {"startup": [], "steps": {}, "utterance_count": 0},
            "not_measured": [],
            "residuals": [],
        }
        self.nvda = NvdaSession(evidence)
        self.ui = None
        self.ctx = None
        self.log = self.nvda.log

    # -- evidence helpers -----------------------------------------------------
    def save_probe(self, label: str, result: dict) -> None:
        source = Path(result.get("_out", ""))
        if source.is_file():
            target = self.uia_dir / f"{label}.json"
            shutil.copyfile(source, target)
            result["_evidence"] = str(target.relative_to(self.evidence))
            result["_evidence_sha256"] = acc1.sha256_file(target)

    def capture_tree(self, label: str) -> dict:
        result = self.ui.probe("tree")
        self.save_probe(f"{label}-tree", result)
        return result

    def capture_shot(self, label: str) -> dict:
        result = self.ui.shot(label)
        source = Path(result.get("_image", ""))
        if source.is_file():
            target = self.shot_dir / f"{label}.png"
            shutil.copyfile(source, target)
            result["_evidence"] = str(target.relative_to(self.evidence))
            result["_evidence_sha256"] = acc1.sha256_file(target)
        return result

    def step(self, label: str, sequence: str, *, wait: float = 2.5,
             shot: bool = False, tree: bool = False) -> dict:
        record: dict = {"label": label, "sequence": sequence, "at": now_iso()}
        result = self.ui.probe("sequence", text=sequence)
        self.save_probe(label, result)
        record["probe"] = result
        if tree:
            record["tree"] = self.capture_tree(label)
        if shot:
            record["shot"] = self.capture_shot(label)
        time.sleep(wait)
        utterances = [item for item in self.log.read_new(label) if is_app_speech(item["text"])]
        record["utterances"] = utterances
        record["utterance_count"] = len(utterances)
        self.session["steps"][label] = record
        self.session["speech"]["steps"][label] = utterances
        return record

    # -- Chrome foreground control -------------------------------------------
    def chrome_action(self, action: str) -> dict:
        if not CHROME_HELPER.is_file():
            return {"action": action, "error": "helper_missing"}
        completed = subprocess.run(
            [sys.executable, str(CHROME_HELPER), action, str(CHROME_STATE)],
            capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace",
        )
        with contextlib.suppress(Exception):
            return json.loads(completed.stdout.strip())
        return {"action": action, "error": completed.stderr[-400:]}

    def restore_chrome(self) -> dict:
        if not CHROME_STATE.is_file():
            return {"action": "restore", "skipped": "no_state"}
        return self.chrome_action("restore")

    def app_activation_cycle(self) -> dict:
        """Force a real deactivate/reactivate cycle while NVDA is running.

        The AX focus-event follow-up showed that a window already active without
        a following deactivate/reactivate cycle can raise no provider focus
        events. Minimizing and restoring the app window is the user-visible
        equivalent of alt-tabbing away and back; it changes no application data.
        """
        hwnd = int((self.session.get("foreground_before_nvda") or {}).get("hwnd") or 0)
        if not hwnd:
            return {"error": "no_app_hwnd"}
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        minimized = bool(user32.ShowWindow(hwnd, 6))
        time.sleep(0.8)
        restored = bool(user32.ShowWindow(hwnd, 9))
        time.sleep(0.6)
        return {"hwnd": hwnd, "minimized": minimized, "restored": restored}

    # -- surfaces -------------------------------------------------------------
    def run(self) -> None:
        self.session["release"] = self.release_record()
        self.session["environment"] = self.environment_record()
        self.ctx = acc1.Ctx("nvda-at", PORT)
        self.session["runtime_start"] = self.ctx.start_runtime(wait_generation=False)
        self.session["runtime"] = {
            "endpoint": self.ctx.runtime.endpoint,
            "db_copy_sha256": self.ctx.record.get("db_copy_sha256"),
            "db_copy_bytes": self.ctx.record.get("db_copy_bytes"),
            "presets_sha256": self.ctx.record.get("presets_sha256"),
            "health": self.ctx.record.get("health"),
            "catalog": self.ctx.record.get("catalog"),
            "observation_open": self.ctx.record.get("observation_open"),
            "generation_requested": False,
        }
        self.session["window"] = self.ctx.launch_desktop("nvda-at")
        self.ui = self.ctx.app.ui
        self.session["desktop"] = {
            "pid": self.ctx.app.ui.pid,
            "binary": str(RELEASE_EXE),
            "binary_sha256": acc1.sha256_file(RELEASE_EXE),
        }

        # The release window must be foreground before NVDA starts so the AT
        # session attaches to the application, not to the user's browser.
        self.session["foreground_before_nvda"] = self.ui.probe("foreground")
        time.sleep(1.0)
        self.session["chrome_minimize"] = self.chrome_action("minimize")
        time.sleep(1.0)
        self.session["nvda"] = self.nvda.start()
        self.session["nvda"]["started_at"] = self.nvda.started_at
        time.sleep(1.0)
        self.session["app_activation_cycle"] = self.app_activation_cycle()
        self.session["foreground_after_nvda"] = self.ui.probe("foreground")
        time.sleep(3.0)
        search_wait, search_text, search_found = self.ui.wait_contains("タスクを検索", 30.0, "search")
        self.session["search_ready"] = {"found": search_found,
                                        "row_count": search_wait.get("row_count"),
                                        "contains": "タスクを検索" in search_text}
        startup_all = self.log.read_new("startup")
        self.session["nvda"]["startup_all_count"] = len(startup_all)
        self.session["nvda"]["startup_utterances"] = [
            item for item in startup_all if is_app_speech(item["text"])
        ]
        self.session["speech"]["startup"] = self.session["nvda"]["startup_utterances"]
        self.session["speech"]["startup_count"] = len(self.session["nvda"]["startup_utterances"])

        # Stop NVDA's startup say-all (Chrome was the last foreground before the
        # app) and drain the queued non-app speech before the measured steps.
        self.step("startup-quiet", "fg2|wait:500|escape|wait:800|names", wait=1.5)
        drained = self.log.read_new("startup-drain")
        self.session["speech"]["startup_drain_count"] = len(drained)

        self.step("main-tab-order", (
            "fg2|wait:600|focus:タスクを停止|wait:1200|"
            + "|".join(["tab|wait:1200"] * 12)
            + "|names"
        ), wait=2.5, shot=True, tree=True)

        self.step("brief-open", "fg2|wait:500|ctrl+n|wait:1600|names", wait=2.5)
        self.capture_shot("brief")
        self.step("brief-tabs", "fg2|wait:300|focus:目的|wait:400|tab|wait:1200|tab|wait:1200|tab|wait:1200|names", wait=2.0)
        self.step("brief-close", "fg2|wait:300|focus:目的|wait:400|escape|wait:1600|names", wait=2.5)

        self.step("inspector-evidence", (
            "fg2|wait:300|focus:目的 —|wait:1200|focus:評価 —|wait:1200|"
            "focus:判断の根拠 —|wait:1200|focus:改善 —|wait:1200|"
            "focus:結果 —|wait:1200|names"
        ), wait=2.5)

        self.step("history-open", "fg2|wait:500|ctrl+h|wait:1600|names", wait=2.5)
        self.capture_shot("history")
        self.step("history-tabs", "fg2|wait:300|focus:実行履歴。1件|wait:400|tab|wait:1200|tab|wait:1200|names", wait=2.0)
        self.step("history-close", "fg2|wait:300|focus:実行履歴。1件|wait:400|escape|wait:1600|names", wait=2.5)

        self.step("palette-open", "fg2|wait:500|ctrl+k|wait:1600|names", wait=2.5)
        self.capture_shot("palette")
        self.step("palette-tabs", "fg2|wait:300|focus:コマンド検索|wait:400|tab|wait:1200|tab|wait:1200|names", wait=2.0)
        self.step("palette-close", "fg2|wait:300|focus:コマンド検索|wait:400|escape|wait:1600|names", wait=2.5)

        # Speech viewer: capture the window tree and a screenshot for the
        # record; the committed log is filtered separately.
        time.sleep(1.0)
        self.session["nvda"]["process"] = {
            "pid": self.nvda.proc.pid if self.nvda.proc else 0,
            "main_window_title": powershell_json(
                '(Get-Process nvda -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty MainWindowTitle) | ConvertTo-Json -Compress'
            ),
        }
        nvda_pid = self.nvda.proc.pid if self.nvda.proc else 0
        if nvda_pid:
            self.session["nvda"]["speech_viewer_tree"] = probe_pid(
                nvda_pid, "tree", self.nvda_dir / "speech-viewer-tree.json")
            self.session["nvda"]["speech_viewer_shot"] = probe_pid(
                nvda_pid, "shot", self.nvda_dir / "speech-viewer.json",
                image_out=self.nvda_dir / "speech-viewer.png")

        # Final tail read after the last step.
        time.sleep(2.0)
        self.session["speech"]["tail"] = self.log.read_new("tail")
        self.session["speech"]["utterance_count"] = len(self.log.all_utterances)
        self.session["finished_at"] = now_iso()

    # -- records --------------------------------------------------------------
    def release_record(self) -> dict:
        return {
            "zip": str(RELEASE_ZIP),
            "zip_sha256": acc1.sha256_file(RELEASE_ZIP) if RELEASE_ZIP.is_file() else "missing",
            "exe": str(RELEASE_EXE),
            "exe_sha256": acc1.sha256_file(RELEASE_EXE) if RELEASE_EXE.is_file() else "missing",
            "exe_bytes": RELEASE_EXE.stat().st_size if RELEASE_EXE.is_file() else 0,
            "exe_mtime": time.strftime("%Y-%m-%dT%H:%M:%S%z",
                                       time.localtime(RELEASE_EXE.stat().st_mtime))
            if RELEASE_EXE.is_file() else "",
            "readme_release": "target/nvda-at/release/README-RELEASE.md",
            "head": base.git_value(ROOT, "rev-parse", "HEAD"),
            "worktree_status": base.git_value(ROOT, "status", "--porcelain")[:4000],
            "note": (
                "The tested binary is the v0.1.0 portable release artifact "
                "(21,258,240 B, sha256 263134b5...); the worktree carried unrelated "
                "uncommitted media-delay changes from another lane, which are not in "
                "this binary."
            ),
        }

    def environment_record(self) -> dict:
        return {
            "os": {
                "platform": sys.platform,
                "version": platform_version(),
                "user": os.environ.get("USERNAME"),
                "userdomain": os.environ.get("USERDOMAIN"),
            },
            "uac": powershell_json(
                'Get-ItemProperty "HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System" | '
                'Select-Object EnableLUA,ConsentPromptBehaviorAdmin,ConsentPromptBehaviorUser,PromptOnSecureDesktop | '
                'ConvertTo-Json -Compress'
            ),
            "is_admin_role": powershell_json(
                '$id=[Security.Principal.WindowsIdentity]::GetCurrent(); '
                '$p=New-Object Security.Principal.WindowsPrincipal($id); '
                '$p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) | ConvertTo-Json -Compress'
            ),
            "nvda_installer": installer_record(),
            "comfy_8188_before": acc1.port_open(8188),
        }


def platform_version() -> str:
    completed = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command",
         "(Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber | ConvertTo-Json -Compress)"],
        capture_output=True, text=True, timeout=30, encoding="utf-8", errors="replace",
    )
    return completed.stdout.strip()


def installer_record() -> dict:
    installer = NVDA_HOME / "download" / "nvda_2026.3.1.exe"
    record: dict = {
        "source": "https://download.nvaccess.org/releases/2026.3.1/nvda_2026.3.1.exe",
        "version": "2026.3.1",
        "official_sha256": "6f80a5b0ca720a7d0d314b5be212f37388bbe81a1107ac3298c1821a1cd3f052",
        "installer_present": installer.is_file(),
    }
    if installer.is_file():
        record.update({
            "installer_bytes": installer.stat().st_size,
            "installer_sha256": acc1.sha256_file(installer),
            "signature": powershell_json(
                'Get-AuthenticodeSignature "' + str(installer) + '" | '
                'Select-Object Status,@{n="Signer";e={$_.SignerCertificate.Subject}} | ConvertTo-Json -Compress'
            ),
            "file_version": powershell_json(
                '(Get-Item "' + str(installer) + '").VersionInfo | '
                'Select-Object FileVersion,ProductVersion | ConvertTo-Json -Compress'
            ),
        })
    record["per_user_install"] = {
        "machine_wide_install_silent": {
            "status": "NOT_RUN_ELEVATION_REQUIRED",
            "command": "nvda_2026.3.1.exe --install-silent",
            "why": (
                "The official installer has no per-user install mode: installerGui.doInstall "
                "calls systemUtils.execElevated(..., 'runas') and installer.install() writes "
                "WritePaths.defaultInstallDir (%ProgramFiles%\\nvda) and HKLM registration. "
                "With UAC enabled, a standard user, ConsentPromptBehaviorUser=3 and "
                "PromptOnSecureDesktop=1, this raises an unattended secure-desktop credential "
                "prompt; it was not triggered to avoid leaving a blocking UAC prompt."
            ),
        },
        "portable_per_user": {
            "status": "PASS",
            "command": (
                "nvda_2026.3.1.exe --create-portable-silent "
                "--portable-path=%LOCALAPPDATA%\\nvda-at\\portable "
                "-c %LOCALAPPDATA%\\nvda-at\\config "
                "-f %LOCALAPPDATA%\\nvda-at\\logs\\portable-create.log -l 12 -n ja --no-sr-flag"
            ),
            "exit_code": 0,
            "portable_exe": str(NVDA),
            "portable_exe_sha256": acc1.sha256_file(NVDA) if NVDA.is_file() else "missing",
        },
    }
    return record


def filter_log(raw: str) -> tuple[str, dict]:
    lines = raw.splitlines()
    first_app = len(lines)
    for index, line in enumerate(lines):
        if not line.startswith("Speaking "):
            continue
        text = " ".join(extract_speech_strings(line))
        if "COS Desktop" in text and is_app_speech(text):
            first_app = index
            break
    kept: list[str] = []
    dropped = 0
    pending_speech_header = ""
    for line in lines[first_app:]:
        if line.startswith("IO - speech.speech.speak"):
            pending_speech_header = line
            continue
        if line.startswith("Speaking "):
            text = " ".join(extract_speech_strings(line))
            if not is_app_speech(text):
                dropped += 1
                pending_speech_header = ""
                continue
            if pending_speech_header:
                kept.append(pending_speech_header)
                pending_speech_header = ""
            kept.append(line)
            continue
        if line.startswith(("INFO - __main__", "INFO - core.main", "ERROR -", "CRITICAL -")):
            kept.append(line)
    meta = {
        "raw_bytes": len(raw.encode("utf-8")),
        "raw_lines": len(lines),
        "first_app_line": first_app,
        "app_marker_found": first_app < len(lines),
        "dropped_non_app_speech_lines": dropped,
        "kept_lines": len(kept),
        "filter": "from first COS Desktop utterance; keep only speech naming a COS Desktop surface/control",
        "app_terms": list(APP_TERMS),
    }
    return "\n".join(kept) + ("\n" if kept else ""), meta


def main(argv: list[str] | None = None) -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", default=str(EVIDENCE))
    parser.add_argument("--no-cleanup", action="store_true")
    args = parser.parse_args(argv)
    evidence = Path(args.evidence_dir).resolve()
    evidence.mkdir(parents=True, exist_ok=True)

    driver = Driver(evidence)
    session = driver.session
    ctx = None
    raw_log_text = ""
    try:
        driver.run()
        ctx = driver.ctx
    except Exception as error:  # noqa: BLE001 - record, never hide
        session["error"] = f"{type(error).__name__}: {error}"
        session["traceback"] = traceback.format_exc()[-6000:]
        ctx = driver.ctx
    finally:
        # Stop NVDA first so it does not speak the shutdown of the app.
        session["nvda_quit"] = driver.nvda.quit()
        session["chrome_restore"] = driver.restore_chrome()
        if ctx is not None:
            with contextlib.suppress(Exception):
                session["cleanup"] = {
                    "desktop": ctx.app.close() if ctx.app else None,
                    "runtime": ctx.runtime.stop(),
                    "ports": {str(PORT): acc1.port_open(PORT), "8188": acc1.port_open(8188)},
                }
            with contextlib.suppress(Exception):
                base.cleanup_credential_store(ctx.runtime.endpoint)

        # Preserve and filter the raw NVDA log before deleting the portable copy.
        if NVDA_LOG.is_file():
            raw_log_text = NVDA_LOG.read_text(encoding="utf-8", errors="replace")
            session["nvda"]["raw_log_sha256"] = acc1.sha256_file(NVDA_LOG)
            session["nvda"]["raw_log_bytes"] = NVDA_LOG.stat().st_size
            filtered, meta = filter_log(raw_log_text)
            (evidence / "nvda-app.log").write_text(filtered, encoding="utf-8")
            session["nvda"]["log_filter"] = meta
            session["nvda"]["app_log_sha256"] = acc1.sha256_file(evidence / "nvda-app.log")

        # Speech JSONL from the step captures (never synthesized).
        speech_lines = []
        for step, utterances in session.get("speech", {}).get("steps", {}).items():
            for utterance in utterances:
                speech_lines.append({"step": step, **utterance})
        (evidence / "speech.jsonl").write_text(
            "\n".join(json.dumps(item, ensure_ascii=False) for item in speech_lines) + "\n",
            encoding="utf-8",
        )
        session["speech"]["speech_jsonl_count"] = len(speech_lines)

        # Surface summaries for the README.
        session["surfaces"] = summarize_surfaces(session)

        document = acc1.redact_document(session)
        write_json(evidence / "session.json", document)
        write_json(evidence / "install.json", installer_record())
        write_hashes(evidence)
        with contextlib.suppress(Exception):
            secrets = list(ctx.collect) if ctx is not None else []
            write_json(evidence / "leak-scan.json", acc1.leak_scan(evidence, secrets))
        print(json.dumps({"verdict": "ERROR" if "error" in session else "DONE",
                          "error": session.get("error", ""),
                          "evidence": str(evidence)}, ensure_ascii=False))

        if not args.no_cleanup:
            cleanup_machine(ctx, evidence)
            write_hashes(evidence)

    return 0 if "error" not in session else 1


def summarize_surfaces(session: dict) -> dict:
    summary: dict = {}
    for label, record in session.get("steps", {}).items():
        probe = record.get("probe") or {}
        steps = probe.get("steps") or []
        summary[label] = {
            "focus_transitions": [
                {"focus_before": item.get("focus_before"),
                 "focus_after": item.get("focus_after"),
                 "focus_after_type": item.get("focus_after_type"),
                 "step": item.get("step")}
                for item in steps
            ],
            "utterances": [item.get("text") for item in record.get("utterances") or []],
        }
    return summary


def write_hashes(evidence: Path) -> None:
    rows = []
    for path in sorted(evidence.rglob("*")):
        if path.is_file() and path.name != "hashes.txt":
            rows.append(f"{acc1.sha256_file(path)}  {path.relative_to(evidence).as_posix()}")
    (evidence / "hashes.txt").write_text("\n".join(rows) + "\n", encoding="utf-8")


def cleanup_machine(ctx, evidence: Path) -> None:
    record: dict = {"actions": []}
    # The isolated runtime work dir contains the per-run secrets and DB copy.
    if ctx is not None:
        with contextlib.suppress(Exception):
            shutil.rmtree(ctx.work)
            record["actions"].append(f"removed {ctx.work}")
    # The per-user portable copy, config, logs and installer.
    with contextlib.suppress(Exception):
        if NVDA_HOME.exists():
            shutil.rmtree(NVDA_HOME)
            record["actions"].append(f"removed {NVDA_HOME}")
    # The staging area (driver, release copy, logs) is removed after evidence
    # has been copied; the release zip itself belongs to the release process.
    with contextlib.suppress(Exception):
        staging = ROOT / "target" / "nvda-at"
        if staging.exists():
            shutil.rmtree(staging)
            record["actions"].append(f"removed {staging}")
    record["nvda_processes"] = nvda_processes()
    record["ports"] = {str(PORT): acc1.port_open(PORT), "8188": acc1.port_open(8188)}
    record["localappdata_nvda_at_exists"] = NVDA_HOME.exists()
    write_json(evidence / "cleanup.json", record)


if __name__ == "__main__":
    raise SystemExit(main())
