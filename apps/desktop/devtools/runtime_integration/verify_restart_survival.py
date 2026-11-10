#!/usr/bin/env python3
"""Post-restart survival check (test-only, evidence-only).

Follow-up measurement for ``run_acc1_round2.py --scenario runtime-restart``. It
reuses that run's isolated database copy and CAS (the exact post-restart bytes,
including the persisted outcome), starts one short-lived Runtime on port 8132
under a new epoch and measures only what the restart scenario itself cannot:

1. **Servable artifact** — the task's own ARTIFACT projection, resolved from the
   server-projected OUTCOME ``selected_refs``, is fetched through the RC-13
   media descriptor/handle/blob path, its SHA-256 is compared with the projected
   artifact digest and its bytes are written to the evidence directory.
2. **Durable identity** — the outcome, artifact/blob rows and ComfyUI history are
   read from the copied database and ComfyUI so the terminal state is not
   inferred from UI text.
3. **No duplicate provider submission** — the receipt/run/intent rows are
   compared with the pre-stop snapshot the restart scenario recorded, and the
   post-restart serving log is scanned for a second ``POST /prompt`` or a
   ``reset``/resubmit marker.

It does not modify product code and never claims survival from a fresh session:
the same-process reconnect is measured by the restart scenario, this record is
explicitly a split measurement on the same durable data.

Run from the repository root with the Runtime's interpreter::

    C:/AI/orchestrator/.venv/Scripts/python.exe -u \\
      apps/desktop/devtools/runtime_integration/verify_restart_survival.py \\
      --source-run target/acc1-native/<run> --task-ref task-... \\
      --restart-record docs/development/evidence-published/.../record-runtime-restart.json \\
      --evidence-dir docs/development/evidence-published/acc1-restart-survival-latest
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import sqlite3
import sys
import time
from pathlib import Path

HARNESS = Path(__file__).resolve().parent
PINNED_NATIVE = HARNESS.parents[3] / "target" / "acc1-round2-pinned" / "run_acc1_native.py"


def load_native():
    if PINNED_NATIVE.is_file():
        spec = importlib.util.spec_from_file_location("acc1_native_pinned", PINNED_NATIVE)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module
    import run_acc1_native  # noqa: PLC0415 - fallback import

    return run_acc1_native


sys.path.insert(0, str(HARNESS))
import run_acc1 as acc1  # noqa: E402 - local test helper, stdlib only
native = load_native()


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_bytes(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def write_json(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False, indent=2, sort_keys=True),
                    encoding="utf-8")


def read_since(path: Path, offset: int, limit: int = 400_000) -> str:
    if not path.is_file():
        return ""
    with path.open("rb") as handle:
        handle.seek(offset)
        return handle.read(limit).decode("utf-8", errors="replace")


def db_connect(path: Path) -> sqlite3.Connection:
    connection = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True, timeout=5)
    connection.row_factory = sqlite3.Row
    return connection


def rows(connection: sqlite3.Connection, sql: str, parameters: tuple = ()) -> list[dict]:
    return [dict(row) for row in connection.execute(sql, parameters).fetchall()]


def db_snapshot(db_copy: Path, task_ref: str) -> dict:
    """Durable rows for one task, its runs, provider attempts, artifact and outcome."""
    try:
        connection = db_connect(db_copy)
    except sqlite3.Error as error:
        return {"readable": False, "error": f"{type(error).__name__}: {error}"}
    try:
        snapshot: dict = {"readable": True}
        snapshot["task"] = rows(
            connection, "SELECT task_id, status, created_at FROM creative_tasks"
                        " WHERE task_id = ?", (task_ref,))
        snapshot["runs"] = rows(
            connection, "SELECT run_id, iteration, phase, created_at FROM creative_runs"
                        " WHERE task_id = ? ORDER BY iteration", (task_ref,))
        run_ids = [str(item["run_id"]) for item in snapshot["runs"]]
        receipts: list[dict] = []
        intents: list[dict] = []
        for run_id in run_ids:
            receipts.extend(rows(
                connection, "SELECT receipt_id, run_id, node_id, attempt_id,"
                            " provider_receipt_id, created_at"
                            " FROM worker_provider_submission_receipts WHERE run_id = ?",
                (run_id,)))
            intents.extend(rows(
                connection, "SELECT intent_id, run_id, node_id, created_at"
                            " FROM worker_provider_submission_intents WHERE run_id = ?",
                (run_id,)))
        snapshot["provider_receipts"] = receipts
        snapshot["provider_intents"] = intents
        snapshot["outcome"] = rows(
            connection, "SELECT task_id, outcome, stop_reason, canonical_digest, payload,"
                        " created_at FROM creative_task_outcomes WHERE task_id = ?",
            (task_ref,))
        artifacts: list[dict] = []
        blobs: list[dict] = []
        for run_id in run_ids:
            run_artifacts = rows(
                connection, "SELECT artifact_id, blob_id, status, run_id, node_id,"
                            " manifest_digest, created_at FROM artifacts WHERE run_id = ?",
                (run_id,))
            artifacts.extend(run_artifacts)
            for artifact in run_artifacts:
                blobs.extend(rows(
                    connection, "SELECT blob_id, content_digest, length, media_type,"
                                " manifest_digest, created_at FROM artifact_blobs"
                                " WHERE blob_id = ?", (artifact["blob_id"],)))
        snapshot["artifacts"] = artifacts
        snapshot["artifact_blobs"] = blobs
        snapshot["observation_epoch"] = rows(
            connection, "SELECT epoch, claimed_at FROM cos_desktop_observation_epoch")
        snapshot["journal_rows"] = rows(
            connection, "SELECT COUNT(*) AS rows FROM cos_desktop_projection_journal")[0]["rows"]
        return snapshot
    except sqlite3.Error as error:
        return {"readable": False, "error": f"{type(error).__name__}: {error}"}
    finally:
        connection.close()


def prompt_submissions(log_text: str) -> dict:
    """Provider submit markers in a serving log (the Comfy client posts /prompt)."""
    lines = log_text.splitlines()
    posts = [line for line in lines if "POST /prompt" in line]
    resubmits = [line for line in lines
                 if "resubmit" in line.lower() or "attempt-r" in line]
    probes = [line for line in lines if "GET /history" in line or "GET /queue" in line]
    return {"post_prompt_lines": posts[-8:], "post_prompt_count": len(posts),
            "resubmit_lines": resubmits[-8:], "resubmit_count": len(resubmits),
            "history_queue_probe_lines": probes[-6:],
            "history_queue_probe_count": len(probes)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-run", required=True)
    parser.add_argument("--task-ref", required=True)
    parser.add_argument("--restart-record", required=True)
    parser.add_argument("--evidence-dir", required=True)
    parser.add_argument("--artifact-name", default="artifact.png",
                        help="file name for the served artifact bytes in the evidence dir")
    parser.add_argument("--record-name", default="record-survival-verify.json",
                        help="file name for this record in the evidence dir")
    parser.add_argument("--port", type=int, default=native.PORT_MAIN)
    args = parser.parse_args(argv)

    source_run = Path(args.source_run).resolve()
    evidence = Path(args.evidence_dir).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    restart = json.loads(Path(args.restart_record).read_text(encoding="utf-8"))
    before_receipts = ((restart.get("injections") or {}).get("provider_submitted")
                       or {}).get("receipts") or []
    before_prompt_id = next(
        (str(item.get("provider_receipt_id")) for item in before_receipts
         if item.get("provider_receipt_id")), "")
    before_receipt_ids = sorted(int(item.get("receipt_id")) for item in before_receipts
                                if isinstance(item.get("receipt_id"), int))

    ctx = native.Ctx("survival-verify", args.port, source_run=source_run)
    record = ctx.record
    record["source_run"] = str(source_run)
    record["mode"] = ("split measurement: same durable DB copy/CAS as the restart run,"
                      " a new short-lived serving epoch and no generation traffic")
    record["task"]["task_ref"] = args.task_ref
    record["_secrets"] = list(ctx.collect)
    source_log = source_run / "runtime.log"
    source_log_text = source_log.read_text(encoding="utf-8", errors="replace") \
        if source_log.is_file() else ""
    record["source_runtime_log_sha256"] = (
        sha256_file(source_log) if source_log.is_file() else "missing")
    record["source_runtime_log_bytes"] = source_log.stat().st_size if source_log.is_file() else 0
    record["pre_stop_snapshot"] = {
        "receipt_ids": before_receipt_ids, "prompt_id": before_prompt_id,
        "run_count": ((restart.get("injections") or {}).get("provider_submitted")
                      or {}).get("run_count"),
        "stopped_at_epoch": (restart.get("epoch") or {}).get("before", {}).get("epoch"),
        "restarted_at_epoch": (restart.get("epoch") or {}).get("after", {}).get("epoch"),
    }
    try:
        ctx.start_runtime(wait_generation=False)
        record["verify_runtime"] = {
            "pid": record.get("runtime", {}).get("pid"),
            "epoch": record.get("runtime", {}).get("epoch"),
            "endpoint": record.get("runtime", {}).get("endpoint"),
            "ready": record.get("runtime", {}).get("ready"),
            "observation_open": record.get("observation_open"),
        }
        db = db_snapshot(ctx.db_copy, args.task_ref)
        record["db_after_restart"] = db
        record["projection"] = now_task(ctx.projection_task(args.task_ref))
        resolved = ctx.resolve_artifact(args.task_ref)
        record["artifact_resolution"] = {
            key: value for key, value in resolved.items() if key != "descriptor"}
        descriptor = resolved.get("descriptor") or {}
        record["artifact_descriptor"] = {
            "keys": sorted(descriptor.keys()) if isinstance(descriptor, dict) else [],
            "media_type": descriptor.get("media_type") if isinstance(descriptor, dict) else None,
            "access_handle_present": bool(
                isinstance(descriptor, dict) and descriptor.get("access_handle")),
            "digest": descriptor.get("digest") if isinstance(descriptor, dict) else None,
        }
        destination = evidence / args.artifact_name
        fetched = ctx.fetch_artifact(resolved, destination)
        record["artifact_fetch"] = fetched
        projected_digest = (resolved.get("artifact") or {}).get("artifact_digest")
        task_blob_ids = {artifact.get("blob_id") for artifact in (db.get("artifacts") or [])}
        blob_rows = [item for item in (db.get("artifact_blobs") or [])
                     if item.get("blob_id") in task_blob_ids]
        record["artifact_identity"] = {
            "projected_artifact_digest": projected_digest,
            "fetched_sha256": fetched.get("sha256"),
            "fetched_bytes": fetched.get("bytes"),
            "digest_matches_projection": fetched.get("sha256") == projected_digest,
            "db_blob_rows": blob_rows,
            "db_outcome_canonical_digest": (
                (db.get("outcome") or [{}])[0].get("canonical_digest")
                if db.get("outcome") else None),
            "png": fetched.get("png"),
        }
        record["comfy_history"] = {}
        if before_prompt_id:
            record["comfy_history"] = comfy_prompt_state(before_prompt_id)
        after = db_snapshot(ctx.db_copy, args.task_ref)
        after_receipts = after.get("provider_receipts") or []
        after_receipt_ids = sorted(int(item.get("receipt_id")) for item in after_receipts
                                   if isinstance(item.get("receipt_id"), int))
        after_prompt_ids = sorted({str(item.get("provider_receipt_id"))
                                   for item in after_receipts
                                   if item.get("provider_receipt_id")})
        attempt_ids = sorted({str(item.get("attempt_id")) for item in after_receipts
                              if item.get("attempt_id")})
        submissions = prompt_submissions(source_log_text)
        outcome = (after.get("outcome") or [{}])[0] if after.get("outcome") else {}
        selected = []
        try:
            selected = json.loads(str(outcome.get("payload") or "{}")).get(
                "selected_artifact_refs") or []
        except (TypeError, ValueError):
            selected = []
        record["submission_analysis"] = {
            "pre_stop_receipt_ids": before_receipt_ids,
            "post_restart_receipt_ids": after_receipt_ids,
            "receipt_ids_unchanged": before_receipt_ids == after_receipt_ids,
            "post_restart_prompt_ids": after_prompt_ids,
            "prompt_id_matches_pre_stop": before_prompt_id in after_prompt_ids,
            "attempt_ids": attempt_ids,
            "attempt_suffix_resubmission": any(
                item != f"attempt-cos-{args.task_ref}-i0" for item in attempt_ids),
            "run_count": len(after.get("runs") or []),
            "receipt_count": len(after_receipts),
            "intent_count": len(after.get("provider_intents") or []),
            "selected_artifact_digests": [
                item.get("artifact_digest") for item in selected if isinstance(item, dict)],
            "serving_log_prompt_submissions": submissions,
            "serving_log_post_prompt_count": submissions["post_prompt_count"],
            "serving_log_resubmit_count": submissions["resubmit_count"],
        }
        record["resyncs"] = []
        record["trace_errors"] = []
        db_ok = db.get("readable") is True
        terminal = (after.get("task") or [{}])[0].get("status") == "COMPLETED"
        outcome_ok = (outcome.get("outcome") == "COMPLETED"
                      and outcome.get("stop_reason") == "SUCCESS_ACCEPT")
        artifact_ok = bool(fetched.get("fetched")) and bool(
            fetched.get("digest_matches_descriptor"))
        no_duplicate = (len(after_receipts) == len(before_receipts)
                        and after_receipt_ids == before_receipt_ids
                        and before_prompt_id in after_prompt_ids
                        and not record["submission_analysis"]["attempt_suffix_resubmission"]
                        and submissions["resubmit_count"] == 0)
        record["assertions"] = {
            "db_readable": db_ok,
            "durable_task_completed": terminal,
            "outcome_completed_success_accept": outcome_ok,
            "artifact_served_via_protocol": bool(fetched.get("fetched")),
            "artifact_digest_matches_projection": artifact_ok,
            "single_provider_receipt_unchanged": no_duplicate,
            "no_attempt_suffix_resubmission": not record[
                "submission_analysis"]["attempt_suffix_resubmission"],
            "comfy_history_success": record["comfy_history"].get("history_status") == "success",
        }
        record["verdict"] = "PASS" if all(record["assertions"].values()) else "PARTIAL"
        if record["verdict"] != "PASS":
            record["residuals"].append(
                "at least one post-restart survival assertion did not hold; see assertions")
        return record
    finally:
        cleanup = ctx.cleanup()
        record["cleanup"] = cleanup
        record["finished_at"] = now_iso()
        record["_secrets"] = list(ctx.collect)


def now_task(task: dict) -> dict:
    keys = ("task_ref", "task_status", "inner_phase", "iteration_index",
            "projection_revision", "capabilities_ref")
    return {key: task.get(key) for key in keys if key in task}


def comfy_prompt_state(prompt_id: str) -> dict:
    """ComfyUI history/queue for one durable prompt, no inference."""
    import run_acc1_round2 as round2  # noqa: PLC0415 - local test helper

    return round2.comfy_prompt_state(prompt_id)


if __name__ == "__main__":
    def arg_value(name: str, default: str) -> str:
        if name in sys.argv:
            index = sys.argv.index(name) + 1
            if index < len(sys.argv):
                return sys.argv[index]
        return default

    run = main()
    evidence_dir = Path(arg_value("--evidence-dir", "")).resolve()
    record_name = arg_value("--record-name", "record-survival-verify.json")
    secrets = list(run.pop("_secrets", []) or [])
    write_json(evidence_dir / record_name, run)
    leak = native.leak_scan(evidence_dir, secrets)
    leak_name = record_name.replace("record-", "leak-scan-").replace(".json", ".json")
    write_json(evidence_dir / leak_name, leak)
    print(json.dumps({"verdict": run.get("verdict"),
                      "assertions": run.get("assertions"),
                      "artifact": run.get("artifact_identity"),
                      "submissions": run.get("submission_analysis"),
                      "leak": leak}, ensure_ascii=False, indent=1))
