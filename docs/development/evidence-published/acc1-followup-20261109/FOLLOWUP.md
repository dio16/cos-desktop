# MP-ACC1 follow-up — observation-epoch ownership fix and live re-measurement (2026-11-09)

This directory is a **follow-up record** to
[`acc1-latest`](../acc1-latest/session.json); that record is retained unchanged.
It is a bounded acceptance re-attempt, not product acceptance.

- **Measurement:** MP-ACC1 Golden Creative Workflow acceptance attempt, follow-up
- **Started / finished:** 2026-11-09T08:13:14+0900 / 2026-11-09T08:15:07+0900
- **Verdict:** `LIMITED` (same scope limits as the original attempt)
- **Runtime:** `C:/AI/orchestrator`, fix commit `d6e0ba202f4068860c85208f9ea6e3b3b681bfcb`
- **Desktop:** `C:/dev/cos-desktop`, head `09a298f3bb5ea23e76734221e590dd52fb421dcc`
  (only the test-only `run_acc1.py` evidence-directory flag changed; no Desktop product code)
- **Task:** `task-fc882edc96874fb5b6111ce8c5887401`
- **Outcome:** `COMPLETED` / `SUCCESS_ACCEPT` (OUTCOME projection, `projection_revision` 266,
  outcome ref `sha256:f91432355681651a2bc419f8a5c14b7f01bc371bd99fb41c65a63eec60d40fe1`)

## Finding being fixed

The original attempt found that the supervised **generation worker process** opened the
same Runtime database through `Database()` and claimed the single
`cos_desktop_observation_epoch` row at startup. The serving API process already owned that
row, so the worker's claim invalidated the serving observation: delta cursors failed closed
and a fresh observation open returned `409 RESYNC_REQUIRED` (`observation epoch changed in
the database ledger`) for the rest of the serving process lifetime. Recovery required a
Runtime restart after the task reached terminal, so live projection continuity across
generation could not be measured.

## Fix (Runtime, commit `d6e0ba2`)

Only the **serving** Runtime process may claim/ensure the desktop observation epoch.

- `orchestrator_app/process_epoch.py` adds the explicit role contract:
  `COS_RUNTIME_PROCESS_ROLE=generation-worker`, with `serving` as the fail-closed default
  for an absent, empty or unrecognized value.
- The dispatch path (`creative_loop._dispatch_run_attempt`) sets that marker in the
  worker's allowlisted environment (the supervisor clears and re-supplies the environment,
  so the marker is part of the worker spec).
- `comfy_worker_process.main()` also declares the role before it opens a database, so the
  worker module can never claim the ledger even if invoked without the launcher marker.
- `Database._ensure_cos_desktop_task_journal()` under the worker role still creates the
  additive projection-journal schema (so worker writes are journaled and visible to the
  serving observation) but never claims the epoch row, clears the journal or resets its
  sequence. The second-serving-process invalidation rule is unchanged.

Runtime docs: `C:/AI/orchestrator/docs/desktop-command-protocol.md` epoch/ownership section
updated (Startup, DB and epoch; RT8 epoch paragraph).

### Runtime tests run (targeted, `C:/AI/orchestrator`)

```text
./.venv/Scripts/python.exe -m pytest tests/test_cos_frontend_contract.py -k "desktop or epoch" -q
  -> 43 passed, 64 deselected
./.venv/Scripts/python.exe -m pytest tests/test_cos_desktop_hello_resume.py \
  tests/test_cos_desktop_reads.py tests/test_cos_desktop_commands.py \
  tests/test_cos_desktop_projection_kinds.py tests/test_cos_production_runtime.py \
  tests/test_cos_production_bootstrap.py tests/test_no_worker_side_effect.py \
  tests/test_runtime_discovery.py tests/test_comfy_worker_adapter.py \
  --deselect "tests/test_cos_production_runtime.py::test_candidate_page_serves_thumbnail_for_published_handoff" -q
  -> 151 passed, 1 deselected
```

Total **194 passed**, 0 failed in the targeted set, plus the new real-process coverage:

- serving process + live worker process on the same DB copy: the epoch stays the serving
  process's, fresh observation open/delta/resume keep working while the worker is alive,
  and the worker's persisted task write still reaches the serving delta stream;
- two serving processes still invalidate each other (fail-closed unchanged);
- a new serving process still claims a new epoch and empties the journal (restart semantics);
- the real supervised dispatch (`worker-session-runner` + Comfy worker process, stub
  ComfyUI) keeps the serving epoch and journal while executing/persisting the attempt.

The deselected test `test_candidate_page_serves_thumbnail_for_published_handoff` fails at
the pre-existing Runtime HEAD as well (`NOT NULL constraint failed:
cos_desktop_projection_journal.entity_id` from the direct `evaluation_decisions` fixture;
reproduced with `HEAD:orchestrator/orchestrator_app/db.py`). It is unrelated to this fix and
was not modified.

## Live re-measurement

Same harness and preconditions as the original attempt; this run used a **new evidence
directory** so the original evidence was not rewritten:

```text
python apps/desktop/devtools/runtime_integration/run_acc1.py \
  --evidence-dir docs/development/evidence-published/acc1-followup-20261109
```

- Isolated Runtime on `127.0.0.1:8131`, fresh DB copy
  (`11,493,376 B`, sha256 `f8771100…bfb3e3d`), copied presets, own config/logs/CAS/outputs,
  per-run `ORCHESTRATOR_AUTH_SECRET` and `ORCHESTRATOR_COS_MEDIA_HANDLE_SECRET`, redirected
  RT8 discovery descriptor.
- Direct preflight: ComfyUI `0.35.1` `/system_stats` 200 with the documented argv; Ollama
  `/api/tags` 200 with 6 models. Runtime-reported health `{"llm": true, "comfy": true}`,
  generation plane ready.
- Runtime epoch `e1a79182d47941b19810863ec0b0e548` (PID 40864) and the epoch in the first
  observation grant are the same. The ledger epoch never changed during generation.
- Authenticated catalog (one recipe `recipe-cos-production-still`,
  `graph-cos-production-001` v2), Desktop `CommandEnvelope CREATE_TASK` accepted
  (`cmd-ebec06edea47b3290cf1e064`, task above).
- Live observation trace (`projection_trace`, 3 transactions):
  - `transaction-95a80c5b…` — TASK 1 (revision 261);
  - `transaction-81da570f…` — TASK 1 (revision 262);
  - `transaction-035ed6ac…` — TASK 1, ITERATION 1, CANDIDATE 1, ARTIFACT 1, EVALUATION 1,
    OUTCOME 1 (terminal transaction).
- State timeline through the live channel: `ASSIGNED` (261) → `ACTIVE` (262) →
  `ACTIVE`/`RUNNING`/iteration 0 (263) → `COMPLETED`/`RUNNING`/iteration 0 with
  `formal_outcome_ref` (266). The attempt broke on the observed terminal state and outcome;
  it did not time out (`task.timed_out=false`; ≈ 81 s from the first observed `ACTIVE` to
  the terminal transaction).
- Media: RC-13 descriptor 200 → handle → blob 200, **1,386,281 B**, computed SHA-256
  `sha256:06e97cd17bc42dc8904bcb34195292f5f7b38c047b9fef82d84a39c002033228`
  equals the descriptor digest, PNG 1024×1024; stored as [artifact.png](artifact.png).
- Evaluation read: candidate page projects decision `PASS`
  (`dec-8600a70313f061ba80d9fe868353d71e`), no local best/winner derivation.
- History read: 200, one iteration (`RUNNING`, plan/run ids), `has_more=false`.
- Post-terminal Runtime restart: new epoch `8a67ed5c99d04998a5044bee894b1216`, fresh
  observation 200 with 267 terminal mutations (TASK 40, ITERATION 38, CANDIDATE 38,
  ARTIFACT 37, EVALUATION 38, OUTCOME 38, GRAPH 38); delta probe 200/no transaction.
- Cleanup: both Runtime processes exited 0, port 8131 free, discovery descriptor removed,
  ComfyUI (started for this run) stopped afterwards, prior environment restored. Leak scan
  clean (17 capability/secret values checked, 0 findings).

## Before / after (the measured delta)

| Fact | `acc1-latest` (before) | this follow-up (after) |
|---|---|---|
| `observation epoch changed in the database ledger` | yes, on delta and on observation open | **absent from the Runtime log** |
| fresh observation open during/after generation | `409 RESYNC_REQUIRED` (refused) | **HTTP 200** on every open |
| serving ledger epoch during generation | replaced by the worker | **stable** (`e1a79182…`) |
| recovery | Runtime restart required for observation | **no restart needed**; post-terminal restart is only the planned new-epoch step |
| live projection through terminal | `NOT_MEASURED` (canonical reads only) | **measured** via the observation channel (timeline + terminal transaction) |
| formal outcome / media / history | measured | `COMPLETED`/`SUCCESS_ACCEPT`, 1,386,281 B digest-verified, history 200 |

**Resync detail (honest limitation):** the run still records **3 recoverable** `409
RESYNC_REQUIRED` answers on the delta path during generation, all with an immediate
`200` observation reopen and continued streaming:

1. `task history cannot be replayed as a state diff` — two task-state transitions landed
   between 0.75 s polls, so the journal cannot express the missed history as a diff;
2. `unknown-owner journal row cannot be replayed` — a worker-side journal row has no
   task attribution at that instant (artifact/handoff journaling);
3. the same unknown-owner reason close to completion.

None is epoch invalidation, none refuses an observation open, and none requires a restart;
the first was already present in the original attempt before the epoch claim. The literal
"zero 409 during dispatch/iteration" bar was therefore **not** met: the epoch-invalidation
channel is closed, while the protocol's designed resync-and-reopen recovery remains in use.
Making those journal rows replayable/attributable is a separate observation-journal task.

## Residuals

- This is a bounded attempt, not product acceptance; the Desktop GUI binary is not launched
  (protocol-level observation client), so native observation withdrawal/recovery and native
  artifact display remain `NOT_MEASURED`.
- DD-25 Start/Next Iteration semantics remain blocked, so those steps are `NOT_MEASURED`
  (the Runtime drain auto-dispatched the create).
- Five of six DC-MP-ACC1 failure injections remain `NOT_RUN`, and the planned
  client-side observation disconnect/resume did not trigger in this run
  (`failure_injection.attempted=false`: the terminal transaction arrived before the harness's
  3 s ACTIVE condition); the observation resync path was exercised by real journal movement
  instead.
- Compare interaction and multi-recipe breadth are not exercised.
- The runtime worktree still contains the seven pre-existing unrelated dirty entries; they
  were never staged. The fix commit is local (no push).
- The remaining journal-representability resyncs described above are a separate residual.
- `record.logs.runtime_log_sha256` is computed over the LF-normalized sanitized text while
  the stored file is written with the platform newline (CRLF on Windows); the digest is a
  text-identity record, not the stored-file byte digest. The same convention applies to the
  retained `acc1-latest` record.

## Reproduction

```text
cd C:/AI/orchestrator
git checkout d6e0ba2
./.venv/Scripts/python.exe -m pytest tests/test_cos_frontend_contract.py -k "desktop or epoch" -q

cd C:/dev/cos-desktop
python apps/desktop/devtools/runtime_integration/run_acc1.py \
  --evidence-dir docs/development/evidence-published/acc1-followup-20261109
```

The harness requires ComfyUI on `127.0.0.1:8188` and Ollama on `127.0.0.1:11434`, port
8131 free, `C:/AI/orchestrator/.venv`, and the built `worker-session-runner` binary; it
starts the isolated Runtime, restores the environment afterwards, and stops the processes
it owns.
