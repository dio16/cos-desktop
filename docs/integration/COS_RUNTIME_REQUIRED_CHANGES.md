# COS Runtime Required Changes

Status: **Proposed** Runtime implementation requests, except the **scoped SPEC_APPROVED MP-PUB1 consumer handoff** in RC-02/03/08/15 ratified by historical direct owner approval (see [disclosure](../development/evidence/README.md#historical-records)). This register addresses the external Runtime repository at `<runtime-root>`; it authorizes no Runtime implementation/scheduling. Desktop may implement its own approved protocol/client/Fixture side only after its separate start instruction. Contract approval is not evidence of existing authenticated server capability or real integration.

Initial state (2026-09-21T01:00:19.464Z): Runtime at `<runtime-root>`, branch `dev`, HEAD `f94985bd4c5a3a0714153f9d068f24439d57a1f2`, clean worktree including untracked; Desktop repository uninitialized except the canonical Markdown. Evidence anchors are in `./CURRENT_COS_RUNTIME_SURVEY.md` (§ references below). Survey date 2026-09-21; re-run §18 of the survey before acting.

Priorities: **P0** = required before any real Desktop integration is meaningful; **P1** = required for the protocol guarantees the Draft claims; **P2** = valuable, schedulable after P0/P1.

## Summary

| ID | Change | Owner | Priority | Bootstrap covers? |
|---|---|---|---|---|
| RC-01 | Runtime-owned desktop protocol adapter (cosd conversion) | Runtime | P0 | No (fixture-only stand-in) |
| RC-02 | Desktop principal + credential issuance | Runtime | P0 | No |
| RC-03 | Read authorization / audience binding | Runtime | P0 | No |
| RC-04 | Runtime epoch + session identity | Runtime | P0 | No (fixtures simulate) |
| RC-05 | Atomic observation session (cut + durable delta journal) | Runtime | P0 | No |
| RC-06 | Projection revisions | Runtime | P0 | Partially (fixture revision) |
| RC-07 | Structured error envelope + correlation IDs | Runtime | P1 | No |
| RC-08 | Dedupe retention / `ReceiptExpired` semantics | Runtime | P1 | No |
| RC-09 | Command `expected_revision` preconditions | Runtime | P1 | No |
| RC-10 | Stop projection and reflection alignment | Runtime | P1 | Partially |
| RC-11 | Capability projection with real grants | Runtime | P1 | No |
| RC-12 | Graph/Plan inspection read surface | Runtime | P1 | No |
| RC-13 | Media descriptor and media access hardening | Runtime | P1 | No |
| RC-14 | Frozen phase vocabulary / unknown preservation | Runtime | P1 | Partially |
| RC-15 | Protocol/schema version negotiation + capabilities | Runtime | P1 | Client-side only |
| RC-16 | Desktop launch/lifecycle integration (service, not checkout) | Runtime | P0 | No |
| RC-17 | Task-scoped resumable SSE binding (after RC-05, if selected) | Runtime | P1 | No |

Current Runtime frontend lanes exclude pause/resume, clear-history and operator/filesystem surfaces and prohibit opportunistic new endpoints. These are constraints on those lanes, not amendments to Desktop product canon. New Desktop observation/security work requires a separately authorized Runtime task/contract. Preserve stop/outcome authority and existing validation throughout.

---

## RC-01 — Runtime-owned desktop protocol adapter (cosd conversion)

- **Required change:** Implement a runtime-owned protocol endpoint (a `cosd`-like service or an adapter process inside the runtime repository) that serves the typed Desktop protocol over a versioned transport, delegating to the existing authority path. It must not re-implement any semantics; it converts the existing read projections and commands into protocol envelopes (negotiation, observation, commands, receipts, media descriptors).
- **Reason:** The Desktop canon assumes process separation (`COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md` §23-24) and a cosd process, while the runtime today exposes only a loopback FastAPI browser app with cookie-oriented auth and per-request Pydantic JSON (§3, §4 of the survey). README and repository architecture explicitly say `cosd` is future runtime work; Desktop must not embed Python or import internal crates (`../architecture/REPOSITORY_ARCHITECTURE.md`).
- **Owner:** Runtime repository (new owner surface; protocol server), with the Desktop `desktop-protocol` crate as the consumer contract owner.
- **Proposed API (sketch, not final):** `Hello/Negotiate → OpenObservation → Snapshot(…) → ProjectionDelta(…) | ResyncRequired`, `SubmitCommand(create_task|stop_task) → CommandReceipt`, `LookupReceipt`, `GetMediaDescriptor`, `Hello.capabilities`. Envelope carries `desktop_version`, `cosd_version`, `protocol_version`, `schema_version`, message kind, correlation/stream identity (per `../architecture/DESKTOP_PROTOCOL.md` §1, §5). Desktop-side direction (**Accepted** by the Desktop owner 2026-09-21): first candidate transport is loopback HTTP against a runtime-owned adapter; a future **task-scoped resumable SSE adapter** is the intended streaming path (never the legacy generic `/api/events` bus). The adapter's `Hello` must carry a stable server identity — none exists today beyond a process, so server-identity issuance is a real-integration gate alongside RC-03/RC-04.
- **Compatibility:** Additive to the SPA. The FastAPI surface stays as-is; the adapter reuses `cos_frontend_api`, `cos_frontend_media`, and `creative_loop` functions (no new semantics). Existing browser clients and smoke lanes are unaffected.
- **Migration:** Phase 1 adapter process exposing reads + create/stop by delegation; Phase 2 switch observation semantics behind the protocol (RC-04/05/06); Phase 3 retire any duplicate mapping code. No DB migration needed for the adapter itself.
- **Desktop dependency:** Entire `desktop-protocol` + `cos-client`; bootstrap uses a fixture provider behind the same client boundary and must not claim server guarantees (`DESKTOP_PROTOCOL.md` §7).
- **Priority:** P0.
- **Bootstrap vs real integration:** Bootstrap can only fake this with an in-repo fixture provider. Real integration requires the runtime adapter; no amount of Desktop code can create the server side.

## RC-02 — Desktop principal and credential issuance

- **Required change:** Define a first-class Desktop principal and an issuance path for Desktop credentials (server-bound identity and scopes), instead of relying on a browser CSRF cookie or a loopback-issued generic bearer token whose caller string is a shared env value.
- **Reason:** Today the only issuer is `POST /api/auth/token` gated by `X-Auth-Secret` (`ORCHESTRATOR_AUTH_SECRET`) with server-bound caller `ORCHESTRATOR_CALLER_ID` or `"loopback-client"` (`api.py:117-119`, `639-668`), and browser sessions get the reduced scope set. Task ownership (`created_by`) is part of stop authorization (`api.py:2935-2937`), so the Desktop's principal must be stable across its own lifetime. The protocol Draft binds dedupe to `(principal, command_id)` (`DESKTOP_PROTOCOL.md` §3) — a shared `"loopback-client"` string cannot express that.
- **Owner:** Runtime (credential/identity owner).
- **Approved MP-PUB1 boundary:** verified server/channel identity plus synchronous principal/auth-context/channel-change detection before new credentials/bytes/wake, using the client/adapter close-only control in DC-MP-PUB1. Actual issuer, secure storage and production adapter delivery remain external/unverified; no new issuer, OS permission or loopback trust is granted by this approval.
- **Proposed API:** Operator-approved credential issuance bound to a stable Desktop principal and authenticated server, with scopes/expiry/revocation. Store credentials in an OS secure store, never plaintext; see Protocol sections 4 and 6 and PROCESS_MODEL security policy.
- **Compatibility:** Additive; the browser session scopes and token endpoint remain. Rate limit and route policy registration (`_ROUTE_POLICY`, `api.py:199-232`) must gain entries for any new route.
- **Migration:** Reuse `_issue_token` machinery with an explicit principal parameter; add issuance policy + audit record (existing `provenance_events` or a dedicated credential audit row), then document rotation/revocation.
- **Desktop dependency:** `cos-client` credential handling, `platform` (credential store) later; bootstrap uses a fake credential with no server meaning.
- **Priority:** P0.
- **Bootstrap vs real integration:** Bootstrap cannot validate real auth (`DESKTOP_PROTOCOL.md` §7); real integration is blocked on this.

## RC-03 — Read authorization and audience binding

- **Required change:** Decide and implement read authorization for COS projections and media (scope or capability-based), and bind observation scopes/cursors to the authenticated principal and authorization context so that an authorization change invalidates cursors as the protocol Draft requires (`DESKTOP_PROTOCOL.md` §2).
- **Reason:** COS projection/media reads lack caller authentication in the surveyed routes. Loopback binding restricts reachability but does not authenticate the server or same-user caller. Principal-scoped observation and authorization-change invalidation require stronger identity/access control.
- **Owner:** Runtime (auth policy), with protocol surface in RC-01.
- **Approved MP-PUB1 handoff:** [Protocol authorized read handoff](../architecture/DESKTOP_PROTOCOL.md#authorized-read-handoff) specifies fresh request/context/read-revision/scope/epoch binding, complete ≤448 retained-ID decisions, Runtime local-intent permission, ordered grant invalidation and stamped state/receipt outcomes. Runtime evaluates current principal/policy/cursor audience and never relies on Desktop Task/status inference. Scope/identity changes invalidate old reads synchronously at the client's detection boundary. Runtime enforcement, credentials and delivery need a separately authorized Runtime task; Desktop Fixture transcripts are not that enforcement.
- **Compatibility:** Breaking for anonymous same-host readers. Keep an explicit loopback-trust mode only if the runtime owner wants it; otherwise gate reads and update the SPA bootstrap (the cookie session can carry read scopes).
- **Migration:** Introduce scope checks on COS read routes behind a config switch; update smoke lanes; then flip. Media URLs in descriptors must remain usable by the desktop media fetch path (signed or session-bound; see RC-13).
- **Desktop dependency:** `cos-client` must present credentials for reads and handle `Unauthorized/ResyncRequired`.
- **Priority:** P0.
- **Bootstrap vs real integration:** the consumer contract is approved, so Desktop can verify scripted grant/revocation/denial handling in explicit Fixture mode. Real policy, authenticated adapter/server, cursor-audience enforcement and remote revocation delivery remain NOT_RUN; this is not a new MP-PUB1 hard prerequisite or simulated real success.

## RC-04 — Runtime epoch and session identity

- **Required change:** Introduce a runtime-owned epoch/session identity that changes whenever prior cursor validity cannot be guaranteed (restart at minimum), expose it in the protocol, and persist/broadcast it so clients can detect discontinuity.
- **Reason:** No Desktop-facing restart epoch was identified in the surveyed source trees. Existing attempt fencing and harness registry epochs are different scopes. The Draft requires server-owned identity/epoch, not a client-generated substitute; durable reuse across restart requires proof.
- **Owner:** Runtime.
- **Proposed API:** `runtime_epoch` (monotonic or opaque string) in `Hello`/session establishment and on every snapshot/delta header; documented invalidation rules (restart, DB restore). If a durable epoch is chosen, it must be backed by a persisted boot/epoch row and replay guarantees.
- **Compatibility:** Additive field; SPA unaffected. Command receipts may optionally carry the epoch that accepted them.
- **Migration:** Start with process-start epoch (non-durable, invalidates cursors on restart). Upgrade to durable only with RC-05's journal.
- **Desktop dependency:** Protocol §2; client treats epoch change as `ResyncRequired`.
- **Priority:** P0.
- **Bootstrap vs real integration:** Fixtures simulate epoch changes for staging logic; the real value must come from the runtime.

## RC-05 — Atomic observation session: consistent cut + durable delta journal

- **Required change:** Implement an atomic observation session: establish a consistent read cut and replay position atomically, send a complete snapshot bundle with a commit marker, then stream/replay committed deltas in order; support explicit `ResyncRequired` and never silently drop updates.
- **Reason:** Today only single-task transactional reads exist (`get_task_view`, `cos_frontend_api.py:673-746`) plus keyset pages; there is no cross-projection cut, no delta history for COS, and the only stream is an in-memory SSE bus never touched by `creative_loop.py` (survey §11, §16.1). The Draft explicitly forbids assembling unrelated endpoint responses and calling it atomic (`DESKTOP_PROTOCOL.md` §2).
- **Owner:** Runtime.
- **Proposed API:** `OpenObservation(scope, last_seen_cursor?, epoch?)` → `SnapshotBegin{snapshot_id, scope, epoch, cursor, expected_counts, integrity_digest}` + snapshot entities + `SnapshotEnd`; then `DeltaTransaction{cursor, predecessor, base/new revisions, upserts/removals}`. Server pins delivery after the cut before releasing it. Implementation options (runtime's choice): append-only COS projection journal table (recommended, e.g. a monotonic `cos_projection_events` sequence with the same write transaction as domain mutations), or a derived outbox over existing append-only tables (`creative_runs`, `cos_commands`, `artifact_handoff_*`, `evaluation_*`, `creative_task_outcomes`, `provenance_events`) with a documented completeness proof.
- **Compatibility:** Purely additive to REST; the SPA can keep polling or later adopt deltas. No change to authority paths if the journal is written in the same transactions as existing row writes.
- **Migration:** (1) define the journal schema + writers; (2) backfill/serve initial snapshots from existing reads under a cut; (3) expose deltas behind the adapter; (4) migrate the SPA opportunistically. DB migration must follow ADR-005 phased migration rules and the runtime's migration leaf discipline.
- **Desktop dependency:** Core of `cos-client` observation/recovery; the protocol's `ProjectionDelta`, snapshot staging, duplicate/gap handling and overflow behavior all depend on it.
- **Priority:** P0 (largest single work item).
- **Bootstrap vs real integration:** Bootstrap cannot emulate atomic cuts against the real DB; fixtures test only client staging logic. Real integration requires this.

## RC-06 — Projection revisions

- **Required change:** Give every projection (task, candidate, artifact, evaluation, iteration, outcome, graph) an opaque monotonically increasing revision within `(runtime_epoch, projection key)`, returned on reads and used by deltas for `base_revision`/`new_revision` checks.
- **Reason:** Surveyed COS projection DTOs lack a projection_revision field. Existing Domain graph/registry revisions are different identities and cannot stand in for arbitrary projection changes. Introduce a scoped projection revision without claiming that persistence has no revisions of any kind.
- **Owner:** Runtime.
- **Proposed API:** Additive `projection_revision` on protocol projections (not necessarily on the legacy REST payloads; the adapter may compute it from the journal sequence or a per-row version). Revisions must never be compared numerically to GraphRevision identities.
- **Compatibility:** Additive; GraphRevision identities stay untouched.
- **Migration:** If RC-05 uses a monotonic journal, the journal sequence can serve as the revision source for delta-driven projections; static reads may expose a derived revision via a durable row version column if needed.
- **Desktop dependency:** Protocol §1, §2; UI staging and conflict rules.
- **Priority:** P0.
- **Bootstrap vs real integration:** Fixtures can produce synthetic revisions; real consistency requires runtime computation.

## RC-07 — Structured error envelope and correlation IDs

- **Required change:** Return structured errors (code, safe message, correlation/command id, scope, optional `retry_after`, field errors, current revision, resync reason) from the protocol adapter, mapping the existing typed codes.
- **Reason:** Today errors are HTTP status + `detail: "CODE: message"` strings (`CosHttpError.fromBody`, `frontend/src/api/http/client.ts:28`; maps `api.py:2591-2611`); `request_id` exists only inside task `issues` and read-error handling, and is not propagated on every response (A5 was deferred in `FRONTEND_TARGET_ARCHITECTURE.md`). The Draft's error taxonomy (`DESKTOP_PROTOCOL.md` §4) needs it.
- **Owner:** Runtime (adapter), with code vocabulary owned by runtime.
- **Proposed API:** `{code, message, correlation_id, command_id?, scope?, retry_after?, field_errors?, current_revision?, resync_reason?}` with the Draft's code list mapped onto existing codes (`COMMAND_CONFLICT/FORBIDDEN/UNKNOWN`, `INVALID_INPUT`, `RECIPE_UNAVAILABLE`, `READ_*`, `MEDIA_*`, plus new `Unauthorized/Forbidden/ResyncRequired/ReceiptExpired/CommandIdConflict/...`).
- **Compatibility:** Additive for the adapter; SPA parsing unchanged.
- **Migration:** Start by mapping existing typed exceptions; no domain change.
- **Desktop dependency:** `cos-client` error model; UI error surfaces.
- **Priority:** P1.
- **Bootstrap vs real integration:** Bootstrap uses the same error enums client-side; the mapping itself is real-integration work.

## RC-08 — Dedupe retention and `ReceiptExpired` semantics

- **Required change:** Define and advertise the dedupe retention window and the behavior when a receipt is no longer retained (explicit expired/unknown outcome), instead of today's unbounded-though-immutable store with no expiry contract.
- **Reason:** `cos_commands` never prunes and nothing advertises a window (survey §16.2); the Draft requires "Dedupe retention is negotiated and advertised as an expiry for lookup/replay safety" and forbids assuming equivalence beyond the window (`DESKTOP_PROTOCOL.md` §3).
- **Owner:** Runtime.
- **Approved MP-PUB1 read distinction:** current stamped Allowed/no receipt means absent/expired lookup, not non-execution; Denied reveals no receipt/retention/outcome and never converts a retained command to Failed or settles it. Existing retained unknown/receipt survives denial/expiry; positive read evidence plus true local absence alone permits NotRetained. Actual command rejection is separate Runtime truth. No server retention duration/pruning/longer dedupe guarantee or automatic resend is approved.
- **Proposed API:** Retention policy in `Hello.capabilities`/`CreativeCapabilities`; receipt lookup returns an explicit expired/unknown distinction; command acceptance documents the retention promise per principal.
- **Compatibility:** Additive; if the runtime later prunes tombstones, that is a semantic change requiring a review (tombstone triggers currently forbid delete; pruning would need an owner-approved archive path).
- **Migration:** Publish the policy first (even "retained indefinitely") so the client can reason; add expiry only with a pruning design.
- **Desktop dependency:** Retry policy and `ResultUnknown` handling in `cos-client`.
- **Priority:** P1.
- **Bootstrap vs real integration:** Fixture can simulate expiry; the real guarantee is runtime-owned.

## RC-09 — Command preconditions (`expected_revision`)

- **Required change:** Support optional `expected_revision`/`expected_epoch` preconditions on mutating commands whose correctness depends on a viewed version, returning a conflict with the current reference/revision instead of last-write-wins.
- **Reason:** Commands today carry only `command_id` (+ payload); there is no precondition support (survey §16.2.4). The Draft requires expected revisions for such mutations (`DESKTOP_PROTOCOL.md` §3).
- **Owner:** Runtime.
- **Proposed API:** Optional fields on the command envelope; `RevisionConflict` error carrying current revision; for creation, an explicit creation scope instead of a fake task target.
- **Compatibility:** Additive; absent precondition preserves current behavior.
- **Migration:** Requires RC-06 revisions to exist.
- **Desktop dependency:** `app-core` command orchestration and conflict re-evaluation.
- **Priority:** P1.
- **Bootstrap vs real integration:** Fixtures can test conflict handling; server enforcement is real work.

## RC-10 — Stop projection and reflection alignment

- **Required change:** Make stop intent and stop completion observable and consistent across statuses: expose a first-class `stop_requested`/`stop_pending` on TaskView for every non-terminal status, and expose the eventual reflection of an accepted stop command (terminal status + stop reason/outcome) so a command id can be reconciled with canonical effect.
- **Reason:** `available_actions.stop.enabled=false` with `reason_code="not-dispatched"` for ASSIGNED (`cos_frontend_api.py:718-726`) coexists with POST stop accepting ASSIGNED intents that later cancel before dispatch (`creative_loop.py:1233-1244`); `TaskSummaryPayload` already carries `stop_pending` (`cos_frontend_models.py:216-227`) but `TaskViewPayload` (`:199-213`) only surfaces it as an issue when ACTIVE (survey §7, §16.4.5). The Draft's receipt state `CanonicalReflected` requires the link (`DESKTOP_PROTOCOL.md` §3).
- **Owner:** Runtime.
- **Proposed API:** Additive `stop_requested`/`stop_pending` field on `TaskViewPayload`; optionally a `command_reflection` query (stop command → current task status/outcome) by the owning principal.
- **Compatibility:** Additive to the models; the SPA can ignore new fields; `extra="forbid"` means the SPA's generated types must be regenerated when models change (they are generated from the snapshot; keep the snapshot in step).
- **Migration:** Add field, regenerate snapshot/generated types, update SPA tests.
- **Desktop dependency:** Stop UX (pending intent vs completed) and receipt reconciliation.
- **Priority:** P1.
- **Bootstrap vs real integration:** Fixtures can show pending/final states; the runtime must produce them.

## RC-11 — Capability projection with real grants

- **Required change:** Wire capability snapshots/revocations into the COS path and expose a `CreativeCapabilities` projection (supported commands/media/features, constraints, availability/reason, scope, revision) instead of the zeroed stub.
- **Reason:** The dispatch session passes `capability_id: None` and zero digests (`creative_loop.py:2056-2061`), while capability tables exist and are immutable/sequenced (`db.py:666-729`); the Desktop protocol requires capability-driven UI without provider-name inference (`DESKTOP_PROTOCOL.md` §4, `COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md` §44).
- **Owner:** Runtime.
- **Proposed API:** `CreativeCapabilities` projection served by the adapter; capability revision/digest pinning in command receipts and graph receipts (already persisted fields exist).
- **Compatibility:** Additive.
- **Migration:** Replace the zeroed stub with real digests from the capability owner; publish semantics of availability vs permission.
- **Desktop dependency:** `app-core`/UI feature gating; unknown capability defaults disabled.
- **Priority:** P1.
- **Bootstrap vs real integration:** Bootstrap fixtures may interpolate capabilities; real gating requires the runtime projection.

## RC-12 — Graph/Plan inspection read surface

- **Required change:** Provide a scoped, read-only graph/plan inspection projection (GraphSnapshot: canonical GraphRevision identity, plan refs, nodes/edges for display, provenance/refinement refs) sourced from `graph_versions`, `graph_change_receipts`, `graph_plan_lineage` and graph-kernel projections.
- **Reason:** Graph identities exist and are persisted, and outcome payloads carry `final_graph_*`/`final_plan_*`, but no HTTP/protocol surface exposes them (survey §9); the Desktop canon makes Flow/DMLG a Surface (`COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md` §19-20) and the protocol lists `GraphSnapshot`.
- **Owner:** Runtime (read authority; graph-kernel owns projection semantics).
- **Proposed API:** `GetGraphSnapshot(task_ref|graph_ref, revision?)` returning a bounded inspection graph (nodes/edges with layer/kind labels, provenance refs), plus explicit "not available for historical revisions" semantics. Presentation revision ≠ GraphRevision identity.
- **Compatibility:** Additive; no mutation endpoints.
- **Migration:** Reuse `project-v2`/`ValidatedGraphProjectionV2` for a display-shaped projection; bound sizes (protocol frame limits) with paging/collapse rules.
- **Desktop dependency:** Flow surface, Inspector; bootstrap can use fixtures.
- **Priority:** P1 before real GraphSnapshot integration; bootstrap uses fixtures and needs no Runtime graph change.
- **Bootstrap vs real integration:** Fixture graphs suffice for bootstrap; real inspection needs the server projection.

## RC-13 — Media descriptor and media access hardening

- **Required change:** Extend the media descriptor with opaque `descriptor_id`, representation kind, verification state and expected digest, server identity/audience scope and expiry, and authorize media fetches consistently with RC-03. Keep bytes out of the control channel.
- **Reason:** Current descriptor has availability/media metadata and authorized-looking URLs but no descriptor identity/expiry/audience, and blob/thumbnail reads are unauthenticated (`api.py:3032-3106`; survey §8, §16.4.6). The Draft's media contract requires per-principal, expiring, same-authority locators with re-authorization (`DESKTOP_PROTOCOL.md` §6).
- **Owner:** Runtime.
- **Proposed API:** Additive descriptor fields + a fetch policy that re-checks task/artifact authorization per request (existing pipeline already verifies SHA-256 and supports ranges).
- **Compatibility:** Additive fields; SPA URL usage continues if URL shape stays compatible; retire anonymous access with RC-03.
- **Migration:** Add fields first; then bind fetch to credential once RC-02/03 exist.
- **Desktop dependency:** `cos-client` + future `media` crate; Desktop must not treat URLs as filesystem paths or unrestricted capabilities.
- **Priority:** P1.
- **Bootstrap vs real integration:** Bootstrap can only use fixture descriptors/bytes; real media interop needs this plus a real media fetch.

## RC-14 — Frozen phase vocabulary and unknown preservation

- **Required change:** Freeze the `phase` vocabulary (or explicitly define an open-with-unknown rule) for task views/history, aligned with `contract_core::refinement::InnerLoopPhase`, and state how unknown values are transported and rendered.
- **Reason:** `creative_runs.phase` is open TEXT and only `STOPPED ⇒ stop_reason` is machine-checked (`db.py:5762-5786`); observed values include `"RUNNING"` and `"EVALUATED"` (`creative_loop.py:1845-1870`, `2247-2251`), while the Rust inner-loop vocabulary has 13 phases (`refinement.rs:44-59`). A protocol mapping cannot be stable without a decision (survey §16.4.4).
- **Owner:** Runtime (phase authority).
- **Proposed API:** Documented mapping table runtime-phase → protocol phase; unknown values preserved verbatim as `Unknown(raw)` client-side and inhibit dependent actions (per Draft §1).
- **Compatibility:** Additive if treated as an open vocabulary with preserved unknowns; breaking only if values are renamed (do not rename).
- **Migration:** Either add a CHECK constraint at the next migration leaf (with backfill review) or document the open rule and pin the observed set in tests.
- **Desktop dependency:** Status/phase display and action gating; bootstrap fixture uses ACTIVE/REFINE-like labels that must not be claimed as runtime wire values.
- **Priority:** P1.
- **Bootstrap vs real integration:** Partially bootstrap-coverable as a client rule; the runtime decision is real work.

## RC-15 — Protocol/schema version negotiation and capabilities advertisement

- **Required change:** Add runtime-side protocol/schema version advertisement and negotiation for the Desktop protocol (`desktop_version`, `cosd_version`, `protocol_version`, `schema_version` ranges), distinct from the worker transport identity.
- **Reason:** The only versioned cross-process contract today is `worker-session-runner-v1` + worker-message schema 1 (`worker-runtime/src/lib.rs:31-39`, `worker_message.rs:14-16`) and the OpenAPI snapshot (`frontend/contracts/cos-openapi.json:7-11`); there is no product protocol negotiation (survey §16.4.7). The Draft defines the negotiation handshake and refusal rules (`DESKTOP_PROTOCOL.md` §5).
- **Owner:** Runtime (server side) with protocol ownership per `./PROTOCOL_OWNERSHIP.md`.
- **Approved MP-PUB1 transition:** use the [Protocol](../architecture/DESKTOP_PROTOCOL.md#authorized-read-handoff) required-field/interaction major target2.0/2.0; negotiate/confirm before state/commands, reject1.0-only/unstamped/disjoint/mixed unsupported peers for authorized reads and never default missing grants/stamps. Target coordinate is not a released/deployed server. Other capability/transport choices remain separately owned; maintain contract→server→client→compatibility/migration.
- **Compatibility:** Additive; the SPA does not use it.
- **Migration:** Implement in the adapter; keep the negotiated pair fixed per session.
- **Desktop dependency:** Observed bootstrap Rust/fixtures are1.0/1.0. Approved MP-PUB1 Desktop migration targets2.0/2.0 and compatibility refusal; the real authenticated server/adapter is separately delivered and NOT_RUN here.
- **Priority:** P1.
- **Bootstrap vs real integration:** Client-side negotiation logic is bootstrap-testable; the server half is real integration.

## RC-16 — Desktop launch/lifecycle integration (service, not checkout)

- **Required change:** Provide a runtime-owned way for the Desktop to discover/start the protocol service and interact with credentials without a hardcoded `<runtime-root>` checkout path or direct DB access, including shutdown/restart semantics (desktop exit must not stop task execution; runtime restart must be survivable).
- **Reason:** Desktop lifecycle must not own Runtime task execution. Source shows API-lifespan drain/reconciliation, but actual Task survival after Desktop exit and runtime restart was not exercised. Existing launcher topology is not proof of the final lifecycle contract.
- **Owner:** Runtime.
- **Proposed API:** A documented service contract: install location or service registration, `Hello` readiness, credential bootstrap for the desktop principal (RC-02), graceful stop semantics, and a statement of which restarts preserve task execution (drain recovers ASSIGNED/ACTIVE tasks; §6 of the survey) and which lose observation state (RC-04/05).
- **Compatibility:** Additive; the batch launchers remain for the browser workflow.
- **Migration:** Stage 1: document the current launcher as the only supported startup and require Desktop discovery handshake; Stage 2: a proper service wrapper. Desktop must never launch Python from a hardcoded path.
- **Desktop dependency:** `apps/desktop` config/validation + `cos-client` connection lifecycle; bootstrap uses an injected fixture endpoint.
- **Priority:** P0 for real integration (no bootstrap substitute).
- **Bootstrap vs real integration:** Cannot be bootstrapped; it is the definition of real integration.

## RC-17 — Task-scoped resumable SSE adapter (defer until after RC-05)

- **Required change (if selected as framing):** Carry RC-05's atomic snapshot/delta observation over task/principal-scoped SSE. SSE is a transport encoding of deltas, not an alternative source of progress or a second observation stream.
- **Reason:** Today's only stream is the legacy in-memory bus with no task correlation and no resume protocol (survey §11, §16.1). The Desktop owner's chosen direction (2026-09-21) is loopback HTTP first with a future task-scoped resumable SSE adapter; building it before the durable journal would create a second, non-authoritative observation path and contradict the Draft's atomic-session model.
- **Owner:** Runtime.
- **Proposed API:** One OpenObservation session with replay position and complete snapshot followed by delta transactions, encoded as bounded SSE events. A separate Subscribe convenience function must use the same reserved session/cut; never race a fresh subscription against a prior snapshot.
- **Compatibility:** Additive; the legacy `/api/events` bus stays for the legacy loop and is not repurposed.
- **Migration:** Only after RC-05; otherwise not at all.
- **Desktop dependency:** A real streaming binding is required; SSE is the current candidate, replaceable by another binding with identical guarantees.
- **Priority:** P1 if SSE is selected for real integration; no bootstrap transport implementation.
- **Bootstrap vs real integration:** Real only.

## Identified gap — Runtime intervention/attention projection

**Status: Proposed / unapproved; no new RC ID or wire field.** Product Canon §18 requires Runtime-provided actionable issues. The current Desktop `TaskSnapshot`/`ProjectionState` and wire DTOs have no issue collection; the historical Runtime survey's task `issues` path is not a Desktop contract. Runtime and protocol owners must revalidate current source, choose the existing read/observation adapter or a separately approved request, and freeze identity/severity/visibility before implementation. Recommend reusing an authoritative existing read projection through the approved observation adapter, not creating an independent event bus or Desktop-derived severity.

MP-NAV1/MP-INS3 can implement only the typed, labelled Desktop-local presentation conditions frozen in their DC contracts. Their Runtime-provided issue/attention acceptance remains blocked, as does that part of MP-ACC1/2; no scope downgrade, new endpoint, schema or Runtime work is authorized here. DD-26 still belongs to Product/UX for presentation priority; it cannot approve Runtime issue semantics.

---

## Do-not-build list (explicitly rejected / excluded)

- Do not import existing browser-only operator controls or legacy workflow-map editing into Desktop bootstrap. Desktop Flow/DMLG inspection remains a canonical product requirement; existing frontend exclusions do not remove it.
- Auto-retry of mutating commands, optimistic mutation of canonical state (`DESKTOP_PROTOCOL.md` §3; `FRONTEND_POLICY.md` invariant "projection plus explicit commands").
- A second observation path that bypasses the journal/session (RC-17 constraint), and any client-side fabrication of formal selection, winner or outcome.
- New queues/schedulers/executors in the HTTP handler lane (`ORIGEN-COS-FRONTEND-EXECUTION-WIRING` closure); the single drain consumer remains the execution owner.

## Revalidation

Before executing any change request:

1. Re-run the survey revalidation procedure (`./CURRENT_COS_RUNTIME_SURVEY.md` §18) and confirm HEAD/clean state.
2. Re-confirm the specific anchors each RC cites (symbols, not line numbers).
3. Confirm the prohibiting contracts are unchanged (`contracts/ORIGEN-COS-*.yaml`, ADR-012, ADR-005 for migration rules).
4. Any runtime change above must go through the runtime's own REOS lane (contract packet → implementation → verification facts → closure); this document is only the request.

## Uncertainties

- The runtime may already plan several of these under its own roadmap names (e.g. `cosd`, projection engine); this register asserts gaps in the surveyed revision, not absence of runtime plans. Runtime owners should re-map RCs to their existing packets before duplication.
- Other API shapes remain Draft; the bounded MP-PUB1 handoff above is SPEC_APPROVED. [Protocol ownership](PROTOCOL_OWNERSHIP.md) keeps one Rust-type machine-readable source; this request register is not an independent schema or Runtime implementation permission.
- RC-05's implementation choice (journal vs derived outbox) is deliberately left to the runtime; the requirement is the guarantee, not the mechanism.
- Retention/pruning for RC-08 interacts with the immutable-tombstone design (`trg_cos_commands_no_delete`); any pruning design must be explicitly approved by the runtime owner.
- Graph/media Runtime changes are gated by real feature integration, not fixture complexity. Bootstrap requires no Runtime implementation of these requests.
- Scoped MP-PUB1 contract decisions are ratified, but no Runtime implementation, scheduling, deployment or real verification is claimed by this task.
