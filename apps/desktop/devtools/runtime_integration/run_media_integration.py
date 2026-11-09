"""Live IMG6/RT6 media integration evidence (Desktop binary boundary <-> Runtime).

One isolated run:
  * copy the production DB read-only into the workspace,
  * start ``orchestrator_app.api`` on 127.0.0.1:8127 with a per-run auth secret,
    an isolated config file and a per-run media-handle secret,
  * issue Desktop credentials, open an observation and select a task-linked
    artifact with available bytes from the DB copy (descriptor probe),
  * run the real Desktop executor live test (descriptor -> blob -> admission ->
    decode -> presentation request) and the 401/403/410 negative matrix,
  * launch the release Desktop natively with the endpoint and a per-run issuance
    secret (fresh protected-store entry), invoke a Dock candidate through UIA,
    and capture the artifact surface with PrintWindow.

Never written to disk: tokens, the issuance secret, the media-handle secret and
access-handle values. The summary records descriptor fields, byte counts, digests,
status codes, UIA texts and screenshot hashes.
"""

from __future__ import annotations

import argparse
import base64
import contextlib
import ctypes
import hashlib
import hmac
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
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path

DESKTOP_ROOT = Path(__file__).resolve().parents[4]
RUNTIME_ROOT = Path("C:/AI/orchestrator")
RUNTIME_PYTHON = RUNTIME_ROOT / ".venv" / "Scripts" / "python.exe"
SOURCE_DB = RUNTIME_ROOT / "db" / "generation.sqlite"
SOURCE_CAS = RUNTIME_ROOT / "db" / "cas"
CONFIG_TEMPLATE = RUNTIME_ROOT / "configs" / "default.yaml"
DESKTOP_EXE = DESKTOP_ROOT / "target" / "release" / "cos-desktop.exe"
EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "img6-rt6-latest"
)
WORK_ROOT = DESKTOP_ROOT / "target" / "media-live"
PROBE = Path(__file__).resolve().parent / "probe_window.ps1"
LOCK = Path("C:/tmp/cos-desktop-native.lock")
PORT = 8127
ENDPOINT = f"http://127.0.0.1:{PORT}"
LABEL = "run1"

STATUS_LABELS = {
    "ASSIGNED": "割り当て済み（ASSIGNED）",
    "ACTIVE": "実行中（ACTIVE）",
    "COMPLETED": "完了（COMPLETED）",
    "CANCELLED": "中止（CANCELLED）",
}
MEDIA_STATE_MARKERS = (
    "画像を利用できません",
    "画像を表示する候補",
    "メディア画像",
    "画像を準備",
)
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


def screenshot_pair_stats(before: Path, after: Path) -> dict | None:
    """Measured pixel difference between the two PrintWindow captures.

    This records what the captures show; it does not by itself prove GPU composition. A media
    surface that stayed an empty panel changes only its status text, while a shown picture changes
    a large region with many distinct colours.
    """
    try:
        from PIL import Image, ImageChops
    except Exception:  # pragma: no cover - optional analysis dependency
        return None
    if not (before.is_file() and after.is_file()):
        return None
    with Image.open(before) as before_image, Image.open(after) as after_image:
        if before_image.size != after_image.size:
            return {
                "same_size": False,
                "before_size": list(before_image.size),
                "after_size": list(after_image.size),
            }
        left = before_image.convert("RGB")
        right = after_image.convert("RGB")
        difference = ImageChops.difference(left, right)
        box = difference.getbbox()
        changed = sum(1 for pixel in difference.getdata() if pixel != (0, 0, 0))
        distinct = 0
        if box is not None:
            distinct = len(set(difference.crop(box).getdata()))
        return {
            "same_size": True,
            "size": list(left.size),
            "changed_pixels": changed,
            "total_pixels": left.size[0] * left.size[1],
            "changed_bbox": list(box) if box is not None else None,
            "distinct_diff_colours": distinct,
        }


def free_port_fixed() -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.3)
        if probe.connect_ex(("127.0.0.1", PORT)) == 0:
            raise RuntimeError(f"port {PORT} is already in use; the harness owns {PORT}")


def pid_alive(pid: int) -> bool:
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
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http_json(
    method: str,
    url: str,
    *,
    body: dict | None = None,
    headers: dict | None = None,
    timeout: float = 30.0,
) -> tuple[int, dict | None]:
    data = None
    request_headers = {"Accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        request_headers["Content-Type"] = "application/json"
    if headers:
        request_headers.update(headers)
    request = urllib.request.Request(url, data=data, headers=request_headers, method=method)
    try:
        with _opener().open(request, timeout=timeout) as response:
            raw = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        raw = error.read()
        status = error.code
    try:
        document = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
        document = None
    return status, document


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


# --------------------------------------------------------------------------- handles


def b64encode(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")


def media_handle(
    *,
    secret: str,
    server_identity: str,
    epoch: str,
    principal: str,
    artifact_ref: str,
    representation: str,
    version: int,
    digest_hex: str,
    expires_at: int,
) -> str:
    payload = {
        "v": 1,
        "sid": server_identity,
        "epc": epoch,
        "sub": principal,
        "art": artifact_ref,
        "rep": representation,
        "ver": version,
        "dig": digest_hex,
        "exp": int(expires_at),
    }
    body = b64encode(json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8"))
    key = hashlib.sha256(secret.encode("utf-8")).digest()
    signature = b64encode(hmac.new(key, body.encode("ascii"), hashlib.sha256).digest())
    return f"mh1.{body}.{signature}"


def handle_path(handle: str) -> str:
    return f"/api/cos/media/{handle}/blob"


# --------------------------------------------------------------------------- Runtime


class RuntimeServer:
    def __init__(self, *, work: Path, db_copy: Path, secret: str, handle_secret: str):
        self.work = work
        self.db_copy = db_copy
        self.secret = secret
        self.handle_secret = handle_secret
        self.process: subprocess.Popen | None = None
        self.log_path = work / "runtime.log"
        self.log_handle = None
        self.config_path = work / "runtime-config.yaml"

    def _write_config(self) -> None:
        template = CONFIG_TEMPLATE.read_text(encoding="utf-8")
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

    def start(self, timeout: float = 120.0) -> dict:
        free_port_fixed()
        self._write_config()
        environment = {
            **os.environ,
            "ORCHESTRATOR_AUTH_SECRET": self.secret,
            "ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET": self.handle_secret,
            # Isolated, read-only view of the production artifacts: the DB and CAS
            # are workspace copies, so this run cannot write production state.
            "ORCHESTRATOR_COS_CAS_ROOT": str((self.work / "cas").as_posix()),
            "ORCHESTRATOR_COS_WORK_ROOT": str((self.work / "cos-work").as_posix()),
            "ORCHESTRATOR_COS_PROJECT_ROOT": str(RUNTIME_ROOT.as_posix()),
        }
        environment.pop("ORCHESTRATOR_AUTH_DISABLED", None)
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- start {now_iso()} ---\n".encode())
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
        status, hello = http_json("GET", f"{ENDPOINT}/api/cos/desktop/server")
        if status != 200 or not isinstance(hello, dict):
            raise RuntimeError(f"HELLO failed ({status})")
        server = hello["server"]
        return {
            "pid": self.process.pid,
            "server_identity": server["server_identity"],
            "server_build": server["server_build"],
            "epoch": server["epoch"],
            "protocol": server["protocol"],
            "schema": server["schema"],
        }

    def issue_credential(self) -> dict:
        status, document = http_json(
            "POST",
            f"{ENDPOINT}/api/cos/desktop/credentials",
            headers={"X-Auth-Secret": self.secret},
        )
        if status != 200 or not isinstance(document, dict):
            raise RuntimeError(f"credential issuance failed ({status})")
        return document

    def open_observation(self, token: str) -> dict:
        body = {
            "schema_version": 1,
            "request_id": uuid.uuid4().hex,
            "protocol": {"major": 2, "minor": 0},
            "schema": {"major": 2, "minor": 0},
            "scope": {"task_ids": [], "filters": [{"name": "view", "value": "all"}]},
            "retained_commands": [],
        }
        status, document = http_json(
            "POST",
            f"{ENDPOINT}/api/cos/desktop/observation",
            body=body,
            headers={"Authorization": f"Bearer {token}"},
        )
        if status != 200 or not isinstance(document, dict):
            raise RuntimeError(f"observation open failed ({status})")
        return document

    def descriptor_status(self, token: str, context: str, digest: str) -> tuple[int, dict | None]:
        return http_json(
            "GET",
            f"{ENDPOINT}/api/cos/media/{digest}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Cos-Context": context,
            },
        )

    def stop(self) -> dict:
        if self.process is None:
            return {"stopped": False}
        pid = self.process.pid
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                forced = kill_tree(pid)
                if self.log_handle is not None:
                    self.log_handle.close()
                    self.log_handle = None
                return {"pid": pid, "clean": False, "forced": forced}
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {"pid": pid, "clean": True, "exit": self.process.returncode}


# --------------------------------------------------------------------------- projection / DB


def projection_tasks(observation: dict) -> list[dict]:
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
                }
            )
    return tasks


def choose_expected(tasks: list[dict]) -> dict:
    for task in tasks:
        if task["title"]:
            return task
    raise RuntimeError("the live projection contains no TASK title")


def artifact_candidates(db_copy: Path) -> list[dict]:
    connection = sqlite3.connect(f"file:{db_copy.as_posix()}?mode=ro", uri=True)
    try:
        rows = connection.execute(
            "SELECT a.artifact_id, a.status, b.content_digest, b.length, b.media_type"
            " FROM artifacts AS a INNER JOIN artifact_blobs AS b ON b.blob_id = a.blob_id"
            " WHERE a.status = 'available' AND b.media_type LIKE 'image/%'"
            " ORDER BY a.artifact_id"
        ).fetchall()
    finally:
        connection.close()
    return [
        {
            "artifact_ref": str(row[0]),
            "status": str(row[1]),
            "digest": str(row[2]),
            "length": int(row[3]),
            "media_type": str(row[4]),
        }
        for row in rows
    ]


def choose_artifact(server: RuntimeServer, token: str, context: str, candidates: list[dict]) -> dict:
    """First task-linked, servable artifact whose ORIGINAL descriptor has a handle."""
    rejected = []
    for candidate in candidates[:72]:
        status, document = server.descriptor_status(token, context, candidate["digest"])
        if status != 200 or not isinstance(document, dict):
            rejected.append({"artifact_ref": candidate["artifact_ref"], "status": status})
            continue
        if document.get("availability") != "AVAILABLE" or not document.get("access_handle"):
            rejected.append(
                {
                    "artifact_ref": candidate["artifact_ref"],
                    "status": status,
                    "availability": document.get("availability"),
                }
            )
            continue
        if document.get("representation") != "ORIGINAL":
            rejected.append({"artifact_ref": candidate["artifact_ref"], "representation": True})
            continue
        return {
            **candidate,
            "descriptor": {
                key: value
                for key, value in document.items()
                if key
                in {
                    "descriptor_id",
                    "artifact_ref",
                    "artifact_digest",
                    "representation",
                    "representation_version",
                    "media_type",
                    "byte_length",
                    "width",
                    "height",
                    "availability",
                    "verification",
                    "server_identity",
                    "audience",
                    "expires_at_unix_ms",
                }
            },
            "handle_present": True,
            "probed": len(rejected) + 1,
            "rejected_sample": rejected[:6],
        }
    raise RuntimeError(f"no task-linked servable artifact was found ({len(rejected)} probed)")


# --------------------------------------------------------------------------- desktop probe


def probe(
    action: str,
    pid: int,
    out_path: Path,
    *,
    image_out: Path | None = None,
    name_match: str | None = None,
) -> dict:
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


def wait_window(pid: int, out_path: Path, timeout: float = 45.0) -> dict:
    deadline = time.monotonic() + timeout
    last: dict = {"found": False}
    while time.monotonic() < deadline:
        last = probe("dump", pid, out_path)
        if last.get("found") and last.get("title"):
            return last
        if not pid_alive(pid):
            break
        time.sleep(0.6)
    return last


def wait_uia(
    pid: int, out_path: Path, predicate, timeout: float, interval: float = 0.6
) -> tuple[dict, str]:
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


class DesktopApp:
    def __init__(self, *, work: Path, issuance_secret: str, localappdata: Path):
        self.work = work
        self.issuance_secret = issuance_secret
        self.localappdata = localappdata
        self.process: subprocess.Popen | None = None
        self.log_path = work / "desktop.log"
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
        # The app issues and stores its own credential: the stored principal is the
        # authenticated media audience. A fresh protected-store entry is ensured by
        # the caller with cmdkey before launch.
        environment["COS_DESKTOP_RUNTIME_ISSUANCE_SECRET"] = self.issuance_secret
        environment.pop("COS_DESKTOP_RUNTIME_TOKEN", None)
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
                result = probe("close", pid, self.work / "close.json")
                closed = bool(result.get("sent"))
        outcome = self.wait_exit()
        if not outcome["exited"]:
            outcome["forced"] = kill_tree(pid)
            outcome["exited"] = not pid_alive(pid)
        outcome["wm_close_sent"] = closed
        return outcome


# --------------------------------------------------------------------------- evidence helpers


MEDIA_HANDLE_RE = re.compile(r"mh1\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+")


def redact(text: str, secrets_list: list[str]) -> str:
    redacted = text
    for secret in secrets_list:
        if secret:
            redacted = redacted.replace(secret, "<redacted>")
    # An access handle is a capability, not just a path: no log may retain it.
    return MEDIA_HANDLE_RE.sub("mh1.<redacted>", redacted)


def bounded(text: str, limit: int = 8000) -> str:
    return text if len(text) <= limit else "...[truncated]...\n" + text[-limit:]


def sanitized_log(path: Path, secrets_list: list[str], limit: int = 8000) -> str:
    if not path.is_file():
        return ""
    text = path.read_text(encoding="utf-8", errors="replace")
    return bounded(redact(text, secrets_list), limit)


def write_json(path: Path, document: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def copy_database(source: Path, destination: Path) -> None:
    reader = sqlite3.connect(f"file:{source.as_posix()}?mode=ro", uri=True)
    writer = sqlite3.connect(destination)
    try:
        reader.backup(writer)
    finally:
        writer.close()
        reader.close()


CREDENTIAL_USER = "desktop-observation"
CREDENTIAL_SERVICE_PREFIX = "cos-desktop/observation"


def credential_targets(origin: str) -> list[str]:
    """Mirror of ``run_integration.credential_targets`` (this file is standalone).

    The keyring crate composes ``{user}.{service}``, so the stored target is
    ``desktop-observation.cos-desktop/observation/<sha256(origin)[:32]>``; the
    bare service string never matches it.
    """
    head = hashlib.sha256(origin.encode("utf-8")).hexdigest()[:32]
    return [
        f"{CREDENTIAL_USER}.{CREDENTIAL_SERVICE_PREFIX}/{head}",
        f"{CREDENTIAL_SERVICE_PREFIX}/{head}",
    ]


def _cmdkey_outcome(completed: subprocess.CompletedProcess[str]) -> tuple[str, str]:
    output = (completed.stdout + completed.stderr).strip()
    if completed.returncode == 0:
        return "deleted", output
    lowered = output.lower()
    if "not found" in lowered or "\u898b\u3064\u304b\u308a\u307e\u305b\u3093" in output:
        return "not_found", output
    return "failed", output


def cleanup_credential_store(origin: str) -> dict:
    """Mirror of ``run_integration.cleanup_credential_store``; never raises."""
    attempts: list[dict] = []
    deleted = False
    for target in credential_targets(origin):
        try:
            completed = subprocess.run(
                ["cmdkey", f"/delete:{target}"], capture_output=True, text=True, timeout=20
            )
        except (OSError, subprocess.SubprocessError) as error:
            attempts.append(
                {"target": target, "result": "error", "error": type(error).__name__}
            )
            continue
        result, output = _cmdkey_outcome(completed)
        deleted = deleted or result == "deleted"
        attempts.append(
            {
                "target": target,
                "exit": completed.returncode,
                "result": result,
                "output": output[:200],
            }
        )
    return {"origin": origin, "targets": attempts, "deleted": deleted}


# --------------------------------------------------------------------------- media request summary

MEDIA_REQUEST_RE = re.compile(r'GET /api/cos/media/(\S+) HTTP/1\.1"\s+(\d{3})')


def media_request_summary(log_path: Path, *, byte_offset: int) -> dict:
    """Attribute media-route requests from the Runtime access log to the app.

    Access-handle values are capabilities and are never recorded: a ``mh1.``
    path is counted only. Descriptor digests are canonical identities and may be
    recorded after percent-decoding.
    """
    if not log_path.is_file():
        return {"descriptor_requests": 0, "blob_requests": 0, "statuses": {}}
    with log_path.open("rb") as handle:
        handle.seek(max(0, byte_offset))
        text = handle.read().decode("utf-8", errors="replace")
    descriptor_digests: list[str] = []
    descriptor_statuses: dict[str, int] = {}
    blob_requests = 0
    blob_statuses: dict[str, int] = {}
    for path, status in MEDIA_REQUEST_RE.findall(text):
        if path.startswith("mh1.") or "/mh1." in path:
            blob_requests += 1
            blob_statuses[status] = blob_statuses.get(status, 0) + 1
        else:
            digest = urllib.parse.unquote(path)
            descriptor_digests.append(digest)
            descriptor_statuses[status] = descriptor_statuses.get(status, 0) + 1
    return {
        "descriptor_requests": len(descriptor_digests),
        "descriptor_digests": descriptor_digests,
        "descriptor_statuses": descriptor_statuses,
        "blob_requests": blob_requests,
        "blob_statuses": blob_statuses,
        "handle_values_recorded": False,
    }


def media_png_dimensions(cas_root: Path, digest: str) -> dict | None:
    hex_digest = digest.split(":", 1)[-1]
    if len(hex_digest) != 64 or not all(character in "0123456789abcdef" for character in hex_digest):
        return None
    path = cas_root / "blobs" / hex_digest[:2] / hex_digest
    if not path.is_file():
        return None
    with path.open("rb") as handle:
        header = handle.read(33)
    if len(header) < 33 or header[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    width = int.from_bytes(header[16:20], "big")
    height = int.from_bytes(header[20:24], "big")
    return {
        "width": width,
        "height": height,
        "bit_depth": header[24],
        "colour_type": header[25],
        "header_sha256_of_33_bytes": "sha256:" + hashlib.sha256(header).hexdigest(),
    }


def requested_artifact_facts(db_copy: Path, cas_root: Path, digests: list[str]) -> list[dict]:
    connection = sqlite3.connect(f"file:{db_copy.as_posix()}?mode=ro", uri=True)
    facts = []
    try:
        for digest in digests:
            row = connection.execute(
                "SELECT a.artifact_id, b.length, b.media_type"
                " FROM artifacts AS a INNER JOIN artifact_blobs AS b ON b.blob_id = a.blob_id"
                " WHERE b.content_digest = ?",
                (digest,),
            ).fetchone()
            facts.append(
                {
                    "digest": digest,
                    "artifact_ref": str(row[0]) if row else None,
                    "byte_length": int(row[1]) if row else None,
                    "media_type": str(row[2]) if row else None,
                    "png_header": media_png_dimensions(cas_root, digest),
                }
            )
    finally:
        connection.close()
    return facts


# --------------------------------------------------------------------------- live executor test
def run_executor_live_test(
    *,
    work: Path,
    out_path: Path,
    token: str,
    other_token: str,
    context: str,
    digest: str,
    server_identity: str,
    epoch: str,
    artifact_ref: str,
    audience: str,
    expired_handle: str,
    foreign_handle: str,
) -> dict:
    environment = {**os.environ}
    tmp = work / "tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    environment["TEMP"] = str(tmp)
    environment["TMP"] = str(tmp)
    environment.update(
        {
            "COS_DESKTOP_MEDIA_LIVE_ENDPOINT": ENDPOINT,
            "COS_DESKTOP_MEDIA_LIVE_TOKEN": token,
            "COS_DESKTOP_MEDIA_LIVE_OTHER_TOKEN": other_token,
            "COS_DESKTOP_MEDIA_LIVE_CONTEXT": context,
            "COS_DESKTOP_MEDIA_LIVE_DIGEST": digest,
            "COS_DESKTOP_MEDIA_LIVE_SERVER": server_identity,
            "COS_DESKTOP_MEDIA_LIVE_EPOCH": epoch,
            "COS_DESKTOP_MEDIA_LIVE_ARTIFACT": artifact_ref,
            "COS_DESKTOP_MEDIA_LIVE_AUDIENCE": audience,
            "COS_DESKTOP_MEDIA_LIVE_EXPIRED_HANDLE": handle_path(expired_handle),
            "COS_DESKTOP_MEDIA_LIVE_FOREIGN_HANDLE": handle_path(foreign_handle),
            "COS_DESKTOP_MEDIA_LIVE_OUT": str(out_path),
            # If any proxy were consulted, these dead proxies would fail the fetch.
            "HTTP_PROXY": "http://127.0.0.1:9",
            "HTTPS_PROXY": "http://127.0.0.1:9",
            "ALL_PROXY": "http://127.0.0.1:9",
            "NO_PROXY": "",
        }
    )
    command = [
        "cargo",
        "test",
        "--locked",
        "-q",
        "-p",
        "cos-desktop",
        "--bin",
        "cos-desktop",
        "media_executor::tests::live_media_evidence",
        "--",
        "--ignored",
        "--nocapture",
    ]
    completed = subprocess.run(
        command,
        cwd=str(DESKTOP_ROOT),
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=900,
    )
    output = (completed.stdout or "") + (completed.stderr or "")
    result: dict = {
        "command": "cargo test --locked -p cos-desktop --bin cos-desktop media_executor::tests::live_media_evidence -- --ignored --nocapture",
        "exit": completed.returncode,
        "produced_evidence": out_path.is_file(),
        "tail": bounded(redact(output, [token, other_token]), 3000),
    }
    if out_path.is_file():
        with contextlib.suppress(ValueError):
            result["observed"] = json.loads(out_path.read_text(encoding="utf-8"))
    return result


# --------------------------------------------------------------------------- run


def run_once() -> dict:
    started = now_iso()
    work = WORK_ROOT / LABEL
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True, exist_ok=True)
    evidence = EVIDENCE_DIR
    if evidence.exists():
        shutil.rmtree(evidence)
    evidence.mkdir(parents=True, exist_ok=True)

    run_secret = secrets.token_hex(24)
    handle_secret = secrets.token_hex(32)
    collected: list[str] = [run_secret, handle_secret]
    record: dict = {
        "label": LABEL,
        "started_at": started,
        "endpoint": ENDPOINT,
        "source": {},
        "runtime": {},
        "credentials": {},
        "observation": {},
        "artifact": {},
        "executor_live": {},
        "native": {},
        "scenarios": {},
        "residuals": [
            "PREVIEW stays UNSUPPORTED on this route (no transcode).",
            "THUMBNAIL/METADATA byte routes are not exercised by this slice.",
            "No EXIF/orientation interpretation; Orientation::Normal is the only admitted class.",
            "A real handle-TTL expiry and a Runtime-instance-change 410 are not separately measured.",
            "No product/UX, GPU residency or Present claim; PrintWindow evidence only.",
        ],
    }

    db_copy = work / "generation-copy.sqlite"
    copy_database(SOURCE_DB, db_copy)
    record["source"]["db_sha256"] = sha256_file(db_copy)
    record["source"]["db_bytes"] = db_copy.stat().st_size
    cas_copy = work / "cas"
    shutil.copytree(SOURCE_CAS, cas_copy)
    record["source"]["cas_copy_files"] = sum(
        1 for path in cas_copy.rglob("*") if path.is_file()
    )
    record["source"]["cas_copy_bytes"] = sum(
        path.stat().st_size for path in cas_copy.rglob("*") if path.is_file()
    )
    record["source"]["cas_source_sha256_marker"] = "per-blob sha256 verified by the live executor"

    localappdata = work / "localappdata"
    (localappdata / "cos-desktop").mkdir(parents=True, exist_ok=True)
    record["source"]["desktop_binary_sha256"] = (
        sha256_file(DESKTOP_EXE) if DESKTOP_EXE.is_file() else "missing"
    )
    record["source"]["runtime_python"] = str(RUNTIME_PYTHON)
    record["source"]["desktop_git"] = subprocess.run(
        ["git", "-C", str(DESKTOP_ROOT), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        timeout=20,
    ).stdout.strip()

    server = RuntimeServer(
        work=work, db_copy=db_copy, secret=run_secret, handle_secret=handle_secret
    )
    app: DesktopApp | None = None
    try:
        with native_lock("media-live/img6-rt6"):
            record["runtime"] = server.start()
            app_credential = server.issue_credential()
            other_credential = server.issue_credential()
            collected.extend([app_credential["token"], other_credential["token"]])
            record["credentials"] = {
                "desktop_principal": app_credential["principal"],
                "other_principal": other_credential["principal"],
                "scopes": app_credential["scopes"],
                "expires_in": app_credential["expires_in"],
                "token_recorded": False,
            }
            observation = server.open_observation(app_credential["token"])
            stamp = observation["grant"]["stamp"]
            tasks = projection_tasks(observation)
            expected = choose_expected(tasks)
            record["observation"] = {
                "scope_id": stamp["scope_id"],
                "read_revision": stamp["read_revision"],
                "server_id": stamp["server_id"],
                "epoch": stamp["epoch"],
                # The context id is a bearer capability and is never recorded.
                "context_recorded": False,
                "task_count": len(tasks),
                "expected_task_ref": expected["task_ref"],
                "expected_title": expected["title"],
            }
            candidates = artifact_candidates(db_copy)
            record["source"]["candidate_count"] = len(candidates)
            chosen = choose_artifact(
                server, app_credential["token"], stamp["context_id"], candidates
            )
            descriptor = chosen["descriptor"]
            verification = descriptor.get("verification") or {}
            expected_hex = str(verification.get("expected_digest") or "")
            record["artifact"] = {
                "artifact_ref": chosen["artifact_ref"],
                "digest": chosen["digest"],
                "db_media_type": chosen["media_type"],
                "db_length": chosen["length"],
                "probed_candidates": chosen["probed"],
                "probe_rejections_sample": chosen["rejected_sample"],
                "descriptor": descriptor,
                "descriptor_http_status": 200,
                "access_handle_recorded": False,
            }
            now_seconds = int(time.time())
            expired = media_handle(
                secret=handle_secret,
                server_identity=record["runtime"]["server_identity"],
                epoch=record["runtime"]["epoch"],
                principal=app_credential["principal"],
                artifact_ref=chosen["artifact_ref"],
                representation="ORIGINAL",
                version=int(descriptor.get("representation_version") or 1),
                digest_hex=chosen["digest"].split(":", 1)[1],
                expires_at=now_seconds - 10,
            )
            foreign = media_handle(
                secret=handle_secret,
                server_identity=record["runtime"]["server_identity"],
                epoch=record["runtime"]["epoch"],
                principal=other_credential["principal"],
                artifact_ref=chosen["artifact_ref"],
                representation="ORIGINAL",
                version=int(descriptor.get("representation_version") or 1),
                digest_hex=chosen["digest"].split(":", 1)[1],
                expires_at=now_seconds + 300,
            )
            live_out = work / "executor-live.json"
            record["executor_live"] = run_executor_live_test(
                work=work,
                out_path=live_out,
                token=app_credential["token"],
                other_token=other_credential["token"],
                context=stamp["context_id"],
                digest=chosen["digest"],
                server_identity=record["runtime"]["server_identity"],
                epoch=record["runtime"]["epoch"],
                artifact_ref=chosen["artifact_ref"],
                audience=app_credential["principal"],
                expired_handle=expired,
                foreign_handle=foreign,
            )
            observed = record["executor_live"].get("observed") or {}
            scenarios = record["scenarios"]

            def check(name: str, condition: bool, detail: dict) -> None:
                scenarios[name] = {"verdict": "PASS" if condition else "FAIL", **detail}

            check(
                "descriptor_closed_v2",
                descriptor.get("availability") == "AVAILABLE"
                and descriptor.get("server_identity") == record["runtime"]["server_identity"]
                and descriptor.get("audience") == app_credential["principal"],
                {
                    "availability": descriptor.get("availability"),
                    "server_identity_ok": descriptor.get("server_identity")
                    == record["runtime"]["server_identity"],
                    "audience_ok": descriptor.get("audience") == app_credential["principal"],
                },
            )
            check(
                "bytes_verified_and_admitted",
                bool(observed)
                and observed.get("fetched_bytes") == chosen["length"]
                and observed.get("byte_length") == chosen["length"]
                and observed.get("computed_digest") == expected_hex
                and observed.get("digest_matches_declared") is True
                and observed.get("admitted_verified") is True,
                {
                    "db_length": chosen["length"],
                    "descriptor_byte_length": observed.get("byte_length"),
                    "fetched_bytes": observed.get("fetched_bytes"),
                    "declared_digest": expected_hex or None,
                    "computed_digest": observed.get("computed_digest"),
                    "digest_matches_declared": observed.get("digest_matches_declared"),
                    "admitted_verified": observed.get("admitted_verified"),
                },
            )
            decoded = observed.get("decoded") or {}
            check(
                "decoded_bounded",
                bool(decoded) and int(decoded.get("rgba_bytes") or 0) > 0,
                {"decoded": decoded, "decode_error": observed.get("decode_error")},
            )
            check(
                "presentation_request_built",
                observed.get("presentation") == "request"
                and int(decoded.get("width") or 0) == int(observed.get("width") or 0)
                and int(decoded.get("height") or 0) == int(observed.get("height") or 0),
                {
                    "presentation": observed.get("presentation"),
                    "descriptor_geometry": {
                        "width": observed.get("width"),
                        "height": observed.get("height"),
                    },
                    "decoded_geometry": {
                        "width": decoded.get("width"),
                        "height": decoded.get("height"),
                    },
                },
            )
            negatives = observed.get("negatives") or {}
            check(
                "negative_matrix",
                negatives.get("descriptor_bogus_context") == "Err(Unauthorized)"
                and negatives.get("descriptor_foreign_principal") == "Err(Forbidden)"
                and negatives.get("expired_handle") == "Err(DescriptorExpired)"
                and negatives.get("foreign_handle") == "Err(Forbidden)",
                dict(negatives),
            )
            record["artifact"]["presentation_outcome"] = observed.get("presentation")

            # --- native artifact surface -------------------------------------
            runtime_log_before = (
                server.log_path.stat().st_size if server.log_path.is_file() else 0
            )
            cleanup_credential_store(ENDPOINT)
            app = DesktopApp(
                work=work, issuance_secret=run_secret, localappdata=localappdata
            )
            app.launch()
            native: dict = {"pid": app.process.pid if app.process else None}
            record["native"] = native
            window = wait_window(app.process.pid, work / "uia-window.json")
            native["window"] = {
                "found": window.get("found"),
                "title": window.get("title"),
                "width": window.get("width"),
                "height": window.get("height"),
            }
            media_marker = probe("dump", app.process.pid, work / "uia-connected.json")
            media_text = uia_text(media_marker)
            native["connected_uia_contains_title"] = expected["title"] in media_text
            native["connected_uia_contains_surface_statement"] = (
                "Runtime が認可した成果物メディア" in media_text
            )
            probe(
                "shot",
                app.process.pid,
                work / "shot-before.json",
                image_out=evidence / "artifact-surface-before.png",
            )
            dock_candidates = [
                name
                for name in (media_marker.get("names") or [])
                # A Dock row is "<task_ref> — 第N回 — <label> …"; the task rail
                # row shares the task title but not the iteration marker.
                if " — 第" in name
            ]
            native["dock_candidate_names"] = len(dock_candidates)
            invoked: dict = {"invoked": False}
            status_text = ""
            if dock_candidates:
                target = dock_candidates[0]
                native["invoked_name_prefix"] = target[:80]
                invoked = probe(
                    "invoke",
                    app.process.pid,
                    work / "uia-invoke.json",
                    name_match=target,
                )
                native["invoke"] = {
                    "invoked": invoked.get("invoked"),
                    "pattern": invoked.get("pattern"),
                    "patterns_tried": invoked.get("patterns_tried"),
                    "control_type": invoked.get("matched_control_type"),
                    "error": invoked.get("error"),
                }
                if invoked.get("invoked"):
                    dump, status_text = wait_uia(
                        app.process.pid,
                        work / "uia-media-status.json",
                        lambda joined: any(marker in joined for marker in MEDIA_STATE_MARKERS),
                        45.0,
                    )
                    native["status_uia_elements"] = dump.get("elements")
            native["media_status_present"] = any(
                marker in status_text for marker in MEDIA_STATE_MARKERS
            )
            native["media_status_excerpt"] = bounded(
                "\n".join(
                    line
                    for line in status_text.splitlines()
                    if any(marker in line for marker in MEDIA_STATE_MARKERS)
                ),
                1200,
            )
            probe(
                "shot",
                app.process.pid,
                work / "shot-after.json",
                image_out=evidence / "artifact-surface-media.png",
            )
            native["screenshots"] = {
                name: sha256_file(evidence / name)
                for name in ("artifact-surface-before.png", "artifact-surface-media.png")
                if (evidence / name).is_file()
            }
            native["screenshot_diff"] = screenshot_pair_stats(
                evidence / "artifact-surface-before.png",
                evidence / "artifact-surface-media.png",
            )
            closed = app.close()
            native["close"] = closed
            native["media_requests"] = media_request_summary(
                server.log_path, byte_offset=runtime_log_before
            )
            requested_digests = (native["media_requests"] or {}).get("descriptor_digests") or []
            native["media_requests"]["requested_artifacts"] = requested_artifact_facts(
                db_copy, cas_copy, requested_digests
            )
            log_text = sanitized_log(app.log_path, collected)
            record["native"]["stderr_sanitized"] = log_text
            record["native"]["close_markers"] = [
                marker for marker in CLOSE_MARKERS if marker in log_text
            ]
            check(
                "native_surface_runtime_binding",
                bool(window.get("found"))
                and native.get("connected_uia_contains_title") is True
                and native.get("connected_uia_contains_surface_statement") is True,
                {
                    "window_found": window.get("found"),
                    "title_visible": native.get("connected_uia_contains_title"),
                    "runtime_statement_visible": native.get(
                        "connected_uia_contains_surface_statement"
                    ),
                },
            )
            if native.get("media_status_present"):
                verdict = "PASS"
                state = native.get("media_status_excerpt")
            elif not dock_candidates:
                verdict = "NOT_MEASURED"
                state = "no Dock candidate button was exposed to UIA"
            else:
                verdict = "NOT_MEASURED"
                state = "the Dock candidate invocation did not surface a media status"
            scenarios["native_media_surface"] = {
                "verdict": verdict,
                "observed_state": state,
                "invoked": invoked.get("invoked"),
                "close_markers": record["native"]["close_markers"],
            }
            requests = native.get("media_requests") or {}
            app_descriptor_requests = int(requests.get("descriptor_requests") or 0)
            app_blob_requests = int(requests.get("blob_requests") or 0)
            app_descriptor_ok = "200" in (requests.get("descriptor_statuses") or {})
            app_blob_ok = "200" in (requests.get("blob_statuses") or {})
            scenarios["native_production_call_site"] = {
                "verdict": (
                    "PASS"
                    if app_descriptor_requests >= 1
                    and app_blob_requests >= 1
                    and app_descriptor_ok
                    and app_blob_ok
                    else ("NOT_MEASURED" if app_descriptor_requests == 0 else "FAIL")
                ),
                "descriptor_requests": app_descriptor_requests,
                "descriptor_statuses": requests.get("descriptor_statuses"),
                "blob_requests": app_blob_requests,
                "blob_statuses": requests.get("blob_statuses"),
                "attribution": "Runtime access log after the app launch; handle values never recorded",
            }
            facts = (requests.get("requested_artifacts") or [{}])[0]
            png = facts.get("png_header") or {}
            if any(
                marker in status_text
                for marker in ("表示できます", "メディア画像（", "実験用タイル表示")
            ):
                rendered_verdict = "PASS"
                rendered_state = native.get("media_status_excerpt")
            elif "画像寸法は検証済みの範囲外" in status_text:
                rendered_verdict = "NOT_MEASURED"
                rendered_state = (
                    "the admitted artifact is %sx%s (PNG bit depth %s, colour type %s); "
                    "the admitted presentation class is the explicit square set "
                    "media::APPROVED_EDGES = [320, 1024, 2048], so the surface refused "
                    "instead of fabricating a picture"
                    % (
                        png.get("width"),
                        png.get("height"),
                        png.get("bit_depth"),
                        png.get("colour_type"),
                    )
                )
            else:
                rendered_verdict = "NOT_MEASURED"
                rendered_state = "the surface showed no picture or explicit picture state"
            scenarios["native_image_rendered"] = {
                "verdict": rendered_verdict,
                "observed_state": rendered_state,
                "requested_artifact": facts,
                "status_excerpt": native.get("media_status_excerpt"),
            }
            if rendered_verdict == "PASS":
                record["residuals"].append(
                    "The shown picture is the admitted preview mip (viewer PreviewOnly mode); "
                    "product full-detail, GPU residency and Present acceptance remain open."
                )
            else:
                record["residuals"].append(
                    "The live artifact was admitted and decoded but the presentation surface did "
                    "not show a picture; the observed state is recorded, not fabricated."
                )

            # --- shutdown / evidence ----------------------------------------
            server_stop = server.stop()
            record["runtime"]["stop"] = server_stop
            checked = [
                item
                for item in scenarios.values()
                if item.get("verdict") in {"PASS", "FAIL"}
            ]
            failures = [name for name, item in scenarios.items() if item.get("verdict") == "FAIL"]
            record["verdict"] = "PASS" if not failures else "FAIL"
            record["failures"] = failures
            record["scenario_count"] = len(checked)
            record["finished_at"] = now_iso()
            record["evidence_files"] = sorted(
                path.name for path in evidence.iterdir() if path.is_file()
            )
            record["runtime_log_sanitized"] = sanitized_log(server.log_path, collected)
            record["_secrets"] = collected
            return record
    finally:
        if app is not None and app.process is not None and pid_alive(app.process.pid):
            kill_tree(app.process.pid)
        with contextlib.suppress(Exception):
            server.stop()
        cleanup_credential_store(ENDPOINT)


def configure(argv: list[str] | None = None) -> None:
    """Bind the run's port, work label and evidence directory.

    Defaults keep the original stage-3 run unchanged (8127 / run1 / img6-rt6-latest); a follow-up
    slice passes its own port, label and time-stamped evidence directory so earlier evidence is
    never overwritten.
    """
    global PORT, ENDPOINT, LABEL, EVIDENCE_DIR
    parser = argparse.ArgumentParser(
        description="Isolated Desktop<->Runtime media integration evidence run."
    )
    parser.add_argument(
        "--port",
        type=int,
        default=int(os.environ.get("COS_MEDIA_PORT", str(PORT))),
        help="loopback port the harness owns (default 8127)",
    )
    parser.add_argument(
        "--label",
        default=os.environ.get("COS_MEDIA_LABEL", LABEL),
        help="work-directory label under target/media-live (default run1)",
    )
    parser.add_argument(
        "--evidence-dir",
        default=os.environ.get("COS_MEDIA_EVIDENCE_DIR", str(EVIDENCE_DIR)),
        help="evidence output directory (default img6-rt6-latest)",
    )
    args = parser.parse_args(argv)
    PORT = args.port
    ENDPOINT = f"http://127.0.0.1:{PORT}"
    LABEL = args.label
    EVIDENCE_DIR = Path(args.evidence_dir)


def main(argv: list[str] | None = None) -> int:
    configure(argv)
    if not DESKTOP_EXE.is_file():
        raise RuntimeError("build the release Desktop first: cargo build --locked --release -p cos-desktop")
    record = run_once()
    secrets_list = record.pop("_secrets", [])
    evidence = EVIDENCE_DIR
    write_json(evidence / "record.json", record)
    session = {
        "label": record["label"],
        "started_at": record["started_at"],
        "finished_at": record["finished_at"],
        "verdict": record["verdict"],
        "failures": record["failures"],
        "source": record["source"],
        "runtime": {
            key: value
            for key, value in record.get("runtime", {}).items()
            if key in {"pid", "server_identity", "server_build", "epoch", "protocol", "schema", "stop"}
        },
        "credentials": record.get("credentials"),
        "observation": record.get("observation"),
        "artifact": {
            key: value
            for key, value in record.get("artifact", {}).items()
            if key != "descriptor"
        },
        "executor_live": {
            "exit": record.get("executor_live", {}).get("exit"),
            "produced_evidence": record.get("executor_live", {}).get("produced_evidence"),
            "command": record.get("executor_live", {}).get("command"),
        },
        "native": {
            key: value
            for key, value in record.get("native", {}).items()
            if key != "stderr_sanitized"
        },
        "scenarios": record.get("scenarios"),
        "residuals": record.get("residuals"),
        "evidence_files": record.get("evidence_files"),
    }
    write_json(evidence / "session.json", session)
    (evidence / "descriptor.json").write_text(
        json.dumps(record.get("artifact", {}).get("descriptor") or {}, indent=2, ensure_ascii=False)
        + "\n",
        encoding="utf-8",
    )
    observed = record.get("executor_live", {}).get("observed") or {}
    (evidence / "executor-observations.json").write_text(
        json.dumps(observed, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    (evidence / "runtime.log.sanitized.txt").write_text(
        record.get("runtime_log_sanitized") or "", encoding="utf-8"
    )
    (evidence / "desktop.log.sanitized.txt").write_text(
        record.get("native", {}).get("stderr_sanitized") or "", encoding="utf-8"
    )
    leaks = []
    for path in sorted(evidence.iterdir()):
        if path.suffix == ".png" or not path.is_file():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        if MEDIA_HANDLE_RE.search(text):
            leaks.append({"file": path.name, "kind": "media_handle"})
        for secret in secrets_list:
            if secret and secret in text:
                leaks.append({"file": path.name, "kind": "secret"})
    record["leak_check"] = {"clean": not leaks, "findings": leaks}
    if leaks:
        write_json(evidence / "record.json", record)
        print("verdict=FAIL leak_check findings=%d evidence=%s" % (len(leaks), evidence))
        return 1
    write_json(evidence / "record.json", record)
    print(
        "verdict=%s scenarios=%d executor_live_exit=%s evidence=%s"
        % (
            record.get("verdict"),
            len(record.get("scenarios") or {}),
            record.get("executor_live", {}).get("exit"),
            evidence,
        )
    )
    return 0 if record.get("verdict") == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
