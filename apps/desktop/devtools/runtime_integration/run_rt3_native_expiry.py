#!/usr/bin/env python3
"""RT3 media-expiry residual campaign: native Expired UI, 300 s default TTL, ephemeral 401.

The three remaining NOT_MEASURED items of ``rt3-residual-latest`` are:

1. **Native Expired UI.**  A real 60 s media descriptor is delivered late on
   purpose (a harness-only ASGI delivery delay after the Runtime has minted it),
   so the Desktop client observes the real server-issued expiry.  The native
   artifact view is first observed with an admitted image; a Runtime restart
   changes the epoch/principal and forces a fresh media load.  The exact UIA
   state, screenshots and the Runtime request sequence are recorded.
2. **300 s default TTL.**  A Runtime with no TTL override mints a descriptor;
   the advertised expiry (and the handle ``exp``) must be ~300 s and the blob
   must fetch.  The actual 300 s expiry transition is NOT waited for.
3. **Ephemeral handle secret.**  A Runtime with no
   ``ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`` is restarted; the old handle fails
   MAC verification with 401 (not 410), and a fresh descriptor+handle works.
   The Desktop client mapping (401 -> ``Unauthorized`` -> ``Forbidden``) is
   fixed by the app-core mapping test and recorded here.

This is evidence/test scaffolding only.  It does not modify product or Runtime
code, never generates media, and never writes a token, context, principal or
access handle to evidence.  Runtime logs are sanitized; a leak scan is part of
the output.  The production DB is copied read-only (the native task comes from
the prior isolated closure run, whose capability seed is test-only material).

Usage (from the repository root)::

    python apps/desktop/devtools/runtime_integration/run_rt3_native_expiry.py
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
import run_acc1_native as acc1  # noqa: E402 - local test helper, stdlib only
import run_integration as base  # noqa: E402 - local test helper, stdlib only
import run_rt3_residual as residual  # noqa: E402 - local test helper, stdlib only

ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
RUNTIME_PYTHON = base.RUNTIME_PYTHON
DESKTOP_EXE = base.DESKTOP_EXE
SOURCE_PRESETS = acc1.SOURCE_PRESETS
DELAY_LAUNCHER = HARNESS / "isolated_runtime_descriptor_delay.py"

EVIDENCE = (
    ROOT / "docs" / "development" / "evidence-published" / "media-expiry-native-latest"
)
WORK_ROOT = ROOT / "target" / "media-expiry-native"
SOURCE_RUN = ROOT / "target" / "acc1-native" / "20261109T233420-golden"
TASK_REF = "task-ab47119a8bec467a8e128d7b82f1220a"
PORT_NATIVE = 8132
PORT_PROTOCOL = 8165

MEDIA_DISPLAYED_MARKER = "メディア画像（"
MEDIA_EXPIRED_TEXT = (
    "画像を利用できません: メディア記述子の有効期限が切れました。再取得が必要です。"
)
MEDIA_MISSING_TEXT = "画像を利用できません: この成果物のメディアが見つかりません。"
MEDIA_FORBIDDEN_TEXT = (
    "画像を利用できません: このセッションにはメディアの閲覧権限がありません。"
)
VIEWPORT_MARKERS = ("全体表示 (0)", "物理ピクセル1:1 (1)", "拡大 (+)", "縮小 (-)")
DESCRIPTOR_DELAY_SECONDS = 63.0
TTL_NATIVE_SECONDS = 60
TTL_DEFAULT_SECONDS = 300


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


def sha256_bytes(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def short_hash(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def redact_document(value: object) -> object:
    if isinstance(value, str):
        return acc1.redact_text(value)
    if isinstance(value, list):
        return [redact_document(item) for item in value]
    if isinstance(value, dict):
        return {key: redact_document(item) for key, item in value.items()}
    return value


def run_git(*args: str) -> str:
    completed = subprocess.run(
        ["git", *args], capture_output=True, text=True, timeout=30, cwd=str(ROOT)
    )
    return (completed.stdout or "").lstrip("\ufeff").rstrip("\r\n")


def port_open(port: int) -> bool:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) == 0
    finally:
        probe.close()


def wait_port_free(port: int, timeout: float = 30.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not port_open(port):
            return True
        time.sleep(0.2)
    return not port_open(port)


def media_lines(names: list[str], limit: int = 40) -> list[str]:
    return [
        line
        for line in names
        if any(
            needle in line
            for needle in ("メディア", "画像", "成果物", "記述子", "再取得", "閲覧権限")
        )
    ][:limit]


def media_state(names: list[str]) -> str:
    text = "\n".join(names)
    if MEDIA_EXPIRED_TEXT in text:
        return "EXPIRED"
    if MEDIA_DISPLAYED_MARKER in text:
        return "DISPLAYED"
    if MEDIA_FORBIDDEN_TEXT in text:
        return "FORBIDDEN"
    if MEDIA_MISSING_TEXT in text:
        return "MISSING"
    if "画像を利用できません" in text:
        return "UNAVAILABLE"
    if "メディア画像を準備中" in text:
        return "PREPARING"
    return "UNKNOWN"


def viewport_present(names: list[str]) -> bool:
    # Only the viewer controls, not the hint sentence that names the same keys.
    return any(
        line in {"全体表示 (0)", "物理ピクセル1:1 (1)", "拡大 (+)", "縮小 (-)"}
        for line in names
    )


def capture(ui: acc1.Ui, label: str) -> dict:
    """Persist one UIA dump and screenshot under the evidence directory."""
    uia_dir = EVIDENCE / "uia"
    shot_dir = EVIDENCE / "shots"
    uia_dir.mkdir(parents=True, exist_ok=True)
    shot_dir.mkdir(parents=True, exist_ok=True)
    dump = ui.probe("dump")
    raw = {key: value for key, value in dump.items() if not key.startswith("_")}
    uia_path = uia_dir / f"{label}.json"
    uia_path.write_text(
        json.dumps(raw, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )
    image = shot_dir / f"{label}.png"
    shot = ui.probe("shot", shot=image)
    names = list(dump.get("names") or [])
    return {
        "label": label,
        "at": now_iso(),
        "state": media_state(names),
        "elements": dump.get("elements"),
        "window": {
            "title": dump.get("title"),
            "class": dump.get("class"),
            "width": dump.get("width"),
            "height": dump.get("height"),
        },
        "uia_file": str(uia_path.relative_to(EVIDENCE)),
        "uia_sha256": sha256_file(uia_path),
        "names_sha256": sha256_bytes("\n".join(names).encode("utf-8")),
        "names_count": len(names),
        "media_lines": media_lines(names),
        "viewport_present": viewport_present(names),
        "has_displayed_marker": MEDIA_DISPLAYED_MARKER in "\n".join(names),
        "refresh_controls": [
            line
            for line in names
            if line in {"再取得", "再読み込み", "更新", "再試行", "リロード"}
        ],
        "screenshot": {
            "file": str(image.relative_to(EVIDENCE)),
            "sha256": shot.get("_image_sha256"),
            "bytes": shot.get("_image_bytes"),
            "printed": shot.get("ok"),
            "width": shot.get("width"),
            "height": shot.get("height"),
        },
    }


def wait_for_state(ui: acc1.Ui, target: str, timeout: float) -> tuple[dict, list[str], bool]:
    deadline = time.monotonic() + timeout
    last_dump: dict = {}
    last_names: list[str] = []
    while time.monotonic() < deadline:
        last_dump = ui.probe("dump")
        last_names = list(last_dump.get("names") or [])
        if media_state(last_names) == target:
            return last_dump, last_names, True
        if not base.pid_alive(ui.pid):
            break
        time.sleep(1.5)
    return last_dump, last_names, False


def log_since(path: Path, offset: int) -> str:
    with contextlib.suppress(OSError):
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            handle.seek(offset)
            return handle.read()
    return ""


# --------------------------------------------------------------------------- native Runtime


class DelayRuntime(acc1.NativeRuntime):
    """NativeRuntime with the annotation-free descriptor delivery delay."""

    delay_seconds: float = 0.0
    delay_flag: Path | None = None

    def _environment(self) -> dict:
        environment = super()._environment()
        environment["ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS"] = str(
            TTL_NATIVE_SECONDS
        )
        environment["ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_SECONDS"] = (
            f"{self.delay_seconds:.3f}"
        )
        if self.delay_flag is not None:
            environment["ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_FLAG"] = str(
                self.delay_flag
            )
        return environment


def scenario_native() -> dict:
    started_at = now_iso()
    record: dict = {
        "label": "native-expiry",
        "started_at": started_at,
        "verdict": "NOT_RUN",
        "precondition": {},
        "initial_display": {},
        "expiry_sequence": {},
        "native_expired": {},
        "fallback_cold_relaunch": {},
        "fresh_fetch_after_expiry": {},
        "runtime_requests": {},
        "cleanup": {},
        "residuals": [],
    }
    secrets_list: list[str] = []
    work = WORK_ROOT / f"{time.strftime('%Y%m%dT%H%M%S')}-native"
    work.mkdir(parents=True, exist_ok=True)
    delay_flag = work / "descriptor-delay.flag"
    with contextlib.suppress(FileNotFoundError):
        delay_flag.unlink()

    acc1.WORK_ROOT = work / "ctx"
    acc1.NativeRuntime = DelayRuntime
    acc1.CONTROL_LAUNCHER = DELAY_LAUNCHER
    ctx = acc1.Ctx("expiry", PORT_NATIVE, control=True, source_run=SOURCE_RUN)
    ctx.runtime.delay_seconds = 0.0
    ctx.runtime.delay_flag = delay_flag
    ui: acc1.Ui | None = None
    cleanup: dict = {}
    try:
        start1 = ctx.start_runtime(wait_generation=False)
        secrets_list.extend([ctx.secret, ctx.handle_secret, ctx.token])
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline and not ctx.runtime.discovery_path.is_file():
            time.sleep(0.2)
        record["precondition"] = {
            "task_ref": TASK_REF,
            "source_run": str(SOURCE_RUN),
            "desktop_binary": {
                "path": str(DESKTOP_EXE),
                "sha256": sha256_file(DESKTOP_EXE),
                "bytes": DESKTOP_EXE.stat().st_size,
                "built_at": time.strftime(
                    "%Y-%m-%dT%H:%M:%S", time.localtime(DESKTOP_EXE.stat().st_mtime)
                ),
            },
            "desktop_head": run_git("rev-parse", "HEAD"),
            "runtime_head": subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=str(RUNTIME_ROOT),
                capture_output=True,
                text=True,
                timeout=30,
            ).stdout.strip(),
            "runtime_epoch_1": start1["epoch"],
            "runtime_server_identity": start1["server_identity"],
            "ttl_seconds": TTL_NATIVE_SECONDS,
            "descriptor_delay_seconds": DESCRIPTOR_DELAY_SECONDS,
            "delay_mechanism": (
                "harness-only ASGI wrapper sleeps after the real Runtime has minted"
                " the descriptor response and before its bytes are written; no"
                " descriptor/handle document is rewritten"
            ),
            "db_copy_sha256": ctx.record.get("db_copy_sha256"),
            "db_copy_bytes": ctx.record.get("db_copy_bytes"),
        }
        task = ctx.projection_task(TASK_REF)
        record["precondition"]["task_projection"] = {
            key: task.get(key)
            for key in ("title", "task_status", "inner_phase", "capabilities_ref")
        }
        goal = str(task.get("title") or "")
        if not goal:
            record["verdict"] = "NOT_RUN"
            record["residuals"].append("the reused DB carries no TASK projection")
            return record
        capability = ctx.wait_capability_entry(
            TASK_REF, "media", "ORIGINAL", lambda support: support == "SUPPORTED", 45.0
        )
        record["precondition"]["capability_media"] = capability

        connected = ctx.launch_desktop("expiry")
        ui = ctx.app.ui if ctx.app is not None else None
        record["initial_display"]["connected"] = connected
        select = ctx.select_task(goal, "expiry-task-selected", TASK_REF)
        record["initial_display"]["task_selected"] = select
        view_before = ctx.view_artifact(TASK_REF, "expiry-before")
        record["initial_display"]["artifact_surface"] = view_before
        record["initial_display"]["uia"] = capture(ui, "native-displayed")
        resolved = ctx.resolve_artifact(TASK_REF)
        digest = str((resolved.get("artifact") or {}).get("artifact_digest") or "")
        record["initial_display"]["resolved_artifact"] = {
            "artifact": resolved.get("artifact"),
            "descriptor_http_status": resolved.get("descriptor_http_status"),
            "descriptor": residual.descriptor_summary(
                resolved.get("descriptor") or {}
            ),
        }
        if not digest:
            record["verdict"] = "NOT_RUN"
            record["residuals"].append("the artifact digest was not projected")
            return record
        if view_before.get("state") not in {"DISPLAYED", "AVAILABLE"}:
            record["verdict"] = "NOT_RUN"
            record["residuals"].append(
                f"the native artifact view did not display before expiry "
                f"({view_before.get('state')})"
            )
            return record

        # Let the descriptor/handle minted for the display pass its 60 s TTL
        # with margin (the display's descriptor is older than this sleep).
        wait_started = time.time()
        time.sleep(70.0)
        record["expiry_sequence"] = {
            "waited_after_display_seconds": round(time.time() - wait_started, 3),
            "at": now_iso(),
            "at_unix": time.time(),
            "image_displayed_before_expiry": True,
            "native_state_before_restart": media_state(
                list(ui.probe("dump").get("names") or [])
            ),
        }

        # Do not leave the Artifact surface active across the restart: the
        # shell must not start a media resolution from the previous lease with
        # the old credential before the same-process reauthorization finishes.
        # This keeps the same-process probe below a clean test of the defect
        # path instead of a race decided by timing.
        flow_before_stop = ui.invoke("処理の流れ  Ctrl+3")
        time.sleep(1.0)
        record["expiry_sequence"]["flow_before_restart"] = flow_before_stop
        stop1 = ctx.runtime.stop()
        record["expiry_sequence"]["runtime_1_stop"] = stop1
        log_offset = ctx.runtime.log_path.stat().st_size
        # The delay flag stays absent for the same-process probe; the launcher
        # is armed with the delay budget at process start and applies it only
        # while the flag file exists.
        ctx.runtime.delay_seconds = DESCRIPTOR_DELAY_SECONDS
        with contextlib.suppress(FileNotFoundError):
            delay_flag.unlink()
        start2 = ctx.runtime.start()
        record["expiry_sequence"]["runtime_2"] = {
            "pid": start2.get("pid"),
            "epoch": start2.get("epoch"),
            "server_identity": start2.get("server_identity"),
            "epoch_changed": start2.get("epoch") != start1.get("epoch"),
            "descriptor_delay_seconds": DESCRIPTOR_DELAY_SECONDS,
            "delay_flag": False,
            "at": now_iso(),
        }
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline and not ctx.runtime.discovery_path.is_file():
            time.sleep(0.2)
        record["expiry_sequence"]["discovery_reappeared"] = (
            ctx.runtime.discovery_path.is_file()
        )

        # Wait for the app's bounded reauthorization (new credential + fresh
        # observation) before touching the Artifact surface.
        reconnect: dict = {"at": now_iso(), "seen": False, "log_lines": []}
        deadline = time.monotonic() + 120.0
        while time.monotonic() < deadline:
            new_log = log_since(ctx.runtime.log_path, log_offset)
            reconnect["log_lines"] = [
                line
                for line in new_log.splitlines()
                if "credentials" in line or "observation" in line
            ][-12:]
            if (
                'POST /api/cos/desktop/credentials HTTP/1.1" 200' in new_log
                and 'POST /api/cos/desktop/observation HTTP/1.1" 200' in new_log
            ):
                reconnect["seen"] = True
                break
            if not ctx.app.alive():
                break
            time.sleep(2.0)
        reconnect["at_after"] = now_iso()
        record["expiry_sequence"]["same_process_reconnect"] = reconnect
        time.sleep(2.0)

        # Same-process probe: the production media binding captured the startup
        # credential, so this resolution is expected to be refused with 401
        # even after the observation client reauthorized. It is recorded as a
        # defect reproduction, not as the Expired measurement.
        probe_started = time.time()
        artifact_after = ui.invoke("成果物  Ctrl+1")
        if not artifact_after.get("invoked"):
            artifact_after = ui.click("成果物  Ctrl+1")
        _, probe_names, probe_expired = wait_for_state(ui, "EXPIRED", 20.0)
        probe_state = media_state(probe_names)
        if not probe_expired and probe_state == "UNKNOWN":
            # The media decision may still be in flight; give it one bounded wait
            # for the visible forbidden sentence.
            _, probe_names, _ = wait_for_state(ui, "FORBIDDEN", 20.0)
            probe_state = media_state(probe_names)
        probe_log = log_since(ctx.runtime.log_path, log_offset)
        record["same_process_probe"] = {
            "at": now_iso(),
            "artifact_invoke": artifact_after,
            "state": probe_state,
            "exact_expired_text_present": MEDIA_EXPIRED_TEXT
            in "\n".join(probe_names),
            "forbidden_text_present": MEDIA_FORBIDDEN_TEXT in "\n".join(probe_names),
            "uia": capture(ui, "native-same-process-after-restart"),
            "media_get_lines": [
                line
                for line in probe_log.splitlines()
                if "/api/cos/media/" in line and "GET " in line
            ][-12:],
            "elapsed_seconds": round(time.time() - probe_started, 3),
            "defect": (
                "the media binding keeps the startup credential; after the Runtime"
                " restart the descriptor is refused 401 and the pane shows the"
                " Forbidden sentence instead of Expired, without a retry"
            )
            if (MEDIA_FORBIDDEN_TEXT in "\n".join(probe_names))
            else None,
        }

        # The native Expired state is measured through a cold relaunch: a new
        # process rebuilds the media binding with the current credential, while
        # the prior process had displayed the same admitted artifact. The
        # descriptor delivery delay is armed only after the cold process has
        # connected and selected the task, so no unrelated media request is
        # delayed.
        closed = ctx.app.close()
        base.cleanup_credential_store(ctx.runtime.endpoint)
        app = acc1.NativeApp(
            label="expiry-cold",
            work=ctx.work,
            secret=ctx.secret,
            discovery=ctx.runtime.discovery_path,
        )
        pid = app.launch()
        ctx.app = app
        ui = app.ui
        window = app.wait_window(60.0)
        _, text, connected_cold = app.wait_connected(90.0)
        cold: dict = {
            "at": now_iso(),
            "at_unix": time.time(),
            "close": closed,
            "pid": pid,
            "window_found": window.get("found"),
            "connected": connected_cold,
        }
        if connected_cold:
            ui.invoke("処理の流れ  Ctrl+3")
            cold["task_selected"] = ctx.select_task(goal, "expiry-cold-task", TASK_REF)
            # The artifact viewer resolves only for the viewed candidate; mark
            # it while the shell is still off the Artifact surface, then arm the
            # delivery delay before view_candidate's final Artifact switch.
            ui.invoke("処理の流れ  Ctrl+3")
            time.sleep(1.0)
            cold_log_offset = ctx.runtime.log_path.stat().st_size
            delay_flag.write_text("delay", encoding="utf-8")
            cold["delay_armed_at"] = now_iso()
            cold["view_candidate"] = ctx.view_candidate(TASK_REF)
            _, cold_names, cold_expired = wait_for_state(ui, "EXPIRED", 240.0)
            cold["expired_found"] = cold_expired
            cold["state"] = media_state(cold_names)
            cold["uia"] = capture(ui, "native-expired-cold")
            cold["exact_text_present"] = MEDIA_EXPIRED_TEXT in "\n".join(cold_names)
            cold["stale_image_absent"] = MEDIA_DISPLAYED_MARKER not in "\n".join(
                cold_names
            )
            cold["viewport_absent"] = not viewport_present(cold_names)
            cold["refresh_affordance"] = {
                "short_control_names": cold["uia"].get("refresh_controls"),
                "sentence_says_refetch_needed": "再取得が必要です"
                in "\n".join(cold_names),
                "actionable_control_present": bool(
                    cold["uia"].get("refresh_controls")
                ),
            }
            cold["uia_names_excerpt"] = [
                line for line in cold_names if "画像" in line or "メディア" in line
            ][:20]
            cold["elapsed_seconds"] = round(
                time.time() - cold["at_unix"], 3
            )
            cold_log = log_since(ctx.runtime.log_path, cold_log_offset)
            cold["delay_begins"] = cold_log.count(
                "harness media descriptor delay begin"
            )
            cold["delay_releases"] = cold_log.count(
                "harness media descriptor delay release"
            )
            cold["media_get_lines"] = [
                line for line in cold_log.splitlines() if "/api/cos/media/" in line
            ][-12:]
            cold["blob_get_lines"] = [
                line
                for line in cold_log.splitlines()
                if "/blob" in line and "GET " in line
            ]
            record["native_expired"] = {
                "found": cold_expired,
                "state": cold["state"],
                "exact_text_expected": MEDIA_EXPIRED_TEXT,
                "exact_text_present": cold["exact_text_present"],
                "stale_image_absent": cold["stale_image_absent"],
                "viewport_absent": cold["viewport_absent"],
                "uia": cold["uia"],
                "refresh_affordance": cold["refresh_affordance"],
            }
            # Fresh fetch after the flag is removed: proves the Runtime still
            # serves the canonical bytes and the expired pane did not fabricate
            # or retain bytes.
            with contextlib.suppress(FileNotFoundError):
                delay_flag.unlink()
            credential = ctx.runtime.issue_credential()
            token = str((credential.get("document") or {}).get("token") or "")
            secrets_list.append(token)
            observation = ctx.runtime.open_observation(token)
            document = observation.get("document") or {}
            context = str(
                ((document.get("grant") or {}).get("stamp") or {}).get("context_id")
                or ""
            )
            descriptor = ctx.runtime.media_descriptor(token, context, digest)
            descriptor_document = descriptor.get("document") or {}
            handle = str(descriptor_document.get("access_handle") or "")
            blob = ctx.runtime.media_blob(token, handle) if handle else {}
            raw = bytes(blob.get("raw") or b"")
            payload = residual.decode_handle(handle).get("payload") or {}
            record["fresh_fetch_after_expiry"] = {
                "at": now_iso(),
                "credential_http_status": credential.get("status"),
                "observation_http_status": observation.get("status"),
                "descriptor_http_status": descriptor.get("status"),
                "descriptor": residual.descriptor_summary(descriptor_document),
                "handle_expires_in_seconds": round(
                    float(payload.get("exp") or 0) - time.time(), 3
                ),
                "blob_http_status": blob.get("status"),
                "blob_bytes": len(raw),
                "blob_sha256": sha256_bytes(raw) if raw else None,
                "digest_matches_descriptor": (
                    sha256_bytes(raw) == descriptor_document.get("artifact_digest")
                )
                if raw
                else False,
                "delay_flag_removed": not delay_flag.exists(),
            }
        record["fallback_cold_relaunch"] = cold
        record["verdict"] = (
            "MEASURED"
            if (
                record["native_expired"].get("exact_text_present")
                and record["native_expired"].get("stale_image_absent")
                and record["native_expired"].get("uia", {}).get(
                    "has_displayed_marker"
                )
                is False
                and record["fresh_fetch_after_expiry"].get("blob_http_status") == 200
                and record["fresh_fetch_after_expiry"].get(
                    "digest_matches_descriptor"
                )
            )
            else "NOT_MEASURED"
        )
        record["residuals"].append(
            "the viewer keeps the zoom/one-to-one hint and control labels visible while"
            " the image is absent; the explicit Expired sentence, the missing"
            " 'メディア画像（…）' admission marker and the empty image area are the"
            " measured non-fabrication evidence, not the absence of those controls."
        )
        if record.get("same_process_probe", {}).get("defect"):
            record["residuals"].append(
                "REAL DEFECT (not fixed in this evidence-only commit): after a Runtime"
                " restart the same process's media binding presents the startup"
                " credential, so the media descriptor is refused 401 and the pane shows"
                " Forbidden instead of Expired; this was reproduced and the cold-relaunch"
                " path was used to measure the Expired UI."
            )
        return record

    finally:
        with contextlib.suppress(FileNotFoundError):
            delay_flag.unlink()
        if ctx.app is not None:
            with contextlib.suppress(Exception):
                record["app_log_tail"] = acc1.bounded(
                    acc1.redact_text(ctx.app.log_text()), 6000
                )
        ctx.cleanup()
        cleanup = ctx.record.get("cleanup", {})
        record["cleanup"] = cleanup
        record["finished_at"] = now_iso()
        record["_secrets"] = secrets_list
        record["runtime_log_path"] = str(ctx.runtime.log_path)
        record["desktop_log_path"] = str(ctx.app.log_path) if ctx.app is not None else None
        record["work"] = str(work)


# --------------------------------------------------------------------------- protocol Runtime


class ProtocolRuntime:
    """One isolated Runtime on a caller-chosen port with optional TTL/secret."""

    def __init__(
        self,
        *,
        label: str,
        port: int,
        work: Path,
        db_copy: Path,
        cas_copy: Path,
        presets: Path,
        auth_secret: str,
        handle_secret: str | None,
        ttl_seconds: int | None,
    ):
        self.label = label
        self.port = port
        self.endpoint = f"http://127.0.0.1:{port}"
        self.work = work
        self.work.mkdir(parents=True, exist_ok=True)
        self.db_copy = db_copy
        self.cas_copy = cas_copy
        self.presets = presets
        self.auth_secret = auth_secret
        self.handle_secret = handle_secret
        self.ttl_seconds = ttl_seconds
        self.config_path = work / "runtime-config.yaml"
        self.log_path = work / "runtime.log"
        self.stop_file = work / "runtime.stop"
        self.ready_file = work / "runtime.ready"
        self.discovery_path = work / "runtime-discovery.json"
        self.log_handle = None
        self.process: subprocess.Popen | None = None
        self.facts: dict = {}

    def _write_config(self) -> None:
        template = residual.CONFIG_TEMPLATE.read_text(encoding="utf-8")
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
                raise RuntimeError(f"config template has no unique {key} line")
        self.config_path.write_text(rendered, encoding="utf-8")
        (self.work / "outputs").mkdir(parents=True, exist_ok=True)
        (self.work / "logs").mkdir(parents=True, exist_ok=True)

    def _environment(self) -> dict:
        environment = dict(os.environ)
        for key in (
            "ORCHESTRATOR_AUTH_SECRET",
            "ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET",
            "ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS",
            "ORCHESTRATOR_AUTH_DISABLED",
            "ORCHESTRATOR_COS_CAS_ROOT",
            "ORCHESTRATOR_COS_WORK_ROOT",
            "ORCHESTRATOR_COS_PROJECT_ROOT",
            "ORCHESTRATOR_COS_PRESETS",
            "ORCHESTRATOR_RUNTIME_ROOT",
            "COS_RUNTIME_ORIGIN",
            "COS_RUNTIME_DISCOVERY_PATH",
            "COS_RUNTIME_DISCOVERY_DISABLED",
            "COS_RUNTIME_PROCESS_ROLE",
        ):
            environment.pop(key, None)
        environment.update(
            {
                "ORCHESTRATOR_AUTH_SECRET": self.auth_secret,
                "ORCHESTRATOR_COS_CAS_ROOT": str(self.cas_copy),
                "ORCHESTRATOR_COS_WORK_ROOT": str(self.work / "cos-work"),
                "ORCHESTRATOR_COS_PROJECT_ROOT": str(RUNTIME_ROOT),
                "ORCHESTRATOR_COS_PRESETS": str(self.presets),
                "ORCHESTRATOR_RUNTIME_ROOT": str(RUNTIME_ROOT),
                "COS_RUNTIME_ORIGIN": self.endpoint,
                "COS_RUNTIME_DISCOVERY_PATH": str(self.discovery_path),
            }
        )
        if self.handle_secret:
            environment["ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET"] = self.handle_secret
        if self.ttl_seconds is not None:
            environment["ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS"] = str(
                self.ttl_seconds
            )
        return environment

    def start(self) -> dict:
        residual.ensure_port_free(self.port)
        self._write_config()
        for stale in (self.stop_file, self.ready_file):
            with contextlib.suppress(FileNotFoundError):
                stale.unlink()
        self.log_handle = self.log_path.open("ab")
        self.log_handle.write(f"\n--- start {now_iso()} ---\n".encode())
        self.log_handle.flush()
        self.process = subprocess.Popen(  # noqa: S603 - fixed argv, loopback only
            [
                str(RUNTIME_PYTHON),
                str(residual.LAUNCHER),
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
            env=self._environment(),
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        started = time.monotonic()
        while True:
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"Runtime {self.label} exited during startup "
                    f"(exit {self.process.returncode})"
                )
            try:
                ready = residual.http_json("GET", f"{self.endpoint}/api/ready")
                ready_status = ready[0]
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                ready_status = 0
            if ready_status == 200:
                break
            if time.monotonic() - started > 120.0:
                raise RuntimeError(f"Runtime {self.label} readiness timed out")
            time.sleep(0.2)
        status, hello = residual.http_json(
            "GET", f"{self.endpoint}/api/cos/desktop/server"
        )
        if status != 200 or not isinstance(hello, dict):
            raise RuntimeError(f"Runtime {self.label} HELLO failed ({status})")
        server = hello["server"]
        discovery: dict = {"present": False}
        deadline = time.monotonic() + 20.0
        while time.monotonic() < deadline:
            if self.discovery_path.is_file():
                raw = self.discovery_path.read_bytes()
                document = json.loads(raw.decode("utf-8"))
                discovery = {
                    "present": True,
                    "sha256": sha256_bytes(raw),
                    "bytes": len(raw),
                    "origin": document.get("origin"),
                    "epoch": document.get("epoch"),
                    "pid": document.get("pid"),
                    "ready": document.get("ready"),
                }
                break
            time.sleep(0.1)
        self.facts = {
            "label": self.label,
            "port": self.port,
            "pid": self.process.pid,
            "endpoint": self.endpoint,
            "server_identity": server["server_identity"],
            "epoch": server["epoch"],
            "protocol": server["protocol"],
            "schema": server["schema"],
            "ttl_env": self.ttl_seconds,
            "handle_secret": "persistent" if self.handle_secret else "ephemeral",
            "discovery": discovery,
            "config_sha256": sha256_file(self.config_path),
            "started_at": now_iso(),
        }
        return self.facts

    def issue_credential(self) -> dict:
        status, document = residual.http_json(
            "POST",
            f"{self.endpoint}/api/cos/desktop/credentials",
            headers={"X-Auth-Secret": self.auth_secret},
        )
        return {"status": status, "document": document}

    def open_observation(self, token: str) -> dict:
        body = {
            "schema_version": 1,
            "request_id": uuid.uuid4().hex,
            "protocol": {"major": 2, "minor": 0},
            "schema": {"major": 2, "minor": 0},
            "scope": {"task_ids": [], "filters": [{"name": "view", "value": "all"}]},
            "retained_commands": [],
        }
        status, document = residual.http_json(
            "POST",
            f"{self.endpoint}/api/cos/desktop/observation",
            body=body,
            headers={"Authorization": f"Bearer {token}"},
        )
        return {"status": status, "document": document}

    def descriptor(self, token: str, context: str, digest: str) -> tuple[int, dict | None]:
        status, raw, _headers = residual.http(
            "GET",
            f"{self.endpoint}/api/cos/media/{digest}",
            headers={"Authorization": f"Bearer {token}", "X-Cos-Context": context},
        )
        try:
            document = json.loads(raw.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            document = None
        return status, document

    def fetch_blob(self, token: str, descriptor: dict, *, label: str) -> dict:
        handle = str(descriptor.get("access_handle") or "")
        path = residual.handle_path(handle)
        started = time.monotonic()
        status, raw, headers = residual.http(
            "GET",
            f"{self.endpoint}{path}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Media-Handle": path,
                "Accept": str(descriptor.get("media_type") or "*/*"),
            },
            timeout=120.0,
        )
        result = {
            "label": label,
            "status": status,
            "elapsed_ms": round((time.monotonic() - started) * 1000.0, 3),
            "bytes": len(raw),
            "at": now_iso(),
        }
        if status == 200:
            result["sha256"] = sha256_bytes(raw)
            result["declared_byte_length"] = descriptor.get("byte_length")
            result["length_matches_descriptor"] = (
                int(descriptor.get("byte_length") or -1) == len(raw)
            )
            result["digest_matches_descriptor"] = (
                result["sha256"] == descriptor.get("artifact_digest")
            )
        else:
            try:
                body = json.loads(raw.decode("utf-8"))
            except (ValueError, UnicodeDecodeError):
                body = None
            result["body"] = body
            result["body_code"] = body.get("code") if isinstance(body, dict) else None
            result["content_length_header"] = residual.header_value(
                headers, "content-length"
            )
        return result

    def present_handle(self, token: str, handle: str, *, label: str) -> dict:
        path = residual.handle_path(handle)
        started = time.monotonic()
        status, raw, headers = residual.http(
            "GET",
            f"{self.endpoint}{path}",
            headers={
                "Authorization": f"Bearer {token}",
                "X-Media-Handle": path,
            },
            timeout=30.0,
        )
        body = None
        with contextlib.suppress(ValueError, UnicodeDecodeError):
            body = json.loads(raw.decode("utf-8"))
        return {
            "label": label,
            "status": status,
            "elapsed_ms": round((time.monotonic() - started) * 1000.0, 3),
            "body": body,
            "body_code": body.get("code") if isinstance(body, dict) else None,
            "bytes": len(raw),
            "content_length_header": residual.header_value(headers, "content-length"),
            "at": now_iso(),
        }

    def stop(self) -> dict:
        if self.process is None:
            return {"stopped": False, "reason": "not_started"}
        pid = self.process.pid
        self.stop_file.write_text("stop", encoding="utf-8")
        try:
            self.process.wait(timeout=60)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=20)
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        deadline = time.monotonic() + 15.0
        while self.discovery_path.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline and port_open(self.port):
            time.sleep(0.1)
        return {
            "pid": pid,
            "exit": self.process.returncode,
            "clean_exit": self.process.returncode == 0,
            "port_closed": not port_open(self.port),
            "discovery_removed": not self.discovery_path.exists(),
        }


def scenario_protocol() -> dict:
    started_at = now_iso()
    record: dict = {
        "label": "protocol-ttl-401",
        "started_at": started_at,
        "verdict": "NOT_RUN",
        "default_ttl": {},
        "ephemeral_401": {},
        "fresh_after_401": {},
        "cleanup": {},
        "residuals": [],
    }
    work = WORK_ROOT / f"{time.strftime('%Y%m%dT%H%M%S')}-protocol"
    work.mkdir(parents=True, exist_ok=True)
    db_copy = work / "generation.sqlite"
    cas_copy = work / "cas"
    runtime1 = runtime2 = None
    secrets_list: list[str] = []
    try:
        residual.copy_database(residual.SOURCE_DB, db_copy)
        shutil.copytree(residual.SOURCE_CAS, cas_copy)
        presets = work / "cos_presets.json"
        shutil.copyfile(SOURCE_PRESETS, presets)
        run_secret = secrets.token_hex(24)
        secrets_list.append(run_secret)
        runtime1 = ProtocolRuntime(
            label="protocol-1",
            port=PORT_PROTOCOL,
            work=work / "runtime-1",
            db_copy=db_copy,
            cas_copy=cas_copy,
            presets=presets,
            auth_secret=run_secret,
            handle_secret=None,
            ttl_seconds=None,
        )
        facts1 = runtime1.start()
        credential1 = runtime1.issue_credential()
        token1 = str((credential1.get("document") or {}).get("token") or "")
        secrets_list.append(token1)
        observation1 = runtime1.open_observation(token1)
        grant1 = ((observation1.get("document") or {}).get("grant") or {}).get(
            "stamp"
        ) or {}
        context1 = str(grant1.get("context_id") or "")
        if credential1.get("status") != 200 or observation1.get("status") != 200:
            raise RuntimeError("protocol credential/observation failed")
        candidates = residual.artifact_candidates(db_copy)
        chosen = residual.choose_artifact(runtime1, token1, context1, candidates)
        descriptor1 = chosen["descriptor"]
        digest = str(chosen["digest"])
        issued_at = time.time()
        now_before = now_ms()
        expires_at = int(descriptor1["expires_at_unix_ms"])
        payload = (residual.decode_handle(descriptor1.get("access_handle") or "").get("payload") or {})
        blob1 = runtime1.fetch_blob(token1, descriptor1, label="default-ttl-original")
        record["default_ttl"] = {
            "runtime": facts1,
            "candidate_probe_count": chosen.get("probed"),
            "artifact": {
                "artifact_ref": chosen.get("artifact_ref"),
                "digest": digest,
                "length": chosen.get("length"),
                "media_type": chosen.get("media_type"),
            },
            "descriptor": residual.descriptor_summary(descriptor1),
            "issued_at": issued_at,
            "now_ms_before": now_before,
            "descriptor_expires_at_unix_ms": expires_at,
            "advertised_remaining_ms_at_receipt": expires_at - now_before,
            "advertised_remaining_seconds_at_receipt": round(
                (expires_at - now_before) / 1000.0, 3
            ),
            "expected_default_ttl_seconds": TTL_DEFAULT_SECONDS,
            "within_expected_window": (
                TTL_DEFAULT_SECONDS * 1000 - 2500
                <= expires_at - now_before
                <= TTL_DEFAULT_SECONDS * 1000 + 1000
            ),
            "handle_exp": payload.get("exp"),
            "handle_remaining_seconds": round(
                float(payload.get("exp") or 0) - time.time(), 3
            ),
            "handle_epoch": payload.get("epc"),
            "handle_representation": payload.get("rep"),
            "fetch": blob1,
            "expiry_at_300s_observed": False,
            "expiry_at_300s_note": (
                "not waited: the advertised 300 s expiry transition is NOT_MEASURED "
                "in this session"
            ),
        }

        stop1 = runtime1.stop()
        wait_port_free(PORT_PROTOCOL, 30.0)
        record["default_ttl"]["runtime_stop"] = stop1
        record["default_ttl"]["runtime_1_epoch"] = facts1["epoch"]

        runtime2 = ProtocolRuntime(
            label="protocol-2",
            port=PORT_PROTOCOL,
            work=work / "runtime-2",
            db_copy=db_copy,
            cas_copy=cas_copy,
            presets=presets,
            auth_secret=run_secret,
            handle_secret=None,
            ttl_seconds=None,
        )
        facts2 = runtime2.start()
        credential2 = runtime2.issue_credential()
        token2 = str((credential2.get("document") or {}).get("token") or "")
        secrets_list.append(token2)
        observation2 = runtime2.open_observation(token2)
        grant2 = ((observation2.get("document") or {}).get("grant") or {}).get(
            "stamp"
        ) or {}
        context2 = str(grant2.get("context_id") or "")
        if credential2.get("status") != 200 or observation2.get("status") != 200:
            raise RuntimeError("runtime-2 credential/observation failed")

        old_handle = str(descriptor1.get("access_handle") or "")
        old_payload = (
            residual.decode_handle(old_handle).get("payload") or {}
        )
        presented = runtime2.present_handle(
            token2, old_handle, label="old-handle-after-ephemeral-restart"
        )
        fresh_descriptor = runtime2.descriptor(token2, context2, digest)
        fresh_document = fresh_descriptor[1] or {}
        fresh_blob = (
            runtime2.fetch_blob(token2, fresh_document, label="fresh-after-401")
            if fresh_descriptor[0] == 200
            else {}
        )
        fresh_payload = (
            residual.decode_handle(fresh_document.get("access_handle") or "").get(
                "payload"
            )
            or {}
        )
        record["ephemeral_401"] = {
            "runtime_1": {
                "epoch": facts1["epoch"],
                "handle_secret": "ephemeral (no ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET)",
                "ttl_env": None,
            },
            "runtime_2": {
                "epoch": facts2["epoch"],
                "epoch_changed": facts2["epoch"] != facts1["epoch"],
                "handle_secret": "ephemeral (new per-process secret)",
                "ttl_env": None,
            },
            "old_handle_payload": {
                "v": old_payload.get("v"),
                "epc": old_payload.get("epc"),
                "exp": old_payload.get("exp"),
                "remaining_seconds_at_present": round(
                    float(old_payload.get("exp") or 0) - time.time(), 3
                ),
                "rep": old_payload.get("rep"),
                "ver": old_payload.get("ver"),
            },
            "presented_with_fresh_credential": True,
            "presented": presented,
            "expected_code": "MEDIA_UNAUTHORIZED",
            "expected_status": 401,
            "status_matches": presented.get("status") == 401,
            "body_code_matches": presented.get("body_code") == "MEDIA_UNAUTHORIZED",
            "not_410": presented.get("status") != 410,
            "mac_mismatch_before_epoch_check": True,
            "desktop_mapping": {
                "client": "cos_client::media::MediaFetchError::Unauthorized",
                "app_core": "app_core::MediaAvailability::Forbidden",
                "native_sentence": MEDIA_FORBIDDEN_TEXT,
                "source": [
                    "crates/cos-client/src/media.rs:423 (401 -> Unauthorized)",
                    "crates/app-core/src/media_presentation.rs:209 (Unauthorized -> Forbidden)",
                    "crates/desktop-gpui/src/media_presentation.rs:186 (Forbidden sentence)",
                ],
                "mapping_test": (
                    "app-core every_fetch_failure_class_maps_to_one_availability_state_"
                    "and_expiry_is_not_missing"
                ),
            },
        }
        record["fresh_after_401"] = {
            "descriptor_http_status": fresh_descriptor[0],
            "descriptor": residual.descriptor_summary(fresh_document),
            "handle_differs_from_old": (
                sha256_bytes(str(fresh_document.get("access_handle") or "").encode())
                != sha256_bytes(old_handle.encode())
            ),
            "handle_epoch": fresh_payload.get("epc"),
            "handle_matches_runtime_2_epoch": fresh_payload.get("epc") == facts2["epoch"],
            "handle_remaining_seconds": round(
                float(fresh_payload.get("exp") or 0) - time.time(), 3
            ),
            "fetch": fresh_blob,
        }
        record["verdict"] = (
            "MEASURED"
            if (
                record["default_ttl"]["within_expected_window"]
                and record["default_ttl"]["fetch"].get("status") == 200
                and record["default_ttl"]["fetch"].get("digest_matches_descriptor")
                and record["ephemeral_401"]["status_matches"]
                and record["ephemeral_401"]["body_code_matches"]
                and record["ephemeral_401"]["not_410"]
                and record["fresh_after_401"]["descriptor_http_status"] == 200
                and record["fresh_after_401"]["fetch"].get("status") == 200
                and record["fresh_after_401"]["fetch"].get(
                    "digest_matches_descriptor"
                )
            )
            else "PARTIAL"
        )
        record["cleanup"]["runtime_2"] = runtime2.stop()
        runtime2 = None
        record["_secrets"] = secrets_list
        record["work"] = str(work)
        return record
    finally:
        if runtime2 is not None:
            with contextlib.suppress(Exception):
                runtime2.stop()
        if runtime1 is not None:
            with contextlib.suppress(Exception):
                runtime1.stop()
        wait_port_free(PORT_PROTOCOL, 15.0)
        record["finished_at"] = now_iso()
        record["work"] = str(work)
        record["_secrets"] = secrets_list


def mapping_test() -> dict:
    command = [
        "cargo",
        "test",
        "--locked",
        "-p",
        "app-core",
        "--lib",
        "every_fetch_failure_class_maps_to_one_availability_state_and_expiry_is_not_missing",
        "--",
        "--nocapture",
    ]
    started = time.monotonic()
    completed = subprocess.run(
        command,
        cwd=str(ROOT),
        capture_output=True,
        text=True,
        timeout=900,
        encoding="utf-8",
        errors="replace",
    )
    return {
        "command": " ".join(command),
        "exit": completed.returncode,
        "elapsed_s": round(time.monotonic() - started, 2),
        "stdout_tail": completed.stdout[-4000:],
        "stderr_tail": completed.stderr[-2000:],
    }


# --------------------------------------------------------------------------- evidence


def sanitize_and_copy(source: Path, destination: Path, secrets_list: list[str]) -> dict:
    text = source.read_text(encoding="utf-8", errors="replace")
    sanitized, redactions = residual.sanitized_log(text, secrets_list)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(sanitized, encoding="utf-8")
    return {
        "source": str(source),
        "file": destination.name,
        "sha256": sha256_file(destination),
        "bytes": destination.stat().st_size,
        "redactions": redactions,
    }


def leak_scan(evidence: Path, secrets_list: list[str]) -> dict:
    findings: list[str] = []
    checked = 0
    for path in sorted(evidence.rglob("*")):
        if not path.is_file() or path.suffix not in {".json", ".md", ".txt", ".sha256"}:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        checked += 1
        for secret in secrets_list:
            if secret and secret in text:
                findings.append(f"{path.name}: exact secret material")
        if residual.HANDLE_PATTERN.search(text):
            findings.append(f"{path.name}: access-handle value")
        if re.search(r"Bearer\s+[A-Za-z0-9._\-]{16,}", text):
            findings.append(f"{path.name}: bearer token")
    return {
        "files_checked": checked,
        "findings": findings,
        "clean": not findings,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--part",
        choices=("all", "native", "protocol"),
        default="all",
        help="run one scenario or both",
    )
    parser.add_argument("--keep-work", action="store_true")
    args = parser.parse_args(argv)

    if EVIDENCE.exists():
        shutil.rmtree(EVIDENCE)
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    (EVIDENCE / "uia").mkdir(exist_ok=True)
    (EVIDENCE / "shots").mkdir(exist_ok=True)
    (EVIDENCE / "logs").mkdir(exist_ok=True)
    WORK_ROOT.mkdir(parents=True, exist_ok=True)

    session: dict = {
        "schema_version": 1,
        "task": "RT3 media-expiry residuals (native Expired UI, 300 s default TTL, ephemeral 401)",
        "started_at": now_iso(),
        "desktop_root": str(ROOT),
        "runtime_root": str(RUNTIME_ROOT),
        "desktop_head": run_git("rev-parse", "HEAD"),
        "desktop_status_porcelain": run_git("status", "--porcelain"),
        "runtime_head": subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=str(RUNTIME_ROOT),
            capture_output=True,
            text=True,
            timeout=30,
        ).stdout.strip(),
        "runtime_status_porcelain": subprocess.run(
            ["git", "status", "--porcelain"],
            cwd=str(RUNTIME_ROOT),
            capture_output=True,
            text=True,
            timeout=30,
        ).stdout.strip(),
        "desktop_binary": {
            "path": str(DESKTOP_EXE),
            "sha256": sha256_file(DESKTOP_EXE),
            "bytes": DESKTOP_EXE.stat().st_size,
        },
        "source_db": {
            "path": str(residual.SOURCE_DB),
            "sha256": sha256_file(residual.SOURCE_DB),
            "bytes": residual.SOURCE_DB.stat().st_size,
        },
        "native_source_run": {
            "path": str(SOURCE_RUN),
            "db_sha256": sha256_file(SOURCE_RUN / "generation.sqlite"),
            "db_bytes": (SOURCE_RUN / "generation.sqlite").stat().st_size,
            "task_ref": TASK_REF,
            "note": (
                "fresh copy of the prior isolated closure run's DB/CAS; the task's"
                " capability snapshot set is test-only material, no generation was"
                " executed by this session"
            ),
        },
        "ports": {"native": PORT_NATIVE, "protocol": PORT_PROTOCOL},
        "measurements": {},
        "cleanup": {},
        "not_measured": [
            "the actual 300 s expiry transition for the default-TTL descriptor",
            "a native UI transition to Forbidden on a live 401 (the client does not"
            " retain a handle across a Runtime restart; the 401 mapping is measured"
            " by the app-core mapping test, not by the native window)",
            "PREVIEW/THUMBNAIL/METADATA byte routes and non-PNG media types",
            "clock skew and simultaneous multi-handle expiry",
        ],
        "residuals": [],
    }

    native_record: dict | None = None
    protocol_record: dict | None = None
    try:
        if args.part in {"all", "native"}:
            native_record = scenario_native()
            session["measurements"]["native_expiry"] = {
                "verdict": native_record.get("verdict"),
                "initial_state": (
                    native_record.get("initial_display", {})
                    .get("uia", {})
                    .get("state")
                ),
                "expired_found": native_record.get("native_expired", {}).get("found"),
                "expired_text_present": native_record.get("native_expired", {}).get(
                    "exact_text_present"
                ),
                "stale_image_absent": native_record.get("native_expired", {}).get(
                    "stale_image_absent"
                ),
                "viewport_absent": native_record.get("native_expired", {}).get(
                    "viewport_absent"
                ),
                "runtime_2_epoch": native_record.get("expiry_sequence", {})
                .get("runtime_2", {})
                .get("epoch"),
                "descriptor_delay_seconds": DESCRIPTOR_DELAY_SECONDS,
                "fresh_fetch_status": native_record.get(
                    "fresh_fetch_after_expiry", {}
                ).get("blob_http_status"),
                "cleanup": native_record.get("cleanup"),
            }
            sanitized = sanitize_and_copy(
                Path(native_record["runtime_log_path"]),
                EVIDENCE / "logs" / "runtime-native.log.sanitized.txt",
                native_record.get("_secrets", []),
            )
            native_record["runtime_log_sanitized"] = sanitized
            if native_record.get("desktop_log_path"):
                desktop = sanitize_and_copy(
                    Path(native_record["desktop_log_path"]),
                    EVIDENCE / "logs" / "desktop-native.log.sanitized.txt",
                    native_record.get("_secrets", []),
                )
                native_record["desktop_log_sanitized"] = desktop
            output = {
                key: value
                for key, value in native_record.items()
                if key not in {"_secrets"}
            }
            write_json(EVIDENCE / "record-native-expiry.json", redact_document(output))
            session["measurements"]["native_expiry"]["cleanup"] = native_record.get(
                "cleanup"
            )

        if args.part in {"all", "protocol"}:
            protocol_record = scenario_protocol()
            session["measurements"]["default_ttl"] = {
                "verdict": protocol_record.get("verdict"),
                "advertised_remaining_seconds": protocol_record.get(
                    "default_ttl", {}
                ).get("advertised_remaining_seconds_at_receipt"),
                "within_expected_window": protocol_record.get("default_ttl", {}).get(
                    "within_expected_window"
                ),
                "blob_status": protocol_record.get("default_ttl", {})
                .get("fetch", {})
                .get("status"),
                "digest_matches": protocol_record.get("default_ttl", {})
                .get("fetch", {})
                .get("digest_matches_descriptor"),
                "expiry_at_300s_observed": False,
            }
            session["measurements"]["ephemeral_401"] = {
                "status": protocol_record.get("ephemeral_401", {})
                .get("presented", {})
                .get("status"),
                "code": protocol_record.get("ephemeral_401", {})
                .get("presented", {})
                .get("body_code"),
                "epoch_changed": protocol_record.get("ephemeral_401", {})
                .get("runtime_2", {})
                .get("epoch_changed"),
                "fresh_descriptor_status": protocol_record.get(
                    "fresh_after_401", {}
                ).get("descriptor_http_status"),
                "fresh_blob_status": protocol_record.get("fresh_after_401", {})
                .get("fetch", {})
                .get("status"),
                "cleanup": protocol_record.get("cleanup"),
            }
            for index, path_key in enumerate(("runtime-1/runtime.log", "runtime-2/runtime.log")):
                source = Path(protocol_record["work"]) / path_key
                if source.is_file():
                    sanitized = sanitize_and_copy(
                        source,
                        EVIDENCE
                        / "logs"
                        / f"runtime-protocol-{index + 1}.log.sanitized.txt",
                        protocol_record.get("_secrets", []),
                    )
                    protocol_record.setdefault("runtime_log_sanitized", []).append(
                        sanitized
                    )
            output = {
                key: value
                for key, value in protocol_record.items()
                if key not in {"_secrets"}
            }
            write_json(EVIDENCE / "record-protocol.json", redact_document(output))
            session["measurements"]["ephemeral_401"]["cleanup"] = protocol_record.get(
                "cleanup"
            )

        session["mapping_test"] = mapping_test()
        session["finished_at"] = now_iso()
        session["verdicts"] = {
            "native_expired_ui": (native_record or {}).get("verdict", "NOT_RUN"),
            "default_ttl_300_advertised": (
                "MEASURED"
                if (protocol_record or {}).get("default_ttl", {}).get(
                    "within_expected_window"
                )
                else "NOT_MEASURED"
            ),
            "default_ttl_expiry_at_300s": "NOT_MEASURED",
            "ephemeral_401": (
                "MEASURED"
                if (protocol_record or {}).get("ephemeral_401", {}).get(
                    "status_matches"
                )
                else "NOT_MEASURED"
            ),
            "desktop_401_mapping": (
                "MEASURED (app-core test)"
                if session["mapping_test"].get("exit") == 0
                else "NOT_MEASURED"
            ),
        }
        session["secrets_retained"] = False
        session["evidence"] = {
            "uia_files": sorted(path.name for path in (EVIDENCE / "uia").glob("*.json")),
            "screenshots": sorted(
                path.name for path in (EVIDENCE / "shots").glob("*.png")
            ),
            "logs": sorted(
                path.name for path in (EVIDENCE / "logs").glob("*.txt")
            ),
        }
        write_json(EVIDENCE / "session.json", redact_document(session))
        scan = leak_scan(EVIDENCE, (native_record or {}).get("_secrets", []) + (protocol_record or {}).get("_secrets", []))
        write_json(EVIDENCE / "leak-scan.json", scan)
        # Final hashes cover every evidence file except the hash list itself.
        lines = []
        for path in sorted(EVIDENCE.rglob("*")):
            if path.is_file() and path.name != "session.sha256":
                lines.append(f"{sha256_file(path).removeprefix('sha256:')}  {path.relative_to(EVIDENCE).as_posix()}")
        (EVIDENCE / "session.sha256").write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(json.dumps(session["verdicts"], indent=2, ensure_ascii=False))
        print(f"evidence: {EVIDENCE}", flush=True)
        return 0
    finally:
        if not args.keep_work:
            # Keep the evidence; remove the large DB/CAS work copies.
            for path in (WORK_ROOT,):
                with contextlib.suppress(OSError):
                    shutil.rmtree(path, ignore_errors=True)
        print(f"[{now_iso()}] done", flush=True)


if __name__ == "__main__":
    raise SystemExit(main())
