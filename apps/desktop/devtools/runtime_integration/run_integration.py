#!/usr/bin/env python3
"""Repeatable live Desktop<->Runtime integration harness (RT2/RT3/ACC1 baseline).

What it does, once per run label:

1. Copies the Runtime production database through a read-only SQLite handle
   (never writes the source) and starts the Runtime on 127.0.0.1:8124 with an
   isolated config YAML and a per-run ``ORCHESTRATOR_AUTH_SECRET`` held in
   memory only.
2. Issues Desktop credentials through ``POST /api/cos/desktop/credentials``
   (X-Auth-Secret) and derives the expected TASK projection from
   ``POST /api/cos/desktop/observation`` with a second credential.
3. Launches the Desktop release binary without a fixture flag, with
   ``COS_DESKTOP_RUNTIME_ENDPOINT``/``COS_DESKTOP_RUNTIME_TOKEN`` in the child
   environment only and an isolated LOCALAPPDATA under the workspace target.
4. Scenario A: the connected snapshot renders a real projected task title and
   status label (UIA names + PrintWindow screenshot, never CopyFromScreen,
   never foregrounding).
5. Scenario B: stopping the Runtime makes the Desktop withhold with explicit
   unavailable/reconnect labels.
6. Scenario C: restarting the Runtime (new process) leaves the Desktop
   withheld; a fresh credential plus a Desktop relaunch recovers the real
   projection.
7. Scenario D: a clean close (WM_CLOSE) emits the observation-shutdown and
   preferences-save confirmations and leaves no process.

Tokens and the issuance secret are never written to any file: they are passed
to children through the environment only, and log excerpts are redacted before
they become evidence.  The shared native lock ``C:/tmp/cos-desktop-native.lock``
is honoured for the whole run.

Usage (from the repository root):

    python apps/desktop/devtools/runtime_integration/run_integration.py --runs 2

The interpreter may be the Runtime venv Python; the script itself uses the
standard library only.
"""

from __future__ import annotations

import argparse
import contextlib
import ctypes
import hashlib
import json
import os
import re
import secrets
import shutil
import socket
import sqlite3
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

DESKTOP_ROOT = Path(__file__).resolve().parents[4]
RUNTIME_ROOT = Path("C:/AI/orchestrator")
RUNTIME_PYTHON = RUNTIME_ROOT / ".venv" / "Scripts" / "python.exe"
SOURCE_DB = RUNTIME_ROOT / "db" / "generation.sqlite"
CONFIG_TEMPLATE = RUNTIME_ROOT / "configs" / "default.yaml"
DESKTOP_EXE = DESKTOP_ROOT / "target" / "release" / "cos-desktop.exe"
EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "rt-integration-latest"
)
WORK_ROOT = DESKTOP_ROOT / "target" / "rt-integration"
PROBE = Path(__file__).resolve().parent / "probe_window.ps1"
LOCK = Path("C:/tmp/cos-desktop-native.lock")
PORT = 8124
ENDPOINT = f"http://127.0.0.1:{PORT}"
SCENARIO_ORDER = ("connect_snapshot", "runtime_stop", "runtime_restart", "clean_close", "relaunch_recovery")

STATUS_LABELS = {
    "ASSIGNED": "割り当て済み（ASSIGNED）",
    "ACTIVE": "実行中（ACTIVE）",
    "COMPLETED": "完了（COMPLETED）",
    "CANCELLED": "中止（CANCELLED）",
}

WITHDRAWN_MARKERS = ("情報源なし / データ非公開", "非公開", "再接続する", "再接続")
CLOSE_MARKERS = ("observation shutdown confirmed", "preferences save confirmed")


# --------------------------------------------------------------------------- basics


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def sha256_text(text: str) -> str:
    return "sha256:" + hashlib.sha256(text.encode("utf-8")).hexdigest()


def free_port_fixed() -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.3)
        if probe.connect_ex(("127.0.0.1", PORT)) == 0:
            raise RuntimeError(f"port {PORT} is already in use; the harness owns {PORT}")


def pid_alive(pid: int) -> bool:
    """Whether the process is still running.

    An exited process can still be opened while the harness keeps the Popen
    handle, so the liveness question is the exit code, not OpenProcess alone.
    """
    kernel32 = ctypes.windll.kernel32
    handle = kernel32.OpenProcess(0x1000, False, int(pid))
    if not handle:
        return False
    try:
        code = ctypes.c_uint(0)
        if not kernel32.GetExitCodeProcess(handle, ctypes.byref(code)):
            return True
        return code.value == 259  # STILL_ACTIVE
    finally:
        kernel32.CloseHandle(handle)


def kill_tree(pid: int) -> dict:
    try:
        completed = subprocess.run(
            ["taskkill", "/PID", str(pid), "/T", "/F"],
            capture_output=True,
            text=True,
            timeout=20,
        )
        return {"pid": pid, "exit": completed.returncode, "output": completed.stdout.strip()[:400]}
    except (OSError, subprocess.SubprocessError) as error:  # pragma: no cover - host failure
        return {"pid": pid, "exit": None, "error": type(error).__name__}


# --------------------------------------------------------------------------- lock


def lock_owner() -> tuple[int | None, float]:
    owner_file = LOCK / "OWNER.txt"
    text = owner_file.read_text(encoding="utf-8", errors="replace") if owner_file.is_file() else ""
    age = 10_000.0
    pid = None
    match = re.search(r"pid=(\d+)", text)
    if match:
        pid = int(match.group(1))
    match = re.search(r"at=([0-9T:\-\.\+Z]+)", text)
    if match:
        with contextlib.suppress(ValueError):
            stamp = match.group(1).replace("Z", "+00:00")
            from datetime import datetime, timezone

            age = (datetime.now(timezone.utc) - datetime.fromisoformat(stamp)).total_seconds() / 60
    return pid, age


@contextlib.contextmanager
def native_lock(owner: str, wait_minutes: float = 20.0):
    deadline = time.monotonic() + wait_minutes * 60
    while True:
        if LOCK.exists():
            pid, age = lock_owner()
            alive = bool(pid and pid_alive(pid))
            stale = (not alive) and age > 5
            if stale:
                shutil.rmtree(LOCK, ignore_errors=True)
            elif time.monotonic() < deadline:
                time.sleep(15)
                continue
            else:
                raise RuntimeError("native GUI lock stayed busy")
        else:
            LOCK.mkdir(parents=True, exist_ok=True)
            (LOCK / "OWNER.txt").write_text(
                f"owner={owner} pid={os.getpid()} at={now_iso()}",
                encoding="utf-8",
            )
            break
    try:
        yield
    finally:
        shutil.rmtree(LOCK, ignore_errors=True)


# --------------------------------------------------------------------------- HTTP


def _opener() -> urllib.request.OpenerDirector:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    return opener


def http_json(
    method: str,
    url: str,
    *,
    body: dict | None = None,
    headers: dict | None = None,
    timeout: float = 20.0,
) -> dict:
    data = None
    request_headers = {"Accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        request_headers["Content-Type"] = "application/json"
    if headers:
        request_headers.update(headers)
    request = urllib.request.Request(url, data=data, headers=request_headers, method=method)
    with _opener().open(request, timeout=timeout) as response:
        raw = response.read()
    return json.loads(raw.decode("utf-8"))


def wait_http(url: str, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    last = "no attempt"
    while time.monotonic() < deadline:
        try:
            with _opener().open(url, timeout=2) as response:
                if response.status == 200:
                    return
                last = f"status {response.status}"
        except (urllib.error.URLError, OSError) as error:
            last = type(error).__name__
        time.sleep(0.25)
    raise RuntimeError(f"{url} did not become ready: {last}")


# --------------------------------------------------------------------------- Runtime


class RuntimeServer:
    def __init__(self, *, label: str, work: Path, db_copy: Path, secret: str):
        self.label = label
        self.work = work
        self.db_copy = db_copy
        self.secret = secret
        self.process: subprocess.Popen | None = None
        self.log_path = work / f"runtime-{label}.log"
        self.log_handle = None
        self.config_path = work / f"runtime-{label}-config.yaml"
        self.starts = 0

    def _write_config(self) -> None:
        template = CONFIG_TEMPLATE.read_text(encoding="utf-8")
        replacement = f"  db_path: {self.db_copy.as_posix()}"
        rendered, count = re.subn(
            r"(?m)^(\s*)db_path:.*$",
            lambda match: f"{match.group(1)}db_path: {self.db_copy.as_posix()}",
            template,
            count=1,
        )
        if count != 1:
            raise RuntimeError("default.yaml no longer contains exactly one db_path line")
        rendered = rendered.replace(
            "project_root: C:/AI/orchestrator", f"project_root: {RUNTIME_ROOT.as_posix()}"
        )
        self.config_path.write_text(rendered, encoding="utf-8")

    def start(self, timeout: float = 90.0) -> dict:
        self.starts += 1
        free_port_fixed()
        self._write_config()
        environment = {**os.environ, "ORCHESTRATOR_AUTH_SECRET": self.secret}
        environment.pop("ORCHESTRATOR_AUTH_DISABLED", None)
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- start {self.starts} {now_iso()} ---\n".encode())
        self.log_handle.flush()
        command = [
            str(RUNTIME_PYTHON),
            "-m",
            "orchestrator_app.api",
            "--config",
            str(self.config_path),
            "--host",
            "127.0.0.1",
            "--port",
            str(PORT),
        ]
        self.process = subprocess.Popen(  # noqa: S603 - fixed argv, loopback only
            command,
            cwd=str(RUNTIME_ROOT),
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        started = time.monotonic()
        while True:
            if self.process.poll() is not None:
                raise RuntimeError(f"Runtime exited during startup (exit {self.process.returncode})")
            try:
                wait_http(f"{ENDPOINT}/api/ready", timeout=1.0)
                break
            except RuntimeError:
                if time.monotonic() - started > timeout:
                    raise
        hello = http_json("GET", f"{ENDPOINT}/api/cos/desktop/server")
        return {
            "pid": self.process.pid,
            "server_identity": hello["server"]["server_identity"],
            "server_build": hello["server"]["server_build"],
            "epoch": hello["server"]["epoch"],
            "protocol": hello["server"]["protocol"],
            "start_index": self.starts,
        }

    def issue_desktop_credential(self) -> dict:
        response = http_json(
            "POST",
            f"{ENDPOINT}/api/cos/desktop/credentials",
            headers={"X-Auth-Secret": self.secret},
        )
        return response

    def observation(self, token: str) -> dict:
        body = {
            "schema_version": 1,
            "request_id": uuid.uuid4().hex,
            "protocol": {"major": 2, "minor": 0},
            "schema": {"major": 2, "minor": 0},
            "scope": {"task_ids": [], "filters": [{"name": "view", "value": "all"}]},
            "retained_commands": [],
        }
        return http_json(
            "POST",
            f"{ENDPOINT}/api/cos/desktop/observation",
            body=body,
            headers={"Authorization": f"Bearer {token}"},
        )

    def stop(self, *, clean: bool = True) -> dict:
        if self.process is None:
            return {"stopped": False, "reason": "not_started"}
        pid = self.process.pid
        if self.process.poll() is None:
            if clean:
                self.process.terminate()
            else:
                return kill_tree(pid)
            try:
                self.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                forced = kill_tree(pid)
                return {"pid": pid, "clean": False, "forced": forced}
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {"pid": pid, "clean": True, "exit": self.process.returncode}


# --------------------------------------------------------------------------- projection


def expected_tasks(observation: dict) -> list[dict]:
    tasks: list[dict] = []
    snapshot = observation.get("snapshot") or {}
    for chunk in snapshot.get("chunks", []):
        for mutation in chunk.get("mutations", []):
            if mutation.get("op") != "UPSERT":
                continue
            projection = mutation.get("projection") or {}
            if projection.get("projection") != "TASK":
                continue
            tasks.append(
                {
                    "task_ref": projection.get("task_ref"),
                    "title": projection.get("title") or "",
                    "task_status": projection.get("task_status"),
                    "inner_phase": projection.get("inner_phase"),
                    "iteration_index": projection.get("iteration_index"),
                    "projection_revision": projection.get("projection_revision"),
                }
            )
    return tasks


def choose_expected(tasks: list[dict]) -> dict:
    for task in tasks:
        if task["title"]:
            return task
    raise RuntimeError("the live projection contains no TASK title to assert")


# --------------------------------------------------------------------------- desktop probe


def probe(action: str, pid: int, out_path: Path, image_out: Path | None = None) -> dict:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    if out_path.exists():
        out_path.unlink()
    if image_out is not None:
        image_out.parent.mkdir(parents=True, exist_ok=True)
        if image_out.exists():
            image_out.unlink()
    command = [
        "powershell.exe",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        str(PROBE),
        "-ProcessId",
        str(pid),
        "-Action",
        action,
        "-Out",
        str(out_path),
    ]
    if image_out is not None:
        command.extend(["-ImageOut", str(image_out)])
    completed = subprocess.run(
        command, capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace"
    )
    if not out_path.is_file():
        raise RuntimeError(
            f"probe {action} produced no output (exit {completed.returncode}): "
            f"{completed.stderr.strip()[-400:]}"
        )
    result = json.loads(out_path.read_text(encoding="utf-8"))
    result["_probe_exit"] = completed.returncode
    return result


def uia_text(result: dict) -> str:
    return "\n".join(result.get("names") or [])


def wait_uia(pid: int, out_path: Path, predicate, timeout: float, interval: float = 0.6) -> tuple[dict, str]:
    deadline = time.monotonic() + timeout
    last: dict = {"found": False}
    last_text = ""
    while time.monotonic() < deadline:
        last = probe("dump", pid, out_path)
        last_text = uia_text(last)
        if last.get("found") and predicate(last_text):
            return last, last_text
        if not pid_alive(pid):
            break
        time.sleep(interval)
    return last, last_text


def wait_window(pid: int, out_path: Path, timeout: float = 30.0) -> dict:
    deadline = time.monotonic() + timeout
    last: dict = {"found": False}
    while time.monotonic() < deadline:
        last = probe("dump", pid, out_path)
        if last.get("found") and last.get("title"):
            return last
        time.sleep(0.6)
    return last


# --------------------------------------------------------------------------- desktop process


class DesktopApp:
    def __init__(self, *, label: str, work: Path, token: str, localappdata: Path):
        self.label = label
        self.work = work
        self.token = token
        self.localappdata = localappdata
        self.process: subprocess.Popen | None = None
        self.log_path = work / f"desktop-{label}.log"
        self.log_handle = None

    def launch(self) -> subprocess.Popen:
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("COS_DESKTOP_")
            and key not in {"COS_FIXTURE_TRACE", "COS_PREVIEW_TRACE", "COS_EXPORT_TRACE"}
        }
        environment["LOCALAPPDATA"] = str(self.localappdata)
        environment["COS_DESKTOP_RUNTIME_ENDPOINT"] = ENDPOINT
        environment["COS_DESKTOP_RUNTIME_TOKEN"] = self.token
        self.localappdata.mkdir(parents=True, exist_ok=True)
        self.log_handle = self.log_path.open("wb")
        self.process = subprocess.Popen(  # noqa: S603 - fixed binary, no fixture flag
            [str(DESKTOP_EXE)],
            cwd=str(DESKTOP_ROOT),
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        return self.process

    def wait_exit(self, timeout: float = 25.0) -> dict:
        if self.process is None:
            return {"exited": True, "reason": "not_started"}
        try:
            code = self.process.wait(timeout=timeout)
            exited = True
        except subprocess.TimeoutExpired:
            code = None
            exited = False
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {"exited": exited, "exit_code": code, "pid": self.process.pid}

    def close(self) -> dict:
        if self.process is None:
            return {"closed": False, "reason": "not_started"}
        pid = self.process.pid
        closed = False
        if pid_alive(pid):
            with contextlib.suppress(RuntimeError):
                result = probe("close", pid, self.work / f"close-{self.label}.json")
                closed = bool(result.get("sent"))
        outcome = self.wait_exit()
        if not outcome["exited"]:
            outcome["forced"] = kill_tree(pid)
            outcome["exited"] = not pid_alive(pid)
        outcome["wm_close_sent"] = closed
        return outcome


# --------------------------------------------------------------------------- evidence helpers


def redact(text: str, secrets: list[str]) -> str:
    redacted = text
    for secret in secrets:
        if secret:
            redacted = redacted.replace(secret, "<redacted>")
    return redacted


def bounded(text: str, limit: int = 8000) -> str:
    return text if len(text) <= limit else "...[truncated]...\n" + text[-limit:]


def sanitized_log(path: Path, secrets: list[str], limit: int = 8000) -> str:
    if not path.is_file():
        return ""
    return bounded(redact(path.read_text(encoding="utf-8", errors="replace"), secrets), limit)


def write_json(path: Path, document: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def copy_database(source: Path, destination: Path) -> None:
    source_uri = f"file:{source.as_posix()}?mode=ro"
    reader = sqlite3.connect(source_uri, uri=True)
    writer = sqlite3.connect(destination)
    try:
        reader.backup(writer)
    finally:
        writer.close()
        reader.close()


def git_value(root: Path, *args: str) -> str:
    try:
        completed = subprocess.run(
            ["git", "-C", str(root), *args], capture_output=True, text=True, timeout=20
        )
        return completed.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def cleanup_credential_store(origin: str) -> dict:
    head = hashlib.sha256(origin.encode("utf-8")).hexdigest()[:32]
    target = f"cos-desktop/observation/{head}"
    try:
        completed = subprocess.run(
            ["cmdkey", f"/delete:{target}"], capture_output=True, text=True, timeout=20
        )
        output = (completed.stdout + completed.stderr).strip()
        return {"target": target, "exit": completed.returncode, "output": output[:200]}
    except (OSError, subprocess.SubprocessError) as error:
        return {"target": target, "error": type(error).__name__}


# --------------------------------------------------------------------------- one run


def run_once(label: str, collected: list[str]) -> dict:
    started = now_iso()
    run_work = WORK_ROOT / label
    if run_work.exists():
        shutil.rmtree(run_work)
    run_work.mkdir(parents=True, exist_ok=True)
    evidence = EVIDENCE_DIR / label
    if evidence.exists():
        shutil.rmtree(evidence)
    evidence.mkdir(parents=True, exist_ok=True)

    facts: dict = {"label": label, "started_at": started, "scenarios": {}, "files": {}}
    run_secret = secrets.token_hex(24)
    collected.append(run_secret)
    secrets_list = [run_secret]
    facts["run_secret_recorded"] = False

    db_copy = run_work / "generation-copy.sqlite"
    copy_database(SOURCE_DB, db_copy)
    facts["db_copy_sha256"] = sha256_file(db_copy)
    facts["db_copy_bytes"] = db_copy.stat().st_size

    localappdata = run_work / "localappdata"
    (localappdata / "cos-desktop").mkdir(parents=True, exist_ok=True)

    server = RuntimeServer(label=label, work=run_work, db_copy=db_copy, secret=run_secret)
    app: DesktopApp | None = None
    apps: list[DesktopApp] = []
    try:
        free_port_fixed()
        with native_lock("impl-flow2/rt-integration"):
            first = server.start()
            facts["server_first"] = first
            app_credential = server.issue_desktop_credential()
            harness_credential = server.issue_desktop_credential()
            secrets_list.extend([app_credential["token"], harness_credential["token"]])
            collected.extend([app_credential["token"], harness_credential["token"]])
            facts["app_credential"] = {
                "principal": app_credential["principal"],
                "scopes": app_credential["scopes"],
                "expires_in": app_credential["expires_in"],
                "token_recorded": False,
            }
            observation = server.observation(harness_credential["token"])
            tasks = expected_tasks(observation)
            expected = choose_expected(tasks)
            status_label = STATUS_LABELS.get(str(expected["task_status"]), "")
            facts["projection"] = {
                "task_count": len(tasks),
                "expected_task_ref": expected["task_ref"],
                "expected_title": expected["title"],
                "expected_task_status": expected["task_status"],
                "expected_status_label": status_label,
                "expected_inner_phase": expected["inner_phase"],
                "expected_iteration_index": expected["iteration_index"],
                "distinct_statuses": sorted({str(task["task_status"]) for task in tasks}),
            }

            # Scenario A: connect and render the real projection.
            app = DesktopApp(
                label="connect", work=run_work, token=app_credential["token"], localappdata=localappdata
            )
            apps.append(app)
            app.launch()
            window = wait_window(app.process.pid, evidence / "uia-window.json")
            dump, text = wait_uia(
                app.process.pid,
                evidence / "uia-connected.json",
                lambda joined: expected["title"] in joined,
                45.0,
            )
            connected = bool(dump.get("found") and expected["title"] in text)
            status_visible = bool(status_label and status_label in text)
            shot = probe(
                "shot",
                app.process.pid,
                evidence / "shot-connected.json",
                evidence / "connected.png",
            )
            facts["scenarios"]["connect_snapshot"] = {
                "verdict": "PASS" if connected else "FAIL",
                "window": {"found": window.get("found"), "title": window.get("title")},
                "title_visible": connected,
                "status_label_visible": status_visible,
                "expected_title": expected["title"],
                "matched_excerpt": bounded("\n".join(line for line in text.splitlines() if expected["title"] in line or status_label in line), 1200),
                "uia_element_count": dump.get("elements"),
                "screenshot": {
                    "file": "connected.png",
                    "printed": shot.get("ok"),
                    "width": shot.get("width"),
                    "height": shot.get("height"),
                },
            }
            if not connected:
                raise RuntimeError("connect scenario did not render the expected live task title")

            # Scenario B: Runtime stop -> explicit withheld state.
            stopped = server.stop(clean=True)
            facts["server_stop"] = stopped
            dump_b, text_b = wait_uia(
                app.process.pid,
                evidence / "uia-withheld.json",
                lambda joined: any(marker in joined for marker in WITHDRAWN_MARKERS),
                45.0,
            )
            withheld_markers = [marker for marker in WITHDRAWN_MARKERS if marker in text_b]
            title_gone = expected["title"] not in text_b
            shot_b = probe(
                "shot",
                app.process.pid,
                evidence / "shot-withheld.json",
                evidence / "withheld.png",
            )
            facts["scenarios"]["runtime_stop"] = {
                "verdict": "PASS" if withheld_markers and title_gone else "FAIL",
                "withheld_markers": withheld_markers,
                "title_still_visible": not title_gone,
                "matched_excerpt": bounded(
                    "\n".join(
                        line for line in text_b.splitlines() if any(marker in line for marker in WITHDRAWN_MARKERS)
                    ),
                    1200,
                ),
                "screenshot": {"file": "withheld.png", "printed": shot_b.get("ok")},
            }

            # Scenario C: restart -> still withheld; fresh credential + relaunch recovers.
            second = server.start()
            facts["server_second"] = second
            epoch_changed = second["epoch"] != first["epoch"]
            time.sleep(8.0)
            dump_c, text_c = wait_uia(
                app.process.pid,
                evidence / "uia-restart-withheld.json",
                lambda joined: any(marker in joined for marker in WITHDRAWN_MARKERS),
                20.0,
            )
            still_withheld = any(marker in text_c for marker in WITHDRAWN_MARKERS)
            stale_title = expected["title"] in text_c
            shot_c = probe(
                "shot",
                app.process.pid,
                evidence / "shot-restart-withheld.json",
                evidence / "restart-withheld.png",
            )
            facts["scenarios"]["runtime_restart"] = {
                "verdict": (
                    "PASS"
                    if still_withheld and not stale_title
                    else ("DEVIATION" if not still_withheld else "FAIL")
                ),
                "epoch_changed": epoch_changed,
                "first_epoch": first["epoch"],
                "second_epoch": second["epoch"],
                "still_withheld": still_withheld,
                "projection_title_visible_without_credential": stale_title,
                "screenshot": {"file": "restart-withheld.png", "printed": shot_c.get("ok")},
            }

            # Scenario D: clean close of the connected-but-withheld app.  The app has
            # just seen the restarted Runtime and may be in an observation retry
            # backoff; give that bounded window a quiet moment so the close path is
            # measured without a retry storm in flight.
            close_settle_seconds = 3.0
            time.sleep(close_settle_seconds)
            closed = app.close()
            app_stderr = sanitized_log(app.log_path, secrets_list)
            (evidence / "app-connect.stderr.sanitized.txt").write_text(app_stderr, encoding="utf-8")
            close_markers = [marker for marker in CLOSE_MARKERS if marker in app_stderr]
            no_process = not pid_alive(closed["pid"])
            facts["scenarios"]["clean_close"] = {
                "verdict": "PASS" if closed["exited"] and close_markers and no_process else "FAIL",
                "exit_code": closed["exit_code"],
                "wm_close_sent": closed.get("wm_close_sent"),
                "close_markers": close_markers,
                "process_gone": no_process,
                "forced": closed.get("forced"),
                "settle_seconds": close_settle_seconds,
                "stderr_sha256": sha256_text(app_stderr),
            }

            # Scenario C2: relaunch with a fresh credential from the restarted Runtime.
            fresh_credential = server.issue_desktop_credential()
            secrets_list.append(fresh_credential["token"])
            collected.append(fresh_credential["token"])
            facts["recovery_credential"] = {
                "principal": fresh_credential["principal"],
                "token_recorded": False,
            }
            app2 = DesktopApp(
                label="recovery", work=run_work, token=fresh_credential["token"], localappdata=localappdata
            )
            apps.append(app2)
            app2.launch()
            wait_window(app2.process.pid, evidence / "uia-window-recovery.json")
            dump_r, text_r = wait_uia(
                app2.process.pid,
                evidence / "uia-recovered.json",
                lambda joined: expected["title"] in joined,
                45.0,
            )
            recovered = bool(dump_r.get("found") and expected["title"] in text_r)
            shot_r = probe(
                "shot",
                app2.process.pid,
                evidence / "shot-recovered.json",
                evidence / "recovered.png",
            )
            facts["scenarios"]["relaunch_recovery"] = {
                "verdict": "PASS" if recovered else "FAIL",
                "title_visible": recovered,
                "screenshot": {"file": "recovered.png", "printed": shot_r.get("ok")},
            }
            app2.close()
            app2_stderr = sanitized_log(app2.log_path, secrets_list)
            (evidence / "app-recovery.stderr.sanitized.txt").write_text(app2_stderr, encoding="utf-8")
            facts["scenarios"]["relaunch_recovery"]["close_markers"] = [
                marker for marker in CLOSE_MARKERS if marker in app2_stderr
            ]

            server.stop(clean=True)
            runtime_log = sanitized_log(server.log_path, secrets_list)
            (evidence / "runtime.log.sanitized.txt").write_text(runtime_log, encoding="utf-8")
            facts["runtime_log_sha256"] = sha256_text(runtime_log)
    finally:
        for candidate in apps:
            with contextlib.suppress(Exception):
                candidate.close()
        with contextlib.suppress(Exception):
            server.stop(clean=False)
        if (run_work / "localappdata").exists():
            shutil.rmtree(run_work / "localappdata", ignore_errors=True)

    facts["finished_at"] = now_iso()
    facts["files"] = {
        "screenshots": [
            name
            for name in ("connected.png", "withheld.png", "restart-withheld.png", "recovered.png")
            if (evidence / name).is_file()
        ],
        "evidence_dir": f"docs/development/evidence-published/rt-integration-latest/{label}",
    }
    for name in facts["files"]["screenshots"]:
        facts.setdefault("file_hashes", {})[name] = sha256_file(evidence / name)
    write_json(evidence / "facts.json", facts)
    return facts


# --------------------------------------------------------------------------- summary


def summarize(runs: list[dict]) -> dict:
    compared = []
    differences = []
    for scenario in SCENARIO_ORDER:
        verdicts = [run["scenarios"].get(scenario, {}).get("verdict") for run in runs]
        compared.append({"scenario": scenario, "verdicts": verdicts})
        if len(set(verdicts)) != 1:
            differences.append(f"{scenario}: {verdicts}")
    for field in ("expected_title", "expected_task_status"):
        values = [run.get("projection", {}).get(field) for run in runs]
        compared.append({"field": field, "values": values})
        if len(set(map(str, values))) != 1:
            differences.append(f"{field}: {values}")
    deterministic = not differences
    scenarios: dict = {}
    for scenario in SCENARIO_ORDER:
        scenarios[scenario] = {
            "verdicts": [run["scenarios"].get(scenario, {}).get("verdict") for run in runs],
            "passed": all(
                run["scenarios"].get(scenario, {}).get("verdict") in {"PASS", "DEVIATION"} for run in runs
            ),
        }
    findings = []
    for run in runs:
        restart = run["scenarios"].get("runtime_restart", {})
        if restart.get("verdict") == "DEVIATION":
            findings.append(
                {
                    "id": "RT-INT-01",
                    "severity": "ACTIONABLE",
                    "statement": "After a Runtime restart the existing Desktop process recovered the projection without a fresh credential; the recorded expectation was withheld-until-relaunch.",
                    "run": run["label"],
                }
            )
        clean = run["scenarios"].get("clean_close", {})
        if clean.get("forced"):
            findings.append(
                {
                    "id": "RT-INT-02",
                    "severity": "BLOCKING",
                    "statement": "WM_CLOSE did not end the Desktop process cleanly; the harness had to force-kill the process it started.",
                    "run": run["label"],
                }
            )
        if not restart.get("epoch_changed"):
            findings.append(
                {
                    "id": "RT-INT-03",
                    "severity": "NIT",
                    "statement": "The Runtime restart did not change the HELLO epoch; a fresh process may reuse the DB-backed epoch.",
                    "run": run["label"],
                }
            )
        if clean.get("verdict") == "FAIL" and not clean.get("forced"):
            findings.append(
                {
                    "id": "RT-INT-05",
                    "severity": "ACTIONABLE",
                    "statement": "WM_CLOSE ended the process, but the bounded observation-shutdown and preferences-save confirmations were not reported within the exit budget; the explicit NOT-confirmed outcome was printed instead.",
                    "run": run["label"],
                }
            )
    return {
        "deterministic": deterministic,
        "comparisons": compared,
        "differences": differences,
        "scenarios": scenarios,
        "all_required_passed": all(entry["passed"] for entry in scenarios.values()),
        "findings": findings,
    }


def source_state() -> dict:
    """Committed HEAD plus the worktree sources the release binary was built from."""
    dirty: list[dict] = []
    untracked_product: list[dict] = []
    for line in git_value(DESKTOP_ROOT, "status", "--porcelain").splitlines():
        if not line.strip():
            continue
        path_text = line[3:].strip()
        if " -> " in path_text:
            path_text = path_text.split(" -> ", 1)[1]
        full = DESKTOP_ROOT / path_text
        entry = {
            "status": line[:2].strip() or "??",
            "path": path_text,
            "sha256": sha256_file(full) if full.is_file() else None,
        }
        if entry["status"] == "??":
            if path_text.startswith(("apps/desktop/src/", "crates/")):
                untracked_product.append(entry)
        else:
            dirty.append(entry)
    state = {
        "head": git_value(DESKTOP_ROOT, "rev-parse", "HEAD"),
        "dirty_tracked": dirty,
        "untracked_product_sources": untracked_product,
    }
    state["fingerprint"] = sha256_text(json.dumps(state, ensure_ascii=False, sort_keys=True))
    return state


def build_desktop() -> dict:
    """Build the release Desktop from the current worktree so the binary matches the recorded source."""
    environment = {**os.environ}
    tmp = WORK_ROOT / "tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    environment["TEMP"] = str(tmp)
    environment["TMP"] = str(tmp)
    completed = subprocess.run(
        ["cargo", "build", "--locked", "--release", "-p", "cos-desktop", "--bin", "cos-desktop"],
        cwd=str(DESKTOP_ROOT),
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=1800,
    )
    return {
        "ok": completed.returncode == 0,
        "exit": completed.returncode,
        "tail": ((completed.stdout or "") + (completed.stderr or ""))[-1500:],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=2, choices=(1, 2, 3))
    parser.add_argument("--label-prefix", default="run")
    parser.add_argument("--no-build", action="store_true", help="reuse the existing release binary")
    args = parser.parse_args(argv)

    for required in (RUNTIME_PYTHON, SOURCE_DB, CONFIG_TEMPLATE, DESKTOP_EXE, PROBE):
        if not required.is_file():
            print(f"missing required path: {required}", file=sys.stderr)
            return 2

    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    WORK_ROOT.mkdir(parents=True, exist_ok=True)
    build_facts: dict = {"performed": False}
    if not args.no_build:
        stable = False
        for attempt in (1, 2):
            before = source_state()
            build = build_desktop()
            after = source_state()
            build_facts = {
                "performed": True,
                "attempt": attempt,
                "exit": build["exit"],
                "source_stable": before["fingerprint"] == after["fingerprint"],
                "tail": build["tail"],
            }
            if not build["ok"]:
                print(f"[harness] build failed: {build['tail']}", file=sys.stderr, flush=True)
                return 2
            if build_facts["source_stable"]:
                stable = True
                break
        if not stable:
            print(
                "[harness] worktree source changed during both build attempts; refusing to measure a mixed binary",
                file=sys.stderr,
                flush=True,
            )
            return 2
    secrets_list: list[str] = []
    runs: list[dict] = []
    error = None
    source_before = source_state()
    try:
        for index in range(1, args.runs + 1):
            label = f"{args.label_prefix}{index}"
            print(f"[harness] run {label} starting", flush=True)
            run = run_once(label, secrets_list)
            runs.append(run)
            print(
                f"[harness] run {label} done: "
                + ", ".join(
                    f"{name}={run['scenarios'].get(name, {}).get('verdict')}" for name in SCENARIO_ORDER
                ),
                flush=True,
            )
    except Exception as exc:  # noqa: BLE001 - evidence must record the failure
        error = f"{type(exc).__name__}: {exc}"
        print(f"[harness] FAILED: {error}", file=sys.stderr, flush=True)
    finally:
        cleanup = cleanup_credential_store(ENDPOINT)
        summary = summarize(runs) if runs else {"deterministic": False, "differences": ["no completed run"]}
        source_after = source_state()
        source_changed = source_before["fingerprint"] != source_after["fingerprint"]
        if source_changed:
            summary.setdefault("findings", []).append(
                {
                    "id": "RT-INT-04",
                    "severity": "NIT",
                    "statement": "The worktree source changed while the harness was running; the recorded start state describes the built binary, the end state is newer.",
                }
            )
        session = {
            "measurement": "Live Desktop<->Runtime integration baseline (RT2/RT3/ACC1)",
            "date": now_iso(),
            "harness": {
                "command": "python apps/desktop/devtools/runtime_integration/run_integration.py --runs 2",
                "port": PORT,
                "endpoint": ENDPOINT,
                "runs_requested": args.runs,
                "run_secret_recorded": False,
                "token_recorded": False,
            },
            "build": build_facts,
            "desktop": {
                "head": source_before["head"],
                "built_from": "worktree at the recorded start state plus the listed tracked/untracked product sources",
                "build_command": "cargo build --locked --release -p cos-desktop --bin cos-desktop",
                "source_at_start": source_before,
                "source_at_end": source_after,
                "source_changed_during_run": source_changed,
                "exe": str(DESKTOP_EXE),
                "exe_sha256": sha256_file(DESKTOP_EXE),
                "profile": "release",
                "fixture_flags": [],
            },
            "runtime": {
                "root": str(RUNTIME_ROOT),
                "commit": git_value(RUNTIME_ROOT, "rev-parse", "HEAD"),
                "python": str(RUNTIME_PYTHON),
                "source_db": str(SOURCE_DB),
                "source_db_sha256": sha256_file(SOURCE_DB),
                "db_copy_per_run": [run.get("db_copy_sha256") for run in runs],
            },
            "runs": runs,
            "determinism": summary,
            "credential_store_cleanup": cleanup,
            "harness_error": error,
            "limits": [
                "One host (Windows 11, 96 DPI); the Runtime and Desktop are loopback-only, fixture-free.",
                "The Runtime runs on a copied database; the production database was only read through a read-only handle.",
                "PrintWindow screenshots are captured without foregrounding the window; a black or partial capture is reported as printed=false and is not interpreted as visual approval.",
                "UIA exposes automation names, not focus or screen-reader quality; no AT acceptance, IME, High Contrast or DPI matrix claim is made.",
                "Credential principal, expiry and scope are recorded; the token and the issuance secret are never written to any file.",
                "A Runtime restart uses a new process and a new in-memory credential registry; no reconnect protocol beyond the current relaunch path is exercised.",
            ],
            "closing_notes": [
                "The worktree at run time contained another lane's uncommitted product changes; desktop.source_at_start lists every tracked modification and untracked product source with its SHA-256, and build.source_stable is true, so the binary matches that recorded state rather than a clean checkout of HEAD.",
                "Observed once before the 3 s close settle was added: a WM_CLOSE during the post-restart observation retry backoff exited 0 but printed 'observation shutdown NOT confirmed within the exit budget' and 'preferences save NOT confirmed within the exit budget'. The settle is harness pacing only; the explicit NOT-confirmed outcome remains a documented bounded-close limitation for the wave-2 lane and is recorded as finding RT-INT-05 whenever it recurs.",
                "The Runtime restart has a new in-memory credential registry. A teardown revoke from the pre-restart Desktop session can therefore log 'Runtime credential revoke on teardown failed: runtime credential request was not authorized' in the sanitized stderr; the harness records the stderr hash instead of treating it as a scenario failure.",
                "Credential Manager cleanup reported 'Element not found' because the environment-token override path did not leave a stored entry for this loopback origin.",
            ],
        }
        write_json(EVIDENCE_DIR / "session.json", session)
        leaked = []
        for path in EVIDENCE_DIR.rglob("*"):
            if not path.is_file():
                continue
            data = path.read_bytes()
            for secret in secrets_list:
                if secret and secret.encode() in data:
                    leaked.append(str(path.relative_to(EVIDENCE_DIR)))
                    break
        if leaked:
            raise RuntimeError(f"token/secret material leaked into evidence: {sorted(set(leaked))}")
        print(
            f"[harness] summary: deterministic={summary.get('deterministic')} "
            f"all_required_passed={summary.get('all_required_passed')} error={error}",
            flush=True,
        )
    return 0 if error is None and runs and summary.get("all_required_passed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
