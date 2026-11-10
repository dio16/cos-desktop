#!/usr/bin/env python3
"""RT3 media-expiry refetch native measurement and default-TTL expiry measurement.

Measures the remaining media-expiry UX gap recorded in
``docs/development/evidence-published/media-expiry-native-latest``:

1. an isolated Runtime (fresh copy of a prior isolated closure run's DB/CAS; no
   generation) serves one already-admitted artifact on port 8135 with the 60 s
   test TTL and per-run secrets;
2. the native Desktop displays that artifact (``DISPLAYED``);
3. the handle TTL passes, the Runtime is cleanly restarted on the same DB copy
   (new epoch / new principal) and the shell re-resolves in the same process;
   a harness-only **descriptor delivery delay** (63 s, past the real minted
   expiry while the wire stays 200) drives the explicit ``EXPIRED`` state;
4. the ``EXPIRED`` pane must now expose one actionable refetch control
   (``メディアを再取得 (R)``, UIA Button + Invoke). Activating it re-resolves the
   same canonical identity through the current session identity; a second
   failure keeps the explicit state, and a success replaces it with
   ``DISPLAYED`` and a digest-matching blob;
5. the wire ``410 MEDIA_HANDLE_EXPIRED`` path is produced with a harness-only
   **blob request delay** (63 s before the Runtime handles ``/blob``): the
   server itself answers 410 for the real expired handle, the client maps it to
   ``Expired``, and the refetch control recovers the picture.

A second, protocol-only scenario measures an actual **default 300 s TTL**
expiry: a descriptor is minted with no TTL override, its advertised remaining
time is recorded, the same handle is presented after 305 s and must answer 410,
and a fresh descriptor/blob must still serve the canonical bytes.

Runtime/Desktop logs are sanitized; tokens, principals and access handles are
never written to evidence.

Usage (from the repository root)::

    python apps/desktop/devtools/runtime_integration/run_rt3_media_refetch.py \\
        --desktop-exe target/release/cos-desktop.exe
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
sys.path.insert(0, str(HARNESS))
import run_acc1_native as acc1  # noqa: E402 - local test helper, stdlib only
import run_integration as base  # noqa: E402 - local test helper, stdlib only
import run_rt3_residual as residual  # noqa: E402 - local test helper, stdlib only

ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
EVIDENCE = ROOT / "docs" / "development" / "evidence-published" / "media-refetch-latest"
WORK_ROOT = ROOT / "target" / "media-refetch"
SOURCE_RUN_CANDIDATES = (
    ROOT / "target" / "acc1-native" / "20261109T233420-golden",
    Path("C:/dev/cos-desktop/target/acc1-native/20261109T233420-golden"),
)
SOURCE_RUN = next(
    (candidate for candidate in SOURCE_RUN_CANDIDATES if candidate.is_dir()),
    SOURCE_RUN_CANDIDATES[0],
)
TASK_REF = "task-ab47119a8bec467a8e128d7b82f1220a"
PORT_NATIVE = 8135
PORT_TTL = 8168
TTL_NATIVE_SECONDS = 60
DESCRIPTOR_DELAY_SECONDS = 63.0
BLOB_DELAY_SECONDS = 63.0
DEFAULT_TTL_SECONDS = 300
DEFAULT_TTL_EXPIRY_WAIT_SECONDS = 305.0
MEDIA_DELAY_LAUNCHER = HARNESS / "isolated_runtime_media_delay.py"
BASE_NATIVE_RUNTIME = acc1.NativeRuntime

MEDIA_DISPLAYED_MARKER = "メディア画像（"
MEDIA_EXPIRED_TEXT = (
    "画像を利用できません: メディア記述子の有効期限が切れました。再取得が必要です。"
)
MEDIA_FORBIDDEN_TEXT = (
    "画像を利用できません: このセッションにはメディアの閲覧権限がありません。"
)
MEDIA_MISSING_TEXT = "画像を利用できません: この成果物のメディアが見つかりません。"
REFETCH_CONTROL_NAME = "メディアを再取得 (R)"
MEDIA_ACCESS = re.compile(
    r'"(?:GET|HEAD) (?P<path>/api/cos/media/[^ ]*) HTTP/1\.1" (?P<status>\d{3})'
)
BLOB_DELAY_LOG = re.compile(r"harness media blob (?:request delay|response)")


# --------------------------------------------------------------------------- basics


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_bytes(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(acc1.redact_document(document), ensure_ascii=False, indent=1) + "\n",
        encoding="utf-8",
    )


def run_git(*args: str, cwd: Path = ROOT) -> str:
    completed = subprocess.run(
        ["git", *args], cwd=str(cwd), capture_output=True, text=True, timeout=60
    )
    return completed.stdout.strip()


def log_since(path: Path, offset: int) -> str:
    with contextlib.suppress(OSError):
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            handle.seek(offset)
            return handle.read()
    return ""


# --------------------------------------------------------------------------- UIA helpers


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


def refetch_control_present(names: list[str]) -> bool:
    return REFETCH_CONTROL_NAME in names


def viewport_present(names: list[str]) -> bool:
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
        json.dumps(acc1.redact_document(raw), ensure_ascii=False, indent=1) + "\n",
        encoding="utf-8",
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
        "expired_text_present": MEDIA_EXPIRED_TEXT in "\n".join(names),
        "refetch_control_present": refetch_control_present(names),
        "refetch_control_name": REFETCH_CONTROL_NAME,
        "screenshot": {
            "file": str(image.relative_to(EVIDENCE)),
            "sha256": shot.get("_image_sha256"),
            "bytes": shot.get("_image_bytes"),
            "printed": shot.get("ok"),
            "width": shot.get("width"),
            "height": shot.get("height"),
        },
    }


def wait_state(ui: acc1.Ui, target: str, timeout: float) -> tuple[dict, list[str], bool]:
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
        time.sleep(1.0)
    return last_dump, last_names, False


def wait_control(
    ui: acc1.Ui, present: bool, timeout: float
) -> tuple[dict, list[str], bool]:
    deadline = time.monotonic() + timeout
    last_dump: dict = {}
    last_names: list[str] = []
    while time.monotonic() < deadline:
        last_dump = ui.probe("dump")
        last_names = list(last_dump.get("names") or [])
        if refetch_control_present(last_names) == present:
            return last_dump, last_names, True
        if not base.pid_alive(ui.pid):
            break
        time.sleep(0.5)
    return last_dump, last_names, False


def activate_refetch(ui: acc1.Ui) -> dict:
    """Activate the refetch control through UIA Invoke, else a synthetic click."""
    invoked = ui.invoke(REFETCH_CONTROL_NAME)
    if invoked.get("invoked"):
        return {"method": "uia_invoke", "result": invoked}
    clicked = ui.click(REFETCH_CONTROL_NAME)
    return {"method": "wm_lbutton_at_uia_bounds", "result": clicked}


def media_access(text: str) -> dict:
    entries = [
        {"path": match.group("path"), "status": int(match.group("status"))}
        for match in MEDIA_ACCESS.finditer(text)
    ]
    return {
        "entries": entries[-24:],
        "descriptor_statuses": [
            entry["status"]
            for entry in entries
            if not entry["path"].endswith("/blob")
        ],
        "blob_statuses": [
            entry["status"] for entry in entries if entry["path"].endswith("/blob")
        ],
        "blob_410": [
            entry for entry in entries
            if entry["path"].endswith("/blob") and entry["status"] == 410
        ],
        "descriptor_count": sum(
            1 for entry in entries if not entry["path"].endswith("/blob")
        ),
        "blob_count": sum(1 for entry in entries if entry["path"].endswith("/blob")),
    }


def blob_delay_lines(text: str, limit: int = 16) -> list[str]:
    return [line for line in text.splitlines() if BLOB_DELAY_LOG.search(line)][-limit:]


def ensure_task_selected(ctx: "acc1.Ctx", goal: str, task_ref: str) -> tuple[dict, list[dict]]:
    """Select the measured task, retrying the rail filter/activation a bounded number of times.

    A restored window layout can make the first search-box click or rail click
    miss; the harness must prove the selected task before it measures media, and
    must never fall through to a different task's artifact.
    """
    ui = ctx.app.ui if ctx.app is not None else None
    results: list[dict] = []
    if ui is None:
        return {}, results
    for attempt in range(3):
        names_text = "\n".join(ui.dump().get("names") or [])
        if goal not in names_text:
            ui.click("タスクを検索")
            time.sleep(0.3)
            ui.probe("key", text="ctrl+a")
            ui.probe("key", text="backspace")
            ui.type_text("タスクを検索", goal)
            deadline = time.monotonic() + 10.0
            while time.monotonic() < deadline:
                if goal in "\n".join(ui.dump().get("names") or []):
                    break
                time.sleep(1.0)
        select = ctx.select_task(goal, f"media-refetch-task-{attempt}", task_ref)
        if select.get("selected"):
            results.append(select)
            return select, results
        # Fallback: the rail row's accessible name carries the task ref; click
        # that row and prove the inspector switched to the measured task.
        clicked = ui.click(task_ref)
        dump, text, selected = ui.wait_names(
            lambda value: f"目的 — 利用可能: {task_ref}" in value,
            20.0,
            "task-selected-ref",
        )
        select = {
            **select,
            "fallback_click": clicked,
            "fallback_selected": selected,
            "uia_sha256": dump.get("_names_sha256"),
        }
        results.append(select)
        if selected:
            select["selected"] = True
            select["scope_task_matched"] = True
            return select, results
        results.append({"attempt": attempt, "uia": capture(ui, f"selection-attempt-{attempt}")})
        time.sleep(2.0)
    return (results[-1] if results else {}), results


def safe_descriptor(descriptor: dict) -> dict:
    """Descriptor evidence with the principal claim reduced to a short hash."""
    summary = residual.descriptor_summary(descriptor or {})
    audience = summary.pop("audience", None)
    if audience:
        summary["audience_sha256_16"] = (
            "sha256:" + hashlib.sha256(str(audience).encode("utf-8")).hexdigest()[:16]
        )
    return summary


def descriptor_timing(descriptor: dict) -> dict:
    expires_at = descriptor.get("expires_at_unix_ms")
    if not isinstance(expires_at, (int, float)):
        return {"advertised_remaining_seconds": None}
    remaining = float(expires_at) / 1000.0 - time.time()
    return {
        "descriptor_expires_at_unix_ms": int(expires_at),
        "advertised_remaining_seconds": round(remaining, 3),
    }


# --------------------------------------------------------------------------- Runtime boxes


class MediaDelayRuntime(acc1.NativeRuntime):
    """NativeRuntime with the 60 s test TTL and the two harness-only media delays."""

    descriptor_flag: Path | None = None
    blob_flag: Path | None = None

    def _environment(self) -> dict:
        environment = super()._environment()
        environment["ORCHESTRATOR_COS_MEDIA_HANDLE_TTL_SECONDS"] = str(
            TTL_NATIVE_SECONDS
        )
        environment["ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_SECONDS"] = (
            f"{DESCRIPTOR_DELAY_SECONDS:.3f}"
        )
        environment["ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_FLAG"] = str(
            self.descriptor_flag
        )
        environment["ORCHESTRATOR_HARNESS_MEDIA_BLOB_DELAY_SECONDS"] = (
            f"{BLOB_DELAY_SECONDS:.3f}"
        )
        environment["ORCHESTRATOR_HARNESS_MEDIA_BLOB_DELAY_FLAG"] = str(self.blob_flag)
        return environment


def arm(flag: Path | None) -> None:
    if flag is not None:
        flag.write_text("delay", encoding="utf-8")


def disarm(flag: Path | None) -> None:
    if flag is not None:
        with contextlib.suppress(FileNotFoundError):
            flag.unlink()


# --------------------------------------------------------------------------- native scenario


def scenario_native(*, desktop_exe: Path) -> dict:
    started_at = now_iso()
    work = WORK_ROOT / f"{time.strftime('%Y%m%dT%H%M%S')}-native"
    work.mkdir(parents=True, exist_ok=True)
    descriptor_flag = work / "descriptor-delay.flag"
    blob_flag = work / "blob-delay.flag"
    disarm(descriptor_flag)
    disarm(blob_flag)
    acc1.WORK_ROOT = work / "ctx"
    acc1.DESKTOP_EXE = desktop_exe
    acc1.NativeRuntime = MediaDelayRuntime
    acc1.CONTROL_LAUNCHER = MEDIA_DELAY_LAUNCHER

    record: dict = {
        "schema_version": 1,
        "kind": "native-measurement",
        "label": "native-refetch",
        "started_at": started_at,
        "verdict": "NOT_RUN",
        "precondition": {},
        "cold_start": {},
        "expiry": {},
        "expired_ui": {},
        "refetch_after_descriptor_delay": {},
        "wire_410": {},
        "refetch_success": {},
        "protocol_check": {},
        "cleanup": {},
        "residuals": [],
    }
    secrets_list: list[str] = []
    ctx = acc1.Ctx("media-refetch", PORT_NATIVE, control=True, source_run=SOURCE_RUN)
    ctx.runtime.descriptor_flag = descriptor_flag
    ctx.runtime.blob_flag = blob_flag
    ui: acc1.Ui | None = None
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
                "path": str(desktop_exe),
                "sha256": sha256_file(desktop_exe),
                "bytes": desktop_exe.stat().st_size,
                "built_at": time.strftime(
                    "%Y-%m-%dT%H:%M:%S", time.localtime(desktop_exe.stat().st_mtime)
                ),
            },
            "desktop_head": run_git("rev-parse", "HEAD"),
            "desktop_status_porcelain": run_git("status", "--porcelain"),
            "runtime_head": run_git("rev-parse", "HEAD", cwd=RUNTIME_ROOT),
            "runtime_epoch_1": start1["epoch"],
            "runtime_server_identity": start1["server_identity"],
            "ttl_seconds": TTL_NATIVE_SECONDS,
            "descriptor_delay_seconds": DESCRIPTOR_DELAY_SECONDS,
            "blob_delay_seconds": BLOB_DELAY_SECONDS,
            "descriptor_delay_mechanism": (
                "harness-only ASGI wrapper sleeps after the real Runtime minted the"
                " descriptor and before its bytes are written; the descriptor/handle"
                " document is not rewritten"
            ),
            "blob_delay_mechanism": (
                "harness-only ASGI wrapper sleeps before the Runtime handles /blob;"
                " the server's own expiry check then answers 410 for the real handle"
            ),
            "db_copy_sha256": ctx.record.get("db_copy_sha256"),
            "db_copy_bytes": ctx.record.get("db_copy_bytes"),
            "discovery_only": True,
            "port": PORT_NATIVE,
            "generation_executed": False,
        }
        task = ctx.projection_task(TASK_REF)
        goal = str(task.get("title") or "")
        record["precondition"]["task_projection"] = {
            key: task.get(key)
            for key in ("title", "task_status", "inner_phase", "capabilities_ref")
        }
        if not goal:
            record["residuals"].append("the reused DB carries no TASK projection")
            return record
        capability = ctx.wait_capability_entry(
            TASK_REF, "media", "ORIGINAL", lambda support: support == "SUPPORTED", 45.0
        )
        record["precondition"]["capability_media"] = capability

        connected = ctx.launch_desktop("media-refetch")
        ui = ctx.app.ui if ctx.app is not None else None
        record["cold_start"]["connected"] = connected
        select, selection_attempts = ensure_task_selected(ctx, goal, TASK_REF)
        record["cold_start"]["task_selected"] = {
            key: select.get(key)
            for key in ("method", "selected", "scope_task_matched", "filter")
        }
        record["cold_start"]["task_selection_attempts"] = len(selection_attempts)
        if not select.get("selected"):
            record["residuals"].append(
                "the task rail did not select the measured task before the cold display"
            )
            return record
        before_offset = ctx.runtime.log_path.stat().st_size
        view_before = ctx.view_artifact(TASK_REF, "media-refetch-before")
        cold_capture = capture(ui, "cold-displayed")
        resolved = ctx.resolve_artifact(TASK_REF)
        digest = str((resolved.get("artifact") or {}).get("artifact_digest") or "")
        record["cold_start"]["artifact_surface"] = {
            key: view_before.get(key)
            for key in ("state", "viewport_present", "inspector_task_match")
        }
        record["cold_start"]["resolved_artifact"] = {
            "artifact": resolved.get("artifact"),
            "descriptor_http_status": resolved.get("descriptor_http_status"),
            "descriptor": safe_descriptor(resolved.get("descriptor") or {}),
            "descriptor_timing": descriptor_timing(resolved.get("descriptor") or {}),
        }
        if not digest:
            record["residuals"].append("the artifact digest was not projected")
            return record
        if view_before.get("state") not in {"DISPLAYED", "AVAILABLE"}:
            record["residuals"].append(
                f"the native artifact view did not display before expiry "
                f"({view_before.get('state')})"
            )
            return record
        cold_log = log_since(ctx.runtime.log_path, before_offset)
        record["cold_start"]["media_access"] = media_access(cold_log)
        record["cold_start"]["uia"] = cold_capture
        record["cold_start"]["refetch_control_absent"] = not cold_capture[
            "refetch_control_present"
        ]
        if not record["cold_start"]["refetch_control_absent"]:
            record["residuals"].append(
                "the DISPLAYED pane exposed a refetch control before any expiry"
            )

        # Let the descriptor/handle minted for the display pass the 60 s TTL.
        wait_started = time.time()
        time.sleep(70.0)
        record["expiry"] = {
            "waited_after_display_seconds": round(time.time() - wait_started, 3),
            "at": now_iso(),
            "image_displayed_before_expiry": True,
            "state_before_restart": media_state(
                list(ui.probe("dump").get("names") or [])
            ),
        }

        # Leave the Artifact surface before the restart so no resolution starts
        # from the previous lease/credential while the reconnect is in flight.
        record["expiry"]["flow_before_restart"] = ui.invoke("処理の流れ  Ctrl+3")
        time.sleep(1.0)
        # Arm the descriptor delivery delay before the Runtime restart so a
        # post-restart resolution cannot race the arm. The Desktop is on the Flow
        # surface, so no media request is in flight while the flag is set.
        arm(descriptor_flag)
        record["expiry"]["descriptor_delay_armed_before_restart"] = (
            descriptor_flag.is_file()
        )
        stop1 = ctx.runtime.stop()
        restart_offset = ctx.runtime.log_path.stat().st_size
        record["expiry"]["runtime_1_stop"] = stop1
        record["expiry"]["port_free_after_stop"] = acc1.wait_port_free(
            PORT_NATIVE, 30.0
        )
        start2 = ctx.runtime.start()
        record["expiry"]["runtime_2"] = {
            "pid": start2.get("pid"),
            "epoch": start2.get("epoch"),
            "server_identity": start2.get("server_identity"),
            "epoch_changed": start2.get("epoch") != start1.get("epoch"),
            "at": now_iso(),
        }
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline and not ctx.runtime.discovery_path.is_file():
            time.sleep(0.2)
        record["expiry"]["discovery_reappeared"] = ctx.runtime.discovery_path.is_file()
        if not record["expiry"]["runtime_2"]["epoch_changed"]:
            record["residuals"].append("the restarted Runtime reported the same epoch")

        # Wait for the bounded same-process reauthorization (new credential and
        # observation) before touching the Artifact surface again.
        reconnect: dict = {"at": now_iso(), "seen": False, "log_lines": []}
        deadline = time.monotonic() + 150.0
        while time.monotonic() < deadline:
            new_log = log_since(ctx.runtime.log_path, restart_offset)
            reconnect["log_lines"] = [
                line
                for line in new_log.splitlines()
                if "desktop/credentials" in line or "desktop/observation" in line
            ][-16:]
            if (
                'POST /api/cos/desktop/credentials HTTP/1.1" 200' in new_log
                and 'POST /api/cos/desktop/observation HTTP/1.1" 200' in new_log
            ):
                reconnect["seen"] = True
                break
            if not ctx.app.alive():
                break
            time.sleep(2.0)
        reconnect["after"] = now_iso()
        record["expiry"]["same_process_reconnect"] = reconnect
        if not reconnect["seen"]:
            record["residuals"].append(
                "the Desktop reconnect credential/observation sequence was not observed"
            )

        # The reconnect reconciliation may move the local dock cursor; prove the
        # measured task is selected again before re-entering the Artifact surface.
        reselect, reselect_attempts = ensure_task_selected(ctx, goal, TASK_REF)
        record["expiry"]["task_reselect"] = {
            key: reselect.get(key)
            for key in ("method", "selected", "scope_task_matched")
        }
        record["expiry"]["task_reselect_attempts"] = len(reselect_attempts)
        if not reselect.get("selected"):
            record["residuals"].append(
                "the task rail did not re-select the measured task after the Runtime restart"
            )
            return record

        # The epoch/principal change re-keys the media resolution; both descriptor
        # attempts are delivered after the real minted expiry while the wire
        # stays 200, so the client-side descriptor check reaches Expired.
        expired_offset = ctx.runtime.log_path.stat().st_size
        reentry_at = time.time()
        reentry: dict = {
            "at": now_iso(),
            "attempts": [],
            "descriptor_delay_armed": descriptor_flag.is_file(),
        }
        names: list[str] = []
        expired = False
        # A surface transition is what re-runs the shell's media sync with the
        # current lease. The first transition can be a no-op when the shell
        # already returned to Artifact, so retry a bounded number of times until
        # a delayed descriptor request is observed (never an automatic retry of
        # the media itself: the toggle only re-runs the identity-keyed sync).
        for attempt in range(4):
            flow = ui.invoke("処理の流れ  Ctrl+3")
            time.sleep(0.7)
            art = ui.invoke("成果物  Ctrl+1")
            if not art.get("invoked"):
                art = ui.click("成果物  Ctrl+1")
            reentry["attempts"].append(
                {
                    "flow_invoked": flow.get("invoked"),
                    "artifact_invoked": art.get("invoked"),
                    "at": now_iso(),
                }
            )
            deadline = time.monotonic() + 12.0
            started = False
            while time.monotonic() < deadline:
                window = log_since(ctx.runtime.log_path, expired_offset)
                if "harness media descriptor delay begin" in window:
                    started = True
                    break
                if not ctx.app.alive():
                    break
                time.sleep(1.0)
            reentry["attempts"][-1]["delayed_descriptor_seen"] = started
            if started:
                _, names, expired = wait_state(ui, "EXPIRED", 220.0)
                break
            time.sleep(2.0)
        record["expired_ui"] = {
            "at": now_iso(),
            "reentry": reentry,
            "descriptor_delay_armed": descriptor_flag.is_file(),
        }
        record["expired_ui"]["found"] = expired
        record["expired_ui"]["state"] = media_state(names)
        expired_capture = capture(ui, "expired-control")
        record["expired_ui"]["uia"] = expired_capture
        expired_log = log_since(ctx.runtime.log_path, expired_offset)
        record["expired_ui"]["media_access"] = media_access(expired_log)
        record["expired_ui"]["descriptor_delay_lines"] = [
            line
            for line in expired_log.splitlines()
            if "harness media descriptor delay" in line
        ][-8:]
        record["expired_ui"]["exact_expired_text_present"] = expired_capture[
            "expired_text_present"
        ]
        record["expired_ui"]["refetch_control_present"] = expired_capture[
            "refetch_control_present"
        ]
        record["expired_ui"]["image_absent"] = not expired_capture[
            "has_displayed_marker"
        ]
        record["expired_ui"]["elapsed_after_reentry_seconds"] = round(
            time.time() - reentry_at, 3
        )
        if not expired:
            record["residuals"].append(
                "the native pane did not reach the Expired state under the"
                " descriptor delivery delay"
            )
            return record

        # Activate the refetch control while the descriptor delay is still armed:
        # the pane must drop the control for the one in-flight attempt (no retry
        # storm) and then keep the explicit state when the attempt fails again.
        activate_offset = ctx.runtime.log_path.stat().st_size
        activation = activate_refetch(ui)
        record["refetch_after_descriptor_delay"] = {
            "at": now_iso(),
            "activation": activation,
        }
        time.sleep(1.5)
        _, in_flight_names, in_flight_gone = wait_control(ui, False, 20.0)
        record["refetch_after_descriptor_delay"]["control_absent_in_flight"] = (
            in_flight_gone
        )
        record["refetch_after_descriptor_delay"]["state_in_flight"] = media_state(
            in_flight_names
        )
        record["refetch_after_descriptor_delay"]["uia_in_flight"] = capture(
            ui, "refetch-in-flight"
        )
        if in_flight_gone:
            _, retry_names, retry_failed = wait_control(ui, True, 260.0)
        else:
            retry_names, retry_failed = in_flight_names, False
        record["refetch_after_descriptor_delay"]["control_returned"] = retry_failed
        record["refetch_after_descriptor_delay"]["state_after_retry"] = media_state(
            retry_names
        )
        record["refetch_after_descriptor_delay"]["uia_after_retry"] = capture(
            ui, "expired-after-refetch-failure"
        )
        retry_log = log_since(ctx.runtime.log_path, activate_offset)
        record["refetch_after_descriptor_delay"]["media_access"] = media_access(
            retry_log
        )
        record["refetch_after_descriptor_delay"]["descriptor_attempts"] = (
            media_access(retry_log)["descriptor_count"]
        )
        disarm(descriptor_flag)

        # Wire 410: arm the blob request delay so the Runtime itself receives the
        # real handle after its TTL and answers 410 MEDIA_HANDLE_EXPIRED. The
        # client reauthorizes the descriptor once and the second attempt also
        # answers 410, so the pane reaches Expired through the wire answer.
        arm(blob_flag)
        wire_offset = ctx.runtime.log_path.stat().st_size
        wire_activation = activate_refetch(ui)
        record["wire_410"] = {
            "at": now_iso(),
            "activation": wire_activation,
            "blob_delay_armed": blob_flag.is_file(),
        }
        _, wire_in_flight, wire_control_gone = wait_control(ui, False, 20.0)
        record["wire_410"]["control_absent_in_flight"] = wire_control_gone
        if wire_control_gone:
            _, wire_names, wire_failed = wait_control(ui, True, 260.0)
        else:
            wire_names, wire_failed = wire_in_flight, False
        record["wire_410"]["control_returned"] = wire_failed
        record["wire_410"]["state"] = media_state(wire_names)
        record["wire_410"]["uia"] = capture(ui, "expired-wire-410")
        wire_log = log_since(ctx.runtime.log_path, wire_offset)
        wire_access = media_access(wire_log)
        record["wire_410"]["media_access"] = wire_access
        record["wire_410"]["blob_410_count"] = len(wire_access["blob_410"])
        record["wire_410"]["blob_delay_lines"] = blob_delay_lines(wire_log)
        record["wire_410"]["exact_expired_text_present"] = (
            MEDIA_EXPIRED_TEXT in "\n".join(wire_names)
        )
        record["wire_410"]["refetch_control_present"] = refetch_control_present(
            wire_names
        )
        disarm(blob_flag)

        # Success: with no delay armed the same control must resolve a fresh
        # descriptor/handle through the current session identity and present the
        # canonical bytes (DISPLAYED), never a stale frame or fabricated image.
        success_offset = ctx.runtime.log_path.stat().st_size
        success_activation = activate_refetch(ui)
        record["refetch_success"] = {
            "at": now_iso(),
            "activation": success_activation,
        }
        _, _, success_control_gone = wait_control(ui, False, 20.0)
        record["refetch_success"]["control_absent_in_flight"] = success_control_gone
        if success_control_gone:
            _, success_names, displayed = wait_state(ui, "DISPLAYED", 120.0)
        else:
            success_names, displayed = [], False
        success_capture = capture(ui, "displayed-after-refetch")
        record["refetch_success"]["found"] = displayed
        record["refetch_success"]["state"] = media_state(success_names)
        record["refetch_success"]["uia"] = success_capture
        record["refetch_success"]["refetch_control_absent"] = not success_capture[
            "refetch_control_present"
        ]
        success_log = log_since(ctx.runtime.log_path, success_offset)
        success_access = media_access(success_log)
        record["refetch_success"]["media_access"] = success_access
        record["refetch_success"]["descriptor_200"] = 200 in success_access[
            "descriptor_statuses"
        ]
        record["refetch_success"]["blob_200"] = 200 in success_access["blob_statuses"]
        record["refetch_success"]["blob_410"] = 410 in success_access["blob_statuses"]
        if not displayed:
            record["residuals"].append(
                "the refetch after a fresh descriptor did not reach DISPLAYED"
            )

        # Independent protocol check with a fresh credential: the server still
        # serves the canonical bytes and the digest matches the descriptor.
        credential = ctx.runtime.issue_credential()
        token = str((credential.get("document") or {}).get("token") or "")
        secrets_list.append(token)
        observation = ctx.runtime.open_observation(token)
        document = observation.get("document") or {}
        context = str(
            ((document.get("grant") or {}).get("stamp") or {}).get("context_id") or ""
        )
        descriptor = ctx.runtime.media_descriptor(token, context, digest)
        descriptor_document = descriptor.get("document") or {}
        handle = str(descriptor_document.get("access_handle") or "")
        blob = ctx.runtime.media_blob(token, handle) if handle else {}
        raw = bytes(blob.get("raw") or b"")
        record["protocol_check"] = {
            "credential_http_status": credential.get("status"),
            "observation_http_status": observation.get("status"),
            "descriptor_http_status": descriptor.get("status"),
            "descriptor": safe_descriptor(descriptor_document),
            "descriptor_timing": descriptor_timing(descriptor_document),
            "blob_http_status": blob.get("status"),
            "blob_bytes": len(raw),
            "blob_sha256": f"sha256:{sha256_bytes(raw)}" if raw else None,
            "digest_matches_descriptor": (
                f"sha256:{sha256_bytes(raw)}" == digest if raw else False
            ),
        }

        passed = (
            record["cold_start"]["refetch_control_absent"]
            and expired
            and expired_capture["expired_text_present"]
            and expired_capture["refetch_control_present"]
            and expired_capture["viewport_present"]
            and not expired_capture["has_displayed_marker"]
            and record["refetch_after_descriptor_delay"]["control_absent_in_flight"]
            and record["refetch_after_descriptor_delay"]["state_after_retry"]
            == "EXPIRED"
            and record["refetch_after_descriptor_delay"]["control_returned"]
            and record["wire_410"]["blob_410_count"] >= 2
            and record["wire_410"]["state"] == "EXPIRED"
            and record["wire_410"]["refetch_control_present"]
            and displayed
            and record["refetch_success"]["control_absent_in_flight"]
            and not success_capture["refetch_control_present"]
            and record["refetch_success"]["descriptor_200"]
            and record["refetch_success"]["blob_200"]
            and record["protocol_check"]["descriptor_http_status"] == 200
            and record["protocol_check"]["blob_http_status"] == 200
            and record["protocol_check"]["digest_matches_descriptor"]
        )
        record["verdict"] = "PASS" if passed else "FAIL"
        return record
    finally:
        if ctx.app is not None:
            with contextlib.suppress(Exception):
                record["desktop_log_tail"] = acc1.bounded(
                    acc1.redact_text(ctx.app.log_text()), 6000
                )
        ctx.cleanup()
        record["cleanup"] = ctx.record.get("cleanup", {})
        record["finished_at"] = now_iso()
        record["_secrets"] = [value for value in secrets_list if value]
        record["runtime_log_path"] = str(ctx.runtime.log_path)
        record["desktop_log_path"] = (
            str(ctx.app.log_path) if ctx.app is not None else None
        )


# --------------------------------------------------------------------------- default TTL


def scenario_default_ttl() -> dict:
    started_at = now_iso()
    work = WORK_ROOT / f"{time.strftime('%Y%m%dT%H%M%S')}-ttl"
    work.mkdir(parents=True, exist_ok=True)
    acc1.WORK_ROOT = work / "ctx"
    acc1.NativeRuntime = BASE_NATIVE_RUNTIME

    record: dict = {
        "schema_version": 1,
        "kind": "protocol-measurement",
        "label": "default-ttl",
        "started_at": started_at,
        "verdict": "NOT_RUN",
        "precondition": {},
        "descriptor": {},
        "immediate_blob": {},
        "expiry_wait": {},
        "expired_blob": {},
        "fresh_fetch_after_expiry": {},
        "cleanup": {},
        "residuals": [],
    }
    secrets_list: list[str] = []
    ctx = acc1.Ctx("default-ttl", PORT_TTL, control=False, source_run=SOURCE_RUN)
    try:
        start = ctx.start_runtime(wait_generation=False)
        secrets_list.extend([ctx.secret, ctx.handle_secret, ctx.token])
        record["precondition"] = {
            "task_ref": TASK_REF,
            "source_run": str(SOURCE_RUN),
            "runtime_head": run_git("rev-parse", "HEAD", cwd=RUNTIME_ROOT),
            "runtime_epoch": start["epoch"],
            "runtime_server_identity": start["server_identity"],
            "ttl_env": None,
            "ttl_default_seconds": DEFAULT_TTL_SECONDS,
            "port": PORT_TTL,
            "db_copy_sha256": ctx.record.get("db_copy_sha256"),
            "db_copy_bytes": ctx.record.get("db_copy_bytes"),
            "generation_executed": False,
        }
        task = ctx.projection_task(TASK_REF)
        goal = str(task.get("title") or "")
        if not goal:
            record["residuals"].append("the reused DB carries no TASK projection")
            return record
        resolved = ctx.resolve_artifact(TASK_REF)
        digest = str((resolved.get("artifact") or {}).get("artifact_digest") or "")
        if not digest:
            record["residuals"].append("the artifact digest was not projected")
            return record
        record["precondition"]["artifact"] = resolved.get("artifact")

        credential = ctx.runtime.issue_credential()
        token = str((credential.get("document") or {}).get("token") or "")
        secrets_list.append(token)
        observation = ctx.runtime.open_observation(token)
        document = observation.get("document") or {}
        context = str(
            ((document.get("grant") or {}).get("stamp") or {}).get("context_id") or ""
        )
        minted_at = time.time()
        descriptor = ctx.runtime.media_descriptor(token, context, digest)
        descriptor_document = descriptor.get("document") or {}
        handle = str(descriptor_document.get("access_handle") or "")
        record["descriptor"] = {
            "http_status": descriptor.get("status"),
            "descriptor": safe_descriptor(descriptor_document),
            "minted_at": now_iso(),
            "minted_at_unix": minted_at,
            "timing": descriptor_timing(descriptor_document),
            "ttl_override_present": False,
        }
        if descriptor.get("status") != 200 or not handle:
            record["residuals"].append("the default-TTL descriptor was not issued")
            return record
        remaining = record["descriptor"]["timing"]["advertised_remaining_seconds"]
        record["descriptor"]["within_expected_window"] = (
            remaining is not None and 295.0 <= remaining <= 300.0
        )
        if not record["descriptor"]["within_expected_window"]:
            record["residuals"].append(
                "the advertised default expiry is outside the 295-300 s window"
            )

        blob = ctx.runtime.media_blob(token, handle)
        raw = bytes(blob.get("raw") or b"")
        record["immediate_blob"] = {
            "http_status": blob.get("status"),
            "bytes": len(raw),
            "sha256": f"sha256:{sha256_bytes(raw)}" if raw else None,
            "digest_matches_descriptor": (
                f"sha256:{sha256_bytes(raw)}" == digest if raw else False
            ),
            "elapsed_seconds": round(time.time() - minted_at, 3),
        }

        # Wait past the advertised default TTL with a small margin, then present
        # the very same handle: the Runtime's own expiry check must answer 410.
        target = minted_at + DEFAULT_TTL_EXPIRY_WAIT_SECONDS
        wait_started = time.time()
        while time.time() < target:
            time.sleep(min(5.0, target - time.time()))
        record["expiry_wait"] = {
            "waited_seconds": round(time.time() - wait_started, 3),
            "since_descriptor_seconds": round(time.time() - minted_at, 3),
            "at": now_iso(),
        }
        expired = ctx.runtime.media_blob(token, handle)
        expired_raw = bytes(expired.get("raw") or b"")
        expired_document = expired.get("document") or {}
        record["expired_blob"] = {
            "http_status": expired.get("status"),
            "bytes": len(expired_raw),
            "document": expired_document,
            "wire_code": (
                expired_document.get("code")
                if isinstance(expired_document, dict)
                else None
            ),
            "elapsed_after_descriptor_seconds": round(time.time() - minted_at, 3),
        }

        fresh = ctx.runtime.media_descriptor(token, context, digest)
        fresh_document = fresh.get("document") or {}
        fresh_handle = str(fresh_document.get("access_handle") or "")
        fresh_blob = (
            ctx.runtime.media_blob(token, fresh_handle) if fresh_handle else {}
        )
        fresh_raw = bytes(fresh_blob.get("raw") or b"")
        record["fresh_fetch_after_expiry"] = {
            "descriptor_http_status": fresh.get("status"),
            "descriptor": safe_descriptor(fresh_document),
            "descriptor_timing": descriptor_timing(fresh_document),
            "blob_http_status": fresh_blob.get("status"),
            "blob_bytes": len(fresh_raw),
            "blob_sha256": f"sha256:{sha256_bytes(fresh_raw)}" if fresh_raw else None,
            "digest_matches_descriptor": (
                f"sha256:{sha256_bytes(fresh_raw)}" == digest if fresh_raw else False
            ),
        }

        passed = (
            record["descriptor"]["http_status"] == 200
            and record["descriptor"]["within_expected_window"]
            and record["immediate_blob"]["http_status"] == 200
            and record["immediate_blob"]["digest_matches_descriptor"]
            and record["expired_blob"]["http_status"] == 410
            and record["expired_blob"]["elapsed_after_descriptor_seconds"]
            >= DEFAULT_TTL_SECONDS
            and record["fresh_fetch_after_expiry"]["descriptor_http_status"] == 200
            and record["fresh_fetch_after_expiry"]["blob_http_status"] == 200
            and record["fresh_fetch_after_expiry"]["digest_matches_descriptor"]
        )
        record["verdict"] = "PASS" if passed else "FAIL"
        if record["expired_blob"]["http_status"] != 410:
            record["residuals"].append(
                "the same handle after 305 s did not answer the 410 expiry"
            )
        return record
    finally:
        ctx.cleanup()
        record["cleanup"] = ctx.record.get("cleanup", {})
        record["finished_at"] = now_iso()
        record["_secrets"] = [value for value in secrets_list if value]
        record["runtime_log_path"] = str(ctx.runtime.log_path)


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
        if re.search(r"desktop:[0-9a-f]{32}", text):
            findings.append(f"{path.name}: unredacted Desktop principal")
    return {"files_checked": checked, "findings": findings, "clean": not findings}


def write_manifest(evidence: Path) -> None:
    lines = []
    for path in sorted(evidence.rglob("*")):
        if not path.is_file() or path.name == "session.sha256":
            continue
        lines.append(f"{sha256_file(path)}  {path.relative_to(evidence).as_posix()}")
    (evidence / "session.sha256").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--desktop-exe", type=Path, default=base.DESKTOP_EXE)
    parser.add_argument("--only", choices=("native", "ttl"), default=None)
    parser.add_argument("--keep-work", action="store_true")
    parser.add_argument(
        "--manifest-only",
        action="store_true",
        help="Regenerate session.sha256 for the existing evidence directory.",
    )
    args = parser.parse_args(argv)
    if args.manifest_only:
        EVIDENCE.mkdir(parents=True, exist_ok=True)
        write_manifest(EVIDENCE)
        print(f"manifest written: {EVIDENCE / 'session.sha256'}")
        return 0
    desktop_exe = args.desktop_exe.resolve()
    if not desktop_exe.is_file():
        print(f"desktop binary missing: {desktop_exe}", file=sys.stderr)
        return 2
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    secrets_all: list[str] = []
    exit_code = 0

    native: dict | None = None
    if args.only in (None, "native"):
        native = scenario_native(desktop_exe=desktop_exe)
        secrets_all.extend(native.pop("_secrets", []))
        runtime_log = Path(native.get("runtime_log_path") or "")
        desktop_log = Path(native.get("desktop_log_path") or "")
        native["runtime_log"] = (
            sanitize_and_copy(
                runtime_log, EVIDENCE / "logs" / "runtime-native.log.sanitized.txt",
                secrets_all,
            )
            if runtime_log.is_file()
            else {"missing": str(runtime_log)}
        )
        native["desktop_log"] = (
            sanitize_and_copy(
                desktop_log, EVIDENCE / "logs" / "desktop-native.log.sanitized.txt",
                secrets_all,
            )
            if desktop_log.is_file()
            else {"missing": str(desktop_log)}
        )
        native.pop("runtime_log_path", None)
        native.pop("desktop_log_path", None)
        write_json(EVIDENCE / "record-native.json", native)
        print(f"native verdict: {native.get('verdict')}")
        if native.get("verdict") != "PASS":
            exit_code = 1

    ttl: dict | None = None
    if args.only in (None, "ttl"):
        ttl = scenario_default_ttl()
        secrets_all.extend(ttl.pop("_secrets", []))
        runtime_log = Path(ttl.get("runtime_log_path") or "")
        ttl["runtime_log"] = (
            sanitize_and_copy(
                runtime_log, EVIDENCE / "logs" / "runtime-default-ttl.log.sanitized.txt",
                secrets_all,
            )
            if runtime_log.is_file()
            else {"missing": str(runtime_log)}
        )
        ttl.pop("runtime_log_path", None)
        write_json(EVIDENCE / "record-default-ttl.json", ttl)
        print(f"default TTL verdict: {ttl.get('verdict')}")
        if ttl.get("verdict") != "PASS":
            exit_code = 1

    session_path = EVIDENCE / "session.json"
    previous: dict = {}
    if session_path.is_file():
        with contextlib.suppress(Exception):
            previous = json.loads(session_path.read_text(encoding="utf-8"))
    measurements = dict(previous.get("measurements") or {})
    if native is not None:
        measurements["native"] = native.get("verdict")
    if ttl is not None:
        measurements["default_ttl"] = ttl.get("verdict")
    session = {
        "schema_version": 1,
        "kind": "measurement-session",
        "at": now_iso(),
        "updated_at": now_iso(),
        "desktop": {
            "root": str(ROOT),
            "head": run_git("rev-parse", "HEAD"),
            "status_porcelain": run_git("status", "--porcelain"),
            "binary": {
                "path": str(desktop_exe),
                "sha256": sha256_file(desktop_exe),
                "bytes": desktop_exe.stat().st_size,
                "built_at": time.strftime(
                    "%Y-%m-%dT%H:%M:%S", time.localtime(desktop_exe.stat().st_mtime)
                ),
            },
        },
        "runtime": {
            "root": str(RUNTIME_ROOT),
            "head": run_git("rev-parse", "HEAD", cwd=RUNTIME_ROOT),
            "status_porcelain": run_git("status", "--porcelain", cwd=RUNTIME_ROOT),
        },
        "harness": {
            "script": str(Path(__file__).resolve().relative_to(ROOT)),
            "script_sha256": sha256_file(Path(__file__).resolve()),
            "launcher": str(MEDIA_DELAY_LAUNCHER.relative_to(ROOT)),
            "launcher_sha256": sha256_file(MEDIA_DELAY_LAUNCHER),
            "helper_runtime": str(
                (HARNESS / "isolated_runtime.py").relative_to(ROOT)
            ),
            "helper_runtime_sha256": sha256_file(HARNESS / "isolated_runtime.py"),
            "python": sys.version.split()[0],
        },
        "ports": {"native": PORT_NATIVE, "default_ttl": PORT_TTL},
        "secrets": "per-run, memory only; tokens/principals/handles never written",
        "measurements": measurements,
    }
    write_json(session_path, session)
    scan = leak_scan(EVIDENCE, secrets_all)
    write_json(EVIDENCE / "leak-scan.json", scan)
    write_manifest(EVIDENCE)
    print(f"leak scan clean: {scan['clean']} ({scan['files_checked']} files)")
    if not scan["clean"]:
        exit_code = 1
    if not args.keep_work:
        # The harness work directory holds the per-run DB/CAS copy and the raw
        # logs; the durable sanitized evidence is under docs/evidence-published.
        shutil.rmtree(WORK_ROOT, ignore_errors=True)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
