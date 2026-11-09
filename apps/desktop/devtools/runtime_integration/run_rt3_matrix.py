#!/usr/bin/env python3
"""Live RT3 reconnect/resync/epoch matrix against an isolated Runtime.

This harness extends the two-run RT3/RT8 campaign (``run_rt3_rt8.py``) to the
scenario matrix DC-MP-RT3 requires. It is test-only: it never changes product
code, starts one isolated Runtime per lane port (8135 primary, 8136 alternate)
from an immutable database copy, and drives the release Desktop binary through:

1. ``gap_closed_resync`` - a lost journal segment (a real TASK status change
   whose history is purged atomically) forces ``409 RESYNC_REQUIRED`` and the
   client performs one bounded closed resync (fresh snapshot, new cursor) with
   the missed projection recovered and no retry loop.
2. ``cursor_overflow_resync`` - a journal advanced past the retention bound
   (out-of-range cursor) forces the bounded overflow resync; the Runtime prunes
   and the client recovers through the fresh snapshot.
3. ``desktop_clean_restart`` - clean Desktop close mid-observation, relaunch;
   fresh observation and a cursor that was never used before.
4. ``desktop_forced_restart`` - forced close (process tree) mid-observation,
   relaunch; fresh observation and no reused cursor.
5. ``runtime_epoch_change_desktop_restart`` - the Runtime is stopped and
   restarted on the alternate port while the Desktop is closed; the relaunched
   Desktop rediscovers the new origin/epoch and opens a fresh observation.
6. ``epoch_change_under_load`` - a live projection stream (TASK status change)
   is applied, the Runtime is restarted under the same Desktop process, the
   withheld state is observed and the same process recovers through bounded
   reauthorization and a fresh observation (no relaunch).
7. ``long_observation_idle`` - the live session is held for a bounded idle
   window (default 180 s); every keepalive delta poll is counted, and no false
   disconnect, no resync, no reauthorization and no duplicate task row appear.
8. ``auth_change_bounded_reauth`` - the Runtime credential registry is pushed
   past its bound three times (64 issuances each, the supported issuance API);
   each push revokes the Desktop credential and the client refreshes/reissues
   exactly once, retries the read, fails closed on the new principal's old
   context, and reopens a fresh observation with a new cursor.
9. ``auth_change_exclusion_honest`` - a fourth push exhausts the per-session
   replacement budget: the client returns the 401, sends no further credential
   request, and renders the explicit unauthorized/unavailable state. The
   reconnect strip action is invoked and must not fabricate a recovery.
10. ``post_exclusion_relaunch_recovery`` - a fresh Desktop session (relaunch)
    reauthorizes from the operator issuance secret and recovers the projection.

Every scenario records exact statuses, request counts, epochs and cursors in
``session.json``. Tokens and the issuance secret are passed through the child
environment only and never written; the final leak scan fails the run if any
collected secret appears in the evidence directory.

Usage (from the repository root):

    python apps/desktop/devtools/runtime_integration/run_rt3_matrix.py
"""

from __future__ import annotations

import argparse
import contextlib
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
import threading
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_integration as base  # noqa: E402 - local harness helpers, stdlib only

DESKTOP_ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
RUNTIME_PYTHON = base.RUNTIME_PYTHON
SOURCE_DB = base.SOURCE_DB
CONFIG_TEMPLATE = base.CONFIG_TEMPLATE
DESKTOP_EXE = base.DESKTOP_EXE
PROBE = base.PROBE
HERE = Path(__file__).resolve().parent

EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "rt3-matrix-latest"
)
WORK_ROOT = DESKTOP_ROOT / "target" / "rt3-matrix"
RUN_LABEL = "run1"

PORT = 8135
ENDPOINT = f"http://127.0.0.1:{PORT}"
ALT_PORT = 8136
ALT_ENDPOINT = f"http://127.0.0.1:{ALT_PORT}"

# Desktop client bounds this campaign must respect (fixed source values).
CLIENT_BOUNDS = {
    "max_reauth_attempts": 3,
    "reauth_min_interval_seconds": 2.0,
    "max_resync_attempts": 5,
    "delta_poll_millis": 250,
    "credential_registry_max": 64,
}

LIVE_LABEL = "情報を受信中"
WITHDRAWN_LABELS = (
    "情報源なし / データ非公開",
    "データは非公開",
    "再接続中",
)
EXCLUDED_LABELS = ("データは非公開 — 現在の許可がありません", "情報源なし / データ非公開")
RECONNECT_LABEL = "再接続する"

STATUS_LABELS = base.STATUS_LABELS
WITHDRAWN_MARKERS = base.WITHDRAWN_MARKERS
CLOSE_MARKERS = base.CLOSE_MARKERS

SCENARIO_ORDER = (
    "gap_closed_resync",
    "cursor_overflow_resync",
    "desktop_clean_restart",
    "desktop_forced_restart",
    "runtime_epoch_change_desktop_restart",
    "epoch_change_under_load",
    "long_observation_idle",
    "auth_change_bounded_reauth",
    "auth_change_exclusion_honest",
    "post_exclusion_relaunch_recovery",
)

ACCESS_LINE = re.compile(
    r'"(?P<method>GET|POST) (?P<path>[^"]+) HTTP/1\.1" (?P<status>\d{3})'
)
CURSOR = re.compile(r"[?&]cursor=(?P<cursor>[A-Za-z0-9_-]+)")
HEX_ID = re.compile(r"^[0-9a-f]{8,}$")


# --------------------------------------------------------------------------- basics


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def short_hash(value: str) -> str:
    """Trace identity for one opaque capability (never the capability itself)."""
    text = base.sha256_text(value)
    return "sha256:" + text.removeprefix("sha256:")[:16]


def http_call(
    method: str,
    url: str,
    *,
    body: object | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = 30.0,
) -> dict:
    """One HTTP call; status/document are returned even for error statuses."""
    data = None
    request_headers = {"Accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        request_headers["Content-Type"] = "application/json"
    if headers:
        request_headers.update(headers)
    request = urllib.request.Request(url, data=data, headers=request_headers, method=method)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=timeout) as response:
            raw = response.read()
            status = int(response.status)
    except urllib.error.HTTPError as error:
        raw = error.read()
        status = int(error.code)
    except (urllib.error.URLError, OSError) as error:
        return {"status": None, "document": None, "bytes": 0, "error": type(error).__name__}
    document = None
    with contextlib.suppress(ValueError):
        document = json.loads(raw.decode("utf-8"))
    return {"status": status, "document": document, "bytes": len(raw)}


CREDENTIAL_USER = "desktop-observation"
CREDENTIAL_SERVICE_PREFIX = "cos-desktop/observation"


def credential_targets(origin: str) -> list[str]:
    """Exact Windows Credential Manager target names for one Desktop origin.

    The keyring crate composes ``{user}.{service}`` and the Desktop service is
    ``cos-desktop/observation/<sha256(origin)[:32]>``. ``cmdkey /delete`` always
    needs the composed target, not only the service string; deleting the bare
    service leaves the entry behind and a later launch picks up a stale
    credential (observed as an avoidable startup 401 in this campaign).
    """
    head = hashlib.sha256(origin.encode("utf-8")).hexdigest()[:32]
    return [
        f"{CREDENTIAL_USER}.{CREDENTIAL_SERVICE_PREFIX}/{head}",
        f"{CREDENTIAL_SERVICE_PREFIX}/{head}",
    ]


def cleanup_credential_store(origin: str) -> dict:
    targets: list[dict] = []
    deleted = False
    for target in credential_targets(origin):
        try:
            completed = subprocess.run(
                ["cmdkey", f"/delete:{target}"],
                capture_output=True,
                text=True,
                timeout=20,
            )
            output = (completed.stdout + completed.stderr).strip()
            targets.append(
                {"target": target, "exit": completed.returncode, "output": output[:200]}
            )
            deleted = deleted or completed.returncode == 0
        except (OSError, subprocess.SubprocessError) as error:
            targets.append({"target": target, "error": type(error).__name__})
    return {"origin": origin, "targets": targets, "deleted": deleted}


def port_free(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.3)
        return probe.connect_ex(("127.0.0.1", port)) != 0


def probe_window(action: str, pid: int, out_path: Path, name_match: str | None = None) -> dict:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    if out_path.exists():
        out_path.unlink()
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
    if name_match is not None:
        command.extend(["-NameMatch", name_match])
    completed = subprocess.run(
        command, capture_output=True, text=True, timeout=90, encoding="utf-8", errors="replace"
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


def wait_until(predicate, timeout: float, interval: float = 0.25) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return False


# --------------------------------------------------------------------------- access log


class AccessLog:
    """Timestamped tail of one uvicorn access log (arrival time, ±50 ms)."""

    def __init__(self, path: Path):
        self.path = path
        self.entries: list[dict] = []
        self.lock = threading.Lock()
        self.stop_event = threading.Event()
        self.offset = 0
        self.pending = b""
        self.thread = threading.Thread(target=self._run, name="rt3-access-log", daemon=True)
        self.thread.start()

    def _run(self) -> None:
        while not self.stop_event.is_set():
            try:
                data = self.path.read_bytes()
            except OSError:
                data = b""
            if len(data) > self.offset:
                chunk = data[self.offset :]
                self.offset = len(data)
                self.pending += chunk
                lines = self.pending.split(b"\n")
                self.pending = lines.pop()
                stamp = now_iso()
                monotonic = time.monotonic()
                with self.lock:
                    for raw in lines:
                        text = raw.decode("utf-8", errors="replace")
                        match = ACCESS_LINE.search(text)
                        if not match:
                            continue
                        path = match.group("path")
                        cursor = CURSOR.search(path)
                        self.entries.append(
                            {
                                "at": stamp,
                                "monotonic": monotonic,
                                "method": match.group("method"),
                                "path": path,
                                "status": int(match.group("status")),
                                "cursor": cursor.group("cursor") if cursor else None,
                            }
                        )
            time.sleep(0.05)

    def stop(self) -> None:
        self.stop_event.set()
        self.thread.join(timeout=2)

    def snapshot(self) -> list[dict]:
        with self.lock:
            return list(self.entries)

    def mark(self) -> int:
        with self.lock:
            return len(self.entries)

    def since(self, mark: int) -> list[dict]:
        with self.lock:
            return list(self.entries[mark:])

    def wait_for(self, predicate, timeout: float, start: int = 0) -> dict | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for entry in self.since(start):
                if predicate(entry):
                    return entry
            time.sleep(0.05)
        return None

    def wait_for_all(self, predicate, timeout: float, start: int = 0) -> dict | None:
        """Wait for the next new entry matching ``predicate`` (same start mark)."""
        return self.wait_for(predicate, timeout, start)


def match(
    entry: dict,
    *,
    method: str | None = None,
    prefix: str | None = None,
    status: int | None = None,
) -> bool:
    if method is not None and entry["method"] != method:
        return False
    if prefix is not None and not entry["path"].startswith(prefix):
        return False
    if status is not None and entry["status"] != status:
        return False
    return True


def entries_of(
    entries: list[dict],
    *,
    method: str | None = None,
    prefix: str | None = None,
    status: int | None = None,
) -> list[dict]:
    return [
        entry
        for entry in entries
        if match(entry, method=method, prefix=prefix, status=status)
    ]


def index_of(entries: list[dict], target: dict) -> int:
    for index, entry in enumerate(entries):
        if (
            entry["monotonic"] == target["monotonic"]
            and entry["method"] == target["method"]
            and entry["path"] == target["path"]
            and entry["status"] == target["status"]
        ):
            return index
    return -1


def exact_path(entry: dict) -> str:
    return entry["path"].split("?", 1)[0]


def cursors_of(entries: list[dict], prefix: str = "/api/cos/desktop/deltas") -> list[str]:
    seen: list[str] = []
    for entry in entries_of(entries, prefix=prefix):
        cursor = entry.get("cursor")
        if cursor and cursor not in seen:
            seen.append(cursor)
    return seen


def max_gap_seconds(entries: list[dict]) -> float:
    if len(entries) < 2:
        return 0.0
    return max(
        later["monotonic"] - earlier["monotonic"]
        for earlier, later in zip(entries, entries[1:])
    )


# --------------------------------------------------------------------------- Runtime


class RuntimeProcess:
    """One isolated Runtime launcher on a harness-owned port."""

    def __init__(
        self,
        *,
        work: Path,
        db_copy: Path,
        secret: str,
        port: int,
        discovery_path: Path,
        label: str,
    ):
        self.work = work
        self.db_copy = db_copy
        self.secret = secret
        self.port = port
        self.endpoint = f"http://127.0.0.1:{port}"
        self.discovery_path = discovery_path
        self.label = label
        self.process: subprocess.Popen | None = None
        self.log_path = work / f"runtime-{port}.log"
        self.log_handle = None
        self.config_path = work / f"runtime-{port}-config.yaml"
        self.stop_file = work / f"runtime-{port}.stop"
        self.ready_file = work / f"runtime-{port}.ready"
        self.starts = 0
        self.access = AccessLog(self.log_path)

    def _write_config(self) -> None:
        template = CONFIG_TEMPLATE.read_text(encoding="utf-8")
        replacements = (
            ("db_path", self.db_copy.as_posix()),
            ("log_dir", (self.work / f"logs-{self.port}").as_posix()),
            ("output_root", (self.work / f"outputs-{self.port}").as_posix()),
        )
        rendered = template
        for key, value in replacements:
            rendered, count = re.subn(
                rf"(?m)^(\s*){key}:.*$",
                lambda match, key=key, value=value: f"{match.group(1)}{key}: {value}",
                rendered,
                count=1,
            )
            if count != 1:
                raise RuntimeError(f"default.yaml no longer contains exactly one {key} line")
        rendered = rendered.replace(
            "project_root: C:/AI/orchestrator", f"project_root: {RUNTIME_ROOT.as_posix()}"
        )
        self.config_path.write_text(rendered, encoding="utf-8")

    def start(self, timeout: float = 120.0) -> dict:
        if not port_free(self.port):
            raise RuntimeError(f"port {self.port} is already in use; the harness owns it")
        self.starts += 1
        for directory in (self.work / f"logs-{self.port}", self.work / f"outputs-{self.port}"):
            directory.mkdir(parents=True, exist_ok=True)
        self._write_config()
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("COS_DESKTOP_")
            and not key.startswith("COS_RUNTIME_")
            and key != "ORCHESTRATOR_AUTH_DISABLED"
        }
        environment.update(
            {
                "ORCHESTRATOR_AUTH_SECRET": self.secret,
                "ORCHESTRATOR_RUNTIME_ROOT": str(RUNTIME_ROOT),
                # RT8/RC-16: never publish into the real per-user location.
                "COS_RUNTIME_DISCOVERY_PATH": str(self.discovery_path),
                "COS_RUNTIME_ORIGIN": self.endpoint,
            }
        )
        for stale in (self.stop_file, self.ready_file):
            with contextlib.suppress(FileNotFoundError):
                stale.unlink()
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- start {self.starts} {now_iso()} ---\n".encode())
        self.log_handle.flush()
        self.process = subprocess.Popen(  # noqa: S603 - fixed argv, loopback only
            [
                str(RUNTIME_PYTHON),
                str(HERE / "isolated_runtime.py"),
                "--config",
                str(self.config_path),
                "--host",
                "127.0.0.1",
                "--port",
                str(self.port),
                "--stop-file",
                str(self.stop_file),
                "--ready-file",
                str(self.ready_file),
            ],
            cwd=str(RUNTIME_ROOT),
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        started = time.monotonic()
        while True:
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"Runtime {self.port} exited during startup (exit {self.process.returncode})"
                )
            ready = http_call("GET", f"{self.endpoint}/api/ready", timeout=2.0)
            if ready["status"] == 200:
                break
            if time.monotonic() - started > timeout:
                raise RuntimeError(f"Runtime {self.port} readiness timed out")
            time.sleep(0.25)
        hello = self.server()
        if hello["status"] != 200 or not isinstance(hello["document"], dict):
            raise RuntimeError(f"HELLO failed ({hello['status']})")
        server = hello["document"]["server"]
        return {
            "pid": self.process.pid,
            "port": self.port,
            "origin": self.endpoint,
            "start_index": self.starts,
            "started_at": now_iso(),
            "epoch": server["epoch"],
            "server_identity": server["server_identity"],
            "server_build": server["server_build"],
            "protocol": server["protocol"],
        }

    def server(self) -> dict:
        return http_call("GET", f"{self.endpoint}/api/cos/desktop/server")

    def epoch(self) -> str | None:
        response = self.server()
        if response["status"] != 200 or not isinstance(response["document"], dict):
            return None
        return (response["document"].get("server") or {}).get("epoch")

    def issue_credential(self) -> dict:
        return http_call(
            "POST",
            f"{self.endpoint}/api/cos/desktop/credentials",
            headers={"X-Auth-Secret": self.secret},
        )

    def refresh_credential(self, token: str) -> dict:
        return http_call(
            "POST",
            f"{self.endpoint}/api/cos/desktop/credentials/refresh",
            headers={"Authorization": f"Bearer {token}"},
        )

    def revoke_credential(self, token: str) -> dict:
        return http_call(
            "POST",
            f"{self.endpoint}/api/cos/desktop/credentials/revoke",
            headers={"Authorization": f"Bearer {token}"},
        )

    def open_observation(self, token: str) -> dict:
        body = {
            "schema_version": 1,
            "request_id": uuid.uuid4().hex,
            "protocol": {"major": 2, "minor": 0},
            "schema": {"major": 2, "minor": 0},
            "scope": {"task_ids": [], "filters": [{"name": "view", "value": "all"}]},
            "retained_commands": [],
        }
        return http_call(
            "POST",
            f"{self.endpoint}/api/cos/desktop/observation",
            body=body,
            headers={"Authorization": f"Bearer {token}"},
        )

    def stop(self) -> dict:
        if self.process is None:
            return {"stopped": False, "reason": "not_started"}
        pid = self.process.pid
        clean = True
        if self.process.poll() is None:
            self.stop_file.write_text("stop", encoding="utf-8")
            try:
                self.process.wait(timeout=90)
            except subprocess.TimeoutExpired:
                clean = False
                forced = base.kill_tree(pid)
                with contextlib.suppress(subprocess.TimeoutExpired):
                    self.process.wait(timeout=20)
                if self.log_handle is not None:
                    self.log_handle.close()
                    self.log_handle = None
                return {
                    "pid": pid,
                    "clean": False,
                    "exit": self.process.returncode,
                    "forced": forced,
                    "stopped_at": now_iso(),
                }
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {
            "pid": pid,
            "clean": clean,
            "exit": self.process.returncode,
            "stopped_at": now_iso(),
        }

    def kill(self) -> dict:
        if self.process is None:
            return {"killed": False}
        pid = self.process.pid
        forced = base.kill_tree(pid) if base.pid_alive(pid) else None
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {"pid": pid, "killed": True, "forced": forced}

    def wait_descriptor(self, timeout: float = 20.0) -> dict:
        deadline = time.monotonic() + timeout
        last: dict = {"present": False}
        while time.monotonic() < deadline:
            if self.discovery_path.is_file():
                raw = self.discovery_path.read_bytes()
                try:
                    document = json.loads(raw.decode("utf-8"))
                except ValueError:
                    document = None
                last = {
                    "present": True,
                    "bytes": len(raw),
                    "sha256": base.sha256_file(self.discovery_path),
                    "origin": (document or {}).get("origin"),
                    "epoch": (document or {}).get("epoch"),
                    "schema_version": (document or {}).get("schema_version"),
                    "ready": (document or {}).get("ready"),
                }
                if last["schema_version"] == 1 and last["origin"] == self.endpoint and last["ready"]:
                    return last
            time.sleep(0.1)
        raise RuntimeError(f"descriptor did not appear for {self.endpoint}: {last}")

    def wait_descriptor_gone(self, timeout: float = 20.0) -> bool:
        return wait_until(lambda: not self.discovery_path.exists(), timeout, 0.1)

    def close(self) -> None:
        self.access.stop()


# --------------------------------------------------------------------------- Desktop


class DesktopProcess:
    """One Desktop binary instance using discovery only (no endpoint/token)."""

    def __init__(
        self,
        *,
        name: str,
        work: Path,
        localappdata: Path,
        secret: str,
        discovery_path: Path,
        evidence: Path,
        secrets_list: list[str],
    ):
        self.name = name
        self.work = work
        self.localappdata = localappdata
        self.secret = secret
        self.discovery_path = discovery_path
        self.evidence = evidence
        self.secrets_list = secrets_list
        self.process: subprocess.Popen | None = None
        self.log_path = work / f"desktop-{name}.log"
        self.log_handle = None
        self.environment: dict[str, str] = {}
        self.stderr_offset = 0

    def launch(self) -> dict:
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("COS_DESKTOP_")
            and not key.startswith("COS_RUNTIME_")
            and key not in {"COS_FIXTURE_TRACE", "COS_PREVIEW_TRACE", "COS_EXPORT_TRACE"}
        }
        environment["LOCALAPPDATA"] = str(self.localappdata)
        # Deliberately no COS_DESKTOP_RUNTIME_ENDPOINT / TOKEN: discovery and the
        # issuance secret are the only connection inputs.
        environment["COS_DESKTOP_RUNTIME_ISSUANCE_SECRET"] = self.secret
        environment["COS_DESKTOP_RUNTIME_DISCOVERY_PATH"] = str(self.discovery_path)
        self.environment = environment
        self.localappdata.mkdir(parents=True, exist_ok=True)
        self.log_handle = self.log_path.open("wb")
        self.process = subprocess.Popen(  # noqa: S603 - fixed binary, no fixture flag
            [str(DESKTOP_EXE)],
            cwd=str(DESKTOP_ROOT),
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        return {
            "pid": self.process.pid,
            "launched_at": now_iso(),
            "endpoint_env_absent": "COS_DESKTOP_RUNTIME_ENDPOINT" not in environment,
            "token_env_absent": "COS_DESKTOP_RUNTIME_TOKEN" not in environment,
            "issuance_secret_present": "COS_DESKTOP_RUNTIME_ISSUANCE_SECRET" in environment,
            "discovery_override_present": "COS_DESKTOP_RUNTIME_DISCOVERY_PATH" in environment,
        }

    def alive(self) -> bool:
        return self.process is not None and self.process.poll() is None

    def dump(self, tag: str) -> tuple[dict, str]:
        out = self.evidence / f"uia-{self.name}-{tag}.json"
        result = probe_window("dump", self.pid, out)
        return result, uia_text(result)

    @property
    def pid(self) -> int:
        if self.process is None:
            raise RuntimeError("desktop not started")
        return self.process.pid

    def wait_text(self, tag: str, predicate, timeout: float, interval: float = 0.6):
        out = self.evidence / f"uia-{self.name}-{tag}.json"
        deadline = time.monotonic() + timeout
        last: dict = {"found": False}
        last_text = ""
        while time.monotonic() < deadline:
            last = probe_window("dump", self.pid, out)
            last_text = uia_text(last)
            if last.get("found") and predicate(last_text):
                return last, last_text, True
            if not self.alive():
                break
            time.sleep(interval)
        return last, last_text, False

    def wait_live(self, title: str, timeout: float = 90.0) -> dict:
        started = time.monotonic()
        dump, text, ok = self.wait_text(
            "live",
            lambda joined: title in joined and LIVE_LABEL in joined,
            timeout,
        )
        return {
            "ok": ok,
            "seconds": round(time.monotonic() - started, 3),
            "title_visible": title in text,
            "live_label": LIVE_LABEL in text,
            "uia_element_count": dump.get("elements"),
            "excerpt": base.bounded(
                "\n".join(line for line in text.splitlines() if title in line or LIVE_LABEL in line),
                1200,
            ),
        }

    def wait_status_label(self, title: str, label: str, timeout: float = 60.0) -> dict:
        started = time.monotonic()
        dump, text, ok = self.wait_text(
            "status",
            lambda joined: title in joined and label in joined and LIVE_LABEL in joined,
            timeout,
        )
        return {
            "ok": ok,
            "seconds": round(time.monotonic() - started, 3),
            "label": label,
            "label_visible": label in text,
            "title_visible": title in text,
            "live_label": LIVE_LABEL in text,
            "uia_element_count": dump.get("elements"),
            "excerpt": base.bounded(
                "\n".join(line for line in text.splitlines() if label in line or title in line),
                1200,
            ),
        }

    def wait_withheld(self, title: str, timeout: float = 60.0) -> dict:
        started = time.monotonic()
        dump, text, ok = self.wait_text(
            "withheld",
            lambda joined: title not in joined
            and any(label in joined for label in WITHDRAWN_LABELS),
            timeout,
        )
        return {
            "ok": ok,
            "seconds": round(time.monotonic() - started, 3),
            "title_absent": title not in text,
            "labels_present": [
                label for label in WITHDRAWN_LABELS if label in text
            ],
            "uia_element_count": dump.get("elements"),
            "excerpt": base.bounded(
                "\n".join(
                    line
                    for line in text.splitlines()
                    if any(label in line for label in WITHDRAWN_LABELS)
                ),
                1000,
            ),
        }

    def wait_excluded(self, title: str, timeout: float = 45.0) -> dict:
        started = time.monotonic()
        dump, text, ok = self.wait_text(
            "excluded",
            lambda joined: title not in joined
            and all(label in joined for label in ("情報源なし / データ非公開", "データは非公開 — 現在の許可がありません")),
            timeout,
        )
        return {
            "ok": ok,
            "seconds": round(time.monotonic() - started, 3),
            "title_absent": title not in text,
            "mode_label": "情報源なし / データ非公開" in text,
            "explicit_label": "データは非公開 — 現在の許可がありません" in text,
            "reconnect_action_visible": RECONNECT_LABEL in text,
            "uia_element_count": dump.get("elements"),
            "excerpt": base.bounded(
                "\n".join(
                    line
                    for line in text.splitlines()
                    if any(
                        fragment in line
                        for fragment in (
                            "情報源なし",
                            "データは非公開",
                            "認証",
                            "再接続",
                            "タスク情報",
                        )
                    )
                ),
                1200,
            ),
        }

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

    def close(self, timeout: float = 30.0) -> dict:
        if self.process is None:
            return {"closed": False, "reason": "not_started"}
        pid = self.process.pid
        sent = False
        if base.pid_alive(pid):
            with contextlib.suppress(RuntimeError):
                result = probe_window("close", pid, self.work / f"close-{self.name}.json")
                sent = bool(result.get("sent"))
        outcome = self.wait_exit(timeout)
        forced = None
        if not outcome["exited"]:
            forced = base.kill_tree(pid)
            outcome["exited"] = not base.pid_alive(pid)
        log = self.stderr_text()
        outcome.update(
            {
                "wm_close_sent": sent,
                "close_markers": [marker for marker in CLOSE_MARKERS if marker in log],
                "forced": forced,
            }
        )
        return outcome

    def force_stop(self) -> dict:
        if self.process is None:
            return {"forced": False, "reason": "not_started"}
        pid = self.process.pid
        forced = base.kill_tree(pid) if base.pid_alive(pid) else None
        outcome = self.wait_exit(10)
        outcome.update({"forced": forced, "alive_after": base.pid_alive(pid)})
        return outcome

    # ---------------------------------------------------------------- stderr

    def stderr_text(self) -> str:
        with contextlib.suppress(OSError):
            return self.log_path.read_text(encoding="utf-8", errors="replace")
        return ""

    def write_stderr_extract(self, tag: str) -> dict:
        text = self.stderr_text()
        size = len(text.encode("utf-8", errors="replace"))
        extract = text[self.stderr_offset :]
        self.stderr_offset = size
        redacted = base.redact(extract, self.secrets_list)
        path = self.evidence / f"app-{self.name}-{tag}.stderr.sanitized.txt"
        path.write_text(redacted, encoding="utf-8")
        return {
            "file": path.name,
            "bytes": len(redacted.encode("utf-8")),
            "lines": redacted.count("\n") + (1 if redacted and not redacted.endswith("\n") else 0),
            "sha256": base.sha256_text(redacted),
        }

    def invoke(self, name_match: str, tag: str) -> dict:
        out = self.evidence / f"uia-{self.name}-invoke-{tag}.json"
        result = probe_window("invoke", self.pid, out, name_match=name_match)
        return {
            "invoked": bool(result.get("invoked")),
            "matched_name": result.get("matched_name"),
            "button_count": result.get("button_count"),
            "error": result.get("error"),
            "file": out.name,
        }


# --------------------------------------------------------------------------- database


class JournalDB:
    """Test-only hooks against the *isolated copy* (never the production DB)."""

    def __init__(self, path: Path):
        self.path = path

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(str(self.path), timeout=60)
        connection.execute("PRAGMA busy_timeout=30000")
        return connection

    def head(self) -> int:
        connection = self._connect()
        try:
            row = connection.execute(
                "SELECT seq FROM sqlite_sequence WHERE name = 'cos_desktop_projection_journal'"
            ).fetchone()
        finally:
            connection.close()
        if row is None or row[0] is None:
            return 0
        return int(row[0])

    def journal_rows(self) -> int:
        connection = self._connect()
        try:
            return int(
                connection.execute(
                    "SELECT COUNT(*) FROM cos_desktop_projection_journal"
                ).fetchone()[0]
            )
        finally:
            connection.close()

    def task_status(self, task_ref: str) -> str | None:
        connection = self._connect()
        try:
            row = connection.execute(
                "SELECT status FROM creative_tasks WHERE task_id = ?", (task_ref,)
            ).fetchone()
        finally:
            connection.close()
        return None if row is None else str(row[0])

    def set_task_status(self, task_ref: str, status: str) -> dict:
        connection = self._connect()
        try:
            before = self.head()
            connection.execute("BEGIN IMMEDIATE")
            cursor = connection.execute(
                "UPDATE creative_tasks SET status = ? WHERE task_id = ?", (status, task_ref)
            )
            connection.commit()
            after = self.head()
        finally:
            connection.close()
        return {
            "updated_rows": cursor.rowcount,
            "head_before": before,
            "head_after": after,
            "journal_rows": self.journal_rows(),
        }

    def inject_gap(self, task_ref: str, status: str, cut_head: int) -> dict:
        """One atomic lost-history segment: real status change, then purge.

        The trigger inserts the real journal row, the sequence head is advanced
        past it and the row is deleted inside the same transaction, so a reader
        either sees the old state or a gap - never the representable delta.
        """
        connection = self._connect()
        try:
            connection.execute("BEGIN IMMEDIATE")
            updated = connection.execute(
                "UPDATE creative_tasks SET status = ? WHERE task_id = ?", (status, task_ref)
            ).rowcount
            target = cut_head + 2
            changed = connection.execute(
                "UPDATE sqlite_sequence SET seq = ?"
                " WHERE name = 'cos_desktop_projection_journal'",
                (target,),
            ).rowcount
            if changed == 0:
                connection.execute(
                    "INSERT INTO sqlite_sequence(name, seq) VALUES"
                    " ('cos_desktop_projection_journal', ?)",
                    (target,),
                )
            deleted = connection.execute(
                "DELETE FROM cos_desktop_projection_journal WHERE seq > ?", (cut_head,)
            ).rowcount
            connection.commit()
        finally:
            connection.close()
        return {
            "updated_rows": updated,
            "head_before": cut_head,
            "head_after": self.head(),
            "expected_head_after": cut_head + 2,
            "journal_rows_after": self.journal_rows(),
            "deleted_rows": deleted,
        }

    def inject_overflow(self, task_ref: str, rows: int) -> dict:
        connection = self._connect()
        try:
            before = self.head()
            connection.execute("BEGIN IMMEDIATE")
            connection.executemany(
                "INSERT INTO cos_desktop_projection_journal"
                " (task_id, projection_kind, entity_id, operation, recorded_at)"
                " VALUES (?, 'TASK', ?, 'UPDATE', datetime('now'))",
                [(task_ref, task_ref) for _ in range(rows)],
            )
            connection.commit()
            after = self.head()
            count = self.journal_rows()
        finally:
            connection.close()
        return {
            "rows_inserted": rows,
            "head_before": before,
            "head_after": after,
            "journal_rows_after": count,
        }


# --------------------------------------------------------------------------- campaign


class Campaign:
    def __init__(self, *, long_seconds: int, keep_work: bool):
        self.long_seconds = long_seconds
        self.keep_work = keep_work
        self.started = now_iso()
        self.work = WORK_ROOT / RUN_LABEL
        self.evidence = EVIDENCE_DIR / RUN_LABEL
        self.localappdata = self.work / "localappdata"
        self.descriptor_path = self.work / "runtime-discovery.json"
        self.secrets_list: list[str] = []
        self.harness_cursors: list[str] = []
        self.desktops: list[DesktopProcess] = []
        self.current_app: DesktopProcess | None = None
        self.cursor_sets: dict[str, list[str]] = {}
        self.scenarios: dict[str, dict] = {}
        self.facts: dict = {
            "label": RUN_LABEL,
            "started_at": self.started,
            "scenarios": self.scenarios,
            "files": {},
            "findings": [],
        }
        self.main: RuntimeProcess | None = None
        self.alt: RuntimeProcess | None = None
        self.db_copy: Path | None = None
        self.journal: JournalDB | None = None
        self.target: dict | None = None
        self.expected_title = ""
        self.status_label = ""
        self.excluded_label = ""

    # ---------------------------------------------------------------- setup

    def prepare(self) -> None:
        if self.work.exists():
            shutil.rmtree(self.work, ignore_errors=True)
        if self.evidence.exists():
            shutil.rmtree(self.evidence, ignore_errors=True)
        self.work.mkdir(parents=True, exist_ok=True)
        self.evidence.mkdir(parents=True, exist_ok=True)
        self.localappdata.mkdir(parents=True, exist_ok=True)
        run_secret = secrets.token_hex(24)
        self.secrets_list.append(run_secret)
        self.facts["run_secret_recorded"] = False
        db_copy = self.work / "generation-copy.sqlite"
        base.copy_database(SOURCE_DB, db_copy)
        self.db_copy = db_copy
        self.journal = JournalDB(db_copy)
        self.facts["db_copy"] = {
            "source": str(SOURCE_DB),
            "source_sha256": base.sha256_file(SOURCE_DB),
            "copy": str(db_copy),
            "copy_sha256": base.sha256_file(db_copy),
            "copy_bytes": db_copy.stat().st_size,
        }
        self.main = RuntimeProcess(
            work=self.work,
            db_copy=db_copy,
            secret=run_secret,
            port=PORT,
            discovery_path=self.descriptor_path,
            label="main",
        )
        self.alt = RuntimeProcess(
            work=self.work,
            db_copy=db_copy,
            secret=run_secret,
            port=ALT_PORT,
            discovery_path=self.descriptor_path,
            label="alt",
        )
        self.facts["harness"] = {
            "command": (
                "python apps/desktop/devtools/runtime_integration/run_rt3_matrix.py"
                f" --long-seconds {self.long_seconds}"
            ),
            "ports": [PORT, ALT_PORT],
            "endpoint_passed_to_desktop": False,
            "token_passed_to_desktop": False,
            "discovery_redirect_env": "COS_DESKTOP_RUNTIME_DISCOVERY_PATH",
            "run_secret_recorded": False,
            "client_bounds": CLIENT_BOUNDS,
            "long_observation_seconds": self.long_seconds,
        }
        self.facts["desktop"] = {
            "head": base.git_value(DESKTOP_ROOT, "rev-parse", "HEAD"),
            "head_tree": base.git_value(DESKTOP_ROOT, "rev-parse", "HEAD^{tree}"),
            "binary": str(DESKTOP_EXE),
            "binary_sha256": base.sha256_file(DESKTOP_EXE),
            "binary_bytes": DESKTOP_EXE.stat().st_size,
            "binary_built_at": time.strftime(
                "%Y-%m-%dT%H:%M:%S%z", time.localtime(DESKTOP_EXE.stat().st_mtime)
            ),
            "last_product_commit": base.git_value(
                DESKTOP_ROOT, "log", "-1", "--format=%h %ct %ci", "--", "apps", "crates"
            ),
            "source_state": base.source_state(),
            "source_spot_checks": base.source_spot_checks(),
        }
        try:
            runtime_status = subprocess.run(
                ["git", "-C", str(RUNTIME_ROOT), "status", "--porcelain=v1"],
                capture_output=True,
                text=True,
                timeout=20,
            ).stdout
        except (OSError, subprocess.SubprocessError):
            runtime_status = ""
        self.facts["runtime"] = {
            "root": str(RUNTIME_ROOT),
            "head": base.git_value(RUNTIME_ROOT, "rev-parse", "HEAD"),
            "dirty_paths": [
                line[3:]
                for line in runtime_status.splitlines()
                if len(line) > 3 and line[2] == " "
            ],
            "python": str(RUNTIME_PYTHON),
            "python_version": sys.version.split()[0],
        }

    def start_main(self) -> dict:
        assert self.main is not None
        facts = self.main.start()
        descriptor = self.main.wait_descriptor(20.0)
        self.facts["main_server"] = {**facts, "descriptor": descriptor}
        self.observe_expected()
        return facts

    def observe_expected(self) -> None:
        """Derive the expected projection from the harness's own observation."""
        assert self.main is not None and self.alt is not None
        server = self.main if self.main.starts > 0 else self.alt
        credential = server.issue_credential()
        if credential["status"] != 200:
            raise RuntimeError(f"harness credential issuance failed ({credential['status']})")
        token = credential["document"]["token"]
        self.secrets_list.append(token)
        observation = server.open_observation(token)
        if observation["status"] != 200:
            raise RuntimeError(f"harness observation open failed ({observation['status']})")
        document = observation["document"]
        stamp = document["grant"]["stamp"]
        snapshot = document["snapshot"]
        self.harness_cursors.append(snapshot["end"]["cursor"])
        self.harness_cursors.append(stamp["context_id"])
        tasks = [
            mutation["projection"]
            for chunk in snapshot["chunks"]
            for mutation in chunk.get("mutations", [])
            if mutation.get("projection", {}).get("projection") == "TASK"
        ]
        if not tasks:
            raise RuntimeError("the live projection contains no TASK")
        target = tasks[0]
        self.target = target
        self.expected_title = str(target["title"] or "")
        self.status_label = STATUS_LABELS.get(str(target["task_status"]), "")
        if not self.expected_title or not self.status_label:
            raise RuntimeError("the expected projection has no title or M1 status label")
        self.facts["projection"] = {
            "task_count": len(tasks),
            "target_task_ref": target["task_ref"],
            "target_title_sha256_16": short_hash(self.expected_title),
            "target_title": self.expected_title,
            "task_status": target["task_status"],
            "status_label": self.status_label,
            "inner_phase": target.get("inner_phase"),
            "iteration_index": target.get("iteration_index"),
            "projection_revision": target.get("projection_revision"),
            "epoch": stamp.get("epoch"),
            "snapshot_id": snapshot.get("begin", {}).get("snapshot_id"),
            "snapshot_expected": snapshot.get("end", {}).get("expected"),
            "snapshot_mutations": sum(
                len(chunk.get("mutations", [])) for chunk in snapshot["chunks"]
            ),
            "context_sha256_16": short_hash(stamp["context_id"]),
            "scope_sha256_16": short_hash(stamp["scope_id"]),
            "cursor_sha256_16": short_hash(snapshot["end"]["cursor"]),
        }

    def launch_desktop(self, name: str, timeout: float = 120.0) -> dict:
        app = DesktopProcess(
            name=name,
            work=self.work,
            localappdata=self.localappdata,
            secret=self.secrets_list[0],
            discovery_path=self.descriptor_path,
            evidence=self.evidence,
            secrets_list=self.secrets_list,
        )
        launch = app.launch()
        self.desktops.append(app)
        self.current_app = app
        live = app.wait_live(self.expected_title, timeout)
        self.cursor_sets[name] = []
        return {
            "instance": name,
            "launch": launch,
            "live": live,
            "alive": app.alive(),
        }

    def desktop_cursors(self, access: AccessLog | None = None) -> list[str]:
        """Cursors minted for the Desktop (the harness's own are excluded)."""
        entries = (access or self.access()).snapshot()
        return [
            cursor
            for cursor in cursors_of(entries)
            if cursor not in self.harness_cursors
        ]

    def remember_cursors(self, name: str) -> list[str]:
        cursors = self.desktop_cursors()
        self.cursor_sets[name] = cursors
        return cursors

    # ---------------------------------------------------------------- helpers

    def app(self) -> DesktopProcess:
        if self.current_app is None:
            raise RuntimeError("no Desktop instance")
        return self.current_app

    def access(self) -> AccessLog:
        assert self.main is not None
        if getattr(self, "_use_alt", False):
            assert self.alt is not None
            return self.alt.access
        return self.main.access

    def server(self) -> RuntimeProcess:
        return self.alt if getattr(self, "_use_alt", False) else self.main  # type: ignore[return-value]

    def record(self, name: str, verdict: str, details: dict, error: str | None = None) -> None:
        entry = {
            "verdict": verdict,
            "recorded_at": now_iso(),
            "details": details,
        }
        if error:
            entry["error"] = error
        self.scenarios[name] = entry
        print(f"[harness] {name}: {verdict}", flush=True)

    def run_scenario(self, name: str, function) -> None:
        print(f"[harness] {name}: starting", flush=True)
        try:
            verdict, details = function()
        except Exception as error:  # noqa: BLE001 - the harness records its own failure
            import traceback

            traceback.print_exc()
            self.record(name, "FAIL", {"exception": type(error).__name__}, error=str(error)[:2000])
            raise
        self.record(name, verdict, details)

    # ---------------------------------------------------------------- scenarios

    def scenario_gap(self) -> tuple[str, dict]:
        assert self.journal is not None and self.target is not None
        app = self.app()
        access = self.access()
        before_mark = access.mark()
        desktop_cursors = self.desktop_cursors(access)
        head = self.journal.head()
        status_before = self.journal.task_status(self.target["task_ref"])
        prior_label = STATUS_LABELS.get(status_before or "", "")
        injected = self.journal.inject_gap(self.target["task_ref"], "CANCELLED", head)
        expected_label = STATUS_LABELS["CANCELLED"]
        conflict = access.wait_for(
            lambda entry: match(entry, prefix="/api/cos/desktop/deltas", status=409),
            15.0,
            before_mark,
        )
        resync = None
        if conflict is not None:
            conflict_mark = index_of(access.snapshot(), conflict)
            resync = access.wait_for(
                lambda entry: (
                    match(entry, prefix="/api/cos/desktop/snapshot", status=200)
                    or match(entry, prefix="/api/cos/desktop/observation", status=200)
                )
                and entry["monotonic"] >= conflict["monotonic"],
                20.0,
                conflict_mark,
            )
        live = app.wait_status_label(self.expected_title, expected_label, 60.0)
        settle_start = access.mark()
        time.sleep(8.0)
        settle = access.since(settle_start)
        extra_conflicts = entries_of(settle, prefix="/api/cos/desktop/deltas", status=409)
        new_cursors = cursors_of(settle)
        stderr = app.write_stderr_extract("gap")
        self.remember_cursors(app.name)
        status_after = self.journal.task_status(self.target["task_ref"])
        verdict = (
            "PASS"
            if (
                conflict is not None
                and resync is not None
                and live["ok"]
                and status_after == "CANCELLED"
                and injected["head_after"] == injected["expected_head_after"]
                and injected["journal_rows_after"] == 0
                and not extra_conflicts
            )
            else "FAIL"
        )
        details = {
            "injection": injected,
            "cursor_before_sha256_16": short_hash(
                desktop_cursors[0] if desktop_cursors else ""
            ),
            "status_before": status_before,
            "status_after": status_after,
            "prior_label": prior_label,
            "expected_label": expected_label,
            "conflict": {
                "status": conflict["status"] if conflict else None,
                "path_sha256_16": short_hash(conflict["path"]) if conflict else None,
                "cursor_sha256_16": short_hash(conflict["cursor"] or "") if conflict else None,
            },
            "resync_request": {
                "method": resync["method"] if resync else None,
                "path": resync["path"].split("?")[0] if resync else None,
                "status": resync["status"] if resync else None,
            },
            "ui": live,
            "settle_window_seconds": 8.0,
            "additional_conflicts": len(extra_conflicts),
            "cursors_after_sha256_16": [
                short_hash(cursor) for cursor in new_cursors
            ],
            "stderr_extract": stderr,
        }
        return verdict, details

    def scenario_overflow(self) -> tuple[str, dict]:
        assert self.journal is not None and self.target is not None
        app = self.app()
        access = self.access()
        before_mark = access.mark()
        status_before = self.journal.task_status(self.target["task_ref"])
        injected = self.journal.inject_overflow(self.target["task_ref"], rows=10001)
        conflict = access.wait_for(
            lambda entry: match(entry, prefix="/api/cos/desktop/deltas", status=409),
            20.0,
            before_mark,
        )
        resync = None
        if conflict is not None:
            conflict_mark = index_of(access.snapshot(), conflict)
            resync = access.wait_for(
                lambda entry: (
                    match(entry, prefix="/api/cos/desktop/snapshot", status=200)
                    or match(entry, prefix="/api/cos/desktop/observation", status=200)
                )
                and entry["monotonic"] >= conflict["monotonic"],
                25.0,
                conflict_mark,
            )
        live = app.wait_status_label(self.expected_title, STATUS_LABELS["CANCELLED"], 60.0)
        rows_after = self.journal.journal_rows()
        settle_start = access.mark()
        time.sleep(8.0)
        settle = access.since(settle_start)
        extra_conflicts = entries_of(settle, prefix="/api/cos/desktop/deltas", status=409)
        status_after = self.journal.task_status(self.target["task_ref"])
        self.remember_cursors(app.name)
        verdict = (
            "PASS"
            if (
                conflict is not None
                and resync is not None
                and live["ok"]
                and rows_after == 0
                and status_after == status_before
                and not extra_conflicts
            )
            else "FAIL"
        )
        details = {
            "injection": injected,
            "retention_bound": 10000,
            "journal_rows_after_prune": rows_after,
            "status_before": status_before,
            "status_after": status_after,
            "conflict": {
                "status": conflict["status"] if conflict else None,
                "cursor_sha256_16": short_hash(conflict["cursor"] or "") if conflict else None,
            },
            "resync_request": {
                "method": resync["method"] if resync else None,
                "path": resync["path"].split("?")[0] if resync else None,
                "status": resync["status"] if resync else None,
            },
            "ui": live,
            "settle_window_seconds": 8.0,
            "additional_conflicts": len(extra_conflicts),
            "stderr_extract": app.write_stderr_extract("overflow"),
        }
        return verdict, details

    def scenario_clean_restart(self) -> tuple[str, dict]:
        access = self.access()
        app = self.app()
        prior = self.remember_cursors(app.name)
        close = app.close()
        ready_after = http_call("GET", f"{ENDPOINT}/api/ready", timeout=5)
        relaunch = self.launch_desktop("clean")
        access_after = self.access()
        fresh = [cursor for cursor in self.desktop_cursors(access_after) if cursor not in prior]
        observations = entries_of(
            access_after.snapshot(), method="POST", prefix="/api/cos/desktop/observation", status=200
        )
        previous = self.desktops[-2]
        stderr_close = previous.write_stderr_extract("clean-close")
        stderr_relaunch = self.app().write_stderr_extract("clean-relaunch")
        verdict = (
            "PASS"
            if (
                close["exited"]
                and "observation shutdown confirmed" in close["close_markers"]
                and ready_after["status"] == 200
                and relaunch["live"]["ok"]
                and fresh
                and observations
            )
            else "FAIL"
        )
        details = {
            "close": close,
            "runtime_ready_after_desktop_exit": ready_after["status"] == 200,
            "relaunch": relaunch,
            "prior_cursors_sha256_16": [short_hash(cursor) for cursor in prior],
            "fresh_cursors_sha256_16": [short_hash(cursor) for cursor in fresh],
            "reused_cursor": not fresh,
            "observation_opens_total": len(observations),
            "epoch": self.server().epoch(),
            "stderr_extracts": {"close": stderr_close, "relaunch": stderr_relaunch},
        }
        return verdict, details

    def scenario_forced_restart(self) -> tuple[str, dict]:
        access = self.access()
        app = self.app()
        prior = self.remember_cursors(app.name)
        forced = app.force_stop()
        close_stderr = app.write_stderr_extract("forced-close")
        ready_after = http_call("GET", f"{ENDPOINT}/api/ready", timeout=5)
        relaunch = self.launch_desktop("forced")
        access_after = self.access()
        fresh = [cursor for cursor in self.desktop_cursors(access_after) if cursor not in prior]
        verdict = (
            "PASS"
            if (
                not forced["alive_after"]
                and ready_after["status"] == 200
                and relaunch["live"]["ok"]
                and fresh
            )
            else "FAIL"
        )
        details = {
            "forced": forced,
            "close_markers_in_forced_close": [
                marker for marker in CLOSE_MARKERS if marker in app.stderr_text()
            ],
            "runtime_ready_after_desktop_exit": ready_after["status"] == 200,
            "relaunch": relaunch,
            "prior_cursors_sha256_16": [short_hash(cursor) for cursor in prior],
            "fresh_cursors_sha256_16": [short_hash(cursor) for cursor in fresh],
            "reused_cursor": not fresh,
            "stderr_extracts": {
                "forced_close": close_stderr,
                "relaunch": self.app().write_stderr_extract("forced-relaunch"),
            },
        }
        return verdict, details

    def scenario_epoch_change_desktop_restart(self) -> tuple[str, dict]:
        assert self.main is not None and self.alt is not None
        main_access = self.main.access
        app = self.app()
        prior = self.remember_cursors(app.name)
        close = app.close()
        close_marker_time = time.monotonic()
        time.sleep(1.0)
        requests_after_close = [
            entry
            for entry in main_access.snapshot()
            if entry["monotonic"] >= close_marker_time
            and match(entry, prefix="/api/cos/desktop/")
            and not exact_path(entry).endswith("/credentials/revoke")
        ]
        main_epoch = self.main.epoch()
        stop = self.main.stop()
        descriptor_removed = self.main.wait_descriptor_gone(20.0)
        self._use_alt = True
        started = self.alt.start()
        descriptor = self.alt.wait_descriptor(20.0)
        self.observe_expected_alt()
        launch = self.launch_desktop("epoch")
        new_cursors = [cursor for cursor in self.desktop_cursors(self.alt.access) if cursor not in prior]
        verdict = (
            "PASS"
            if (
                close["exited"]
                and stop["clean"]
                and descriptor_removed
                and descriptor["origin"] == ALT_ENDPOINT
                and descriptor["epoch"] == started["epoch"]
                and main_epoch != started["epoch"]
                and launch["live"]["ok"]
                and new_cursors
                and not requests_after_close
            )
            else "FAIL"
        )
        details = {
            "close": close,
            "old_origin": ENDPOINT,
            "old_epoch": main_epoch,
            "desktop_requests_to_old_origin_after_close": len(requests_after_close),
            "old_origin_requests_after_close": [
                {"path": entry["path"], "status": entry["status"]} for entry in requests_after_close
            ],
            "teardown_revoke_requests": [
                {"path": entry["path"], "status": entry["status"]}
                for entry in main_access.snapshot()
                if entry["monotonic"] >= close_marker_time
                and exact_path(entry).endswith("/credentials/revoke")
            ],
            "main_stop": stop,
            "descriptor_removed_on_stop": descriptor_removed,
            "new_origin": ALT_ENDPOINT,
            "new_epoch": started["epoch"],
            "epoch_changed": main_epoch != started["epoch"],
            "descriptor": descriptor,
            "relaunch": launch,
            "fresh_cursors_sha256_16": [short_hash(cursor) for cursor in new_cursors],
            "reused_cursor": not new_cursors,
            "stderr_extract": self.app().write_stderr_extract("epoch-relaunch"),
        }
        return verdict, details

    def observe_expected_alt(self) -> None:
        """Re-derive the expected projection from the alternate Runtime."""
        assert self.alt is not None
        self.observe_expected_on(self.alt)

    def observe_expected_on(self, server: RuntimeProcess) -> None:
        credential = server.issue_credential()
        if credential["status"] != 200:
            raise RuntimeError(f"harness credential issuance failed ({credential['status']})")
        token = credential["document"]["token"]
        self.secrets_list.append(token)
        observation = server.open_observation(token)
        if observation["status"] != 200:
            raise RuntimeError(f"harness observation open failed ({observation['status']})")
        document = observation["document"]
        stamp = document["grant"]["stamp"]
        snapshot = document["snapshot"]
        self.harness_cursors.extend([snapshot["end"]["cursor"], stamp["context_id"]])
        tasks = [
            mutation["projection"]
            for chunk in snapshot["chunks"]
            for mutation in chunk.get("mutations", [])
            if mutation.get("projection", {}).get("projection") == "TASK"
        ]
        target = next(
            (task for task in tasks if task["task_ref"] == self.target["task_ref"]),
            tasks[0],
        )
        self.target = target
        self.expected_title = str(target["title"] or "")
        self.status_label = STATUS_LABELS.get(str(target["task_status"]), "")
        self.facts.setdefault("projection_after_epoch_change", {})[server.label] = {
            "epoch": stamp.get("epoch"),
            "task_count": len(tasks),
            "target_task_ref": target["task_ref"],
            "target_title_sha256_16": short_hash(self.expected_title),
            "task_status": target["task_status"],
            "status_label": self.status_label,
            "snapshot_id": snapshot.get("begin", {}).get("snapshot_id"),
            "snapshot_mutations": sum(
                len(chunk.get("mutations", [])) for chunk in snapshot["chunks"]
            ),
        }

    def scenario_epoch_under_load(self) -> tuple[str, dict]:
        assert self.journal is not None and self.alt is not None and self.target is not None
        app = self.app()
        pid_before = app.pid
        access = self.alt.access
        mark = access.mark()
        changed = self.journal.set_task_status(self.target["task_ref"], "COMPLETED")
        expected_label = STATUS_LABELS["COMPLETED"]
        stream = app.wait_status_label(self.expected_title, expected_label, 60.0)
        if stream["ok"]:
            self.status_label = expected_label
        stream_entries = access.since(mark)
        stream_delta_count = len(entries_of(stream_entries, prefix="/api/cos/desktop/deltas"))
        stream_conflicts = len(
            entries_of(stream_entries, prefix="/api/cos/desktop/deltas", status=409)
        )
        old_epoch = self.alt.epoch()
        stop = self.alt.stop()
        withheld = app.wait_withheld(self.expected_title, 60.0)
        epoch_down = self.alt.epoch()
        recovery_mark = access.mark()
        started = self.alt.start()
        descriptor = self.alt.wait_descriptor(20.0)
        live = app.wait_status_label(self.expected_title, expected_label, 150.0)
        recovery_entries = access.since(recovery_mark)
        alive = app.alive()
        stderr_text = app.stderr_text()
        stderr = app.write_stderr_extract("epoch-under-load")
        reauth_after_recovery = self.reauth_line_count()
        deltas_401 = entries_of(recovery_entries, prefix="/api/cos/desktop/deltas", status=401)
        observations_401 = entries_of(
            recovery_entries, prefix="/api/cos/desktop/observation", status=401
        )
        observations_200 = entries_of(
            recovery_entries, prefix="/api/cos/desktop/observation", status=200
        )
        refreshes_401 = entries_of(
            recovery_entries, prefix="/api/cos/desktop/credentials/refresh", status=401
        )
        issues_200 = [
            entry
            for entry in recovery_entries
            if entry["method"] == "POST"
            and exact_path(entry) == "/api/cos/desktop/credentials"
            and entry["status"] == 200
        ]
        snapshots_200 = entries_of(
            recovery_entries, prefix="/api/cos/desktop/snapshot", status=200
        )
        conflicts = entries_of(recovery_entries, prefix="/api/cos/desktop/deltas", status=409)
        commands = entries_of(recovery_entries, prefix="/api/cos/desktop/commands")
        # The automatic-replacement budget is a property of the process runtime
        # binding, not of one observation: the recovery above consumed one of the
        # three replacements. Record an explicit Desktop relaunch so the auth
        # matrix starts from a fresh per-process budget (and prove that this
        # relaunch itself consumes no replacement: the stored credential is valid).
        self.remember_cursors(app.name)
        reset_prior = [cursor for cursors in self.cursor_sets.values() for cursor in cursors]
        budget_reset_close = app.close()
        reset_mark = access.mark()
        budget_reset = self.launch_desktop("reset")
        budget_reset_entries = access.since(reset_mark)
        budget_reset_observations = entries_of(
            budget_reset_entries,
            method="POST",
            prefix="/api/cos/desktop/observation",
            status=200,
        )
        reset_cursors = [
            cursor for cursor in self.desktop_cursors(access) if cursor not in reset_prior
        ]
        verdict = (
            "PASS"
            if (
                stream["ok"]
                and stop["clean"]
                and withheld["ok"]
                and descriptor["epoch"] == started["epoch"]
                and descriptor["origin"] == ALT_ENDPOINT
                and old_epoch != started["epoch"]
                and live["ok"]
                and alive
                and len(refreshes_401) <= 1
                and len(issues_200) <= 1
                and len(observations_200) >= 1
                and len(observations_200) <= CLIENT_BOUNDS["max_resync_attempts"]
                and not commands
                and reauth_after_recovery == 1
                and budget_reset_close.get("exited", False)
                and budget_reset["live"]["ok"]
                and budget_reset_observations
                and reset_cursors
            )
            else "FAIL"
        )
        details = {
            "projection_stream": {
                "status_change": changed,
                "expected_label": expected_label,
                "ui": stream,
                "delta_requests_during_stream": stream_delta_count,
                "delta_conflicts_during_stream": stream_conflicts,
                "journal_head": self.journal.head(),
            },
            "runtime_stop": stop,
            "withheld": withheld,
            "epoch_before_stop": old_epoch,
            "epoch_while_down": epoch_down,
            "epoch_after_restart": started["epoch"],
            "epoch_changed": old_epoch != started["epoch"],
            "descriptor": descriptor,
            "recovery": {
                "ui": live,
                "same_process": alive and app.process is not None and app.pid == pid_before,
                "desktop_pid_before": pid_before,
                "desktop_pid_after": app.pid,
                "deltas_401": len(deltas_401),
                "deltas_409": len(conflicts),
                "observations_401": len(observations_401),
                "observations_200": len(observations_200),
                "refreshes_401": len(refreshes_401),
                "issues_200": len(issues_200),
                "snapshots_200": len(snapshots_200),
                "command_requests": len(commands),
                "reauth_budget": CLIENT_BOUNDS["max_reauth_attempts"],
                "reauthorized_line": "Runtime credential reauthorized after rejection"
                in stderr_text,
            },
            "reauth_line_count_after_recovery": reauth_after_recovery,
            "budget_reset_relaunch": {
                "reason": (
                    "the automatic-replacement budget is per process runtime binding:"
                    " the epoch-change recovery above consumed one of three replacements,"
                    " so a relaunch is recorded explicitly to start the auth-change"
                    " matrix from a fresh per-process budget"
                ),
                "close": budget_reset_close,
                "relaunch": budget_reset,
                "fresh_observation": bool(budget_reset_observations),
                "fresh_cursors_sha256_16": [
                    short_hash(cursor) for cursor in reset_cursors
                ],
                "reauth_line_count_after_relaunch": self.reauth_line_count(),
            },
            "stderr_extract": stderr,
        }
        return verdict, details

    def scenario_long_observation(self) -> tuple[str, dict]:
        """Hold the live session across a bounded idle window.

        UIA enumeration is bounded to the start and the end of the window: each
        ``dump`` walks the whole accessibility tree through PowerShell and can
        stall the UI-driven poll loop, which would then be measured as a
        keepalive gap caused by the harness rather than by the client. Request
        continuity (the 250 ms delta poll, one cursor, no 401/409/observation
        reopen) plus the end-of-window UIA state is the disconnect evidence.
        """
        app = self.app()
        access = self.access()
        mark = access.mark()
        epoch_before = self.server().epoch()
        start_dump, start_text, start_ok = app.wait_text(
            "long-start",
            lambda joined: self.expected_title in joined
            and LIVE_LABEL in joined
            and self.status_label in joined,
            30.0,
        )
        started = time.monotonic()
        deadline = started + self.long_seconds
        checks: list[dict] = []
        next_check = started + 15.0
        while time.monotonic() < deadline:
            if time.monotonic() >= next_check:
                checks.append(
                    {
                        "at_seconds": round(time.monotonic() - started, 2),
                        "alive": app.alive(),
                        "epoch": self.server().epoch(),
                    }
                )
                next_check += 15.0
            time.sleep(0.5)
        end_dump, end_text, end_ok = app.wait_text(
            "long-end",
            lambda joined: self.expected_title in joined
            and LIVE_LABEL in joined
            and self.status_label in joined,
            30.0,
        )
        window_seconds = round(time.monotonic() - started, 2)
        window = access.since(mark)
        deltas = entries_of(window, prefix="/api/cos/desktop/deltas", status=200)
        conflicts = entries_of(window, prefix="/api/cos/desktop/deltas", status=409)
        auth_errors = [
            entry
            for entry in window
            if entry["status"] == 401 or exact_path(entry).startswith("/api/cos/desktop/credentials")
        ]
        observations = [
            entry
            for entry in window
            if exact_path(entry) == "/api/cos/desktop/observation"
        ]
        commands = entries_of(window, prefix="/api/cos/desktop/commands")
        epoch_after = self.server().epoch()
        stderr = app.write_stderr_extract("long")
        max_gap = max_gap_seconds(deltas)
        cursors = {entry["cursor"] for entry in deltas if entry["cursor"]}
        title_occurrences = {
            start_text.count(self.expected_title),
            end_text.count(self.expected_title),
        }
        checks_alive = all(check["alive"] for check in checks)
        checks_epoch = {check["epoch"] for check in checks}
        verdict = (
            "PASS"
            if (
                app.alive()
                and start_ok
                and end_ok
                and checks_alive
                and not conflicts
                and not auth_errors
                and not observations
                and not commands
                and epoch_before == epoch_after
                and len(deltas) >= self.long_seconds * 2
                and max_gap < 3.0
                and len(cursors) == 1
                and len(title_occurrences) == 1
            )
            else "FAIL"
        )
        details = {
            "window_seconds": window_seconds,
            "requested_seconds": self.long_seconds,
            "policy": (
                "no mid-window UIA enumeration: PowerShell UIA tree walks can"
                " stall the UI-driven poll loop and would be measured as a"
                " harness-induced keepalive gap"
            ),
            "start_ui": {
                "ok": start_ok,
                "title_visible": self.expected_title in start_text,
                "live_label": LIVE_LABEL in start_text,
                "status_label": self.status_label in start_text,
                "uia_element_count": start_dump.get("elements"),
            },
            "end_ui": {
                "ok": end_ok,
                "title_visible": self.expected_title in end_text,
                "live_label": LIVE_LABEL in end_text,
                "status_label": self.status_label in end_text,
                "uia_element_count": end_dump.get("elements"),
            },
            "alive_checks": checks,
            "alive_check_count": len(checks),
            "checks_all_alive": checks_alive,
            "delta_requests": len(deltas),
            "distinct_delta_cursors": len(cursors),
            "expected_polls_at_250ms": int(self.long_seconds / 0.25),
            "max_delta_gap_seconds": round(max_gap, 3),
            "conflict_requests": len(conflicts),
            "credential_or_401_requests": len(auth_errors),
            "observation_requests": len(observations),
            "command_requests": len(commands),
            "epoch_before": epoch_before,
            "epoch_after": epoch_after,
            "epoch_stable": epoch_before == epoch_after,
            "checks_epochs": sorted(str(epoch) for epoch in checks_epoch),
            "title_occurrences_set": sorted(title_occurrences),
            "stderr_extract": stderr,
        }
        return verdict, details

    def issue_burst(self, count: int = 64) -> dict:
        assert self.alt is not None
        started = time.monotonic()
        statuses: list[int | None] = []
        tokens: list[str] = []
        for _ in range(count):
            response = self.alt.issue_credential()
            statuses.append(response["status"])
            document = response.get("document") or {}
            if document.get("token"):
                tokens.append(document["token"])
        self.secrets_list.extend(tokens)
        return {
            "issued": count,
            "all_ok": all(status == 200 for status in statuses),
            "statuses": sorted({str(status) for status in statuses}),
            "seconds": round(time.monotonic() - started, 3),
            "registry_bound": CLIENT_BOUNDS["credential_registry_max"],
        }

    def issue_until_reaction(self, mark: int, limit: int = 192) -> dict:
        """Evict the Desktop credential, stopping as soon as it reacts.

        Each issuance is the supported credential route. The registry keeps at
        most 64 entries, so pushing past the bound evicts the oldest credential.
        The loop stops at the first observed 401/re-issue attempt, so the
        Desktop's own issuance is not mixed with the harness burst.
        """
        assert self.alt is not None
        access = self.alt.access
        started = time.monotonic()
        issued = 0
        statuses: list[int | None] = []
        tokens: list[str] = []
        reaction: str | None = None
        while issued < limit:
            response = self.alt.issue_credential()
            statuses.append(response["status"])
            issued += 1
            document = response.get("document") or {}
            if document.get("token"):
                tokens.append(document["token"])
            if issued >= 8:
                entries = access.since(mark)
                if entries_of(entries, prefix="/api/cos/desktop/deltas", status=401):
                    reaction = "delta_401"
                    break
                if entries_of(entries, prefix="/api/cos/desktop/observation", status=401):
                    reaction = "observation_401"
                    break
                if entries_of(
                    entries, prefix="/api/cos/desktop/credentials/refresh", status=401
                ):
                    reaction = "refresh_401"
                    break
            time.sleep(0.005)
        self.secrets_list.extend(tokens)
        return {
            "issued": issued,
            "all_ok": all(status == 200 for status in statuses),
            "statuses": sorted({str(status) for status in statuses}),
            "seconds": round(time.monotonic() - started, 3),
            "registry_bound": CLIENT_BOUNDS["credential_registry_max"],
            "stopped_on": reaction,
        }

    def reauth_line_count(self) -> int:
        return self.app().stderr_text().count("Runtime credential reauthorized after rejection")

    def scenario_auth_burst(self) -> tuple[str, dict]:
        assert self.alt is not None
        app = self.app()
        access = self.alt.access
        attempts: list[dict] = []
        for index in range(1, CLIENT_BOUNDS["max_reauth_attempts"] + 1):
            mark = access.mark()
            prior_cursors = self.desktop_cursors(access)
            reauth_before = self.reauth_line_count()
            burst = self.issue_until_reaction(mark)
            trigger_401 = access.wait_for(
                lambda entry: (
                    match(entry, prefix="/api/cos/desktop/deltas", status=401)
                    or match(entry, prefix="/api/cos/desktop/observation", status=401)
                ),
                25.0,
                mark,
            )
            refresh = None
            issue = None
            delta_403 = None
            observation_403 = None
            observation_200 = None
            if trigger_401 is not None:
                delta_mark = index_of(access.snapshot(), trigger_401)
                refresh = access.wait_for(
                    lambda entry: match(
                        entry, prefix="/api/cos/desktop/credentials/refresh", status=401
                    ),
                    15.0,
                    delta_mark,
                )
                if refresh is not None:
                    refresh_mark = index_of(access.snapshot(), refresh)
                    issue = access.wait_for(
                        lambda entry: (
                            entry["method"] == "POST"
                            and exact_path(entry) == "/api/cos/desktop/credentials"
                            and entry["status"] == 200
                        ),
                        15.0,
                        refresh_mark,
                    )
                    if issue is not None:
                        issue_mark = index_of(access.snapshot(), issue)
                        delta_403 = access.wait_for(
                            lambda entry: match(
                                entry, prefix="/api/cos/desktop/deltas", status=403
                            ),
                            15.0,
                            issue_mark,
                        )
                        forbidden_mark = index_of(
                            access.snapshot(),
                            delta_403
                            if delta_403 is not None
                            else issue,
                        )
                        observation_403 = access.wait_for(
                            lambda entry: match(
                                entry, prefix="/api/cos/desktop/observation", status=403
                            ),
                            15.0,
                            forbidden_mark,
                        )
                        if observation_403 is not None:
                            observation_mark = index_of(access.snapshot(), observation_403)
                            observation_200 = access.wait_for(
                                lambda entry: match(
                                    entry, prefix="/api/cos/desktop/observation", status=200
                                ),
                                20.0,
                                observation_mark,
                            )
            live = app.wait_status_label(self.expected_title, self.status_label, 60.0)
            fresh = [
                cursor for cursor in self.desktop_cursors(access) if cursor not in prior_cursors
            ]
            self.remember_cursors(app.name)
            reauth_after = self.reauth_line_count()
            stderr = app.write_stderr_extract(f"auth-burst-{index}")
            attempts.append(
                {
                    "attempt": index,
                    "burst": burst,
                    "trigger_401": {
                        "path": exact_path(trigger_401) if trigger_401 else None,
                        "status": trigger_401["status"] if trigger_401 else None,
                    },
                    "trigger_401_present": trigger_401 is not None,
                    "refresh_401": refresh is not None,
                    "issue_200_after_refresh": issue is not None,
                    "delta_403_principal_mismatch": delta_403 is not None,
                    "observation_403_principal_mismatch": observation_403 is not None,
                    "observation_200_fresh": observation_200 is not None,
                    "fresh_cursor": bool(fresh),
                    "fresh_cursors_sha256_16": [
                        short_hash(cursor) for cursor in fresh
                    ],
                    "reauth_line_delta": reauth_after - reauth_before,
                    "ui_live": live,
                    "stderr_extract": stderr,
                }
            )
            time.sleep(2.6)
        commands = entries_of(access.snapshot(), prefix="/api/cos/desktop/commands")
        reauth_lines = self.reauth_line_count()
        verdict = (
            "PASS"
            if (
                len(attempts) == CLIENT_BOUNDS["max_reauth_attempts"]
                and all(item["trigger_401_present"] for item in attempts)
                and all(item["refresh_401"] for item in attempts)
                and all(item["issue_200_after_refresh"] for item in attempts)
                and all(item["observation_200_fresh"] for item in attempts)
                and all(item["ui_live"]["ok"] for item in attempts)
                and all(item["reauth_line_delta"] == 1 for item in attempts)
                and not commands
                and reauth_lines == CLIENT_BOUNDS["max_reauth_attempts"]
            )
            else "FAIL"
        )
        details = {
            "mechanism": (
                "Runtime credential registry pressure: each burst issues past the"
                " registry bound (64), which evicts the Desktop's stored credential"
                " through the supported issuance API (no test-only Runtime hook)."
            ),
            "attempts": attempts,
            "reauth_line_count": reauth_lines,
            "reauth_bound": CLIENT_BOUNDS["max_reauth_attempts"],
            "reauth_min_interval_seconds": CLIENT_BOUNDS["reauth_min_interval_seconds"],
            "command_requests": len(commands),
            "observed_transition": (
                "delta 401 -> refresh 401 -> issue 200 (new principal) -> retried read"
                " 403 PRINCIPAL_MISMATCH -> resume open 403 -> fresh observation 200"
                " with a new cursor"
            ),
            "findings": [
                "The reissued credential belongs to a new server-managed principal, so"
                " the old grant cannot be inherited: the retried read and the resumed"
                " open fail closed with 403 before the client opens a fresh observation."
                " This is an observed limitation of credential recovery without a"
                " Runtime restart, not a silent recovery: no stale projection is"
                " admitted and the projection is re-read from a fresh snapshot.",
            ],
        }
        return verdict, details

    def scenario_auth_exclusion(self) -> tuple[str, dict]:
        assert self.alt is not None
        app = self.app()
        access = self.alt.access
        mark = access.mark()
        reauth_before = self.reauth_line_count()
        burst = self.issue_burst(64)
        delta_401 = access.wait_for(
            lambda entry: match(entry, prefix="/api/cos/desktop/deltas", status=401),
            25.0,
            mark,
        )
        excluded = app.wait_excluded(self.expected_title, 60.0)
        exclusion_mark = access.mark()
        time.sleep(10.0)
        after = access.since(exclusion_mark)
        credential_requests = [
            entry
            for entry in after
            if exact_path(entry).startswith("/api/cos/desktop/credentials")
        ]
        command_requests = entries_of(after, prefix="/api/cos/desktop/commands")
        reconnect = app.invoke(RECONNECT_LABEL, "excluded")
        invoke_mark = access.mark()
        time.sleep(3.0)
        after_invoke = access.since(invoke_mark)
        after_invoke_credential = [
            entry
            for entry in after_invoke
            if exact_path(entry).startswith("/api/cos/desktop/credentials")
        ]
        after_invoke_delta = entries_of(after_invoke, prefix="/api/cos/desktop/deltas")
        still_excluded = app.wait_excluded(self.expected_title, 10.0)
        self.remember_cursors(app.name)
        stderr = app.write_stderr_extract("auth-excluded")
        reauth_delta = self.reauth_line_count() - reauth_before
        verdict = (
            "PASS"
            if (
                delta_401 is not None
                and excluded["ok"]
                and not credential_requests
                and not command_requests
                and still_excluded["ok"]
                and not after_invoke_credential
                and reauth_delta == 0
            )
            else "FAIL"
        )
        details = {
            "burst": burst,
            "delta_401": delta_401 is not None,
            "reauth_line_delta": reauth_delta,
            "excluded_ui": excluded,
            "exclusion_window_seconds": 10.0,
            "credential_requests_after_exclusion": len(credential_requests),
            "command_requests_after_exclusion": len(command_requests),
            "reconnect_action": reconnect,
            "requests_after_reconnect_action": {
                "credential": len(after_invoke_credential),
                "delta": len(after_invoke_delta),
            },
            "still_excluded_after_reconnect": still_excluded["ok"],
            "stderr_extract": stderr,
            "findings": [
                "After the per-session replacement budget is exhausted the client sends"
                " the 401 through and stops: no credential request follows and the UI"
                " renders 情報源なし / データ非公開 plus"
                " データは非公開 — 現在の許可がありません.",
                "The intervention strip's 再接続する action re-subscribes the local"
                " publication only; it does not fabricate a new credential or projection.",
            ],
        }
        return verdict, details

    def scenario_post_exclusion_relaunch(self) -> tuple[str, dict]:
        assert self.alt is not None
        app = self.app()
        self.remember_cursors(app.name)
        close = app.close()
        before = self.alt.access.mark()
        launch = self.launch_desktop("relaunch")
        access = self.alt.access
        entries = access.since(before)
        refreshes_401 = entries_of(
            entries, prefix="/api/cos/desktop/credentials/refresh", status=401
        )
        issues_200 = [
            entry
            for entry in entries
            if entry["method"] == "POST"
            and exact_path(entry) == "/api/cos/desktop/credentials"
            and entry["status"] == 200
        ]
        observations_200 = entries_of(
            entries, prefix="/api/cos/desktop/observation", status=200
        )
        prior = [cursor for cursors in self.cursor_sets.values() for cursor in cursors]
        new_cursors = [
            cursor for cursor in self.desktop_cursors(access) if cursor not in prior
        ]
        stderr = self.app().write_stderr_extract("relaunch-recovery")
        verdict = (
            "PASS"
            if (
                close.get("exited", False)
                and launch["live"]["ok"]
                and len(refreshes_401) <= 1
                and len(issues_200) <= 1
                and len(observations_200) >= 1
                and new_cursors
            )
            else "FAIL"
        )
        details = {
            "close": close,
            "relaunch": launch,
            "refreshes_401": len(refreshes_401),
            "issues_200": len(issues_200),
            "observations_200": len(observations_200),
            "fresh_cursors_sha256_16": [
                short_hash(cursor) for cursor in new_cursors
            ],
            "epoch": self.alt.epoch(),
            "stderr_extract": stderr,
        }
        return verdict, details

    # ---------------------------------------------------------------- cleanup

    def cleanup(self) -> dict:
        facts: dict = {"started_at": now_iso()}
        for app in self.desktops:
            if app.alive():
                app.force_stop()
        facts["desktop_processes_alive"] = [
            app.pid for app in self.desktops if base.pid_alive(app.pid)
        ]
        for server in (self.alt, self.main):
            if server is not None and server.process is not None:
                facts[f"runtime_{server.port}_stop"] = server.stop()
        for server in (self.alt, self.main):
            if server is not None:
                server.close()
        facts["credential_store_cleanup"] = {
            ENDPOINT: cleanup_credential_store(ENDPOINT),
            ALT_ENDPOINT: cleanup_credential_store(ALT_ENDPOINT),
        }
        facts["ports_free"] = {str(PORT): port_free(PORT), str(ALT_PORT): port_free(ALT_PORT)}
        if self.db_copy is not None:
            facts["db_copy_after"] = {
                "sha256": base.sha256_file(self.db_copy),
                "bytes": self.db_copy.stat().st_size,
                "journal_rows": self.journal.journal_rows() if self.journal else None,
            }
        with contextlib.suppress(OSError):
            shutil.rmtree(self.localappdata)
        facts["localappdata_removed"] = not self.localappdata.exists()
        facts["work_retained"] = str(self.work)
        facts["work_note"] = (
            "target/rt3-matrix/run1 retains the isolated DB copy, config and raw logs"
            " for this run; nothing outside target/ is created."
        )
        facts["finished_at"] = now_iso()
        return facts

    # ---------------------------------------------------------------- evidence

    def write_evidence(self) -> dict:
        for server in (self.main, self.alt):
            if server is None:
                continue
            entries = server.access.snapshot()
            summary = {
                "port": server.port,
                "log_file": server.log_path.name,
                "requests": len(entries),
                "by_status": {},
                "by_route": {},
                "deltas": len(entries_of(entries, prefix="/api/cos/desktop/deltas")),
                "delta_conflicts": len(
                    entries_of(entries, prefix="/api/cos/desktop/deltas", status=409)
                ),
                "delta_401": len(
                    entries_of(entries, prefix="/api/cos/desktop/deltas", status=401)
                ),
                "observation_opens": len(
                    entries_of(entries, method="POST", prefix="/api/cos/desktop/observation", status=200)
                ),
                "observation_refused": len(
                    entries_of(entries, method="POST", prefix="/api/cos/desktop/observation")
                )
                - len(
                    entries_of(entries, method="POST", prefix="/api/cos/desktop/observation", status=200)
                ),
                "credential_issues": len(
                    entries_of(entries, method="POST", prefix="/api/cos/desktop/credentials", status=200)
                ),
                "credential_refresh_refused": len(
                    entries_of(entries, prefix="/api/cos/desktop/credentials/refresh", status=401)
                ),
                "snapshot_refreshes": len(
                    entries_of(entries, prefix="/api/cos/desktop/snapshot", status=200)
                ),
                "command_requests": len(
                    entries_of(entries, prefix="/api/cos/desktop/commands")
                ),
            }
            for entry in entries:
                summary["by_status"][str(entry["status"])] = (
                    summary["by_status"].get(str(entry["status"]), 0) + 1
                )
                route = entry["path"].split("?")[0]
                summary["by_route"][route] = summary["by_route"].get(route, 0) + 1
            base.write_json(self.evidence / f"access-{server.port}.json", summary)
            sanitized = base.redact(
                server.log_path.read_text(encoding="utf-8", errors="replace"),
                self.secrets_list,
            )
            (self.evidence / f"runtime-{server.port}.log.sanitized.txt").write_text(
                sanitized, encoding="utf-8"
            )
            summary["sanitized_log_bytes"] = len(sanitized.encode("utf-8"))
            summary["sanitized_log_sha256"] = base.sha256_text(sanitized)
            self.facts.setdefault("access_logs", {})[str(server.port)] = summary
        # Evidence file hashes.
        hashes: dict[str, str] = {}
        for path in sorted(self.evidence.rglob("*")):
            if path.is_file():
                hashes[path.relative_to(self.evidence).as_posix()] = base.sha256_file(path)
        self.facts["files"] = hashes
        return hashes


# --------------------------------------------------------------------------- main


def summarize(scenarios: dict) -> dict:
    verdicts = {
        name: scenarios.get(name, {}).get("verdict", "NOT_RUN") for name in SCENARIO_ORDER
    }
    return {
        "verdicts": verdicts,
        "passed": [name for name, verdict in verdicts.items() if verdict == "PASS"],
        "failed": [name for name, verdict in verdicts.items() if verdict == "FAIL"],
        "not_run": [name for name, verdict in verdicts.items() if verdict == "NOT_RUN"],
        "all_required_passed": all(verdict == "PASS" for verdict in verdicts.values()),
    }


def leak_scan(evidence: Path, secrets_list: list[str]) -> dict:
    patterns = {
        "bearer_header": re.compile(r"Bearer [A-Za-z0-9._-]{16,}"),
        "issued_secret_assignment": re.compile(r"ORCHESTRATOR_AUTH_SECRET\s*[=:]\s*\S+"),
        "desktop_secret_assignment": re.compile(r"COS_DESKTOP_RUNTIME_ISSUANCE_SECRET\s*[=:]\s*\S+"),
    }
    findings: list[dict] = []
    for path in evidence.rglob("*"):
        if not path.is_file():
            continue
        data = path.read_bytes()
        text = data.decode("utf-8", errors="replace")
        for secret in secrets_list:
            if secret and secret.encode("utf-8") in data:
                findings.append(
                    {"file": path.name, "kind": "known_secret", "sha256_16": base.sha256_text(secret)[:16]}
                )
                break
        for name, pattern in patterns.items():
            match_object = pattern.search(text)
            if match_object:
                findings.append({"file": path.name, "kind": name})
    return {
        "secrets_checked": len([secret for secret in secrets_list if secret]),
        "patterns": sorted(patterns),
        "findings": findings,
        "clean": not findings,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--long-seconds", type=int, default=180)
    parser.add_argument("--keep-work", action="store_true", default=True)
    parser.add_argument("--quick", action="store_true", help="short long-observation window")
    arguments = parser.parse_args(argv)
    long_seconds = 30 if arguments.quick else max(30, arguments.long_seconds)

    for required in (RUNTIME_PYTHON, SOURCE_DB, DESKTOP_EXE, PROBE, CONFIG_TEMPLATE):
        if not required.is_file():
            print(f"missing required path: {required}", file=sys.stderr)
            return 2
    for port in (PORT, ALT_PORT):
        if not port_free(port):
            print(f"port {port} is already in use; refusing to run", file=sys.stderr)
            return 2

    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    WORK_ROOT.mkdir(parents=True, exist_ok=True)
    for stale in ("failure.json", "session.json"):
        (EVIDENCE_DIR / stale).unlink(missing_ok=True)

    campaign = Campaign(long_seconds=long_seconds, keep_work=arguments.keep_work)
    error: Exception | None = None
    cleanup_facts: dict = {}
    try:
        with base.native_lock("impl-flow2/rt3-matrix"):
            campaign.prepare()
            for origin in (ENDPOINT, ALT_ENDPOINT):
                cleanup_credential_store(origin)
            campaign.start_main()
            campaign.launch_desktop("a0")
            campaign.run_scenario("gap_closed_resync", campaign.scenario_gap)
            campaign.run_scenario("cursor_overflow_resync", campaign.scenario_overflow)
            campaign.run_scenario("desktop_clean_restart", campaign.scenario_clean_restart)
            campaign.run_scenario("desktop_forced_restart", campaign.scenario_forced_restart)
            campaign.run_scenario(
                "runtime_epoch_change_desktop_restart",
                campaign.scenario_epoch_change_desktop_restart,
            )
            campaign.run_scenario("epoch_change_under_load", campaign.scenario_epoch_under_load)
            campaign.run_scenario("long_observation_idle", campaign.scenario_long_observation)
            campaign.run_scenario("auth_change_bounded_reauth", campaign.scenario_auth_burst)
            campaign.run_scenario("auth_change_exclusion_honest", campaign.scenario_auth_exclusion)
            campaign.run_scenario(
                "post_exclusion_relaunch_recovery",
                campaign.scenario_post_exclusion_relaunch,
            )
    except Exception as error_object:  # noqa: BLE001 - harness records its own failure
        error = error_object
        print(f"[harness] error: {error_object}", file=sys.stderr, flush=True)
    finally:
        try:
            cleanup_facts = campaign.cleanup()
        except Exception as cleanup_error:  # noqa: BLE001 - cleanup failures are recorded
            cleanup_facts = {"error": f"{type(cleanup_error).__name__}: {cleanup_error}"}
            print(f"[harness] cleanup error: {cleanup_error}", file=sys.stderr, flush=True)

    campaign.facts["cleanup"] = cleanup_facts
    campaign.facts["finished_at"] = now_iso()
    summary = summarize(campaign.scenarios)
    campaign.facts["summary"] = summary
    session = {
        "measurement": "RT3 full reconnect/resync/epoch matrix (isolated Runtime, native Desktop)",
        "date": now_iso(),
        "harness": campaign.facts.get("harness"),
        "desktop": campaign.facts.get("desktop"),
        "runtime": campaign.facts.get("runtime"),
        "db_copy": campaign.facts.get("db_copy"),
        "projection": campaign.facts.get("projection"),
        "projection_after_epoch_change": campaign.facts.get("projection_after_epoch_change"),
        "main_server": campaign.facts.get("main_server"),
        "scenarios": campaign.scenarios,
        "summary": summary,
        "access_logs": campaign.facts.get("access_logs"),
        "cleanup": cleanup_facts,
        "residuals": [
            "Real-provider generation/task survival during a Desktop restart is NOT_RUN:"
            " no generation provider was invoked in this campaign (terminal tasks in the"
            " isolated DB copy; the projection stream is a test-only TASK status write in"
            " the copy).",
            "The Desktop GUI command path was not driven, so a live command resend after a"
            " 401 is NOT_RUN; the campaign records zero command requests and relies on the"
            " client command-non-retry unit tests rather than claiming a live command probe.",
            "GPU/media/UX acceptance is out of scope; the campaign observes projection"
            " states, UIA labels and Runtime request statuses only.",
            "The auth-change matrix uses the Runtime credential-registry bound (issuance"
            " pressure) as the supported revocation mechanism; a real token TTL expiry is"
            " not measured.",
        ],
    }
    if error is not None:
        session["error"] = {"type": type(error).__name__, "message": str(error)[:2000]}
    base.write_json(EVIDENCE_DIR / "session.json", session)
    hashes = campaign.write_evidence()
    hashes.pop("session.json", None)
    session["access_logs"] = campaign.facts.get("access_logs")
    scan = leak_scan(EVIDENCE_DIR, campaign.secrets_list)
    session["files"] = hashes
    session["leak_scan"] = scan
    base.write_json(EVIDENCE_DIR / "session.json", session)
    # The final session document cannot contain its own hash; record it beside.
    (EVIDENCE_DIR / "session.sha256").write_text(
        f"{base.sha256_file(EVIDENCE_DIR / 'session.json')}  session.json\n",
        encoding="utf-8",
    )
    print(
        f"[harness] summary: passed={len(summary['passed'])} failed={len(summary['failed'])}"
        f" not_run={len(summary['not_run'])} leak_clean={scan['clean']}",
        flush=True,
    )
    if not scan["clean"] or error is not None or not summary["all_required_passed"]:
        base.write_json(
            EVIDENCE_DIR / "failure.json",
            {
                "error": type(error).__name__ if error is not None else "scenario_failure",
                "message": str(error)[:2000] if error is not None else "one or more scenarios did not pass",
                "summary": summary,
                "leak_scan": scan,
            },
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
