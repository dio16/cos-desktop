# COS Desktop — canonical component contracts

**Status:** Frozen Desktop-side **design specification**. Provenance: initially transcribed from the plan at `f3ea37eaba8ed36551b2cfa3d8b370cd01956d70`, with review-round-8 history recorded in `70df916db98aec87845dba2f1e3678b492f4dab7`; this updated specification is not itself externally reviewed by those historical sources. The **DD design gates** were approved, the **R0D numeric gates** were ratified on a **declared-value basis** (not measured), and the **MP-VID1 capability probe** was authorised by owner ratification on 2026-10-24 ([ADR-0008](../decisions/ADR-0008-OWNER-RATIFICATION.md)). Each contract's own behavioural text is unchanged, and a question the documents do not answer stays open. **Unchanged by that ratification:** Runtime (RC-*) acceptance and real-Runtime evidence still require the Runtime owner and an operative Runtime, and MP-VID2–VID4 remain unauthorised until the VID1 probe reports. Historically, before that ratification, DD/RC/R0/UX/video gates remained approval-pending; requirements are not proof of implementation. Product authority remains the sole [Product/UX Canon](COS_DESKTOP_PRODUCT_UX_ARCHITECTURE.md); cross-cutting ownership and decision register are in [Desktop Design Canon](DESKTOP_DESIGN_CANON.md).

**Single-owner rule:** The 62 `DC-MP-*` sections below own scope, behavior, public-API proposals, failure, rollback and observable acceptance. The [Implementation Master Plan](../development/IMPLEMENTATION_MASTER_PLAN.md) is a **derived execution index** for task order, prerequisites, DAG, scheduling, evidence and blockers. Every task card links exactly one contract; disagreements are a stop-and-reconcile condition. No new task was authorized and no Runtime API was made real.

**Applicable inherited invariants:** Runtime alone owns canonical Task/evaluation/formal outcome/artifact identity/graph/plan/stop completion; UI rendering and presentation-local selections never invent these facts. UI never blocks on IPC/decoding/search. Fixture cannot stand in for real integration. No silent fallback, unbounded resources, insecure logging, automatic unsafe command retry, `unsafe`, new dependencies or pin changes. Apply accepted rules in [Repository Architecture](REPOSITORY_ARCHITECTURE.md), [Dependency Rules](DEPENDENCY_RULES.md), [Process Model](PROCESS_MODEL.md), [Protocol Draft](DESKTOP_PROTOCOL.md) where approved, [Rendering Architecture](RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) and [ADRs](../decisions/ADR-0007-RENDERING-PERFORMANCE-VISUAL-ARCHITECTURE.md).

**Evidence ownership:** test commands, artifacts, task priorities, approval status and actual PASS/FAIL/NOT_MEASURED are tracked by the implementation plan and task execution report. The assertions below are forward-looking acceptance contracts, **not test results**.
**Common contract clauses (inherited defaults; apply unless a contract below overrides them):** compressed contracts reference the clauses below instead of restating them. Tasks where defaults are insufficient restate their own failure, rollback and stop conditions: DC-MP-COR1, DC-MP-PUB1, DC-MP-RT1–RT8, DC-MP-SEC1–SEC3, DC-MP-REL1–REL3, DC-MP-ACC1–ACC2.

| Clause | Default for every contract |
|---|---|
| Authority | Runtime alone owns canonical state and completion. |
| Thread/ownership | UI thread does input/layout/paint/focus only; I/O, serialization, decode, layout, search and derivation run on bounded background executors with cancellation; no new global state. |
| Failure | Fail closed and honestly: explicit error/unavailable state; never fabricate success, never silently substitute data; never auto-retry mutating commands. |
| Security | No credentials/tokens/signed locators/private paths in logs, diagnostics, projections or errors; external media/metadata is untrusted. |
| Rollback | Revertible without restoring known-invalid behavior; if revert reintroduces a correctness defect, disable affected path (bounded degraded/unavailable state); see DC-MP-COR1. |
| Stop/escalation | Stop and escalate if work requires new public API, state-ownership change, open UX default, Runtime semantic, dependency/pin change, unsafe-code exception or persistence change; canonical decision register §6 governs approvals. |
| Evidence | Each task records command/run, environment and observed result; unmeasured items are labelled `NOT_MEASURED`. |

## Windows native-requirements crosswalk

This compact acceptance crosswalk is not a second platform specification. Product Canon §§52–53 and §§60–68 remains authority; the single listed DC owns detailed task behavior. All rows are Product requirements for the Windows-first MVP unless explicitly marked conditional/open; unresolved UX is not silently deferred or invented. Acceptance evidence is native Windows on the supported configuration, not fixture/screenshot-only evidence. AX/DPI evidence belongs to MP-AX2/MP-ACC2; actual supported matrix/release claims remain DD-23 pending. These are requirements, not test results.

| # | Requirement | Owner (MP / DC) | Product source | MVP / gate status | Observable acceptance | OS boundary / native evidence | Approval / open condition |
|---:|---|---|---|---|---|---|---|
| 1 | Drag and drop | MP-NAV5 / DC-MP-NAV5 | §52 | MVP native input; payload/use semantics open | Supported drop reaches validated app-core intent; unsupported payload rejected visibly; no path executed as command | OS drag/drop payload boundary; native drag, cancel, malformed/unsupported tests | Product/UX settles accepted payloads |
| 2 | Clipboard | MP-NAV5 / DC-MP-NAV5 | §§21, 52 | MVP; copy/paste inventory open | Copy exposes only supported selected value; paste validated and causes no canonical change absent explicit command | OS clipboard; native Unicode/empty/large/denied tests | DD-05 palette/keyboard inventory and Product/UX action choice |
| 3 | Native file picker | MP-IMG1 / DC-MP-IMG1 | §52 | MVP explicit local input; import scope/formats open | Cancel works; selected file bounded/validated and local/fixture-labelled; no arbitrary metadata path | OS picker/file-handle; native cancel/permission/unsupported/oversize tests | Product/UX settles import/export; cannot bypass descriptor security |
| 4 | Fullscreen | MP-IMG5 / DC-MP-IMG5 | §§20, 52–53 | MVP Artifact control | Enter/exit preserves viewed identity, focus/selection and artifact state | Window/display manager; native toggle/escape/restore tests | UX-01 if compare/fullscreen semantics change |
| 5 | Multiple monitors | MP-REC3 / DC-MP-REC3 | §§52–53, 65 | MVP recovery | Reopen/move to another display is usable; bounds clamp to present monitor set | Windows topology; native detach/reorder/removed-monitor run | DD-23 matrix pending |
| 6 | Mixed-DPI and DPI migration | MP-AX1 / DC-MP-AX1 | §§51–52, 65–66 | MVP quality; supported matrix pending | Supported mixed-DPI move preserves usable hit targets, popup, text, anchor and selection; unsupported combinations documented | Per-monitor DPI/window scaling; native 100–200% and mixed-scale tests | DD-04b typography + DD-23 release matrix pending |
| 7 | IME | MP-AX1 / DC-MP-AX1 | §§51–52, 65 | MVP text entry | Japanese IME composition/commit/cancel preserved; partial composition never dispatches | Windows text-input/IME; native composition sequence test | No new product default |
| 8 | Native notifications | MP-INS3 / DC-MP-INS3 | §52 | Product requirement; types/permission UX open | Supported notification reflects known state without inventing canonical fact; deny/unsupported remains honest | Windows notification permission/service; native grant/deny/failure | Product/UX approves content, triggers and opt-in; Runtime issue feed absent |
| 9 | Deep links | MP-NAV3 / DC-MP-NAV3 | §52 | Requirement; routes/allowlist open | Approved link opens only authorized existing entity/view; malformed/unauthorized activation fails closed without command | OS URL activation boundary; native valid/malformed/untrusted tests | Product/UX route + security allowlist approval |
| 10 | Window restoration | MP-REC3 / DC-MP-REC3 | §§52–54, 68 | MVP | Restore closed local preferences only; missing identities explicit unavailable; geometry clamps to current displays | Per-user preferences/window manager; restart/topology-change test | Existing closed fields; no canonical persistence |
| 11 | Single-instance handling | MP-RT8 / DC-MP-RT8 | §§52–53 | Requirement; second-launch behavior open | Second activation routes to existing process or bounded honest failure; no competing Runtime owner | Windows process activation; concurrent launch/activation test | Product/Runtime lifecycle owner approves; no Runtime launch authority inferred |
| 12 | Crash diagnostics | MP-SEC2 / DC-MP-SEC2 | §§52, 58–59 | MVP bounded/redacted invariants; contents open | Bounded report excludes credential, private path, signed locator and payload | OS crash/error-report boundary; synthetic-secret crash-path test | DD-22 retention/export details pending; invariants apply now |
| 13 | File association where useful | MP-REL1 / DC-MP-REL1 | §52 | Conditional (“where useful”) | If approved type exists, explicit open reaches validated import; unsupported type never executes | Windows association/activation; install/open/uninstall tests | Product/operator chooses useful formats and consent |
| 14 | Signed installer | MP-REL1 / DC-MP-REL1 | §§60, 68 | MVP signed package | Authentic signature/publisher/version verified; install/start without Runtime path coupling | Windows installer/UAC/signature; clean install/upgrade/signature failure | DD-21 identity/format/operator authorization pending |
| 15 | Portable archive | MP-REL1 / DC-MP-REL1 | §60 | MVP alongside installer | Runs without install; does not silently persist credentials or imply Runtime portability | Filesystem/profile boundary; clean-user no-install run | DD-21 distribution/operator approval pending |
| 16 | Update | MP-REL2 / DC-MP-REL2 | §61 | MVP contract; distribution channel open | Check/download/stage; switch only after signature and protocol compatibility verification | Update service/process replacement; invalid signature/incompatible version tests | DD-21 channel/signing decisions pending |
| 17 | Rollback | MP-REL2 / DC-MP-REL2 | §61 | MVP updater contract | Failed update leaves prior version usable, no half-switched installation | Installer/update filesystem; forced failure/recovery run | DD-21 gates; no claim before native evidence |
| 18 | Uninstall and cleanup | MP-REL1 / DC-MP-REL1 | §§60, 68 | Required distribution behavior; cleanup policy open | Remove approved app-owned binaries/associations, disclose retained user data; never delete Runtime data/tasks | Uninstaller/user-data boundary; uninstall/reinstall test | Product/operator approves retained prefs/logs |
| 19 | Shutdown | MP-RT8 / DC-MP-RT8 | §§52–53, 68; PROCESS_MODEL | MVP recovery invariant | Shutdown cancels Desktop work/flushes allowed prefs; no implicit Stop, Runtime kill or false settlement | Windows close/session shutdown; active Task/unknown receipt test | Task survival requires RC-16 and real integration |
| 20 | OS credential store | MP-SEC1 / DC-MP-SEC1 | §§55–56 | Required before production credentials; provider store only if approved | Store/retrieve/revoke securely; absent/revoked fails closed; never plaintext config/diagnostics | Windows secure-store boundary; native store/revoke/redaction test | DD-12 + RC-02 issuance/scope/revocation/operator approval pending |

Product §53's remaining window requirement maps to DC-MP-PUB1/MP-PUB1 (single core session/projection owner, never a window) and DC-MP-IMG5/MP-IMG5 (MVP primary window plus fullscreen Artifact). Detached Artifact, Compare and Flow windows are future scope explicitly stated by Product §53, not an agent-deferred MVP feature; if later authorized, DC-MP-PUB3 verifies shared authorized revision and independent local presentation. Existing multi-consumer tests do not authorize those product windows.

Unresolved decisions permit feasibility tests only within frozen boundaries; they do not authorize choosing UX, registering OS handlers, requesting permissions, distributing signed artifacts or claiming release behavior.


## DC-MP-COR1

**Contract:** Delta effective-state integrity (COR-01). **Execution ID:** [MP-COR1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** A published projection can never contain a dangling or reciprocal-inconsistent reference; the UI can trust `snapshot()` absolutely.
- **Scope:** Validate the **effective** transaction state before publishing: compute the candidate state = current state + all upserts − all removals of the transaction, then run the same aggregate rules `validate_integrity` applies (task↔iteration/candidate/outcome/brief/capabilities, candidate↔task/iteration/artifacts/evaluations, evaluation↔candidate/artifacts, iteration↔task/candidate ownership reciprocity, outcome↔task, graph node/edge integrity). Only on success publish (single swap) and advance the cursor; on failure invalidate with `ResyncReason::ProtocolViolation` and leave state/cursor/epoch untouched.
- **Non-goals:** no wire change; no new protocol fields; no change to duplicate/gap/epoch/predecessor logic; no "repair" heuristics; no weakening of the snapshot path.
- **Contract:** `handle_delta` gains a staged-validate step; rejection keeps `last_seen_cursor`, `projection`, `applied` and connection live-state semantics exactly as the existing rejection paths do; the code must reuse the shared integrity rules rather than duplicate a second rule set (a single internal function evaluated against a "view" of the state).
- **Authority invariants:** Runtime remains source of truth; client rejects transactions it cannot publish coherently (Protocol §2). A single aggregate validation owner/rule set is shared by snapshot and delta paths. Do not duplicate integrity rules in performance/measurement work.
- **Threading/ownership:** unchanged — runs inside `pump*` on background executor; staging and index preparation must use explicit fallible admission where recoverable failure is required. If such an operation reports failure before commit, reject without partial publication or cursor advance. A Rust allocation that aborts the process is not a recoverable transaction error; do not claim OOM recovery or impossibility, and do not add an injectable allocator/framework.
- **Failure/cancellation/recovery:** invalid effective state or reported precommit admission failure → reject/resync through existing bounded recovery path; repeated violations count toward limit; preserve last valid projection/cursor; no partial publish. Process-aborting OOM is outside recoverable-error guarantees.
- **Security/privacy:** rejection messages carry projection kind/identity only — never payload bodies or untrusted digests (SEC2).
- **Acceptance (observable) — each test names the bug it catches:**
  1. *Detects: an artifact removed while a candidate still references it (repro A).* Transaction = `Remove{Artifact}` referenced by a candidate ⇒ error event, connection leaves `Live`, `snapshot()` unchanged, `last_seen_cursor` unchanged, resync performed on the next pump.
  2. *Detects: reciprocal iteration↔candidate mismatch inside one transaction (repro B).* Two invalid upserts in the same transaction ⇒ rejected with identical invariants.
  3. *Detects: over-strict validation breaking legal rewires.* Transaction removes old iteration and upserts candidate with new `iteration_ref` ⇒ accepted; final state passes same aggregate validator.
  4. *Detects: legitimate updates or precommit admission failure breaking integrity.* Existing recovery/commands/fixture suites remain green; when an explicit fallible reservation/admission reports an error, no partial publication or cursor advance occurs. This does not claim recovery from process-aborting OOM.
- **Downstream consumers:** MP-PUB1, MP-DOCK1/2, MP-INS*, MP-IMG*, MP-FLOW*.

## DC-MP-COR2

**Contract:** Graph validation scaling (PERF-01). **Execution ID:** [MP-COR2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** Graph-bearing snapshots/deltas validate in near-linear time so ingestion cannot dominate large-graph workloads.
- **Scope:** build a deterministic node-key index per graph (ordered map/set unless measured need justifies hashing) for uniqueness and endpoint membership, as a performance implementation detail within COR1's single shared aggregate-validator owner. This is not a second rule set or validator; error messages/identities stay identical.
- **Non-goals:** no change to what is rejected; no caching across transactions; no new dependency; no benchmark framework.
- **Contract:** same public behavior; an optional bounded counter set (nodes, edges, index builds) may be exposed, but **completion does not depend on it**: correctness/scaling verification uses public-API behaviour comparison and external timing, so MP-COR2 is independent of the unapproved R0 harness. Wiring the counters into the R0 diagnostics surface is follow-up work owned by MP-R0A.
- **Threading/ownership:** unchanged (background).
- **Failure/recovery:** an explicit fallible reservation/admission error before commit fails closed through COR1's existing recovery path; a process-aborting Rust OOM is not recoverable and is not claimed tested. Preserve last valid state/cursor.
- **Security/privacy:** error text must not embed unbounded node lists.
- **Acceptance:** (1) *Detects: behavior drift* — for a generated corpus of valid and invalid graphs at the rendering §10 sizes, accept/reject results and error kinds are identical before and after the change (public-API comparison); (2) *Detects: quadratic validation cost* — measured validation time and traversal work grow near-linearly with edges at 10k and 50k edges, with the baseline measured on the pre-change code in the same session; (3) *Detects: nondeterminism* — repeated runs produce identical error ordering/identity. Internal node/edge/index counters instrument the cause only and are **not** the acceptance criterion (they may be removed without failing the task).
- **Rollback:** revert; validation remains correct but slower.

## DC-MP-PUB1

**Contract:** Observation session owner and atomic publication (ARC-01). **Execution ID:** [MP-PUB1](../development/IMPLEMENTATION_MASTER_PLAN.md).

**Approval:** Option B is `SPEC_APPROVED`; Desktop MP-PUB1 has scoped `IMPLEMENTATION_AUTHORIZED` in the [direct owner approval](../development/evidence/README.md#historical-records). The approval covers DD-01 and the bounded DD-12/13/15/19 read-boundary amendments, not a Runtime implementation, deployed protocol, product acceptance or later-step start. The reviewed proposal at `ca25e3586c6a0ebb61de624ba9bb157edc391bff` is historical provenance; this contract and the [Protocol](DESKTOP_PROTOCOL.md) now own the adopted requirements. STEP3 implementation status (Observed, not accepted): the owner/publication/read-boundary delta is implemented across desktop-protocol, cos-client, app-core, ui-model, desktop-gpui and binary composition, with fixture-transcript and native-shell evidence; A19 real authenticated Runtime integration is NOT_RUN (no verified adapter), and final product acceptance, merge and release remain with the Caller.

### Ownership and complete publication

- `app-core` owns exactly one process-wide `ObservationSessionOwner`, driven on the background executor. The binary constructs/injects the client and executor, starts/stops the owner and supplies the same handle to every surface. Windows never own/pump clients; UI Model and GPUI never call cos-client.
- Runtime alone authenticates principal/context, chooses read policy and per-command read permission, binds cursors and produces grant/invalidation/receipt decisions. `cos-client` validates authenticated channel plus exact current request/context/scope/epoch/read revision and owns projection, staging, cursor and the whole internal command registry. app-core applies this evidence, never its own ACL or a second registry.
- Each authorized immutable publication carries `snapshot`, `revision`, safe `connection`, visible `source`, `stale`, safe `last_error`, authorized command views, non-reused `session_generation` and `publication_version`. Fields are never independently swapped. Consumers reconstruct current authorized reconciliation from one newest record, not a specific event. Publication changes include preparation/send/registry/connection/error changes without `ClientEvent`.
- `revision` is the owner-local **projection-commit marker**, scoped by `session_generation`, not a Runtime entity projection revision, cursor, read revision or GraphRevision. The owner advances it only when a fully validated immutable snapshot handle changes; connection/command/error-only publications advance publication_version but not this marker. Keep one strong last-snapshot reference for identity comparison so allocator address reuse cannot create an ABA match; no deep projection comparison or max-of-entity-counter inference. Runtime per-entity revisions remain unchanged inside the DTOs. A withheld publication exposes no projection marker; restoring a compatible same snapshot may restore its marker under a fresh valid visibility tag.
- Unverified, awaiting/staging grant, revoked/changed/uncertain authorization and closed state admit no snapshot/revision/command data. Safe connection/source/error categories may be displayed; raw identity, grant, credentials, endpoint strings, hidden IDs and remote error text must not reach window/diagnostic APIs. Fixture mode is explicit and visible, with no automatic fallback.
- Window-local Viewed, Compare Selected, navigation, focus, zoom/pan, scroll and disclosure remain per-window; publication does not mutate them. Removed identities follow DC-MP-DOCK2. Primary and the existing fullscreen surface share the owner; no new detached-window product feature is authorized. Multiple consumers see complete versions; after processing the same latest notice their generation/revision/state agree, without requiring simultaneous paint.

### Authorization, invalidation and disclosure

- The [Protocol §2 authorized read handoff](DESKTOP_PROTOCOL.md#authorized-read-handoff) supplies exact Runtime-issued grant and per-ID decisions. Scope/Task membership, `Live`, generation, epoch, stored receipt and endpoint locality are not permission.
- A principal/authentication-context/read-grant/subscription change closes prior projection/receipt visibility synchronously before queued work or new-context data. Runtime invalidation is locally linearized at authenticated decode; channel/principal/auth-context loss detected by an adapter closes before new credentials/bytes or waking the owner. A wake alone is not closure.
- A mandatory `ObservationControl` gives client and adapter the same **close-only** capability for the actual app-core shared fence. It synchronously advances to a non-reused closed sequence, wakes subscribers and returns the sequence; no I/O, policy choice, deferred close or reopen. app-core supplies it before usable observation; there is no no-op default. During invalidation decode, the client closes before events, later frames or pump return, then cancels obsolete staging/lookups.
- Owner generation, local visibility sequence and Runtime read stamp are separate. Close advances to a fresh odd sequence; reopening CAS is allowed only from the exact expected closed sequence to its next even tag after a fresh matching grant and complete validated projection. Concurrent close makes reopening fail. A late old grant, lookup, derived result or owner cannot restore old visibility. Counters never wrap/reuse; exhaustion stays closed.
- All disconnect/reconnect intervals conservatively withhold public data. The same client/registry survives; fresh verified-channel request/grant is required. Only after exact reauthorization may a compatible retained projection be restored; otherwise obtain a complete snapshot. Epoch changes require Runtime-permitted snapshot/replay and never settle commands. No automatic command resend.
- Owner replacement closes, advances generation, cancels/joins the old pump, transfers the same client/registry and reauthorizes. No overlapping owners or replacement by an empty registry; failed transfer stays unavailable. Closing one surface only unsubscribes/releases its references. Final application shutdown closes first, rejects intents, cancels bounded work/deadlines, wakes subscribers closed, joins and disposes client state once; it never stops Runtime Tasks.
- Per-record private origin-context and current decision-tag metadata remain in cos-client, sharing context IDs rather than copying full grants per record. Local Prepared/Pending/receipt-free unknown lifecycle is readable only with exact validated origin context and Runtime local-intent permission. Receipt-bearing/server-derived state requires current explicit per-ID Allowed. A different context needs its own positive decision; Task overlap never suffices.
- Receipt read Denied exposes neither status/receipt nor retention/execution fact, and does not mutate command truth/capacity. Allowed/no server receipt returns the retained actual state when present; only current positive read evidence plus a true absent internal record returns NotRetained. Expiration/absence is not non-execution. Scoped read denial is routed outside generic command-execution failure handling. Actual authorized execution rejection remains a separate Runtime result.
- New `AuthorizedCommandReceipt` includes current positive per-ID permission, so authorized reconciliation publishes without a UI lookup click. Missing current evidence for a retained server-derived state sets a private `needs_read` bit; the owner fairly scans the bounded registry and automatically uses the same coalesced authorized lookup path. Unknown/Denied stays filtered; positive reply drives a whole publication. No second store or unbounded authorization queue.
- Automatic and explicit reads share at most 64 pending requests and the existing combined 64-reference budget; only admitted in-flight reads retain a reference, released on reply/cancel. Legitimately held references may delay automatic reads, never expand the bound. Denied is not polled again until a new binding or explicit intent; timeout preserves unknown and records a safe diagnostic without silent retry. Fresh binding/new genuine server evidence may initiate a new read. No stale Pending is presented as the current receipt-bearing state.

### Registry bounds and recovery

- **Ordinary unsettled:** at most `max_unresolved = 64` Prepared, Pending, Accepted-not-canonically-reflected and ordinary ResultUnknown. At capacity, reject before preparation/send; no tombstone borrowing, silent drop/eviction, resend or replacement ID. Do not use current `CommandStatus::is_settled()` for this classification.
- **Receipt tombstones:** an actual new untracked unsettled receipt takes a free ordinary slot, otherwise one of `max_tombstones = 64` disjoint slots. An ID occupies one class; repeats update the same entry. Combined unsettled maximum 128. A tombstone is actual `ResultUnknown { reason: "CommandReceiptTombstone", receipt }`, not fabricated acceptance/completion. At both caps, raise existing capacity error without overwriting tracked state or changing projection/cursor.
- **Settled/reference:** retain while surface-referenced plus at most 256 most-recently-settled entries; deterministic insertion-order eviction only at zero references. References are bounded at 64, over-cap retain errors, every retain pairs with release. Maximum distinct retained records is therefore 448; the protocol retained-ID request is bounded by this derived maximum, not a new allowance.
- **Capacity error:** existing `ClientError::InvalidCommand("CommandCapacityExceeded".to_owned())`, not a new variant/wire error; no capacity-induced resync. Persist while ordinary capacity is blocked; clear only on genuine ordinary-capacity recovery, final shutdown or later error supersession, never clearing unrelated error. Filtering does not free capacity; safe blockage indication exposes no hidden ID/status.
- Denial, disconnect, surface close, observation closure, resync, scope/epoch change and owner replacement neither settle nor clear unresolved state. Genuine settlement releases its class and preserves actual status under settled/reference rules. Local retention is not server receipt-retention/dedupe truth.
- Existing overflow/gap/integrity/backoff recovery remains owned by cos-client. Recovery is honest unavailable/stale/error, never partial/speculative state. Diagnostic ring is at most 1,024 with explicit `event_gap { from, to }`; it is not a correctness channel. Display notices coalesce to the latest version with at most one pending notice per surface, never delaying publication.

### Approved client and transport interfaces

The existing DesktopClient operations remain trusted client operations. The adopted additions are object-safe and GPUI-free. `pump`/`pump_bounded` may remain test helpers; they are not window execution paths. Whole-registry reads are internal retention facts, not a UI/protocol disclosure API.

```rust
fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport;
// PumpReport { frames: usize, events: Vec<ClientEvent>, changed: bool,
//              next_deadline_ms: Option<u64> }
fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus>;
fn lookup_command(&self, id: &CommandId) -> CommandLookup;
// CommandLookup::{Found(CommandStatus), NotRetained}
fn retain_command(&mut self, id: &CommandId) -> Result<(), ClientError>;
fn release_command(&mut self, id: &CommandId);
fn begin_authorized_observation(&mut self, request: OpenAuthorizedObservation)
    -> Result<(), ClientError>;
fn publication_access(&self) -> PublicationAccess<'_>;
fn lookup_receipt_authorized(&mut self, request: AuthorizedReceiptLookup)
    -> Result<(), ClientError>;
fn bind_observation_control(&mut self, control: ObservationControl);

// cos-client-owned composition value; no dependency back to app-core.
pub struct ObservationControl {
    pub wake: std::task::Waker,
    pub close_visibility: Arc<dyn Fn() -> u64 + Send + Sync>,
}
pub enum PublicationAccess<'a> {
    Withheld { reason: ReadUnavailable },
    Validated {
        stamp: &'a ReadStamp,
        scope: &'a ObservationScope,
        visibility_sequence: u64,
        readable_commands: ReadableCommands<'a>,
    },
}
```

`ReadableCommands` is a borrowed bounded iterator over existing per-record evidence, not another map. Its anticipated active visibility tag follows the close sequence for that request; only app-core can reopen after full validation. A borrowed view cannot authorize after its tag closes. `changed` covers all publication fields, not just events; pump is nonblocking and processes at most `budget` frames.

The existing Transport gains `fn bind_observation_control(&mut self, control: ObservationControl)`, forwarded from the client. Register before polling; drain a bounded step, recheck readiness/deadline, then yield so arrival during suspension registration is not lost. Arrival/output capacity wakes; detected auth/channel change closes synchronously then wakes. `send` enqueues bounded output without waiting on network/filesystem. Unsupported blocking/no-auth adapters stay unavailable. `next_deadline_ms` covers existing reconnect/pending deadlines; steady live idle with no input/intent/deadline does no periodic receiver work. No UI polling or new executor dependency is authorized.

### App-core consumer interface and atomicity

These are app-core-owned presentation contracts, not cos-client re-exports or new Domain definitions. Source/connection/errors are safe categories; receipt facts use direct authorized protocol DTOs. Command presentation maps Prepared→Prepared, Pending→AwaitingResponse, Accepted/Rejected→ReceiptAvailable with original receipt state, CanonicalReflected→CanonicalEffectObserved, ResultUnknown→OutcomeUnconfirmed, Unsupported/Failed→safe issue categories. It never derives Task Status, completion, winner or formal outcome.

```rust
pub struct ObservationPublication {
    pub snapshot: Option<SnapshotHandle>,
    pub revision: Option<Counter>, // local projection-commit marker, not Domain truth
    pub connection: PublicationConnection,
    pub source: PublicationSource,
    pub stale: bool,
    pub last_error: Option<PublicationError>,
    pub command_statuses: BTreeMap<CommandId, CommandReadView>,
    pub session_generation: u64,
    pub publication_version: u64,
}
pub enum PublicationConnection {
    Disconnected, Connecting, Observing, Recovering, Unauthorized, Incompatible,
}
pub enum PublicationSource { Runtime, Fixture, Unavailable }
pub enum CommandPresentation {
    Prepared, AwaitingResponse, ReceiptAvailable, CanonicalEffectObserved,
    OutcomeUnconfirmed, Unsupported, Failed,
}
pub struct CommandReadView {
    pub presentation: CommandPresentation,
    pub receipt: Option<CommandReceipt>,
    pub issue: Option<PublicationError>,
}
pub enum CommandRead { Found(CommandReadView), NotRetained }
pub enum CommandReadError {
    AuthorizationUnavailable, AuthorizationDenied, Superseded, CapacityExceeded,
    Client(PublicationError),
}
pub struct PublicationNotice {
    pub session_generation: u64,
    pub publication_version: u64,
}
// Opaque handles/leases have private storage and no untagged payload getter.
impl ObservationHandle {
    pub fn subscribe(&self) -> PublicationSubscription;
    pub fn current(&self) -> Result<PublicationLease, ReadUnavailable>;
    pub fn change_scope(&self, scope: ObservationScopeRequest)
        -> OwnerReply<Result<(), CommandReadError>>;
    pub fn authentication_changed(&self)
        -> OwnerReply<Result<(), CommandReadError>>;
    pub fn request_stop(&self, task: TaskId)
        -> OwnerReply<Result<CommandId, CommandReadError>>;
    pub fn read_command(&self, id: CommandId)
        -> OwnerReply<Result<AuthorizedCommandRead, CommandReadError>>;
    pub fn shutdown(&self) -> OwnerReply<()>;
}
impl PublicationSubscription {
    pub fn changed(&mut self)
        -> impl Future<Output = Option<PublicationNotice>> + '_;
}
impl PublicationLease {
    pub fn read<R>(&self, derive: impl FnOnce(&ObservationPublication) -> R)
        -> Result<TaggedRead<R>, ReadUnavailable>;
    pub fn is_current(&self) -> bool;
}
impl AuthorizedCommandRead {
    pub fn read<R>(&self, derive: impl FnOnce(&CommandRead) -> R)
        -> Result<TaggedRead<R>, ReadUnavailable>;
}
impl<T> TaggedRead<T> {
    pub fn consume(&self, use_current: impl FnOnce(&T))
        -> Result<(), ReadUnavailable>;
}
```

`OwnerReply` is a bounded one-response future. `read_command` returns no bare receipt/status: current positive decision and actual retention determine a tagged Found/NotRetained; uncertain/denied/superseded replies are separate errors. Tags are rechecked before completion, when the future is consumed and at lease use. Scope/auth-change calls synchronously close and wake before enqueueing I/O; newest observation replacement has one bounded overwrite slot. Ordinary command/read intents share a 64-entry mailbox and return explicit pre-dispatch capacity errors; already-dispatched commands are never overwritten/retried. At most one pending read per retained surface reference; duplicate ID/binding reads coalesce, no unbounded waiter list. All intents carry creation generation/visibility tags; stale Stop/read is rejected before dispatch rather than redirected into another principal.

The background owner assembles/filter/derives off-thread, then swaps one immutable Arc under a short mutex. UI reads use acquire sequence → `try_lock`/small Arc clone → acquire sequence, accepting only matching active record/tag/generation; contention yields transient unavailable and a wake, never main-thread waiting. Install a complete candidate with anticipated active tag while closed; CAS opens only the expected sequence, concurrent close defeats it. Local intent closes before queueing; authenticated decode/adapter detection closes within that path. Notifications carry version only; a closed fence wakes even if the version is unchanged, and consumers read fresh rather than skipping the wake by version equality.

Read derives a tagged value and checks again before returning. Retained DTO/string/media derivatives remain inside `TaggedRead`, never blessed by a newer unrelated lease. The GPUI controller revalidates at scene creation and immediately before cache install/paint; background results preserve tags; export/action consumer checks immediately before its effect. `consume` checks before its short callback: a prior close prevents invocation; a later close cannot undo an earlier authorized effect, and deferred work needs a new check. Trusted Rust callbacks can deliberately copy out through captured storage, so untagged retention is prohibited, not claimed impossible by the type system. A previously authorized copy/frame cannot be retroactively erased; unavailable is rendered on the next scheduled native frame. Remote revocation guarantee begins at local authenticated detection, not before network delivery.

- **Scope/non-goals:** implement this owner/publication/subscription and approved read-boundary delta, registry support, binary composition and native subscription. No Domain policy, duplicate service/store, new third-party dependency, Runtime implementation, full Artifact/Compare/Flow/video feature, detached-window UX or release change. Direct UI raw snapshot/status access and per-window clients are removed at cutover, not retained as bypass shims.
- **Acceptance:** independently authored Runtime/Fixture transcripts provide decisions; deterministic public-seam barriers provide race scheduling. Do not copy the production gate as an oracle or count incidental wiring/source/getter assertions. All cases below are required; real server claims remain separately NOT_RUN until exercised.

| ID | Required observable scenario |
|---|---|
| A01 | Same-scope reconnect withholds until exact reauthorization; unresolved state survives and no command is resent. |
| A02 | Principal switch with overlapping Tasks closes synchronously; old grant/state/reply cannot reopen; only B-authorized data is visible. |
| A03 | Same principal but changed authentication context rejects old context even with the same Task/scope set. |
| A04 | Revocation during staging/after Ready closes before later processing; no partial/old admitted read and no registry settlement; late equal/older grant rejected. |
| A05 | Scope shrink and expansion both close immediately; only exact new request/grant is used; window-local state survives without old content display. |
| A06 | Epoch change rejects old cursor/stamp, preserves unresolved Stop, restores only by Runtime-permitted grant/snapshot/replay. |
| A07 | Owner replacement has one owner, transferred registry and newer generation; old work/notice rejected; failed join stays unavailable. |
| A08 | Prepared/Pending/Accepted-not-reflected/ResultUnknown Stop remains capacity-unsettled through disconnect/revocation/surface close; acceptance never means completion. |
| A09 | Old receipt lookup Denied is the same public denial for retained/absent IDs, with no receipt/NotRetained/execution leak or execution-Failed mutation. |
| A10 | Current explicit Allowed restores actual retained unknown/receipt after reauthorization, including an explicitly permitted new context; no synthetic state. |
| A11 | Missing/unverified/auth failure admits no snapshot/revision/command data, with honest safe unavailable state and no Fixture fallback; valid evidence restores functionality. |
| A12 | With/without events, revision/connection/source/stale/error/command changes each produce one complete version, never mixed old-scope fields. |
| A13 | Barriers at grant/commit/CAS/read/delayed reply reject stale work; derive A then revoke and attempt delayed tagged install/render/export, including after B becomes Ready, is rejected. |
| A14 | Multiple GPUI consumers share complete latest generation/revision with independent locals; each rejects old tagged DTO/text/media/background caches; closing one does not stop observation. |
| A15 | Ordinary 63/64/65, tombstones 63/64/65, combined128, recent255/256/257 and refs63/64/65 prove no unsettled eviction/borrow, deterministic zero-ref settled eviction, true capacity recovery and no filtering-induced capacity release. |
| A16 | Allowed/no receipt+retained unknown gives actual Found; Allowed+absent gives NotRetained; Denied stays Denied; none implies non-execution/replay. |
| A17 | Missing/extra/duplicate decisions and wrong channel/scope/revision/request/unstamped data fail closed; command-specific read denial never leaks hidden IDs or becomes execution/session success. |
| A18 | Pause owner scheduling and pump after decode: detected adapter/client invalidation has already closed the actual shared fence before callback/pump return; suspend-race arrival, deadline, full mailbox, no periodic idle pump and shutdown obey bounds and no implicit Stop. |
| A19 | Real authenticated adapter/server proves identity, revocation ordering, audience-bound cursor and per-ID reads; if unavailable this boundary is NOT_RUN, never Fixture PASS or a new MP-PUB1 hard prerequisite. |
| A20 | Post-grant new command receipt with positive decision updates automatically without UI lookup; missing decisions use bounded fair lookup; more than64 candidates drain when references permit; Denied/timeouts do not loop or disclose protected payload. |

Frame/staging/rx limits remain Protocol-owned; grant IDs ≤448, pending reads/intents ≤64, replacement slot1, notice backlog1/surface, diagnostic ring1024. Include empty valid snapshots, duplicate/extra IDs, full queue during close, nonce/revision/generation exhaustion and last-subscriber removal. Native evidence shows live Fixture delta and withholding/recovery on actual GPUI consumers, local-state retention and responsive UI; shared-consumer API tests alone are not native proof. No new product surface/UX approval is inferred.

- **Failure/rollback:** client errors/overflow/integrity/backoff remain bounded and honest; no invalid partial publication or unsafe command retry. Unsupported/unverified real adapters stay unavailable. Keep the background owner: optional notification disable renders latest state at next natural interaction/explicit owner-routed refresh; disabling the owner is explicit unavailable/stale, never UI pump, periodic fallback or known-invalid frozen data.


## DC-MP-PUB2

**Contract:** Incremental window refresh and derived-view policy. **Execution ID:** [MP-PUB2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** Updates repaint only what changed; derivation scales to large collections.
- **Scope:** replace one-shot `WorkbenchText` derivation with revision-keyed derivation: derive display data per projection family/identity on the background executor; invalidate by (family, stable identity, source revision); never re-derive unrelated families (e.g. a task status change must not reformat graph text or decode media). Keep presentation types GPUI-free.
- **Non-goals:** no persistent collection library, no index, no cache design beyond the smallest revision-keyed maps until MP-PUB4 evidence; no second Domain model.
- **Contract:** derivation API takes `(&SnapshotHandle, revision, derivation scope)` and returns typed display records tagged with the source revision; the shell reuses unchanged rows by stable identity (element IDs must be scoped identities, never enumeration indices — this task removes the `("candidate", index)` pattern).
- **Threading:** derive off-thread; UI thread only consumes records.
- **Failure/recovery:** stale derivation results (older revision than currently published) are discarded; cancellation token per scope.
- **Acceptance:** (1) *Detects: unrelated updates causing unnecessary work* — task-only deltas preserve unrelated visible candidate/graph/evaluation content and satisfy the declared update workload; derivation counters are optional measurement evidence, not a private-helper call-count oracle; (2) *Detects: unstable identity after insertion* — inserting a candidate before the viewed one preserves its Viewed identity, focus and scroll anchor through the public presentation/native boundary, without asserting a private container/key layout; (3) *Detects: stale text after update* — displayed row content equals the newest authorized published revision. TESTING owns test admission and independent oracles.
- **Rollback:** keep one-shot derivation per family (still revision-gated).

**Observed implementation (2026-10-18, not Caller acceptance):** `ui-model::DerivationCache::derive_snapshot` consumes the existing app-core-visible `SnapshotHandle`, owner-local marker and generation scope. Current-snapshot-only display rows reuse authoritative per-entity revisions within a generation/epoch; the selected task is cached separately, and an unchanged complete marker avoids family traversal. Cancellation/withholding drops reusable state. `DisplayCache` admits the complete publication tag at install/render and rejects stale completions without erasing a newer current view. GPUI uses the full scoped candidate identity for element/focus state and restores the visible horizontal identity/offset after list changes. [Local quality](../development/evidence/README.md#historical-records), [actual normal-binary Fixture evidence](../development/evidence/README.md#historical-records) and [pixel comparison](../development/evidence/README.md#historical-records) record their own scope and limitations; independent Gate, remote review and Caller disposition remain separate.

## DC-MP-PUB3

**Contract:** Multi-window, generation and stale-result conformance tests. **Execution ID:** [MP-PUB3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** The publication contract is enforced by tests, not by convention.
- **Scope:** contract tests for: monotone revisions; generation rejection; atomic record consistency; event-drop accounting; consumer notification coalescing; multi-consumer independence; disconnect not mutating canonical state; reconnect restoring observation only.
- **Non-goals:** no UI behaviour epics; no benchmark.
- **Acceptance:** each named invariant has a test that fails if the invariant is removed (e.g. removing the generation check fails test 2). *Detects:* silent stale publication, lost notifications beyond the declared bound, canonical mutation on disconnect.
- **Rollback:** n/a (tests).

**Observed implementation (2026-10-18, not Caller acceptance):** the eight invariants were inventoried against the existing assertions before any test was written; every already-covered invariant keeps its existing stronger test and only real partials were strengthened (atomic record consistency, disconnect/reconnect preservation and no re-dispatch), plus two genuine gaps: an equal-version/new-generation notice for a coalescing consumer, and the inherited DC-MP-PUB1 diagnostic bound. That bound was unimplemented at the owner seam — `ObservationSessionOwner::step` discarded `PumpReport::events` — so `crates/app-core/src/diagnostics.rs` now provides the smallest safe accounting required by the existing clause: at most 1,024 category-only events, a monotone non-reusing sequence, an explicit `event_gap`, nondestructive per-reader cursors, and no identity, receipt, grant, cursor or remote text. Diagnostics never drive publication, authorization or command truth. Invariant removal was demonstrated by temporary single-line production probes (retention bound, gap accounting, generation coordinate, withheld disclosure), each reverted byte-exactly; [evidence](../development/evidence/README.md#historical-records). No Runtime/A19, UX or performance claim is made. Diagnostics *policy* — what is redacted, retained and exposed by product surfaces — remains MP-SEC2/DC-MP-SEC2-owned; this task implements only the bounded accounting the DC-MP-PUB1 clause already required.

## DC-MP-PUB4

**Contract:** R2 representation experiment (COW / index / cache / sharing). **Execution ID:** [MP-PUB4](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** Choose the smallest change that meets ratified update/tail/memory gates for 100/1k/10k/100k entities.
- **Experiment contract (fixed now):** identical valid transaction streams; vary one change at a time — (a) current `Arc<ProjectionState>` + `Arc::make_mut` baseline, (b) family-scoped immutable ownership, (c) task-owned entity secondary index, (d) revision-keyed display caches, (e) allocation-reduced text ownership; persistent collections only if a simpler option cannot meet the gates. Record: retained-handle COW frequency, bytes cloned (with accounting method), allocations, collection traversals, mutation/publication/derivation latency, memory high-water, stale-cancel counts, and changed-entity vs total-size curves. Include one modified/inserted/removed entity, bursts, and held vs promptly-dropped snapshot handles.
- **Non-goals:** no unconditional O(1) promise; no duplication of Domain state; no exposing partial family transactions; no change to atomic publication semantics from MP-PUB1.
- **Acceptance/exit:** simplest variant meeting ratified gates with no lost event/receipt, no stale overwrite, no partial publication; unchanged visible families avoid unnecessary derivation; if none passes, keep baseline and report the miss (escalate to rendering owner).
- **Rollback:** revert to MP-PUB1 publication on the existing `Arc<ProjectionState>`.

## DC-MP-R0A

**Contract:** R0 harness implementation. **Execution ID:** [MP-R0A](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** implement exactly rendering §10.1 (manifest fields incl. hashes/dirty state/build profile/display/power), rendering §11.1 endpoint table, rendering §11.2 JSON result contract, scenario families for implemented surfaces only; absent surfaces are recorded as `NOT_IMPLEMENTED/NOT_MEASURED` while keeping their definitions.
- **Non-goals:** no new generic benchmark framework/crate; no numeric SLO invented; no privileged cache reset assumed; no GPU counter claims beyond available endpoints.
- **Exit:** a replayer can reproduce aggregates from retained definitions/hashes; missing probes block their own claim only.

## DC-MP-R0B

**Contract:** Baseline capture, noise and endpoint availability. **Execution ID:** [MP-R0B](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** startup/idle/present baseline for the locked Release shell; baseline-vs-baseline noise; instrumentation on/off overhead; clock/endpoint availability table; cold/warm definition honesty (true cold = NOT_MEASURED unless a declared reset exists).
- **Exit:** noise floor and valid estimators declared; every missing endpoint listed, not zero-filled.

## DC-MP-R0C

**Contract:** Windows scheduling and stall attribution. **Execution ID:** [MP-R0C](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** the complete rendering §11.1 native stress matrix — continuous keyboard/held-key, pointer movement, scrolling, popup interaction and entity-update bursts (individually and in combination), **controlled posted/foreground-task saturation with declared traffic and cadence**, achieved producer cadence vs requested cadence, demand-vs-present correlation via local frame serial, gap detection with exploratory detector settings, bounded queue/cancel/restoration policy, and attribution to message servicing / frame processing / submit / Present / composition. Idle/occluded/suppressed spans are classified separately.
- **Exit (rendering §11.1):** no unexplained prolonged stall under ratified visible-demand coverage; no busy-spin; discrete key/focus/IME/command semantics preserved; missing present observation keeps the stall gate unverified.

## DC-MP-R0D

**Contract:** R0 ratification record. **Execution ID:** [MP-R0D](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** ratify which rendering §10 hypotheses become gates, with run counts, estimator validity and per-endpoint availability; publish the ratification in rendering §10 (single numeric owner — this plan copies no numbers).
- **Exit:** ratified gates + explicit NOT_MEASURED list; downstream tasks may then cite gates instead of hypotheses.

## DC-MP-UX1

**Contract:** Operable prototype of the Golden Workflow surfaces. **Execution ID:** [MP-UX1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** Validate comprehension *before* production surfaces (canon §65, §67 steps 4–7).
- **Scope:** operable prototype covering: task status in 5 s, artifact dominance, candidate loop comprehension, viewed vs formal selection distinction, evaluation→evidence navigation, compare reachability, flow on demand, intervention comprehension.
- **Non-goals:** no production-quality implementation; no canonical semantics in the prototype; prototype must not become the shipping UI without passing UX validation.
- **Acceptance:** validation record with each of the 8 comprehension checks, per-participant observations, and explicit failures; *detects: silent misinterpretation of viewed vs formal selection, artifact not dominant, compare unreachable.*
- **Rollback:** prototype is throwaway.

## DC-MP-UX2

**Contract:** UX-01 decision register closure (compare & viewing). **Execution ID:** [MP-UX2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Questions to answer (frozen list, no implementation until answered):** initial candidate to view (runtime-provided current vs explicit empty state); whether selecting a second candidate enters compare; zero/one candidate behaviour; pair creation semantics; linked-transform model (relative-to-fit vs physical 1:1) and default; fit/1:1 switching; cursor-anchor mapping and per-pane clamping; link/unlink transitions and what unlink restores; swap semantics (pane transforms vs swap-and-keep); pair replacement retaining link state; "restore previous view" definition; keyboard/focus transfer into/out of panes and across swap/removal; unavailable-artifact presentation per representation; same-key resource reuse visibility.
- **Fallback if unanswered:** Compare remains `DESIGN_READY_NOT_IMPLEMENTED`; no implementation may invent a default.

## DC-MP-UX3

**Contract:** Visual direction and token validation plan. **Execution ID:** [MP-UX3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** select/ratify token candidates in rendering §9 (typography stack, type scale, geometry, semantics, motion) — Proposed values only become Accepted through this decision plus native evidence.
- **Non-goals:** no canon change; no per-widget colours; no bundled font without license clearance.

## DC-MP-UX4

**Contract:** Keyboard map and Command Palette inventory. **Execution ID:** [MP-UX4](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** decide the command palette inventory and default keymap (canon §21), including virtualized-list navigation, compare actions, inspector/flow jumps; record which actions require a capability gate.

## DC-MP-TYP1

**Contract:** Token, typography and geometry implementation. **Execution ID:** [MP-TYP1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** implement the ratified tokens; mixed Japanese/Latin shaping with the ratified fallback stack; tabular numerals for aligned numerics; 1-physical-pixel separators; focus outline distinct from separators; DPI/text-scale invalidation that preserves focus/selection/scroll anchor.
- **Non-goals:** no font bundling without license decision; no per-glyph fallback hacks; no layout system beyond GPUI primitives.
- **Acceptance (R1 exit, rendering §13 R1):** no clipping/missing glyphs/focus/contrast failures across the declared DPI matrix (100/125/150/175/200 %) and mixed-script goldens; text/layout frame cost within ratified gates; *detects: missing-glyph boxes in Japanese fallback, sub-pixel divider blur at fractional DPI, focus outline lost on high contrast.*
- **Rollback:** revert token set to previous constants without dropping information/accessibility.

## DC-MP-TYP2

**Contract:** Colour, motion, icons and depth policy. **Execution ID:** [MP-TYP2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** semantic palette application with state coverage (disabled/focus/error/selection/high-contrast), reduced-motion honouring, one-shot motion tokens only, icon strategy chosen by measured rendering (vector vs raster atlas vs SDF) with DPI/scheme/device invalidation.
- **Icon selection criteria (fixed):** render the ratified icon set at the declared sizes (16/20/24 DIP) and DPI matrix; adopt the strategy with (i) no visible quality loss at 100/150/200 % (edge/legibility check, not pixel-exact matching), (ii) atlas invalidation cost bounded on DPI/scheme/device changes, (iii) measured GPU/frame cost inside the ratified §10 budgets. If two strategies tie on (i)–(iii), prefer the simpler one (vector, then raster atlas, then SDF).
- **Non-goals:** no idle/looping animation; no acrylic/live-blur default; no shadow without measured popup budget.
- **Acceptance:** *detects: idle GPU work from animation, contrast regression on disabled/selected states, icon blur at 150 % DPI, popup cost outside ratified budget.*

## DC-MP-NAV1

**Contract:** Task Rail. **Execution ID:** [MP-NAV1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** virtualized rail (visible + bounded overscan), variable-height rows, async filter/search with supersession, stable task identity keys, keyboard traversal that moves the viewport and keeps active-descendant semantics, selection preserved across updates.
- **Frozen behavior (no implementation choice left open):**
  - **Row fields (only fields that exist in the current contract):** `display_id`, `title`, `task_status`, `inner_phase`, `iteration_index` from `TaskSnapshot`. Attention is **Desktop-local and presentation-only** (stale connection, unsettled/unknown command, unavailable reference), typed and labelled local; never a Runtime canonical fact. Runtime issue/attention projection does not exist; any required field needs a Runtime contract request, not an invented local field.
  - **Search normalization:** query is trimmed, case-folded (Unicode simple case folding) and compared as a substring; no diacritic folding, no fuzzy/typo matching in this task.
  - **Search fields:** display id, title (both from the projection). Nothing else, so no local index invents semantics.
  - **Ranking/order:** (1) exact display-id match, (2) display-id prefix, (3) title prefix, (4) title substring; within a tier order by task status (ACTIVE, ASSIGNED, COMPLETED, CANCELLED, Unknown last), then iteration index descending, then display id ascending. The same rule defines the unfiltered list order.
  - **Empty results / empty collection:** an explicit "no matching task" state that echoes the query, and a distinct "no tasks observed" state for an empty projection; the rail is never hidden and never falls back to a stale or synthetic list.
  - **Async search:** each keystroke supersedes the previous query generation; a late result for a superseded generation is discarded (never rendered).
- **Non-goals:** no eager materialization; no rail-driven canonical commands; no local status computation.
- **Acceptance (rendering §13 R3 tail + rendering §5):** *detects: 10k-row rail mounting all rows, focus loss when the focused row scrolls out, selection jump on insert/remove, search results from a stale query generation.*

## DC-MP-NAV2

**Contract:** Task creation (Creative Brief). **Execution ID:** [MP-NAV2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** central Brief surface composing the frozen field set; submission builds the existing `CreateTaskCommand` payload through `app-core`; unsupported capability ⇒ explicit unavailable state with reason; no optimistic task creation. The **Start** action and any **advance-to-next-iteration** control are *not* designed here: they are implemented only after DD-25 records whether an existing Runtime operation exists and which layer owns the call.
- **Non-goals:** no brief templates beyond the frozen set; no local task identity invention; no auto-retry.
- **Acceptance:** *detects: creating a task while capability is unsupported (must be blocked with reason), brief validation drift (required-field violations surface before send), optimistic insertion of a task that the Runtime later rejects.*

## DC-MP-NAV3

**Contract:** History and reopen. **Execution ID:** [MP-NAV3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** History uses only authoritative projections with stable continuation keys; “reopen” restores local view only; older data loads on demand with accessible loading/end states. Approved deep-link routing and OS activation are covered by [Windows crosswalk row 9](#windows-native-requirements-crosswalk); route/allowlist remain open.
- **Non-goals:** no local history store of canonical facts; no truncation presented as absence.
- **Acceptance:** *detects: client-side history invented from iteration numbers, absence shown for unloaded pages, reopen mutating canonical selection.*

## DC-MP-NAV4

**Contract:** Shell chrome: independent state axes and Stop. **Execution ID:** [MP-NAV4](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** render Task Status, Inner Phase, Iteration, Artifact Availability, Connection and Formal Outcome as independent labelled axes (no collapsing into one badge); Stop issues `request_stop` through `app-core` and presents prepared/pending/accepted/rejected/unknown/canonical-reflected distinctly from completion. Unknown/expired/lost receipt remains possibly executed; authorized receipt lookup/reconciliation and canonical reflection are distinct from accepted intent. Manual intervention is honest when lookup/retention/authorization is unavailable; never silently retry or mint a replacement ID.
- **Admission UX:** capacity-blocked submission/receipt handling has clear state and bounded diagnostics; preserve task context and command identity. Identify ordinary admission versus receipt capacity without leaking another scope. No eviction, implicit retry, or second Registry.
- **Indefinite blockage/recovery:** if Runtime receipt lookup/reflection is unavailable, retention expired/unknown, auth unavailable, or no canonical reflection arrives, remain Unknown/Unavailable indefinitely; do not report false settlement, even at final shutdown. Safe recovery depends on Runtime capabilities (RC-08/10/11) and authorized lookup/observation; unavailable capability means manual user/operator resolution, never client invention.
- **Acceptance:** *A — fixture:* assert each Stop lifecycle transition, capacity/backpressure, lookup/reconciliation and no false settlement from the fixture client. *B — native interaction (independent of RT4):* native Stop/admission/unknown/manual-intervention and accessibility paths remain responsive, accurately labelled and bounded; no Runtime success inferred. *C — Runtime:* authorized lookup and canonical reflection are proven only with Runtime receipt/Stop contract (RT4 plus applicable RC-08/10 and authorization gates); Unknown persists otherwise. *D — Product:* Stop/unknown-resolution experience is accepted only after C and applicable Product UX approval. Every tier detects accepted Stop shown complete, disconnect changing task status, missing outcome rendered failure, stale state unmarked, unknown hidden/false-settled/retried, blocked admission unexplained, old-scope receipt shown, unbounded diagnostics, or final shutdown treated as settlement.
- **Observed implementation (2026-10-18, not acceptance):** the shell/model implementation and [public-seam smoke](../development/evidence/README.md#historical-records) implement the independent axes and distinguish Stop lifecycle/admission/receipt-reference limits without mutating Task truth. [Gate R3](../development/evidence/README.md#historical-records) is `SUCCEEDED` on the repaired `2f89b178dcdfdcf6e44ef0746ff34f8190ea8733` (189 tests, zero warnings, native integrity verified), and [remote review R2](../development/evidence/README.md#historical-records) on that SHA is `0/0/0` with the two earlier findings closed. [Gate R4](../development/evidence/README.md#historical-records) `5a196893-66e8-4d7e-ac82-e9d210d05192` is `SUCCEEDED` on the tier B candidate `3052525396608b4d8c93b7d08d35cce287c8c9d4` (fmt, metadata, dependency self-test/rules, clippy, tests, build, head and git-clean all pass; ten outputs; cleanup complete; integrity verify successful). [Tier B native evidence](../development/evidence/README.md#historical-records) records 23 runs ([index](../development/evidence/README.md#historical-records), [manifest](../development/evidence/README.md#historical-records)). The provider live confirmation that held the work earlier is resolved: attempts 1–2/5–6 timed out (history) and attempts 7–8 succeeded, so the input-dependent matrix ran inside checked active grants — N02 first native Stop proof (one `event=command` record, `STOP_TASK`, one distinct id with occurrence 1, Task Status stays ACTIVE), N03a/N03b single-flight bursts, N04d/N04e/N04f accepted/rejected/canonical-reflected distinct from completion, N05 65 s unknown with one command and manual intervention, N07a/N08d explicit read across the timed revocation with no stale authorization, N10a A→B→A with one admitted Stop per window, N13a/N13b close before/after retain with no implicit Stop, N14 keyboard/focus/AX, N15b disabled Stop inert, and N06b/N09b 64 admitted Stops (64 events, 64 distinct ids, occurrence 1) with the 65th refused before dispatch and no re-send on retire/release. The input-free runs remain as recorded (six independent axes, absent values left `Unknown`, withheld authorization as `Unknown` plus `Recovering · stale`, the revocation/recovery trace, the projection-driven display switch, disconnected startup with the normal-close handshake), and the r1 accessibility defect (gated controls exposed with `IsEnabled=true`) is corrected and retested natively. Branches that stay unmeasured are listed explicitly: `NOT_RUN` for N07b/N08a/N09a/N10b (no native control surface can construct them) and `NOT_MEASURED` for N04a (Prepared never published), N11 (`session_generation` only changes in `ObservationSessionOwner::replace_client`), N13c (no native observer for the owner ledger) and the N08c NotRetained branch (it needs an identity absent from the local registry; the public seam proves it in `app-core`'s `a16b_reads_report_exact_found_not_retained_and_denied`). [Remote review R3](../development/evidence/README.md#historical-records) on `3052525` returned `HOLD 0 BLOCKING / 3 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE`; those three findings were closed in `a08005e`, A-NB-4 in `5fa85b3` (final confirmation `PASS 0/0/0`), and the tier B matrix review on `fc3976c` returned `HOLD 0/2/0`, whose two documentation findings are closed in this candidate. C/D remain conditional and `NOT_RUN`; no full NAV4 completion, Runtime success or Product acceptance is claimed.
- **A/B technical completion (2026-10-18, independent review — not C/D acceptance):** the [tier B matrix review](../development/evidence/README.md#historical-records) on `fc3976c` returned `HOLD 0/2/0` and its two documentation findings were closed, and the [final independent review](../development/evidence/README.md#historical-records) on `b9136b2` returned **PASS 0 BLOCKING / 0 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE**, closing A-NB-3 and approving **MP-NAV4 A/B TECHNICALLY COMPLETE**: A (fixture/headless) and B (Windows native, reachable range) are technically complete, the eight unreachable/unmeasured branches (N07b, N08a, N09a, N10b, N04a, N11, N13c, N08c-NotRetained) stay `NOT_RUN`/`NOT_MEASURED` with their recorded reasons, [Gate R4](../development/evidence/README.md#historical-records) remains `SUCCEEDED` on `3052525`, C (authenticated Runtime) is `NOT_RUN` and depends on RT4/RC-08/10 plus real authorization, and D (product acceptance) is `NOT_RUN` and depends on C and applicable Product UX approval. Product release and full acceptance remain unapproved.

## DC-MP-NAV5

**Contract:** Command Palette (implementation). **Execution ID:** [MP-NAV5](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** every major workflow is keyboard-reachable from one command surface (canon §21).
- **Scope:** palette surface with substring search over approved inventory; app-core-only dispatch; capability gating and reason; same confirmations as owning surface; focus restoration; AX names/roles; bounded results. [Windows crosswalk rows 1–2](#windows-native-requirements-crosswalk) own the native drag/drop and clipboard OS boundary; payload/actions remain open pending Product/UX decisions.
- **Non-goals:** no scripting/macro system; no new commands beyond the approved inventory; no palette-only capabilities that bypass surface rules.
- **Contract:** commands are typed descriptors `{id, title, category, availability(capability), enabled_reason, invoke(app-core)}`; the palette never mutates canonical state directly and never fabricates availability.
- **Frozen behavior:** matching is case-folded substring over command title, category and stable command id (exact id match ranks first, then title prefix, then substring, then category); no typo-tolerant/fuzzy matching in this task. Confirmation semantics are **identical to the owning surface**: the palette invokes the same `app-core` call with the same confirmation requirement — it never adds, skips, or weakens a confirmation. Empty states: "no commands match" (query echoed) versus "no commands available" (capability-driven) are distinct and explicit.
- **Failure/recovery:** dismissing restores focus and leaves no partial action; a command that loses availability between listing and invocation fails with the honest reason.
- **Acceptance (observable):** *detects: a canon workflow unreachable without the mouse, an action enabled while its capability is unsupported, focus lost after dismiss, palette results from a stale inventory after capability revocation, keyboard trap inside the palette.*
- **Rollback:** remove the palette surface; keyboard shortcuts remain functional.

## DC-MP-DOCK1

**Contract:** Candidate Dock. **Execution ID:** [MP-DOCK1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** virtualized dock; scoped stable identities as list keys (fix the enumeration-index IDs); metadata/placeholder first, media later; viewed/compare focus retained across insert/removal; iteration grouping from authoritative iteration records.
- **Non-goals:** no decode of all candidates; no score sorting as "best"; no local ranking.
- **Acceptance:** *detects: reorder/insert shifting viewed selection, index-keyed element identity, eager decode of non-visible rows, best-so-far recomputation from scores.*
- **A/B technical completion (independent review — not Runtime/Product acceptance):** product `f1d0db1406a01725ad4ff4e4ef1bef2e0f7d57e8`, [released independent Gate R2](../development/evidence/README.md#historical-records) `0a48ea6d-4090-48db-928a-e0eee5a2809a`, [source-bound Native crosswalk](../development/evidence/README.md#historical-records) and [remote R4](../development/evidence/README.md#historical-records) on `1be9ecd48d84ce49415c7216461b5c2ab675886f` establish **MP-DOCK1 A/B TECHNICALLY COMPLETE**, **PASS 0 BLOCKING / 0 ACTIONABLE NON-BLOCKING / 0 EXCESSIVE**. [The completion record](../development/evidence/README.md#historical-records) preserves model-only Compare proof, the client-rejected missing-iteration Native branch, absent scrollbar, no-effect edge drag, one-pixel outer-bound discrepancy and exact regrant/delta timing as explicitly reviewed qualifications. Failed/interrupted Native runs and prior Gates remain historical. A19 authenticated Runtime and Product UX acceptance remain NOT_RUN; DD/R0/release approvals are not inferred. The scope/non-goals/acceptance above are unchanged.

## DC-MP-DOCK2

**Contract:** Iteration/history timeline, viewed/compare persistence, unavailable policy. **Execution ID:** [MP-DOCK2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** authoritative chronological iteration/history list; persistence of viewed/compare references across snapshots; when an identity disappears: explicit unavailable state, never silent substitution, never a formal-selection change; removal of one compare side leaves the pane in an explicit unavailable state until the user acts.
- **Non-goals:** no inference of "current iteration" from local ordering; no canonical selection edits.
- **Acceptance:** *detects: compare silently swapping in another candidate, viewed identity lost on removal, history order derived locally.*

- **Determined behaviour (UX-01 closure, [ADR-0010](../decisions/ADR-0010-UX01-DOCK-CLOSURE.md); each line individually rejectable):**
  - **D-1 Initial view:** with no persisted reference the viewed candidate is *explicitly empty*; nothing is auto-selected and no Runtime "current" candidate is auto-shown.
  - **D-2 Cardinality:** zero candidates gives an empty dock and surface; one candidate allows viewing while Compare is unavailable **with a stated reason**.
  - **D-3 Pair creation:** the first explicit selection sets side A only when Compare is not active, the second sets B and switches the surface to Compare; afterwards a plain selection changes *viewed only* and a side changes only through an explicit change-side action.
  - **D-4 Viewed disappearing:** the reference is kept and the surface shows an explicit unavailable state until the user acts — no automatic clear, no automatic replacement.
  - **D-5 One compare side disappearing:** Compare stays open with that side explicitly unavailable; changing the side or closing Compare resolves it, never automatically.
  - **D-6 Resolution:** changing the affected side or closing Compare; if the same identity reappears while the reference is held it becomes available again, which is not a substitution.
  - **D-7 Close:** returns to the artifact surface with the *viewed* candidate and that candidate's own framing; compare-session transforms are discarded and no other history is replayed.
  - **D-8 Persistence:** REC3 stays authoritative (one viewed reference, all-or-nothing pair, documented identity rule); no per-task memory map in the MVP.
  - **D-9 Task switch:** the window carries one reference set; after a task switch the dock follows D-1.
  - **D-10 Timeline content:** only authoritative iteration records carrying candidate rows, in their authoritative order — no synthesised nodes, no local "current iteration" inference; the formal-outcome display belongs to the inspector.
  - **D-11 "History":** the per-task iteration timeline only; an application-level history surface stays with DC-MP-NAV3 and DD-07 (Runtime-contract-gated).
  - **D-12 Focus:** independent of viewed and Compare; a disappearing focus target moves to the nearest valid neighbour and never changes selection.
  - **D-13 Formal marker:** display only — clicking it issues no canonical command.

## Media input admission and integrity

This is the single cross-task owner of media trust/provenance, expected-digest status and admission boundaries. [Rendering Architecture](RENDERING_PERFORMANCE_VISUAL_ARCHITECTURE.md) consumes this contract for measurement; it does not define a second admission policy. Product media UX remains the Product Canon. These are unapproved requirements, not evidence that media/runtime integration exists.

| Input class / task owner | Provenance and trust | Expected digest / verification | Admission, failure and acceptance |
|---|---|---|---|
| Locally generated input — MP-IMG1 / DC-MP-IMG1 | Record local generator/source and manifest; never pass as a Runtime artifact or authorized external descriptor | Preserve expected digest provenance if available; verify only against a known expected value. No digest → not Verified | Bounded decode can proceed without a digest; observable provenance remains local/generated. Test rejects accidental canonical-identity or Runtime-authorization claim |
| Fixture input — MP-IMG1 / DC-MP-IMG1 | Fixture identifier and source manifest are explicit; fixture label persists in UI/test evidence; never implies real-server behavior | Fixture manifest digest is fixture evidence only; absent digest → not Verified | Bounded local path may decode; test prevents fixture-to-production/user-visible identity conflation |
| Authorized external descriptor — MP-RT6 / DC-MP-RT6 (authorization), MP-IMG6 / DC-MP-IMG6 (bytes) | Treat descriptor, locator, metadata and bytes as untrusted; enforce current principal/audience/scope/expiry and reauthorization; server remains authority | Expected digest must have authorized descriptor provenance; verify length/type/digest before decode when supplied. Missing expected digest stays not Verified; it is not permission to claim verification | Denial/expiry/auth change → unavailable; mismatch → quarantine/discard; cancellation and bounded transfer; tests cover expiry/revocation, mismatch, absent digest, cancellation and no unauthorized fetch |
| All decode inputs — MP-IMG2 / DC-MP-IMG2 | Declared codec/decoder/version; metadata never executable | Verification state is not inferred from codec/type success | Validate type, dimensions, bit depth, codec, colour profile/transform, orientation, region/sequential capability before allocation; checked max decoded size and scratch/intermediate budget; bounded/cancellable decode. Unsupported/oversize/corrupt → explicit preview or unavailable/quarantine, never unbounded fallback |
| Adversarial media — MP-SEC3 / DC-MP-SEC3 | External data/metadata adversarial, including lengths, paths and declared type | Verification does not replace admission; mismatch never accepted as success | Reject impossible/absurd dimensions before allocation; no path traversal, executable metadata or unchecked allocation; native/unit acceptance covers hostile metadata/size and failure without partial upload |
| Authorization boundary — MP-RT6 / DC-MP-RT6 | Sole external descriptor auth/re-auth/denial owner; no media/client duplicate decision | Authorization is independent of digest/content verification | Auth changes re-evaluate descriptor; denial yields unavailable-with-reason before bytes are exposed |
| Transfer/cache boundary — MP-IMG6 / DC-MP-IMG6 | `crates/media` transfers external bytes under authorized descriptors; no control-plane bytes | Verify when expected digest exists and preserve provenance; no expected digest means not Verified | Bounded concurrency/bytes/scratch, cancellation and quarantine; `cos-client` only carries DTOs; no URL-as-path/unrestricted redirect |

**Cross-boundary acceptance tests:** generated vs fixture vs authorized external provenance are distinguishable; absent expected digest is never called Verified but does not alone block safe bounded local decode; wrong type/dimensions/codec/colour/orientation are explicit; declared and actual allocation/scratch stay within bounds; work cancels; auth failure/revocation and expiry prevent fetch/display; digest mismatch quarantines; unsupported formats remain unavailable/preview; no media key/bytes enter `cos-client`; no media data reaches GPU before required admission. Each outcome maps to the single owner listed above; this table does not add public APIs, wire fields, codecs or Runtime capability.

Local/generated and fixture feasibility may proceed without Runtime. Production external bytes/full detail remain conditional on MP-IMG6 and applicable DD/RC authorization and product gates; absent Runtime support does not authorize an alternative fetch or auth path.

## DC-MP-IMG1

**Contract:** Media representation keys and local-input feasibility (no Runtime). **Execution ID:** [MP-IMG1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** Own local representation keys and local input feasibility independently of Runtime authorization; common boundaries are in [Media input admission and integrity](#media-input-admission-and-integrity).
- **Scope:** local input provenance (generated vs fixture), typed representation/cache keys and bounded local acquisition/cache only. Native file picker acceptance is [Windows crosswalk row 3](#windows-native-requirements-crosswalk); picker permission/cancel never bypasses validation. No production acquisition, external authorization or decode-admission policy.
- **Ownership boundary:** use the common table as sole cross-task boundary for `media`, `cos-client`, GPUI, IMG2/IMG6, RT6 and SEC3. `cos-client` carries descriptor DTOs only; never media keys or bytes.
- **Non-goals:** no network fetch, descriptor authorization, bytes in control channel, canonical identity from bytes or whole-artifact eager decode.
- **Acceptance:** common table's generated/fixture provenance, absent digest not Verified, bounded local feasibility and no media key/bytes in `cos-client`.
- **Physical local feasibility boundary (MP-IMG1):** `media` accepts an explicit owned file or reader capability on a background executor, not a path/URL/descriptor. Local manifest-scoped identity/revision, representation kind, requested dimensions/orientation and declared decode/colour policy form the key; canonical artifact keys retain server/epoch/reference/revision and are metadata-only here. Independent manifest expectations and computed SHA-256 remain separate. Encoded byte/entry reservations follow the last live consumer even after cache eviction. The isolated `cos-desktop` `media_picker` example exercises the pinned GPUI native picker; it is not a production import action. Its PNG signature recognition establishes encoded-container feasibility only, not image validity, decoder admission, Product format support or rendering. Import/export scope and formats remain Product/UX decisions.
- **Probe opened-object boundary (R1 correction):** the Fixture-only example pins an ordinary local root directory handle and rejects reparse/unknown roots. Safe Windows open options reject final-component reparse redirection; the opened file's metadata and handle-derived final path must agree with the pinned direct-root boundary before encoded reads or cache reservation. The reviewed Windows-only dev dependency `filepath = 0.2.0` supplies a safe final-handle-path API without own-code `unsafe` or a broad filesystem service. This is a narrow probe boundary, not a production filesystem sandbox or a change to media authorization.
- **Rollback:** remove the local-input path; nothing user-visible depends on it.

## DC-MP-IMG6

**Contract:** Production media acquisition through authorized descriptors. **Execution ID:** [MP-IMG6](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Purpose:** Fetch real artifact bytes for the representations defined in MP-IMG1.
- **Scope:** bounded/cancellable external byte acquisition only through authorized descriptors, with mismatch quarantine; cross-task ownership and admission acceptance are owned by [Media input admission and integrity](#media-input-admission-and-integrity). RT6 alone owns descriptor authorization/reauthorization; IMG6 does not duplicate it. RT2 transports DTOs only; IMG2 validates decode admission; SEC3 handles adversarial-input hardening.
- **Non-goals:** no URL-as-path, unrestricted redirect, control-channel bytes, Runtime storage access or second authorization decision.
- **Acceptance:** *detects: expired/unauthorized fetch; auth change not reevaluated; type/digest mismatch accepted; absent digest marked Verified; unbounded transfer; cancellation leak; quarantine bypass.*

## DC-MP-IMG2

**Contract:** Decode admission gate (R4 "Decode"). **Execution ID:** [MP-IMG2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** per-codec decoder admission only; implement declared media type, dimensions/bit depth/codec/orientation/colour handling, decoder/version, maximum dimensions, region capability, sequential/tiled behavior, scratch/allocation bounds and cancellation granularity per [Media input admission and integrity](#media-input-admission-and-integrity). IMG2 does not own descriptor authorization, external transfer/digest verification or SEC3 adversarial-input policy.
- **Candidate set (fixed for the comparison; decode stage only):** (a) Windows Imaging Component / GDI+ decode worker (the path used for the 16K measurement), (b) FFmpeg 8.0 as an external decoder harness (used only where a codec's decode behaviour needs comparison; product adoption of an external decoder is conditional on a separate dependency/licence review). The pinned GPUI image path is **not** a decode candidate here — its whole-image upload behaviour is measured in MP-IMG3 (Upload gate). Adoption rule: for each supported codec/dimension class, pick the candidate with the smallest declared peak scratch that still meets the decoded-output requirement; a candidate is rejected for a class if it cannot decode it, exceeds declared peak bytes, or cannot cancel within the declared granularity. The chosen set is recorded in the admission table (codec → decoder → max dimensions → region capability → peak scratch → cancellation granularity → preview/unavailable fallback).
- **Non-goals:** no silent full-size decode fallback; no unbounded scratch; no promise of partial decode where unsupported; no new dependency added without an approved decision.
- **Physical preflight implementation (MP-IMG2, decoder-agnostic slice):** `crates/media::decode` exposes `DecodeLimits`, `DecodeRequest`, `DecodeAdmission` and `admit_decode`. It validates caller budgets, dimensions against `max_dimension`/`max_pixels`, per-channel bit depth (1–16) and channel count (1–4), computes decoded bytes with checked arithmetic (bit-rounded), and rejects decoded or declared-scratch bytes above the caller ceilings before any allocation; cancellation is observed before and after the decision. It performs no decode, selects no decoder, claims no codec/colour/profile/region/sequential capability, allocates no buffer and touches no GPU object; decoder adapters must honour an admitted result. R0 measurement, per-codec comparison, decoder selection and every numeric budget remain separate decisions.
- **Acceptance (rendering §6.1 Decode):** common table's unsupported/oversize/corrupt input, checked allocation/scratch, cancellation and no-absent-digest-Verified scenarios; rendering §6.1 owns measurement criteria.

## DC-MP-IMG3

**Contract:** Upload gate (R4 "Upload"). **Execution ID:** [MP-IMG3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** prove the pinned public image/upload path: whole-unit vs supported partial update, pitch/alignment, staging lifetime, synchronization, cancellation, stale source/device generation, same-key reuse; fixed process-wide allowance implemented first; upload unit must fit the smallest active cap or use a proven bounded chunk API; publish only complete current-generation resources.
- **Non-goals:** no assumption that `draw_surfaces`/video surfaces work; no unsafe/low-level escape; no adaptive scheduler before evidence.
- **Acceptance (rendering §6.1 Upload + rendering §6.2):** *detects: oversized indivisible upload blocking the frame budget, per-pane duplicate upload of an identical key, uploading a stale-generation resource after device reset, retained CPU payload longer than declared.*

## DC-MP-IMG4

**Contract:** Eviction, lifetime and device generation (R4 "Eviction"). **Execution ID:** [MP-IMG4](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** repeated load→present→switch→evict→load with a fixed manifest (dimensions/formats/identities, same-key panes, window close, device loss); residency plateau proof within ratified bounds; retained bytes after eviction have an explained owner/lifetime; GPU resources tagged by device generation and recreated lazily (the tagging/lifetime contract lives here; the device-loss integration scenario and its acceptance are MP-REC1's).
- **Non-goals:** no claim that logical cache bytes bound decoder scratch/atlas residency; no freeing resources still referenced by an in-flight frame.
- **Acceptance (rendering §6.1 Eviction):** *detects: unbounded residency growth over switching loops, atlas entries surviving eviction, eviction dropping a live frame's resource, unexplained retained bytes.*

## DC-MP-IMG5

**Contract:** Full view, zoom/pan/tiles, physical 1:1. **Execution ID:** [MP-IMG5](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** bounded thumbnail → preview → visible-detail promotion; tile request/cancel keyed to viewport+zoom+revision; physical-pixel 1:1 with explicit logical↔physical conversion; orientation and fit/crop in display transform, not identity; downscale via mips, upscale without implying detail. Fullscreen behavior maps to [Windows crosswalk row 4](#windows-native-requirements-crosswalk).
- **Candidate set and adoption rule (tiling):** (a) whole-image texture (**reference baseline; rejected** for 8K-class sources by the measured residency evidence), (b) bounded tile-image pool with complete bounded tiles, (c) preview-only capability mode when no bounded path exists for a format. Adopt (b) when it meets the rendering §10 readiness/residency gates for the declared codec set with equivalent fidelity; otherwise adopt (c) and record the capability limitation explicitly (never present preview-only as full detail). Partial texture updates are not required if (b) passes.
- **Non-goals:** no promise that every format supports random-access tiles; no full-resolution texture for 8K/16K sources; no feature registration/auto-alignment.
- **Path condition:** local-input feasibility (tiling/preview/residency work) may proceed without Runtime; the production full-detail path additionally requires MP-IMG6 plus the MP-IMG5 UX-01 decisions, and acceptance is claimed only for the path that was actually measured.
- **Acceptance:** *detects: full-image GPU texture for an oversized source (the measured 16K whole-image path), tiles requested for non-visible regions, 1:1 that silently changes source sampling on DPI change, stale tiles arriving after revision change.*

## DC-MP-CMP1

**Contract:** Compare (R5). **Execution ID:** [MP-CMP1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** two-pane compare implementing the ratified UX-01 semantics; same-key panes share one decode/upload entry; different keys are separate; independent per-pane clamping; fairness across panes/windows under one process-wide budget; compare never touches formal selection.
- **Reuse/fairness comparison protocol (fixed):** identical camera/pan-zoom trace applied to (i) shared-resource path and (ii) independently decoded panes, same-key and distinct-key cases; record per-pane upload/decode counts, queue wait, frame-pacing distribution, peak residency and transform-coupling traces (does either pane's clamp alter the other's anchor?). Adoption rule: sharing is adopted only if the same-key case shows one decode/upload, no transform coupling, no pane starvation and no resource regression against the independent case; otherwise independent per-pane resources remain and the miss is reported.
- **Non-goals:** no synchronized audio/video playback before MP-VID4; no coupling of independent transforms; no cross-key dedupe assumptions.
- **Acceptance (rendering §13 R5):** *detects: duplicate decode of an identical key, one pane starving another under load, linked transform drifting because a clamped pane's anchor is fed back, compare mutating formal selection, unequal-DPI panes sampling differently.*

## DC-MP-INS1

**Contract:** Inspector (why is this artifact here?). **Execution ID:** [MP-INS1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** progressive disclosure of Goal/Evaluation (decision, axes, units, confidence, authority)/Evidence summaries/Refinement reason/Outcome; disclosed sections keyed by stable identity and revision; keyboard reachable; no interpretation beyond projection values.
- **Non-goals:** no local score interpretation, no "best" computation, no reading of evidence bodies here.
- **Acceptance:** *detects: missing score rendered as zero, absent outcome rendered as failure, evaluation attributed to the wrong candidate after reorder, disclosure state lost on update.*

## DC-MP-INS2

**Contract:** Evidence detail and authorization. **Execution ID:** [MP-INS2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** fetch evidence detail only on demand/near-visible, through authorized references; per-record unavailable states (pending/missing/forbidden/unsupported); truncation never presented as absence.
- **Non-goals:** no direct filesystem/database access to evidence.
- **Acceptance:** *detects: rendering a forbidden record as absent, fetching all evidence eagerly, stale detail displayed under a newer evaluation revision.*

## DC-MP-INS3

**Contract:** Intervention strip. **Execution ID:** [MP-INS3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** surface actionable issues from authorized projections, prioritize one, expose remaining count/list; never auto-raise destructive actions. Native notification behavior (conditional pending Product/UX consent/content decisions) is [Windows crosswalk row 8](#windows-native-requirements-crosswalk).
- **Frozen inputs (available data only):** the current protocol carries no canonical issue collection in `TaskSnapshot`/`ProjectionState`; version 1 renders only typed, labelled Desktop-local conditions (connection freshness, Stop/settlement state, unavailable/missing references). They never claim Runtime canonical issue/severity. Canon §18 Runtime issues remain a Runtime contract gap recorded in the Runtime Required Changes register.
- **Frozen ordering:** (1) Runtime-supplied severity/code when present, (2) task status order (ACTIVE before ASSIGNED before terminal), (3) stable issue identity — deterministic and declared. DD-26 (Product/UX owner) ratifies or replaces this rule before the strip ships.
- **Non-goals:** no issue invention from connection errors alone (connection is its own axis).
- **Acceptance:** *detects: stale issue persisting after resolution, issue priority invented locally, non-deterministic reordering between renders, dialog focus not restored after dismissal.*

## DC-MP-FLOW1

**Contract:** Graph ingestion and accessible inspection list. **Execution ID:** [MP-FLOW1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** render the authoritative graph projection with stable node IDs; provide the non-canvas accessible structure **first** (searchable node/edge list with keyboard navigation) so the graph is never canvas-only; display the exact `GraphRevision` identity (id+version+digest) currently rendered.
- **Non-goals:** no topology inference, no edge creation, no local revision generation.
- **Acceptance:** *detects: graph displayed for a stale revision without label, keyboard user unable to reach a node, canvas-only information, label derived from layout position.*
- **Physical in-process implementation (MP-FLOW1):** `ui-model` derives immutable `GraphDisplay` rows that retain the exact canonical `GraphRef` (`graph_id` + `version` + `digest`) separately from the authoritative projection revision, plus stable `GraphNodeDisplay`/`GraphEdgeDisplay` records carrying NodeId/EdgeId, semantic label/layer/kind and exact endpoint identities. A row is reused only when both the projection revision and the full `GraphRef` match. Filtered inspection rows are produced on the GPUI background executor by a cancellable worker and admitted only while the complete identity (session generation, publication version, epoch, exact revision and query) still matches; a superseded query, revoked publication or older revision yields no partial or stale rows, and keyboard inspection consumes only admitted rows without rescanning the graph. `desktop-gpui` renders the non-canvas Flow surface first: a searchable, virtualized node list and edge list, exact revision headers, explicit empty/updating/withheld/unavailable states, and keyboard cursors keyed by exact revision plus NodeId/EdgeId. Search filters only visible inspection rows; it never changes graph topology or identity, and no layout position is used as a label or identity.

## DC-MP-FLOW2

**Contract:** Culling and cached geometry (R6 stages A–C). **Execution ID:** [MP-FLOW2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** staged comparison on the same canonical graph/camera trace: baseline full path generation (A) → viewport culling with query overhead charged (B) → cached geometry + spatial index keyed by graph revision/layout version/style (C); record layout/query/geometry/paint/submit/GPU/Present separately.
- **Non-goals:** no custom element/painter before stages A–C fail (see MP-FLOW3); no stale-revision geometry reuse.
- **Acceptance:** *detects: culling that changes displayed topology, geometry cache reuse across revisions, measurement that attributes GPU time to CPU path generation.*

## DC-MP-FLOW3

**Contract:** LOD/batching and the custom-element decision (R6 stages D–F). **Execution ID:** [MP-FLOW3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** level-of-detail that preserves an equivalent full-inspection route; batched geometry; stage E viewport-transform-centred reuse limited by what the pin supports (retained GPU geometry is not guaranteed); stage F only after a demonstrated capability gap, with accessibility preserved.
- **Non-goals:** independent DirectX renderer, fork, pin change, unsafe code, GPU handles outside `desktop-gpui`.
- **Acceptance (rendering §8.1/§13 R6):** *detects: LOD hiding information with no inspection route, faster paint achieved by dropping labels/interaction, unsupported retention assumed.*

## DC-MP-RT1

**Contract:** Contract normalization and freeze (joint). **Execution ID:** [MP-RT1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** freeze required Desktop wire semantics: server identity/epoch, scope/cursor, projection revisions, structured errors/correlation, receipt retention advertising, capabilities, graph/plan inspection, media descriptor identity/expiry/audience, negotiation and lifecycle/readiness. Reuse canonical identities; no new Domain concepts. Detailed requirements are owned by this protocol draft and Runtime change requests, not inferred from an execution plan.
- **Acceptance:** compatibility matrix entries defined for each supported pair; unknown-value preservation rules stated (RC-14); *detects: adapter exposing non-canonical identities, silent semantic drift between Runtime and protocol types.*

## DC-MP-RT2

**Contract:** Real transport binding. **Execution ID:** [MP-RT2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** first binding per ADR-0005 direction (loopback HTTP against a Runtime-owned adapter) implementing the existing `Transport` seam: negotiation, observation open/resume, delta stream, command send, receipt lookup, media descriptor fetch; credentials from the OS secure store (MP-SEC1), never printed.
- **Non-goals:** no reuse of legacy generic `/api/events`; no second observation path; no client-authored epochs.
- **Completion boundary:** MP-RT2 completes when the transport *implementation* satisfies the frozen contract (negotiation code, observation open/resume, delta stream, command send, receipt lookup, descriptor fetch) against the fixture/loopback harness. It never authorizes a production-connection claim: real-server compatibility, refusal ordering and unknown-event behaviour are MP-RT7's acceptance, and production readiness may not be claimed before MP-RT7 completes.
- **Acceptance:** *detects: connecting without a negotiated pair, printing endpoint/credential values, treating a transport error as task failure, replaying a mutating command after a lost response.*

## DC-MP-RT3

**Contract:** Reconnect/resync/epoch campaign against a real server. **Execution ID:** [MP-RT3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** exercised scenarios: server restart (epoch change), gap creation, overflow, authorization change invalidating cursors, Desktop restart, long observation with bursts; assert state/cursor invariants and that local presentation survives where identities remain valid.
- **Acceptance:** *detects: cursor reused across epochs, silent gap acceptance, stale publication after reconnect, Real Task state changed by Desktop disconnect.*

## DC-MP-RT4

**Contract:** Receipt reconciliation and dedupe retention. **Execution ID:** [MP-RT4](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** implement lookup/retry policy from the advertised retention window; keep `ResultUnknown` until disambiguated; tie accepted stop to canonical reflection (RC-10) when available; no automatic resend, no new command ID to hide uncertainty.
- **Acceptance:** *detects: retrying inside an unknown-retention window, treating `ReceiptExpired` as failure, accepted stop rendered as completion.*

## DC-MP-RT5

**Contract:** Capability-driven gating with real grants. **Execution ID:** [MP-RT5](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** UI gates every command/media/feature on the authoritative capability projection; unknown capability ⇒ disabled with reason; no provider-name inference; capability revision changes re-evaluate gating.
- **Acceptance:** *detects: enabling an action from a provider name, stale capability grant after revocation, disabled UI treated as authorization.*

## DC-MP-RT6

**Contract:** Media authorization integration. **Execution ID:** [MP-RT6](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** sole external descriptor authorization interaction — validate identity/scope/audience/expiry against current Runtime-validated context, request reauthorization for expired descriptor, and transition denial to unavailable-with-reason. The common [Media input admission and integrity](#media-input-admission-and-integrity) table owns the task split; RT6 makes no byte-transfer, digest-verification, quarantine, cache or decoder decision.
- **Acceptance:** *detects: using an expired descriptor without re-authorization, an authorization change not re-evaluated, a re-authorization denial rendered as available, a denial that leaves the surface in a fabricated available state.* (Byte-level digest/type mismatch, quarantine and concurrency failures belong to MP-IMG6's acceptance.)

## DC-MP-RT7

**Contract:** Version negotiation with the real server. **Execution ID:** [MP-RT7](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** complete the negotiation half that fixture tests cannot prove: incompatible ranges refuse before side effects, negotiated pair fixed for the session, unknown state-changing events force resync.
- **Acceptance:** *detects: silent downgrade, side effects before negotiation failure, unknown state-changing event ignored.*

## DC-MP-RT8

**Contract:** Service lifecycle and discovery. **Execution ID:** [MP-RT8](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** Desktop discovers/attaches to Runtime-owned service without hardcoded checkout path; documents what survives Desktop exit/crash/update and Runtime restart; no Desktop-owned kill-on-close integration. [Windows crosswalk rows 11 and 19](#windows-native-requirements-crosswalk) own single-instance and native shutdown acceptance; neither grants Desktop Runtime-process ownership.
- **Acceptance:** *detects: Desktop spawning a Runtime process from a hardcoded path, task execution terminated by Desktop exit, credentials in config files.*
- **Physical Desktop-local implementation (Native acceptance pending):** `apps/desktop::instance` holds an exclusive OS file lock on `%LOCALAPPDATA%\cos-desktop\desktop.instance` before GPUI or observation initialization. File existence is never ownership; the file is retained after exit. Windows holds the directory and file without delete sharing, opens the actual entries rather than reparse targets and rejects reparse entries. A competing launch exits 3 without activation routing; unavailable ownership exits 2 with a static diagnostic. The guard spans the app loop and OS termination releases its handles. **Caller-authorized bounded-save decision:** after [the explicit Caller choice and independent design review](../development/evidence/README.md#historical-records), final quit synchronously closes publication/command admission and requests observation shutdown, then attempts the existing atomic REC3 save once on the background executor. The attempt is detached from the bounded wait, so finishing the wait early neither prevents the attempt from starting nor cancels a late completion. Observation and preferences have separate results and share a single callback-start time: each deadline is at most 100 ms (half the pinned GPUI `SHUTDOWN_TIMEOUT` of 200 ms); configured observation budget may only shorten its own deadline. Zero is unconfirmed; delayed waiter/timer start never renews a deadline. Observation distinguishes confirmed, unconfirmed, no actual driver and callback not observed. Preferences distinguish saved, failed, unconfirmed and callback not observed; only actual save success confirmed before its deadline is saved, and late completion cannot upgrade a finalized unconfirmed report. Initial results are conservative before asynchronous work. GPUI's 200 ms bounds its wait for returned futures, not callback synchronous execution or OS I/O duration. An overdue I/O operation cannot be forcibly cancelled and may finish before process termination; no completion or latest-settings guarantee follows. Atomic replacement/old-file protection and the instance guard remain intact. Shell drop cancels Rail and Flow derivation. The GPUI-independent watcher caps future deadlines at 5 s and fails closed on timer creation failure, deadline expiry or poisoned waiter state; dropping a finished waiter cancels its timer wait. No Runtime discovery, launch, kill, Stop, retry or receipt settlement is introduced. Executable contention/release, normal/forced exit, preference restore, ACTIVE/unknown-receipt close and timeout Native acceptance remain HOLD; unit tests are not the Windows matrix.

## DC-MP-SEC1

**Contract:** Credential storage. **Execution ID:** [MP-SEC1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** OS secure-store integration, credential lifetime/rotation hooks, redaction guarantees; no plaintext in config/logs/diagnostics/crash reports. [Windows crosswalk row 20](#windows-native-requirements-crosswalk) owns native secure-store evidence; production credential use remains gated by DD-12/RC-02.
- **Non-goals:** no custom crypto, no secrets in projections.
- **Acceptance:** *detects: plaintext credential in a diagnostics dump, credential logged on failure, credential surviving revocation.*

## DC-MP-SEC2

**Contract:** Redaction, diagnostics and logging policy. **Execution ID:** [MP-SEC2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** implement non-negotiable redaction of credentials/tokens/signed locators/private paths in every log, diagnostic and error path; bounded subsystem logs/diagnostics, correlation IDs and no secrets in crash/error output. [Windows crosswalk row 12](#windows-native-requirements-crosswalk) owns native crash-diagnostics evidence. Retention/export/split details are DD-22 and require `SPEC_APPROVED` (MP-REL3 consumes them).
- **Acceptance:** *detects: signed media locator in a log, unbounded log growth, diagnostics report leaking a credential, correlation ID absent across a command/receipt pair.*
- **Physical in-process implementation (MP-SEC2):** app-core owns the closed `PublicationError` categories (server free text and local rejection text are dropped) and a bounded free-text redaction fallback used by the debug-only fixture trace. The fallback is a stateful incremental parser with a 256-byte token buffer and no full-input copy or token vector; sensitive context survives whitespace/control separators, so split assignments, Bearer markers, signed locators, quoted/absolute path values, detached signature continuations and multi-token path continuations cannot leak, while opaque namespaced identities and ordinary numeric fields remain visible. A quoted multi-word path retains continuation state through its closing quote; an unquoted path fails closed until a structured non-path field. An overlong component never clears active continuation state: a real closing quote is detected without retaining the token, an unquoted path keeps withholding later fragments, a long token beginning with an absolute path enters withholding, a long key=value/private-path prefix is classified with the same rules as its short form, an overlong separated secret fails closed, a sensitive key split from its separator still withholds the following value, and an overlong URL whose query/fragment appears after the retained prefix withholds the following token because a detached signature boundary cannot be established. Once a separated secret or late signed-URL boundary is unknowable, the rest of the line is withheld except an explicit Bearer marker; a complete same-token value does not hide following safe fields, and no later token length or classification can deactivate that withholding. Trace admission uses a saturating atomic quota (dock 4,096; rail 8,192) and the complete emitted line, including prefix, area, fields and newline, is within 512 bytes. The rail trace records query presence only, never raw search text. Startup/error output remains static; the command map key and receipt `command_id` are the correlation identity. DD-22 retention/export/split decisions and the Windows Error Reporting boundary remain separate and pending.

## DC-MP-SEC3

**Contract:** Untrusted media hardening. **Execution ID:** [MP-SEC3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** adversarial-input hardening only: treat all external bytes/metadata as untrusted; reject unsafe size/type/path/executable metadata and absurd dimensions before allocation; prevent partial upload after invalid admission. [Media input admission and integrity](#media-input-admission-and-integrity) owns the boundary; SEC3 does not own descriptor authorization (RT6), transfer/digest/quarantine (IMG6) or decoder/capability selection (IMG2).
- **Acceptance:** *detects: oversized allocation from attacker-controlled dimensions, media path traversal, metadata treated as instruction.*
- **Physical implementation (MP-SEC3, Desktop-local encoded boundary):** `crates/media` admits an explicit owned `File`/reader capability only; representation identity remains an opaque bounded identifier and no API resolves metadata to a path, URL or command. Acquisition reads at most the fixed 33-byte PNG signature+IHDR header before reserving payload capacity, then requires a complete chunk layout and positive width/height, rejects an area above the caller-configured `LocalLimits::max_pixels` with a bounded `TooLarge`, and performs the remaining bounded reads only after admission. `PixelSize::checked_area`/`checked_rgba_bytes` return explicit errors instead of wrapping; checked arithmetic covers reservation growth, byte accounting and header dimensions. This is adversarial admission, not decode admission: CRC, bit depth, colour, codec capability and decoded/scratch budgets remain MP-IMG2, transfer/quarantine remains MP-IMG6 and descriptor authorization remains MP-RT6. Metadata (for example `tEXt`) stays opaque bytes and is never executed, launched, fetched or surfaced. Identifier charset no longer accepts `:`, horizontal slash, backslash, `.`/`..` or URL shapes, so an identifier cannot impersonate a drive path or locator. Rejection or cancellation publishes no entry, retains no reservation and returns only bounded error categories. This narrows MP-IMG1's earlier signature-only acceptance to a complete header; cache ownership, eviction and last-consumer release semantics are unchanged. No decoder, network, GPU, dependency, unsafe code or Product import UX is added.

## DC-MP-REC1

**Contract:** Device-loss integration and recovery verification (R8). **Execution ID:** [MP-REC1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** invalidate GPU objects by device generation, retain safe CPU/encoded cache under bounds, recreate lazily, report a recoverable unavailable state; decoder reset/format changes tracked by generation; no stale-frame overwrite.
- **Acceptance:** *detects: white/blank shell after device reset with no state, stale texture displayed after reset, cache dropped wholesale on reset.*

## DC-MP-REC2

**Contract:** Cache plateau and release proofs (R8). **Execution ID:** [MP-REC2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** long-running switching/open/close loops; prove residency plateau within ratified bounds and explained retention; reconcile process counters with application accounting; missing endpoints stay NOT_MEASURED.
- **Acceptance:** *detects: monotonically growing residency over repeated open/close, eviction that never releases, plateau claims from a single short run.*

## DC-MP-REC3

**Contract:** Crash / restart / reopen recovery behaviour. **Execution ID:** [MP-REC3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Frozen local contract (implement exactly):**
  - **Persisted fields (closed list):** window geometry/DPR + maximized state, panel/rail/inspector widths, surface selection, last task id, viewed/compare candidate references, zoom/pan per viewed candidate, theme/density/text scale, command-palette recents. Nothing else — canonical truth, credentials, tokens, artifact bytes, evidence content and media locators are never persisted.
  - **Product §54 optional permissions vs this task's closed schema:** keyboard bindings, recent locations and explicitly saved Brief presets are permitted preferences, not mandatory MVP persistence. None is provided or persisted by REC3's initial contract above; this is not a Product-wide prohibition. If subsequently authorized, keyboard configuration belongs to MP-UX4/MP-NAV5's approved inventory, recent-location behavior to MP-IMG1's explicit local-input UX/security boundary, and explicitly saved presets to MP-NAV2/DD-06 (never automatic prompt history). Their owners first amend the permitted behavior and REC3 schema in the canonical contracts, then mirror the existing task cards; reuse the single preferences file, no new task/store or automatic permission. Product/UX choice and separate implementation authorization remain pending.
  - **Store:** one local preferences file under the per-user app data directory; integer schema version; atomic write (temp + replace); corrupt/unknown-version file is ignored with defaults restored and a bounded diagnostic notice — never a crash and never a partial merge.
  - **Identity rules:** restored references apply only if present in current projection; otherwise explicit unavailable, never silent substitution. Window bounds clamp to current monitor set. [Windows crosswalk rows 5 and 10](#windows-native-requirements-crosswalk) own native multi-monitor and restore acceptance.
  - **Task survival:** Desktop restart re-establishes observation and restores the above; claims about Task execution surviving Desktop exit/restart require MP-RT8 evidence and are `NOT_MEASURED` before that.
- **Acceptance:** *detects: Desktop restart rendering an empty workspace while tasks exist, local state restored against changed identities, corrupt preferences file crashing startup, canonical data or credentials written to the preferences file, "task survived" claimed from fixture evidence.*
- **Physical in-process implementation (MP-REC3 Desktop-local):** `app-core::preferences` owns a closed, deny-unknown-fields schema with integer version, bounded file/counts and static notices; `save_atomic` writes a unique temp file, syncs and renames it, removing the temp on failure, while `load` restores full defaults on missing/corrupt/unknown-version/oversized or out-of-bounds input. Acquisition is bounded on the opened file handle (at most 64 KiB + 1 bytes) and serialization validates the borrowed schema and bounds and admits the complete borrowed document through a bounded writer before cloning; only then is a bounded normalized copy made and written. Neither a post-metadata growth race nor an oversized in-memory document can allocate past the cap. The app-quit worker saves the locked borrowed value rather than cloning it first; outcome/report locks are never held across filesystem I/O or asynchronous waits. Geometry is clamped in position and dimensions to the present logical display bounds; DPR is metadata, and a display smaller than the configured minimum yields display-sized content rather than off-screen extents. `apps/desktop` loads the per-user file, opens the saved/maximized/clamped bounds and snapshots local surface/task/Viewed/Compare/geometry during render. **Caller-authorized change:** the previous save-before-observation-stop ordering and normal-exit latest-settings guarantee are replaced by [DC-MP-RT8](#dc-mp-rt8)'s close-first, concurrent one-save attempt with a separate absolute deadline and saved/failed/unconfirmed/not-observed reporting. Bounded shutdown takes precedence over confirmation of latest settings; expiry does not cancel blocked OS I/O, and a late successful write is not reported as confirmed saved. This does not change the atomic writer, closed schema or preservation of an existing valid file on save failure. The shell restores the remembered surface at construction; after the first authorized cache install it restores Task, Viewed and Compare only when those exact identities are present, otherwise it shows an explicit bounded unavailable notice without substitution. Theme/density/text-scale/zoom-pan/palette-recents and panel-resize controls do not exist yet, so their allowed fields remain defaults. No canonical projection, credential, media locator or Runtime command is serialized; crash-before-quit latest-change durability and real Task survival remain unmeasured.

## DC-MP-VID1

**Contract:** Video capability/interop probe. **Execution ID:** [MP-VID1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope (when authorized):** establish whether any supported route can present decoded video frames at the pinned GPUI revision (Windows `draw_surfaces` is a no-op today): candidate evaluation of Media Foundation + D3D11 surfaces, copy counts, synchronization, colour metadata, device-loss behaviour, plus a declared software/unavailable fallback; measure per rendering §7/§10 hypotheses.
- **Non-goals:** no zero-copy claim without measurement; no fixture-still-frame pretending to be playback; no dependency addition without an approved decision.
- **Exit:** capability report; either a concrete supported route with evidence or an explicit capability-restriction finding that keeps video unimplemented-and-honest.

## DC-MP-VID2

**Contract:** Video authorization decision package. **Execution ID:** [MP-VID2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** decide route (pin-supported path, bounded patch/update decision, or explicit capability restriction), fallback policy, licence/redistribution implications, and the acceptance contract; record the decision as an ADR if it changes a boundary.

## DC-MP-VID3

**Contract:** Video implementation. **Execution ID:** [MP-VID3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope (after MP-VID2):** bounded frame ring, monotonic playback clock (never UI animation ticks), async generation-tagged seeks, frame-step policy, device/decoder generation invalidation, explicit unavailable/fallback states.
- **Acceptance (later):** start/seek/frame-step/compare-PTS hypotheses from rendering §10 with declared endpoints; no assumed zero-copy.

## DC-MP-VID4

**Contract:** Video acceptance. **Execution ID:** [MP-VID4](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** end-to-end Windows evidence per ratified gates; compare-pane PTS synchronization only if UX allows; explicit fallback matrix recorded.

## DC-MP-AX1

**Contract:** Accessibility, DPI and IME program (R10, continuous). **Execution ID:** [MP-AX1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** screen-reader names/roles; keyboard-only virtualized content; IME composition; high contrast/reduced motion; dialog focus restoration; DPI migration preserving focus/selection/anchor; document unsupported combinations. [Windows crosswalk rows 6–7](#windows-native-requirements-crosswalk) own native mixed-DPI and IME evidence; DD-23 release support matrix remains pending.
- **Acceptance:** *detects: virtualized row losing accessible identity, IME composition breaking candidate text entry, focus lost after popup close, high-contrast selection invisible, DPI move resetting scroll anchor.*
- **Observed (2026-10-18, not acceptance):** the NAV4 tier B native runs show the GPUI window exposes its interactive controls to UIA (names, roles, disabled state and disabled reasons — see [r2 run index](../development/evidence/README.md#historical-records)), while the static shell text (the six state-axis cells, task rail, surface labels and inspector lines) is not present in the accessibility tree; those values were therefore verified from window screenshots. `aria_keyshortcuts("Enter Space")` also does not surface as the UIA AcceleratorKey property. This observation is recorded here for the MP-AX1/MP-ACC2 screen-reader scope (canon §51) and is not a claim that screen-reader support passes or fails.

## DC-MP-AX2

**Contract:** Usability and keyboard verification runs. **Execution ID:** [MP-AX2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** verify the canon §65 comprehension checks on the real shell, keyboard-completability of major workflows, and compare/inspector keyboard paths.
- **Acceptance:** *detects: workflow reachable only by mouse, palette action missing for a canon product action.*

## DC-MP-PL1

**Contract:** Visual polish (R9). **Execution ID:** [MP-PL1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** icons/motion/depth/focus polish with measured cost; earlier visual gates remain in force; no idle animation.
- **Acceptance:** *detects: polish that degrades legibility or adds idle GPU work, shadow cost outside popup budget.*

## DC-MP-REL1

**Contract:** Packaging (installer + portable archive). **Execution ID:** [MP-REL1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** signed installer is standard path plus portable archive; licence/notice inventory checked; `desktop_version` distinct from protocol/schema in About/diagnostics; no secrets in packaging. [Windows crosswalk rows 13–15 and 18](#windows-native-requirements-crosswalk) own conditional file association, portable/installer and uninstall/cleanup acceptance; no file-type registration or cleanup policy is implied beyond approved rows.
- **Acceptance:** *detects: unsigned artifact shipped, update blocked by missing signature verification, version reported as protocol version.*

## DC-MP-REL2

**Contract:** Updater. **Execution ID:** [MP-REL2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** check → download → verify signature → stage → restart → rollback, with protocol-compatibility check before switching; failure leaves prior version usable. [Windows crosswalk rows 16–17](#windows-native-requirements-crosswalk) define native update/rollback evidence.
- **Acceptance:** *detects: applying an incompatible protocol pair, rollback leaving a broken installation, update applied without signature check.*

## DC-MP-REL3

**Contract:** Diagnostics and support report. **Execution ID:** [MP-REL3](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** app-provided report (versions, connection, projection revisions, cache/subscription counters in bounds, redacted) plus bounded log export.
- **Acceptance:** *detects: report containing credentials/locators/private content, unbounded export size.*

## DC-MP-ACC1

**Contract:** Golden Creative Workflow acceptance. **Execution ID:** [MP-ACC1](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** run Product Canon §64 end-to-end on real integration: New Task → Brief → Start → Candidate → Artifact → Evaluation → Next Iteration → Compare → Evidence → Flow → Formal Outcome → reopen from History, plus six failure injections (generation failure, evaluation failure, missing media, connection loss, stop request, Desktop restart). Start/Next Iteration are accepted only after DD-25 records command semantics; until then report those steps `NOT_MEASURED`/blocked. Acceptance evidence must cover each named task contract explicitly (MP-NAV2, MP-IMG5, MP-IMG6, MP-CMP1, MP-INS1/2, MP-FLOW1, MP-NAV3, MP-RT3/4/5/7, MP-REC3, MP-REL1); this list does not make them hard DAG ancestors or promote their conditional Runtime/product gates.
- **Acceptance:** every step is proven by authoritative projection transitions; failure cases end in honest, recoverable states; *detects: any step satisfied only by fixture behaviour, optimistic rendering, or locally derived canonical values.*

## DC-MP-ACC2

**Contract:** Product acceptance matrix (canon §63–66). **Execution ID:** [MP-ACC2](../development/IMPLEMENTATION_MASTER_PLAN.md).

- **Scope:** assemble evidence across Domain/protocol/projection/ui-model/GPUI/Golden/performance/recovery/AX/packaging; label unverified `NOT_MEASURED`; include ratified performance gates; no screenshot-only claims. Acceptance evidence explicitly covers MP-ACC1, MP-FLOW3, MP-REL2, MP-REC2, MP-NAV5, MP-PL1, MP-AX2 and MP-PUB3; this evidence list is not a hard DAG prerequisite. Typography acceptance remains conditional on DD-04b. Video remains an MVP acceptance requirement: MP-VID4 evidence or an explicit Product-owner scope exception naming the limitation; this conditional acceptance gate does not add a hard implementation DAG edge.
- **Exit:** an acceptance report that a reviewer can falsify item by item; release decision remains a human one (checkbox-free honesty).

