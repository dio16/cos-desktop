#!/usr/bin/env python3
"""MP-ACC1 / RT3 round-2 acceptance harness (test-only, evidence-only).

Three bounded native scenarios against the release Desktop binary and one
isolated Runtime started from ``C:/AI/orchestrator`` (fresh read-only DB copy,
own ports, per-run secrets, redirected discovery descriptor):

1. ``golden`` — the existing closure scenario (``run_acc1_native.py``): one
   real Brief generation after the ComfyUI 0.44.0 / comfy-aimdo 0.6.0 /
   comfy-kitchen 0.2.41 repair, terminal projection plus digest-matching
   artifact. Reused verbatim; the ComfyUI process itself is started outside
   this harness with ``scripts/start_services.ps1``.
2. ``stop-revoked`` — during a live ACTIVE generation, revoke every Desktop
   credential held by the serving Runtime (test-only control channel) and
   invoke the native STOP control immediately after the acknowledgement; the
   record keeps the exact command HTTP outcome, the UI label, whether an
   accepted stop command row exists, and the projected task state. No success
   is claimed from UI text alone.
3. ``runtime-restart`` — during a real generation whose ComfyUI submission
   receipt is already durable, stop the isolated Runtime and restart it on the
   same database copy (new process/epoch). The Desktop stays up and reconnects
   through discovery; the record measures the epoch/credential behaviour, the
   projected task state, the durable provider receipt, and any loss. A terminal
   generation is never inferred: survival is only reported from a projected
   terminal state.

This module does not modify product code. It writes
``docs/development/evidence-published/acc1-round2-latest/``.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import secrets
import shutil
import sqlite3
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
_PINNED_NATIVE = HARNESS.parents[3] / "target" / "acc1-round2-pinned" / "run_acc1_native.py"


def _load_native():
    """Prefer the pinned HEAD copy so a concurrently edited working harness cannot
    change this campaign mid-run; fall back to the live module when it is absent."""
    if _PINNED_NATIVE.is_file():
        spec = importlib.util.spec_from_file_location("acc1_native_pinned", _PINNED_NATIVE)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules["acc1_native_pinned"] = module
        spec.loader.exec_module(module)
        return module
    import run_acc1_native  # noqa: PLC0415 - fallback import

    return run_acc1_native


sys.path.insert(0, str(HARNESS))
import run_acc1 as acc1  # noqa: E402 - local test helper, stdlib only
native = _load_native()
import run_integration as base  # noqa: E402 - local test helper, stdlib only

ROOT = native.ROOT
RUNTIME_ROOT = native.RUNTIME_ROOT
RUNTIME_PYTHON = native.RUNTIME_PYTHON
DESKTOP_EXE = native.DESKTOP_EXE
EVIDENCE_DIR = (
    ROOT / "docs" / "development" / "evidence-published" / "acc1-round2-latest"
)
COMFY_STATS = "http://127.0.0.1:8188/system_stats"
COMFY_HISTORY = "http://127.0.0.1:8188/history/{}"
COMFY_QUEUE = "http://127.0.0.1:8188/queue"

RESTART_SETTLE_SECONDS = 8.0
POST_RESTART_WAIT_SECONDS = 90.0


# --------------------------------------------------------------------------- basics


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def short(value: str) -> str:
    return hashlib.sha256(str(value).encode("utf-8")).hexdigest()[:16]


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2, sort_keys=True),
                    encoding="utf-8")


def bounded(text: str, limit: int = 6000) -> str:
    return text if len(text) <= limit else text[:limit] + "\n...[truncated]"


def file_size(path: Path) -> int:
    try:
        return path.stat().st_size
    except OSError:
        return 0


def read_since(path: Path, offset: int, limit: int = 400_000) -> str:
    try:
        with path.open("rb") as handle:
            handle.seek(max(0, offset))
            return handle.read(limit).decode("utf-8", "replace")
    except OSError as error:
        return f"<unreadable: {type(error).__name__}>"


def selected_lines(text: str, needles: tuple[str, ...], limit: int = 160) -> list[str]:
    lines = [line for line in text.splitlines()
             if any(needle.lower() in line.lower() for needle in needles)]
    return lines[-limit:]


def http_lines(text: str, limit: int = 240) -> list[str]:
    """Serving-log lines relevant to the command/credential path."""
    keys = ("/api/cos/desktop/commands", "/api/cos/desktop/credentials",
            "/api/cos/desktop/observation", "/api/cos/desktop/deltas",
            " 401 ", " 403 ", "Started server", "Shutting down", "Application shutdown",
            "discovery", "epoch")
    return selected_lines(text, keys, limit)


def now_task(task: dict) -> dict:
    keys = ("task_ref", "task_status", "inner_phase", "iteration_index",
            "projection_revision", "capabilities_ref")
    return {key: task.get(key) for key in keys if key in task}


def comfy_stats() -> dict:
    try:
        with urllib.request.urlopen(COMFY_STATS, timeout=8) as response:
            document = json.loads(response.read().decode("utf-8"))
        system = document.get("system") or {}
        packages = {item.get("name"): item.get("installed")
                    for item in system.get("comfy_package_versions") or []}
        return {
            "http_status": response.status,
            "comfyui_version": system.get("comfyui_version"),
            "python_version": system.get("python_version"),
            "argv": system.get("argv"),
            "devices": [item.get("name") for item in document.get("devices") or []],
            "comfy_aimdo": packages.get("comfy-aimdo"),
            "comfy_kitchen": packages.get("comfy-kitchen"),
        }
    except (urllib.error.URLError, TimeoutError, ValueError, OSError) as error:
        return {"http_status": 0, "error": f"{type(error).__name__}: {error}"}


def comfy_prompt_state(prompt_id: str) -> dict:
    if not prompt_id:
        return {"prompt_id_present": False}
    result: dict = {"prompt_id": prompt_id}
    try:
        with urllib.request.urlopen(COMFY_HISTORY.format(prompt_id), timeout=10) as response:
            history = json.loads(response.read().decode("utf-8"))
    except (urllib.error.URLError, TimeoutError, ValueError, OSError) as error:
        result["history_error"] = f"{type(error).__name__}: {error}"
        history = {}
    entry = (history or {}).get(prompt_id) if isinstance(history, dict) else None
    result["history_present"] = entry is not None
    if isinstance(entry, dict):
        status = entry.get("status") or {}
        result["history_status"] = status.get("status_str")
        result["history_completed"] = status.get("completed")
        outputs = entry.get("outputs") or {}
        result["output_keys"] = sorted(outputs)[:8] if isinstance(outputs, dict) else []
        result["output_image_count"] = sum(
            len(node.get("images") or []) for node in outputs.values()
            if isinstance(node, dict)) if isinstance(outputs, dict) else 0
    try:
        with urllib.request.urlopen(COMFY_QUEUE, timeout=10) as response:
            queue = json.loads(response.read().decode("utf-8"))
        running = [item[1] for item in (queue.get("queue_running") or [])
                   if isinstance(item, list) and len(item) > 1]
        pending = [item[1] for item in (queue.get("queue_pending") or [])
                   if isinstance(item, list) and len(item) > 1]
        result["queue_running"] = prompt_id in running
        result["queue_pending"] = prompt_id in pending
        result["queue_running_count"] = len(running)
        result["queue_pending_count"] = len(pending)
    except (urllib.error.URLError, TimeoutError, ValueError, OSError) as error:
        result["queue_error"] = f"{type(error).__name__}: {error}"
    return result


# --------------------------------------------------------------------------- read-only DB


def _ro_connect(db_copy: Path) -> sqlite3.Connection:
    connection = sqlite3.connect(f"file:{db_copy.as_posix()}?mode=ro", uri=True, timeout=5)
    connection.row_factory = sqlite3.Row
    return connection


def db_scalar(db_copy: Path, sql: str, parameters: tuple) -> object:
    try:
        connection = _ro_connect(db_copy)
    except sqlite3.Error:
        return None
    try:
        row = connection.execute(sql, parameters).fetchone()
        return row[0] if row is not None else None
    except sqlite3.Error:
        return None
    finally:
        connection.close()


def db_task_rows(db_copy: Path, task_ref: str) -> dict:
    result: dict = {}
    try:
        connection = _ro_connect(db_copy)
    except sqlite3.Error as error:
        return {"readable": False, "error": f"{type(error).__name__}: {error}"}
    try:
        row = connection.execute(
            "SELECT task_id, status, created_at FROM creative_tasks"
            " WHERE task_id = ?", (task_ref,)).fetchone()
        result["task"] = dict(row) if row is not None else None
        result["runs"] = [dict(item) for item in connection.execute(
            "SELECT run_id, iteration, phase, created_at FROM creative_runs"
            " WHERE task_id = ? ORDER BY iteration", (task_ref,)).fetchall()]
        run_ids = [str(item["run_id"]) for item in result["runs"]]
        receipts: list[dict] = []
        for run_id in run_ids:
            receipts.extend(dict(item) for item in connection.execute(
                "SELECT receipt_id, run_id, node_id, attempt_id,"
                " provider_receipt_id, created_at FROM worker_provider_submission_receipts"
                " WHERE run_id = ?", (run_id,)).fetchall())
        result["provider_receipts"] = receipts
        result["stop_commands"] = [dict(item) for item in connection.execute(
            "SELECT command_id, actor_id, operation, status, receipt_payload, created_at"
            " FROM cos_commands WHERE task_id = ? AND operation = 'stop_task'"
            " ORDER BY created_at", (task_ref,)).fetchall()]
        for command in result["stop_commands"]:
            actor = str(command.pop("actor_id", ""))
            command["actor_sha256_16"] = short(actor)
        outcome = connection.execute(
            "SELECT task_id, outcome, stop_reason, canonical_digest, created_at"
            " FROM creative_task_outcomes WHERE task_id = ?",
            (task_ref,)).fetchone()
        result["outcome"] = dict(outcome) if outcome is not None else None
        result["readable"] = True
        return result
    except sqlite3.Error as error:
        result["readable"] = False
        result["error"] = f"{type(error).__name__}: {error}"
        return result
    finally:
        connection.close()


def wait_for_run(db_copy: Path, task_ref: str, timeout: float) -> dict:
    deadline = time.monotonic() + timeout
    last: dict = {}
    while time.monotonic() < deadline:
        last = db_task_rows(db_copy, task_ref)
        if last.get("runs"):
            return last
        time.sleep(1.0)
    return last


def wait_for_provider_receipt(db_copy: Path, task_ref: str, timeout: float) -> dict:
    deadline = time.monotonic() + timeout
    last: dict = {}
    while time.monotonic() < deadline:
        last = db_task_rows(db_copy, task_ref)
        if last.get("provider_receipts"):
            return last
        time.sleep(1.0)
    return last


def task_state_snapshot(ctx: native.Ctx, task_ref: str) -> dict:
    """Canonical task projection read with a fresh observation (read-only)."""
    response = ctx.runtime.desktop_task(ctx.token, ctx.trace.context, task_ref)
    return {"http_status": response.get("status"),
            "task": now_task((response.get("document") or {}).get("task") or {})}


def wait_button(ui: native.Ui, enabled_label: str, disabled_label: str,
                timeout: float) -> dict:
    deadline = time.monotonic() + timeout
    names: list[str] = []
    while time.monotonic() < deadline:
        names = list(ui.dump().get("names") or [])
        if enabled_label in names:
            return {"enabled": True, "labels": [line for line in names if "停止" in line][:8]}
        if disabled_label in names:
            return {"enabled": False, "labels": [line for line in names if "停止" in line][:8]}
        time.sleep(0.5)
    return {"enabled": False, "labels": [line for line in names if "停止" in line][:8],
            "timed_out": True}


def invoke_stop(ui: native.Ui) -> dict:
    invoked = ui.invoke("タスクを停止")
    method = "uia_invoke"
    if not invoked.get("invoked"):
        invoked = ui.click("タスクを停止")
        method = "synthetic_wm_lbutton_at_uia_bounds"
    return {"method": method, "result": invoked, "at": now_iso()}


def wait_json_file(path: Path, timeout: float) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            try:
                return json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                return {}
        time.sleep(0.1)
    return {}


def terminal(task: dict) -> bool:
    return str(task.get("task_status")) in {"COMPLETED", "CANCELLED", "FAILED"}


def create_brief_with_retry(ctx: native.Ctx, goal: str, direction: str) -> dict:
    """Open/submit the native Brief, retrying once if the surface closed early.

    The closure helper assumes the Brief stays open between its 'surface is
    present' wait and the field entry. In this campaign a Brief sometimes closed
    before the entry, so the field click landed on the inspector. A failed entry
    submits nothing; one bounded reopen+retry is recorded, never a fabricated
    task.
    """
    attempts: list[dict] = []
    brief: dict = {}
    for attempt in (1, 2):
        brief = ctx.create_via_brief(goal, direction)
        attempts.append({
            "attempt": attempt, "verdict": brief.get("verdict"),
            "task_ref": brief.get("task_ref"), "error": brief.get("error"),
            "brief_open": brief.get("brief_open"),
            "submit_enabled": brief.get("submit_enabled"),
        })
        if brief.get("task_ref"):
            break
        ui = ctx.app.ui
        if ui is not None:
            ui.invoke("ブリーフを閉じる")
        time.sleep(3.0)
    brief["attempts"] = attempts
    return brief


def leftover_workers(work: Path) -> list[dict]:
    """Processes whose command line names this run's isolated work directory."""
    needle = str(work).replace("'", "''")
    script = (
        "Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and"
        " $_.CommandLine -like "
        f"'*{needle}*' }} | Select-Object ProcessId,Name,CommandLine | ConvertTo-Json -Compress"
    )
    try:
        completed = subprocess.run(  # noqa: S603 - fixed argv, host inspection only
            ["powershell.exe", "-NoProfile", "-Command", script],
            capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace")
        payload = (completed.stdout or "").strip()
        if not payload:
            return []
        document = json.loads(payload)
        rows = document if isinstance(document, list) else [document]
        return [{key: row.get(key) for key in ("ProcessId", "Name", "CommandLine")}
                for row in rows if isinstance(row, dict)]
    except (OSError, subprocess.SubprocessError, ValueError):
        return []


def settle_leftovers(work: Path, cleanup: dict) -> dict:
    """Terminate only processes that name this run's work directory, and record."""
    before = leftover_workers(work)
    terminated: list[dict] = []
    for row in before:
        pid = row.get("ProcessId")
        if isinstance(pid, int):
            terminated.append(base.kill_tree(pid))
    after = leftover_workers(work)
    cleanup["leftover_workers_before"] = before
    cleanup["leftover_workers_terminated"] = terminated
    cleanup["leftover_workers_after"] = after
    return cleanup


# --------------------------------------------------------------------------- scenario: STOP 401


def scenario_stop_revoked(args: argparse.Namespace) -> dict:
    """Revoke the Desktop credentials mid-generation, then invoke native STOP."""
    ctx = native.Ctx("stop-revoked", native.PORT_INJECT, control=True)
    record = ctx.record
    goal = f"{native.GOAL_PREFIX}-STOP401 {time.strftime('%Y%m%d-%H%M%S')} production still"
    record["goal"] = goal
    record["_secrets"] = list(ctx.collect)
    try:
        ctx.start_runtime()
        record["preflight"] = acc1.probe_generation_plane()
        ctx.launch_desktop("stop-revoked")
        brief = create_brief_with_retry(ctx, goal, "cinematic mechanical desktop object, no text")
        task_ref = brief.get("task_ref", "") or ctx.find_task_ref_in_projection(goal)
        record["task"]["task_ref"] = task_ref
        record["task"]["task_ref_source"] = brief.get("task_ref_source")
        if not task_ref:
            record.update(verdict="NOT_RUN",
                          reason="the Brief created no resolvable task")
            return record
        ui = ctx.app.ui
        assert ui is not None
        record["steps"]["brief_close"] = ui.invoke("ブリーフを閉じる")
        record["steps"]["task_selected"] = ctx.select_task(goal, "stop-revoked-selected", task_ref)
        active = native.wait_task_state(
            ctx, task_ref, lambda task: str(task.get("task_status")) == "ACTIVE", 180.0)
        record["injections"]["wait_active"] = active
        if not active.get("matched"):
            record.update(verdict="NOT_RUN",
                          reason="no ACTIVE projection inside 180 s; no live generation")
            return record
        runs = wait_for_run(ctx.db_copy, task_ref, 90.0)
        record["steps"]["runs_before_seed"] = {
            "run_count": len(runs.get("runs") or []),
            "provider_receipts": len(runs.get("provider_receipts") or []),
        }
        seed = ctx.seed_capabilities(task_ref)
        record["steps"]["capability_seed"] = {
            "seeded": seed.get("seeded"), "run_id": seed.get("run_id"),
            "run_source": seed.get("run_source"),
            "task_status_at_seed": seed.get("task_status_at_seed"),
            "projection_visible": seed.get("projection_visible"),
            "error": seed.get("error")}
        supported = ctx.wait_capability_entry(
            task_ref, "commands", "STOP_TASK", lambda value: value == "SUPPORTED", 90.0)
        record["injections"]["stop_supported_projection"] = supported
        button = wait_button(ui, "タスクを停止", "タスクを停止（現在は操作できません）", 90.0)
        record["injections"]["stop_button_before_revoke"] = button
        record["injections"]["uia_before_revoke"] = native.uia_snapshot(ui, "stop401-before")
        before = task_state_snapshot(ctx, task_ref)
        record["injections"]["task_before_revoke"] = before
        if not button.get("enabled"):
            record.update(verdict="NOT_RUN",
                          reason="the native STOP control never became enabled")
            return record
        if terminal(before.get("task") or {}):
            record.update(verdict="NOT_RUN",
                          reason="the task turned terminal before the revocation injection")
            return record
        app_offset = file_size(ctx.app.log_path)
        runtime_offset = file_size(ctx.runtime.log_path)
        trigger = ctx.control_dir / "revoke-desktop-credentials"
        trigger.write_text("revoke", encoding="utf-8")
        ack = wait_json_file(ctx.control_dir / "revoke-desktop-credentials.ack.json", 20.0)
        record["injections"]["credential_revocation"] = {
            "trigger": "isolated control channel", "ack": ack,
            "acknowledged": bool(ack), "at": now_iso()}
        command = invoke_stop(ui)
        record["injections"]["command"] = command
        time.sleep(2.5)
        after_dump = ui.dump()
        names_after = list(after_dump.get("names") or [])
        record["injections"]["uia_command_result"] = native.uia_snapshot(
            ui, "stop401-command-result")
        record["injections"]["stop_surface_lines"] = [
            line for line in names_after
            if any(key in line for key in (
                "停止", "受付", "拒否", "失敗", "不明", "認証", "再接続", "許可"))][:40]
        record["injections"]["serving_http_window"] = http_lines(
            read_since(ctx.runtime.log_path, runtime_offset))
        record["injections"]["app_log_window"] = selected_lines(
            read_since(ctx.app.log_path, app_offset),
            ("credential", "reauth", "401", "runtime", "observation", "connection"))
        record["injections"]["db_after_command"] = db_task_rows(ctx.db_copy, task_ref)

        credential = ctx.runtime.issue_credential()
        new_token = str((credential.get("document") or {}).get("token") or "")
        record["injections"]["observer_reissue"] = {
            "http_status": credential.get("status"), "token_recorded": False}
        if new_token:
            ctx.collect.append(new_token)
            ctx.token = new_token
            ctx.trace = native.Trace(ctx.runtime, new_token)
            record["injections"]["observer_reopen"] = ctx.trace.open()

        reauth_line = ""
        deadline = time.monotonic() + 120.0
        while time.monotonic() < deadline:
            for line in ctx.app.log_text().splitlines():
                if "reauthorized after rejection" in line:
                    reauth_line = line
            if reauth_line:
                break
            time.sleep(1.0)
        record["injections"]["reauth_log_line"] = reauth_line

        transitions: list[dict] = []
        final_task: dict = {}
        deadline = time.monotonic() + min(float(args.timeout), 600.0)
        while time.monotonic() < deadline:
            polled = ctx.trace.poll(task_ref)
            state = (polled or {}).get("state")
            if state is not None:
                entry = {"at": now_iso(), **now_task(state)}
                if not transitions or transitions[-1].get("task_status") != entry.get("task_status"):
                    transitions.append(entry)
            response = ctx.runtime.desktop_task(ctx.token, ctx.trace.context, task_ref)
            final_task = (response.get("document") or {}).get("task") or {}
            if terminal(final_task):
                break
            time.sleep(2.0)
        record["injections"]["transitions"] = transitions
        record["injections"]["final_task"] = now_task(final_task)
        record["injections"]["uia_terminal"] = native.uia_snapshot(ui, "stop401-terminal")
        record["injections"]["db_final"] = db_task_rows(ctx.db_copy, task_ref)
        record["injections"]["app_log_final"] = selected_lines(
            ctx.app.log_text(), ("credential", "reauth", "401"))
        record["injections"]["serving_http_final"] = http_lines(
            ctx.runtime.log_tail(400_000))

        db_final = record["injections"]["db_final"]
        accepted_rows = [command for command in db_final.get("stop_commands") or []
                         if command.get("status") == "accepted"]
        final_names = "\n".join((record["injections"]["uia_terminal"] or {}).get("names") or [])
        immediate_names = "\n".join(record["injections"].get("stop_surface_lines") or [])
        claimed_accepted = ("停止要求を受け付けました" in final_names
                            or "停止要求を受け付けました" in immediate_names)
        claimed_failure = any(marker in (final_names + immediate_names) for marker in (
            "停止要求に失敗しました", "停止要求の結果は不明です", "停止要求は拒否されました",
            "停止操作は未対応です", "停止要求を受け付けられません",
            "停止状態を確認できません"))
        serving_lines = record["injections"].get("serving_http_final") or []
        command_lines = [line for line in serving_lines
                         if "/api/cos/desktop/commands HTTP" in line]
        lookup_lines = [line for line in serving_lines
                        if "/api/cos/desktop/commands/lookup" in line]
        command_sent = bool(command_lines)
        command_401 = any(" 401 " in line for line in command_lines)
        command_statuses = [re.search(r'"\s+(\d{3})\s', line) for line in command_lines]
        command_statuses = [match.group(1) for match in command_statuses if match]
        observation_401 = any(
            ("/api/cos/desktop/deltas" in line or "/api/cos/desktop/observation" in line)
            and " 401 " in line for line in serving_lines)
        refresh_401 = any("/api/cos/desktop/credentials/refresh" in line
                          and " 401 " in line for line in serving_lines)
        record["injections"]["assertions"] = {
            "stop_command_request_observed": command_sent,
            "stop_command_statuses": command_statuses,
            "command_status_lookup_observed": bool(lookup_lines),
            "command_path_401_observed": command_401,
            "observation_401_observed": observation_401,
            "credential_refresh_401_observed": refresh_401,
            "accepted_stop_row_exists": bool(accepted_rows),
            "ui_claimed_accepted": claimed_accepted,
            "ui_claimed_failure_or_unknown": claimed_failure,
            "fabricated_acceptance": claimed_accepted and not accepted_rows,
        }
        if not command_sent:
            record["verdict"] = "NOT_RUN"
            record["residuals"].append(
                "the native STOP control sent no stop command after the revocation (the"
                " serving access log has no /api/cos/desktop/commands request; only the"
                " command-status lookup was sent); the control rendered the disabled label"
                " at click time, so the stop-command 401 path was not exercised")
        elif claimed_accepted and not accepted_rows:
            record["verdict"] = "FAIL"
            record["residuals"].append(
                "the native surface claimed 停止要求を受け付けました although the isolated"
                " database has no accepted stop_task command")
        elif command_401 and not claimed_accepted and final_task:
            record["verdict"] = "PASS"
            record["residuals"].append(
                "the revoked-credential command returned 401 and the surface reported a"
                " failure/unknown result; completion is read from the projection, not the"
                " command label")
        elif accepted_rows and str(final_task.get("task_status")) == "CANCELLED":
            record["verdict"] = "PARTIAL"
            record["residuals"].append(
                "the STOP command was accepted before the revocation took effect (race);"
                " the command 401 path was not exercised in this run")
        else:
            record["verdict"] = "PARTIAL"
            record["residuals"].append(
                "the command outcome is recorded, but the expected 401-plus-honest-surface"
                " combination was not observed")
        return record
    finally:
        cleanup = ctx.cleanup()
        settle_leftovers(ctx.work, cleanup)
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


# --------------------------------------------------------------------------- scenario: restart


def scenario_runtime_restart(args: argparse.Namespace) -> dict:
    """Stop/restart the Runtime mid-generation; measure provider task survival."""
    ctx = native.Ctx("runtime-restart", native.PORT_MAIN)
    record = ctx.record
    goal = f"{native.GOAL_PREFIX}-RESTART {time.strftime('%Y%m%d-%H%M%S')} production still"
    record["goal"] = goal
    record["_secrets"] = list(ctx.collect)
    try:
        ctx.start_runtime()
        record["preflight"] = acc1.probe_generation_plane()
        epoch_before = native.read_observation_epoch(ctx.db_copy)
        record["epoch"] = {"before": epoch_before,
                           "runtime_reported_before": (record.get("runtime") or {}).get("epoch")}
        ctx.launch_desktop("restart")
        brief = create_brief_with_retry(ctx, goal, "cinematic mechanical desktop object, no text")
        task_ref = brief.get("task_ref", "") or ctx.find_task_ref_in_projection(goal)
        record["task"]["task_ref"] = task_ref
        record["task"]["task_ref_source"] = brief.get("task_ref_source")
        if not task_ref:
            record.update(verdict="NOT_RUN",
                          reason="the Brief created no resolvable task")
            return record
        ui = ctx.app.ui
        assert ui is not None
        record["steps"]["brief_close"] = ui.invoke("ブリーフを閉じる")
        record["steps"]["task_selected"] = ctx.select_task(goal, "restart-selected", task_ref)
        active = native.wait_task_state(
            ctx, task_ref, lambda task: str(task.get("task_status")) == "ACTIVE", 180.0)
        record["injections"]["wait_active"] = active
        if not active.get("matched"):
            record.update(verdict="NOT_RUN",
                          reason="no ACTIVE projection inside 180 s; no live generation")
            return record
        with_receipt = wait_for_provider_receipt(ctx.db_copy, task_ref, 180.0)
        record["injections"]["provider_submitted"] = {
            "run_count": len(with_receipt.get("runs") or []),
            "receipt_count": len(with_receipt.get("provider_receipts") or []),
            "receipts": with_receipt.get("provider_receipts") or [],
            "at": now_iso(),
        }
        if not with_receipt.get("provider_receipts"):
            record.update(verdict="NOT_RUN",
                          reason="no durable ComfyUI submission receipt appeared inside"
                                 " 180 s; there is no live provider task to restart under")
            return record
        prompt_id = str(with_receipt["provider_receipts"][0].get("provider_receipt_id") or "")
        record["injections"]["comfy_before_stop"] = comfy_prompt_state(prompt_id)
        record["injections"]["task_before_stop"] = task_state_snapshot(ctx, task_ref)
        record["injections"]["uia_before_stop"] = native.uia_snapshot(ui, "restart-before-stop")

        app_offset = file_size(ctx.app.log_path)
        runtime_offset = file_size(ctx.runtime.log_path)
        stop_started = time.monotonic()
        stopped = ctx.runtime.stop()
        record["injections"]["runtime_stop"] = {
            "result": stopped, "seconds": round(time.monotonic() - stop_started, 2),
            "at": now_iso(),
            "port_free": native.wait_port_free(ctx.port, 60.0),
            "discovery_present": ctx.runtime.discovery_path.exists(),
        }
        record["injections"]["uia_withheld"] = native.uia_snapshot(ui, "restart-withheld")
        time.sleep(RESTART_SETTLE_SECONDS)
        restarted = ctx.runtime.start()
        record["injections"]["runtime_restart"] = {
            "pid": restarted.get("pid"), "epoch": restarted.get("epoch"),
            "endpoint": restarted.get("endpoint"), "ready": restarted.get("ready"),
            "discovery_present": ctx.runtime.discovery_path.exists(),
            "at": now_iso(),
        }
        record["epoch"]["after"] = native.read_observation_epoch(ctx.db_copy)
        try:
            descriptor = json.loads(ctx.runtime.discovery_path.read_text(encoding="utf-8"))
            record["injections"]["discovery_descriptor"] = {
                "keys": sorted(descriptor.keys()),
                "server_identity": descriptor.get("server_identity"),
                "epoch": descriptor.get("epoch"),
                "origin": descriptor.get("origin"),
            }
        except (OSError, json.JSONDecodeError) as error:
            record["injections"]["discovery_descriptor"] = {
                "error": f"{type(error).__name__}: {error}"}
        record["injections"]["serving_http_restart"] = http_lines(
            read_since(ctx.runtime.log_path, runtime_offset))

        credential = ctx.runtime.issue_credential()
        new_token = str((credential.get("document") or {}).get("token") or "")
        record["injections"]["observer_reissue"] = {
            "http_status": credential.get("status"), "token_recorded": False}
        if new_token:
            ctx.collect.append(new_token)
            ctx.token = new_token
            ctx.trace = native.Trace(ctx.runtime, new_token)
            record["injections"]["observer_reopen"] = ctx.trace.open()

        recovered = False
        auto_deadline = time.monotonic() + 45.0
        while time.monotonic() < auto_deadline:
            dump = ui.dump()
            text = "\n".join(dump.get("names") or [])
            if goal in text and "タスクの状態" in text:
                recovered = True
                break
            time.sleep(2.0)
        record["injections"]["automatic_reconnect_observed"] = recovered
        record["injections"]["uia_after_restart"] = native.uia_snapshot(ui, "restart-after")
        manual: dict | None = None
        if not recovered:
            after_names = "\n".join(
                (record["injections"]["uia_after_restart"] or {}).get("names") or [])
            if "再接続する" in after_names:
                invoked = ui.invoke("再接続する")
                method = "uia_invoke"
                if not invoked.get("invoked"):
                    invoked = ui.click("再接続する")
                    method = "synthetic_wm_lbutton_at_uia_bounds"
                manual = {"method": method, "result": invoked, "at": now_iso()}
            record["injections"]["manual_reconnect"] = manual
            deadline = time.monotonic() + 90.0
            while time.monotonic() < deadline:
                dump = ui.dump()
                text = "\n".join(dump.get("names") or [])
                if goal in text and "タスクの状態" in text:
                    recovered = True
                    break
                time.sleep(2.0)
        record["injections"]["desktop_reconnected"] = recovered
        record["injections"]["manual_reconnect_required"] = not bool(
            record["injections"]["automatic_reconnect_observed"])
        record["injections"]["uia_reconnected"] = native.uia_snapshot(ui, "restart-reconnected")
        record["injections"]["app_log_window"] = selected_lines(
            read_since(ctx.app.log_path, app_offset),
            ("credential", "reauth", "401", "epoch", "observation", "runtime"))
        record["injections"]["serving_http_after_restart"] = http_lines(
            read_since(ctx.runtime.log_path, runtime_offset))
        restart_log = read_since(ctx.runtime.log_path, runtime_offset)
        record["injections"]["serving_log_lock_errors"] = [
            line for line in restart_log.splitlines() if "database is locked" in line]
        record["injections"]["serving_log_drain_errors"] = [
            line for line in restart_log.splitlines()
            if "cos drain" in line and ("failed" in line or "error" in line.lower())]
        if recovered:
            record["steps"]["task_reselected"] = ctx.select_task(
                goal, "restart-reselected", task_ref)
            record["injections"]["uia_task_selected"] = native.uia_snapshot(
                ui, "restart-task-selected")
            record["injections"]["desktop_projection_after_reconnect"] = task_state_snapshot(
                ctx, task_ref)

        transitions: list[dict] = []
        final_task: dict = {}
        deadline = time.monotonic() + POST_RESTART_WAIT_SECONDS
        while time.monotonic() < deadline:
            polled = ctx.trace.poll(task_ref) if ctx.trace.context else {}
            state = (polled or {}).get("state")
            if state is not None:
                entry = {"at": now_iso(), **now_task(state)}
                if not transitions or transitions[-1].get("task_status") != entry.get("task_status"):
                    transitions.append(entry)
            response = ctx.runtime.desktop_task(ctx.token, ctx.trace.context, task_ref)
            final_task = (response.get("document") or {}).get("task") or {}
            if terminal(final_task):
                break
            time.sleep(2.0)
        record["injections"]["transitions_after_restart"] = transitions
        record["injections"]["final_task"] = now_task(final_task)
        record["injections"]["comfy_after_restart"] = comfy_prompt_state(prompt_id)
        record["injections"]["db_after_restart"] = db_task_rows(ctx.db_copy, task_ref)
        record["injections"]["app_log_final"] = selected_lines(
            read_since(ctx.app.log_path, app_offset),
            ("credential", "reauth", "401", "epoch", "observation", "runtime"))
        record["injections"]["serving_http_final"] = http_lines(
            ctx.runtime.log_tail(400_000))

        generation_terminal = str(final_task.get("task_status")) == "COMPLETED"
        same_row = bool((record["injections"]["db_after_restart"].get("task") or {}).get("task_id")
                        == task_ref)
        record["injections"]["assertions"] = {
            "durable_row_present_after_restart": same_row,
            "generation_reached_projected_terminal": generation_terminal,
            "provider_receipt_durable": bool(
                record["injections"]["db_after_restart"].get("provider_receipts")),
            "outcome_present": bool(record["injections"]["db_after_restart"].get("outcome")),
            "no_automatic_resubmission": not generation_terminal,
            "automatic_reconnect_observed": bool(
                record["injections"]["automatic_reconnect_observed"]),
            "manual_reconnect_required": bool(
                record["injections"]["manual_reconnect_required"]),
            "desktop_reconnected": bool(record["injections"]["desktop_reconnected"]),
        }
        if generation_terminal:
            record["verdict"] = "PASS"
        elif same_row and recovered:
            record["verdict"] = "PARTIAL"
            record["residuals"].append(
                "the task row survived the Runtime restart and the Desktop recovered after"
                " the restart, but the in-flight generation did not reach a projected"
                " terminal state; the durable provider receipt remains for manual recovery"
                " and the Runtime did not resubmit")
            if record["injections"]["manual_reconnect_required"]:
                record["residuals"].append(
                    "automatic Desktop reconnection was not observed; the explicit"
                    " 再接続する action was required to reopen the observation under the"
                    " new epoch")
        else:
            record["verdict"] = "NOT_RUN"
            record["residuals"].append(
                "the task row or Desktop reconnection was not observable after the restart")
        return record
    finally:
        cleanup = ctx.cleanup()
        settle_leftovers(ctx.work, cleanup)
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


# --------------------------------------------------------------------------- scenario: fresh-session recovery


def scenario_restart_followup(args: argparse.Namespace) -> dict:
    """Fresh Desktop session on the post-restart DB copy: durable recovery path."""
    source = Path(args.source_run)
    ctx = native.Ctx("restart-followup", native.PORT_MAIN, source_run=source)
    record = ctx.record
    task_ref = args.task_ref
    record["source_run"] = str(source)
    record["task"]["task_ref"] = task_ref
    record["_secrets"] = list(ctx.collect)
    try:
        ctx.start_runtime(wait_generation=False)
        record["epoch"] = {
            "after_restart": native.read_observation_epoch(ctx.db_copy),
            "runtime_reported": (record.get("runtime") or {}).get("epoch")}
        task = ctx.projection_task(task_ref)
        goal = str(task.get("title") or "")
        record["task"]["projection_before_desktop"] = now_task(task) | {"title": goal}
        if not task:
            record.update(verdict="NOT_RUN",
                          reason="the task is not present in the copied post-restart database")
            return record
        ctx.launch_desktop("followup")
        ui = ctx.app.ui
        assert ui is not None
        dump, text, visible = ui.wait_names(
            lambda value: task_ref in value or (goal and goal in value), 90.0, "followup-task")
        record["steps"]["task_visible"] = {
            "visible": visible, "goal": goal,
            "uia": native.uia_snapshot(ui, "followup-connected"),
            "excerpt": "\n".join(
                line for line in text.splitlines()
                if task_ref in line or (goal and goal in line) or "タスクの状態" in line)[:2000],
        }
        if visible:
            record["steps"]["task_selected"] = ctx.select_task(
                goal, "followup-selected", task_ref)
        record["injections"]["task_after_desktop"] = now_task(ctx.projection_task(task_ref))
        record["injections"]["uia_after"] = native.uia_snapshot(ui, "followup-after")
        db = db_task_rows(ctx.db_copy, task_ref)
        record["injections"]["db_after_desktop"] = db
        record["injections"]["assertions"] = {
            "fresh_session_recovered_task": bool(visible),
            "durable_row_present": bool((db.get("task") or {}).get("task_id") == task_ref),
            "task_still_active": (db.get("task") or {}).get("status") == "ACTIVE",
            "provider_receipt_durable": bool(db.get("provider_receipts")),
            "outcome_absent": not db.get("outcome"),
        }
        record["verdict"] = "PASS" if (
            visible and (db.get("task") or {}).get("task_id") == task_ref) else "FAIL"
        if not visible:
            record["residuals"].append(
                "the fresh Desktop session did not show the durable ACTIVE task under the"
                " new epoch")
        return record
    finally:
        cleanup = ctx.cleanup()
        settle_leftovers(ctx.work, cleanup)
        record["_secrets"] = list(ctx.collect)
        record["finished_at"] = now_iso()


# --------------------------------------------------------------------------- main


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", action="append",
                        choices=("golden", "stop-revoked", "runtime-restart",
                                 "restart-followup"),
                        help="scenario to run (repeatable)")
    parser.add_argument("--evidence-dir", default=str(EVIDENCE_DIR))
    parser.add_argument("--desktop-exe", default="")
    parser.add_argument("--timeout", type=float, default=900.0)
    parser.add_argument("--source-run", default="",
                        help="reuse this run's isolated DB copy and CAS")
    parser.add_argument("--task-ref", default="",
                        help="task identity inside the reused DB copy")
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args(argv)
    if "restart-followup" in (args.scenario or []) and (
            not args.source_run or not args.task_ref):
        raise SystemExit("--source-run and --task-ref are required for restart-followup")
    if args.desktop_exe:
        global DESKTOP_EXE
        DESKTOP_EXE = Path(args.desktop_exe).resolve()
        native.DESKTOP_EXE = DESKTOP_EXE
    args.evidence_dir = Path(args.evidence_dir).resolve()
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    (args.evidence_dir / "logs").mkdir(parents=True, exist_ok=True)
    scenarios = args.scenario or ["golden"]
    stats = comfy_stats()

    session: dict = {
        "measurement": "MP-ACC1 round 2 (post-repair generation, revoked-credential STOP,"
                       " provider task survival across Runtime restart)",
        "contracts": ["DC-MP-ACC1", "DC-MP-RT3"],
        "started_at": now_iso(), "verdict": "NOT_RUN",
        "harness": {
            "command": "python apps/desktop/devtools/runtime_integration/run_acc1_round2.py",
            "scenarios": scenarios, "evidence_dir": str(args.evidence_dir),
            "ports": {"8132": "runtime-restart", "8133": "stop-revoked (control)"},
            "observation": "native window (UIA + PrintWindow) plus a harness read:observe client",
        },
        "comfyui_preflight": stats,
        "desktop": {
            "root": str(ROOT), "head": base.git_value(ROOT, "rev-parse", "HEAD"),
            "dirty_paths": [line for line in base.git_value(
                ROOT, "status", "--porcelain=v1").splitlines()][:40],
            "binary_launched": str(DESKTOP_EXE),
            "binary_sha256": sha256_file(DESKTOP_EXE) if DESKTOP_EXE.is_file() else "missing",
            "binary_bytes": DESKTOP_EXE.stat().st_size if DESKTOP_EXE.is_file() else 0,
            "build": {} if args.no_build else native.build_desktop(),
        },
        "runtime_source": {
            "root": str(RUNTIME_ROOT), "head": base.git_value(RUNTIME_ROOT, "rev-parse", "HEAD"),
            "python": str(RUNTIME_PYTHON),
            "dirty_tracked": [line for line in base.git_value(
                RUNTIME_ROOT, "status", "--porcelain=v1").splitlines() if line[:2].strip()][:40],
        },
        "runs": {}, "leak_check": {}, "cleanup": {}, "not_measured": [], "residuals": [],
    }
    if stats.get("http_status") != 200:
        session["verdict"] = "NOT_RUN"
        session["error"] = ("ComfyUI /system_stats did not answer 200; the generation plane"
                            " is not available (exact probe retained)")
        write_json(args.evidence_dir / "session.json", session)
        return 2
    if not args.no_build and session["desktop"]["build"].get("exit_code") != 0:
        session["verdict"] = "FAIL"
        session["error"] = "the release binary did not build at the current HEAD"
        write_json(args.evidence_dir / "session.json", session)
        return 1

    collected_secrets: list[str] = []
    prior_runs: dict = {}
    prior_leak_checks: list[dict] = []
    prior_path = args.evidence_dir / "session.json"
    if prior_path.is_file():
        try:
            prior_session = json.loads(prior_path.read_text(encoding="utf-8")) or {}
            prior_runs = prior_session.get("runs") or {}
            prior_leak_checks = list(prior_session.get("leak_checks") or [])
            if prior_session.get("leak_check"):
                prior_leak_checks.append(prior_session["leak_check"])
        except (OSError, json.JSONDecodeError):
            prior_runs = {}
    for name in scenarios:
        try:
            if name == "golden":
                run = native.scenario_golden(args)
            elif name == "stop-revoked":
                run = scenario_stop_revoked(args)
            elif name == "restart-followup":
                run = scenario_restart_followup(args)
            else:
                run = scenario_runtime_restart(args)
        except Exception as error:  # noqa: BLE001 - record the failure, never hide it
            import traceback
            run = {
                "label": name, "started_at": now_iso(), "verdict": "FAIL",
                "error": f"{type(error).__name__}: {error}",
                "traceback": traceback.format_exc()[-4000:],
                "work_dir": "", "_secrets": [],
            }
        collected_secrets.extend(run.get("_secrets") or [])
        native.copy_run_evidence(run, args.evidence_dir)
        run = native.redact_document(run)
        run.pop("_secrets", None)
        session["runs"][name] = run
        write_json(args.evidence_dir / f"record-{name}.json", run)
        write_json(args.evidence_dir / "session-partial.json", session)
        if comfy_stats().get("http_status") != 200:
            session["generation_plane_lost_at"] = now_iso()
            session["generation_plane_last_run"] = name
            session["generation_plane_probe"] = comfy_stats()
            break

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
        "ports_after_runs": {str(port): native.port_open(port)
                             for port in (native.PORT_MAIN, native.PORT_INJECT)},
        "comfyui_8188_open": native.port_open(8188),
        "runs": {name: run.get("cleanup") for name, run in session["runs"].items()},
    }
    session = native.redact_document(session)
    session["leak_check"] = native.leak_scan(args.evidence_dir, collected_secrets)
    session["leak_checks"] = prior_leak_checks + [session["leak_check"]]
    write_json(args.evidence_dir / "session.json", session)
    write_json(args.evidence_dir / "record.json", {"runs": session["runs"]})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
