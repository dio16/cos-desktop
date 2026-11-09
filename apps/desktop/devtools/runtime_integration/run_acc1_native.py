#!/usr/bin/env python3
"""MP-ACC1 native golden workflow + bounded failure injections (test-only).

This harness launches the *release Desktop binary* against an isolated Runtime
started from ``C:/AI/orchestrator`` and measures what the real window/UIA
surface shows. It is evidence-only: it does not change product code and it
never fabricates a displayed image or a successful step.

What one run does (``--scenario golden``):

1. Copies the Runtime database through a read-only SQLite backup, starts the
   isolated Runtime on ``127.0.0.1:8132`` with a per-run auth secret, media
   handle secret, redirected RT8 discovery descriptor and isolated
   config/logs/CAS/outputs.
2. Verifies ``/api/ready``, the Runtime generation-plane health and the direct
   ComfyUI/Ollama preflight.
3. Launches the release Desktop binary with **no** endpoint/token override:
   only the per-run issuance secret and the redirected discovery path. Waits
   for the native window and the connected task rail (UIA + PrintWindow).
4. Opens the Brief surface through its UIA button, fills 目的/方向性 with
   PostMessage WM_CHAR text into the clicked UIA edit element, selects the
   catalog recipe (UIA Invoke when exposed, otherwise a synthetic
   WM_LBUTTONDOWN/UP inside the UIA bounding rectangle), and submits.
5. Follows the live observation channel (harness-side read:observe credential)
   through ASSIGNED/ACTIVE/RUNNING to the terminal projection, taking UIA
   excerpts and PrintWindow screenshots at each new state.
6. Restarts the Desktop mid-generation (WM_CLOSE, relaunch with the same
   discovery/issuance secret) and records whether the native app resyncs and
   shows the task again.
7. Selects the task through the rail search, views its candidate, captures the
   artifact surface and records whether the image is displayed, unavailable or
   still preparing; then fetches the canonical media bytes through RC-13 and
   verifies the digest.
8. Cleans up every process it started and verifies the ports are free.

Failure-injection scenarios are separate runs (``--scenario stop``,
``--scenario revoke``, ``--scenario missing-media``) so one injection cannot
mask another. A scenario that cannot be constructed safely records NOT_RUN with
the exact reason instead of a fabricated result.

The evidence is written to
``docs/development/evidence-published/acc1-native-latest/``.
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
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
sys.path.insert(0, str(HARNESS))
import run_acc1 as acc1  # noqa: E402 - local test helper, stdlib only
import run_integration as base  # noqa: E402 - local test helper, stdlib only

ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
RUNTIME_PYTHON = base.RUNTIME_PYTHON
SOURCE_DB = base.SOURCE_DB
SOURCE_PRESETS = acc1.SOURCE_PRESETS
DESKTOP_EXE = base.DESKTOP_EXE
PROBE = base.PROBE
CONTROL_LAUNCHER = HARNESS / "isolated_runtime_control.py"
EVIDENCE_DIR = (
    ROOT / "docs" / "development" / "evidence-published" / "acc1-native-closure-latest"
)
WORK_ROOT = ROOT / "target" / "acc1-native"
PORT_MAIN = 8132
PORT_INJECT = 8133
GOAL_PREFIX = "ACC1-NATIVE"
RECIPE_LABEL = "Production still (Krea2)"

MEDIA_AVAILABLE = "メディア画像を表示できます。"
MEDIA_DISPLAYED_MARKER = "メディア画像（"
MEDIA_PREPARING_MARKERS = ("メディア画像を準備中", "準備中")
MEDIA_MISSING = "画像を利用できません: この成果物のメディアが見つかりません。"
MEDIA_FAILED = "画像を利用できません: メディアの取得または検証に失敗しました。"
MEDIA_UNAVAILABLE_MARKER = "画像を利用できません"
CAPABILITY_BLOCKED_MARKER = "機能の利用可否情報が投影されていません"

# Test-only authoritative capability material.  The Runtime composes the
# CAPABILITIES projection only when a capability snapshot set exists for the
# task's run (RC-11 keeps that contract); the production-derived DB copy has
# none, so the native gate would stay "capability not projected" forever.  This
# harness seeds one Rust-validated set into the isolated copy, exactly like the
# RT5 lane: it never writes a projection, only the authoritative snapshot set.
CAPABILITY_EFFECTS = ["compute", "local_write"]
CAPABILITY_PERMISSIONS = ["artifact.write"]
CAPABILITY_SEED_ID = "img-gen-v1"
CAPABILITY_SEED_VERSION = "1.0.0"


# --------------------------------------------------------------------------- basics


def redact_text(text: str) -> str:
    """Redact media capabilities and bearer values from retained evidence."""
    text = re.sub(r"mh1\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+", "<redacted-media-handle>", text)
    return re.sub(r"Bearer\s+[A-Za-z0-9._\-]{16,}", "Bearer <redacted>", text)


def redact_document(value: object) -> object:
    """Recursively redact string values before a record is written."""
    if isinstance(value, str):
        return redact_text(value)
    if isinstance(value, list):
        return [redact_document(item) for item in value]
    if isinstance(value, dict):
        return {key: redact_document(item) for key, item in value.items()}
    return value


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def sha256_bytes(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def short_hash(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2), encoding="utf-8")


def port_open(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.3)
        return probe.connect_ex(("127.0.0.1", port)) == 0


def wait_port_free(port: int, timeout: float = 30.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not port_open(port):
            return True
        time.sleep(0.5)
    return not port_open(port)


def bounded(text: str, limit: int = 6000) -> str:
    return text[-limit:] if len(text) > limit else text


# --------------------------------------------------------------------------- UIA probe


def capability_seed_modules():
    """Import the Runtime's own DB/registry helpers in the harness process.

    ``COS_RUNTIME_PROCESS_ROLE=generation-worker`` keeps ``Database.init()``
    from claiming the Desktop observation epoch; the serving Runtime stays the
    only epoch owner (d6e0ba2).  This is the same seeding method the RT5 lane
    uses, and the isolated copy is test-only.
    """
    os.environ["COS_RUNTIME_PROCESS_ROLE"] = "generation-worker"
    root = str(RUNTIME_ROOT)
    if root not in sys.path:
        sys.path.insert(0, root)
    from orchestrator_app import capability_registry  # noqa: PLC0415
    from orchestrator_app.contracts.canonical import digest_v1  # noqa: PLC0415
    from orchestrator_app.db import Database  # noqa: PLC0415

    return Database, digest_v1, capability_registry


def build_capability_snapshot(digest_v1, capability_id: str, version: str) -> dict:
    """One authoritative CapabilitySnapshotV1 with its Rust digest (RT5 shape)."""
    snapshot = {
        "capability_id": capability_id,
        "schema_version": 1,
        "implementation_version": version,
        "implementation_digest": "sha256:" + hashlib.sha256(
            f"{capability_id}:{version}:impl".encode("utf-8")).hexdigest(),
        "source": "local",
        "executable_or_package_digest": "sha256:" + hashlib.sha256(
            f"{capability_id}:{version}:package".encode("utf-8")).hexdigest(),
        "input_schema_digest": "sha256:" + hashlib.sha256(
            f"{capability_id}:input".encode("utf-8")).hexdigest(),
        "output_schema_digest": "sha256:" + hashlib.sha256(
            f"{capability_id}:output".encode("utf-8")).hexdigest(),
        "config_schema_digest": "sha256:" + hashlib.sha256(
            f"{capability_id}:config".encode("utf-8")).hexdigest(),
        "effects": list(CAPABILITY_EFFECTS),
        "permissions": list(CAPABILITY_PERMISSIONS),
        "auth_requirement": {"mode": "none"},
        "resource": {"estimated_vram_mb": 1024, "estimated_runtime_s": 30,
                     "cost_budget": 10, "timeout_s": 120},
    }
    snapshot["descriptor_digest"] = digest_v1(
        "capability",
        {key: value for key, value in snapshot.items() if key != "descriptor_digest"},
    )
    authorization = {
        "capability_id": capability_id,
        "known_implementation_versions": [version],
        "authorized_effects": list(CAPABILITY_EFFECTS),
        "authorized_permissions": list(CAPABILITY_PERMISSIONS),
    }
    return {"snapshot": snapshot, "authorization": authorization}


def seed_task_capability(db_copy: Path, task_ref: str) -> dict:
    """Bind one authoritative capability set to the task's latest run."""
    Database, digest_v1, registry = capability_seed_modules()
    material = build_capability_snapshot(digest_v1, CAPABILITY_SEED_ID,
                                         CAPABILITY_SEED_VERSION)
    database = Database(str(db_copy))
    try:
        row = database._conn.execute(
            "SELECT run_id, iteration, phase FROM creative_runs WHERE task_id = ?"
            " ORDER BY iteration DESC, created_at DESC, run_id DESC LIMIT 1",
            (task_ref,),
        ).fetchone()
        run_source = "drain-created-run"
        if row is None:
            maximum = database._conn.execute(
                "SELECT MAX(iteration) AS maximum FROM creative_runs WHERE task_id = ?",
                (task_ref,),
            ).fetchone()
            iteration = int((maximum["maximum"] if maximum else 0) or 0) + 1
            run_id = f"run-acc1-cap-{task_ref[-8:]}-i{iteration}"
            database._conn.execute(
                "INSERT INTO creative_runs(run_id, task_id, iteration, parent_run_id,"
                " plan_id, plan_digest, proposal_id, phase, stop_reason, record_json,"
                " created_at) VALUES (?, ?, ?, NULL, ?, ?, NULL, 'REFINE', NULL, '{}', ?)",
                (run_id, task_ref, iteration, f"plan-acc1-cap-{task_ref[-8:]}-i{iteration}",
                 "sha256:" + hashlib.sha256(f"{run_id}:plan".encode("utf-8")).hexdigest(),
                 now_iso()),
            )
            database._conn.commit()
            run_source = "harness-inserted-run"
        else:
            run_id = str(row["run_id"])
        result = registry.store_snapshot_set(
            database, run_id, {CAPABILITY_SEED_ID: material["snapshot"]},
            {CAPABILITY_SEED_ID: material["authorization"]},
        )
        status_row = database._conn.execute(
            "SELECT status FROM creative_tasks WHERE task_id = ?", (task_ref,)
        ).fetchone()
        return {
            "seeded_at": now_iso(),
            "db_copy": str(db_copy),
            "run_id": run_id,
            "run_source": run_source,
            "task_status_at_seed": str(status_row["status"]) if status_row else None,
            "store_result": result,
            "snapshot": material["snapshot"],
            "authorization": material["authorization"],
        }
    finally:
        database.close()


def read_observation_epoch(db_copy: Path) -> dict:
    """Read the serving-owned epoch ledger without opening a Runtime Database.

    A plain read-only SQLite connection never runs ``Database.init()``, so this
    cannot claim or invalidate the serving process's epoch.
    """
    try:
        connection = sqlite3.connect(f"file:{db_copy.as_posix()}?mode=ro", uri=True, timeout=5)
    except sqlite3.Error as error:
        return {"readable": False, "error": f"{type(error).__name__}: {error}"}
    try:
        connection.row_factory = sqlite3.Row
        row = connection.execute(
            "SELECT epoch, claimed_at FROM cos_desktop_observation_epoch WHERE singleton = 1"
        ).fetchone()
        if row is None:
            return {"readable": True, "row": None}
        return {"readable": True, "epoch": str(row["epoch"]),
                "claimed_at": str(row["claimed_at"])}
    except sqlite3.Error as error:
        return {"readable": False, "error": f"{type(error).__name__}: {error}"}
    finally:
        connection.close()


class Ui:
    """One Desktop window addressed through ``probe_window.ps1``."""

    def __init__(self, pid: int, work: Path):
        self.pid = pid
        self.work = work
        self.uia_dir = work / "uia"
        self.shot_dir = work / "shots"
        self.uia_dir.mkdir(parents=True, exist_ok=True)
        self.shot_dir.mkdir(parents=True, exist_ok=True)
        self.calls = 0

    def probe(self, action: str, *, match: str = "", text: str = "", shot: Path | None = None) -> dict:
        self.calls += 1
        stamp = f"{self.calls:03d}-{action}"
        out = self.uia_dir / f"{stamp}.json"
        image_out = shot or (self.shot_dir / f"{stamp}.png" if action == "shot" else None)
        if out.exists():
            out.unlink()
        command = [
            "powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass",
            "-File", str(PROBE), "-ProcessId", str(self.pid), "-Action", action, "-Out", str(out),
        ]
        if image_out is not None:
            command += ["-ImageOut", str(image_out)]
        if match:
            command += ["-NameMatch", match]
        if text:
            command += ["-Text", text]
        completed = subprocess.run(
            command, capture_output=True, text=True, timeout=120, encoding="utf-8", errors="replace"
        )
        if not out.is_file():
            return {"action": action, "found": False,
                    "error": f"probe produced no output (exit {completed.returncode})",
                    "stderr": completed.stderr[-400:]}
        result = json.loads(out.read_text(encoding="utf-8"))
        result["_out"] = str(out)
        if image_out is not None and image_out.is_file():
            result["_image"] = str(image_out)
            result["_image_sha256"] = sha256_file(image_out)
            result["_image_bytes"] = image_out.stat().st_size
        if action == "dump":
            result["_names_sha256"] = sha256_bytes("\n".join(result.get("names") or []).encode("utf-8"))
        return result

    def dump(self) -> dict:
        return self.probe("dump")

    def names(self) -> list[str]:
        return list(self.dump().get("names") or [])

    def wait_names(self, predicate, timeout: float, label: str) -> tuple[dict, str, bool]:
        deadline = time.monotonic() + timeout
        last: dict = {"found": False}
        last_text = ""
        while time.monotonic() < deadline:
            last = self.dump()
            last_text = "\n".join(last.get("names") or [])
            if last.get("found") and predicate(last_text):
                return last, last_text, True
            if not base.pid_alive(self.pid):
                break
            time.sleep(1.0)
        return last, last_text, False

    def wait_contains(self, needle: str, timeout: float, label: str) -> tuple[dict, str, bool]:
        return self.wait_names(lambda value: needle in value, timeout, label)

    def shot(self, label: str) -> dict:
        path = self.shot_dir / f"{label}.png"
        result = self.probe("shot", shot=path)
        result["label"] = label
        return result

    def invoke(self, match: str) -> dict:
        return self.probe("invoke", match=match)

    def click(self, match: str) -> dict:
        return self.probe("click", match=match)

    def type_text(self, match: str, text: str) -> dict:
        return self.probe("type", match=match, text=text)

    def valueread(self, match: str) -> dict:
        return self.probe("valueread", match=match)

    def excerpt(self, needles: tuple[str, ...], limit: int = 2500) -> str:
        lines = [line for line in self.names() if any(needle in line for needle in needles)]
        return bounded("\n".join(lines), limit)


def uia_snapshot(ui: Ui, label: str) -> dict:
    dump = ui.probe("dump")
    shot = ui.shot(label)
    return {
        "label": label,
        "at": now_iso(),
        "elements": dump.get("elements"),
        "names_sha256": dump.get("_names_sha256"),
        "names": (dump.get("names") or [])[:120],
        "window": {
            "title": dump.get("title"), "class": dump.get("class"),
            "width": dump.get("width"), "height": dump.get("height"),
        },
        "screenshot": {
            "file": str(shot.get("_image") or ""),
            "sha256": shot.get("_image_sha256"),
            "bytes": shot.get("_image_bytes"),
            "printed": shot.get("ok"),
            "width": shot.get("width"),
            "height": shot.get("height"),
        },
    }


# --------------------------------------------------------------------------- Desktop app


class NativeApp:
    def __init__(self, *, label: str, work: Path, secret: str, discovery: Path):
        self.label = label
        self.work = work
        self.secret = secret
        self.discovery = discovery
        self.localappdata = work / f"localappdata-{label}"
        self.log_path = work / f"desktop-{label}.log"
        self.log_handle = None
        self.process: subprocess.Popen | None = None
        self.ui: Ui | None = None
        self.launches = 0

    def launch(self) -> int:
        self.launches += 1
        environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith("COS_DESKTOP_") and not key.startswith("COS_RUNTIME_")
            and key not in {"COS_FIXTURE_TRACE", "COS_PREVIEW_TRACE", "COS_EXPORT_TRACE"}
        }
        environment["LOCALAPPDATA"] = str(self.localappdata)
        environment["COS_DESKTOP_RUNTIME_ISSUANCE_SECRET"] = self.secret
        environment["COS_DESKTOP_RUNTIME_DISCOVERY_PATH"] = str(self.discovery)
        self.localappdata.mkdir(parents=True, exist_ok=True)
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- launch {self.launches} {now_iso()} ---\n".encode())
        self.log_handle.flush()
        self.process = subprocess.Popen(  # noqa: S603 - fixed binary, no fixture flag
            [str(DESKTOP_EXE)], cwd=str(ROOT), env=environment,
            stdout=self.log_handle, stderr=subprocess.STDOUT,
        )
        self.ui = Ui(self.process.pid, self.work)
        return self.process.pid

    def alive(self) -> bool:
        return self.process is not None and base.pid_alive(self.process.pid)

    def wait_window(self, timeout: float = 60.0) -> dict:
        assert self.ui is not None
        return base.wait_window(self.process.pid, self.work / "uia-window.json", timeout)

    def wait_connected(self, timeout: float = 90.0) -> tuple[dict, str, bool]:
        assert self.ui is not None
        return self.ui.wait_names(
            lambda value: "タスク一覧" in value and "タスクの状態" in value, timeout, "connected"
        )

    def close(self) -> dict:
        if self.process is None:
            return {"closed": False, "reason": "not_started"}
        pid = self.process.pid
        sent = False
        if base.pid_alive(pid) and self.ui is not None:
            with contextlib.suppress(RuntimeError):
                sent = bool(self.ui.probe("close").get("sent"))
        try:
            code = self.process.wait(timeout=30)
            exited = True
        except subprocess.TimeoutExpired:
            code = None
            exited = False
        forced = None
        if not exited:
            forced = base.kill_tree(pid)
            exited = not base.pid_alive(pid)
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {"pid": pid, "wm_close_sent": sent, "exited": exited,
                "exit_code": code, "forced": forced}

    def force_stop(self) -> dict:
        if self.process is None:
            return {"stopped": False}
        result = base.kill_tree(self.process.pid) if base.pid_alive(self.process.pid) else {}
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return result

    def log_text(self) -> str:
        if not self.log_path.is_file():
            return ""
        return self.log_path.read_text(encoding="utf-8", errors="replace")


# --------------------------------------------------------------------------- Runtime box


class NativeRuntime(acc1.IsolatedRuntime):
    """Isolated Runtime on a caller-chosen 81xx port (8132 main / 8133 injections)."""

    def __init__(self, *, port: int, control_dir: Path | None = None, **kwargs):
        super().__init__(**kwargs)
        self.port = port
        self.endpoint = f"http://127.0.0.1:{port}"
        self.control_dir = control_dir

    def _environment(self) -> dict:
        environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith("COS_DESKTOP_") and not key.startswith("COS_RUNTIME_")
            and key != "ORCHESTRATOR_AUTH_DISABLED"
        }
        environment.update({
            "ORCHESTRATOR_AUTH_SECRET": self.secret,
            "ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET": self.handle_secret,
            "ORCHESTRATOR_COS_PRESETS": str(self.presets),
            "ORCHESTRATOR_COS_CAS_ROOT": str(self.cas_root),
            "ORCHESTRATOR_COS_WORK_ROOT": str(self.cos_work),
            "ORCHESTRATOR_COS_PROJECT_ROOT": str(RUNTIME_ROOT),
            "ORCHESTRATOR_RUNTIME_ROOT": str(RUNTIME_ROOT),
            "COS_RUNTIME_DISCOVERY_PATH": str(self.discovery_path),
            "COS_RUNTIME_ORIGIN": self.endpoint,
        })
        return environment

    def start(self, timeout: float = 150.0) -> dict:
        if port_open(self.port):
            raise RuntimeError(f"port {self.port} is already in use")
        for directory in (self.cas_root, self.cos_work, self.log_dir, self.output_root):
            directory.mkdir(parents=True, exist_ok=True)
        self._write_config()
        for stale in (self.stop_file, self.ready_file):
            with contextlib.suppress(FileNotFoundError):
                stale.unlink()
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- start {now_iso()} ---\n".encode())
        self.log_handle.flush()
        launcher = CONTROL_LAUNCHER if self.control_dir is not None else HARNESS / "isolated_runtime.py"
        argv = [str(RUNTIME_PYTHON), str(launcher), "--config", str(self.config_path),
                "--host", "127.0.0.1", "--port", str(self.port),
                "--stop-file", str(self.stop_file), "--ready-file", str(self.ready_file)]
        if self.control_dir is not None:
            argv += ["--control-dir", str(self.control_dir)]
        self.process = subprocess.Popen(  # noqa: S603 - fixed argv, loopback only
            argv, cwd=str(RUNTIME_ROOT), env=self._environment(),
            stdout=self.log_handle, stderr=subprocess.STDOUT,
        )
        started = time.monotonic()
        while True:
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"Runtime exited during startup (exit {self.process.returncode});"
                    f" log tail: {self.log_tail()}"
                )
            ready = self.http("GET", f"{self.endpoint}/api/ready", timeout=2.0)
            if ready["status"] == 200:
                break
            if time.monotonic() - started > timeout:
                raise RuntimeError(f"Runtime readiness timed out; log tail: {self.log_tail()}")
            time.sleep(0.25)
        hello = self.http("GET", f"{self.endpoint}/api/cos/desktop/server")
        if hello["status"] != 200 or not isinstance(hello["document"], dict):
            raise RuntimeError(f"HELLO failed ({hello['status']})")
        server = hello["document"]["server"]
        return {
            "pid": self.process.pid, "port": self.port, "endpoint": self.endpoint,
            "ready": ready, "hello": hello["document"],
            "server_identity": server["server_identity"], "server_build": server["server_build"],
            "epoch": server["epoch"], "protocol": server["protocol"], "schema": server["schema"],
            "config_path": str(self.config_path), "config_sha256": sha256_file(self.config_path),
            "discovery_path": str(self.discovery_path), "cas_root": str(self.cas_root),
            "log_dir": str(self.log_dir),
        }

    def http(self, method: str, url: str, *, body: object | None = None,
             headers: dict[str, str] | None = None, timeout: float = 30.0) -> dict:
        data = None
        request_headers = dict(headers or {})
        if body is not None:
            data = json.dumps(body).encode("utf-8")
            request_headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=data, method=method, headers=request_headers)
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                raw = response.read()
                try:
                    document = json.loads(raw.decode("utf-8")) if raw else None
                except (UnicodeDecodeError, json.JSONDecodeError):
                    document = {"raw_bytes": len(raw)}
                return {"status": response.status, "document": document}
        except urllib.error.HTTPError as error:
            raw = error.read()
            try:
                document = json.loads(raw.decode("utf-8")) if raw else None
            except (UnicodeDecodeError, json.JSONDecodeError):
                document = {"raw_bytes": len(raw)}
            return {"status": error.code, "document": document}
        except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
            return {"status": 0, "document": None, "error": type(error).__name__}

    def http_bytes(self, url: str, headers: dict[str, str], timeout: float = 60.0) -> dict:
        request = urllib.request.Request(url, method="GET", headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return {"status": response.status, "raw": response.read()}
        except urllib.error.HTTPError as error:
            raw = error.read()
            try:
                document = json.loads(raw.decode("utf-8")) if raw else None
            except (UnicodeDecodeError, json.JSONDecodeError):
                document = {"raw_bytes": len(raw)}
            return {"status": error.code, "raw": raw, "document": document}

    def status(self) -> dict:
        return self.http("GET", f"{self.endpoint}/api/status")

    def issue_credential(self) -> dict:
        return self.http("POST", f"{self.endpoint}/api/cos/desktop/credentials",
                         headers={"X-Auth-Secret": self.secret})

    def catalog(self, token: str) -> dict:
        return self.http("GET", f"{self.endpoint}/api/cos/desktop/catalog",
                         headers={"Authorization": f"Bearer {token}"})

    def open_observation(self, token: str, *, resume: dict[str, str] | None = None) -> dict:
        body: dict = {
            "schema_version": 1, "request_id": uuid.uuid4().hex,
            "protocol": dict(acc1.OBSERVATION_PROTOCOL), "schema": dict(acc1.OBSERVATION_SCHEMA),
            "scope": {"task_ids": [], "filters": [{"name": "view", "value": "all"}]},
            "retained_commands": [],
        }
        if resume is not None:
            body.update(resume)
        return self.http("POST", f"{self.endpoint}/api/cos/desktop/observation", body=body,
                         headers={"Authorization": f"Bearer {token}"})

    def deltas(self, token: str, cursor: str, context: str) -> dict:
        return self.http("GET", f"{self.endpoint}/api/cos/desktop/deltas?cursor={cursor}",
                         headers={"Authorization": f"Bearer {token}", "X-Cos-Context": context})

    def command(self, token: str, context: str, envelope: dict) -> dict:
        return self.http("POST", f"{self.endpoint}/api/cos/desktop/commands", body=envelope,
                         headers={"Authorization": f"Bearer {token}", "X-Cos-Context": context})

    def desktop_task(self, token: str, context: str, task_ref: str) -> dict:
        return self.http("GET", f"{self.endpoint}/api/cos/desktop/tasks/{task_ref}",
                         headers={"Authorization": f"Bearer {token}", "X-Cos-Context": context})

    def desktop_history(self, token: str, context: str, task_ref: str) -> dict:
        return self.http("GET", f"{self.endpoint}/api/cos/desktop/tasks/{task_ref}/history?limit=20",
                         headers={"Authorization": f"Bearer {token}", "X-Cos-Context": context})

    def media_descriptor(self, token: str, context: str, digest: str) -> dict:
        return self.http("GET", f"{self.endpoint}/api/cos/media/{digest}",
                         headers={"Authorization": f"Bearer {token}", "X-Cos-Context": context})

    def media_blob(self, token: str, access_handle: str) -> dict:
        path = access_handle
        if not path.startswith("/api/cos/media/"):
            path = f"/api/cos/media/{access_handle}/blob"
        handle = path.split("/api/cos/media/", 1)[1].rsplit("/blob", 1)[0]
        return self.http_bytes(f"{self.endpoint}{path}",
                               headers={"Authorization": f"Bearer {token}", "X-Media-Handle": handle})

    def stop(self) -> dict:
        result = super().stop()
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return result

    def log_tail(self, limit: int = 6000) -> str:
        with contextlib.suppress(OSError):
            return self.log_path.read_text(encoding="utf-8", errors="replace")[-limit:]
        return ""


# --------------------------------------------------------------------------- observation trace


class Trace:
    """Harness-side read:observe channel; records revisions and resyncs."""

    def __init__(self, runtime: NativeRuntime, token: str):
        self.runtime = runtime
        self.token = token
        self.context = ""
        self.cursor = ""
        self.scope_id = ""
        self.epoch = ""
        self.transactions: list[dict] = []
        self.resyncs: list[dict] = []
        self.errors: list[dict] = []
        self.timeline: list[dict] = []
        self._last_state: tuple | None = None

    def open(self) -> dict:
        response = self.runtime.open_observation(self.token)
        document = response.get("document") or {}
        grant = (document.get("grant") or {}).get("stamp") or {}
        snapshot = document.get("snapshot") or {}
        end = snapshot.get("end") or {}
        self.context = str(grant.get("context_id") or "")
        self.scope_id = str(grant.get("scope_id") or "")
        self.cursor = str(end.get("cursor") or "")
        self.epoch = str(grant.get("epoch") or "")
        return {
            "http_status": response["status"], "read_revision": grant.get("read_revision"),
            "epoch": self.epoch, "context_sha256_16": short_hash(self.context),
            "scope_sha256_16": short_hash(self.scope_id), "cursor_sha256_16": short_hash(self.cursor),
            "snapshot_mutations": len(acc1.snapshot_mutations(document)),
            "snapshot_kind_counts": acc1.count_kinds(acc1.snapshot_mutations(document)),
        }

    def observe_state(self, task_ref: str, mutations: list[dict]) -> dict | None:
        state = None
        for mutation in mutations:
            candidate = acc1.task_state(mutation)
            if candidate is not None and str(candidate.get("task_ref")) == task_ref:
                state = candidate
        if state is not None:
            key = (state.get("task_status"), state.get("inner_phase"), state.get("iteration_index"))
            if key != self._last_state:
                self._last_state = key
                self.timeline.append({"at": now_iso(), **state})
        return state

    def poll(self, task_ref: str) -> dict:
        if not self.context or not self.cursor:
            return {"error": "trace_not_open"}
        delta = self.runtime.deltas(self.token, self.cursor, self.context)
        document = delta.get("document") or {}
        if delta["status"] == 409 and document.get("code") == "RESYNC_REQUIRED":
            reopen = self.runtime.open_observation(self.token)
            fresh = reopen.get("document") or {}
            if reopen["status"] != 200:
                self.errors.append({"at": now_iso(), "kind": "resync_open_failed",
                                    "http_status": reopen["status"]})
                return {"error": "resync_open_failed"}
            grant = (fresh.get("grant") or {}).get("stamp") or {}
            end = (fresh.get("snapshot") or {}).get("end") or {}
            mutations = acc1.snapshot_mutations(fresh)
            self.resyncs.append({
                "at": now_iso(), "reason": "RESYNC_REQUIRED", "delta_error": document,
                "reopen_http_status": reopen["status"], "read_revision": grant.get("read_revision"),
                "snapshot_mutations": len(mutations),
                "snapshot_kind_counts": acc1.count_kinds(mutations),
            })
            self.context = str(grant.get("context_id") or "")
            self.cursor = str(end.get("cursor") or "")
            return {"resync": True, "state": self.observe_state(task_ref, mutations)}
        if delta["status"] == 0:
            self.errors.append({"at": now_iso(), "kind": "delta_transport", "error": document})
            return {"error": "transport"}
        if delta["status"] != 200:
            self.errors.append({"at": now_iso(), "kind": "delta_http",
                                "http_status": delta["status"], "document": document})
            return {"error": "http", "http_status": delta["status"]}
        transaction = document.get("transaction")
        if transaction is None:
            return {"transaction": None}
        mutations = transaction.get("mutations") or []
        entry = {
            "at": now_iso(), "transaction_id": transaction.get("transaction_id"),
            "mutation_count": transaction.get("mutation_count"),
            "integrity_digest": transaction.get("integrity_digest"),
            "kind_counts": acc1.count_kinds(mutations),
            "changes": [acc1.change_summary(mutation) for mutation in mutations],
        }
        self.transactions.append(entry)
        self.cursor = str(transaction.get("cursor") or self.cursor)
        return {"transaction": entry, "state": self.observe_state(task_ref, mutations),
                "mutations": mutations}


# --------------------------------------------------------------------------- run context


class Ctx:
    def __init__(self, label: str, port: int, *, control: bool = False,
                 source_run: Path | None = None):
        stamp = time.strftime("%Y%m%dT%H%M%S")
        self.label = label
        self.port = port
        self.work = WORK_ROOT / f"{stamp}-{label}"
        if self.work.exists():
            shutil.rmtree(self.work)
        self.work.mkdir(parents=True, exist_ok=True)
        self.secret = secrets.token_hex(24)
        self.handle_secret = secrets.token_hex(24)
        self.collect = [self.secret, self.handle_secret]
        self.source_run = Path(source_run) if source_run else None
        self.db_copy = self.work / "generation.sqlite"
        self.presets = self.work / "cos_presets.json"
        self.control_dir = self.work / "control" if control else None
        self.runtime = NativeRuntime(
            port=port, control_dir=self.control_dir, work=self.work, db_copy=self.db_copy,
            presets=self.presets, secret=self.secret, handle_secret=self.handle_secret,
        )
        self.app: NativeApp | None = None
        self.token = ""
        self._rail_filter: tuple[int, str] | None = None
        self.trace = Trace(self.runtime, "")
        self.record: dict = {
            "label": label, "started_at": now_iso(), "port": port, "verdict": "NOT_RUN",
            "work_dir": str(self.work),
            "steps": {}, "task": {}, "states": [], "projection_trace": [], "resyncs": [],
            "trace_errors": [], "screenshots": {}, "media": {}, "injections": {},
            "cleanup": {}, "residuals": [], "_secrets": [],
        }

    def start_runtime(self, *, wait_generation: bool = True) -> dict:
        if self.source_run is not None:
            # Bounded follow-up session: reuse the exact isolated DB copy and CAS
            # of a measured run (same task, same canonical artifact bytes) instead
            # of generating again. The serving epoch is new, so this session's
            # publication is its own; it is recorded as a split measurement.
            source_db = self.source_run / "generation.sqlite"
            if not source_db.is_file():
                raise RuntimeError(f"the source run has no DB copy: {source_db}")
            base.copy_database(source_db, self.db_copy)
            source_cas = self.source_run / "cas"
            if source_cas.is_dir():
                shutil.copytree(source_cas, self.work / "cas", dirs_exist_ok=True)
        else:
            base.copy_database(SOURCE_DB, self.db_copy)
        shutil.copyfile(SOURCE_PRESETS, self.presets)
        self.record["db_copy_sha256"] = sha256_file(self.db_copy)
        self.record["db_copy_bytes"] = self.db_copy.stat().st_size
        self.record["presets_sha256"] = sha256_file(self.presets)
        start = self.runtime.start()
        self.record["runtime"] = {
            "pid": start["pid"], "endpoint": start["endpoint"],
            "server_identity": start["server_identity"], "server_build": start["server_build"],
            "epoch": start["epoch"], "protocol": start["protocol"], "schema": start["schema"],
            "config_path": start["config_path"], "config_sha256": start["config_sha256"],
            "discovery_path": start["discovery_path"], "cas_root": start["cas_root"],
            "ready": start["ready"],
        }
        statuses: list[dict] = []
        deadline = time.monotonic() + (240.0 if wait_generation else 0.0)
        health: dict = {}
        while True:
            response = self.runtime.status()
            document = response.get("document") or {}
            health = document.get("health") or {}
            statuses.append({"at": now_iso(), "http_status": response["status"],
                             "health": health, "loop": document.get("loop")})
            if not wait_generation or (health.get("llm") is True and health.get("comfy") is True):
                break
            if time.monotonic() >= deadline:
                break
            time.sleep(3.0)
        self.record["health"] = {
            "polls": len(statuses), "transitions": statuses[:2] + statuses[-2:],
            "runtime_reported": health,
            "generation_plane_ready": health.get("llm") is True and health.get("comfy") is True,
            "generation_plane_required": wait_generation,
            "direct_preflight": acc1.probe_generation_plane(),
        }
        credential = self.runtime.issue_credential()
        document = credential.get("document") or {}
        self.token = str(document.get("token") or "")
        if credential["status"] != 200 or not self.token:
            raise RuntimeError(f"credential issuance failed ({credential['status']})")
        self.collect.append(self.token)
        catalog_response = self.runtime.catalog(self.token)
        catalog_document = catalog_response.get("document") or {}
        catalog = catalog_document.get("catalog") or {}
        recipes = catalog.get("recipes") or []
        self.record["catalog"] = {
            "http_status": catalog_response["status"], "can_submit": catalog.get("can_submit"),
            "reason_codes": catalog.get("reason_codes"),
            "recipes": [{"label": recipe.get("label"), "recipe_ref": recipe.get("recipe_ref"),
                         "execution_limits": recipe.get("execution_limits")} for recipe in recipes],
        }
        if not recipes:
            raise RuntimeError("the authenticated catalog carries no recipe")
        self.trace = Trace(self.runtime, self.token)
        self.record["observation_open"] = self.trace.open()
        if self.record["observation_open"]["http_status"] != 200:
            raise RuntimeError("initial observation was refused")
        self.record["credential"] = {
            "principal": document.get("principal"), "scopes": document.get("scopes"),
            "expires_in": document.get("expires_in"), "token_recorded": False,
        }
        return self.record["runtime"]

    def seed_capabilities(self, task_ref: str) -> dict:
        """Seed one authoritative capability set and prove it publishes.

        The set is test-only material for the isolated copy: the Runtime still
        derives command/media support itself (RC-11), and the Desktop still
        fails closed when the projection is absent.
        """
        step: dict = {"at": now_iso(), "seeded": False, "task_ref": task_ref}
        try:
            step.update(seed_task_capability(self.db_copy, task_ref))
            step["seeded"] = True
        except Exception as error:  # noqa: BLE001 - record, never fake a projection
            step["error"] = f"{type(error).__name__}: {error}"
            self.record["capability_seed"] = step
            return step
        observed: dict = {}
        deadline = time.monotonic() + 45.0
        while True:
            response = self.runtime.open_observation(self.token)
            document = response.get("document") or {}
            capabilities = None
            task_projection = None
            for mutation in acc1.snapshot_mutations(document):
                projection = mutation.get("projection") or {}
                kind = projection.get("projection")
                if kind == "CAPABILITIES":
                    scope = projection.get("scope") or {}
                    if str(scope.get("task_id") or "") == task_ref:
                        capabilities = projection
                elif kind == "TASK" and str(projection.get("task_ref") or "") == task_ref:
                    task_projection = projection
            if capabilities is not None:
                observed = {
                    "at": now_iso(),
                    "http_status": response["status"],
                    "capabilities": capabilities,
                    "task_capabilities_ref": (task_projection or {}).get("capabilities_ref"),
                    "capabilities_ref_matches_task": (
                        (task_projection or {}).get("capabilities_ref")
                        == capabilities.get("capabilities_ref")),
                }
                break
            if time.monotonic() >= deadline:
                break
            time.sleep(1.5)
        step["observed"] = observed
        step["projection_visible"] = bool(observed)
        if not observed:
            step["reason"] = "the CAPABILITIES projection did not become visible in 45 s"
        self.record["capability_seed"] = step
        return step

    def list_desktop_principals(self) -> dict:
        """Read the serving credential registry's principals (test control)."""
        if self.control_dir is None:
            return {"available": False, "reason": "this run has no control channel"}
        trigger = self.control_dir / "list-desktop-credentials"
        ack = self.control_dir / "list-desktop-credentials.ack.json"
        with contextlib.suppress(FileNotFoundError):
            ack.unlink()
        trigger.write_text("list", encoding="utf-8")
        deadline = time.monotonic() + 20.0
        while time.monotonic() < deadline:
            if ack.is_file():
                document = json.loads(ack.read_text(encoding="utf-8"))
                document["available"] = True
                document["registry_order"] = "issuance order; the app credential is issued last"
                return document
            time.sleep(0.2)
        return {"available": False, "reason": "the control ack did not appear"}

    def rebind_task_owner(self, task_ref: str, principal: str) -> dict:
        """Test-only owner rebind in the isolated copy (same-operator stop)."""
        connection = sqlite3.connect(str(self.db_copy), timeout=15)
        try:
            before_row = connection.execute(
                "SELECT created_by FROM creative_tasks WHERE task_id = ?", (task_ref,)
            ).fetchone()
            before = before_row[0] if before_row else None
            connection.execute(
                "UPDATE creative_tasks SET created_by = ? WHERE task_id = ?",
                (principal, task_ref))
            connection.commit()
            after_row = connection.execute(
                "SELECT created_by FROM creative_tasks WHERE task_id = ?", (task_ref,)
            ).fetchone()
            return {"before": before, "after": after_row[0] if after_row else None,
                    "principal": principal, "test_only_rebind": True}
        finally:
            connection.close()

    def wait_capability_entry(self, task_ref: str, group: str, name: str,
                              predicate, timeout: float = 60.0) -> dict:
        """Wait until the projected entry satisfies ``predicate`` (harness read)."""
        deadline = time.monotonic() + timeout
        last: dict = {}
        while True:
            response = self.runtime.open_observation(self.token)
            document = response.get("document") or {}
            entries: list[dict] = []
            for mutation in acc1.snapshot_mutations(document):
                projection = mutation.get("projection") or {}
                if projection.get("projection") != "CAPABILITIES":
                    continue
                if str((projection.get("scope") or {}).get("task_id") or "") != task_ref:
                    continue
                entries = list(projection.get(group) or [])
                last = {"at": now_iso(), "http_status": response["status"],
                        "entries": entries,
                        "availability": projection.get("availability"),
                        "capabilities_ref": projection.get("capabilities_ref")}
                entry = next((item for item in entries if str(item.get("operation")
                               or item.get("representation") or "") == name), None)
                if entry is not None and predicate(str(entry.get("support") or "")):
                    last["matched"] = True
                    last["entry"] = entry
                    return last
            last.setdefault("matched", False)
            if time.monotonic() >= deadline:
                return last
            time.sleep(1.5)

    def launch_desktop(self, label: str = "main") -> dict:
        base.cleanup_credential_store(self.runtime.endpoint)
        self.app = NativeApp(label=label, work=self.work, secret=self.secret,
                             discovery=self.runtime.discovery_path)
        pid = self.app.launch()
        window = self.app.wait_window(60.0)
        dump, text, connected = self.app.wait_connected(90.0)
        snapshot = uia_snapshot(self.app.ui, f"connected-{label}")
        self.record["steps"][f"connected-{label}"] = {
            "verdict": "PASS" if (window.get("found") and connected) else "FAIL",
            "pid": pid,
            "window": {"found": window.get("found"), "title": window.get("title"),
                       "class": window.get("class"), "width": window.get("width"),
                       "height": window.get("height")},
            "uia": snapshot,
            "excerpt": bounded("\n".join(
                line for line in text.splitlines()
                if "タスク" in line or "Runtime" in line or "接続" in line), 2000),
        }
        if not connected:
            raise RuntimeError("the native window did not render the connected task rail")
        return self.record["steps"][f"connected-{label}"]

    def create_via_brief(self, goal: str, direction: str, *,
                         receipt_only: bool = False) -> dict:
        assert self.app is not None and self.app.ui is not None
        ui = self.app.ui
        step: dict = {"at": now_iso(), "goal": goal, "direction": direction, "actions": []}
        opened = ui.invoke("新しいタスクを作成")
        step["actions"].append({"action": "invoke_brief_open", "result": opened})
        _, _, brief_open = ui.wait_contains("クリエイティブ・ブリーフ", 20.0, "brief")
        step["brief_open"] = brief_open
        step["uia_brief"] = uia_snapshot(ui, "brief-open")
        if not brief_open:
            step.update(verdict="FAIL", error="Brief surface did not open through its UIA button")
            self.record["steps"]["brief"] = step
            return step
        click_goal = ui.click("目的")
        type_goal = ui.type_text("目的", goal)
        read_goal = ui.valueread("目的")
        click_direction = ui.click("方向性")
        type_direction = ui.type_text("方向性", direction)
        read_direction = ui.valueread("方向性")
        step["field_entry"] = {
            "click_goal": click_goal, "type_goal": type_goal, "read_goal": read_goal,
            "click_direction": click_direction, "type_direction": type_direction,
            "read_direction": read_direction,
        }
        step["goal_value_after"] = read_goal.get("value")
        step["direction_value_after"] = read_direction.get("value")
        recipe = ui.invoke(RECIPE_LABEL)
        recipe_method = "uia_invoke"
        if not recipe.get("invoked"):
            recipe = ui.click(RECIPE_LABEL)
            recipe_method = "synthetic_wm_lbutton_at_uia_bounds"
        step["recipe"] = {"method": recipe_method, "result": recipe}
        names_before = list(ui.dump().get("names") or [])
        enabled = "ブリーフを提出" in names_before
        step["submit_enabled"] = enabled
        step["submit_disabled_label_present"] = "ブリーフを提出（無効）" in names_before
        step["uia_filled"] = uia_snapshot(ui, "brief-filled")
        if not enabled:
            step.update(verdict="FAIL", error="submit stayed disabled after the native form entry")
            self.record["steps"]["brief"] = step
            return step
        submit = ui.invoke("ブリーフを提出")
        step["submit_method"] = "uia_invoke"
        if not submit.get("invoked"):
            submit = ui.click("ブリーフを提出")
            step["submit_method"] = "synthetic_wm_lbutton_at_uia_bounds"
        step["submit"] = submit
        task_ref = ""
        source = ""
        receipt_seen = False
        receipt_at = ""
        deadline = time.monotonic() + (30.0 if receipt_only else 90.0)
        next_projection = 0.0
        while time.monotonic() < deadline and not task_ref:
            dump = ui.dump()
            text = "\n".join(dump.get("names") or [])
            match = re.search(r"受付タスク: (task-[0-9a-f]{32})", text)
            if match:
                task_ref = match.group(1)
                source = "receipt"
                receipt_seen = True
                receipt_at = now_iso()
                break
            if not receipt_only and time.monotonic() >= next_projection:
                task_ref = self.projection_task_ref(goal)
                if task_ref:
                    source = "projection"
                    break
                next_projection = time.monotonic() + 2.5
            time.sleep(0.75)
        step["task_ref"] = task_ref
        step["receipt_visible"] = receipt_seen
        step["receipt_at"] = receipt_at
        step["task_ref_source"] = source
        step["uia_submitted"] = uia_snapshot(ui, "brief-submitted")
        if receipt_seen:
            # Capture the accepted-receipt surface itself before anything else
            # moves: UIA names plus a PrintWindow screenshot.
            step["uia_receipt"] = uia_snapshot(ui, "receipt-accepted")
        step["verdict"] = "PASS" if task_ref else "PARTIAL"
        if not task_ref:
            step["error"] = ("neither an accepted receipt nor a live TASK projection"
                             " surfaced the created task")
        self.record["steps"]["brief"] = step
        return step

    def projection_task_ref(self, goal: str) -> str:
        response = self.runtime.open_observation(self.token)
        document = response.get("document") or {}
        if response["status"] != 200:
            return ""
        for mutation in acc1.snapshot_mutations(document):
            projection = mutation.get("projection") or {}
            if projection.get("projection") != "TASK":
                continue
            if str(projection.get("title") or "") != goal:
                continue
            self.record["task"]["task_ref"] = str(projection.get("task_ref") or "")
            self.record["task"]["projection_at_discovery"] = {
                "task_status": projection.get("task_status"),
                "inner_phase": projection.get("inner_phase"),
                "projection_revision": projection.get("projection_revision"),
            }
            return self.record["task"]["task_ref"]
        return ""

    def find_task_ref_in_projection(self, goal: str) -> str:
        for _ in range(8):
            task_ref = self.projection_task_ref(goal)
            if task_ref:
                return task_ref
            time.sleep(2.5)
        return ""

    def filter_rail(self, needle: str) -> dict:
        assert self.app is not None and self.app.ui is not None
        ui = self.app.ui
        current = ui.dump()
        names = list(current.get("names") or [])
        # The query is local UI state: typing it a second time would append and
        # never match. If this process already received the query, only wait.
        if self._rail_filter == (ui.pid, needle):
            dump, text, visible = ui.wait_contains(needle, 30.0, "rail-filtered")
            return {"click": {"skipped": "already_typed"}, "typed": {"skipped": "already_typed"},
                    "visible": visible, "uia_sha256": dump.get("_names_sha256"),
                    "excerpt": bounded("\n".join(
                        line for line in text.splitlines() if needle in line), 1500)}
        if any(needle in line for line in names):
            return {"click": {"skipped": "already_filtered"}, "typed": {"skipped": "already_filtered"},
                    "visible": True, "uia_sha256": current.get("_names_sha256"),
                    "excerpt": bounded("\n".join(
                        line for line in names if needle in line), 1500)}
        click = ui.click("タスクを検索")
        typed = ui.type_text("タスクを検索", needle)
        self._rail_filter = (ui.pid, needle)
        dump, text, visible = ui.wait_contains(needle, 30.0, "rail-filtered")
        return {"click": click, "typed": typed, "visible": visible,
                "uia_sha256": dump.get("_names_sha256"),
                "excerpt": bounded("\n".join(
                    line for line in text.splitlines() if needle in line), 1500)}

    def select_task(self, goal: str, label: str, task_ref: str = "") -> dict:
        assert self.app is not None and self.app.ui is not None
        ui = self.app.ui
        filtered = self.filter_rail(goal)
        invoked = ui.invoke(goal)
        method = "uia_invoke"
        if not invoked.get("invoked"):
            # The rail row is not a Button (no Invoke/Selection pattern); select
            # it with a synthetic pointer click inside its UIA bounds.
            invoked = ui.click(goal)
            method = "synthetic_wm_lbutton_at_rail_row"

        def selected_predicate(value: str) -> bool:
            if task_ref:
                return f"目的 — 利用可能: {task_ref}" in value
            return "タスクの状態:" in value and goal in value

        dump, text, selected = ui.wait_names(selected_predicate, 20.0, "task-selected")
        return {"filter": filtered, "invoke": invoked, "method": method,
                "selected": selected, "scope_task_matched": selected and bool(task_ref),
                "uia_sha256": dump.get("_names_sha256"),
                "uia": uia_snapshot(ui, label), "excerpt": bounded("\n".join(
                    line for line in text.splitlines()
                    if "タスクの状態" in line or "処理段階" in line or "成果物" in line
                    or "目的 — " in line or goal in line), 2000)}

    def view_candidate(self, task_ref: str) -> dict:
        """Make this task's candidate the viewed candidate through its dock row."""
        assert self.app is not None and self.app.ui is not None
        ui = self.app.ui
        names = list(ui.dump().get("names") or [])
        label = next((line for line in names if task_ref in line and "第0回" in line), "")
        if label and "閲覧中" in label:
            invoked: dict = {"already_viewed": True}
            method = "already_viewed"
        elif label:
            invoked = ui.invoke(f"{task_ref} — 第0回")
            method = "mounted_row_invoke"
        else:
            anchor = next((line for line in names if "第0回 — C1" in line), "")
            focus_row = ui.invoke(anchor) if anchor else {"invoked": False}
            time.sleep(1.0)
            ui.probe("key", text="end")
            invoked = {"focus_row": focus_row.get("invoked"), "reached": False}
            method = "dock_end_left_walk"
            for _ in range(120):
                current = list(ui.dump().get("names") or [])
                match = next((line for line in current
                              if task_ref in line and "第" in line), "")
                if match:
                    invoked = ui.invoke(f"{task_ref} — 第0回")
                    invoked["focus_row"] = focus_row.get("invoked")
                    invoked["reached"] = True
                    method = "dock_end_left_mounted_row_invoke"
                    break
                ui.probe("key", text="left")
                time.sleep(0.35)
        dump, text, viewed = ui.wait_names(
            lambda value: any(task_ref in line and "閲覧中" in line
                              for line in value.splitlines()), 20.0, "candidate-viewed")
        # Force one surface transition: the media binding is (re)triggered from
        # the Artifact surface for a changed viewed identity, and navigating back
        # guarantees that path runs for this candidate.
        surface_switch = ui.invoke("処理の流れ  Ctrl+3")
        time.sleep(0.7)
        back = ui.invoke("成果物  Ctrl+1")
        if not back.get("invoked"):
            back = ui.click("成果物  Ctrl+1")
        time.sleep(1.0)
        return {"method": method, "invoke": invoked, "viewed": viewed,
                "surface_switch": surface_switch.get("invoked"),
                "back_to_artifact": back.get("invoked"),
                "label": next((line for line in (dump.get("names") or [])
                               if task_ref in line and "第0回" in line), ""),
                "names_sha256": dump.get("_names_sha256")}

    def view_artifact(self, task_ref: str, label: str) -> dict:
        assert self.app is not None and self.app.ui is not None
        ui = self.app.ui
        before = uia_snapshot(ui, f"{label}-before")
        invoked = self.view_candidate(task_ref)
        method = invoked.get("method", "")
        deadline = time.monotonic() + 120.0
        state = "PENDING"
        dump: dict = {"found": False}
        while time.monotonic() < deadline:
            dump = ui.dump()
            text = "\n".join(dump.get("names") or [])
            if MEDIA_DISPLAYED_MARKER in text:
                state = "DISPLAYED"
                break
            if MEDIA_AVAILABLE in text:
                state = "AVAILABLE"
                break
            if MEDIA_MISSING in text:
                state = "MISSING"
                break
            if MEDIA_FAILED in text:
                state = "FAILED"
                break
            if CAPABILITY_BLOCKED_MARKER in text:
                state = "BLOCKED_CAPABILITY"
                break
            if any(marker in text for marker in MEDIA_PREPARING_MARKERS):
                state = "PREPARING"
            elif MEDIA_UNAVAILABLE_MARKER in text:
                state = "UNAVAILABLE"
            time.sleep(1.5)
        after = uia_snapshot(ui, label)
        final_text = "\n".join(dump.get("names") or [])
        return {"invoke": invoked, "method": method, "state": state,
                "inspector_task_match": re.search(
                    rf"目的 — 利用可能: {re.escape(task_ref)}", final_text) is not None,
                "viewport_present": any("拡大縮小" in line or "物理等倍" in line
                                        for line in (dump.get("names") or [])),
                "media_lines": [line for line in (dump.get("names") or [])
                                if "画像" in line or "メディア" in line or "候補" in line
                                or "機能の利用可否" in line][:40],
                "uia_before": before, "uia_after": after}

    def projection_task(self, task_ref: str) -> dict:
        response = self.runtime.open_observation(self.token)
        document = response.get("document") or {}
        for mutation in acc1.snapshot_mutations(document):
            projection = mutation.get("projection") or {}
            if (projection.get("projection") == "TASK"
                    and str(projection.get("task_ref") or "") == task_ref):
                return projection
        return {}

    def resolve_artifact(self, task_ref: str) -> dict:
        response = self.runtime.open_observation(self.token)
        document = response.get("document") or {}
        grant = (document.get("grant") or {}).get("stamp") or {}
        context = str(grant.get("context_id") or "")
        mutations = acc1.snapshot_mutations(document)
        outcome = acc1.find_outcome(mutations, task_ref)
        artifact = acc1.find_task_artifact(mutations, task_ref, outcome)
        if artifact is None:
            return {"found": False, "http_status": response["status"]}
        digest = str(artifact.get("artifact_digest") or "")
        descriptor = self.runtime.media_descriptor(self.token, context, digest)
        return {
            "found": True, "context_sha256_16": short_hash(context),
            "snapshot_kind_counts": acc1.count_kinds(mutations),
            "capabilities_projection_present": any(
                (mutation.get("projection") or {}).get("projection") == "CAPABILITIES"
                for mutation in mutations),
            "artifact": {"artifact_ref": artifact.get("artifact_ref"),
                         "artifact_digest": digest, "media_type": artifact.get("media_type"),
                         "availability": artifact.get("availability"),
                         "task_ref": artifact.get("task_ref")},
            "outcome": {"formal_status": (outcome or {}).get("formal_status"),
                        "stop_reason": (outcome or {}).get("stop_reason"),
                        "selected_refs": (outcome or {}).get("selected_refs"),
                        "projection_revision": (outcome or {}).get("projection_revision")},
            "descriptor_http_status": descriptor["status"],
            "descriptor": descriptor.get("document"),
        }

    def fetch_artifact(self, resolved: dict, destination: Path) -> dict:
        if not resolved.get("found"):
            return {"fetched": False, "reason": "artifact_not_found"}
        descriptor = resolved.get("descriptor") or {}
        if resolved.get("descriptor_http_status") != 200:
            return {"fetched": False, "reason": "descriptor_refused",
                    "status": resolved.get("descriptor_http_status")}
        handle = descriptor.get("access_handle")
        if not handle:
            return {"fetched": False, "reason": "no_access_handle"}
        blob = self.runtime.media_blob(self.token, str(handle))
        if blob["status"] != 200 or not blob.get("raw"):
            return {"fetched": False, "reason": "blob_refused", "status": blob["status"],
                    "document": blob.get("document")}
        raw = blob["raw"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(raw)
        expected = resolved.get("artifact", {}).get("artifact_digest")
        return {"fetched": True, "bytes": len(raw), "sha256": sha256_bytes(raw),
                "digest_matches_descriptor": sha256_bytes(raw) == expected,
                "png": acc1.png_dimensions(raw), "file": str(destination)}

    def cleanup(self, *, stop_runtime: bool = True) -> dict:
        app_close = self.app.close() if self.app is not None else {"closed": False}
        runtime_stop = self.runtime.stop() if stop_runtime else {"stopped": False}
        self.record["cleanup"] = {
            "desktop": app_close, "runtime": runtime_stop, "port": self.port,
            "port_free": wait_port_free(self.port, 30.0),
            "credential_store": base.cleanup_credential_store(self.runtime.endpoint),
            "discovery_present_after_stop": self.runtime.discovery_path.exists(),
        }
        return self.record["cleanup"]


# --------------------------------------------------------------------------- shared helpers


def wait_task_state(ctx: Ctx, task_ref: str, predicate, timeout: float,
                    context: str | None = None) -> dict:
    context_id = context or ""
    if not context_id:
        response = ctx.runtime.open_observation(ctx.token)
        document = response.get("document") or {}
        context_id = str(((document.get("grant") or {}).get("stamp") or {}).get("context_id") or "")
    deadline = time.monotonic() + timeout
    last: dict = {}
    while time.monotonic() < deadline:
        response = ctx.runtime.desktop_task(ctx.token, context_id, task_ref)
        task = (response.get("document") or {}).get("task") or {}
        last = {"at": now_iso(), "http_status": response["status"], "task": task}
        if response["status"] == 200 and predicate(task):
            return {"matched": True, "context_sha256_16": short_hash(context_id), **last}
        time.sleep(2.0)
    return {"matched": False, "context_sha256_16": short_hash(context_id), **last}


def terminal(task: dict) -> bool:
    return str(task.get("task_status") or "") in {"COMPLETED", "CANCELLED", "FAILED"}


def redact_descriptor(resolved: dict) -> dict:
    """Descriptor copy safe to retain: capabilities are never evidence."""
    copy = json.loads(json.dumps(resolved, ensure_ascii=False))
    descriptor = copy.get("descriptor")
    if isinstance(descriptor, dict):
        for key in ("access_handle", "original_url", "thumbnail_url"):
            if key in descriptor:
                descriptor[key] = "<redacted-media-capability>"
    return copy


def restart_desktop(ctx: Ctx, goal: str, task_ref: str) -> dict:
    assert ctx.app is not None
    step: dict = {"at": now_iso()}
    close = ctx.app.close()
    step["close"] = close
    ready = ctx.runtime.http("GET", f"{ctx.runtime.endpoint}/api/ready", timeout=5.0)
    step["runtime_ready_after_close"] = ready["status"] == 200
    canonical = ctx.runtime.desktop_task(ctx.token, ctx.trace.context, task_ref)
    step["task_after_close"] = (canonical.get("document") or {}).get("task")
    time.sleep(2.0)
    launch = NativeApp(label="relaunch", work=ctx.work, secret=ctx.secret,
                       discovery=ctx.runtime.discovery_path)
    pid = launch.launch()
    ctx.app = launch
    window = launch.wait_window(60.0)
    _, _, connected = launch.wait_connected(90.0)
    step["relaunch"] = {"pid": pid, "window": {"found": window.get("found"),
                                               "title": window.get("title")},
                        "connected": connected}
    if connected:
        filtered = ctx.filter_rail(goal)
        _, selected_text, visible = launch.ui.wait_names(
            lambda value: "タスクの状態:" in value and goal in value, 30.0, "relaunch-task")
        step["rail_filter"] = filtered
        step["task_visible_after_relaunch"] = visible
        step["uia"] = uia_snapshot(launch.ui, "task-relaunched")
        step["screenshot"] = step["uia"]["screenshot"]
        step["excerpt"] = bounded("\n".join(
            line for line in selected_text.splitlines()
            if "タスクの状態" in line or "処理段階" in line or goal in line or "接続" in line), 2000)
    log = ctx.app.log_text()
    step["app_log_excerpt"] = bounded("\n".join(
        line for line in log.splitlines()
        if "Runtime discovered" in line or "credential" in line or "observation" in line.lower()), 1500)
    step["verdict"] = ("PASS" if close.get("exited") and connected
                       and step.get("task_visible_after_relaunch") else "PARTIAL")
    return step


# --------------------------------------------------------------------------- scenarios


def _lines(text: str, needles: tuple[str, ...], limit: int = 2200) -> str:
    return bounded("\n".join(line for line in text.splitlines()
                            if any(needle in line for needle in needles)), limit)


def _surface_names(ui: Ui, contains: tuple[str, ...], limit: int = 40) -> list[str]:
    text = "\n".join(ui.dump().get("names") or [])
    return [line for line in text.splitlines()
            if any(needle in line for needle in contains)][:limit]


def native_surface_interactions(ctx: Ctx, task_ref: str, goal: str) -> dict:
    """Exercise History/Evidence/Flow/Compare through the native UIA surface.

    Every branch records what the real window exposes. A surface that is absent
    or refused records the exact label instead of an invented state.
    """
    ui = ctx.app.ui
    assert ui is not None
    result: dict = {"at": now_iso(), "task_ref": task_ref}
    # The dock navigation (and any other surface work) can move the rail
    # selection; re-select this task so every surface below is about it.
    reselect = ctx.select_task(goal, "surfaces-task-selected", task_ref)
    result["task_reselected"] = {"selected": reselect.get("selected"),
                                 "uia_sha256": reselect.get("uia_sha256"),
                                 "excerpt": reselect.get("excerpt")}
    result["viewed_candidate"] = ctx.view_candidate(task_ref)
    before = uia_snapshot(ui, "surfaces-before")

    # --- History: open, load a row, close, reopen ---------------------------
    history: dict = {"steps": []}
    try:
        opened = ui.invoke("このタスクの実行履歴を開く")
        method = "uia_invoke_inspector_action"
        if not opened.get("invoked"):
            opened = ui.invoke("履歴を開く（Ctrl+H）")
            method = "uia_invoke_button_text"
        dump, text, visible = ui.wait_names(
            lambda value: "実行履歴" in value or "履歴 — " in value, 25.0, "history-open")
        history.update({"open_method": method, "open_result": opened, "opened": visible,
                        "names_sha256": dump.get("_names_sha256"),
                        "excerpt": _lines(text, ("履歴", "反復", "状態")),
                        "uia": uia_snapshot(ui, "history-open")})
        if visible:
            _, loaded_text, loaded = ui.wait_names(
                lambda value: "履歴を読み込みました" in value or "第0回" in value
                or "実行履歴。" in value, 20.0, "history-loaded")
            history["loaded"] = loaded
            history["loaded_excerpt"] = _lines(loaded_text, ("履歴", "第", "状態", "表示"))
            enter = ui.probe("key", text="enter")
            time.sleep(1.2)
            _, reopened_text, reopened_loaded = ui.wait_names(
                lambda value: "履歴" in value, 10.0, "history-row-activated")
            history["enter"] = enter
            history["row_activated"] = reopened_loaded
            history["row_excerpt"] = _lines(reopened_text, ("履歴", "第", "表示", "復元"))
            close = ui.invoke("履歴を閉じる")
            time.sleep(1.0)
            reopened = ui.invoke("このタスクの実行履歴を開く")
            if not reopened.get("invoked"):
                reopened = ui.invoke("履歴を開く（Ctrl+H）")
            dump2, text2, visible2 = ui.wait_names(
                lambda value: "実行履歴" in value or "履歴 — " in value, 20.0, "history-reopened")
            history["close"] = close
            history["reopen"] = {"invoked": reopened.get("invoked"), "visible": visible2,
                                 "names_sha256": dump2.get("_names_sha256"),
                                 "excerpt": _lines(text2, ("履歴", "第", "状態"))}
            history["reopen_uia"] = uia_snapshot(ui, "history-reopened")
            ui.invoke("履歴を閉じる")
            time.sleep(0.8)
        history["verdict"] = ("PASS" if history.get("opened")
                              and (history.get("reopen") or {}).get("visible")
                              else ("PARTIAL" if history.get("opened") else "NOT_RUN"))
    except Exception as error:  # noqa: BLE001 - record the surface failure
        history.update(verdict="NOT_RUN", error=f"{type(error).__name__}: {error}")
    result["history"] = history

    # --- Evidence: expand the first projected evidence group ----------------
    evidence: dict = {}
    try:
        names = list(ui.dump().get("names") or [])
        groups = [line for line in names if "根拠は" in line and "件です" in line]
        evidence["groups_visible"] = len(groups)
        evidence["group_labels"] = groups[:8]
        evidence["inspector_evidence_lines"] = [
            line for line in names
            if "判断の根拠" in line or "改善" in line or "証拠" in line][:12]
        if not groups:
            evidence["not_projected_lines"] = [
                line for line in names
                if "根拠" in line or "投影されていません" in line or "利用不可" in line][:8]
            explicit_reason = [line for line in names
                               if "判断の根拠" in line or "根拠の詳細情報は投影されていません" in line]
            evidence["verdict"] = "PARTIAL" if explicit_reason else "NOT_RUN"
            evidence["reason"] = (
                "no evidence group is exposed in the native UIA tree"
                + ("; the inspector states an explicit unavailable/not-projected reason"
                   if explicit_reason else ""))
        else:
            expanded = ui.invoke(groups[0])
            method = "uia_invoke_group_header"
            if not expanded.get("invoked"):
                expanded = ui.click(groups[0])
                method = "synthetic_wm_lbutton_at_uia_bounds"
            dump, text, visible = ui.wait_names(
                lambda value: "— " in value or "根拠" in value, 12.0, "evidence-expanded")
            evidence.update({
                "expand_method": method, "expand_result": expanded,
                "expanded": visible, "names_sha256": dump.get("_names_sha256"),
                "excerpt": _lines(text, ("根拠", "状態", "見つかりません", "利用可能",
                                         "利用不可", "投影", "評価", "証拠")),
                "uia": uia_snapshot(ui, "evidence-expanded"),
            })
            detail_rows = [line for line in (dump.get("names") or [])
                           if "—" in line and ("DIGEST" in line or "状態" in line
                                              or "PASS" in line or "SUCCESS" in line)]
            evidence["detail_rows"] = detail_rows[:12]
            evidence["verdict"] = "PASS" if visible else "PARTIAL"
    except Exception as error:  # noqa: BLE001
        evidence.update(verdict="NOT_RUN", error=f"{type(error).__name__}: {error}")
    result["evidence"] = evidence

    # --- Flow: surface switch, node/edge lists ------------------------------
    flow: dict = {}
    try:
        switched = ui.invoke("処理の流れ  Ctrl+3")
        method = "uia_invoke_surface_switch"
        if not switched.get("invoked"):
            switched = ui.click("処理の流れ  Ctrl+3")
            method = "synthetic_wm_lbutton_at_uia_bounds"
        dump, text, visible = ui.wait_names(
            lambda value: "処理の一覧" in value or "接続関係" in value
            or "処理の流れ" in value, 20.0, "flow-open")
        nodes = [line for line in (dump.get("names") or [])
                 if "ノード" in line or "処理" in line][:12]
        edges = [line for line in (dump.get("names") or [])
                 if "接続" in line or "エッジ" in line][:12]
        flow.update({"switch_method": method, "switch_result": switched,
                     "visible": visible, "names_sha256": dump.get("_names_sha256"),
                     "nodes": nodes, "edges": edges,
                     "excerpt": _lines(text, ("処理", "接続", "ノード", "流れ", "検索")),
                     "uia": uia_snapshot(ui, "flow-surface")})
        flow["verdict"] = ("PASS" if visible and (nodes or edges)
                           else ("PARTIAL" if visible else "NOT_RUN"))
        back = ui.invoke("成果物  Ctrl+1")
        if not back.get("invoked"):
            back = ui.click("成果物  Ctrl+1")
        flow["back_to_artifact"] = back
    except Exception as error:  # noqa: BLE001
        flow.update(verdict="NOT_RUN", error=f"{type(error).__name__}: {error}")
    result["flow"] = flow

    # --- Compare: pair selection through the dock ---------------------------
    compare: dict = {}
    try:
        names = list(ui.dump().get("names") or [])
        compare["switcher_before"] = [
            line for line in names if "候補を比較" in line][:4]
        compare["pair_required_label"] = any(
            "候補を2つ選ぶ必要があります" in line for line in names)
        rows = [line for line in names if "第0回" in line]
        compare["dock_rows_visible"] = len(rows)
        compare["dock_rows_sample"] = rows[:6]
        own_row = next((line for line in rows if task_ref in line), "")
        other_row = next((line for line in rows if task_ref not in line), "")
        selections: list[dict] = []
        for label in (own_row, other_row):
            if not label:
                continue
            result_invoke = ui.invoke(label)
            method = "uia_invoke_dock_row"
            if not result_invoke.get("invoked"):
                result_invoke = ui.click(label)
                method = "synthetic_wm_lbutton_at_uia_bounds"
            selections.append({"label": label, "method": method,
                               "result": result_invoke})
            time.sleep(1.5)
        compare["selections"] = selections
        names = list(ui.dump().get("names") or [])
        compare["switcher_after"] = [
            line for line in names if "候補を比較" in line][:4]
        switched = ui.invoke("候補を比較  Ctrl+2")
        method = "uia_invoke_surface_switch"
        if not switched.get("invoked"):
            switched = ui.click("候補を比較  Ctrl+2")
            method = "synthetic_wm_lbutton_at_uia_bounds"
        time.sleep(2.0)
        dump = ui.dump()
        names = list(dump.get("names") or [])
        compare["switch_method"] = method
        compare["switch_result"] = switched
        compare["pair_lines"] = [line for line in names
                                 if "比較" in line or "A側" in line or "B側" in line][:20]
        compare["image_lines"] = [line for line in names
                                  if "画像" in line or "メディア" in line][:10]
        compare["uia"] = uia_snapshot(ui, "compare-surface")
        pair_visible = any("比較対象" in line or "A側" in line or "B側" in line
                           for line in names)
        compare["verdict"] = "PASS" if pair_visible else "PARTIAL"
        back = ui.invoke("成果物  Ctrl+1")
        if not back.get("invoked"):
            back = ui.click("成果物  Ctrl+1")
        compare["back_to_artifact"] = back
    except Exception as error:  # noqa: BLE001
        compare.update(verdict="NOT_RUN", error=f"{type(error).__name__}: {error}")
    result["compare"] = compare

    result["before"] = before
    result["after"] = uia_snapshot(ui, "surfaces-after")
    return result


def scenario_artifact(args: argparse.Namespace) -> dict:
    """Native artifact display + surface interactions on a measured completed task.

    Bounded follow-up session: the isolated DB copy and CAS of a measured run are
    reused (same task identity and the exact canonical bytes the generation
    produced); no new generation is required. Recorded as a split measurement.
    """
    ctx = Ctx("artifact", PORT_MAIN, source_run=Path(args.source_run))
    record = ctx.record
    task_ref = args.task_ref
    record["source_run"] = str(args.source_run)
    record["task"]["task_ref"] = task_ref
    record["measurement_split"] = (
        "follow-up session on the measured run's DB/CAS; the generation, projection"
        " through terminal and OUTCOME were measured in that run, the native artifact"
        " display and surfaces are measured here under a fresh serving epoch")
    try:
        ctx.start_runtime(wait_generation=False)
        task = ctx.projection_task(task_ref)
        record["task"]["projection"] = {
            key: task.get(key)
            for key in ("title", "task_status", "inner_phase", "capabilities_ref")}
        goal = str(task.get("title") or "")
        if not goal:
            record["verdict"] = "NOT_RUN"
            record["residuals"].append("the reused DB carries no TASK projection for the task")
            return record
        ctx.launch_desktop("artifact")
        record["steps"]["task_selected"] = ctx.select_task(goal, "artifact-task-selected", task_ref)
        record["steps"]["capability_media"] = ctx.wait_capability_entry(
            task_ref, "media", "ORIGINAL", lambda support: support == "SUPPORTED", 30.0)
        media_view = ctx.view_artifact(task_ref, "artifact-surface")
        record["steps"]["artifact_surface"] = media_view
        resolved = ctx.resolve_artifact(task_ref)
        media = ctx.fetch_artifact(resolved, args.evidence_dir / "artifact.png")
        record["media"] = {"resolved": redact_descriptor(resolved), "fetch": media,
                           "native_view_state": media_view.get("state"),
                           "native_displayed_image": media_view.get("state") in {"AVAILABLE", "DISPLAYED"},
                           "native_viewport_present": media_view.get("viewport_present"),
                           "native_media_lines": media_view.get("media_lines"),
                           "native_media_requests": [
                               line for line in ctx.runtime.log_tail(400_000).splitlines()
                               if "/media" in line or "media/" in line][-16:]}
        record["outcome"] = resolved.get("outcome")
        record["steps"]["surfaces"] = native_surface_interactions(ctx, task_ref, goal)
        record["resyncs"] = ctx.trace.resyncs
        record["resync_count"] = len(ctx.trace.resyncs)
        record["resync_reasons"] = [item.get("reason") for item in ctx.trace.resyncs]
        record["epoch"] = {"at_start": read_observation_epoch(ctx.db_copy)}
        record["verdict"] = (
            "PASS" if (media_view.get("state") in {"AVAILABLE", "DISPLAYED"}
                       and (media or {}).get("digest_matches_descriptor")) else "PARTIAL")
        surfaces = record["steps"].get("surfaces") or {}
        record["surface_verdicts"] = {
            name: (surfaces.get(name) or {}).get("verdict")
            for name in ("history", "evidence", "flow", "compare")}
        record["not_measured"] = [
            "Start / Next Iteration (DD-25 command semantics remain blocked)",
            "generation in this session (the measured run's artifact is reused)"]
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


def scenario_missing_media_reload(args: argparse.Namespace) -> dict:
    """Display the real image, remove only the isolated CAS blob, reload, observe."""
    ctx = Ctx("missing-media-reload", PORT_INJECT, source_run=Path(args.source_run))
    record = ctx.record
    task_ref = args.task_ref
    record["source_run"] = str(args.source_run)
    record["task"]["task_ref"] = task_ref
    injection: dict = {"at": now_iso()}
    try:
        ctx.start_runtime(wait_generation=False)
        task = ctx.projection_task(task_ref)
        goal = str(task.get("title") or "")
        if not goal:
            injection.update(verdict="NOT_RUN", reason="no TASK projection in the reused DB")
            record["injections"]["missing_media"] = injection
            record["verdict"] = "NOT_RUN"
            return record
        ctx.launch_desktop("missing")
        record["steps"]["task_selected"] = ctx.select_task(goal, "missing-task-selected", task_ref)
        record["steps"]["capability_media"] = ctx.wait_capability_entry(
            task_ref, "media", "ORIGINAL", lambda support: support == "SUPPORTED", 30.0)
        view_before = ctx.view_artifact(task_ref, "missing-artifact-before")
        injection["native_view_before_removal"] = view_before
        injection["native_displayed_before_removal"] = view_before.get("state") in {
            "AVAILABLE", "DISPLAYED"}
        resolved = ctx.resolve_artifact(task_ref)
        digest = (resolved.get("artifact") or {}).get("artifact_digest") or ""
        descriptor_before = ctx.runtime.media_descriptor(ctx.token, ctx.trace.context, digest)
        injection["descriptor_before"] = {
            "http_status": descriptor_before["status"],
            "digest": digest,
            "access_handle_present": bool(
                (descriptor_before.get("document") or {}).get("access_handle")),
        }
        blob = find_cas_blob(ctx.runtime.cas_root, digest) if digest else None
        injection["blob_before"] = (
            {"path_relative_to_cas_root": str(blob.relative_to(ctx.runtime.cas_root)),
             "bytes": blob.stat().st_size, "sha256": sha256_file(blob),
             "digest_matches_descriptor": sha256_file(blob) == digest,
             "isolated_copy_only": True} if blob else None)
        if not injection["native_displayed_before_removal"] or blob is None:
            injection.update(verdict="NOT_RUN", reason=(
                "the image was not displayed before removal"
                if not injection["native_displayed_before_removal"]
                else "the isolated CAS copy carries no blob for the descriptor digest"))
            record["injections"]["missing_media"] = injection
            record["verdict"] = "NOT_RUN"
            return record
        blob.unlink()
        injection["blob_removed"] = not blob.exists()
        after_descriptor = ctx.runtime.media_descriptor(ctx.token, ctx.trace.context, digest)
        injection["descriptor_after_removal"] = {"http_status": after_descriptor["status"]}
        fetch_attempt = ctx.fetch_artifact(resolved, ctx.work / "should-not-exist.png")
        injection["fetch_after_removal"] = {
            "fetched": fetch_attempt.get("fetched"), "reason": fetch_attempt.get("reason"),
            "status": fetch_attempt.get("status")}
        injection["desktop_restart_reload"] = restart_desktop(ctx, goal, task_ref)
        assert ctx.app is not None and ctx.app.ui is not None
        injection["rail_filter_after_restart"] = ctx.filter_rail(goal)
        injection["selected_after_restart"] = ctx.select_task(
            goal, "missing-task-reselected", task_ref).get("selected")
        view_after = ctx.view_artifact(task_ref, "missing-artifact-after")
        injection["native_view_after_removal"] = view_after
        injection["native_media_requests_after_removal"] = [
            line for line in ctx.runtime.log_tail(400_000).splitlines()
            if "/media" in line or "media/" in line][-12:]
        injection["verdict"] = (
            "PASS" if (view_after.get("state") in {"MISSING", "FAILED"}
                       and not fetch_attempt.get("fetched")) else "PARTIAL")
        record["injections"]["missing_media"] = injection
        record["verdict"] = injection["verdict"]
        record["residuals"].append(
            "the removed blob is the only missing-media variant exercised; a corrupt-blob"
            " (digest mismatch) variant is not measured in this run")
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


def scenario_stop_active(args: argparse.Namespace) -> dict:
    """Native STOP against a live ACTIVE task in a reused isolated copy."""
    ctx = Ctx("stop-active", PORT_INJECT, source_run=Path(args.source_run), control=True)
    record = ctx.record
    task_ref = args.task_ref
    record["source_run"] = str(args.source_run)
    record["task"]["task_ref"] = task_ref
    injection: dict = {"at": now_iso()}
    try:
        ctx.start_runtime(wait_generation=False)
        task = ctx.projection_task(task_ref)
        goal = str(task.get("title") or "")
        injection["projection_before"] = {
            key: task.get(key)
            for key in ("task_status", "inner_phase", "capabilities_ref")}
        if str(task.get("task_status")) not in {"ACTIVE", "ASSIGNED"}:
            injection.update(verdict="NOT_RUN", reason=(
                f"the reused task is not live (status={task.get('task_status')});"
                " STOP_TASK cannot be measured"))
            record["injections"]["stop"] = injection
            record["verdict"] = injection["verdict"]
            return record
        seed = ctx.seed_capabilities(task_ref)
        injection["capability_seed"] = {
            "seeded": seed.get("seeded"), "run_id": seed.get("run_id"),
            "run_source": seed.get("run_source"),
            "task_status_at_seed": seed.get("task_status_at_seed"),
            "projection_visible": seed.get("projection_visible"),
            "error": seed.get("error")}
        supported = ctx.wait_capability_entry(
            task_ref, "commands", "STOP_TASK",
            lambda support: support == "SUPPORTED", 60.0)
        injection["stop_supported_projection"] = supported
        ctx.launch_desktop("stop")
        assert ctx.app is not None and ctx.app.ui is not None
        # The app's Desktop principal is issued with the app credential (the last
        # registration in the serving registry). The reused task was created by a
        # previous session's principal, so the isolated copy's owner is rebound to
        # the measured principal; this is test setup, never a fabricated receipt.
        principals = ctx.list_desktop_principals()
        injection["desktop_principals"] = principals
        candidates = list(principals.get("principals") or [])
        if candidates:
            rebind = ctx.rebind_task_owner(task_ref, candidates[-1])
            injection["owner_rebind"] = rebind
        else:
            injection["owner_rebind"] = {"skipped": "no principal in the serving registry"}
        record["steps"]["task_selected"] = ctx.select_task(goal, "stop-task-selected", task_ref)
        ui = ctx.app.ui
        names: list[str] = []
        deadline_ui = time.monotonic() + 60.0
        while time.monotonic() < deadline_ui:
            names = list(ui.dump().get("names") or [])
            if "タスクを停止" in names:
                break
            time.sleep(1.5)
        disabled = "タスクを停止（現在は操作できません）" in names
        enabled = "タスクを停止" in names
        injection.update({"stop_button_enabled": enabled, "stop_button_disabled": disabled,
                          "uia_before": uia_snapshot(ui, "stop-before")})
        if not enabled or disabled:
            injection.update(verdict="NOT_RUN",
                             reason="the native stop button is disabled; the capability"
                                    " projection does not authorize stop for this task")
            record["injections"]["stop"] = injection
            record["verdict"] = injection["verdict"]
            return record
        clicked = ui.invoke("タスクを停止")
        method = "uia_invoke"
        if not clicked.get("invoked"):
            clicked = ui.click("タスクを停止")
            method = "synthetic_wm_lbutton_at_uia_bounds"
        injection["command"] = {"method": method, "result": clicked, "at": now_iso()}
        time.sleep(2.0)
        after_dump = ui.dump()
        injection["receipt_lines_immediate"] = [
            line for line in after_dump.get("names") or []
            if "停止" in line or "受付" in line or "拒否" in line or "送信" in line]
        lookup = ui.invoke("停止の受付状況")
        lookup_method = "uia_invoke"
        if not lookup.get("invoked"):
            lookup = ui.invoke("停止要求の受付状況を確認")
            lookup_method = "uia_invoke_aria_label"
        if not lookup.get("invoked"):
            lookup = ui.click("停止の受付状況")
            lookup_method = "synthetic_wm_lbutton_at_uia_bounds"
        time.sleep(2.0)
        lookup_dump = ui.dump()
        injection["receipt_lookup"] = {
            "method": lookup_method, "result": lookup, "at": now_iso(),
            "lines": [line for line in lookup_dump.get("names") or []
                      if "停止" in line or "受付" in line or "確認" in line][:20],
            "uia": uia_snapshot(ui, "stop-receipt-lookup")}
        injection["screenshot"] = injection["receipt_lookup"]["uia"]["screenshot"]
        transitions: list[dict] = []
        final_task: dict = {}
        deadline = time.monotonic() + 240.0
        while time.monotonic() < deadline:
            response = ctx.runtime.desktop_task(ctx.token, ctx.trace.context, task_ref)
            current = (response.get("document") or {}).get("task") or {}
            state = {"at": now_iso(), "task_status": current.get("task_status"),
                     "inner_phase": current.get("inner_phase"),
                     "projection_revision": current.get("projection_revision")}
            if not transitions or transitions[-1]["task_status"] != state["task_status"]:
                transitions.append(state)
            final_task = current
            if terminal(current):
                break
            time.sleep(2.0)
        injection["transitions"] = transitions
        injection["final_task"] = final_task
        injection["native_after_terminal"] = uia_snapshot(ui, "stop-terminal")
        injection["verdict"] = ("PASS" if str(final_task.get("task_status")) == "CANCELLED"
                                else "PARTIAL")
        record["injections"]["stop"] = injection
        record["verdict"] = injection["verdict"]
        record["residuals"].append(
            "the reused ACTIVE task is an orphaned generation (the generation plane is"
            " unavailable after the ComfyUI dependency break); STOP acceptance is read"
            " from the native stop surface and completion only from projected CANCELLED")
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


def scenario_golden(args: argparse.Namespace) -> dict:
    ctx = Ctx("golden", PORT_MAIN)
    record = ctx.record
    goal = f"{GOAL_PREFIX}-golden {time.strftime('%Y%m%d-%H%M%S')} production still"
    direction = "cinematic mechanical desktop object, no text"
    record["goal"] = goal
    record["_secrets"] = list(ctx.collect)
    try:
        ctx.start_runtime()
        ctx.launch_desktop("main")
        brief = ctx.create_via_brief(goal, direction)
        task_ref = brief.get("task_ref", "")
        if not task_ref:
            task_ref = ctx.find_task_ref_in_projection(goal)
            record["task"]["task_ref_source"] = "projection snapshot after the receipt timeout"
        else:
            record["task"]["task_ref_source"] = (
                "Brief receipt (受付タスク)" if brief.get("receipt_visible")
                else "live TASK projection (receipt not painted)"
            )
        record["task"]["task_ref"] = task_ref
        if not task_ref:
            record["verdict"] = "FAIL"
            record["residuals"].append("the created task could not be resolved in the projection")
            return record

        assert ctx.app is not None and ctx.app.ui is not None
        record["steps"]["brief_close"] = ctx.app.ui.invoke("ブリーフを閉じる")
        record["steps"]["task_selected"] = ctx.select_task(goal, "task-assigned", task_ref)

        restarted = False
        capability_seeded = False
        terminal_record = None
        last_state = None
        first_state = None
        epoch_at_start = read_observation_epoch(ctx.db_copy)
        record["epoch"] = {"at_start": epoch_at_start,
                           "runtime_reported": (ctx.record.get("runtime") or {}).get("epoch")}
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            poll = ctx.trace.poll(task_ref)
            state = poll.get("state")
            if state is not None:
                if first_state is None:
                    first_state = dict(state)
                record["states"].append({"at": now_iso(), **state})
                key = (state.get("task_status"), state.get("inner_phase"))
                if key != last_state:
                    last_state = key
                    label = f"state-{len(record['states'])}-{state.get('task_status', 'unknown')}"
                    snapshot = uia_snapshot(ctx.app.ui, label)
                    record["screenshots"][label] = snapshot["screenshot"]
                    record["steps"].setdefault("state_screenshots", {})[label] = {
                        "state": state, "uia": snapshot,
                        "task_row_excerpt": ctx.app.ui.excerpt((goal, "タスクの状態", "処理段階")),
                    }
                if not capability_seeded and str(state.get("task_status")) == "ACTIVE":
                    capability_seeded = True
                    seeded = ctx.seed_capabilities(task_ref)
                    seeded["at_active_state"] = state
                    record["steps"]["capability_seed"] = seeded
                    record["steps"]["capability_at_active"] = ctx.wait_capability_entry(
                        task_ref, "media", "ORIGINAL", lambda support: True, 15.0)
                if not restarted and str(state.get("task_status")) == "ACTIVE":
                    restarted = True
                    record["injections"]["desktop_restart"] = restart_desktop(ctx, goal, task_ref)
                if terminal(state):
                    terminal_record = state
                    break
            time.sleep(1.0)
        if not restarted:
            record["injections"]["desktop_restart"] = {
                "verdict": "NOT_RUN",
                "reason": (
                    "the task was already terminal when the live trace first observed it"
                    if first_state is not None
                    and str(first_state.get("task_status")) in {"COMPLETED", "CANCELLED", "FAILED"}
                    else "no ACTIVE state was observed inside the run budget"
                ),
            }

        if terminal_record is None:
            awaited = wait_task_state(ctx, task_ref, lambda task: terminal(task), 120.0)
            if awaited.get("matched"):
                terminal_record = awaited["task"]
                record["states"].append({"at": now_iso(), **terminal_record})
        record["terminal"] = terminal_record
        if terminal_record is None:
            record["verdict"] = "PARTIAL"
            record["residuals"].append("the task did not reach a terminal state inside the run budget")
            return record

        record["steps"]["task_selected_terminal"] = ctx.select_task(goal, "task-terminal", task_ref)
        record["steps"]["capability_media_at_terminal"] = ctx.wait_capability_entry(
            task_ref, "media", "ORIGINAL", lambda support: support == "SUPPORTED", 60.0)
        media_view = ctx.view_artifact(task_ref, "artifact-surface")
        record["steps"]["artifact_surface"] = media_view
        resolved = ctx.resolve_artifact(task_ref)
        media = ctx.fetch_artifact(resolved, args.evidence_dir / "artifact.png")
        record["media"] = {"resolved": redact_descriptor(resolved), "fetch": media,
                           "native_view_state": media_view.get("state"),
                           "native_displayed_image": media_view.get("state") in {"AVAILABLE", "DISPLAYED"},
                           "native_viewport_present": media_view.get("viewport_present"),
                           "native_media_lines": media_view.get("media_lines"),
                           "native_media_requests": [
                               line for line in ctx.runtime.log_tail(400_000).splitlines()
                               if "/media" in line or "media/" in line][-16:]}
        record["outcome"] = resolved.get("outcome")
        record["epoch"]["at_terminal"] = read_observation_epoch(ctx.db_copy)
        record["steps"]["surfaces"] = native_surface_interactions(ctx, task_ref, goal)
        fresh = ctx.runtime.open_observation(ctx.token)
        fresh_doc = fresh.get("document") or {}
        context_id = str(((fresh_doc.get("grant") or {}).get("stamp") or {}).get("context_id") or "")
        record["canonical_reads"] = {
            "task": ctx.runtime.desktop_task(ctx.token, context_id, task_ref).get("document"),
            "history": ctx.runtime.desktop_history(ctx.token, context_id, task_ref).get("document"),
        }
        record["projection_trace"] = ctx.trace.transactions
        record["resyncs"] = ctx.trace.resyncs
        record["resync_count"] = len(ctx.trace.resyncs)
        record["resync_reasons"] = [item.get("reason") for item in ctx.trace.resyncs]
        record["unknown_owner_resyncs"] = [
            item for item in ctx.trace.resyncs
            if "unknown-owner" in str(item.get("reason") or "")]
        record["trace_errors"] = ctx.trace.errors
        record["runtime_log_epoch_conflict"] = (
            "observation epoch changed in the database ledger" in ctx.runtime.log_tail(200_000))
        epoch_start = record["epoch"].get("at_start") or {}
        epoch_terminal = record["epoch"].get("at_terminal") or {}
        record["epoch"]["serving_owned"] = (
            bool(epoch_start.get("epoch")) and epoch_start.get("epoch")
            == epoch_terminal.get("epoch"))
        record["epoch"]["unchanged_during_generation"] = record["epoch"]["serving_owned"]
        record["verdict"] = (
            "PASS" if (media_view.get("state") in {"AVAILABLE", "DISPLAYED"}
                       and str(terminal_record.get("task_status")) == "COMPLETED") else "PARTIAL")
        if media_view.get("state") == "BLOCKED_CAPABILITY":
            record["residuals"].append(
                "native artifact display is BLOCKED: the observation snapshot carries no"
                " CAPABILITIES projection for the live-generated task, so the app fails"
                " closed with 機能の利用可否情報が投影されていません; the canonical RC-13 media"
                " bytes were still fetched and digest-verified through the protocol client")
        elif record["verdict"] != "PASS":
            record["residuals"].append(
                f"native artifact view state = {media_view.get('state')};"
                f" terminal status = {terminal_record.get('task_status')}")
        surfaces = record["steps"].get("surfaces") or {}
        record["surface_verdicts"] = {
            name: (surfaces.get(name) or {}).get("verdict")
            for name in ("history", "evidence", "flow", "compare")}
        record["not_measured"] = [
            "Start / Next Iteration (DD-25 command semantics remain blocked)",
            "screen-reader/IME/DPI/High Contrast acceptance (outside this lane)",
        ]
        recipes = (record.get("catalog") or {}).get("recipes") or []
        record["multi_recipe"] = {
            "catalog_recipes": len(recipes),
            "recipes": recipes,
            "verdict": "NOT_RUN" if len(recipes) < 2 else "AVAILABLE",
            "reason": ("the authenticated catalog exposes one recipe; a second recipe"
                       " cannot be run without changing the catalog or the Runtime"
                       if len(recipes) < 2 else "a second catalog recipe is available"),
        }
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()
        record["screenshots"]["connected"] = (
            record.get("steps", {}).get("connected-main", {}).get("uia", {}).get("screenshot"))
        record["screenshots"]["brief_filled"] = (
            record.get("steps", {}).get("brief", {}).get("uia_filled", {}).get("screenshot"))
        record["screenshots"]["artifact"] = (
            record.get("steps", {}).get("artifact_surface", {}).get("uia_after", {}).get("screenshot"))
        record["screenshots"]["terminal"] = (
            record.get("steps", {}).get("task_selected_terminal", {}).get("uia", {}).get("screenshot"))


def scenario_receipt(args: argparse.Namespace) -> dict:
    """Bounded receipt-only native run: stop once the accepted receipt is painted.

    The MP-ACC1 native lane (§19.318) recorded that an accepted Runtime receipt
    was never painted by the Brief surface. This run submits the same native
    Brief and waits only for the accepted-receipt surface (`受付タスク` plus the
    state label), capturing UIA and a screenshot before generation completes.
    It deliberately does not wait for a terminal task, so the generation plane is
    not required.
    """
    ctx = Ctx("receipt", PORT_MAIN)
    record = ctx.record
    goal = f"{GOAL_PREFIX}-receipt {time.strftime('%Y%m%d-%H%M%S')} production still"
    direction = "cinematic mechanical desktop object, no text"
    record["goal"] = goal
    record["_secrets"] = list(ctx.collect)
    record["generation_plane_required"] = False
    try:
        ctx.start_runtime(wait_generation=False)
        ctx.launch_desktop("receipt")
        brief = ctx.create_via_brief(goal, direction, receipt_only=True)
        task_ref = brief.get("task_ref", "")
        receipt_visible = brief.get("receipt_visible") is True
        record["task"]["task_ref"] = task_ref
        record["task"]["task_ref_source"] = (
            "Brief receipt (受付タスク)" if receipt_visible else "receipt not painted")
        record["task"]["receipt_visible"] = receipt_visible
        record["task"]["receipt_at"] = brief.get("receipt_at", "")
        if receipt_visible:
            # The task itself must still resolve only through observation.
            projected = ctx.find_task_ref_in_projection(goal)
            record["task"]["projection_task_ref"] = projected
            record["task"]["receipt_matches_projection"] = projected == task_ref
            record["projection_trace"] = ctx.trace.transactions
            record["resyncs"] = ctx.trace.resyncs
            record["trace_errors"] = ctx.trace.errors
            record["residuals"].append(
                "generation was not awaited: the run stopped once the accepted receipt"
                " was painted, so terminal/artifact surfaces are NOT_MEASURED here")
            record["verdict"] = (
                "PASS" if (projected == task_ref and task_ref) else "PARTIAL")
            if record["verdict"] != "PASS":
                record["residuals"].append(
                    "the painted receipt task ref did not match the observed TASK projection")
        else:
            record["verdict"] = "FAIL"
            record["residuals"].append(
                "the accepted receipt was not painted after submit (no 受付タスク surface)")
            record["residuals"].append(
                "generation was not awaited: the run stopped after the receipt timeout")
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()
        record["screenshots"]["connected"] = (
            record.get("steps", {}).get("connected-receipt", {}).get("uia", {}).get("screenshot"))
        record["screenshots"]["brief_filled"] = (
            record.get("steps", {}).get("brief", {}).get("uia_filled", {}).get("screenshot"))
        record["screenshots"]["receipt"] = (
            record.get("steps", {}).get("brief", {}).get("uia_receipt", {}).get("screenshot"))


def scenario_stop(args: argparse.Namespace) -> dict:
    ctx = Ctx("stop", PORT_INJECT)
    record = ctx.record
    goal = f"{GOAL_PREFIX}-stop {time.strftime('%Y%m%d-%H%M%S')} production still"
    record["goal"] = goal
    record["_secrets"] = list(ctx.collect)
    try:
        ctx.start_runtime()
        ctx.launch_desktop("stop")
        brief = ctx.create_via_brief(goal, "cinematic mechanical desktop object, no text")
        task_ref = brief.get("task_ref", "") or ctx.find_task_ref_in_projection(goal)
        record["task"]["task_ref"] = task_ref
        if not task_ref:
            record["injections"]["stop"] = {"verdict": "NOT_RUN", "reason": "task not resolved"}
            record["verdict"] = "NOT_RUN"
            return record
        assert ctx.app is not None and ctx.app.ui is not None
        ctx.app.ui.invoke("ブリーフを閉じる")
        ctx.select_task(goal, "stop-task-selected", task_ref)
        active = wait_task_state(ctx, task_ref,
                                 lambda task: str(task.get("task_status")) == "ACTIVE", 120.0)
        injection: dict = {"wait_active": active, "at": now_iso()}
        seed = ctx.seed_capabilities(task_ref)
        injection["capability_seed"] = {
            "seeded": seed.get("seeded"), "run_id": seed.get("run_id"),
            "run_source": seed.get("run_source"),
            "task_status_at_seed": seed.get("task_status_at_seed"),
            "projection_visible": seed.get("projection_visible"),
            "error": seed.get("error"),
        }
        supported = ctx.wait_capability_entry(
            task_ref, "commands", "STOP_TASK",
            lambda support: support == "SUPPORTED", 60.0)
        injection["stop_supported_projection"] = supported
        ui = ctx.app.ui
        # The capability delta must reach the window before the control enables.
        names: list[str] = []
        deadline_ui = time.monotonic() + 60.0
        while time.monotonic() < deadline_ui:
            names = list(ui.dump().get("names") or [])
            if "タスクを停止" in names:
                break
            if not supported.get("matched"):
                break
            time.sleep(1.5)
        disabled = "タスクを停止（現在は操作できません）" in names
        enabled = "タスクを停止" in names
        injection.update({"stop_button_enabled": enabled, "stop_button_disabled": disabled,
                          "capability_not_projected": CAPABILITY_BLOCKED_MARKER
                          in "\n".join(names),
                          "uia_before": uia_snapshot(ui, "stop-before")})
        if not enabled or disabled:
            injection.update(verdict="NOT_RUN",
                             reason="the native stop button is disabled; the capability"
                                    " projection does not authorize stop for this task"
                                    + (" (機能の利用可否情報が投影されていません)"
                                       if injection["capability_not_projected"] else ""))
            record["injections"]["stop"] = injection
            record["verdict"] = "NOT_RUN"
            return record
        clicked = ui.invoke("タスクを停止")
        method = "uia_invoke"
        if not clicked.get("invoked"):
            clicked = ui.click("タスクを停止")
            method = "synthetic_wm_lbutton_at_uia_bounds"
        injection["command"] = {"method": method, "result": clicked, "at": now_iso()}
        time.sleep(2.0)
        after_dump = ui.dump()
        injection["receipt_lines_immediate"] = [
            line for line in after_dump.get("names") or []
            if "停止" in line or "受付" in line or "拒否" in line or "送信" in line]
        # Explicit bounded receipt lookup, never a resend.
        lookup = ui.invoke("停止の受付状況")
        lookup_method = "uia_invoke"
        if not lookup.get("invoked"):
            lookup = ui.invoke("停止要求の受付状況を確認")
            lookup_method = "uia_invoke_aria_label"
        if not lookup.get("invoked"):
            lookup = ui.click("停止の受付状況")
            lookup_method = "synthetic_wm_lbutton_at_uia_bounds"
        time.sleep(2.0)
        lookup_dump = ui.dump()
        injection["receipt_lookup"] = {
            "method": lookup_method, "result": lookup, "at": now_iso(),
            "lines": [line for line in lookup_dump.get("names") or []
                      if "停止" in line or "受付" in line or "確認" in line][:20],
            "uia": uia_snapshot(ui, "stop-receipt-lookup"),
        }
        injection["screenshot"] = injection["receipt_lookup"]["uia"]["screenshot"]
        transitions: list[dict] = []
        final_task: dict = {}
        deadline = time.monotonic() + 240.0
        while time.monotonic() < deadline:
            response = ctx.runtime.desktop_task(ctx.token, ctx.trace.context, task_ref)
            task = (response.get("document") or {}).get("task") or {}
            state = {"at": now_iso(), "task_status": task.get("task_status"),
                     "inner_phase": task.get("inner_phase"),
                     "projection_revision": task.get("projection_revision")}
            if not transitions or transitions[-1]["task_status"] != state["task_status"]:
                transitions.append(state)
            final_task = task
            if terminal(task):
                break
            time.sleep(2.0)
        injection["transitions"] = transitions
        injection["final_task"] = final_task
        injection["native_after_terminal"] = uia_snapshot(ui, "stop-terminal")
        injection["native_terminal_excerpt"] = ui.excerpt(
            (goal, "タスクの状態", "処理段階", "キャンセル", "停止"))
        injection["verdict"] = ("PASS" if str(final_task.get("task_status")) == "CANCELLED"
                                else "PARTIAL")
        record["injections"]["stop"] = injection
        record["verdict"] = injection["verdict"]
        record["residuals"].append(
            "STOP_TASK acceptance is read from the native stop-status surface and the"
            " Runtime audit record is not available on an isolated serving process;"
            " completion is claimed only from the projected CANCELLED state")
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


def scenario_revoke(args: argparse.Namespace) -> dict:
    ctx = Ctx("revoke", PORT_INJECT, control=True)
    record = ctx.record
    goal = f"{GOAL_PREFIX}-revoke {time.strftime('%Y%m%d-%H%M%S')} production still"
    record["goal"] = goal
    record["_secrets"] = list(ctx.collect)
    try:
        ctx.start_runtime()
        ctx.launch_desktop("revoke")
        brief = ctx.create_via_brief(goal, "cinematic mechanical desktop object, no text")
        task_ref = brief.get("task_ref", "") or ctx.find_task_ref_in_projection(goal)
        record["task"]["task_ref"] = task_ref
        if not task_ref:
            record["injections"]["credential_revocation"] = {
                "verdict": "NOT_RUN", "reason": "task not resolved"}
            record["verdict"] = "NOT_RUN"
            return record
        assert ctx.app is not None and ctx.app.ui is not None
        ctx.app.ui.invoke("ブリーフを閉じる")
        ctx.select_task(goal, "revoke-task-selected", task_ref)
        active = wait_task_state(ctx, task_ref,
                                 lambda task: str(task.get("task_status")) == "ACTIVE", 120.0)
        injection: dict = {"wait_active": active, "at": now_iso()}
        injection["uia_before"] = uia_snapshot(ctx.app.ui, "revoke-before")
        trigger = ctx.control_dir / "revoke-desktop-credentials"
        trigger.write_text("revoke", encoding="utf-8")
        ack_path = ctx.control_dir / "revoke-desktop-credentials.ack.json"
        deadline = time.monotonic() + 20.0
        ack: dict = {}
        while time.monotonic() < deadline:
            if ack_path.is_file():
                ack = json.loads(ack_path.read_text(encoding="utf-8"))
                break
            time.sleep(0.3)
        injection["ack"] = ack
        injection["trigger_accepted"] = bool(ack)
        time.sleep(3.0)
        injection["uia_after_3s"] = uia_snapshot(ctx.app.ui, "revoke-after-3s")
        reissue = ctx.runtime.issue_credential()
        new_token = (reissue.get("document") or {}).get("token") or ""
        injection["observer_reissue_http_status"] = reissue["status"]
        if new_token:
            ctx.collect.append(new_token)
            ctx.token = new_token
            ctx.trace = Trace(ctx.runtime, new_token)
            injection["observer_reopen"] = ctx.trace.open()
        deadline = time.monotonic() + 30.0
        reauth_line = False
        while time.monotonic() < deadline:
            if "Runtime credential reauthorized after rejection" in ctx.app.log_text():
                reauth_line = True
                break
            time.sleep(1.0)
        injection["reauth_log_line"] = reauth_line
        reselect = ctx.select_task(goal, "revoke-task-reselected", task_ref)
        injection["reselect"] = {"selected": reselect.get("selected")}
        transitions: list[dict] = []
        final_task: dict = {}
        deadline = time.monotonic() + 240.0
        while time.monotonic() < deadline:
            poll = ctx.trace.poll(task_ref)
            if poll.get("state") is not None:
                transitions.append({"at": now_iso(), **poll["state"]})
            response = ctx.runtime.desktop_task(ctx.token, ctx.trace.context, task_ref)
            final_task = (response.get("document") or {}).get("task") or {}
            if terminal(final_task):
                break
            time.sleep(1.5)
        injection["transitions"] = transitions
        injection["final_task"] = final_task
        injection["uia_terminal"] = uia_snapshot(ctx.app.ui, "revoke-terminal")
        injection["verdict"] = (
            "PASS" if reauth_line and str(final_task.get("task_status")) == "COMPLETED"
            else "PARTIAL")
        record["injections"]["credential_revocation"] = injection
        record["verdict"] = injection["verdict"]
        record["residuals"].append(
            "The Desktop token is not exposed by design; the isolated serving process"
            " revoked its own in-memory Desktop credential registry, which is the same"
            " 401 the product revoke route produces")
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


def find_cas_blob(cas_root: Path, digest: str) -> Path | None:
    hexdigest = digest.split(":", 1)[-1]
    candidates = [path for path in cas_root.rglob("*")
                  if path.is_file() and hexdigest in path.as_posix().replace("\\", "/")]
    if len(candidates) == 1:
        return candidates[0]
    for path in cas_root.rglob("*"):
        if path.is_file() and sha256_file(path) == digest:
            return path
    return None


def scenario_missing_media(args: argparse.Namespace) -> dict:
    ctx = Ctx("missing-media", PORT_INJECT)
    record = ctx.record
    goal = f"{GOAL_PREFIX}-missing-media {time.strftime('%Y%m%d-%H%M%S')} production still"
    record["goal"] = goal
    record["_secrets"] = list(ctx.collect)
    injection: dict = {"at": now_iso()}
    try:
        ctx.start_runtime()
        catalog = record["catalog"]["recipes"][0]
        target_recipe_ref = acc1.build_target_recipe_ref(catalog["recipe_ref"])
        command_id = "cmd-" + secrets.token_hex(12)
        envelope = {
            "command_id": command_id,
            "target": {"target": "CREATION", "recipe_ref": target_recipe_ref},
            "expected_revision": None,
            "epoch": record["runtime"]["epoch"],
            "payload": {
                "kind": "CREATE_TASK", "objective_text": goal, "text_prompt": goal,
                "reference_artifact_refs": [], "parameter_values": [],
            },
        }
        create = ctx.runtime.command(ctx.token, ctx.trace.context, envelope)
        receipt = ((create.get("document") or {}).get("receipt") or {})
        task_ref = str(receipt.get("task_id") or "")
        injection["create"] = {"http_status": create["status"], "command_id": command_id,
                               "receipt": receipt}
        if not task_ref:
            injection.update(verdict="NOT_RUN", reason="the protocol create was not accepted")
            record["injections"]["missing_media"] = injection
            record["verdict"] = "NOT_RUN"
            return record
        record["task"]["task_ref"] = task_ref
        injection["terminal"] = wait_task_state(ctx, task_ref, terminal, 300.0)
        resolved = ctx.resolve_artifact(task_ref)
        if not resolved.get("found"):
            injection.update(verdict="NOT_RUN",
                             reason="the terminal task carries no resolvable artifact")
            record["injections"]["missing_media"] = injection
            record["verdict"] = "NOT_RUN"
            return record
        seed = ctx.seed_capabilities(task_ref)
        injection["capability_seed"] = {
            "seeded": seed.get("seeded"), "run_id": seed.get("run_id"),
            "run_source": seed.get("run_source"),
            "task_status_at_seed": seed.get("task_status_at_seed"),
            "projection_visible": seed.get("projection_visible"),
            "error": seed.get("error"),
        }
        supported = ctx.wait_capability_entry(
            task_ref, "media", "ORIGINAL", lambda support: support == "SUPPORTED", 60.0)
        injection["media_supported_projection"] = supported
        digest = resolved["artifact"]["artifact_digest"]
        blob = find_cas_blob(ctx.runtime.cas_root, digest)
        if blob is None:
            injection.update(verdict="NOT_RUN",
                             reason="the isolated CAS carries no blob for the descriptor digest")
            record["injections"]["missing_media"] = injection
            record["verdict"] = "NOT_RUN"
            return record
        before = {"path_relative_to_cas_root": str(blob.relative_to(ctx.runtime.cas_root)),
                  "bytes": blob.stat().st_size, "sha256": sha256_file(blob),
                  "digest_matches_descriptor": sha256_file(blob) == digest}
        injection["blob_before"] = before
        injection["descriptor_before"] = {
            "http_status": resolved["descriptor_http_status"],
            "access_handle_present": bool((resolved.get("descriptor") or {}).get("access_handle")),
        }
        ctx.launch_desktop("missing")
        assert ctx.app is not None and ctx.app.ui is not None
        injection["rail_filter"] = ctx.filter_rail(goal)
        selected = ctx.select_task(goal, "missing-task-selected", task_ref)
        injection["selected"] = selected.get("selected")
        view_before = ctx.view_artifact(task_ref, "missing-artifact-before")
        injection["native_view_before_removal"] = view_before
        injection["native_displayed_before_removal"] = view_before.get("state") in {
            "AVAILABLE", "DISPLAYED"}
        log_before_removal = ctx.runtime.log_tail(400_000)
        injection["native_media_requests_before_removal"] = [
            line for line in log_before_removal.splitlines()
            if "/media" in line or "media/" in line][-12:]
        if view_before.get("state") not in {"AVAILABLE", "DISPLAYED"}:
            injection.update(verdict="NOT_RUN",
                             reason=("the native artifact surface did not display the image"
                                     f" before removal (state={view_before.get('state')});"
                                     " the missing-media injection needs a displayed image"))
            record["injections"]["missing_media"] = injection
            record["verdict"] = "NOT_RUN"
            return record
        blob.unlink()
        injection["blob_removed"] = not blob.exists()
        after_descriptor = ctx.runtime.media_descriptor(
            ctx.token, ctx.trace.context, digest)
        injection["descriptor_after_removal"] = {"http_status": after_descriptor["status"]}
        fetch_attempt = ctx.fetch_artifact(resolved, ctx.work / "should-not-exist.png")
        injection["fetch_after_removal"] = {
            "fetched": fetch_attempt.get("fetched"), "reason": fetch_attempt.get("reason"),
            "status": fetch_attempt.get("status")}
        # A Desktop restart is the deterministic re-open: no in-memory texture can
        # mask the removed canonical blob, so the reload must ask the Runtime again.
        injection["desktop_restart_reload"] = restart_desktop(ctx, goal, task_ref)
        assert ctx.app is not None and ctx.app.ui is not None
        injection["rail_filter_after_restart"] = ctx.filter_rail(goal)
        injection["selected_after_restart"] = ctx.select_task(
            goal, "missing-task-reselected", task_ref).get("selected")
        view_after = ctx.view_artifact(task_ref, "missing-artifact-after")
        injection["native_view_after_removal"] = view_after
        log_after_removal = ctx.runtime.log_tail(400_000)
        injection["native_media_requests_after_removal"] = [
            line for line in log_after_removal.splitlines()
            if "/media" in line or "media/" in line][-12:]
        if view_after.get("state") == "BLOCKED_CAPABILITY":
            injection["verdict"] = "NOT_RUN"
            injection["reason"] = (
                "the native media path refuses before the descriptor/blob fetch because"
                " the observation carries no CAPABILITIES projection (機能の利用可否情報が"
                "投影されていません); removing the CAS blob is therefore not observable"
                " through the native surface, so the missing-media state is NOT_RUN, not"
                " fabricated")
        else:
            injection["verdict"] = (
                "PASS" if (view_after.get("state") in {"MISSING", "FAILED"}
                           and not fetch_attempt.get("fetched")
                           and injection["native_displayed_before_removal"]) else "PARTIAL")
        record["injections"]["missing_media"] = injection
        record["verdict"] = injection["verdict"]
        record["residuals"].append(
            "the removed blob is the only missing-media variant exercised; a corrupt-blob"
            " (digest mismatch) variant is not measured in this run")
        return record
    finally:
        ctx.cleanup()
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


# --------------------------------------------------------------------------- orchestration


def build_desktop() -> dict:
    command = ["cargo", "build", "--locked", "--release", "-p", "cos-desktop"]
    started = time.monotonic()
    completed = subprocess.run(command, cwd=str(ROOT), capture_output=True, text=True,
                               timeout=1800, encoding="utf-8", errors="replace")
    return {"command": " ".join(command), "exit_code": completed.returncode,
            "elapsed_s": round(time.monotonic() - started, 2),
            "stdout_tail": completed.stdout[-1500:], "stderr_tail": completed.stderr[-1500:],
            "binary": str(DESKTOP_EXE),
            "binary_sha256": sha256_file(DESKTOP_EXE) if DESKTOP_EXE.is_file() else "missing",
            "binary_bytes": DESKTOP_EXE.stat().st_size if DESKTOP_EXE.is_file() else 0,
            "binary_mtime": time.strftime("%Y-%m-%dT%H:%M:%S",
                                          time.localtime(DESKTOP_EXE.stat().st_mtime))
            if DESKTOP_EXE.is_file() else None}


def leak_scan(evidence_dir: Path, secrets_list: list[str]) -> dict:
    findings: list[str] = []
    checked = 0
    for path in sorted(evidence_dir.rglob("*")):
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        checked += 1
        for secret in secrets_list:
            if secret and secret in text:
                findings.append(f"secret material in {path.name}")
        for pattern, label in ((r"Bearer\s+[A-Za-z0-9._\-]{16,}", "bearer token"),
                               (r"mh1\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+", "media handle")):
            if re.search(pattern, text):
                findings.append(f"{label} in {path.name}")
    return {"files_checked": checked, "secrets_checked": len(secrets_list),
            "findings": findings, "clean": not findings}


def copy_run_evidence(run: dict, evidence_dir: Path) -> None:
    """Copy sanitized logs, UIA dumps and screenshots into the evidence directory."""
    secrets_list = list(run.get("_secrets") or [])
    work = Path(str(run.get("work_dir") or ""))
    label = str(run.get("label") or "run")
    logs_dir = evidence_dir / "logs" / label
    uia_dir = evidence_dir / "uia" / label
    shots_dir = evidence_dir / "shots" / label
    logs: dict = {}
    if work.is_dir():
        for path in sorted(work.glob("*.log")):
            text = path.read_text(encoding="utf-8", errors="replace")
            for secret in secrets_list:
                if secret:
                    text = text.replace(secret, "<redacted>")
            text = re.sub(r"mh1\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+", "<redacted-media-handle>", text)
            text = re.sub(r"Bearer\s+[A-Za-z0-9._\-]+", "Bearer <redacted>", text)
            logs_dir.mkdir(parents=True, exist_ok=True)
            target = logs_dir / f"{label}-{path.name}"
            target.write_text(text, encoding="utf-8")
            logs[path.name] = {"file": str(target.relative_to(evidence_dir)),
                               "sha256": sha256_bytes(text.encode("utf-8")),
                               "stored_sha256": sha256_file(target),
                               "bytes": target.stat().st_size}
        for source_dir, target_dir, suffixes in (
            (work / "uia", uia_dir, {".json"}),
            (work / "shots", shots_dir, {".png"}),
        ):
            if not source_dir.is_dir():
                continue
            for path in sorted(source_dir.iterdir()):
                if path.suffix not in suffixes or not path.is_file():
                    continue
                target_dir.mkdir(parents=True, exist_ok=True)
                target = target_dir / path.name
                shutil.copyfile(path, target)
    files: dict = {}
    if shots_dir.is_dir():
        for path in sorted(shots_dir.iterdir()):
            files[path.name] = {"file": str(path.relative_to(evidence_dir)),
                                "sha256": sha256_file(path), "bytes": path.stat().st_size}
    run["logs"] = logs
    run["screenshot_files"] = files
    run["uia_dir"] = str(uia_dir.relative_to(evidence_dir)) if uia_dir.is_dir() else ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", action="append",
                        choices=("golden", "receipt", "stop", "revoke", "missing-media",
                                 "artifact", "missing-media-reload", "stop-active"),
                        help="scenario to run (repeatable; default: golden)")
    parser.add_argument("--evidence-dir", default=str(EVIDENCE_DIR))
    parser.add_argument("--desktop-exe", default="",
                        help="bounded variant: launch this release binary instead of target/release")
    parser.add_argument("--timeout", type=float, default=900.0)
    parser.add_argument("--source-run", default="",
                        help="reuse this measured run's isolated DB copy and CAS")
    parser.add_argument("--task-ref", default="",
                        help="task identity inside the reused DB for artifact/stop variants")
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args(argv)
    if args.scenario and set(args.scenario) & {"artifact", "missing-media-reload",
                                               "stop-active"}:
        if not args.source_run or not args.task_ref:
            raise SystemExit("--source-run and --task-ref are required for the reuse variants")
    if args.desktop_exe:
        global DESKTOP_EXE
        DESKTOP_EXE = Path(args.desktop_exe).resolve()
    args.evidence_dir = Path(args.evidence_dir).resolve()
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    (args.evidence_dir / "logs").mkdir(parents=True, exist_ok=True)
    scenarios = args.scenario or ["golden"]
    if PORT_MAIN == PORT_INJECT:
        raise SystemExit("main and injection ports must differ")

    session: dict = {
        "measurement": "MP-ACC1 Golden Creative Workflow native acceptance attempt",
        "contract": "DC-MP-ACC1", "started_at": now_iso(), "verdict": "NOT_RUN",
        "harness": {
            "command": "python apps/desktop/devtools/runtime_integration/run_acc1_native.py",
            "scenarios": scenarios, "ports": {PORT_MAIN: "golden", PORT_INJECT: "injections"},
            "work_root": str(WORK_ROOT), "desktop_binary_launched": True,
            "observation": "native window (UIA + PrintWindow) plus a harness read:observe client",
            "evidence_dir": str(args.evidence_dir),
        },
        "desktop": {
            "root": str(ROOT), "head": base.git_value(ROOT, "rev-parse", "HEAD"),
            "binary_launched": str(DESKTOP_EXE),
            "binary_sha256": sha256_file(DESKTOP_EXE) if DESKTOP_EXE.is_file() else "missing",
            "build": {} if args.no_build else build_desktop(),
        },
        "runtime_source": {
            "root": str(RUNTIME_ROOT), "head": base.git_value(RUNTIME_ROOT, "rev-parse", "HEAD"),
            "python": str(RUNTIME_PYTHON),
            "dirty_tracked": [line for line in base.git_value(
                RUNTIME_ROOT, "status", "--porcelain=v1").splitlines() if line[:2].strip()][:40],
        },
        "runs": {}, "leak_check": {}, "cleanup": {}, "not_measured": [], "residuals": [],
    }
    if not args.no_build and session["desktop"]["build"].get("exit_code") != 0:
        session["verdict"] = "FAIL"
        session["error"] = "the release binary did not build at the current HEAD"
        write_json(args.evidence_dir / "session.json", session)
        return 1

    collected_secrets: list[str] = []
    prior_runs: dict = {}
    prior_path = args.evidence_dir / "session.json"
    if prior_path.is_file():
        try:
            prior_runs = (json.loads(prior_path.read_text(encoding="utf-8")) or {}).get("runs") or {}
        except (OSError, json.JSONDecodeError):
            prior_runs = {}
    for name in scenarios:
        try:
            if name == "golden":
                run = scenario_golden(args)
            elif name == "receipt":
                run = scenario_receipt(args)
            elif name == "stop":
                run = scenario_stop(args)
            elif name == "stop-active":
                run = scenario_stop_active(args)
            elif name == "artifact":
                run = scenario_artifact(args)
            elif name == "missing-media-reload":
                run = scenario_missing_media_reload(args)
            elif name == "revoke":
                run = scenario_revoke(args)
            else:
                run = scenario_missing_media(args)
        except Exception as error:  # noqa: BLE001 - record the failure, never hide it
            import traceback
            run = {
                "label": name, "started_at": now_iso(), "verdict": "FAIL",
                "error": f"{type(error).__name__}: {error}",
                "traceback": traceback.format_exc()[-4000:],
                "work_dir": "", "_secrets": [],
            }
        collected_secrets.extend(run.get("_secrets") or [])
        copy_run_evidence(run, args.evidence_dir)
        run = redact_document(run)
        run.pop("_secrets", None)
        session["runs"][name] = run
        write_json(args.evidence_dir / f"record-{name}.json", run)
        write_json(args.evidence_dir / "session-partial.json", session)

    session["finished_at"] = now_iso()
    merged_runs = dict(prior_runs)
    merged_runs.update(session["runs"])
    session["runs"] = merged_runs
    verdicts = {name: run.get("verdict") for name, run in session["runs"].items()}
    session["verdicts"] = verdicts
    if verdicts and all(value == "PASS" for value in verdicts.values()):
        session["verdict"] = "PASS"
    elif any(value in {"PASS", "PARTIAL"} for value in verdicts.values()):
        session["verdict"] = "PARTIAL"
    else:
        session["verdict"] = "NOT_RUN"
    session["not_measured"] = sorted(
        {item for run in session["runs"].values() for item in (run.get("not_measured") or [])})
    session["residuals"] = sorted(
        {item for run in session["runs"].values() for item in (run.get("residuals") or [])})
    session["cleanup"] = {
        "ports_after_runs": {str(port): port_open(port) for port in (PORT_MAIN, PORT_INJECT)},
        "comfyui_8188_open": port_open(8188),
        "runs": {name: run.get("cleanup") for name, run in session["runs"].items()},
    }
    session = redact_document(session)
    session["leak_check"] = leak_scan(args.evidence_dir, collected_secrets)
    write_json(args.evidence_dir / "session.json", session)
    write_json(args.evidence_dir / "record.json", {"runs": session["runs"]})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
