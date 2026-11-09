#!/usr/bin/env python3
"""RT3 media-expiry residual measurement (RC-13 handle TTL / instance epoch).

One isolated run measures the two residuals the RT6/RC-13 campaign left
``NOT_MEASURED`` at the protocol level:

1. a real handle-TTL expiry (``ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS=60``):
   descriptor -> immediate blob 200 -> wait past the TTL -> same handle 410
   ``MEDIA_HANDLE_EXPIRED`` -> a fresh descriptor mints a working new handle;
2. a Runtime-instance change: a still-valid handle is presented to a restarted
   Runtime (new observation epoch, same per-run handle secret) -> 410
   ``MEDIA_HANDLE_EXPIRED`` -> a fresh descriptor after re-authentication works.

The Desktop side is exercised through the real binary-boundary executor: the
ignored ``media_executor::tests::live_media_evidence`` test is invoked twice
(after the TTL expiry and after the epoch change) with the real handle paths, so
the client mapping of the *live* 410 is recorded as ``Err(DescriptorExpired)``
and its availability projection as ``Expired``. The refetch (rather than
``Missing``) path is pinned by the bounded stub-server executor test in
``apps/desktop/src/media_executor.rs``; both results are recorded here.

Isolation: fresh read-only copy of ``db/generation.sqlite`` and ``db/cas`` under
``target/rt3-residual/run1``, isolated config/output/log roots, own port 8160,
per-run auth and handle secrets held in memory only, and a redirected discovery
descriptor (``COS_RUNTIME_DISCOVERY_PATH``). No product code runs a generation:
the harness only reads existing projections/artifacts. Tokens, context ids and
handle values are never written to disk; runtime logs are handle-redacted.
"""

from __future__ import annotations

import argparse
import base64
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
import traceback
import urllib.error
import urllib.request
import uuid
from pathlib import Path

DESKTOP_ROOT = Path(__file__).resolve().parents[4]
RUNTIME_ROOT = Path("C:/AI/orchestrator")
RUNTIME_PYTHON = RUNTIME_ROOT / ".venv" / "Scripts" / "python.exe"
LAUNCHER = Path(__file__).resolve().parent / "isolated_runtime.py"
SOURCE_DB = RUNTIME_ROOT / "db" / "generation.sqlite"
SOURCE_CAS = RUNTIME_ROOT / "db" / "cas"
CONFIG_TEMPLATE = RUNTIME_ROOT / "configs" / "default.yaml"

EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "rt3-residual-latest"
)
WORK_ROOT = DESKTOP_ROOT / "target" / "rt3-residual"
WORK = WORK_ROOT / "run1"

PORT = 8160
ENDPOINT = f"http://127.0.0.1:{PORT}"
TTL_SECONDS = 60
TTL_WAIT_SECONDS = 65
HANDLE_PATTERN = re.compile(r"mh1\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
HANDLE_PATH_PATTERN = re.compile(
    r"/api/cos/media/mh1\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/(?:blob|thumbnail)"
)

DESCRIPTOR_EXCLUDED = {"access_handle"}


# --------------------------------------------------------------------------- basics


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def now_ms() -> int:
    return int(time.time() * 1000)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def sha256_bytes(payload: bytes) -> str:
    return "sha256:" + hashlib.sha256(payload).hexdigest()


def copy_database(source: Path, destination: Path) -> None:
    reader = sqlite3.connect(f"file:{source.as_posix()}?mode=ro", uri=True)
    writer = sqlite3.connect(destination)
    try:
        reader.backup(writer)
    finally:
        writer.close()
        reader.close()


def ensure_port_free(port: int) -> None:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.settimeout(0.5)
        if probe.connect_ex(("127.0.0.1", port)) == 0:
            raise RuntimeError(f"port {port} is already in use")
    finally:
        probe.close()


def port_is_closed(port: int) -> bool:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) != 0
    finally:
        probe.close()


def git_value(*args: str) -> str:
    completed = subprocess.run(
        ["git", *args], capture_output=True, text=True, timeout=30
    )
    # Only drop the trailing newline: leading porcelain status columns are
    # significant, and the first line can carry a UTF-8 BOM from the console.
    return (completed.stdout or "").lstrip("\ufeff").rstrip("\r\n")


def header_value(headers: dict, name: str) -> str | None:
    """Case-insensitive response header lookup (ASGI sends lower-case names)."""
    wanted = name.lower()
    for key, value in headers.items():
        if key.lower() == wanted:
            return value
    return None


def _opener() -> urllib.request.OpenerDirector:
    # No environment proxy: the media path is same-authority loopback only.
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http(
    method: str,
    url: str,
    *,
    body: dict | None = None,
    headers: dict | None = None,
    timeout: float = 60.0,
) -> tuple[int, bytes, dict]:
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
            return response.status, response.read(), dict(response.headers)
    except urllib.error.HTTPError as error:
        return error.code, error.read(), dict(error.headers)


def http_json(
    method: str,
    url: str,
    *,
    body: dict | None = None,
    headers: dict | None = None,
    timeout: float = 60.0,
) -> tuple[int, dict | None]:
    status, raw, _headers = http(method, url, body=body, headers=headers, timeout=timeout)
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
        time.sleep(0.2)
    raise RuntimeError(f"{url} did not become ready: {last}")


# --------------------------------------------------------------------------- handles


def b64encode(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")


def b64decode(text: str) -> bytes:
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def decode_handle(handle: str) -> dict:
    """Decode a minted capability for measurement only (never write the value)."""
    token = handle.rsplit("/", 2)[-2] if "/" in handle else handle
    parts = token.split(".")
    if len(parts) != 3 or parts[0] != "mh1":
        return {"decodable": False}
    payload = json.loads(b64decode(parts[1]).decode("utf-8"))
    return {"decodable": True, "payload": payload}


def handle_path(handle: str) -> str:
    return handle if handle.startswith("/") else f"/api/cos/media/{handle}/blob"


def wait_past_expiry(descriptor: dict, issued_at: float) -> dict:
    expires_at_ms = int(descriptor["expires_at_unix_ms"])
    target_epoch = max(expires_at_ms / 1000.0 + 2.0, issued_at + TTL_WAIT_SECONDS)
    started = time.monotonic()
    while time.time() < target_epoch:
        time.sleep(min(1.0, max(0.05, target_epoch - time.time())))
    return {
        "waited_seconds": round(time.monotonic() - started, 3),
        "seconds_since_issuance": round(time.time() - issued_at, 3),
        "seconds_past_server_expiry": round(time.time() - expires_at_ms / 1000.0, 3),
    }


# --------------------------------------------------------------------------- runtime


class RuntimeInstance:
    def __init__(
        self,
        *,
        label: str,
        db_copy: Path,
        cas_copy: Path,
        secret: str,
        handle_secret: str,
    ):
        self.label = label
        self.db_copy = db_copy
        self.cas_copy = cas_copy
        self.secret = secret
        self.handle_secret = handle_secret
        self.work = WORK / label
        self.work.mkdir(parents=True, exist_ok=True)
        self.config_path = self.work / "runtime-config.yaml"
        self.log_path = self.work / "runtime.log"
        self.stop_file = self.work / "runtime.stop"
        self.ready_file = self.work / "runtime.ready"
        self.discovery_path = self.work / "runtime-discovery.json"
        self.process: subprocess.Popen | None = None
        self.log_handle = None
        self.started_at = 0.0
        self.facts: dict = {}

    def _write_config(self) -> None:
        template = CONFIG_TEMPLATE.read_text(encoding="utf-8")
        replacements = [
            ("db_path", self.db_copy.as_posix()),
            ("project_root", RUNTIME_ROOT.as_posix()),
            ("output_root", (self.work / "outputs").as_posix()),
            ("log_dir", (self.work / "logs").as_posix()),
        ]
        rendered = template
        for key, value in replacements:
            rendered, count = re.subn(
                rf"(?m)^(\s*){key}:.*$",
                lambda match, value=value: f"{match.group(1)}{key}: {value}",
                rendered,
                count=1,
            )
            if count != 1:
                raise RuntimeError(f"default.yaml no longer contains exactly one {key} line")
        self.config_path.write_text(rendered, encoding="utf-8")
        (self.work / "outputs").mkdir(parents=True, exist_ok=True)
        (self.work / "logs").mkdir(parents=True, exist_ok=True)

    def start(self) -> dict:
        ensure_port_free(PORT)
        self._write_config()
        environment = {
            **os.environ,
            "ORCHESTRATOR_AUTH_SECRET": self.secret,
            "ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET": self.handle_secret,
            "ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS": str(TTL_SECONDS),
            "COS_RUNTIME_ORIGIN": ENDPOINT,
            "COS_RUNTIME_DISCOVERY_PATH": str(self.discovery_path),
            # The DB and CAS are workspace copies: this run cannot write production state.
            "ORCHESTRATOR_COS_CAS_ROOT": str(self.cas_copy.as_posix()),
            "ORCHESTRATOR_COS_WORK_ROOT": str((self.work / "cos-work").as_posix()),
            "ORCHESTRATOR_COS_PROJECT_ROOT": str(RUNTIME_ROOT.as_posix()),
        }
        environment.pop("ORCHESTRATOR_AUTH_DISABLED", None)
        environment.pop("COS_RUNTIME_DISCOVERY_DISABLED", None)
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- start {now_iso()} ---\n".encode())
        self.log_handle.flush()
        command = [
            str(RUNTIME_PYTHON),
            str(LAUNCHER),
            "--config",
            str(self.config_path),
            "--host",
            "127.0.0.1",
            "--port",
            str(PORT),
            "--stop-file",
            str(self.stop_file),
            "--ready-file",
            str(self.ready_file),
        ]
        started = time.monotonic()
        self.process = subprocess.Popen(  # noqa: S603 - fixed argv, loopback only
            command,
            cwd=str(RUNTIME_ROOT),
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        self.started_at = time.time()
        deadline = started + 120
        while True:
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"Runtime {self.label} exited during startup (exit {self.process.returncode})"
                )
            if self.ready_file.is_file():
                break
            if time.monotonic() > deadline:
                raise RuntimeError(f"Runtime {self.label} readiness timed out")
            time.sleep(0.1)
        wait_http(f"{ENDPOINT}/api/ready", 30.0)
        status, hello = http_json("GET", f"{ENDPOINT}/api/cos/desktop/server")
        if status != 200 or not isinstance(hello, dict):
            raise RuntimeError(f"HELLO failed ({status})")
        server = hello["server"]
        discovery = self.wait_discovery(20.0)
        self.facts = {
            "label": self.label,
            "pid": self.process.pid,
            "endpoint": ENDPOINT,
            "server_identity": server["server_identity"],
            "server_build": server["server_build"],
            "epoch": server["epoch"],
            "protocol": server["protocol"],
            "schema": server["schema"],
            "discovery": discovery,
            "startup_seconds": round(time.time() - self.started_at, 3),
        }
        return self.facts

    def wait_discovery(self, timeout: float) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.discovery_path.is_file():
                raw = self.discovery_path.read_bytes()
                document = json.loads(raw.decode("utf-8"))
                return {
                    "present": True,
                    "sha256": sha256_bytes(raw),
                    "bytes": len(raw),
                    "origin": document.get("origin"),
                    "server_identity": document.get("server_identity"),
                    "epoch": document.get("epoch"),
                    "ready": document.get("ready"),
                    "pid": document.get("pid"),
                    "launcher_pid": self.process.pid,
                    "pid_matches_launcher": int(document.get("pid") or 0)
                    == int(self.process.pid),
                    "pid_note": (
                        "the uv-managed venv python.exe is a launcher shim, so the "
                        "launcher pid and the serving interpreter pid can differ"
                    ),
                }
            time.sleep(0.1)
        return {"present": False}

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

    def descriptor(self, token: str, context: str, digest: str) -> tuple[int, dict | None]:
        timeout_headers = {
            "Authorization": f"Bearer {token}",
            "X-Cos-Context": context,
        }
        status, raw, _headers = http(
            "GET",
            f"{ENDPOINT}/api/cos/media/{digest}",
            headers=timeout_headers,
        )
        try:
            document = json.loads(raw.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            document = None
        return status, document

    def fetch_blob(self, token: str, descriptor: dict, *, label: str) -> dict:
        handle = str(descriptor.get("access_handle") or "")
        path = handle_path(handle)
        started = time.monotonic()
        status, raw, headers = http(
            "GET",
            f"{ENDPOINT}{path}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Media-Handle": path,
                "Accept": str(descriptor.get("media_type") or "*/*"),
            },
            timeout=120.0,
        )
        elapsed = round((time.monotonic() - started) * 1000.0, 3)
        record = {
            "label": label,
            "status": status,
            "elapsed_ms": elapsed,
            "response_bytes": len(raw),
            "content_length_header": header_value(headers, "content-length"),
            "content_type": header_value(headers, "content-type"),
            "at": now_iso(),
            "at_unix_ms": now_ms(),
        }
        if status == 200:
            record["body_sha256"] = sha256_bytes(raw)
            record["declared_byte_length"] = descriptor.get("byte_length")
            record["length_matches_descriptor"] = (
                int(descriptor.get("byte_length") or -1) == len(raw)
            )
        else:
            try:
                body = json.loads(raw.decode("utf-8"))
            except (ValueError, UnicodeDecodeError):
                body = None
            record["body"] = body
            record["body_code"] = body.get("code") if isinstance(body, dict) else None
        return record

    def stop(self) -> dict:
        if self.process is None:
            return {"stopped": False}
        pid = self.process.pid
        self.stop_file.write_text("stop", encoding="utf-8")
        try:
            self.process.wait(timeout=45)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=20)
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        deadline = time.monotonic() + 10.0
        while self.discovery_path.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        closed = port_is_closed(PORT)
        log_text = ""
        if self.log_path.is_file():
            log_text = self.log_path.read_text(encoding="utf-8", errors="replace")
        serving = re.search(r"Started server process \[(\d+)\]", log_text)
        return {
            "pid": pid,
            "serving_pid": int(serving.group(1)) if serving else None,
            "exit": self.process.returncode,
            "clean_exit": self.process.returncode == 0,
            "port_closed": closed,
            "discovery_removed": not self.discovery_path.exists(),
            "log": log_text,
        }


# --------------------------------------------------------------------------- artifacts


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


def projection_tasks(observation: dict) -> list[dict]:
    tasks: list[dict] = []
    snapshot = observation.get("snapshot") or {}
    for chunk in snapshot.get("chunks", []):
        for mutation in chunk.get("mutations", []):
            if mutation.get("op") != "UPSERT":
                continue
            projection = mutation.get("projection") or {}
            if projection.get("projection") == "TASK":
                tasks.append(
                    {
                        "task_ref": projection.get("task_ref"),
                        "title": projection.get("title") or "",
                        "task_status": projection.get("task_status"),
                    }
                )
    return tasks


def descriptor_summary(descriptor: dict) -> dict:
    return {
        key: value
        for key, value in descriptor.items()
        if key not in DESCRIPTOR_EXCLUDED
    } | {
        "access_handle_present": bool(descriptor.get("access_handle")),
        "access_handle_length": len(str(descriptor.get("access_handle") or "")),
    }


def choose_artifact(
    server: RuntimeInstance, token: str, context: str, candidates: list[dict]
) -> dict:
    rejected: list[dict] = []
    for candidate in candidates[:72]:
        status, document = server.descriptor(token, context, candidate["digest"])
        if status != 200 or not isinstance(document, dict):
            rejected.append({"artifact_ref": candidate["artifact_ref"], "status": status})
            continue
        if (
            document.get("availability") != "AVAILABLE"
            or not document.get("access_handle")
            or document.get("representation") != "ORIGINAL"
        ):
            rejected.append(
                {
                    "artifact_ref": candidate["artifact_ref"],
                    "status": status,
                    "availability": document.get("availability"),
                    "representation": document.get("representation"),
                }
            )
            continue
        return {
            **candidate,
            "descriptor": document,
            "probed": len(rejected) + 1,
            "rejected_sample": rejected[:6],
        }
    raise RuntimeError(f"no task-linked servable image artifact was found ({len(rejected)} probed)")


# --------------------------------------------------------------------------- live executor


def run_live_executor(
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
    secrets_list: list[str],
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
    started = time.monotonic()
    completed = subprocess.run(
        command,
        cwd=str(DESKTOP_ROOT),
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=1800,
    )
    elapsed = round(time.monotonic() - started, 3)
    output = (completed.stdout or "") + (completed.stderr or "")
    for secret in secrets_list:
        output = output.replace(secret, "<redacted>")
    result: dict = {
        "command": (
            "cargo test --locked -q -p cos-desktop --bin cos-desktop "
            "media_executor::tests::live_media_evidence -- --ignored --nocapture"
        ),
        "exit": completed.returncode,
        "elapsed_seconds": elapsed,
        "produced_evidence": out_path.is_file(),
        "tail": output[-2500:],
        "handle_recorded": False,
    }
    if out_path.is_file():
        observed = json.loads(out_path.read_text(encoding="utf-8"))
        result["observed"] = observed
    return result


# --------------------------------------------------------------------------- evidence


def write_json(path: Path, document: dict) -> None:
    path.write_text(
        json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )


def sanitized_log(log_text: str, secrets_list: list[str]) -> tuple[str, int]:
    text = log_text
    for secret in secrets_list:
        text = text.replace(secret, "<redacted>")
    text, handle_count = HANDLE_PATH_PATTERN.subn("/api/cos/media/<handle-redacted>/blob", text)
    text, bare_count = HANDLE_PATTERN.subn("<handle-redacted>", text)
    return text, handle_count + bare_count


def leak_scan(evidence: Path, secrets_list: list[str]) -> dict:
    findings: list[str] = []
    for path in sorted(evidence.rglob("*")):
        if not path.is_file() or path.suffix not in {".json", ".md", ".txt", ".sha256"}:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for secret in secrets_list:
            if secret and secret in text:
                findings.append(f"{path.name}: secret material")
        if HANDLE_PATTERN.search(text):
            findings.append(f"{path.name}: access-handle value")
        if '"token"' in text:
            findings.append(f"{path.name}: token field")
    return {"files": sum(1 for p in evidence.rglob("*") if p.is_file()), "findings": findings}


# --------------------------------------------------------------------------- run


def run_once(*, keep_work: bool) -> dict:
    started = now_iso()
    if WORK.exists():
        shutil.rmtree(WORK)
    WORK.mkdir(parents=True, exist_ok=True)
    if EVIDENCE_DIR.exists():
        shutil.rmtree(EVIDENCE_DIR)
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    print(f"[{now_iso()}] preparing isolated copy", flush=True)

    run_secret = secrets.token_hex(24)
    handle_secret = secrets.token_hex(32)
    secrets_list = [run_secret, handle_secret]
    record: dict = {
        "label": "run1",
        "started_at": started,
        "endpoint": ENDPOINT,
        "ttl_seconds": TTL_SECONDS,
        "source": {},
        "desktop": {},
        "runtime": {},
        "measurements": {},
        "executor": {},
        "cleanup": {},
        "residuals": [],
    }
    cleanup_notes: list[str] = []

    db_copy = WORK / "generation-copy.sqlite"
    cas_copy = WORK / "cas"
    copy_database(SOURCE_DB, db_copy)
    shutil.copytree(SOURCE_CAS, cas_copy)
    record["source"] = {
        "db_source": str(SOURCE_DB),
        "db_source_sha256": sha256_file(SOURCE_DB),
        "db_source_bytes": SOURCE_DB.stat().st_size,
        "db_copy_sha256_at_start": sha256_file(db_copy),
        "db_copy_bytes_at_start": db_copy.stat().st_size,
        "cas_source": str(SOURCE_CAS),
        "cas_copy_files": sum(1 for path in cas_copy.rglob("*") if path.is_file()),
        "cas_copy_bytes": sum(
            path.stat().st_size for path in cas_copy.rglob("*") if path.is_file()
        ),
    }
    desktop_status = git_value("-C", str(DESKTOP_ROOT), "status", "--porcelain=v1")
    record["desktop"] = {
        "root": str(DESKTOP_ROOT),
        "head": git_value("-C", str(DESKTOP_ROOT), "rev-parse", "HEAD"),
        "head_tree": git_value("-C", str(DESKTOP_ROOT), "rev-parse", "HEAD^{tree}"),
        "dirty_paths": [line[3:] for line in desktop_status.splitlines() if len(line) > 3],
        "executor_source_sha256": sha256_file(
            DESKTOP_ROOT / "apps" / "desktop" / "src" / "media_executor.rs"
        ),
    }
    runtime_status = git_value("-C", str(RUNTIME_ROOT), "status", "--porcelain=v1")
    record["runtime"] = {
        "root": str(RUNTIME_ROOT),
        "head": git_value("-C", str(RUNTIME_ROOT), "rev-parse", "HEAD"),
        "python": str(RUNTIME_PYTHON),
        "python_version": sys.version.split()[0],
        "dirty_paths": [line[3:] for line in runtime_status.splitlines() if len(line) > 3],
        "instance": {},
    }

    instance = RuntimeInstance(
        label="runtime-1",
        db_copy=db_copy,
        cas_copy=cas_copy,
        secret=run_secret,
        handle_secret=handle_secret,
    )
    record["runtime"]["instance"]["first"] = instance.start()
    print(f"[{now_iso()}] runtime-1 started epoch={instance.facts['epoch']}", flush=True)
    token = ""
    other_token = ""
    stopped = False
    try:
        credential = instance.issue_credential()
        token = credential["token"]
        other_credential = instance.issue_credential()
        other_token = other_credential["token"]
        secrets_list.extend([token, other_token])
        observation = instance.open_observation(token)
        stamp = observation["grant"]["stamp"]
        context = stamp["context_id"]
        secrets_list.append(context)
        tasks = projection_tasks(observation)
        candidates = artifact_candidates(db_copy)
        chosen = choose_artifact(instance, token, context, candidates)
        descriptor = chosen["descriptor"]
        record["runtime"]["instance"]["first"]["credential"] = {
            "principal": credential["principal"],
            "other_principal": other_credential["principal"],
            "scopes": credential["scopes"],
            "expires_in": credential["expires_in"],
            "token_recorded": False,
        }
        record["runtime"]["instance"]["first"]["observation"] = {
            "epoch": stamp["epoch"],
            "server_id": stamp["server_id"],
            "read_revision": stamp["read_revision"],
            "task_count": len(tasks),
            "context_recorded": False,
        }
        record["source"]["candidate_count"] = len(candidates)
        artifact = {
            "artifact_ref": chosen["artifact_ref"],
            "digest": chosen["digest"],
            "db_media_type": chosen["media_type"],
            "db_length": chosen["length"],
            "probed_candidates": chosen["probed"],
            "probe_rejections_sample": chosen["rejected_sample"],
            "descriptor": descriptor_summary(descriptor),
            "access_handle_recorded": False,
        }
        record["artifact"] = artifact
        if not chosen["artifact_ref"]:
            raise RuntimeError("artifact selection returned no ref")

        # 1. TTL expiry.
        issued_at = time.time()
        immediate = instance.fetch_blob(token, descriptor, label="immediate")
        expected_digest = "sha256:" + str(
            (descriptor.get("verification") or {}).get("expected_digest") or ""
        )
        immediate_ok = (
            immediate["status"] == 200
            and immediate["body_sha256"] == expected_digest
            and immediate["length_matches_descriptor"]
        )
        record["measurements"]["ttl"] = {
            "descriptor_expires_at_unix_ms": descriptor.get("expires_at_unix_ms"),
            "descriptor_issued_at": issued_at,
            "immediate_fetch": immediate,
            "immediate_ok": immediate_ok,
            "immediate_digest_matches_declared": immediate.get("body_sha256")
            == expected_digest,
        }
        wait = wait_past_expiry(descriptor, issued_at)
        print(f"[{now_iso()}] TTL passed ({wait})", flush=True)
        late = instance.fetch_blob(token, descriptor, label="after_ttl")
        record["measurements"]["ttl"]["wait"] = wait
        record["measurements"]["ttl"]["after_ttl_fetch"] = late
        record["measurements"]["ttl"]["after_ttl_expected"] = {
            "status": 410,
            "code": "MEDIA_HANDLE_EXPIRED",
        }
        fresh_status, fresh_descriptor = instance.descriptor(token, context, chosen["digest"])
        fresh_fetch = (
            instance.fetch_blob(token, fresh_descriptor, label="fresh_after_ttl")
            if fresh_status == 200 and isinstance(fresh_descriptor, dict)
            else None
        )
        record["measurements"]["ttl"]["fresh_descriptor"] = {
            "status": fresh_status,
            "descriptor": descriptor_summary(fresh_descriptor)
            if isinstance(fresh_descriptor, dict)
            else None,
            "new_handle_differs_from_expired": bool(
                isinstance(fresh_descriptor, dict)
                and fresh_descriptor.get("access_handle") != descriptor.get("access_handle")
            ),
            "fetch": fresh_fetch,
            "fetch_ok": bool(
                fresh_fetch
                and fresh_fetch.get("status") == 200
                and fresh_fetch.get("body_sha256") == expected_digest
            ),
        }
        if not immediate_ok or late.get("status") != 410 or not record["measurements"]["ttl"][
            "fresh_descriptor"
        ]["fetch_ok"]:
            raise RuntimeError("TTL-expiry measurement did not produce the expected sequence")

        ttl_out = WORK / "executor-live-ttl.json"
        executor_ttl = run_live_executor(
            work=WORK,
            out_path=ttl_out,
            token=token,
            other_token=other_token,
            context=context,
            digest=chosen["digest"],
            server_identity=instance.facts["server_identity"],
            epoch=instance.facts["epoch"],
            artifact_ref=chosen["artifact_ref"],
            audience=credential["principal"],
            expired_handle=str(descriptor["access_handle"]),
            secrets_list=secrets_list,
        )
        record["executor"]["ttl"] = executor_ttl
        print(f"[{now_iso()}] TTL live executor exit={executor_ttl['exit']}", flush=True)
        expired_negative = (
            (executor_ttl.get("observed") or {}).get("negatives") or {}
        ).get("expired_handle")
        if expired_negative != "Err(DescriptorExpired)":
            raise RuntimeError(
                f"live executor TTL mapping was {expired_negative!r}, expected Err(DescriptorExpired)"
            )

        # 2. Instance change: issue a still-valid handle, immediately restart.
        probe_status, probe_descriptor = instance.descriptor(token, context, chosen["digest"])
        if probe_status != 200 or not isinstance(probe_descriptor, dict):
            raise RuntimeError(f"instance probe descriptor failed ({probe_status})")
        probe_fetch = instance.fetch_blob(token, probe_descriptor, label="before_restart")
        probe_payload = decode_handle(str(probe_descriptor["access_handle"]))
        remaining = (
            int(probe_payload["payload"]["exp"]) - int(time.time())
            if probe_payload.get("decodable")
            else None
        )
        record["measurements"]["instance_change"] = {
            "probe_descriptor_expires_at_unix_ms": probe_descriptor.get("expires_at_unix_ms"),
            "probe_fetch": probe_fetch,
            "handle_epoch_matches_serving_epoch": bool(
                probe_payload.get("decodable")
                and probe_payload["payload"]["epc"] == instance.facts["epoch"]
            ),
            "remaining_ttl_seconds_at_restart_request": remaining,
        }
        if probe_fetch.get("status") != 200 or remaining is None or remaining <= 5:
            raise RuntimeError("instance probe handle was not still valid before restart")

        first_stop = instance.stop()
        stopped = True
        first_log, first_redactions = sanitized_log(
            first_stop.get("log", ""), secrets_list
        )
        first_stop.pop("log", None)
        record["runtime"]["instance"]["first"]["stop"] = first_stop
        record["runtime"]["instance"]["first"]["discovery"][
            "pid_matches_serving_process"
        ] = first_stop.get("serving_pid") == record["runtime"]["instance"]["first"][
            "discovery"
        ].get("pid")
        record["runtime"]["instance"]["first"]["log_text"] = first_log
        record["runtime"]["instance"]["first"]["handle_redactions"] = first_redactions
        if not first_stop["clean_exit"] or not first_stop["port_closed"]:
            raise RuntimeError("the first isolated Runtime did not stop cleanly")
        if not first_stop["discovery_removed"]:
            raise RuntimeError("the first instance did not remove its discovery descriptor")

        second = RuntimeInstance(
            label="runtime-2",
            db_copy=db_copy,
            cas_copy=cas_copy,
            secret=run_secret,
            handle_secret=handle_secret,
        )
        instance = second
        stopped = False
        record["runtime"]["instance"]["second"] = second.start()
        print(f"[{now_iso()}] runtime-2 started epoch={second.facts['epoch']}", flush=True)
        if second.facts["epoch"] == record["runtime"]["instance"]["first"]["epoch"]:
            raise RuntimeError("the restarted Runtime reused the previous observation epoch")
        if second.facts["server_identity"] != record["runtime"]["instance"]["first"][
            "server_identity"
        ]:
            raise RuntimeError("server identity unexpectedly changed across the restart")

        credential2 = second.issue_credential()
        token2 = credential2["token"]
        other2 = second.issue_credential()
        other_token2 = other2["token"]
        secrets_list.extend([token2, other_token2])
        observation2 = second.open_observation(token2)
        stamp2 = observation2["grant"]["stamp"]
        context2 = stamp2["context_id"]
        secrets_list.append(context2)
        record["runtime"]["instance"]["second"]["credential"] = {
            "principal": credential2["principal"],
            "other_principal": other2["principal"],
            "scopes": credential2["scopes"],
            "expires_in": credential2["expires_in"],
            "token_recorded": False,
        }
        record["runtime"]["instance"]["second"]["observation"] = {
            "epoch": stamp2["epoch"],
            "server_id": stamp2["server_id"],
            "read_revision": stamp2["read_revision"],
            "task_count": len(projection_tasks(observation2)),
            "context_recorded": False,
        }
        old_payload = probe_payload.get("payload") or {}
        remaining_at_fetch = int(old_payload.get("exp", 0)) - int(time.time())
        instance_change_fetch = second.fetch_blob(token2, probe_descriptor, label="old_handle_new_instance")
        instance_change_fetch["handle_epoch"] = old_payload.get("epc")
        instance_change_fetch["serving_epoch"] = second.facts["epoch"]
        instance_change_fetch["handle_principal_matches_new_credential"] = (
            old_payload.get("sub") == credential2["principal"]
        )
        instance_change_fetch["remaining_ttl_seconds_at_fetch"] = remaining_at_fetch
        record["measurements"]["instance_change"]["after_restart_fetch"] = instance_change_fetch
        record["measurements"]["instance_change"]["after_restart_expected"] = {
            "status": 410,
            "code": "MEDIA_HANDLE_EXPIRED",
            "epoch_changed": True,
            "server_identity_unchanged": True,
        }
        if instance_change_fetch["status"] != 410:
            raise RuntimeError(
                "the old-epoch handle did not produce the expected 410 after restart"
            )
        fresh2_status, fresh2_descriptor = second.descriptor(token2, context2, chosen["digest"])
        fresh2_fetch = (
            second.fetch_blob(token2, fresh2_descriptor, label="fresh_after_restart")
            if fresh2_status == 200 and isinstance(fresh2_descriptor, dict)
            else None
        )
        record["measurements"]["instance_change"]["fresh_after_restart"] = {
            "status": fresh2_status,
            "descriptor": descriptor_summary(fresh2_descriptor)
            if isinstance(fresh2_descriptor, dict)
            else None,
            "fetch": fresh2_fetch,
            "fetch_ok": bool(
                fresh2_fetch
                and fresh2_fetch.get("status") == 200
                and fresh2_fetch.get("body_sha256") == expected_digest
            ),
        }
        if not record["measurements"]["instance_change"]["fresh_after_restart"]["fetch_ok"]:
            raise RuntimeError("a fresh descriptor/handle after re-authentication did not work")

        epoch_out = WORK / "executor-live-epoch.json"
        executor_epoch = run_live_executor(
            work=WORK,
            out_path=epoch_out,
            token=token2,
            other_token=other_token2,
            context=context2,
            digest=chosen["digest"],
            server_identity=second.facts["server_identity"],
            epoch=second.facts["epoch"],
            artifact_ref=chosen["artifact_ref"],
            audience=credential2["principal"],
            expired_handle=str(probe_descriptor["access_handle"]),
            secrets_list=secrets_list,
        )
        record["executor"]["epoch"] = executor_epoch
        print(f"[{now_iso()}] epoch live executor exit={executor_epoch['exit']}", flush=True)
        epoch_negative = (
            (executor_epoch.get("observed") or {}).get("negatives") or {}
        ).get("expired_handle")
        if epoch_negative != "Err(DescriptorExpired)":
            raise RuntimeError(
                f"live executor epoch mapping was {epoch_negative!r}, expected Err(DescriptorExpired)"
            )

        second_stop = second.stop()
        stopped = True
        second_log, second_redactions = sanitized_log(
            second_stop.get("log", ""), secrets_list
        )
        second_stop.pop("log", None)
        record["cleanup"]["runtime_2_stop"] = second_stop
        record["runtime"]["instance"]["second"]["discovery"][
            "pid_matches_serving_process"
        ] = second_stop.get("serving_pid") == record["runtime"]["instance"]["second"][
            "discovery"
        ].get("pid")
        record["runtime"]["instance"]["second"]["log_text"] = second_log
        record["runtime"]["instance"]["second"]["handle_redactions"] = second_redactions
        if not second_stop["clean_exit"] or not second_stop["port_closed"]:
            raise RuntimeError("the restarted Runtime did not stop cleanly")
        if not second_stop["discovery_removed"]:
            raise RuntimeError("the restarted Runtime did not remove its discovery descriptor")
    finally:
        if not stopped and instance.process is not None and instance.process.poll() is None:
            try:
                instance.stop()
            except Exception:  # pragma: no cover - best-effort cleanup
                instance.process.kill()

    # Evidence: sanitized logs, executor observations, record/session.
    for key, instance_facts in record["runtime"]["instance"].items():
        if key not in {"first", "second"}:
            continue
        log_text = str(instance_facts.pop("log_text", ""))
        redactions = int(instance_facts.pop("handle_redactions", 0))
        instance_facts["log"] = {
            "file": f"runtime-{key}.log.sanitized.txt",
            "handle_redactions": redactions,
            "sha256": sha256_bytes(log_text.encode("utf-8")),
        }
        (EVIDENCE_DIR / f"runtime-{key}.log.sanitized.txt").write_text(
            log_text, encoding="utf-8"
        )
    for label in ("ttl", "epoch"):
        observed = record["executor"][label].get("observed")
        if observed is not None:
            write_json(EVIDENCE_DIR / f"executor-live-{label}.json", observed)
        record["executor"][label].pop("observed", None)

    # The TTL phase is measured; the instance-change phase is measured too.
    record["verdict"] = {
        "ttl_expiry_410": record["measurements"]["ttl"]["after_ttl_fetch"]["status"] == 410
        and record["measurements"]["ttl"]["after_ttl_fetch"].get("body_code")
        == "MEDIA_HANDLE_EXPIRED",
        "fresh_handle_after_ttl": record["measurements"]["ttl"]["fresh_descriptor"]["fetch_ok"],
        "instance_change_410": record["measurements"]["instance_change"][
            "after_restart_fetch"
        ]["status"]
        == 410
        and record["measurements"]["instance_change"]["after_restart_fetch"].get("body_code")
        == "MEDIA_HANDLE_EXPIRED",
        "fresh_handle_after_restart": record["measurements"]["instance_change"][
            "fresh_after_restart"
        ]["fetch_ok"],
        "live_executor_ttl_mapping": "Err(DescriptorExpired)",
        "live_executor_epoch_mapping": "Err(DescriptorExpired)",
        "stub_executor_refetch_test": "apps/desktop/src/media_executor.rs::tests::"
        "an_expired_handle_reauthorizes_the_descriptor_and_admits",
    }
    record["residuals"] = [
        "The live executor mapping was measured from the live 410 status; the one-shot "
        "descriptor reauthorization/refetch is pinned by the bounded stub-server test "
        "(the live server cannot be forced to expire only the handle while a fresh "
        "descriptor read succeeds under the same repository call).",
        "No Desktop GPUI/native rendering is exercised by this slice (executor/client "
        "boundary only); the RT6 native artifact surface evidence stays separate.",
        "Only ORIGINAL PNG artifacts are measured; PREVIEW/THUMBNAIL byte routes and "
        "non-image types are out of scope.",
        "The instance-change 410 uses the same per-run handle secret across both "
        "processes; with the default ephemeral secret a restart would instead be a 401 "
        "MAC failure (documented Runtime rotation behavior).",
        "The 60 s TTL minimum was used; the 300 s default was not separately timed.",
    ]
    record["finished_at"] = now_iso()

    session = {
        "label": record["label"],
        "started_at": record["started_at"],
        "finished_at": record["finished_at"],
        "endpoint": ENDPOINT,
        "ttl_seconds": TTL_SECONDS,
        "source": record["source"],
        "desktop": record["desktop"],
        "runtime": {
            key: value
            for key, value in record["runtime"].items()
            if key not in {"instance"}
        },
        "runtime_instances": {
            key: {
                field: value
                for field, value in facts.items()
                if field not in {"log"}
            }
            for key, facts in record["runtime"]["instance"].items()
            if key in {"first", "second"}
        },
        "artifact": record.get("artifact"),
        "measurements": record["measurements"],
        "executor": record["executor"],
        "cleanup": record["cleanup"],
        "verdict": record["verdict"],
        "residuals": record["residuals"],
    }

    # Remove the raw logs (they contain handle-bearing request lines) after the
    # sanitized copies are written; the sanitized evidence and hashes remain.
    write_json(EVIDENCE_DIR / "record.json", record)
    write_json(EVIDENCE_DIR / "session.json", session)

    record["cleanup"]["db_copy_sha256_at_end"] = None
    if db_copy.is_file():
        record["cleanup"]["db_copy_sha256_at_end"] = sha256_file(db_copy)
    record["cleanup"]["db_copy_bytes_at_end"] = db_copy.stat().st_size
    record["cleanup"]["work_root"] = str(WORK)
    record["cleanup"]["work_root_removed"] = False

    scan = leak_scan(EVIDENCE_DIR, secrets_list)
    record["cleanup"]["leak_scan"] = scan
    if scan["findings"]:
        write_json(EVIDENCE_DIR / "record.json", record)
        raise RuntimeError(f"evidence leak scan failed: {scan['findings']}")

    if not keep_work:
        shutil.rmtree(WORK)
        record["cleanup"]["work_root_removed"] = True
        cleanup_notes.append("isolated DB/CAS copy and raw logs removed after evidence write")

    # Re-write after the cleanup fields are final, then verify the port/discovery.
    cleanup = {
        **record["cleanup"],
        "port_closed": port_is_closed(PORT),
        "discovery_absent": not (
            WORK / "runtime-1" / "runtime-discovery.json"
        ).exists()
        and not (WORK / "runtime-2" / "runtime-discovery.json").exists(),
        "notes": cleanup_notes,
    }
    record["cleanup"] = cleanup
    session["cleanup"] = cleanup
    write_json(EVIDENCE_DIR / "record.json", record)
    write_json(EVIDENCE_DIR / "session.json", session)

    # Final leak scan after the last writes.
    final_scan = leak_scan(EVIDENCE_DIR, secrets_list)
    record["cleanup"]["leak_scan"] = final_scan
    session["cleanup"]["leak_scan"] = final_scan
    write_json(EVIDENCE_DIR / "record.json", record)
    write_json(EVIDENCE_DIR / "session.json", session)
    if final_scan["findings"]:
        raise RuntimeError(f"final evidence leak scan failed: {final_scan['findings']}")

    return session


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keep-work", action="store_true", help="keep the isolated copy")
    arguments = parser.parse_args(argv)
    try:
        session = run_once(keep_work=arguments.keep_work)
    except Exception:
        traceback.print_exc()
        print("RT3 residual measurement FAILED", file=sys.stderr)
        return 1
    verdict = session["verdict"]
    print(json.dumps(verdict, indent=2, ensure_ascii=False))
    print(f"evidence: {EVIDENCE_DIR}")
    return 0 if all(value is not False for value in verdict.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
