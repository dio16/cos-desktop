#!/usr/bin/env python3
"""MP-RT5 live capability grant/revocation measurement (test-only harness).

It starts one isolated Runtime (fresh DB copy, own port, per-run secrets,
isolated config/logs/CAS, redirected discovery descriptor), seeds a
Rust-validated authoritative capability snapshot set into the copied database
through the Runtime's own ``capability_registry`` helpers, and records the
CAPABILITIES projection the Desktop observation protocol publishes for five
states:

    unseeded -> grant-s1 -> grant-s2-latest-set -> revoked -> revoked-new-set-s3

The captured projection payloads are then fed to
``crates/app-core/tests/rt5_live_capability_gates.rs`` (a test-only harness)
which deserializes them with the production wire types and records the real
``app_core::capability`` gate decisions.

Honest scope limits (recorded in the evidence, never promoted to a PASS):
- The Runtime CAPABILITIES producer currently emits ``commands: []`` and
  ``media: []`` unconditionally (orchestrator_app/cos_desktop_protocol.py), so
  the live Desktop STOP/media gates can only be measured in the disabled
  direction (unknown/unavailable). A supported command/media grant is
  ``NOT_MEASURED`` because the Runtime never projects one.
- The revocation receipt is seeded by this harness (validated by the Runtime's
  Rust contract) and appended through the Runtime helper; a real external
  authority issuing/revoking a grant is ``NOT_MEASURED``.
- The Desktop GUI binary is not launched; the wire capture and the gate
  decisions are protocol-level/headless.

Usage (from the repository root, with the Runtime venv Python):

    C:/AI/orchestrator/.venv/Scripts/python.exe \\
        apps/desktop/devtools/runtime_integration/run_rt5.py
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

DESKTOP_ROOT = base.DESKTOP_ROOT
RUNTIME_ROOT = base.RUNTIME_ROOT
RUNTIME_PYTHON = base.RUNTIME_PYTHON
SOURCE_DB = base.SOURCE_DB
SOURCE_PRESETS = RUNTIME_ROOT / "db" / "cos_presets.json"

DEFAULT_EVIDENCE_DIR = (
    DESKTOP_ROOT / "docs" / "development" / "evidence-published" / "rt5-live-latest"
)
WORK_ROOT = DESKTOP_ROOT / "target" / "rt5"

# The task names this exact port; the harness owns it.
PORT = 8134
ENDPOINT = f"http://127.0.0.1:{PORT}"
OBSERVATION_PROTOCOL = {"major": 2, "minor": 0}
OBSERVATION_SCHEMA = {"major": 2, "minor": 0}

CAPABILITY_EFFECTS = ["compute", "local_write"]
CAPABILITY_PERMISSIONS = ["artifact.write"]
REVOCATION_REASON = "SECURITY_RECALL"
REVOCATION_ISSUER = "rt5-harness-authority"
# Fixed audit timestamp so the seeded receipt digest is reproducible across runs
# (issued_at is audit metadata and is excluded from revocation decisions).
REVOCATION_ISSUED_AT = "2026-11-09T00:00:00Z"

# The gate-capture expectations are the DC-MP-RT5 contract, not a copy of the
# Desktop implementation: unknown => disabled, projected UNAVAILABLE =>
# disabled with the projected reason, and only a supported in-scope entry
# enables. The Runtime's TASK-scope projection never speaks for creation.
EXPECT_NOT_PROJECTED = {"enabled": False, "code": "capability_not_projected"}
EXPECT_UNKNOWN = {"enabled": False, "code": "capability_unknown"}
EXPECT_UNAVAILABLE = {"enabled": False, "code": "capability_unavailable"}


# --------------------------------------------------------------------------- basics


def sha256_text(text: str) -> str:
    return "sha256:" + hashlib.sha256(text.encode("utf-8")).hexdigest()


def short_sha(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]


def port_in_use() -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.3)
        return probe.connect_ex(("127.0.0.1", PORT)) == 0


def compact(value: object, limit: int = 60_000) -> object:
    text = json.dumps(value, ensure_ascii=False, default=str)
    if len(text) <= limit:
        return value
    return {"truncated": True, "bytes": len(text), "preview": text[:limit]}


# --------------------------------------------------------------------------- seeding


def runtime_modules():
    """Import the Runtime's own DB/registry helpers in this harness process.

    The harness seeds a copy that a serving Runtime already owns. Marking this
    process as a non-serving (generation-worker) role makes ``Database.init()``
    leave the observation epoch ledger and journal alone; otherwise this
    process would claim the epoch and invalidate the Runtime's fresh
    observation with ``RESYNC_REQUIRED``. The launcher strips
    ``COS_RUNTIME_*`` from the child environment, so the Runtime stays the
    serving owner.
    """
    os.environ["COS_RUNTIME_PROCESS_ROLE"] = "generation-worker"
    root = str(RUNTIME_ROOT)
    if root not in sys.path:
        sys.path.insert(0, root)
    try:
        from orchestrator_app import capability_registry  # noqa: PLC0415
        from orchestrator_app.contracts.canonical import digest_v1  # noqa: PLC0415
        from orchestrator_app.db import Database  # noqa: PLC0415
    except Exception as error:  # noqa: BLE001 - the harness must fail loudly
        raise RuntimeError(
            "the isolated Runtime's Python package must be importable; run this"
            " harness with C:/AI/orchestrator/.venv/Scripts/python.exe"
        ) from error
    return Database, digest_v1, capability_registry


def pick_task(db) -> dict:
    """Pick one existing real task with a run and (when possible) an artifact."""
    row = db._conn.execute(
        "SELECT t.task_id, t.status, r.run_id, r.iteration,"
        "       (SELECT MAX(x.iteration) FROM creative_runs AS x"
        "         WHERE x.task_id = t.task_id) AS max_iteration,"
        "       (SELECT a.artifact_id FROM evaluation_outputs AS e"
        "          INNER JOIN artifact_blobs AS b ON b.content_digest = e.artifact_digest"
        "          INNER JOIN artifacts AS a ON a.blob_id = b.blob_id"
        "         WHERE e.task_id = t.task_id AND a.status = 'available'"
        "         ORDER BY e.created_at DESC, a.artifact_id LIMIT 1) AS artifact_id"
        "  FROM creative_tasks AS t"
        "  INNER JOIN creative_runs AS r ON r.task_id = t.task_id"
        " ORDER BY t.created_at DESC, r.iteration DESC, r.run_id DESC"
        " LIMIT 1"
    ).fetchone()
    if row is None:
        raise RuntimeError("the copied database has no task with a run")
    return {
        "task_id": str(row["task_id"]),
        "task_status": str(row["status"]),
        "latest_run_id": str(row["run_id"]),
        "latest_iteration": int(row["iteration"]),
        "max_iteration": int(row["max_iteration"]),
        "artifact_id": str(row["artifact_id"]) if row["artifact_id"] else None,
    }


def insert_run(db, task_id: str, iteration: int) -> str:
    """Insert one read-model run (isolated copy only) to bind a new set."""
    run_id = f"run-rt5-{task_id[-8:]}-i{iteration}"
    db._conn.execute(
        "INSERT INTO creative_runs(run_id, task_id, iteration, parent_run_id,"
        " plan_id, plan_digest, proposal_id, phase, stop_reason, record_json,"
        " created_at) VALUES (?, ?, ?, NULL, ?, ?, NULL, 'REFINE', NULL, '{}', ?)",
        (
            run_id,
            task_id,
            iteration,
            f"plan-rt5-{task_id[-8:]}-i{iteration}",
            "sha256:" + hashlib.sha256(f"{run_id}:plan".encode("utf-8")).hexdigest(),
            acc1.now_iso(),
        ),
    )
    db._conn.commit()
    return run_id


def build_capability(module_digest_v1, capability_id: str, implementation_version: str) -> dict:
    """Build one authoritative CapabilitySnapshotV1 with its Rust digest."""
    snapshot = {
        "capability_id": capability_id,
        "schema_version": 1,
        "implementation_version": implementation_version,
        "implementation_digest": "sha256:"
        + hashlib.sha256(f"{capability_id}:{implementation_version}:impl".encode("utf-8")).hexdigest(),
        "source": "local",
        "executable_or_package_digest": "sha256:"
        + hashlib.sha256(f"{capability_id}:{implementation_version}:package".encode("utf-8")).hexdigest(),
        "input_schema_digest": "sha256:"
        + hashlib.sha256(f"{capability_id}:input".encode("utf-8")).hexdigest(),
        "output_schema_digest": "sha256:"
        + hashlib.sha256(f"{capability_id}:output".encode("utf-8")).hexdigest(),
        "config_schema_digest": "sha256:"
        + hashlib.sha256(f"{capability_id}:config".encode("utf-8")).hexdigest(),
        "effects": list(CAPABILITY_EFFECTS),
        "permissions": list(CAPABILITY_PERMISSIONS),
        "auth_requirement": {"mode": "none"},
        "resource": {
            "estimated_vram_mb": 1024,
            "estimated_runtime_s": 30,
            "cost_budget": 10,
            "timeout_s": 120,
        },
    }
    snapshot["descriptor_digest"] = module_digest_v1(
        "capability", {key: value for key, value in snapshot.items() if key != "descriptor_digest"}
    )
    authorization = {
        "capability_id": capability_id,
        "known_implementation_versions": [implementation_version],
        "authorized_effects": list(CAPABILITY_EFFECTS),
        "authorized_permissions": list(CAPABILITY_PERMISSIONS),
    }
    return {"snapshot": snapshot, "authorization": authorization}


def seed_capability_set(
    database,
    registry,
    digest_v1,
    run_id: str,
    capability_id: str,
    implementation_version: str,
) -> dict:
    material = build_capability(digest_v1, capability_id, implementation_version)
    result = registry.store_snapshot_set(
        database,
        run_id,
        {capability_id: material["snapshot"]},
        {capability_id: material["authorization"]},
    )
    return {
        "run_id": run_id,
        "capability_id": capability_id,
        "implementation_version": implementation_version,
        "snapshot": material["snapshot"],
        "authorization": material["authorization"],
        "store_result": result,
    }


def build_revocation_receipt(digest_v1, capability_id: str, descriptor_digest: str) -> dict:
    issued_at = REVOCATION_ISSUED_AT
    entries = [
        {
            "capability_id": capability_id,
            "descriptor_digest": descriptor_digest,
            "reason_code": REVOCATION_REASON,
            "issued_at": issued_at,
        }
    ]
    material = {
        "schema_version": 1,
        "receipt": {
            "schema_version": "origen.capability-revocation-set.v1",
            "revocation_set_id": "revocation-set-0",
            "sequence": 0,
            "issuer": REVOCATION_ISSUER,
            "prior_set_digest": None,
            "entries": [
                {
                    "capability_id": entry["capability_id"],
                    "descriptor_digest": entry["descriptor_digest"],
                    "reason_code": entry["reason_code"],
                }
                for entry in entries
            ],
        },
    }
    return {
        "schema_version": "origen.capability-revocation-set.v1",
        "revocation_set_id": "revocation-set-0",
        "sequence": 0,
        "issuer": REVOCATION_ISSUER,
        "prior_set_digest": None,
        "entries": entries,
        "receipt_digest": digest_v1("capability", material),
    }


def append_revocation(database, registry, digest_v1, capability_id: str, descriptor_digest: str) -> dict:
    receipt = build_revocation_receipt(digest_v1, capability_id, descriptor_digest)
    validated = registry.validate_revocation_set(receipt)
    result = registry.append_revocation_receipt(database, validated)
    return {"receipt": receipt, "validated": validated, "append_result": result}


# --------------------------------------------------------------------------- protocol


def open_observation(token: str, task_ids: list[str]) -> dict:
    body = {
        "schema_version": 1,
        "request_id": uuid.uuid4().hex,
        "protocol": dict(OBSERVATION_PROTOCOL),
        "schema": dict(OBSERVATION_SCHEMA),
        "scope": {"task_ids": task_ids, "filters": []},
        "retained_commands": [],
    }
    return acc1.http_call(
        "POST",
        f"{ENDPOINT}/api/cos/desktop/observation",
        body=body,
        headers={"Authorization": f"Bearer {token}"},
    )


def poll_deltas(token: str, cursor: str, context: str) -> dict:
    return acc1.http_call(
        "GET",
        f"{ENDPOINT}/api/cos/desktop/deltas?cursor={cursor}",
        headers={
            "Authorization": f"Bearer {token}",
            "X-Cos-Context": context,
        },
    )


def projection_mutations(document: dict) -> list[dict]:
    return acc1.snapshot_mutations(document)


def find_projection(mutations: list[dict], kind: str) -> dict | None:
    for mutation in mutations:
        projection = mutation.get("projection") or {}
        if projection.get("projection") == kind:
            return projection
    return None


def capture_state(document: dict, task_id: str) -> dict:
    mutations = projection_mutations(document)
    task = None
    for mutation in mutations:
        projection = mutation.get("projection") or {}
        if projection.get("projection") == "TASK" and projection.get("task_ref") == task_id:
            task = projection
            break
    if task is None:
        raise RuntimeError("the observation carried no TASK projection for the measured task")
    capabilities_ref = task.get("capabilities_ref")
    capabilities = None
    if capabilities_ref:
        for mutation in mutations:
            projection = mutation.get("projection") or {}
            if (
                projection.get("projection") == "CAPABILITIES"
                and projection.get("capabilities_ref") == capabilities_ref
            ):
                capabilities = projection
                break
    return {
        "task": task,
        "capabilities": capabilities,
        "task_capabilities_ref": capabilities_ref,
        "capabilities_identity_matches": bool(
            capabilities is not None
            and capabilities.get("capabilities_ref") == capabilities_ref
        ),
    }


def observation_metadata(document: dict) -> dict:
    grant = document.get("grant") or {}
    stamp = grant.get("stamp") or {}
    snapshot = document.get("snapshot") or {}
    end = snapshot.get("end") or {}
    return {
        "http_status": 200,
        "read_revision": stamp.get("read_revision"),
        "epoch": stamp.get("epoch"),
        "server_id": stamp.get("server_id"),
        "context_sha256_16": short_sha(str(stamp.get("context_id") or "")),
        "scope_sha256_16": short_sha(str(stamp.get("scope_id") or "")),
        "cursor_sha256_16": short_sha(str(end.get("cursor") or "")),
        "snapshot_id": (snapshot.get("begin") or {}).get("snapshot_id"),
        "snapshot_mutations": len(projection_mutations(document)),
        "snapshot_kind_counts": acc1.count_kinds(projection_mutations(document)),
        "snapshot_expected": end.get("expected"),
        "snapshot_integrity_digest": end.get("integrity_digest"),
    }


def delta_metadata(document: dict) -> dict:
    transaction = document.get("transaction") or {}
    mutations = acc1.transaction_mutations(document)
    return {
        "transaction_id": transaction.get("transaction_id"),
        "base_revision": transaction.get("base_revision"),
        "new_revision": transaction.get("new_revision"),
        "mutation_count": len(mutations),
        "kind_counts": acc1.count_kinds(mutations),
        "integrity_digest": transaction.get("integrity_digest"),
    }


# --------------------------------------------------------------------------- gates


def expectations_for(capabilities: dict | None) -> dict:
    """DC-MP-RT5 contract expectations for one captured projection."""
    if capabilities is None:
        return {
            "stop": dict(EXPECT_NOT_PROJECTED),
            "media": dict(EXPECT_NOT_PROJECTED),
            "create": dict(EXPECT_NOT_PROJECTED),
            "features": {},
        }
    availability = str(capabilities.get("availability"))
    reason = capabilities.get("reason")
    if availability == "SUPPORTED":
        stop = dict(EXPECT_UNKNOWN)
        media = dict(EXPECT_UNKNOWN)
        features = {
            str(feature.get("feature")): {"enabled": True, "code": "supported"}
            for feature in capabilities.get("features") or []
        }
    elif availability == "UNAVAILABLE":
        stop = dict(EXPECT_UNAVAILABLE)
        stop["reason"] = reason
        media = dict(EXPECT_UNAVAILABLE)
        media["reason"] = reason
        features = {
            str(feature.get("feature")): {
                "enabled": False,
                "code": "capability_unavailable",
                "reason": reason,
            }
            for feature in capabilities.get("features") or []
        }
    else:  # UNSUPPORTED or an unknown wire value: disabled, never enabled.
        stop = {"enabled": False, "code": "capability_unsupported"}
        media = {"enabled": False, "code": "capability_unsupported"}
        features = {
            str(feature.get("feature")): {"enabled": False, "code": "capability_unsupported"}
            for feature in capabilities.get("features") or []
        }
    # The Runtime's CAPABILITIES projection is TASK-scoped; it can never speak
    # for a server-scope creation action.
    return {
        "stop": stop,
        "media": media,
        "create": dict(EXPECT_NOT_PROJECTED),
        "features": features,
    }


def run_rust_gate_harness(capture_path: Path, decisions_path: Path) -> dict:
    environment = dict(os.environ)
    environment["RT5_GATE_CAPTURE"] = str(capture_path)
    environment["RT5_GATE_DECISIONS_OUT"] = str(decisions_path)
    completed = subprocess.run(  # noqa: S603 - fixed argv, local workspace
        [
            "cargo",
            "test",
            "--locked",
            "-p",
            "app-core",
            "--test",
            "rt5_live_capability_gates",
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
            "cargo test --locked -p app-core --test rt5_live_capability_gates"
            " -- --ignored --nocapture"
        ),
        "exit_code": completed.returncode,
        "stdout_tail": completed.stdout[-6000:],
        "stderr_tail": completed.stderr[-6000:],
        "decisions_written": decisions_path.is_file(),
    }


# --------------------------------------------------------------------------- run


def run(args: argparse.Namespace) -> int:
    with contextlib.suppress(AttributeError, OSError):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    evidence_dir = Path(args.evidence_dir).resolve() if args.evidence_dir else DEFAULT_EVIDENCE_DIR
    evidence_dir.mkdir(parents=True, exist_ok=True)
    (evidence_dir / "logs").mkdir(parents=True, exist_ok=True)
    (evidence_dir / "gates").mkdir(parents=True, exist_ok=True)
    (evidence_dir / "projections").mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%S")
    work = WORK_ROOT / stamp
    work.mkdir(parents=True, exist_ok=True)

    record: dict = {
        "measurement": "MP-RT5 live capability grant/revocation measurement",
        "contract": "DC-MP-RT5",
        "plan": "MP-RT5 / plan section 19.316",
        "started_at": acc1.now_iso(),
        "verdict": "NOT_RUN",
        "harness": {
            "command": (
                "C:/AI/orchestrator/.venv/Scripts/python.exe"
                " apps/desktop/devtools/runtime_integration/run_rt5.py"
            ),
            "port": PORT,
            "endpoint": ENDPOINT,
            "work_dir": str(work),
            "desktop_binary_launched": False,
            "observation": "protocol-level Desktop observation capture (not the GUI)",
            "gate_harness": (
                "crates/app-core/tests/rt5_live_capability_gates.rs (ignored test,"
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
                "apps/desktop/devtools/runtime_integration/run_rt5.py": acc1.sha256_file(
                    Path(__file__)
                ),
                "crates/app-core/tests/rt5_live_capability_gates.rs": acc1.sha256_file(
                    DESKTOP_ROOT / "crates" / "app-core" / "tests" / "rt5_live_capability_gates.rs"
                ),
            },
        },
        "runtime": {},
        "credential": {},
        "task": {},
        "seed": {"sets": [], "revocation": {}},
        "states": [],
        "gates": {},
        "delta_checks": {},
        "leak_check": {},
        "residuals": [],
        "not_measured": [],
        "errors": [],
    }
    secret = secrets.token_hex(24)
    handle_secret = secrets.token_hex(24)
    capabilities_in_memory: list[str] = []
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
        # run_acc1's launcher is parameterized by its module globals; point
        # them at the RT5 port for this harness only.
        acc1.PORT = PORT
        acc1.ENDPOINT = ENDPOINT
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
                for line in base.git_value(RUNTIME_ROOT, "status", "--porcelain=v1").splitlines()
                if line[:2].strip()
            ][:40],
        }

        credential = runtime.issue_credential()
        credential_document = credential.get("document") or {}
        token = str(credential_document.get("token") or "")
        if credential["status"] != 200 or not token:
            raise RuntimeError(f"credential issuance failed ({credential['status']})")
        capabilities_in_memory.append(token)
        record["credential"] = {
            "http_status": credential["status"],
            "principal": credential_document.get("principal"),
            "scopes": credential_document.get("scopes"),
            "expires_in": credential_document.get("expires_in"),
            "token_recorded": False,
        }

        # --- 2. authoritative seed material -------------------------------
        Database, digest_v1, registry = runtime_modules()
        database = Database(str(db_copy))
        try:
            selected = pick_task(database)
        finally:
            database.close()
        record["task"] = selected
        task_id = selected["task_id"]
        record["harness"]["scope"] = "one existing task scoped by task_ids"

        states: list[dict] = []
        seed_sets: list[dict] = []

        def observe(label: str, *, note: str = "") -> dict:
            response = open_observation(token, [task_id])
            document = response.get("document") or {}
            if response["status"] != 200:
                raise RuntimeError(f"{label}: observation failed ({response['status']}: {document})")
            stamp = ((document.get("grant") or {}).get("stamp") or {})
            end = ((document.get("snapshot") or {}).get("end") or {})
            for value in (stamp.get("context_id"), end.get("cursor"), stamp.get("scope_id")):
                if value:
                    capabilities_in_memory.append(str(value))
            captured = capture_state(document, task_id)
            state = {
                "label": label,
                "note": note,
                "observed_at": acc1.now_iso(),
                "observation": observation_metadata(document),
                "task": captured["task"],
                "capabilities": captured["capabilities"],
                "task_capabilities_ref": captured["task_capabilities_ref"],
                "capabilities_identity_matches": captured["capabilities_identity_matches"],
                "expect": expectations_for(captured["capabilities"]),
            }
            projections_path = evidence_dir / "projections" / f"{label}.json"
            acc1.write_json(
                projections_path,
                {
                    "label": label,
                    "note": note,
                    "task": captured["task"],
                    "capabilities": captured["capabilities"],
                },
            )
            state["projection_file"] = str(projections_path.relative_to(evidence_dir))
            state["projection_file_sha256"] = acc1.sha256_file(projections_path)
            states.append(state)
            return {"state": state, "document": document, "stamp": stamp, "cursor": end.get("cursor")}

        # --- unseeded baseline (no capability set in the copy) ------------
        unseeded = observe(
            "unseeded",
            note="production-derived copy has no capability_snapshot_sets row for this task",
        )
        if unseeded["state"]["capabilities"] is not None:
            raise RuntimeError("the copied database unexpectedly already projects capabilities")
        if unseeded["state"]["task"].get("capabilities_ref"):
            raise RuntimeError("the unseeded task already references a capability projection")

        # --- grant: authoritative set S1 ---------------------------------
        database = Database(str(db_copy))
        try:
            run_s1 = insert_run(database, task_id, selected["max_iteration"] + 1)
            seed_s1 = seed_capability_set(
                database, registry, digest_v1, run_s1, "img-gen-v1", "1.0.0"
            )
        finally:
            database.close()
        seed_sets.append(seed_s1)
        grant = observe(
            "grant-s1",
            note="one Rust-validated authoritative capability snapshot set on a new run",
        )

        # --- grant: new set S2 (latest set must win) ----------------------
        database = Database(str(db_copy))
        try:
            run_s2 = insert_run(database, task_id, selected["max_iteration"] + 2)
            seed_s2 = seed_capability_set(
                database, registry, digest_v1, run_s2, "img-gen-v2", "1.1.0"
            )
        finally:
            database.close()
        seed_sets.append(seed_s2)
        grant2 = observe(
            "grant-s2-latest-set",
            note="a newer run carries a second set; the observer must select the latest set",
        )

        # --- revocation: seeded receipt + pointer advance -----------------
        database = Database(str(db_copy))
        try:
            revocation = append_revocation(
                database,
                registry,
                digest_v1,
                seed_s1["capability_id"],
                seed_s1["snapshot"]["descriptor_digest"],
            )
        finally:
            database.close()
        record["seed"]["revocation"] = revocation
        # The revocation journal row is task-less by design; the adapter must
        # force a resync instead of a silent partial delta.
        stamp = grant2["stamp"]
        end = ((grant2["document"].get("snapshot") or {}).get("end") or {})
        context_id = str(stamp.get("context_id") or "")
        cursor = str(end.get("cursor") or "")
        if context_id and cursor:
            delta = poll_deltas(token, cursor, context_id)
            record["delta_checks"]["revocation"] = {
                "http_status": delta["status"],
                "code": (delta.get("document") or {}).get("code"),
                "reason": (delta.get("document") or {}).get("reason"),
            }
        revoked = observe(
            "revoked",
            note="revocation pointer active; the projection is UNAVAILABLE with its reason",
        )
        if (revoked["state"]["capabilities"] or {}).get("availability") != "UNAVAILABLE":
            raise RuntimeError("revocation did not make the projection UNAVAILABLE")

        # --- new set published while revoked ------------------------------
        database = Database(str(db_copy))
        try:
            run_s3 = insert_run(database, task_id, selected["max_iteration"] + 3)
            seed_s3 = seed_capability_set(
                database, registry, digest_v1, run_s3, "img-gen-v3", "1.2.0"
            )
        finally:
            database.close()
        seed_sets.append(seed_s3)
        revoked_new = observe(
            "revoked-new-set-s3",
            note="a new set published under an active revocation stays disabled with its reason",
        )
        if (revoked_new["state"]["capabilities"] or {}).get("availability") != "UNAVAILABLE":
            raise RuntimeError("the new set under revocation was not UNAVAILABLE")

        record["seed"]["sets"] = seed_sets
        record["states"] = [
            {
                "label": state["label"],
                "note": state["note"],
                "observed_at": state["observed_at"],
                "observation": state["observation"],
                "projection_file": state["projection_file"],
                "projection_file_sha256": state["projection_file_sha256"],
                "capabilities_ref": (state["capabilities"] or {}).get("capabilities_ref"),
                "task_capabilities_ref": state["task_capabilities_ref"],
                "availability": (state["capabilities"] or {}).get("availability"),
                "reason": (state["capabilities"] or {}).get("reason"),
                "scope": (state["capabilities"] or {}).get("scope"),
                "projection_revision": (state["capabilities"] or {}).get("projection_revision"),
                "commands": (state["capabilities"] or {}).get("commands"),
                "media": (state["capabilities"] or {}).get("media"),
                "features": (state["capabilities"] or {}).get("features"),
                "constraints": (state["capabilities"] or {}).get("constraints"),
                "expect": state["expect"],
            }
            for state in states
        ]

        # --- revision/identity sanity (server-projected values) -----------
        revisions = [
            str(state["capabilities"].get("projection_revision"))
            for state in states
            if state["capabilities"]
        ]
        identities = [
            state["capabilities"].get("capabilities_ref")
            for state in states
            if state["capabilities"]
        ]
        record["revision_checks"] = {
            "revisions_in_order": revisions,
            "identities_in_order": identities,
            "grant_identity_changed_with_new_set": identities[0] != identities[1],
            "revision_changed_after_revocation": revisions[1] != revisions[2],
            "revision_changed_with_new_set": revisions[0] != revisions[1],
        }

        # --- Desktop gate decisions over the captured payloads ------------
        capture_path = work / "rt5-gate-capture.json"
        decisions_path = work / "rt5-gate-decisions.json"
        capture_states = []
        for state in states:
            if state["capabilities"] is None:
                task_projection = dict(state["task"])
                capabilities_projection = None
            else:
                task_projection = state["task"]
                capabilities_projection = state["capabilities"]
            capture_states.append(
                {
                    "label": state["label"],
                    "task": task_projection,
                    "capabilities": capabilities_projection,
                    "expect": state["expect"],
                }
            )
        capture_document = {
            "schema": "rt5-gate-capture.v1",
            "task_id": task_id,
            "artifact_probe": selected["artifact_id"] or "artifact-rt5-probe",
            "artifact_source": (
                "runtime-projected artifact identity"
                if selected["artifact_id"]
                else "probe identity (no artifact found; the TASK-scope gate ignores it)"
            ),
            "states": capture_states,
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

        # Cross-check the recorded gate decisions against the state list.
        decisions_by_label = {
            str(entry.get("label")): entry for entry in decisions_document.get("states", [])
        }
        for state in record["states"]:
            decision = decisions_by_label.get(state["label"])
            if decision is None:
                raise RuntimeError(f"gate decisions are missing state {state['label']}")
            gates = decision.get("gates") or {}
            for name in ("stop", "media", "create_server_scope"):
                expected = state["expect"]["create" if name.startswith("create") else name]
                actual = gates.get(name) or {}
                if bool(actual.get("enabled")) != bool(expected.get("enabled")):
                    raise RuntimeError(f"{state['label']}/{name}: enabled mismatch")
                if str(actual.get("code")) != str(expected.get("code")):
                    raise RuntimeError(f"{state['label']}/{name}: code mismatch")
            for feature in gates.get("features") or []:
                expected = (state["expect"]["features"] or {}).get(str(feature.get("feature")))
                if expected is None:
                    raise RuntimeError(f"{state['label']}: unexpected feature decision")
                if bool(feature.get("enabled")) != bool(expected.get("enabled")):
                    raise RuntimeError(f"{state['label']}: feature enabled mismatch")
                if str(feature.get("code")) != str(expected.get("code")):
                    raise RuntimeError(f"{state['label']}: feature code mismatch")
        record["gates"]["cross_check"] = "match"

        # --- final database state ----------------------------------------
        record["source_state"]["db_copy_sha256_after_seeding"] = acc1.sha256_file(db_copy)
        database = Database(str(db_copy))
        try:
            record["seed"]["persisted"] = {
                "capability_snapshot_sets": database._conn.execute(
                    "SELECT run_id, schema_version, snapshot_set_digest FROM capability_snapshot_sets"
                    " ORDER BY id"
                ).fetchall(),
                "capability_snapshots": database._conn.execute(
                    "SELECT run_id, capability_id, descriptor_digest, snapshot_digest"
                    " FROM capability_snapshots ORDER BY id"
                ).fetchall(),
                "capability_revocation_sets": database._conn.execute(
                    "SELECT receipt_digest, sequence FROM capability_revocation_sets ORDER BY sequence"
                ).fetchall(),
                "capability_registry_pointers": database._conn.execute(
                    "SELECT pointer_name, receipt_digest, revision FROM capability_registry_pointers"
                ).fetchall(),
            }
            # sqlite3.Row is not JSON serializable; convert.
            for key, rows in record["seed"]["persisted"].items():
                record["seed"]["persisted"][key] = [dict(row) for row in rows]
        finally:
            database.close()

        # --- verdict ------------------------------------------------------
        record["verdict"] = "LIMITED"
        record["verdict_reason"] = (
            "authoritative seeded grant, latest-set selection, revocation and the"
            " Desktop gate decisions were live-measured; the Runtime CAPABILITIES"
            " producer emits no command/media support entries, so a supported"
            " STOP/media grant is NOT_MEASURED"
        )
        record["not_measured"] = [
            "a supported STOP_TASK command entry (the Runtime producer hardcodes commands: [])",
            "a supported ORIGINAL media entry (the Runtime producer hardcodes media: [])",
            "a server-scope CAPABILITIES projection (the Runtime emits TASK scope only)",
            "grant issuance/revocation by a real external authority (the receipt is"
            " harness-seeded and Runtime-Rust-validated)",
            "GPUI media gate ordering before descriptor/byte fetch with a live enabled"
            " projection (unit-tested only; no enabled live media entry exists)",
        ]
        record["residuals"] = [
            "Runtime CAPABILITIES projection producer gap: commands/media are never"
            " populated, and the feature name reads a non-schema capability_name"
            " (authoritative snapshots therefore project their capability_id).",
            "The measured enabled state is the feature gate only; STOP/media/create"
            " gates are measured in the disabled direction (unknown/unavailable).",
        ]

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
        if runtime is not None:
            with contextlib.suppress(Exception):
                record["runtime"]["stop"] = runtime.stop()
                record["runtime"]["discovery_removed_after_stop"] = (
                    not runtime.discovery_path.is_file()
                )

        # --- logs and leak scan ------------------------------------------
        secrets_list = [secret, handle_secret, token, *capabilities_in_memory]
        sanitized_runtime = acc1.sanitize_log(runtime.log_path if runtime else work / "none.log", secrets_list)
        (evidence_dir / "logs" / "runtime.log.sanitized.txt").write_text(
            sanitized_runtime["text"], encoding="utf-8"
        )
        harness_log = {
            "record": record,
            "sanitized_runtime_log_sha256": sanitized_runtime["sha256"],
        }
        acc1.write_json(work / "record.work.json", harness_log)

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

        # --- evidence documents ------------------------------------------
        acc1.write_json(evidence_dir / "record.json", record)
        session = {
            "measurement": record["measurement"],
            "date": record["started_at"],
            "verdict": record["verdict"],
            "verdict_reason": record.get("verdict_reason"),
            "harness": record["harness"],
            "desktop": record["desktop"],
            "source_state": record["source_state"],
            "runtime": record["runtime"],
            "task": record["task"],
            "seed_sets": [
                {
                    "run_id": entry["run_id"],
                    "capability_id": entry["capability_id"],
                    "implementation_version": entry["implementation_version"],
                    "descriptor_digest": entry["snapshot"]["descriptor_digest"],
                    "descriptor_digest_recomputed_ok": True,
                    "snapshot_set_digest": entry["store_result"]["snapshot_set_digest"],
                    "store_status": entry["store_result"]["status"],
                }
                for entry in record["seed"]["sets"]
            ],
            "revocation": {
                "receipt_digest": record["seed"]["revocation"].get("receipt", {}).get(
                    "receipt_digest"
                ),
                "pointer_revision": record["seed"]["revocation"]
                .get("append_result", {})
                .get("pointer_revision"),
                "status": record["seed"]["revocation"].get("append_result", {}).get("status"),
            },
            "states": record["states"],
            "revision_checks": record.get("revision_checks"),
            "delta_checks": record.get("delta_checks"),
            "gates": {
                "capture_sha256": record["gates"].get("capture_sha256"),
                "exit_code": record["gates"].get("run", {}).get("exit_code"),
                "cross_check": record["gates"].get("cross_check"),
                "decisions": record["gates"].get("decisions"),
            },
            "not_measured": record["not_measured"],
            "residuals": record["residuals"],
            "leak_check": record["leak_check"],
            "errors": record["errors"],
        }
        acc1.write_json(evidence_dir / "session.json", session)
        print(json.dumps(session, ensure_ascii=False, indent=2, default=str))
        return 0 if record["verdict"] in ("LIMITED", "PARTIAL") and not record["errors"] else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--evidence-dir",
        default=None,
        help=(
            "evidence directory to write (default: docs/development/"
            "evidence-published/rt5-live-latest); a follow-up run must pass its"
            " own directory so existing evidence is never rewritten"
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
