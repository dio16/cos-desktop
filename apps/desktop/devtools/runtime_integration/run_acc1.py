#!/usr/bin/env python3
"""MP-ACC1 Golden Creative Workflow acceptance attempt (bounded, evidence-recording).

This harness is an *acceptance attempt*, not a product feature: it starts one
isolated Runtime from the COS Runtime checkout (fresh DB copy, own port,
per-run secrets, isolated config/logs/CAS, redirected discovery descriptor),
probes the Runtime's own readiness and generation-plane health, then runs one
bounded golden workflow through the real Desktop protocol surface:

    catalog -> Desktop command (CREATE_TASK) -> observation/deltas
    -> TASK / ITERATION / CANDIDATE / ARTIFACT / EVALUATION / OUTCOME
    -> RC-13 media descriptor -> handle -> blob

Every step is recorded from authoritative projections and receipts; nothing is
derived locally (no best/winner/outcome computation). One failure injection is
performed: the observation session is dropped and resumed through the protocol
resume triplet while the task is non-terminal, proving the projection catches
up without a Runtime restart.

Scope limits recorded honestly in the evidence (not product acceptance):
- DD-25 Start/iteration-advance semantics are NOT recorded in the plan, so the
  create path auto-dispatches (the Runtime drain claims ASSIGNED) and the
  Start/Next Iteration steps remain NOT_MEASURED.
- The Desktop GUI binary is not launched; observation is protocol-level.
- Five of the six DC-MP-ACC1 failure injections are not attempted.

Usage (from the repository root):

    python apps/desktop/devtools/runtime_integration/run_acc1.py
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
import subprocess
import sys
import time
import urllib.error
import urllib.parse
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
SOURCE_PRESETS = RUNTIME_ROOT / "db" / "cos_presets.json"
DESKTOP_EXE = base.DESKTOP_EXE

DEFAULT_EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "acc1-latest"
)
EVIDENCE_DIR = DEFAULT_EVIDENCE_DIR
WORK_ROOT = DESKTOP_ROOT / "target" / "acc1"

PORT = 8131
ENDPOINT = f"http://127.0.0.1:{PORT}"

OBSERVATION_PROTOCOL = {"major": 2, "minor": 0}
OBSERVATION_SCHEMA = {"major": 2, "minor": 0}
TERMINAL_TASK_STATUS = {"COMPLETED", "CANCELLED"}

# Bounded budgets: the whole run must stay well inside the acceptance window.
GENERATION_TIMEOUT_S = 1500.0  # 25 min hard cap for the workflow wait
POLL_INTERVAL_S = 0.75


# --------------------------------------------------------------------------- basics


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_bytes(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def short_hash(value: str) -> str:
    """Trace identity for one opaque capability (never the capability itself)."""
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(document, ensure_ascii=False, indent=2, default=str) + "\n",
        encoding="utf-8",
    )


def port_free() -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.3)
        return probe.connect_ex(("127.0.0.1", PORT)) != 0


def _opener() -> urllib.request.OpenerDirector:
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


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
    try:
        with _opener().open(request, timeout=timeout) as response:
            raw = response.read()
            status = int(response.status)
            response_headers = {k.lower(): v for k, v in response.headers.items()}
    except urllib.error.HTTPError as error:
        raw = error.read()
        status = int(error.code)
        response_headers = {k.lower(): v for k, v in error.headers.items()}
    except (urllib.error.URLError, OSError) as error:
        return {
            "status": None,
            "document": None,
            "bytes": 0,
            "error": type(error).__name__,
            "headers": {},
        }
    document = None
    with contextlib.suppress(ValueError):
        document = json.loads(raw.decode("utf-8"))
    return {
        "status": status,
        "document": document,
        "bytes": len(raw),
        "headers": response_headers,
    }


def http_bytes(url: str, *, headers: dict[str, str], timeout: float = 60.0) -> dict:
    request = urllib.request.Request(url, headers=headers, method="GET")
    try:
        with _opener().open(request, timeout=timeout) as response:
            raw = response.read()
            status = int(response.status)
            response_headers = {k.lower(): v for k, v in response.headers.items()}
    except urllib.error.HTTPError as error:
        raw = error.read()
        status = int(error.code)
        response_headers = {k.lower(): v for k, v in error.headers.items()}
    except (urllib.error.URLError, OSError) as error:
        return {"status": None, "raw": b"", "error": type(error).__name__, "headers": {}}
    return {"status": status, "raw": raw, "headers": response_headers}


def png_dimensions(raw: bytes) -> dict | None:
    if len(raw) < 24 or raw[:8] != b"\x89PNG\r\n\x1a\n" or raw[12:16] != b"IHDR":
        return None
    return {
        "width": int.from_bytes(raw[16:20], "big"),
        "height": int.from_bytes(raw[20:24], "big"),
    }


def compact(document: object, limit: int = 180_000) -> object:
    text = json.dumps(document, ensure_ascii=False, default=str)
    if len(text) <= limit:
        return document
    return {"truncated": True, "bytes": len(text), "preview": text[:limit]}


# --------------------------------------------------------------------------- Runtime


class IsolatedRuntime:
    """One isolated Runtime process on the harness-owned port."""

    def __init__(
        self,
        *,
        work: Path,
        db_copy: Path,
        presets: Path,
        secret: str,
        handle_secret: str,
        phase: str = "",
    ):
        self.work = work
        self.db_copy = db_copy
        self.presets = presets
        self.secret = secret
        self.handle_secret = handle_secret
        self.phase = phase
        suffix = f"-{phase}" if phase else ""
        self.process: subprocess.Popen | None = None
        self.log_path = work / f"runtime{suffix}.log"
        self.log_handle = None
        self.config_path = work / "runtime-config.yaml"
        self.discovery_path = work / "runtime-discovery.json"
        self.stop_file = work / f"runtime{suffix}.stop"
        self.ready_file = work / f"runtime{suffix}.ready"
        self.cas_root = work / "cas"
        self.cos_work = work / "cos-work"
        self.log_dir = work / "runtime-logs"
        self.output_root = work / "outputs"

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
        rendered, log_count = re.subn(
            r"(?m)^(\s*)log_dir:.*$",
            lambda match: f"{match.group(1)}log_dir: {self.log_dir.as_posix()}",
            rendered,
            count=1,
        )
        if log_count != 1:
            raise RuntimeError("default.yaml no longer contains exactly one log_dir line")
        rendered, out_count = re.subn(
            r"(?m)^(\s*)output_root:.*$",
            lambda match: f"{match.group(1)}output_root: {self.output_root.as_posix()}",
            rendered,
            count=1,
        )
        if out_count != 1:
            raise RuntimeError("default.yaml no longer contains exactly one output_root line")
        rendered = rendered.replace(
            "project_root: C:/AI/orchestrator", f"project_root: {RUNTIME_ROOT.as_posix()}"
        )
        self.config_path.write_text(rendered, encoding="utf-8")

    def start(self, timeout: float = 150.0) -> dict:
        if not port_free():
            raise RuntimeError(f"port {PORT} is already in use; the harness owns {PORT}")
        for directory in (self.cas_root, self.cos_work, self.log_dir, self.output_root):
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
                "ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET": self.handle_secret,
                "ORCHESTRATOR_COS_PRESETS": str(self.presets),
                "ORCHESTRATOR_COS_CAS_ROOT": str(self.cas_root),
                "ORCHESTRATOR_COS_WORK_ROOT": str(self.cos_work),
                "ORCHESTRATOR_COS_PROJECT_ROOT": str(RUNTIME_ROOT),
                "ORCHESTRATOR_RUNTIME_ROOT": str(RUNTIME_ROOT),
                # RT8/RC-16: never publish into the real per-user location.
                "COS_RUNTIME_DISCOVERY_PATH": str(self.discovery_path),
                "COS_RUNTIME_ORIGIN": ENDPOINT,
            }
        )
        for stale in (self.stop_file, self.ready_file):
            with contextlib.suppress(FileNotFoundError):
                stale.unlink()
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- start {now_iso()} ---\n".encode())
        self.log_handle.flush()
        self.process = subprocess.Popen(  # noqa: S603 - fixed argv, loopback only
            [
                str(RUNTIME_PYTHON),
                str(Path(__file__).resolve().parent / "isolated_runtime.py"),
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
                    f"Runtime exited during startup (exit {self.process.returncode});"
                    f" log tail: {self.log_tail()}"
                )
            ready = http_call("GET", f"{ENDPOINT}/api/ready", timeout=2.0)
            if ready["status"] == 200:
                break
            if time.monotonic() - started > timeout:
                raise RuntimeError(f"Runtime readiness timed out; log tail: {self.log_tail()}")
            time.sleep(0.25)
        hello = http_call("GET", f"{ENDPOINT}/api/cos/desktop/server")
        if hello["status"] != 200 or not isinstance(hello["document"], dict):
            raise RuntimeError(f"HELLO failed ({hello['status']})")
        server = hello["document"]["server"]
        return {
            "pid": self.process.pid,
            "ready": ready,
            "hello": hello["document"],
            "server_identity": server["server_identity"],
            "server_build": server["server_build"],
            "epoch": server["epoch"],
            "protocol": server["protocol"],
            "schema": server["schema"],
        }

    def log_tail(self, limit: int = 4000) -> str:
        with contextlib.suppress(OSError):
            return self.log_path.read_text(encoding="utf-8", errors="replace")[-limit:]
        return ""

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
                }
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return {"pid": pid, "clean": clean, "exit": self.process.returncode}

    # ------------------------------------------------------------------ protocol

    def status(self) -> dict:
        return http_call("GET", f"{ENDPOINT}/api/status")

    def issue_credential(self) -> dict:
        return http_call(
            "POST",
            f"{ENDPOINT}/api/cos/desktop/credentials",
            headers={"X-Auth-Secret": self.secret},
        )

    def desktop_catalog(self, token: str) -> dict:
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/desktop/catalog",
            headers={"Authorization": f"Bearer {token}"},
        )

    def open_observation(
        self,
        token: str,
        *,
        resume: dict[str, str] | None = None,
    ) -> dict:
        body: dict = {
            "schema_version": 1,
            "request_id": uuid.uuid4().hex,
            "protocol": dict(OBSERVATION_PROTOCOL),
            "schema": dict(OBSERVATION_SCHEMA),
            "scope": {"task_ids": [], "filters": [{"name": "view", "value": "all"}]},
            "retained_commands": [],
        }
        if resume is not None:
            body.update(
                {
                    "scope_id": resume["scope_id"],
                    "last_seen_cursor": resume["last_seen_cursor"],
                    "epoch": resume["epoch"],
                }
            )
        return http_call(
            "POST",
            f"{ENDPOINT}/api/cos/desktop/observation",
            body=body,
            headers={"Authorization": f"Bearer {token}"},
        )

    def deltas(self, token: str, cursor: str, context: str) -> dict:
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/desktop/deltas?cursor={cursor}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Cos-Context": context,
            },
        )

    def command(self, token: str, context: str, envelope: dict) -> dict:
        return http_call(
            "POST",
            f"{ENDPOINT}/api/cos/desktop/commands",
            body=envelope,
            headers={
                "Authorization": f"Bearer {token}",
                "X-Cos-Context": context,
            },
        )

    def desktop_task(self, token: str, context: str, task_ref: str) -> dict:
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/desktop/tasks/{task_ref}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Cos-Context": context,
            },
        )

    def desktop_history(self, token: str, context: str, task_ref: str) -> dict:
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/desktop/tasks/{task_ref}/history?limit=20",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Cos-Context": context,
            },
        )

    def candidates(self, token: str, task_ref: str) -> dict:
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/tasks/{task_ref}/candidates",
            headers={"Authorization": f"Bearer {token}"},
        )

    def evaluation(
        self,
        token: str,
        task_ref: str,
        candidate_id: str,
        *,
        run_id: str,
        iteration: int,
        artifact_ref: str,
    ) -> dict:
        query = urllib.parse.urlencode(
            {
                "run_id": run_id,
                "iteration": iteration,
                "artifact_ref": artifact_ref,
            }
        )
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/tasks/{task_ref}/evaluations/{candidate_id}?{query}",
            headers={"Authorization": f"Bearer {token}"},
        )

    def task_details(self, token: str, task_ref: str) -> dict:
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/tasks/{task_ref}/details",
            headers={"Authorization": f"Bearer {token}"},
        )

    def media_descriptor(self, token: str, context: str, digest: str) -> dict:
        return http_call(
            "GET",
            f"{ENDPOINT}/api/cos/media/{digest}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Cos-Context": context,
            },
        )

    def media_blob(self, token: str, access_handle: str) -> dict:
        """Fetch one blob through the descriptor-issued access handle.

        The descriptor's ``access_handle`` is already the media route path
        (``/api/cos/media/<handle>/blob``); the raw handle token is what the
        ``X-Media-Handle`` capability header carries.
        """
        path = access_handle
        if not path.startswith("/api/cos/media/"):
            path = f"/api/cos/media/{access_handle}/blob"
        handle = path.split("/api/cos/media/", 1)[1].rsplit("/blob", 1)[0]
        return http_bytes(
            f"{ENDPOINT}{path}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Media-Handle": handle,
            },
        )


# --------------------------------------------------------------------------- projections


def snapshot_mutations(document: dict) -> list[dict]:
    mutations: list[dict] = []
    snapshot = document.get("snapshot") or {}
    for chunk in snapshot.get("chunks", []) or []:
        mutations.extend(chunk.get("mutations", []) or [])
    return mutations


def transaction_mutations(document: dict) -> list[dict]:
    transaction = document.get("transaction") or {}
    return transaction.get("mutations", []) or []


def projection_kind(mutation: dict) -> str | None:
    projection = mutation.get("projection") or {}
    kind = projection.get("projection")
    return str(kind) if isinstance(kind, str) else None


def projection_revision(mutation: dict) -> str | None:
    projection = mutation.get("projection") or {}
    revision = projection.get("projection_revision")
    return str(revision) if revision is not None else None


def projection_identity(mutation: dict) -> str:
    projection = mutation.get("projection") or {}
    kind = projection.get("projection")
    if kind == "TASK":
        return str(projection.get("task_ref", ""))
    if kind == "ITERATION":
        return str(projection.get("iteration_id", ""))
    if kind == "CANDIDATE":
        candidate = projection.get("candidate_ref") or {}
        return str(candidate.get("candidate_id", ""))
    if kind == "ARTIFACT":
        return str(projection.get("artifact_ref", ""))
    if kind == "EVALUATION":
        return str(projection.get("evaluation_ref", ""))
    if kind == "OUTCOME":
        return str(projection.get("outcome_ref", ""))
    if kind == "GRAPH":
        return str(projection.get("graph_id", ""))
    if kind == "CAPABILITIES":
        return str(projection.get("capabilities_ref", ""))
    return ""


def task_state(mutation: dict) -> dict | None:
    projection = mutation.get("projection") or {}
    if projection.get("projection") != "TASK":
        return None
    state = {
        "task_ref": projection.get("task_ref"),
        "task_status": projection.get("task_status"),
        "inner_phase": projection.get("inner_phase"),
        "iteration_index": projection.get("iteration_index"),
        "formal_outcome_ref": projection.get("formal_outcome_ref"),
        "revision": projection.get("projection_revision"),
    }
    return {key: value for key, value in state.items() if value is not None}


def task_state_from_snapshot(document: dict, task_ref: str) -> dict | None:
    """TASK projection of one task from a snapshot document (None when absent)."""
    for mutation in snapshot_mutations(document):
        projection = mutation.get("projection") or {}
        if projection.get("projection") != "TASK":
            continue
        if str(projection.get("task_ref", "")) != task_ref:
            continue
        return task_state(mutation)
    return None


def change_summary(mutation: dict) -> dict:
    projection = mutation.get("projection") or {}
    kind = projection_kind(mutation) or "UNKNOWN"
    summary: dict = {
        "op": mutation.get("op"),
        "kind": kind,
        "identity": projection_identity(mutation),
        "revision": projection_revision(mutation),
    }
    if kind == "TASK":
        summary["state"] = task_state(mutation)
    elif kind == "ITERATION":
        summary["phase"] = projection.get("phase")
        summary["iteration"] = projection.get("iteration")
    elif kind == "CANDIDATE":
        summary["candidate"] = projection.get("candidate_ref")
        summary["evaluation_refs"] = projection.get("evaluation_refs")
        summary["display_label"] = projection.get("display_label")
    elif kind == "ARTIFACT":
        summary["artifact_digest"] = projection.get("artifact_digest")
        summary["availability"] = projection.get("availability")
        summary["media_type"] = projection.get("media_type")
    elif kind == "EVALUATION":
        summary["decision"] = projection.get("decision")
        summary["authority"] = projection.get("authority")
        summary["axes"] = projection.get("axes")
    elif kind == "OUTCOME":
        summary["formal_status"] = projection.get("formal_status")
        summary["stop_reason"] = projection.get("stop_reason")
        summary["selected_refs"] = projection.get("selected_refs")
    elif kind == "GRAPH":
        summary["graph"] = {
            key: projection.get(key) for key in ("graph_id", "graph_version") if projection.get(key)
        }
    return summary


def count_kinds(mutations: list[dict]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for mutation in mutations:
        kind = projection_kind(mutation) or "UNKNOWN"
        counts[kind] = counts.get(kind, 0) + 1
    return counts


def find_outcome(mutations: list[dict], task_ref: str) -> dict | None:
    for mutation in mutations:
        projection = mutation.get("projection") or {}
        if projection.get("projection") != "OUTCOME":
            continue
        if str(projection.get("task_ref", "")) != task_ref:
            continue
        return projection
    return None


def find_task_artifact(
    mutations: list[dict], task_ref: str, outcome: dict | None
) -> dict | None:
    """The task's own artifact: OUTCOME selected digest, else its candidate ref.

    A snapshot carries every visible task's ARTIFACT projection, so the first
    AVAILABLE artifact is not necessarily this task's. Never guess: resolve the
    identity through the server-projected OUTCOME/CANDIDATE links.
    """
    selected_digests = {
        str(reference.get("artifact_digest"))
        for reference in (outcome or {}).get("selected_refs", []) or []
        if reference.get("artifact_digest")
    }
    artifacts: dict[str, dict] = {}
    candidates: list[dict] = []
    for mutation in mutations:
        projection = mutation.get("projection") or {}
        kind = projection.get("projection")
        if kind == "ARTIFACT":
            artifacts[str(projection.get("artifact_digest"))] = projection
        elif kind == "CANDIDATE" and str(projection.get("task_ref")) == task_ref:
            candidates.append(projection)
    for digest in sorted(selected_digests):
        if digest in artifacts:
            return artifacts[digest]
    for candidate in candidates:
        for reference in candidate.get("artifact_refs", []) or []:
            for projection in artifacts.values():
                if str(projection.get("artifact_ref")) == str(reference):
                    return projection
    return None


# --------------------------------------------------------------------------- run


def probe_generation_plane() -> dict:
    """Direct environment probes recorded *before* the Runtime's own report."""
    comfy = http_call("GET", "http://127.0.0.1:8188/system_stats", timeout=5.0)
    ollama = http_call("GET", "http://127.0.0.1:11434/api/tags", timeout=5.0)
    comfy_document = comfy.get("document") or {}
    system = comfy_document.get("system") or {}
    return {
        "probe": "direct preflight (not the Runtime report)",
        "at": now_iso(),
        "comfyui": {
            "url": "http://127.0.0.1:8188/system_stats",
            "http_status": comfy["status"],
            "comfyui_version": system.get("comfyui_version"),
            "argv": system.get("argv"),
        },
        "ollama": {
            "url": "http://127.0.0.1:11434/api/tags",
            "http_status": ollama["status"],
            "model_count": len((ollama.get("document") or {}).get("models", []) or []),
        },
    }


def build_target_recipe_ref(catalog_recipe_ref: dict) -> dict:
    """Map the catalog recipe ref onto the Desktop owner wire names."""
    graph = dict(catalog_recipe_ref.get("graph_ref") or {})
    owner: dict = {
        "recipe_id": str(catalog_recipe_ref.get("recipe_id", "")),
        "graph_ref": {
            "graph_id": str(graph.get("graph_id", "")),
            "version": int(graph.get("version", graph.get("graph_version", 0))),
            "digest": str(graph.get("digest", graph.get("graph_digest", ""))),
        },
    }
    for key in (
        "source_harness_version_id",
        "source_harness_digest",
        "profile_ref",
        "projection_digest",
    ):
        if catalog_recipe_ref.get(key) is not None:
            owner[key] = catalog_recipe_ref[key]
    return owner


def sanitize_log(path: Path, secrets_list: list[str], limit: int = 120_000) -> dict:
    text = path.read_text(encoding="utf-8", errors="replace") if path.is_file() else ""
    for secret in secrets_list:
        if secret:
            text = text.replace(secret, "<redacted>")
    text = re.sub(r"mh1\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+", "<redacted-media-handle>", text)
    text = re.sub(r"Bearer\s+[A-Za-z0-9._\-]+", "Bearer <redacted>", text)
    text = re.sub(r"cursor[-_][A-Za-z0-9_\-]+", "<redacted-cursor>", text)
    text = re.sub(r"context[-_][A-Za-z0-9_\-]+", "<redacted-context>", text)
    text = re.sub(r"scope[-_][A-Za-z0-9_\-]+", "<redacted-scope>", text)
    text = text[-limit:]
    return {"text": text, "sha256": sha256_bytes(text.encode("utf-8"))}


def run(args: argparse.Namespace) -> int:
    global EVIDENCE_DIR
    evidence_dir = getattr(args, "evidence_dir", None)
    if evidence_dir:
        EVIDENCE_DIR = Path(evidence_dir).resolve()
    stamp = time.strftime("%Y%m%dT%H%M%S")
    work = WORK_ROOT / stamp
    work.mkdir(parents=True, exist_ok=True)
    (EVIDENCE_DIR / "logs").mkdir(parents=True, exist_ok=True)

    record: dict = {
        "measurement": "MP-ACC1 Golden Creative Workflow acceptance attempt",
        "contract": "DC-MP-ACC1",
        "started_at": now_iso(),
        "verdict": "NOT_RUN",
        "harness": {
            "command": "python apps/desktop/devtools/runtime_integration/run_acc1.py",
            "port": PORT,
            "endpoint": ENDPOINT,
            "work_dir": str(work),
            "generation_timeout_s": args.timeout,
            "desktop_binary_launched": False,
            "observation": "protocol-level Desktop observation client (not the GUI)",
        },
        "desktop": {
            "root": str(DESKTOP_ROOT),
            "head": base.git_value(DESKTOP_ROOT, "rev-parse", "HEAD"),
            "binary": str(DESKTOP_EXE),
            "binary_sha256": sha256_file(DESKTOP_EXE) if DESKTOP_EXE.is_file() else "missing",
        },
        "runtime_source": {
            "root": str(RUNTIME_ROOT),
            "head": base.git_value(RUNTIME_ROOT, "rev-parse", "HEAD"),
            "python": str(RUNTIME_PYTHON),
            "dirty_tracked": [
                line
                for line in base.git_value(RUNTIME_ROOT, "status", "--porcelain=v1").splitlines()
                if line[:2].strip()
            ][:40],
        },
        "environment_preflight": probe_generation_plane(),
        "health": {},
        "recipe": {},
        "task": {},
        "projection_trace": [],
        "task_state_timeline": [],
        "resyncs": [],
        "failure_injection": {},
        "canonical_reads": {},
        "media": {},
        "outcome": {},
        "not_measured": [],
        "residuals": [],
        "leak_check": {},
    }

    secret = secrets.token_hex(24)
    handle_secret = secrets.token_hex(24)
    capabilities: list[str] = []  # opaque bearer capabilities, memory only
    extra_runtimes: list[IsolatedRuntime] = []
    token = ""

    db_copy = work / "generation.sqlite"
    presets = work / "cos_presets.json"
    runtime = IsolatedRuntime(
        work=work,
        db_copy=db_copy,
        presets=presets,
        secret=secret,
        handle_secret=handle_secret,
    )
    try:
        # --- 0. fresh DB copy + preset copy -------------------------------
        base.copy_database(SOURCE_DB, db_copy)
        shutil.copyfile(SOURCE_PRESETS, presets)
        record["source_state"] = {
            "source_db": str(SOURCE_DB),
            "source_db_sha256": sha256_file(SOURCE_DB),
            "db_copy": str(db_copy),
            "db_copy_sha256": sha256_file(db_copy),
            "db_copy_bytes": db_copy.stat().st_size,
            "presets_copy": str(presets),
            "presets_sha256": sha256_file(presets),
        }

        # --- 1. readiness + generation-plane health ----------------------
        start = runtime.start()
        record["runtime"] = {
            "pid": start["pid"],
            "server_identity": start["server_identity"],
            "server_build": start["server_build"],
            "epoch": start["epoch"],
            "protocol": start["protocol"],
            "schema": start["schema"],
            "config_path": str(runtime.config_path),
            "config_sha256": sha256_file(runtime.config_path),
            "discovery_path": str(runtime.discovery_path),
            "cas_root": str(runtime.cas_root),
            "log_dir": str(runtime.log_dir),
        }
        record["health"]["ready"] = start["ready"]
        record["health"]["hello"] = start["hello"]

        # The Runtime's own health probe runs a 1-token LLM completion with a
        # 90 s cold-start allowance, so the first report can stay null while the
        # model loads. Wait boundedly (3 min) and record the transition.
        status_responses: list[dict] = []
        deadline = time.monotonic() + 180.0
        while time.monotonic() < deadline:
            response = runtime.status()
            document = response.get("document") or {}
            health = document.get("health") or {}
            status_responses.append(
                {
                    "at": now_iso(),
                    "http_status": response["status"],
                    "health": health,
                    "loop": document.get("loop"),
                    "config": document.get("config"),
                }
            )
            if isinstance(health.get("llm"), bool) and isinstance(health.get("comfy"), bool):
                break
            time.sleep(3.0)
        latest = status_responses[-1] if status_responses else {}
        record["health"]["status_response_count"] = len(status_responses)
        record["health"]["status_responses"] = (
            status_responses[:2] + status_responses[-2:]
            if len(status_responses) > 4
            else status_responses
        )
        record["health"]["runtime_reported"] = latest.get("health") or {}
        generation_ready = bool(
            (latest.get("health") or {}).get("llm") is True
            and (latest.get("health") or {}).get("comfy") is True
        )
        record["health"]["generation_plane_ready"] = generation_ready
        record["health"]["runtime_loop"] = latest.get("loop")
        record["health"]["runtime_config"] = latest.get("config")

        # --- 2. credential + catalog -------------------------------------
        credential = runtime.issue_credential()
        credential_document = credential.get("document") or {}
        token = str(credential_document.get("token") or "")
        if credential["status"] != 200 or not token:
            raise RuntimeError(f"credential issuance failed ({credential['status']})")
        record["credential"] = {
            "http_status": credential["status"],
            "principal": credential_document.get("principal"),
            "scopes": credential_document.get("scopes"),
            "expires_in": credential_document.get("expires_in"),
            "token_recorded": False,
        }

        catalog_response = runtime.desktop_catalog(token)
        catalog_document = catalog_response.get("document") or {}
        catalog = catalog_document.get("catalog") or {}
        recipes = catalog.get("recipes") or []
        record["catalog"] = {
            "http_status": catalog_response["status"],
            "schema_version": catalog.get("schema_version"),
            "can_submit": catalog.get("can_submit"),
            "reason_codes": catalog.get("reason_codes"),
            "recipe_count": len(recipes),
            "recipes": [
                {
                    "recipe_ref": recipe.get("recipe_ref"),
                    "label": recipe.get("label"),
                    "execution_limits": recipe.get("execution_limits"),
                    "accepts_image_references": recipe.get("accepts_image_references"),
                    "parameter_ids": [
                        parameter.get("id") for parameter in (recipe.get("parameters") or [])
                    ],
                }
                for recipe in recipes
            ],
        }
        if not recipes:
            raise RuntimeError(
                f"catalog has no submittable recipe (can_submit={catalog.get('can_submit')},"
                f" reasons={catalog.get('reason_codes')})"
            )
        if not generation_ready:
            record["verdict"] = "PARTIAL"
            record["stop_reason"] = "generation_plane_not_ready"
            record["not_measured"].append(
                "golden workflow generation/evaluation/outcome: the Runtime reported the"
                " generation plane not ready"
            )
            return finish(
            record,
            runtime,
            work,
            secret,
            handle_secret,
            capabilities,
            extra_runtimes=extra_runtimes,
        )

        # --- 3. observation open -----------------------------------------
        observation = runtime.open_observation(token)
        observation_document = observation.get("document") or {}
        if observation["status"] != 200:
            raise RuntimeError(f"observation open failed ({observation['status']})")
        grant = observation_document.get("grant") or {}
        stamp_document = grant.get("stamp") or {}
        snapshot = observation_document.get("snapshot") or {}
        end = snapshot.get("end") or {}
        context_id = str(stamp_document.get("context_id") or "")
        cursor = str(end.get("cursor") or "")
        scope_id = str(stamp_document.get("scope_id") or "")
        for capability in (context_id, cursor, scope_id):
            if capability:
                capabilities.append(capability)
        if not context_id or not cursor:
            raise RuntimeError("observation grant/snapshot missing context or cursor")
        initial_mutations = snapshot_mutations(observation_document)
        record["observation_open"] = {
            "http_status": observation["status"],
            "read_revision": stamp_document.get("read_revision"),
            "epoch": stamp_document.get("epoch"),
            "server_id": stamp_document.get("server_id"),
            "context_sha256_16": short_hash(context_id),
            "scope_sha256_16": short_hash(scope_id),
            "cursor_sha256_16": short_hash(cursor),
            "snapshot_id": (snapshot.get("begin") or {}).get("snapshot_id"),
            "snapshot_mutations": len(initial_mutations),
            "snapshot_kind_counts": count_kinds(initial_mutations),
            "snapshot_expected": end.get("expected"),
            "snapshot_integrity_digest": end.get("integrity_digest"),
        }

        # --- 4. create task through the Desktop command path --------------
        chosen_recipe = recipes[0]
        target_recipe_ref = build_target_recipe_ref(chosen_recipe.get("recipe_ref") or {})
        objective = args.objective
        command_id = "cmd-" + secrets.token_hex(12)
        envelope = {
            "command_id": command_id,
            "target": {"target": "CREATION", "recipe_ref": target_recipe_ref},
            "expected_revision": None,
            "epoch": start["epoch"],
            "payload": {
                "kind": "CREATE_TASK",
                "objective_text": objective,
                # The production recipe dispatches only with an accepted prompt;
                # a prompt-less envelope is refused closed at dispatch
                # ("production task carries no prompt; refusing invented input").
                "text_prompt": objective,
                "reference_artifact_refs": [],
                "parameter_values": [],
            },
        }
        create_response = runtime.command(token, context_id, envelope)
        create_document = create_response.get("document") or {}
        receipt = create_document.get("receipt") or {}
        task_ref = str(receipt.get("task_id") or "")
        record["recipe"] = {
            "recipe_id": target_recipe_ref.get("recipe_id"),
            "label": chosen_recipe.get("label"),
            "objective_text": objective,
            "execution_limits": chosen_recipe.get("execution_limits"),
            "recipe_ref": target_recipe_ref,
            "source": "authenticated Desktop catalog (GET /api/cos/desktop/catalog)",
        }
        record["task"]["create"] = {
            "path": "POST /api/cos/desktop/commands (Desktop CommandEnvelope, target CREATION)",
            "command_id": command_id,
            "http_status": create_response["status"],
            "receipt": receipt,
            "stamp_read_revision": (create_document.get("stamp") or {}).get("read_revision"),
            "reject_reason": receipt.get("reject_reason"),
        }
        record["task"]["task_ref"] = task_ref
        if create_response["status"] != 200 or not task_ref:
            raise RuntimeError(
                f"create command was not accepted ({create_response['status']}: {create_document})"
            )

        # --- 5. observe the workflow -------------------------------------
        trace: list[dict] = []
        timeline: list[dict] = []
        resyncs: list[dict] = []
        canonical_timeline: list[dict] = []
        seen_state: dict | None = None
        last_outcome: dict | None = None
        last_artifact: dict | None = None
        injection: dict = {"attempted": False, "completed": False}
        injection_done = False
        observation_invalidated: dict | None = None
        pending_cursor = cursor
        pending_context = context_id
        deadline = time.monotonic() + args.timeout
        poll_started = time.monotonic()

        def apply_state(state: dict | None) -> None:
            nonlocal seen_state
            if state is not None and state != seen_state:
                timeline.append({"at": now_iso(), **state})
                seen_state = state

        while time.monotonic() < deadline:
            delta = runtime.deltas(token, pending_cursor, pending_context)
            if delta["status"] == 409 and (delta.get("document") or {}).get(
                "code"
            ) == "RESYNC_REQUIRED":
                # The Runtime's change journal cannot express the move as a
                # state diff. A real Desktop client reopens the observation and
                # continues from the fresh snapshot; record the resync.
                delta_error = delta.get("document") or {}
                if len(resyncs) >= 5:
                    resyncs.append(
                        {
                            "at": now_iso(),
                            "reason": "RESYNC_REQUIRED",
                            "delta_error": delta_error,
                            "action": "gave_up_after_5_resyncs",
                        }
                    )
                    break
                fresh = runtime.open_observation(token)
                fresh_document = fresh.get("document") or {}
                fresh_grant = (fresh_document.get("grant") or {}).get("stamp") or {}
                fresh_snapshot = fresh_document.get("snapshot") or {}
                fresh_end = fresh_snapshot.get("end") or {}
                fresh_mutations = snapshot_mutations(fresh_document)
                fresh_context = str(fresh_grant.get("context_id") or "")
                fresh_cursor = str(fresh_end.get("cursor") or "")
                resync_entry = {
                    "at": now_iso(),
                    "reason": "RESYNC_REQUIRED",
                    "delta_error": delta_error,
                    "reopen_http_status": fresh["status"],
                    "read_revision": fresh_grant.get("read_revision"),
                    "snapshot_mutations": len(fresh_mutations),
                    "snapshot_kind_counts": count_kinds(fresh_mutations),
                    "state_after": None,
                }
                if fresh["status"] != 200 or not fresh_context or not fresh_cursor:
                    resync_entry["action"] = "reopen_refused"
                    resync_entry["reopen_error"] = fresh.get("document")
                    resyncs.append(resync_entry)
                    observation_invalidated = {
                        "at": now_iso(),
                        "reason": "observation_reopen_refused",
                        "last_delta_error": delta_error,
                        "reopen_http_status": fresh["status"],
                        "reopen_error": fresh.get("document"),
                    }
                    break
                for capability in (fresh_context, fresh_cursor):
                    capabilities.append(capability)
                pending_cursor = fresh_cursor
                pending_context = fresh_context
                state_after = task_state_from_snapshot(fresh_document, task_ref)
                resync_entry["state_after"] = state_after
                apply_state(state_after)
                resyncs.append(resync_entry)
                record["resyncs"] = resyncs
                continue
            if delta["status"] != 200:
                trace.append(
                    {
                        "at": now_iso(),
                        "source": "delta",
                        "http_status": delta["status"],
                        "error": delta.get("document"),
                    }
                )
                # A closed failure is recorded, never hidden.
                break
            document = delta.get("document") or {}
            transaction = document.get("transaction")
            if transaction is None:
                time.sleep(POLL_INTERVAL_S)
                continue
            mutations = transaction.get("mutations", []) or []
            entry = {
                "at": now_iso(),
                "source": "delta",
                "transaction_id": transaction.get("transaction_id"),
                "mutation_count": transaction.get("mutation_count"),
                "integrity_digest": transaction.get("integrity_digest"),
                "kind_counts": count_kinds(mutations),
                "changes": [change_summary(mutation) for mutation in mutations],
            }
            trace.append(entry)
            pending_cursor = str(transaction.get("cursor") or pending_cursor)
            capabilities.append(pending_cursor)

            for mutation in mutations:
                state = task_state(mutation)
                if state is not None and str(state.get("task_ref")) == task_ref:
                    apply_state(state)
            outcome_projection = find_outcome(mutations, task_ref)
            if outcome_projection is not None:
                last_outcome = outcome_projection
            artifact_projection = find_task_artifact(mutations, task_ref, last_outcome)
            if artifact_projection is not None:
                last_artifact = artifact_projection

            # Failure injection: drop the observation session once the task is
            # non-terminal (ACTIVE) and resume through the protocol triplet.
            if (
                not injection_done
                and seen_state is not None
                and str(seen_state.get("task_status")) == "ACTIVE"
                and time.monotonic() - poll_started > 3.0
            ):
                injection_done = True
                injection["attempted"] = True
                injection["at"] = now_iso()
                injection["state_before"] = dict(seen_state)
                injection["cursor_before_sha256_16"] = short_hash(pending_cursor)
                # Drop the session: the client simply stops; no Runtime effect.
                time.sleep(2.0)
                resume_response = runtime.open_observation(
                    token,
                    resume={
                        "scope_id": scope_id,
                        "last_seen_cursor": pending_cursor,
                        "epoch": start["epoch"],
                    },
                )
                resume_document = resume_response.get("document") or {}
                resume_grant = resume_document.get("grant") or {}
                resume_stamp = resume_grant.get("stamp") or {}
                injection["resume"] = {
                    "http_status": resume_response["status"],
                    "replay": resume_document.get("replay"),
                    "read_revision": resume_stamp.get("read_revision"),
                    "context_sha256_16": short_hash(str(resume_stamp.get("context_id") or "")),
                    "snapshot_present": "snapshot" in resume_document,
                    "error": resume_response.get("document")
                    if resume_response["status"] != 200
                    else None,
                }
                if resume_response["status"] == 200:
                    new_context = str(resume_stamp.get("context_id") or "")
                    if new_context:
                        capabilities.append(new_context)
                        pending_context = new_context
                    injection["completed"] = True
                # The next delta must catch the projection up from the same cursor.
                catch_up = runtime.deltas(token, pending_cursor, pending_context)
                catch_up_document = catch_up.get("document") or {}
                catch_up_transaction = catch_up_document.get("transaction")
                injection["catch_up"] = {
                    "http_status": catch_up["status"],
                    "transaction_present": catch_up_transaction is not None,
                    "mutation_count": (catch_up_transaction or {}).get("mutation_count"),
                }
                if catch_up_transaction is not None:
                    injection["catch_up"]["kind_counts"] = count_kinds(
                        catch_up_transaction.get("mutations", []) or []
                    )
                    pending_cursor = str(
                        catch_up_transaction.get("cursor") or pending_cursor
                    )
                    capabilities.append(pending_cursor)
                record["failure_injection"] = injection

            if last_outcome is not None and seen_state is not None:
                if str(seen_state.get("task_status")) in TERMINAL_TASK_STATUS:
                    break

        record["projection_trace"] = trace
        record["task_state_timeline"] = timeline
        record["resyncs"] = resyncs
        record["failure_injection"] = injection
        record["observation_invalidated"] = observation_invalidated

        # --- 5b. canonical task tracking if the live projection is invalid
        # The Runtime invalidates observation state when another process claims
        # the database observation epoch (the generation worker does). The
        # Desktop task read still answers, so the attempt continues from the
        # canonical read and records exactly which channel broke.
        if observation_invalidated is not None:
            canonical_deadline = time.monotonic() + max(
                60.0, deadline - time.monotonic()
            )
            canonical_failures = 0
            while time.monotonic() < canonical_deadline:
                view = runtime.desktop_task(token, pending_context, task_ref)
                document = view.get("document") or {}
                task_payload = document.get("task") or {}
                if view["status"] != 200 or not task_payload:
                    canonical_failures += 1
                    if (
                        not canonical_timeline
                        or canonical_timeline[-1].get("http_status") != view["status"]
                    ):
                        canonical_timeline.append(
                            {
                                "at": now_iso(),
                                "http_status": view["status"],
                                "error": view.get("document"),
                            }
                        )
                    if canonical_failures >= 3:
                        break
                    time.sleep(3.0)
                    continue
                canonical_failures = 0
                state = {
                    key: task_payload.get(key)
                    for key in (
                        "task_ref",
                        "task_status",
                        "inner_phase",
                        "iteration_index",
                        "formal_outcome_ref",
                        "projection_revision",
                        "epoch",
                    )
                    if task_payload.get(key) is not None
                }
                if state and (
                    not canonical_timeline
                    or canonical_timeline[-1].get("state") != state
                ):
                    canonical_timeline.append(
                        {"at": now_iso(), "http_status": view["status"], "state": state}
                    )
                if str(task_payload.get("task_status")) in TERMINAL_TASK_STATUS:
                    seen_state = {
                        "task_ref": task_payload.get("task_ref"),
                        "task_status": task_payload.get("task_status"),
                        "inner_phase": task_payload.get("inner_phase"),
                        "iteration_index": task_payload.get("iteration_index"),
                        "formal_outcome_ref": task_payload.get("formal_outcome_ref"),
                        "revision": task_payload.get("projection_revision"),
                    }
                    break
                time.sleep(3.0)
            record["canonical_timeline"] = canonical_timeline

        terminal = seen_state is not None and str(seen_state.get("task_status")) in TERMINAL_TASK_STATUS
        record["task"]["terminal_state"] = seen_state
        record["task"]["timed_out"] = not terminal

        # --- 6. post-terminal projection recovery (new Runtime epoch) ------
        # Restarting is allowed only because the task is terminal. A new
        # process claims a new observation epoch, so the terminal projections
        # (TASK/ITERATION/CANDIDATE/ARTIFACT/EVALUATION/OUTCOME/GRAPH) can be
        # read again from an authoritative snapshot.
        post_terminal: dict = {"performed": False}
        if terminal:
            record["runtime_stop_epoch1"] = runtime.stop()
            recovery = IsolatedRuntime(
                work=work,
                db_copy=db_copy,
                presets=presets,
                secret=secret,
                handle_secret=handle_secret,
                phase="recovery",
            )
            extra_runtimes.append(recovery)
            start2 = recovery.start()
            record["runtime2"] = {
                "pid": start2["pid"],
                "server_identity": start2["server_identity"],
                "epoch": start2["epoch"],
                "protocol": start2["protocol"],
                "schema": start2["schema"],
                "epoch_changed": start2["epoch"] != start["epoch"],
            }
            credential2 = recovery.issue_credential()
            credential2_document = credential2.get("document") or {}
            token = str(credential2_document.get("token") or "")
            observation2 = recovery.open_observation(token)
            observation2_document = observation2.get("document") or {}
            grant2 = (observation2_document.get("grant") or {}).get("stamp") or {}
            snapshot2 = observation2_document.get("snapshot") or {}
            end2 = snapshot2.get("end") or {}
            context2 = str(grant2.get("context_id") or "")
            cursor2 = str(end2.get("cursor") or "")
            for capability in (context2, cursor2):
                if capability:
                    capabilities.append(capability)
            mutations2 = snapshot_mutations(observation2_document)
            delta_probe = {"performed": False}
            if cursor2 and context2:
                probe_response = recovery.deltas(token, cursor2, context2)
                delta_probe = {
                    "performed": True,
                    "http_status": probe_response["status"],
                    "transaction_present": bool(
                        (probe_response.get("document") or {}).get("transaction")
                    ),
                }
            post_terminal = {
                "performed": True,
                "reason": "post_terminal_runtime_restart_new_observation_epoch",
                "observation_http_status": observation2["status"],
                "read_revision": grant2.get("read_revision"),
                "epoch": grant2.get("epoch"),
                "context_sha256_16": short_hash(context2),
                "cursor_sha256_16": short_hash(cursor2),
                "snapshot_mutations": len(mutations2),
                "snapshot_kind_counts": count_kinds(mutations2),
                "terminal_task_projection": task_state_from_snapshot(
                    observation2_document, task_ref
                ),
                "outcome_projection": find_outcome(mutations2, task_ref),
                "artifact_projection": find_task_artifact(
                    mutations2, task_ref, find_outcome(mutations2, task_ref)
                ),
                "delta_probe": delta_probe,
                "credential_http_status": credential2["status"],
            }
            if post_terminal["outcome_projection"] is not None:
                last_outcome = post_terminal["outcome_projection"]
            if post_terminal["artifact_projection"] is not None:
                last_artifact = post_terminal["artifact_projection"]
            if context2:
                pending_context = context2
        record["post_terminal"] = post_terminal

        # --- 7. canonical reads ------------------------------------------
        canonical: dict = {}
        task_view = runtime.desktop_task(token, pending_context, task_ref) if terminal else None
        if task_view is not None:
            task_view_document = task_view.get("document") or {}
            canonical["desktop_task_view"] = {
                "http_status": task_view["status"],
                "task": compact(task_view_document.get("task")),
                "stamp_read_revision": (task_view_document.get("stamp") or {}).get(
                    "read_revision"
                ),
            }
            history = runtime.desktop_history(token, pending_context, task_ref)
            history_document = history.get("document") or {}
            canonical["desktop_history"] = {
                "http_status": history["status"],
                "items": history_document.get("items"),
                "has_more": history_document.get("has_more"),
            }
            candidates_response = runtime.candidates(token, task_ref)
            candidates_document = candidates_response.get("document") or {}
            canonical["candidates"] = {
                "http_status": candidates_response["status"],
                "document": compact(candidates_document),
            }
            candidate_items = (
                candidates_document.get("items")
                or candidates_document.get("candidates")
                or []
            )
            if candidate_items:
                first_candidate = candidate_items[0]
                candidate_id = str(
                    (first_candidate.get("ref") or {}).get("candidate_id")
                    or (first_candidate.get("candidate_ref") or {}).get("candidate_id")
                    or first_candidate.get("candidate_id")
                    or ""
                )
                if candidate_id:
                    candidate_ref = (
                        first_candidate.get("ref")
                        or first_candidate.get("candidate_ref")
                        or {}
                    )
                    evaluation_response = runtime.evaluation(
                        token,
                        task_ref,
                        candidate_id,
                        run_id=str(candidate_ref.get("run_id") or ""),
                        iteration=int(candidate_ref.get("iteration") or 0),
                        artifact_ref=str(candidate_ref.get("artifact_ref") or ""),
                    )
                    canonical["evaluation"] = {
                        "candidate_id": candidate_id,
                        "http_status": evaluation_response["status"],
                        "document": compact(evaluation_response.get("document")),
                    }
            details_response = runtime.task_details(token, task_ref)
            canonical["task_details"] = {
                "http_status": details_response["status"],
                "document": compact(details_response.get("document")),
            }
        record["canonical_reads"] = canonical

        # --- 8. formal outcome (projection only, never derived) -----------
        record["outcome"] = {
            "source": "OUTCOME projection (server-projected formal outcome)",
            "projection": last_outcome,
            "formal_status": (last_outcome or {}).get("formal_status"),
            "stop_reason": (last_outcome or {}).get("stop_reason"),
            "selected_refs": (last_outcome or {}).get("selected_refs"),
            "derived_locally": False,
        }

        # --- 9. media path: descriptor -> handle -> blob -------------------
        media: dict = {"available": False}
        artifact_digest = (last_artifact or {}).get("artifact_digest")
        if artifact_digest:
            descriptor_response = runtime.media_descriptor(
                token, pending_context, str(artifact_digest)
            )
            descriptor_document = descriptor_response.get("document") or {}
            handle = str(descriptor_document.get("access_handle") or "")
            descriptor_record = {
                key: descriptor_document.get(key)
                for key in (
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
                )
                if descriptor_document.get(key) is not None
            }
            media = {
                "available": False,
                "descriptor_http_status": descriptor_response["status"],
                "descriptor": descriptor_record,
                "handle_present": bool(handle),
                "handle_recorded": False,
            }
            if handle:
                capabilities.append(handle)
                blob = runtime.media_blob(token, handle)
                raw = blob.get("raw") or b""
                blob_digest = sha256_bytes(raw)
                dimensions = png_dimensions(raw)
                image_path = EVIDENCE_DIR / "artifact.png"
                if raw:
                    image_path.write_bytes(raw)
                media.update(
                    {
                        "blob_http_status": blob["status"],
                        "bytes": len(raw),
                        "sha256": blob_digest,
                        "digest_matches_descriptor": blob_digest == str(artifact_digest),
                        "content_type": blob.get("headers", {}).get("content-type"),
                        "png_dimensions": dimensions,
                        "image_file": str(image_path.relative_to(DESKTOP_ROOT)),
                        "image_file_sha256": sha256_file(image_path) if raw else None,
                        "display_state": (
                            "bytes_fetched_verified_and_decoded"
                            if raw and dimensions
                            else "bytes_fetched" if raw else "not_fetched"
                        ),
                        "desktop_native_render": "NOT_MEASURED (Desktop GUI not launched)",
                    }
                )
                media["available"] = bool(
                    blob["status"] == 200
                    and blob_digest == str(artifact_digest)
                    and dimensions is not None
                )
        record["media"] = media

        # --- 10. verdict ---------------------------------------------------
        if observation_invalidated is not None:
            record["observation_epoch_conflict"] = {
                "finding": (
                    "the generation worker process claims the single"
                    " cos_desktop_observation_epoch row, so the serving Runtime's"
                    " observation cursors/contexts and fresh observation opens fail"
                    " closed with RESYNC_REQUIRED ('observation epoch changed in the"
                    " database ledger') for the rest of that process lifetime"
                ),
                "detected_at": observation_invalidated.get("at"),
                "recovery": "post-terminal Runtime restart (new process epoch) reopened observation",
                "desktop_impact": (
                    "a live Desktop observation would be invalidated mid-generation;"
                    " the native withdrawal/recovery was NOT_MEASURED"
                ),
                "runtime_owner": "C:/AI/orchestrator (observation epoch ledger)",
            }
        if last_outcome is not None and seen_state is not None:
            record["verdict"] = "LIMITED"
            record["stop_reason"] = "bounded_workflow_reached_formal_outcome"
            record["measured_scope"] = [
                "catalog fetch (authenticated Desktop read)",
                "Desktop CommandEnvelope create submission accepted (receipt)",
                "automatic Runtime dispatch (drain claim; no Desktop Start command)",
                "ASSIGNED -> ACTIVE task projection transition (live observation)",
                "canonical Desktop task read while the live observation was invalid",
                "post-terminal Runtime restart: new epoch, terminal TASK projection,"
                " OUTCOME/CANDIDATE/EVALUATION/ARTIFACT projections",
                "RC-13 media descriptor -> handle -> blob byte fetch",
                "formal outcome read from the OUTCOME projection",
                "reopen-from-history read (Desktop history page)",
            ]
        else:
            record["verdict"] = "PARTIAL"
            record["stop_reason"] = "workflow_did_not_reach_terminal_outcome"
        record["not_measured"].extend(
            [
                "DD-25 Start command and Next Iteration advance: DD-25 is still"
                " RUNTIME_CONTRACT_REQUIRED + DESIGN_DECISION_REQUIRED, so those steps"
                " are blocked and NOT_MEASURED (the Runtime drain auto-dispatches create).",
                "Desktop GUI rendering / native artifact display: the release binary was"
                " not launched in this attempt.",
                "Five of the six DC-MP-ACC1 failure injections (generation failure,"
                " evaluation failure, missing media, stop request, Desktop restart):"
                " not attempted; the observation invalidation was the one failure"
                " exercised, and it was produced by the Runtime rather than injected.",
                "Compare interaction (selecting two candidates in the GUI): only the"
                " Runtime candidate/evaluation reads were exercised.",
                "Live projection continuity across the whole generation: the Runtime"
                " observation epoch was invalidated by the generation worker, so the"
                " mid-generation transitions are measured only through the canonical"
                " task read, not through the observation delta stream.",
            ]
        )
        record["residuals"] = [
            "This is a bounded acceptance attempt, not product acceptance.",
            "Generation used the single existing catalog recipe"
            " (recipe-cos-production-still) and one objective; no breadth claim.",
            "ComfyUI was started for this run with the documented script and stopped"
            " afterwards; the prior state (not running) was restored.",
            "The Runtime checkout was not modified by this attempt; its head and"
            " pre-existing dirty files are recorded in runtime_source.",
        ]
        return finish(
            record,
            runtime,
            work,
            secret,
            handle_secret,
            capabilities,
            extra_runtimes=extra_runtimes,
        )
    except Exception as error:  # noqa: BLE001 - the attempt records its own failure
        record["verdict"] = "NOT_RUN"
        record["stop_reason"] = "harness_or_prerequisite_failure"
        record["error"] = f"{type(error).__name__}: {error}"
        record["runtime_log_tail"] = runtime.log_tail()
        return finish(
            record,
            runtime,
            work,
            secret,
            handle_secret,
            capabilities,
            extra_runtimes=extra_runtimes,
        )


def finish(
    record: dict,
    runtime: IsolatedRuntime,
    work: Path,
    secret: str,
    handle_secret: str,
    capabilities: list[str],
    extra_runtimes: list[IsolatedRuntime] | None = None,
) -> int:
    """Stop the Runtime(s), write evidence, run the leak check, verify cleanup."""
    record["finished_at"] = now_iso()
    terminal_state = (record.get("task") or {}).get("terminal_state") or {}
    record["runtime_stop_after_terminal"] = (
        str(terminal_state.get("task_status")) in TERMINAL_TASK_STATUS
    )
    record["runtime_stop"] = runtime.stop()
    record["runtime_stopped"] = not base.pid_alive(record["runtime_stop"].get("pid") or 0)
    if extra_runtimes:
        record["extra_runtime_stops"] = [extra.stop() for extra in extra_runtimes]
        record["extra_runtimes_stopped"] = all(
            not base.pid_alive(stop.get("pid") or 0)
            for stop in record["extra_runtime_stops"]
        )
    record["port_free_after_stop"] = port_free()
    record["discovery_removed"] = not runtime.discovery_path.exists()

    secrets_list = [secret, handle_secret, *capabilities]
    logs = []
    for candidate in [runtime, *(extra_runtimes or [])]:
        sanitized = sanitize_log(candidate.log_path, secrets_list)
        logs.append(
            {
                "file": candidate.log_path.name,
                "sha256": sanitized["sha256"],
                "text": sanitized["text"],
            }
        )
    combined = "\n\n".join(f"=== {entry['file']} ===\n{entry['text']}" for entry in logs)
    (EVIDENCE_DIR / "logs" / "runtime.log.sanitized.txt").write_text(
        combined, encoding="utf-8"
    )
    evidence_relative = EVIDENCE_DIR.relative_to(DESKTOP_ROOT).as_posix()
    record["logs"] = {
        "runtime_log_sanitized": f"{evidence_relative}/logs/runtime.log.sanitized.txt",
        "runtime_log_sha256": sha256_bytes(combined.encode("utf-8")),
        "raw_logs_retained_in_work_dir": [str(entry.log_path) for entry in [runtime, *(extra_runtimes or [])]],
    }
    write_json(EVIDENCE_DIR / "record.json", record)

    # Leak check: no secret/capability may appear in any published evidence file.
    findings: list[dict] = []
    for path in sorted(EVIDENCE_DIR.rglob("*")):
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for secret_value in secrets_list:
            if secret_value and secret_value in text:
                findings.append({"file": path.name, "kind": "capability_or_secret"})
        for marker in ("mh1.", "Bearer "):
            if marker in text and path.name != "runtime.log.sanitized.txt":
                findings.append({"file": path.name, "kind": f"marker:{marker}"})
    record["leak_check"] = {
        "secrets_checked": len([value for value in secrets_list if value]),
        "findings": findings,
        "clean": not findings,
    }
    write_json(EVIDENCE_DIR / "record.json", record)

    session = {
        "measurement": record["measurement"],
        "date": record["started_at"],
        "verdict": record["verdict"],
        "harness": record["harness"],
        "desktop": record["desktop"],
        "runtime": record["runtime_source"],
        "environment_preflight": record["environment_preflight"],
        "runtime_reported_health": record.get("health", {}).get("runtime_reported"),
        "generation_plane_ready": record.get("health", {}).get("generation_plane_ready"),
        "recipe": record.get("recipe", {}).get("recipe_id"),
        "task_ref": record.get("task", {}).get("task_ref"),
        "outcome": {
            "formal_status": record.get("outcome", {}).get("formal_status"),
            "stop_reason": record.get("outcome", {}).get("stop_reason"),
            "source": record.get("outcome", {}).get("source"),
        },
        "media_available": record.get("media", {}).get("available"),
        "leak_check_clean": record.get("leak_check", {}).get("clean"),
        "files": {
            "record": f"{evidence_relative}/record.json",
            "session": f"{evidence_relative}/session.json",
            "artifact_image": record.get("media", {}).get("image_file"),
            "logs": f"{evidence_relative}/logs/",
        },
    }
    write_json(EVIDENCE_DIR / "session.json", session)
    print(json.dumps(session, ensure_ascii=False, indent=2, default=str))
    return 0 if record["verdict"] in ("LIMITED", "PARTIAL") else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--timeout",
        type=float,
        default=GENERATION_TIMEOUT_S,
        help="bounded workflow wait in seconds (default 1500)",
    )
    parser.add_argument(
        "--objective",
        default="sunlit botanical still life, soft studio light",
        help="objective text for the created task",
    )
    parser.add_argument(
        "--evidence-dir",
        default=None,
        help=(
            "evidence directory to write (default: the published acc1-latest"
            " directory); a follow-up run must pass its own directory so the"
            " existing evidence is never rewritten"
        ),
    )
    args = parser.parse_args(argv)
    for required in (RUNTIME_PYTHON, SOURCE_DB, CONFIG_TEMPLATE, SOURCE_PRESETS):
        if not Path(required).exists():
            print(f"missing prerequisite: {required}", file=sys.stderr)
            return 2
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
