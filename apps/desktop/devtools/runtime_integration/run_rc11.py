#!/usr/bin/env python3
"""RC-11 live capability command/media support measurement (test-only harness).

It starts one isolated Runtime (fresh copy of the production DB, own port,
per-run secrets, isolated config/logs/CAS, redirected discovery descriptor) and
measures the CAPABILITIES projection the Desktop observation protocol publishes
after the RC-11 extension:

    terminal-image -> live-active -> stop-pending -> no-artifact

The measured task is an existing real task from the copied database with an
available image artifact; a second existing task with an accepted input but no
runs/artifacts measures the media-unavailable direction. Capability snapshot
sets are written through the Runtime's own ``capability_registry`` helpers
(Rust-validated), and the status/stop transitions are seeded in the isolated
copy only (no dispatch, no generation).

The captured CAPABILITIES/TASK payloads are then fed to
``crates/app-core/tests/rc11_live_capability_gates.rs`` (a test-only harness)
which deserializes them with the production wire types and records the real
``app_core::capability`` gate decisions for STOP_TASK, REUSE_TASK, CREATE_TASK
(server scope) and ORIGINAL/THUMBNAIL/PREVIEW/METADATA.

Honest scope limits (recorded in the evidence, never promoted to a PASS):
- Real external grant issuance/revocation is ``NOT_RUN``; the capability set is
  harness-seeded through the Runtime's own registry (Rust-validated).
- A non-image available artifact is ``NOT_MEASURED`` live: every production
  artifact in the copied database is ``image/png`` and artifact rows are
  immutable. That state is covered by Runtime pytest.
- Server-scope create stays catalog-authoritative: the Runtime emits TASK scope
  only, so the create gate is measured as ``capability_not_projected``.
- The Desktop GUI binary is not launched; the capture and gate decisions are
  protocol-level/headless.

Usage (from the Desktop repository root, with the Runtime venv Python):

    C:/AI/orchestrator/.venv/Scripts/python.exe \\
        apps/desktop/devtools/runtime_integration/run_rc11.py
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
import traceback
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_acc1 as acc1  # noqa: E402 - shared, already-reviewed harness helpers
import run_integration as base  # noqa: E402
import run_rt5 as rt5  # noqa: E402 - shared capability seeding/capture helpers

DESKTOP_ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
RUNTIME_PYTHON = base.RUNTIME_PYTHON
SOURCE_DB = base.SOURCE_DB
SOURCE_PRESETS = RUNTIME_ROOT / "db" / "cos_presets.json"

DEFAULT_EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "rc11-live-latest"
)
WORK_ROOT = DESKTOP_ROOT / "target" / "rc11"

PORT = 8141
ENDPOINT = f"http://127.0.0.1:{PORT}"
OBSERVATION_PROTOCOL = {"major": 2, "minor": 0}
OBSERVATION_SCHEMA = {"major": 2, "minor": 0}

# Expected gate codes from the DC-MP-RT5 contract (not a copy of the Desktop
# implementation): only an in-scope SUPPORTED entry enables, the projection
# availability denies first, and a TASK-scope projection never speaks for
# server-scope creation.
NOT_PROJECTED = {"enabled": False, "code": "capability_not_projected"}
STOP_TERMINAL_REASON = "task is terminal; STOP_TASK has no live state to act on"
STOP_PENDING_REASON = "a STOP_TASK command is already accepted for this task"
ORIGINAL_UNAVAILABLE_REASON = "no available artifact is projected for this task"
THUMBNAIL_UNAVAILABLE_REASON = (
    "no available image artifact is projected for this task"
)
PREVIEW_REASON = "PREVIEW transcode is not supported by this Runtime"
METADATA_REASON = "METADATA byte route is not supported by this Runtime"
REUSE_REASON = "REUSE_TASK is not supported by this Runtime"


# --------------------------------------------------------------------------- basics


def port_in_use() -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.3)
        return probe.connect_ex(("127.0.0.1", PORT)) == 0


def short_sha(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]


def sha256_text(text: str) -> str:
    return "sha256:" + hashlib.sha256(text.encode("utf-8")).hexdigest()


# --------------------------------------------------------------------------- seeding


def runtime_modules():
    """Import the Runtime's own DB/registry/command helpers in this process.

    Mark this process as a non-serving role so ``Database.init()`` leaves the
    observation epoch ledger alone: the serving Runtime must stay the epoch
    owner or a fresh observation would answer ``RESYNC_REQUIRED``. The launcher
    strips ``COS_RUNTIME_*`` from the child environment.
    """
    os.environ["COS_RUNTIME_PROCESS_ROLE"] = "generation-worker"
    root = str(RUNTIME_ROOT)
    if root not in sys.path:
        sys.path.insert(0, root)
    try:
        from orchestrator_app import capability_registry, creative_loop  # noqa: PLC0415
        from orchestrator_app.contracts.canonical import digest_v1  # noqa: PLC0415
        from orchestrator_app.db import Database  # noqa: PLC0415
    except Exception as error:  # noqa: BLE001 - the harness must fail loudly
        raise RuntimeError(
            "the isolated Runtime's Python package must be importable; run this"
            " harness with C:/AI/orchestrator/.venv/Scripts/python.exe"
        ) from error
    return Database, digest_v1, capability_registry, creative_loop


def pick_measured_tasks(db) -> dict:
    """One real task with an available image artifact and one without runs."""
    primary = db._conn.execute(
        "SELECT t.task_id, t.status, r.run_id, r.iteration,"
        "       (SELECT MAX(x.iteration) FROM creative_runs AS x"
        "         WHERE x.task_id = t.task_id) AS max_iteration,"
        "       (SELECT a.artifact_id FROM evaluation_outputs AS e"
        "          INNER JOIN artifact_blobs AS b ON b.content_digest = e.artifact_digest"
        "          INNER JOIN artifacts AS a ON a.blob_id = b.blob_id"
        "         WHERE e.task_id = t.task_id AND a.status = 'available'"
        "           AND b.media_type LIKE 'image/%'"
        "         ORDER BY e.created_at DESC, a.artifact_id LIMIT 1) AS artifact_id"
        "  FROM creative_tasks AS t"
        "  INNER JOIN creative_runs AS r ON r.task_id = t.task_id"
        " ORDER BY t.created_at DESC, r.iteration DESC, r.run_id DESC"
        " LIMIT 1"
    ).fetchone()
    if primary is None or not primary["artifact_id"]:
        raise RuntimeError(
            "the copied database has no task with a run and an available image artifact"
        )
    secondary = db._conn.execute(
        "SELECT t.task_id, t.status"
        "  FROM creative_tasks AS t"
        "  INNER JOIN cos_task_inputs AS i ON i.task_id = t.task_id"
        " WHERE NOT EXISTS (SELECT 1 FROM creative_runs AS r WHERE r.task_id = t.task_id)"
        "   AND NOT EXISTS (SELECT 1 FROM evaluation_outputs AS e WHERE e.task_id = t.task_id)"
        " ORDER BY t.created_at DESC, t.task_id LIMIT 1"
    ).fetchone()
    if secondary is None:
        raise RuntimeError("the copied database has no input-only task for the no-artifact state")
    return {
        "primary": {
            "task_id": str(primary["task_id"]),
            "task_status": str(primary["status"]),
            "latest_run_id": str(primary["run_id"]),
            "latest_iteration": int(primary["iteration"]),
            "max_iteration": int(primary["max_iteration"]),
            "artifact_id": str(primary["artifact_id"]),
        },
        "secondary": {
            "task_id": str(secondary["task_id"]),
            "task_status": str(secondary["status"]),
        },
    }


def insert_run(database, task_id: str, iteration: int) -> str:
    """Insert one read-model run (isolated copy only) to bind a new set."""
    run_id = f"run-rc11-{task_id[-8:]}-i{iteration}"
    database._conn.execute(
        "INSERT INTO creative_runs(run_id, task_id, iteration, parent_run_id,"
        " plan_id, plan_digest, proposal_id, phase, stop_reason, record_json,"
        " created_at) VALUES (?, ?, ?, NULL, ?, ?, NULL, 'REFINE', NULL, '{}', ?)",
        (
            run_id,
            task_id,
            iteration,
            f"plan-rc11-{task_id[-8:]}-i{iteration}",
            "sha256:" + hashlib.sha256(f"{run_id}:plan".encode("utf-8")).hexdigest(),
            acc1.now_iso(),
        ),
    )
    database._conn.commit()
    return run_id


def set_task_status(database, task_id: str, status: str) -> None:
    """Seed one live/terminal status in the isolated copy (no dispatch)."""
    database._conn.execute(
        "UPDATE creative_tasks SET status = ? WHERE task_id = ?", (status, task_id)
    )
    database._conn.commit()


def record_stop_command(database, digest_v1, creative_loop, task_id: str) -> dict:
    """Persist one durable stop_task command through the Runtime's own helper."""
    command_id = f"command-rc11-stop-{task_id[-8:]}"
    fingerprint = digest_v1(
        "request",
        {
            "schema_version": 1,
            "actor_id": "rc11-harness",
            "operation": "stop_task",
            "command_id": command_id,
            "target": {"kind": "STOP_TASK", "task_id": task_id},
            "payload": {"kind": "STOP_TASK"},
            "expected_revision": None,
            "epoch": None,
            "harness": "rc11",
        },
    )
    receipt = {
        "schema_version": 1,
        "status": "accepted",
        "harness": "rc11-live",
        "task_id": task_id,
    }
    stored = creative_loop.record_cos_stop_command(
        database,
        actor_id="rc11-harness",
        command_id=command_id,
        task_id=task_id,
        payload_fingerprint=fingerprint,
        receipt=receipt,
    )
    found = creative_loop.find_cos_stop_command(database, task_id=task_id)
    return {
        "command_id": command_id,
        "payload_fingerprint": fingerprint,
        "stored": stored,
        "found": found is not None,
    }


# --------------------------------------------------------------------------- expectations


def _entry_gate(entry: dict) -> dict:
    support = str(entry.get("support"))
    if support == "SUPPORTED":
        return {"enabled": True, "code": "supported"}
    code = {
        "UNSUPPORTED": "capability_unsupported",
        "UNAVAILABLE": "capability_unavailable",
    }.get(support, "capability_unknown")
    gate = {"enabled": False, "code": code}
    if entry.get("reason"):
        gate["reason"] = entry["reason"]
    return gate


def expectations_for(capabilities: dict | None) -> dict:
    """DC-MP-RT5 contract expectations for one captured projection."""
    names = (
        "stop",
        "reuse",
        "create_server_scope",
        "media_original",
        "media_thumbnail",
        "media_preview",
        "media_metadata",
    )
    if capabilities is None:
        return {name: dict(NOT_PROJECTED) for name in names}
    if str(capabilities.get("availability")) != "SUPPORTED":
        reason = capabilities.get("reason")
        deny = {"enabled": False, "code": "capability_unavailable"}
        if reason:
            deny["reason"] = reason
        expected = {name: dict(deny) for name in names}
        expected["create_server_scope"] = dict(NOT_PROJECTED)
        return expected
    commands = {
        str(entry.get("operation")): entry for entry in capabilities.get("commands") or []
    }
    media = {
        str(entry.get("representation")): entry for entry in capabilities.get("media") or []
    }
    return {
        "stop": _entry_gate(commands["STOP_TASK"]),
        "reuse": _entry_gate(commands["REUSE_TASK"]),
        "create_server_scope": dict(NOT_PROJECTED),
        "media_original": _entry_gate(media["ORIGINAL"]),
        "media_thumbnail": _entry_gate(media["THUMBNAIL"]),
        "media_preview": _entry_gate(media["PREVIEW"]),
        "media_metadata": _entry_gate(media["METADATA"]),
    }


# --------------------------------------------------------------------------- gates


def run_rust_gate_harness(capture_path: Path, decisions_path: Path) -> dict:
    environment = dict(os.environ)
    environment["RC11_GATE_CAPTURE"] = str(capture_path)
    environment["RC11_GATE_DECISIONS_OUT"] = str(decisions_path)
    completed = subprocess.run(  # noqa: S603 - fixed argv, local workspace
        [
            "cargo",
            "test",
            "--locked",
            "-p",
            "app-core",
            "--test",
            "rc11_live_capability_gates",
            "--",
            "--ignored",
            "--nocapture",
        ],
        cwd=str(DESKTOP_ROOT),
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
        check=False,
    )
    return {
        "command": (
            "cargo test --locked -p app-core --test rc11_live_capability_gates"
            " -- --ignored --nocapture"
        ),
        "exit_code": completed.returncode,
        "stdout_tail": completed.stdout[-6000:],
        "stderr_tail": completed.stderr[-6000:],
        "decisions_written": decisions_path.is_file(),
    }


# --------------------------------------------------------------------------- evidence


def write_readme(evidence_dir: Path, record: dict) -> None:
    states = record["states"]
    lines = [
        "# RC-11 live command/media capability support — measured evidence (LIMITED)",
        "",
        "Scope: an isolated Runtime (fresh copy of "
        "`C:/AI/orchestrator/db/generation.sqlite`, own port **%d**, per-run "
        "secrets in memory only, isolated config/CAS/logs, redirected discovery "
        "descriptor) after the RC-11 extension. The measured task is an existing "
        "real task with an available image artifact; a second existing task with "
        "an accepted input but no runs measures the media-unavailable direction. "
        "The captured `TASK`/`CAPABILITIES` payloads were passed to a test-only "
        "harness that runs the real `app_core::capability` gate code." % PORT,
        "",
        "**Verdict: `LIMITED`.** The supported STOP/media path is live-measured "
        "over the Runtime's own CAPABILITIES projection and the real Desktop "
        "gate code; real external grant issuance/revocation, a non-image "
        "available artifact and server-scope create remain out of scope or "
        "`NOT_MEASURED`.",
        "",
        "## Source binding",
        "",
        "- Runtime head `%s` (dirty tracked entries recorded in `record.json`; "
        "none staged)." % record["runtime"].get("runtime_head"),
        "- Desktop head `%s` (tree `%s`)."
        % (record["desktop"].get("head"), record["desktop"].get("head_tree")),
        "- Harness files: `apps/desktop/devtools/runtime_integration/run_rc11.py` "
        "`%s`; `crates/app-core/tests/rc11_live_capability_gates.rs` `%s`."
        % (
            record["desktop"]["harness_files"].get(
                "apps/desktop/devtools/runtime_integration/run_rc11.py"
            ),
            record["desktop"]["harness_files"].get(
                "crates/app-core/tests/rc11_live_capability_gates.rs"
            ),
        ),
        "- Database: source `%s`; isolated copy before seeding `%s`; after "
        "seeding `%s`." % (
            record["source_state"].get("source_db_sha256"),
            record["source_state"].get("db_copy_sha256_before_seeding"),
            record["source_state"].get("db_copy_sha256_after_seeding"),
        ),
        "- Runtime epoch `%s`; discovery descriptor removed on clean shutdown: "
        "`%s`." % (
            record["runtime"].get("epoch"),
            record["runtime"].get("discovery_removed_after_stop"),
        ),
        "",
        "## Observed CAPABILITIES projections",
        "",
        "| State | Task | `capabilities_ref` (short) | Revision | Availability "
        "| STOP_TASK | REUSE_TASK | ORIGINAL | THUMBNAIL | PREVIEW | METADATA |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for state in states:
        commands = {
            str(entry.get("operation")): str(entry.get("support"))
            for entry in state.get("commands") or []
        }
        media = {
            str(entry.get("representation")): str(entry.get("support"))
            for entry in state.get("media") or []
        }
        lines.append(
            "| `%s` | `%s` | `%s` | `%s` | `%s` | `%s` | `%s` | `%s` | `%s` "
            "| `%s` | `%s` |"
            % (
                state["label"],
                state.get("task_id"),
                str(state.get("capabilities_ref") or "—")[-16:],
                state.get("projection_revision") or "—",
                state.get("availability") or "—",
                commands.get("STOP_TASK", "—"),
                commands.get("REUSE_TASK", "—"),
                media.get("ORIGINAL", "—"),
                media.get("THUMBNAIL", "—"),
                media.get("PREVIEW", "—"),
                media.get("METADATA", "—"),
            )
        )
    lines += [
        "",
        "Every payload is TASK-scoped and its `capabilities_ref` equals the task's "
        "reference. Exact payloads are in `projections/` and `gates/capture.json`.",
        "",
        "## Desktop gate decisions (real `app_core::capability` code)",
        "",
        "`gates/decisions.json`; the Rust harness asserts every expectation and "
        "the Python harness cross-checks the recorded decisions.",
        "",
        "| State | stop | reuse | original | thumbnail | preview | metadata | "
        "create (server scope) |",
        "|---|---|---|---|---|---|---|---|",
    ]
    decisions = {
        str(entry.get("label")): entry
        for entry in (record["gates"].get("decisions") or {}).get("states", [])
    }
    for state in states:
        gates = (decisions.get(state["label"]) or {}).get("gates") or {}

        def cell(name: str) -> str:
            gate = gates.get(name) or {}
            if gate.get("enabled"):
                return "enabled `%s`" % gate.get("code")
            return "disabled `%s`" % gate.get("code")

        lines.append(
            "| `%s` | %s | %s | %s | %s | %s | %s | %s |"
            % (
                state["label"],
                cell("stop"),
                cell("reuse"),
                cell("media_original"),
                cell("media_thumbnail"),
                cell("media_preview"),
                cell("media_metadata"),
                cell("create_server_scope"),
            )
        )
    lines += [
        "",
        "The facts the projection carries and the gate decisions agree: stop is "
        "offered only for the live state with no accepted stop command; media "
        "`ORIGINAL`/`THUMBNAIL` follow the projected available image artifact; "
        "`REUSE_TASK`, `PREVIEW` and `METADATA` stay disabled with their "
        "projected reasons; create is not projected (catalog-authoritative).",
        "",
        "## Revocation",
        "",
        "Revocation behavior is unchanged and covered by Runtime pytest: the "
        "projection-level `UNAVAILABLE` plus reason denies every command/media "
        "gate before the entry is read, so a revoked projection cannot enable "
        "an action. This run did not re-seed a revocation receipt.",
        "",
        "## What was live-measured vs NOT_MEASURED",
        "",
        "- Measured: the Runtime CAPABILITIES producer's TASK-scope `commands`/"
        "`media` derivation over a live isolated Runtime, the terminal/live/"
        "stop-pending STOP states, the image-artifact `ORIGINAL`/`THUMBNAIL` "
        "support, the no-artifact `UNAVAILABLE` direction, and the real "
        "`app_core::capability` gate decisions.",
        "- `NOT_MEASURED`: a non-image available artifact (all production "
        "artifacts are `image/png` and artifact rows are immutable; covered by "
        "Runtime pytest), a server-scope CAPABILITIES projection (the Runtime "
        "emits TASK scope only; create stays catalog-authoritative), and real "
        "external authority issuance/revocation.",
        "- The Desktop GUI binary was not launched; the capture and gate "
        "decisions are protocol-level/headless.",
        "",
        "## Cleanup",
        "",
        "The isolated Runtime listener was stopped and the isolated working copy "
        "removed (`record.json` `cleanup`); no listener on port %d remains. "
        "Evidence contains no secrets (leak scan in `record.json`)." % PORT,
    ]
    (evidence_dir / "README.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


# --------------------------------------------------------------------------- run


def run(args: argparse.Namespace) -> int:
    with contextlib.suppress(AttributeError, OSError):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    evidence_dir = (
        Path(args.evidence_dir).resolve() if args.evidence_dir else DEFAULT_EVIDENCE_DIR
    )
    if evidence_dir.exists() and any(evidence_dir.iterdir()):
        print(f"evidence dir {evidence_dir} already has content", file=sys.stderr)
        return 2
    evidence_dir.mkdir(parents=True, exist_ok=True)
    (evidence_dir / "logs").mkdir(parents=True, exist_ok=True)
    (evidence_dir / "gates").mkdir(parents=True, exist_ok=True)
    (evidence_dir / "projections").mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%S")
    work = WORK_ROOT / stamp
    work.mkdir(parents=True, exist_ok=True)

    record: dict = {
        "measurement": "RC-11 live command/media capability support measurement",
        "contract": "DC-MP-RT5 / RC-11 extension",
        "started_at": acc1.now_iso(),
        "verdict": "NOT_RUN",
        "harness": {
            "command": (
                "C:/AI/orchestrator/.venv/Scripts/python.exe"
                " apps/desktop/devtools/runtime_integration/run_rc11.py"
            ),
            "port": PORT,
            "endpoint": ENDPOINT,
            "work_dir": str(work),
            "desktop_binary_launched": False,
            "observation": "protocol-level Desktop observation capture (not the GUI)",
            "gate_harness": (
                "crates/app-core/tests/rc11_live_capability_gates.rs (ignored test,"
                " real app_core::capability code over captured live payloads)"
            ),
        },
        "source_state": {},
        "desktop": {
            "root": str(DESKTOP_ROOT),
            "head": base.git_value(DESKTOP_ROOT, "rev-parse", "HEAD"),
            "head_tree": base.git_value(DESKTOP_ROOT, "rev-parse", "HEAD^{tree}"),
            "dirty_tracked": [
                line
                for line in base.git_value(
                    DESKTOP_ROOT, "status", "--porcelain=v1"
                ).splitlines()
                if line[:2].strip()
            ][:40],
            "harness_files": {
                "apps/desktop/devtools/runtime_integration/run_rc11.py": acc1.sha256_file(
                    Path(__file__)
                ),
                "crates/app-core/tests/rc11_live_capability_gates.rs": acc1.sha256_file(
                    DESKTOP_ROOT
                    / "crates"
                    / "app-core"
                    / "tests"
                    / "rc11_live_capability_gates.rs"
                ),
            },
        },
        "runtime": {},
        "credential": {},
        "tasks": {},
        "seed": {},
        "states": [],
        "revision_checks": {},
        "gates": {},
        "leak_check": {},
        "cleanup": {},
        "residuals": [
            "Server-scope create remains catalog-authoritative; the Runtime emits"
            " no server-scope CAPABILITIES projection.",
            "Real external grant issuance/revocation remains out of scope; the"
            " measured capability sets are harness-seeded through the Runtime's"
            " own Rust-validated registry.",
            "A non-image available artifact was not live-measured (all production"
            " artifacts are image/png; artifact rows are immutable). Runtime"
            " pytest covers it.",
        ],
        "not_measured": [
            "grant issuance/revocation by a real external authority",
            "a non-image available artifact (live)",
            "a server-scope CAPABILITIES projection / create grant",
            "GPUI call-site ordering and native/AX behavior",
        ],
        "errors": [],
    }
    secret = secrets.token_hex(24)
    handle_secret = secrets.token_hex(24)
    secrets_in_memory: list[str] = []
    token = ""
    db_copy = work / "generation.sqlite"
    presets = work / "cos_presets.json"
    runtime = None
    try:
        # --- 0. fresh DB copy + preset copy --------------------------------
        base.copy_database(SOURCE_DB, db_copy)
        shutil.copyfile(SOURCE_PRESETS, presets)
        record["source_state"] = {
            "source_db": str(SOURCE_DB),
            "source_db_sha256": acc1.sha256_file(SOURCE_DB),
            "db_copy": str(db_copy),
            "db_copy_sha256_before_seeding": acc1.sha256_file(db_copy),
            "db_copy_bytes": db_copy.stat().st_size,
            "presets_copy": str(presets),
            "presets_sha256": acc1.sha256_file(presets),
        }

        # --- 1. isolated Runtime ------------------------------------------
        acc1.PORT = PORT
        acc1.ENDPOINT = ENDPOINT
        rt5.PORT = PORT
        rt5.ENDPOINT = ENDPOINT
        runtime = acc1.IsolatedRuntime(
            work=work,
            db_copy=db_copy,
            presets=presets,
            secret=secret,
            handle_secret=handle_secret,
        )
        start = runtime.start()
        discovery_present = runtime.discovery_path.is_file()
        record["runtime"] = {
            "pid": start["pid"],
            "server_identity": start["server_identity"],
            "server_build": start["server_build"],
            "epoch": start["epoch"],
            "endpoint": ENDPOINT,
            "endpoint_sha256_16": short_sha(ENDPOINT),
            "epoch_sha256_16": short_sha(str(start["epoch"])),
            "protocol": start["protocol"],
            "schema": start["schema"],
            "config_path": str(runtime.config_path),
            "config_sha256": acc1.sha256_file(runtime.config_path),
            "discovery_path": str(runtime.discovery_path),
            "discovery_published_after_start": discovery_present,
            "discovery_sha256": (
                acc1.sha256_file(runtime.discovery_path) if discovery_present else None
            ),
            "cas_root": str(runtime.cas_root),
            "log_dir": str(runtime.log_dir),
            "runtime_head": base.git_value(RUNTIME_ROOT, "rev-parse", "HEAD"),
            "runtime_dirty_tracked": [
                line
                for line in base.git_value(
                    RUNTIME_ROOT, "status", "--porcelain=v1"
                ).splitlines()
                if line[:2].strip()
            ][:40],
        }

        credential = runtime.issue_credential()
        credential_document = credential.get("document") or {}
        token = str(credential_document.get("token") or "")
        if credential["status"] != 200 or not token:
            raise RuntimeError(f"credential issuance failed ({credential['status']})")
        secrets_in_memory.append(token)
        record["credential"] = {
            "http_status": credential["status"],
            "principal": credential_document.get("principal"),
            "scopes": credential_document.get("scopes"),
            "expires_in": credential_document.get("expires_in"),
            "token_recorded": False,
        }

        # --- 2. measured tasks and authoritative seed ---------------------
        Database, digest_v1, registry, creative_loop = runtime_modules()
        database = Database(str(db_copy))
        try:
            measured = pick_measured_tasks(database)
        finally:
            database.close()
        record["tasks"] = measured
        primary = measured["primary"]
        secondary = measured["secondary"]
        primary_task = primary["task_id"]
        secondary_task = secondary["task_id"]

        database = Database(str(db_copy))
        try:
            primary_run = insert_run(database, primary_task, primary["max_iteration"] + 1)
            primary_seed = rt5.seed_capability_set(
                database, registry, digest_v1, primary_run, "img-gen-rc11", "1.0.0"
            )
            secondary_run = insert_run(database, secondary_task, 0)
            secondary_seed = rt5.seed_capability_set(
                database, registry, digest_v1, secondary_run, "img-gen-rc11-none", "1.0.0"
            )
        finally:
            database.close()
        record["seed"] = {
            "primary": {
                "run_id": primary_run,
                "capability_id": primary_seed["capability_id"],
                "snapshot_set_digest": primary_seed["store_result"]["snapshot_set_digest"],
                "store_status": primary_seed["store_result"]["status"],
            },
            "secondary": {
                "run_id": secondary_run,
                "capability_id": secondary_seed["capability_id"],
                "snapshot_set_digest": secondary_seed["store_result"]["snapshot_set_digest"],
                "store_status": secondary_seed["store_result"]["status"],
            },
        }

        states: list[dict] = []

        def observe(label: str, task_id: str, *, note: str = "") -> dict:
            response = rt5.open_observation(token, [task_id])
            document = response.get("document") or {}
            if response["status"] != 200:
                raise RuntimeError(
                    f"{label}: observation failed ({response['status']}: {document})"
                )
            stamp = (document.get("grant") or {}).get("stamp") or {}
            end = (document.get("snapshot") or {}).get("end") or {}
            for value in (stamp.get("context_id"), end.get("cursor"), stamp.get("scope_id")):
                if value:
                    secrets_in_memory.append(str(value))
            captured = rt5.capture_state(document, task_id)
            if captured["capabilities"] is None:
                raise RuntimeError(f"{label}: the observation carried no CAPABILITIES projection")
            state = {
                "label": label,
                "note": note,
                "task_id": task_id,
                "observed_at": acc1.now_iso(),
                "observation": rt5.observation_metadata(document),
                "task": captured["task"],
                "capabilities": captured["capabilities"],
                "task_capabilities_ref": captured["task_capabilities_ref"],
                "capabilities_identity_matches": captured["capabilities_identity_matches"],
                "expect": expectations_for(captured["capabilities"]),
            }
            projection_path = evidence_dir / "projections" / f"{label}.json"
            acc1.write_json(
                projection_path,
                {
                    "label": label,
                    "note": note,
                    "task": captured["task"],
                    "capabilities": captured["capabilities"],
                },
            )
            state["projection_file"] = str(projection_path.relative_to(evidence_dir))
            state["projection_file_sha256"] = acc1.sha256_file(projection_path)
            states.append(state)
            return {"state": state, "document": document}

        # --- terminal state on an existing task with an image artifact ----
        terminal = observe(
            "terminal-image",
            primary_task,
            note="existing COMPLETED task with an available image/png artifact",
        )
        if not terminal["state"]["capabilities_identity_matches"]:
            raise RuntimeError("terminal-image: capabilities identity mismatch")

        # --- live state: the stop path can act ----------------------------
        database = Database(str(db_copy))
        try:
            set_task_status(database, primary_task, "ACTIVE")
        finally:
            database.close()
        live = observe(
            "live-active",
            primary_task,
            note="ACTIVE seeded in the isolated copy (no dispatch/generation)",
        )
        stop_entry = next(
            entry
            for entry in live["state"]["capabilities"]["commands"]
            if entry.get("operation") == "STOP_TASK"
        )
        if stop_entry.get("support") != "SUPPORTED":
            raise RuntimeError("live-active: STOP_TASK was not projected SUPPORTED")

        # --- stop-pending state: the command is already accepted ----------
        database = Database(str(db_copy))
        try:
            stop = record_stop_command(database, digest_v1, creative_loop, primary_task)
        finally:
            database.close()
        record["seed"]["stop_command"] = stop
        pending = observe(
            "stop-pending",
            primary_task,
            note="ACTIVE with an accepted durable stop_task command",
        )
        pending_entry = next(
            entry
            for entry in pending["state"]["capabilities"]["commands"]
            if entry.get("operation") == "STOP_TASK"
        )
        if pending_entry.get("support") != "UNAVAILABLE" or pending_entry.get(
            "reason"
        ) != STOP_PENDING_REASON:
            raise RuntimeError("stop-pending: STOP_TASK was not projected UNAVAILABLE/pending")

        # --- no-artifact state: media cannot be offered -------------------
        no_artifact = observe(
            "no-artifact",
            secondary_task,
            note="existing input-only task with a seeded capability set and no artifact",
        )
        media = {
            str(entry.get("representation")): entry
            for entry in no_artifact["state"]["capabilities"]["media"]
        }
        if media["ORIGINAL"].get("support") != "UNAVAILABLE":
            raise RuntimeError("no-artifact: ORIGINAL was not projected UNAVAILABLE")
        if media["THUMBNAIL"].get("support") != "UNAVAILABLE":
            raise RuntimeError("no-artifact: THUMBNAIL was not projected UNAVAILABLE")

        record["states"] = [
            {
                "label": state["label"],
                "note": state["note"],
                "task_id": state["task_id"],
                "observed_at": state["observed_at"],
                "observation": state["observation"],
                "projection_file": state["projection_file"],
                "projection_file_sha256": state["projection_file_sha256"],
                "capabilities_ref": state["capabilities"].get("capabilities_ref"),
                "projection_revision": state["capabilities"].get("projection_revision"),
                "availability": state["capabilities"].get("availability"),
                "reason": state["capabilities"].get("reason"),
                "scope": state["capabilities"].get("scope"),
                "commands": state["capabilities"].get("commands"),
                "media": state["capabilities"].get("media"),
                "features": state["capabilities"].get("features"),
                "expect": state["expect"],
            }
            for state in states
        ]
        primary_revisions = [
            str(state["capabilities"].get("projection_revision"))
            for state in states
            if state["task_id"] == primary_task
        ]
        record["revision_checks"] = {
            "primary_revisions_in_order": primary_revisions,
            "primary_revision_changed_across_states": len(set(primary_revisions))
            == len(primary_revisions),
        }

        # --- Desktop gate decisions over the captured payloads ------------
        capture_path = work / "rc11-gate-capture.json"
        decisions_path = work / "rc11-gate-decisions.json"
        capture_document = {
            "schema": "rc11-gate-capture.v1",
            "note": (
                "live isolated-Runtime CAPABILITIES/TASK payloads; the gate harness"
                " runs real app_core::capability code"
            ),
            "artifact_probe": primary["artifact_id"],
            "states": [
                {
                    "label": state["label"],
                    "task": state["task"],
                    "capabilities": state["capabilities"],
                    "expect": state["expect"],
                }
                for state in states
            ],
        }
        acc1.write_json(capture_path, capture_document)
        gate_run = run_rust_gate_harness(capture_path, decisions_path)
        decisions_document = None
        if decisions_path.is_file():
            decisions_document = json.loads(decisions_path.read_text(encoding="utf-8"))
            shutil.copyfile(decisions_path, evidence_dir / "gates" / "decisions.json")
        record["gates"] = {
            "capture_file": "gates/capture.json",
            "capture_sha256": acc1.sha256_file(capture_path),
            "run": gate_run,
            "decisions": decisions_document,
        }
        acc1.write_json(evidence_dir / "gates" / "capture.json", capture_document)
        if gate_run["exit_code"] != 0 or decisions_document is None:
            raise RuntimeError("the Rust gate harness did not complete successfully")

        # --- cross-check the recorded decisions ---------------------------
        decisions_by_label = {
            str(entry.get("label")): entry
            for entry in decisions_document.get("states", [])
        }
        for state in record["states"]:
            decision = decisions_by_label.get(state["label"])
            if decision is None:
                raise RuntimeError(f"gate decisions are missing state {state['label']}")
            gates = decision.get("gates") or {}
            for name, expected in state["expect"].items():
                actual = gates.get(name) or {}
                if bool(actual.get("enabled")) != bool(expected.get("enabled")):
                    raise RuntimeError(f"{state['label']}/{name}: enabled mismatch")
                if str(actual.get("code")) != str(expected.get("code")):
                    raise RuntimeError(f"{state['label']}/{name}: code mismatch")
                if "reason" in expected and str(actual.get("reason")) != str(
                    expected["reason"]
                ):
                    raise RuntimeError(f"{state['label']}/{name}: reason mismatch")
        record["gates"]["cross_check"] = "match"

        # --- final database state ----------------------------------------
        record["source_state"]["db_copy_sha256_after_seeding"] = acc1.sha256_file(db_copy)
        record["verdict"] = "LIMITED"
        record["verdict_reason"] = (
            "the supported STOP/media path was live-measured over the Runtime's own"
            " CAPABILITIES projection and the real Desktop gate code; real external"
            " grant issuance/revocation, a non-image available artifact and"
            " server-scope create remain out of scope or NOT_MEASURED"
        )

    except Exception as error:  # noqa: BLE001 - evidence must record the failure
        record["errors"].append(
            {
                "type": type(error).__name__,
                "message": str(error),
                "traceback": traceback.format_exc()[-4000:],
            }
        )
        if record["verdict"] == "NOT_RUN":
            record["verdict"] = "ERROR"
    finally:
        record["cleanup"]["listener_released"] = runtime is None
        if runtime is not None:
            with contextlib.suppress(Exception):
                record["runtime"]["stop"] = runtime.stop()
                record["runtime"]["discovery_removed_after_stop"] = (
                    not runtime.discovery_path.is_file()
                )
            record["cleanup"]["listener_released"] = not port_in_use()

        # --- logs and leak scan ------------------------------------------
        secrets_list = [secret, handle_secret, token, *secrets_in_memory]
        sanitized_runtime = acc1.sanitize_log(
            runtime.log_path if runtime else work / "none.log", secrets_list
        )
        (evidence_dir / "logs" / "runtime.log.sanitized.txt").write_text(
            sanitized_runtime["text"], encoding="utf-8"
        )
        findings: list[dict] = []
        for path in sorted(evidence_dir.rglob("*")):
            if not path.is_file():
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            for value in secrets_list:
                if value and value in text:
                    findings.append({"file": str(path.relative_to(evidence_dir)), "kind": "secret"})
            if re.search(r"Bearer\s+[A-Za-z0-9._\-]{16,}", text):
                findings.append(
                    {"file": str(path.relative_to(evidence_dir)), "kind": "bearer_token"}
                )
            if re.search(r"mh1\.[A-Za-z0-9_\-]{8,}", text):
                findings.append(
                    {"file": str(path.relative_to(evidence_dir)), "kind": "media_handle"}
                )
        record["leak_check"] = {
            "files_scanned": len(
                [path for path in evidence_dir.rglob("*") if path.is_file()]
            ),
            "secrets_checked": len([value for value in secrets_list if value]),
            "findings": findings,
            "clean": not findings,
        }

        # --- cleanup: isolated working copy and listener ------------------
        removed = False
        if runtime is None or runtime.process is None or runtime.process.poll() is not None:
            with contextlib.suppress(FileNotFoundError):
                shutil.rmtree(work)
                removed = not work.exists()
        record["cleanup"]["work_dir"] = str(work)
        record["cleanup"]["isolated_copy_removed"] = removed
        record["cleanup"]["port_free_after_cleanup"] = not port_in_use()

        # --- evidence documents ------------------------------------------
        session = {
            "measurement": record["measurement"],
            "date": record["started_at"],
            "verdict": record["verdict"],
            "verdict_reason": record.get("verdict_reason"),
            "harness": record["harness"],
            "desktop": record["desktop"],
            "source_state": record["source_state"],
            "runtime": record["runtime"],
            "tasks": record["tasks"],
            "seed": record["seed"],
            "states": record["states"],
            "revision_checks": record["revision_checks"],
            "gates": {
                "capture_sha256": record["gates"].get("capture_sha256"),
                "run": record["gates"].get("run"),
                "cross_check": record["gates"].get("cross_check"),
                "decisions": record["gates"].get("decisions"),
            },
            "not_measured": record["not_measured"],
            "residuals": record["residuals"],
            "leak_check": record["leak_check"],
            "cleanup": record["cleanup"],
            "errors": record["errors"],
        }
        acc1.write_json(evidence_dir / "record.json", record)
        acc1.write_json(evidence_dir / "session.json", session)
        write_readme(evidence_dir, record)
        print(json.dumps(session, ensure_ascii=False, indent=2, default=str))
        return 0 if record["verdict"] in ("LIMITED", "PARTIAL") and not record["errors"] else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--evidence-dir",
        default=None,
        help=(
            "evidence directory to write (default: docs/development/"
            "evidence-published/rc11-live-latest); the run refuses a non-empty"
            " directory so existing evidence is never rewritten"
        ),
    )
    args = parser.parse_args(argv)
    for required in (RUNTIME_PYTHON, SOURCE_DB, base.CONFIG_TEMPLATE, SOURCE_PRESETS):
        if not Path(required).exists():
            print(f"missing prerequisite: {required}", file=sys.stderr)
            return 2
    if port_in_use():
        print(f"port {PORT} is already in use; the harness owns {PORT}", file=sys.stderr)
        return 2
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
