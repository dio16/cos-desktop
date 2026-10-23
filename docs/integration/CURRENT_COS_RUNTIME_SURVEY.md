# Current COS Runtime Survey

Status: **Observed** (source survey). Surveyed Runtime: `<runtime-root>`, branch `dev`, HEAD `f94985bd4c5a3a0714153f9d068f24439d57a1f2`, worktree clean including untracked, origin `<runtime-repository-url>`.

Initial state recorded 2026-09-21T01:00:19.464Z: Desktop repository uninitialized except the canonical Markdown; runtime untouched. This document is evidence for the Desktop protocol and required-changes documents; it is not an implementation claim about a future `cosd` and it does not replace runtime-owned documentation.

## 0. States used in this document

| Label | Meaning |
|---|---|
| **Observed** | Directly readable in source at the surveyed revision; later revalidation steps in §18. |
| **Accepted** | A recorded decision/canon that constrains integration (ADR, canonical doc, contract closure), not proof of implementation. |
| **Proposed** | Draft design (Desktop `DESKTOP_PROTOCOL.md`, this repo) or an unapproved change request. |
| **Deferred** | Explicitly postponed by the runtime docs/canon, not merely missing. |
| **Inferred** | Derived from absence or composition; marked inline where used. |

Method: read-only file reads, globs and greps over `<runtime-root>` on 2026-09-21. No runtime was started, no test/build/lint was run. Line numbers are from the surveyed revision and will drift; symbols and file paths are the stable anchors.

## 1. Topology and authority boundary

`<runtime-root>` is one git root with an execution plane (ComfyUI) and a control plane (`<runtime-root>/orchestrator`). The control plane owns API, frontend, launchers, persistence and tests; ComfyUI owns graph execution (`orchestrator/README.md:13`, `orchestrator/AGENTS.md` Ownership boundaries; **Observed**).

Inside the control plane there are two distinct loops, which must not be conflated:

- **COS (product runtime)** — Assigned Creative Task runtime + harness evolution: inner loop Generate→Evaluate→Refine, 9-layer DMLG, immutable ExecutionPlan, formal Outcome. Canon: `orchestrator/docs/architecture/cos-runtime.md` (**Accepted**; marked "planning freeze 2026-08-23, docs-only", `cos-runtime.md:11-13`, pinned to an older baseline `147c8cd4`).
- **REOS (development loop)** — a 5-state DevRun lifecycle (`READY/RUNNING/BLOCKED/DONE/FAILED`) with a Rust-gated closure. Canon: `orchestrator/docs/architecture/reos-development-runtime.md`; implementation `orchestrator_app/repo_loop_core/v4.py:176-184` (**Observed**). `STATE.md`'s "runtime: v4 five-state lifecycle" refers to this DevRun loop, **not** to COS tasks.

Runtime-owned semantics (the Desktop must never own): task lifecycle, phase, accepted input, evaluation/decision, candidate association, formal selection, Outcome, artifact canonical identity/availability, history, stop completion, GraphRevision, ExecutionPlan, provenance, persistence (`orchestrator/docs/architecture/frontend/FRONTEND_POLICY.md:32-33`; `orchestrator/AGENTS.md` Canonical ownership — **Accepted**). The Desktop repository instantiation of the same boundary is `../architecture/REPOSITORY_ARCHITECTURE.md`.

## 2. Rust workspace

Workspace `orchestrator/rust/Cargo.toml:1-13`: resolver 3, edition 2024, `rust-version = "1.97.1"`, `[profile.release] lto = true`, five members. `rust-toolchain.toml` pins channel 1.97.1. `Cargo.lock` pins serde 1.0.229, serde_json 1.0.151 (arbitrary precision), sha2 0.10.9, rusqlite 0.32.1, windows-sys 0.59.0; **no async runtime** (no tokio) — every surface is synchronous CLI/stdin-stdout or a supervised child process (**Observed**).

| Crate | Owns | Evidence |
|---|---|---|
| `contract-core` | Canonicalization, DigestV1/DecimalV1, closed schemas, task/objective/harness/evaluation/selection/refinement/knowledge/publication/verification semantics, the worker-message wire contract, the cross-layer identity envelope, authority/mutation vocabularies. | `crates/contract-core/src/lib.rs:1-14` module list; `AuthorityClass`/`MutationScope` closed enums `lib.rs` (7 and 6 variants); `MAX_FRAME_BYTES = 8 MiB`, `MAX_NESTING_DEPTH = 127` (`lib.rs`). |
| `artifact-core` | CAS: prepare/commit/scan/repair, no-replace install, retained-handle identity. | `crates/artifact-core/src/cas.rs`, `prepare.rs`, `commit.rs`, `scan.rs` (symbol list from the crate survey; `derive_blob_path` is the CAS layout owner referenced by `orchestrator_app/cos_frontend_media.py:67-79`). |
| `plan-core` | Deterministic ExecutionPlan compiler: normalize→validate→Kahn order→canonical digests, plan identity. | `crates/plan-core/src/compile.rs:9-10` (`COMPILER_VERSION = "plan-core-0.3.0"`, `BRIDGE_VERSION = "plan-core-cli.v2"`); `plan_id = "plan:<sha256hex>"` (`src/identity.rs`). |
| `graph-kernel` | SemanticGraph v1/v2 identity, validation, 9 layers, 12 edge kinds, deny-by-default layer/edge matrix, patches, commit/rollback preparation, `project_v2`. | `crates/graph-kernel/src/lib.rs:1-45` exports; `LayerKind` = POLICY, GOAL, CONTEXT, CAPABILITY, CREATION, EVALUATION, EXPERIMENT, FEEDBACK, LEARNING (`src/layer.rs:9-27`) — this is the machine form of the "9-layer DMLG". |
| `worker-runtime` | Byte framing, correlation, lease/fence/quiesce, process supervision, artifact handoff pipeline, provider receipts, runtime promotion, M4B benchmark authority. | `crates/worker-runtime/src/lib.rs:1-39`; `RUNNER_PROTOCOL = "worker-session-runner-v1"`. |

Worker message contract: `contract-core/src/worker_message.rs:14-16` `WORKER_MESSAGE_SCHEMA_VERSION = 1`, failure codes `WORKER_PROTOCOL_VIOLATION` / `WORKER_FRAME_INVALID`; kinds `execute | result | heartbeat | cancel` (`:38-54`); result statuses `accepted | running | succeeded | failed | cancelled` (`:126-150`); duplicates/malformed/sensitive keys fail closed (`:246-262`, `:535-546`).

Refinement/task vocabulary owned by `contract-core/src/refinement.rs` (**Observed**):

- `ProposalStatus` = PROPOSED, VALIDATED, APPLIED, REJECTED (`refinement.rs:20-26`).
- `InnerLoopPhase` (13) = ASSIGNED, CONTEXT_BUILT, GRAPH_READY, PLAN_COMPILED, RUNNING, ARTIFACT_READY, EVALUATED, REFINE_PROPOSED, VALIDATED, NEW_GRAPH_REVISION, NEW_PLAN_COMPILED, STOPPED, ACCEPTED (`refinement.rs:44-59`).
- `StopReason` (10) = SUCCESS_ACCEPT, BUDGET_EXHAUSTED, MAX_ITERATIONS, HARD_CONSTRAINT_FAILURE, POLICY_DENY, NO_MATERIAL_IMPROVEMENT, REPEATED_DUPLICATE, RESOURCE_UNAVAILABLE, HUMAN_STOP, UNRECOVERABLE_RUNTIME_FAILURE (`refinement.rs:62-74`).
- `RefinementRoute` = ACCEPT, PARAM, GRAPH_PATCH, HARNESS_CANDIDATE, STOP(reason) (`refinement.rs:80-86`); route priority is decided in Rust, not Python.
- Task status edges: `validate_status_transition` allows ASSIGNED→ACTIVE, ACTIVE→COMPLETED, ACTIVE→CANCELLED; everything else raises `TASK_AUTHORITY_VIOLATION` (`contract-core/src/objective.rs:611-624`).

The contract-core binary exposes ~65 CLI verbs (`Some("…")` arms in `crates/contract-core/src/main.rs`, counted 65), including the verbs the COS path uses: `task-status-transition-validate` (`main.rs:619-621`), `task-quad-digests` (`:602-618`), `param-proposal-validate` (`:640-660`), `refinement-route` (`:568-570`), `evaluation-fuse` (`:571-583`), `refinement-outcome-select` (`:584-601`), `context-finalize` / `context-manifest-validate` (`:513-515`, `:803-810`), `harness-*`, `graph-receipt-*`, `provenance-*`, `knowledge-*`, `benchmark-*`. Unknown verbs are rejected (`:818+`).

## 3. HTTP surface

One FastAPI app serves everything: `app = FastAPI(title="AI Orchestrator", version="0.2.0", lifespan=lifespan)` (`orchestrator_app/api.py:527`); 44 route decorators, no routers (`api.py` throughout), plus a second unrelated supervisor app in `orchestrator_app/dev_supervisor.py:152` (6 routes, no auth; process supervision only). Bind is loopback-only: `_is_loopback_host` accepts 127.0.0.1/::1/localhost resolved-loopback (`api.py:236-267`), and `main()` refuses a non-loopback host (`api.py:3151-3194`). Static mounts: `/static` and `/assets` from `static/cos` (`api.py:692-697`); `GET /` serves `static/cos/index.html` or an honest 404 (`api.py:720-726`).

### 3.1 COS routes (Desktop-relevant, **Observed**)

| Method | Path | Auth/scopes | Response model | Evidence |
|---|---|---|---|---|
| GET | `/api/cos/catalog` | none | `CatalogPayload` | `api.py:2685-2702` |
| POST | `/api/cos/tasks` (202) | `run:start`, rate-limited | `CommandReceiptPayload` | `api.py:2705-2726` |
| GET | `/api/cos/tasks` (`view=all\|activity`, `limit`, `cursor`) | none | `TaskPagePayload` | `api.py:2729-2745` |
| GET | `/api/cos/tasks/{task_id}` | none | `TaskViewPayload` | `api.py:2748-2764` |
| GET | `/api/cos/tasks/{task_id}/candidates` | none | `CandidatePagePayload` | `api.py:2767-2786` |
| GET | `/api/cos/tasks/{task_id}/candidates/{candidate_id}` (`run_id`,`iteration`,`artifact_ref`) | none | `CandidateDetailPayload` | `api.py:2789-2814` |
| GET | `/api/cos/tasks/{task_id}/evaluations/{candidate_id}` (same tuple) | none | `EvaluationViewPayload` | `api.py:2817-2843` |
| GET | `/api/cos/tasks/{task_id}/history` | none | `TaskHistoryPagePayload` | `api.py:2846-2862` |
| GET | `/api/cos/tasks/{task_id}/details` (optional all-or-none tuple) | none | `DetailsPagePayload` | `api.py:2865-2896` |
| GET | `/api/cos/commands/{command_id}` | any authenticated identity | `CommandReceiptPayload` | `api.py:2899-2915` |
| POST | `/api/cos/tasks/{task_id}/stop` (202) | `run:control` or `run:abort` + owner gate, rate-limited | `StopAckPayload` | `api.py:2918-2967` |
| GET | `/api/cos/media/{digest}` | none | `MediaDescriptorPayload` | `api.py:2970-2996` |
| GET/HEAD | `/api/cos/media/{digest}/blob` (`download`,`filename`) | none | bytes 200/206/416 | `api.py:3032-3064` |
| GET/HEAD | `/api/cos/media/{digest}/thumbnail` | none | bytes 200/206 | `api.py:3067-3106` |
| POST | `/api/cos/media/references` (201, raw body ≤10 MiB) | `references:write`, rate-limited | ad-hoc JSON (no model) | `api.py:3109-3148` |

All JSON reads pass through `_cos_json` (`api.py:2656-2682`): validate against the Pydantic model, serialize with Starlette's JSON settings, fail `413 JSON_RESPONSE_TOO_LARGE` above 256 KiB (`cos_frontend_api.py:569`). Error-code→HTTP maps: intake CONFLICT/CONFLICT/FORBIDDEN/UNKNOWN/INVALID_INPUT/RECIPE_UNAVAILABLE → 409/403/404/422/404; read READ_* + 413; media MEDIA_* (`api.py:2591-2611`); unknown codes fail closed as 500 (`api.py:2639-2653`). Mutation rate limit: 30 per 60 s per caller (`api.py:442-450`, applied at `2712`, `2926`, `3121`).

Non-COS but on the same app (**Observed**, `api.py`): auth routes (§4), `GET /api/ready`, `/api/dev-info`, `/api/models`, `/api/workflows/validate`, `GET /api/events` (SSE), `POST /api/run` (`run:start`), `POST /api/control` (any scope + in-handler checks), `GET /api/status`, admin retention/nightly, `POST /api/references/upload`, `GET /api/images/{path}`, generation/loops/runs reads, review and experiment routes. These belong to the legacy generation loop and are **not** part of the COS task contract; ADR-012 excludes their surfaces from the recovered client scope (`DECISIONS/ADR-012-frontend-recovery-boundaries.md` C01-C05; **Accepted**).

### 3.2 COS route policy registration

`_ROUTE_POLICY` (`api.py:199-232`) registers every mutation with an `Effect` and required scopes, including the three COS mutations (`POST /api/cos/tasks` → `run:start`; `.../stop` → `run:control|run:abort`; `media/references` → `references:write`). `tests/test_api_route_policy.py` introspects the app and fails on any unregistered mutation route (comment `api.py:162-167`; **Observed** in source).

## 4. Identity, auth, authorization

Scope vocabulary, exact (`api.py:180-193`): `run:start`, `run:control`, `run:abort`, `admin:retention`, `admin:nightly`, `references:write`, `review:write`, `experiments:write`, `experiments:promote`. Unknown scopes are rejected 400 at issuance (`api.py:661-663`).

Two credential paths (**Observed**):

1. **Bearer token** — `POST /api/auth/token` requires loopback client + `X-Auth-Secret` matching `ORCHESTRATOR_AUTH_SECRET` (`api.py:591-604`, `639-668`), TTL 24 h (`api.py:160`). Caller identity is server-bound: `ORCHESTRATOR_CALLER_ID` env or `"loopback-client"`, never from the body (`api.py:117-119`, `664-668`); `_issue_token` defaults to per-token `client-<10>` only for internal issuers (`api.py:343-359`).
2. **Browser CSRF session** — one-time bootstrap nonce (30 s TTL) minted only with the issuance secret (`api.py:297-321`, `530-554`), consumed by `GET /auth/browser-bootstrap` → 303 + HttpOnly SameSite=Strict `orchestrator_csrf` cookie (`api.py:557-588`); or `GET /api/auth/csrf` (`api.py:607-634`). Browser sessions receive exactly `run:start, run:control, admin:retention, admin:nightly, references:write, review:write, experiments:write` (`api.py:395-404`) — never `run:abort`, never `experiments:promote`.

`_require_auth` returns `{scopes, caller}`; requests without Bearer need `Origin`/`Referer` matching the loopback origin regex plus a live CSRF cookie (`api.py:325-340`, `361-406`). Reads have no auth dependency at all (table §3.1) — the loopback bind is treated as authenticating the destination, not the caller (comment `api.py:384-391`). `_AUTH_FLAG_DISABLED=1` turns authenticated mutations into 503 (`api.py:409-439`) — a fail-closed off switch, not an open one.

COS actor usage: `actor_id = _identity["caller"]` for intake (`api.py:2720`) and stop owner checks (`api.py:2935-2937`); task row stores `created_by` (`db.py:1298-1301`). Stop authorization: if the caller lacks `run:abort`, the caller must equal `task.created_by` exactly; otherwise 403. Receipt lookup is any-scope, then actor-compared in logic: a different actor gets `COMMAND_FORBIDDEN` → 403; unknown id → 404 (`cos_frontend_api.py:544-567`, `api.py:2905-2912`).

## 5. Wire DTOs and enums (frozen Python shapes)

All 33 models live in `orchestrator_app/cos_frontend_models.py` with `ConfigDict(extra="forbid")`, no aliases/rename (snake_case as-is). Patterns: `sha256:[0-9a-f]{64}`, `hvid_[0-9a-f]{64}`, UUIDv4 (`cos_frontend_models.py:18-23`). Key shapes (**Observed**, exact class anchors):

| Model (line) | Wire contract highlights |
|---|---|
| `GraphRefPayload` (21) | `graph_id`, `graph_version ≥1`, `graph_digest` — an exact revision pointer, never "latest". |
| `RecipeRefPayload` (31) | `recipe_id`, `source_harness_version_id`, `source_harness_digest`, `graph_ref`, optional `profile_ref`, `projection_digest`. |
| `EditableParameterPayload` (59) | `type ∈ {text, integer, decimal, boolean, choice}`, `value: str|int|bool`, `bounds`, `choices`, `editable`. |
| `CatalogPayload` (86) | `schema_version ≥1`, `can_submit`, `reason_codes` (e.g. `NO_CHAMPION`, `RECIPE_UNAVAILABLE:<id>`), `recipes`. |
| `CreateTaskRequestPayload` (102) | `command_id` (uuid4), `recipe_ref`, `objective_text` 1..8000, optional `text_prompt`, `reference_artifact_refs`, `parameter_values`. |
| `CommandReceiptPayload` (115) | `schema_version: 1`, `command_id`, `operation: "create_task"|"stop_task"`, `task_id`, `accepted_at`. |
| `AcceptedInputPayload` (125) | saved-envelope projection: objective/prompt/recipe refs/reference refs (≤8)/parameters/execution_limits/constraint_summary. |
| `SelectedArtifactRefPayload` (159) | formal-selected tuple `{artifact_digest, run_id, evaluation_id, iteration}` — transport mirror only. |
| `CreativeTaskOutcomePayload` (170) | 17 members mirroring `CreativeTaskOutcomeV1`: `outcome`, `selected_artifact_refs[]` (≤40), final evaluation/run/graph/plan ids+digests, harness id+digest, `hard_constraint_status`, `stop_reason`, `lineage_root_ref`, `canonical_digest`. `outcome`/`stop_reason` stay open strings so unknown codes render explicitly. |
| `TaskViewPayload` (199-213) | `schema_version:1`, `task_id`, `task_status: ASSIGNED|ACTIVE|COMPLETED|CANCELLED`, `phase?`, `iteration?`, `watch`, `accepted_input`, `outcome?`, `available_actions.stop`, `issues[]`. |
| `TaskSummaryPayload` (216-227) | + `outcome_code?`, `stop_pending`. |
| `CandidateRefPayload` (238) | server-issued tuple `{task_id, run_id, iteration, candidate_id, artifact_ref}`. |
| `CandidateSummaryPayload` (250) | `thumbnail_url?`, `media_type?`, `availability`, `decision_status?`, `is_outcome_selected`. |
| `MediaDescriptorPayload` (280) | `artifact_ref?`, `artifact_digest`, `media_type?`, `size_bytes/width/height/duration_ms`, `availability`, `original_url?`, `thumbnail_url?`. |
| `EvidenceItemPayload` (298) | `record_ref`, `status: AVAILABLE|MISSING|FORBIDDEN`, `summary?`, `detail?`. |
| `EvaluationViewPayload` (307) | `decision?`, `outputs[]`, `rubric`, `kpis[]`, `evidence[]`, `next_cursor`. |
| `DetailsLineagePayload` (332) | `status: COMPLETE|MANUAL_REPAIR|NOT_RECORDED`. |
| `StopAckPayload` (378-386) | `accepted`, `task_id`, `stopped`, `status` — an acknowledgement, never the outcome. |

Runtime-side persisted vocabularies (**Observed**, `db.py`): task status CHECK 4 values (`:1316-1317`); run `stop_reason` CHECK 10 values (`:1404-1409`); outcome CHECK `COMPLETED|PARTIAL|FAILED|CANCELLED` with the same stop_reason set (`:1413-1425`); evaluation authority levels 7 (`:1364-1372`) and decision statuses `PASS|FAIL|ABSTAIN|INCONCLUSIVE` (`:1382-1395`); handoff stages 6 (`:784-826`); artifact status `staged|available|failed|quarantined` (`artifacts/repository.py:46-83`). `creative_runs.phase` has no CHECK; only `STOPPED` requires a `stop_reason` (`db.py:5762-5786`).

Generated TypeScript alignment: `frontend/contracts/cos-openapi.json` is a checked-in snapshot of "live FastAPI openapi() export", 14 paths / 32 schemas, with a byte-match rule and an explicit note that the full 50-path live spec is not vendored (`cos-openapi.json:7-11`). `frontend/scripts/generate-api.mjs` converts it to `frontend/src/api/generated/cos.ts` (33 DTOs); the generated header forbids hand edits and says convergence onto the live API is owned by the browser-smoke lane, "never by tracking the API here" (`cos.ts:1-2`, `generate-api.mjs:1-5`). No Rust/ts-rs/typeshare codegen exists.

## 6. Task lifecycle: the implemented authority path

Intake (`POST /api/cos/tasks`) — `cos_frontend_api.submit_create_task` (`cos_frontend_api.py:441-542`): re-validate request shape; require the submitted `recipe_ref` to equal the live-projected option (stale → `RECIPE_UNAVAILABLE`); validate reference images exist and are ingestable stills; resolve parameter values against preset controls (unknown/immutable/invalid values reject; editable defaults filled — `:232-340`); mint `task-<uuid4hex>`; fingerprint the canonical request; issue the task quad digests through `contract-core` (`task-quad-digests`, `:342-386`); insert atomically via `db.insert_cos_task_intake_atomic` (`db.py:6208-6323`) writing quad spec rows, the `creative_tasks` row with `status='ASSIGNED'` and the accepted-input envelope `cos_task_inputs`; return the stored receipt. Replays of the same `(actor, command_id, fingerprint)` return the stored receipt with zero new rows; a different actor → `COMMAND_FORBIDDEN`; different operation/fingerprint → `COMMAND_CONFLICT` (`db.py:6236-6323`).

Execution: a single in-process drain consumer starts with the API lifespan (`api.py:496-524`: one daemon thread, `poll_interval_s=5.0`, `dispatch_cos_assigned_task`), and `creative_loop.run_cos_drain_once` (`creative_loop.py:1258-1320`) runs Phase A recovery over ACTIVE tasks then Phase B claim over ASSIGNED tasks (bounded `_COS_DRAIN_SCAN_LIMIT`). Claim (`claim_cos_assigned_task`, `:1189-1256`) is one `BEGIN IMMEDIATE` transaction: Rust `ASSIGNED→ACTIVE` validation, `activate_assigned_task` (knowledge retrieval + context manifest, pinned idempotently), conditional UPDATE with rowcount==1, and if a stop command exists it terminates ACTIVE→CANCELLED **without dispatch**. Only a winning claim dispatches.

Dispatch (`dispatch_cos_assigned_task`, `creative_loop.py:1638-1881`) composes existing owners only: terminal-outcome convergence (if an outcome row already exists → complete and return `observed-terminal`, `:1672-1676`); manifest/activation reuse; deterministic ids `cand-cos-<task>-i0`, `eval-cos-<task>-i0`, `attempt-cos-<task>-i0` (`:1753-1755`); resume paths reuse durable evaluation or handoff records instead of resubmitting (`:1759-1778`, `_dispatch_complete_handoff_digest` `:1936-1948`); task-bound graph template projected through graph-kernel `project-v2` (`:1780-1817`); `compile_cos_plan` → plan-core (`:1819-1845`); `graph_plan_lineage` write and run binding with `phase="RUNNING"` (`:1845-1870`); one failed attempt through the worker session runner; artifact handoff; evaluation via `evaluation-fuse` (`_dispatch_evaluate`, `:2124-2219`); outcome selection via `refinement-outcome-select` and single-row persistence (`select_and_persist_cos_outcome`, `:831-871`); completion (`complete_cos_task`, `:1080-1115`) validates ACTIVE→COMPLETED through Rust, publishes finalized handoff artifacts into the servable registry in the same transaction, then moves the row conditionally. `_dispatch_finish` (`:2221-2291`) routes on `SUCCESS_ACCEPT` with `stop_reason="SUCCESS_ACCEPT"`.

Stop (`POST /api/cos/tasks/{id}/stop`) — `record_cos_stop_command` (`creative_loop.py:652-710`) inserts an immutable `cos_commands` row (`db.py:1993-2014`, tombstone triggers on UPDATE/DELETE) with operation `stop_task`, hardcoded `status='accepted'`, and a caller-computed receipt; same rules as intake for replay/conflict/forgery. Then `acknowledge_cos_stop` (`:726-782`): terminal rows → `{stopped:false, reason:"terminal-kept"}`; non-ACTIVE → `{stopped:false, reason:"not-active"}`; ACTIVE → Rust ACTIVE→CANCELLED validation, conditional UPDATE rowcount==1, then exactly-once `worker.handle_cancel` when a worker is supplied. The HTTP ack is never the outcome; `StopAckPayload` carries `accepted/stopped/status`.

Restart reconciliation — `reconcile_cos_restart` (`creative_loop.py:784-829`) is read-only and returns one of: `terminal-keep` (outcome row exists or status terminal), `cancel-before-dispatch` (ACTIVE + stop command), `dispatch-once` (no runs; ASSIGNED, or ACTIVE without runs — status never moves back), `reconcile-observe` (ACTIVE + runs + provider receipt observed), `ambiguous-manual` (ACTIVE + runs, no receipt). Provider receipt resolution itself stays in the provider owner.

## 7. Stop-state projection semantics (**Observed**)

`get_task_view` (`cos_frontend_api.py:673-746`) computes `available_actions.stop` at `cos_frontend_api.py:718-726`:

| Task state | `stop.enabled` | `reason_code` |
|---|---|---|
| outcome present or status COMPLETED/CANCELLED | false | `terminal` |
| ACTIVE with a recorded stop command | false | `stop-pending` (+ issue) |
| ACTIVE without stop command | true | null |
| any other (i.e. ASSIGNED) | false | `not-dispatched` |

The POST stop route nevertheless accepts a stop for an ASSIGNED task (it only requires the task to exist): the command is recorded, the ack returns `{accepted:true, stopped:false, status:"ASSIGNED"}`, and the drain later cancels it before dispatch. Desktop must therefore not read `available_actions.stop.enabled=false` as "stop impossible"; conversely a projectional `stop_pending` flag exists on summaries (`TaskSummaryPayload`) but TaskView exposes it only via the `stop-pending` issue when ACTIVE.

## 8. Media and CAS (**Observed**)

`cos_frontend_media.py` owns delivery mechanics only. CAS layout delegates to artifact-core `derive_blob_path` (`:67-79`); `find_artifact_row` joins `artifacts`/`artifact_blobs` (`:82-93`); `verify_to_spool` copies to a private temp spool while verifying SHA-256, mismatch → `MEDIA_INTEGRITY` (`:96-133`); `prepare_delivery` handles one RFC 7233 range, rejects >1 GiB (`SPOOL_LIMIT_BYTES` mirroring `api._RESOLVER_SPOOL_LIMIT` at `api.py:1433`), returns headers incl. `Cache-Control: private, no-store` (`:146-238`); `describe_artifact` returns availability plus dimensions/duration where known (`:295-345`); `ingest_reference` caps 10 MiB / 40 MP, PNG/JPEG/WebP only, imports through the artifact repository (`:358-427`); `ensure_thumbnail` renders one tier (long edge ≤320) as a child artifact with its own digest and lineage edge (`:428+`). Routes fill `original_url` only for `availability == "available"` and `thumbnail_url` only for available stills in `INGEST_TYPES` (`api.py:2988-2993`). Media bytes are served from the same app that serves control JSON; no separate media service exists (**Deferred** in runtime docs; `DESKTOP_PROTOCOL.md` §6 marks the media service deferred too).

## 9. Graph / DMLG / ExecutionPlan

Persisted graph substrate (**Observed**, `db.py`): `graph_versions` unique `(graph_id,version)`+digest (`:1060-1068`); `graph_change_receipts` with `committed|rejected` outcome and prev-digest chain, pinning `capability_snapshot_digest` (`:1070-1097`); `graph_patches` FK to receipts (`:1099-1115`); `graph_plan_lineage` binding graph revision → plan/compiler input digest (`:1117-1131`); `graph_head_pointer_history` with revision CAS + rollback receipt (`:1133-1194`). There is **no symbol named DMLG** in Python; the DMLG is the conceptual name (`cos-runtime.md` §5.4; ADR-007) whose machine realization is the graph-kernel v2 model with its nine `LayerKind`s and twelve `EdgeKind`s (`graph-kernel/src/edge.rs`, `validate.rs` matrix deny-by-default), projected via `project-v2` (`creative_loop.py:508-522`, `:1797-1817`).

ExecutionPlan: compiled by plan-core (deterministic; `plan-core-0.3.0`), persisted per run as `run_plan_evidence` (`db.py:437-448`, `persist_run_plan` `:11171-11227`) and for COS as run rows with `plan_id`/`plan_digest` plus `graph_plan_lineage`. **No HTTP route exposes graph or plan data to a client.** Outcome references carry `final_graph_id/version/digest` and `final_plan_id/digest` (§5), so a Desktop can link to identities it cannot yet fetch.

## 10. Provenance, evidence, capability

- `provenance_events` is a per-run hash-chained journal: UNIQUE `event_id` and `idempotency_key`, UNIQUE `(run_id,sequence)`, genesis `(1, '0'*64)`, head-contiguous enforcement, ≤1 MiB canonical JSON (`db.py:546-576`, `append_provenance_event` `:9568-9643`).
- Evidence references surface through evaluation payloads as `EvidenceItemPayload {record_ref, status AVAILABLE|MISSING|FORBIDDEN, summary?, detail?}` (`cos_frontend_models.py:298-304`; evidence collected first-seen `cos_frontend_api.py:1190-1222`).
- Capability registry storage exists: per-run `capability_snapshots` + one `capability_snapshot_sets` per run (immutable), sequenced `capability_revocation_sets` with a CAS pointer row (`db.py:666-729`). `/api/cos` exposes **no** capability projection. The COS dispatch session frame passes a **zeroed stub**: `capability_id: None`, `sha256:`+64 zeros for snapshot/revocation digests, revision 1 (`creative_loop.py:2056-2061`) — real capability wiring is not part of the COS path today.
- Action broker tables (`action_intents`, `action_receipts` with `CHECK(dispatched = 0)`, single-use `action_approval_consumptions`) exist for approval-gated effects (`db.py:992-1057`); publication/knowledge/harness promotion lanes have their own stores (not in the current Desktop scope).

## 11. Events and streams

The only push surface is `GET /api/events`, an unauthenticated SSE over an in-process `EventBus` (`api.py:897-903`). Bus properties (`events.py`): `emit` is thread-safe via `call_soon_threadsafe`; history is a 100-event in-memory deque (`events.py:43-45`) replayed to new subscribers, whose queues are bounded at 500 frames (`events.py:66`); history is cleared at each run start (`api.py:951`, `events.py:81-83`). Frame shape `{"type": str, "data": {}, "ts": float}` with no `id:`, no `retry:`, no `event:` — no `Last-Event-ID` resume protocol. Emitting code is the legacy loop (`main.py` emits `loop_start/loop_end/workflow_step/image_completed/...`; `api.py` emits `error/paused/resumed/aborting`); **`creative_loop.py` never touches the bus** (no `emit(` call sites), so COS task progress is not streamed at all. There is no WebSocket anywhere (runtime docs describe WS as postponed; `docs/archive/.../COS_FRONTEND_IMPLEMENTATION_TASKS.md` F19 — **Deferred**). The browser SPA compensates with polling (§12).

Desktop-side integration direction (**Accepted** by the Desktop owner on 2026-09-21, not a runtime capability): the first candidate transport is loopback HTTP against the runtime, with a future **task-scoped resumable SSE adapter**; the generic `/api/events` bus is not to be reused as the task observation path. Unauthenticated reads and the absence of a stable server identity remain real-integration gates (RC-03, RC-16).

## 12. Current frontend (runtime side) and generated DTO chain

Source SPA `orchestrator/frontend` (Vite 8 + React 19.2.8 + react-router 8.3.1 hash router + TanStack Query 5), built and placed to `orchestrator/static/cos` only by `pnpm run release` (`frontend/scripts/place-release.mjs`, release record `tests/studio_cutover_release.json`). `static/cos` is a build artifact (index.html + hashed JS/CSS), not hand-written; the legacy inline UI is gone. Serving: `GET /` + `/assets` mount (`api.py:692-697`).

Transport: single same-origin fetch wrapper with `baseUrl` default `""` — no env/base-URL injection point, no Tauri/Electron/IPC anywhere (**Observed**, scouts + `frontend/src/api/http/client.ts:89-96`). Session rides the ambient CSRF cookie; there is no token handling in the SPA. Query layer: 10 GET hooks + 3 mutations over 13 endpoints (`frontend/src/api/queries/cos.ts`), polling `COS_TASK_ACTIVE_POLL_MS=2000` / failed `10000`, capped by 413/401-403 stops and 429 `Retry-After`; mutations are never auto-retried; no optimistic writes (`main.tsx:9-24` sets `retry:false`, no focus/reconnect refetch). The sole task observer is `ConnectedWorkspace` (2 s/10 s poll, terminal gating, one-shot invalidation of dependent keys); stop is offered only when `available_actions.stop.enabled`; outcome codes outside the four supported render as "未対応の正式成果" with mutations disabled. Composer keeps a `sessionStorage["cos.pending-command"]` record for lost-response lookup via `GET /api/cos/commands/{id}`. Reuse hand-off reads `accepted_input` once and never executes.

Consequence for Desktop: the only implemented observation pattern is **per-task polling of read projections**; there is no client-independent stream, snapshot session, projection revision or epoch.

## 13. Persistence and durability (**Observed**)

SQLite (WAL, FK on, recursive triggers, busy_timeout 5000; `db.py:2058-2084`) beside the API DB path (`db/generation.sqlite` by default, `api.py:700-708`). Migration head **38** (`db.py:2955`; v37 = COS intake tables, v38 = read indexes `:2947-2955`). Immutability triggers guard `cos_commands`, `cos_task_inputs` and multiple domain tables; intake is atomic across quad + task + envelope + command + receipt. Reads are keyset-paginated (opaque base64url cursors with a scope member; `cos_frontend_api.py:587-611`) and capped (tasks/candidates 40, evidence/history/details 20; `:851-855`); every JSON read enforces the 256 KiB ceiling. Handoff/CAS stores have per-`(run,attempt,request_digest)` uniqueness and stage-unique events (`db.py:784-939`). No COS projection-revision counter, no observation journal and no runtime-epoch column exist; the only epoch-like counters are harness `registry_epoch` (monotonic promotion CAS, `db.py:1814-1818`, `:1966-1968`) and per-attempt lease `fencing_token` (`db.py:732-751`).

## 14. Cross-process IPC / transports

- **Contract/plan/graph CLIs** — one-shot `stdin` JSON → `stdout` JSON; `contracts/canonical.py:_call` (`:73-77`) invokes the resolved binary; `contract_core_binary()` builds it with cargo when missing (`:24-38`) — a dev convenience, not a compiled hot path. `graph-kernel` and `plan-core` resolve through `rust_resolver.resolve_rust_binary` (`rust_resolver.py:1-74`), fail closed when absent. Exit codes/prefixes are stable (`ContractError`, exit 2 on reject per crate survey); frames bounded to 8 MiB/127 depth.
- **Worker session runner** — argv + newline-delimited frames over stdin/stdout with a closed transport identity `worker-session-runner-v1` and worker-message schema 1; `--protocol-info` probe validated by the Python preflight (`creative_loop.py:1423-1482`), refusing missing/truncated/incompatible binaries before the API reports ready; the dispatch hot path never builds (README §Preflight).
- **Staging binding** — before the worker protocol starts, the supervisor writes one binary frame (`ORIGEN-STAGING-BINDING\x01`, length-prefixed token ≤128 B, `<Q` duplicated handle) on stdin; Python consumes it exactly once, compares the token, wraps the handle, and owns no lease/CAS/token semantics (`worker_staging_binding.py:1-79`).
- The Desktop repo must not depend on any of these internals; they are runtime-internal transports (**Accepted**, `../architecture/DEPENDENCY_RULES.md`).

## 15. Runtime documentation and current-lane constraints

- `FRONTEND_POLICY.md` — server authority and projection-plus-explicit-commands invariants; the document itself warns it is "a target design, not a claim that the running application implements it" (`:5`). A1–A5 "backend gaps" recorded there are now implemented in `api.py` (typed transport, outcome mirror, media descriptor, recipe acceptance, keyset reads), while request-ID propagation was marked deferred (`FRONTEND_TARGET_ARCHITECTURE.md` §4 A5).
- ADR-012 (per-decision register, no single status): C01 task-bound stop instead of pause/resume (DEFAULT-ADOPTED), C02 catalog-backed production controls with isolated simulations (DEFAULT-ADOPTED), C03 bounded Details lineage, no workflow map (DEFAULT-ADOPTED), C04 operator/admin surface excluded (DEFAULT-EXCLUDED), C05 clear-history excluded (DEFAULT-EXCLUDED), C06 explicit local draft presets (USER-AUTHORIZED 2026-09-15).
- Contract closures that shaped the current surface: FRONTEND-API-TRANSPORT ("direct polling, no event/session/store framework"), FRONTEND-INTAKE ("actor-bound idempotent command receipt"), FRONTEND-EXECUTION-WIRING ("stop ack never rewrites terminal state"; "at-least-once delivery without new queues"), STUDIO-HANDOFF-COMPARISON (forbids new HTTP routes/WebSocket/SSE), PRODUCTION-RUNTIME-CLOSURE/RECERTIFY (drain dispatch, harness/graph pins, immutable storage, real outcome).
- `STATE.md`/`Task.md` are non-authoritative projections dated 2026-09-12; `task_graph.yaml` states "execution_permission: false". Do not treat them as current authority (ADR-011).
- Known staleness: `cos-runtime.md` is a docs-only planning freeze pinned to an older HEAD; `README.md`/`GOAL.md` carry dangling doc-path references; the README test-count line is older than the latest testing lane (survey notes).

These restrictions describe existing browser/frontend work lanes, not authority to remove Flow/DMLG or other product requirements from the Desktop canon. New observation endpoints require a separate explicitly authorized Runtime contract; they cannot be smuggled into an old lane. Current frontend scope is evidence, not the Desktop product specification.

## 16. Gap analysis (no invention)

### 16.1 No-gap snapshot/stream

- **What exists:** a per-task, single-transaction snapshot (`get_task_view`) including outcome and stop state; keyset summary pages; per-candidate evaluation/evidence/history/details reads; each under a 256 KiB ceiling. This is sufficient for a Desktop **polling** client (the shipped SPA does exactly that at 2 s/10 s).
- **What does not exist:** (a) any cross-entity atomic cut (a task + its candidates + evaluations + outcome as one consistent snapshot bundle); (b) any durable delta/stream of COS state — the only SSE bus is in-memory, cleared per legacy run, never touched by `creative_loop.py`, and carries no task correlation; (c) per-projection revisions — the wire has no revision field, and persistence has no projection-revision column; (d) observation scope/cursor identity — cursors are per-listing keyset positions only, bound to a view name, not to a principal/epoch/scope; (e) `Last-Event-ID`-style replay correctness for anything.
- **Consequence:** `DESKTOP_PROTOCOL.md` §2's atomic observation session, ProjectionDelta and epoch are **not satisfyable by adaptation alone**; they require a runtime-owned observation surface (journal + cut + epoch). Polling conversion is possible now, with the documented staleness windows.

### 16.2 Durable command dedupe

- **What exists:** `cos_commands.command_id` is the global primary key, with actor_id and canonical payload_fingerprint validated on replay (`db.py:1993-2014`). It is not a composite database key. Same owner/payload returns the stored receipt; different actor is forbidden and conflicting payload/operation is rejected. Intake stores authoritative rows and acceptance receipt atomically. Stop reconciliation consults the persisted intent. These are source observations, not executed crash-recovery proof.
- **Unverified guarantees:** retention/expiry advertisement, execution-state reconciliation, backup/restore behavior and server identity continuity. Token rotation does not necessarily change actor_id; changed actor identity blocks owner lookup/replay. An older DB backup may omit a command entirely, so immutable rows alone do not guarantee duplicate prevention after restore. Commands have no observed expected_revision precondition.
- **Consequence:** reuse the existing actor-bound receipt mechanism, but keep lost responses ResultUnknown and avoid automatic mutating replay until the server's identity, retention and restart/restore guarantees are established.

### 16.3 Restart epoch

- **What exists:** restart-safe reconciliation dispositions (§6), immutable stop/create commands, per-attempt lease fencing tokens, harness `registry_epoch` CAS, single-use staging tokens, and a bootstrap sequence that re-pins harness/champion/graph at startup (`cos_production_bootstrap.py`).
- **What does not exist:** any process-run identity (`boot_id`/`startup_epoch`/`runtime_epoch`) exposed on the wire (`[Inferred]` from the absence of any such symbol in `orchestrator_app`); any durable observation history that a reconnecting client could replay; any binding of read cursors or command receipts to an epoch; any guarantee that "same command id after a server restart" is treated as replay rather than a fresh decision (it is treated the same — the DB is the same — but there is no advertised guarantee or epoch to compare).
- **Consequence:** reconnect needs fresh reads/full resync and conservative ResultUnknown handling. The Draft runtime_epoch must be server-owned. A client-local session generation may discard late callbacks but must never masquerade as a server restart epoch.

### 16.4 Other observed gaps relevant to the Desktop contract

1. **Read authorization is absent** while the Desktop protocol assumes authorized scopes and "authorization change invalidates cursors". Today anyone on the loopback host can read everything; mutations need scopes. Conversion is needed before the protocol's authorization claims are true.
2. **Capability projection is a stub in the COS path** (zeroed digests, §10) — a `CreativeCapabilities` projection cannot be derived from real grants yet.
3. **No graph/plan read surface** — identities are carried in outcomes but nothing serves `GraphSnapshot`/inspection data.
4. **`phase` has no frozen vocabulary** (open TEXT except `STOPPED` coupling) — a protocol that must map phase values needs either a runtime-frozen enum or an explicit unknown-preserving rule.
5. **Stop/available-actions asymmetry** (§7) — `stop.enabled=false` does not mean "stop not accepted" for ASSIGNED tasks.
6. **Media descriptors lack `descriptor_id`/audience/expiry/verification fields** and are unauthenticated; the protocol draft expects more (§6).
7. **No protocol/schema version negotiation surface** — only the worker transport identity and the OpenAPI snapshot exist; `cosd_version`, `protocol_version`, `schema_version` do not.
8. **No error envelope** — errors are HTTP status + `detail: "CODE: message"` strings; `request_id` appears only inside task `issues`/read error handling, not on every response.
9. **No conditional read support** (ETag/If-None-Match) and `Cache-Control: private, no-store` on media only.
10. **Single-execution topology** — one GPU slot, `gpu_max_concurrency: 1` (`creative_loop.py:_COS_SCHEDULER_POLICY`), single drain thread, one dispatcher; Desktop concurrency assumptions must respect this.

## 17. Feature classification for the Desktop conversion

Classes: **(1) reusable existing boundary** — Desktop adapter can consume it after mapping; **(2) server desktop conversion** — semantics exist but the server-side surface must be built/converted; **(3) new runtime capability** — authority/semantics that do not exist yet; **(4) bootstrap fixture only** — Desktop-repo fixture work implying no runtime change.

| Feature / capability | Evidence | Class | Note |
|---|---|---|---|
| Create task (idempotent, actor-bound) | §6 `submit_create_task` | 1 | Bearer token path needed instead of browser CSRF for Desktop (§4). |
| Task view / list / history summaries | §3.1, §7 | 1 | Polling semantics; no revision/epoch. |
| Candidates + evaluations + evidence + details reads | §3.1, §5 | 1 | Tuple-parameterized; server-resolved. |
| Stop command + ack | §6, §7 | 1 | Ack ≠ completion; owner gate; ASSIGNED asymmetry. |
| Command receipt lookup | §16.2 | 1 | Owner-scoped; no expiry semantics. |
| CAS media blob/thumbnail/range | §8 | 1 | Unauthenticated; descriptor hardening pending. |
| Reference image ingest | §8 | 1 | `references:write`; 10 MiB cap. |
| Catalog projection (live champion + pinned graph) | §5, `build_catalog` | 1 | `can_submit` + `RECIPE_UNAVAILABLE:*` reasons. |
| Structured error envelope with codes/correlation | §16.4.8 | 2 | Server conversion; codes exist as typed strings. |
| Authenticated read scopes / audience binding | §16.4.1 | 2 | Conversion before protocol authorization claims. |
| Media descriptor hardening (id/expiry/audience/verification) | §16.4.6 | 2 | Additive fields over existing verify pipeline. |
| Version negotiation (protocol/schema/cosd) | §16.4.7 | 2 | Server must advertise; Desktop negotiates. |
| Desktop credential path (token issuance for a desktop principal) | §4 | 2 | Server-side issuance policy; no new semantics. |
| Graph/plan read projection | §9 | 2 + 3 | Read surface (2) over existing persisted identities (1); scoped inspection semantics may be new. |
| Capability projection with real grants | §10, §16.4.2 | 3 | Requires wiring capability snapshots/revocations into the COS session. |
| Runtime epoch + observation scope/cursor + projection revisions | §16.1, §16.3 | 3 | New runtime capability; cannot be fabricated server-side from existing columns safely. |
| Atomic observation session (snapshot cut + delta replay) | §16.1 | 3 | Requires durable observation journal/outbox or equivalent. |
| Command `expected_revision` preconditions | §16.2.4 | 3 | Requires revisions first. |
| Dedupe retention/expiry contract (`ReceiptExpired`) | §16.2 | 3 | Policy + advertised window. |
| Frozen phase vocabulary / unknown preservation | §16.4.4 | 3 | Runtime-owned enum or explicit rule. |
| Desktop Protocol types and scripted fixture provider | Protocol / bootstrap plan | 4 | In-process fixture only, not a dummy successful real transport |
| Media metadata/blob/thumbnail | §8 | 1 + 2; bootstrap 4 | Reuse existing media functionality; harden authorization/descriptor contract. Desktop decode/cache is a separate later presentation implementation, not a reason to invent duplicate Runtime media semantics. |

## 18. Revalidation procedure and uncertainties

Revalidation (read-only, no runtime start):

1. From `<runtime-root>`, run `git rev-parse HEAD` → compare to `f94985bd…`; `git status --porcelain` → expect clean.
2. Re-grep the anchors: `api.py` `/api/cos/` route decorators; `_ROUTE_POLICY`; `_COS_*_HTTP` maps; `cos_frontend_models.py` model set; `db.py` CREATE TABLE blocks and migration head (`INSERT INTO schema_migrations(version) VALUES (38)`); `cos_frontend_api.py` public functions; `creative_loop.py` stop/reconcile/drain/dispatch symbols; `rust/crates/*/src/lib.rs` exports, `refinement.rs` enums, `worker_message.rs` constants; `frontend/contracts/cos-openapi.json` provenance block; `frontend/src/api/queries/cos.ts` endpoint table; `static/cos` release record.
3. Confirm absence claims by grep: `websocket`, `boot_id|startup_epoch|runtime_epoch`, `projection_revision`, `DMLG` (Python), `tauri|electron|ipc` (frontend/static).
4. Run the runtime's own verification only in the runtime repo, by its owners (pytest lanes, `cargo test`, frontend lanes); this survey ran nothing.

Uncertainties / [Inferred] items:

- Route-count and exact line numbers drift with edits; use symbols.
- The vendored OpenAPI snapshot is a checked-in projection whose byte-match rule is enforced by the smoke lane, not by this survey; the 14 vendored paths were cross-checked against `api.py` but the "50-path live spec" claim was not independently counted (only route decorators, 44).
- `_dispatch_resume_decision` discards the stored output record (`creative_loop.py:1950-1971`) — resume evidence coupling is partial [Inferred].
- COS phase vocabulary beyond `STOPPED` is open text [Inferred]; only `STOPPED ⇒ stop_reason` is machine-checked.
- `frontend/src/api/queries/cos.ts` hand-written `ReferenceIngestResult` and approximate line anchors come from the frontend survey; treat exact numbers as approximate.
- No `boot_id`-equivalent exists; the statement "no restart epoch" is an absence claim from greps in `orchestrator_app` (repo-wide symbol search limited to that tree plus the Rust workspace).

## 19. Cross-references

- Desktop protocol draft: `../architecture/DESKTOP_PROTOCOL.md`.
- Required runtime changes: `./COS_RUNTIME_REQUIRED_CHANGES.md`.
- Ownership of the future machine-readable contract: `./PROTOCOL_OWNERSHIP.md`.
- Physical repository/dependency rules: `../architecture/REPOSITORY_ARCHITECTURE.md`, `../architecture/DEPENDENCY_RULES.md`.

## Bootstrap revalidation (2026-09-21)

Observed again during the authorized Desktop implementation: Runtime root `<runtime-root>`, branch `dev`, HEAD `f94985bd4c5a3a0714153f9d068f24439d57a1f2`, clean porcelain including untracked files, unchanged origin `<runtime-repository-url>`. The surveyed commit has not changed. Rechecked Rust workspace members, COS route declarations, Task/Candidate/Evaluation/Stop DTO anchors, and stop/restart/dispatch function anchors. The classifications and RC-01–RC-17 remain applicable; no new server capabilities are inferred from Desktop fixture work.

No Runtime build, test, process launch, database read, configuration import, or source mutation was performed. Final baseline comparison is recorded separately in the bootstrap report; concurrent external changes must not be reverted.
