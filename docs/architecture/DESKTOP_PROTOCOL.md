# Desktop Protocol Draft

Status: **Proposed — Draft**, except authority/safety rules marked **Accepted**. This is not an implemented server API. Bootstrap will implement only a fixture-tested contract/client. [Current runtime survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md) is the evidence for existing APIs; do not assume they already provide these guarantees. [Ownership](../integration/PROTOCOL_OWNERSHIP.md) governs the future machine-readable source.

## 1. Scope, identity and data

**Accepted:** projections are a Desktop/COS contract, not copies of Domain objects. Runtime validates identities, authorization, transitions and expected revisions. Desktop cannot manufacture semantic identities or infer Formal Outcome. Existing authoritative identifiers and validations must survive server mapping; display labels never become wire identifiers.

**Draft:** JSON control messages encoded UTF-8; no binary/base64 media. Every envelope carries negotiated protocol/schema versions, message kind and correlation/stream identity as applicable. Integer counters use decimal strings on the wire to avoid cross-language precision loss. Unknown fields follow section 5. Required fields and references are validated before publication.

An entity reference carries `kind` plus opaque server-issued `id`. Existing canonical task/graph/plan/artifact identity formats are preserved, not replaced with a Desktop UUID. For fixture-only synthetic references use an explicitly isolated `fixture:` namespace; they must never be accepted as real server entities. A task display label `Task-184` and artifact labels `A`–`E` are independent fields. Artifact digest and task/candidate identity are distinct even if candidates share bytes. No guessed local CAS path.

Observed candidate references are the tuple `{task_id, run_id, iteration, candidate_id, artifact_ref}`; formal selection uses `{artifact_digest, run_id, evaluation_id, iteration}`. An adapter must preserve these typed identity components (or issue an opaque server-resolved handle for the exact tuple), never flatten them to a display label or assume candidate_id alone is sufficient. Graph references retain graph_id/version/digest and plans retain id/digest. New handle mapping is Runtime-owned, not a client identity factory.

The current wire task statuses are ASSIGNED/ACTIVE/COMPLETED/CANCELLED. Current phase is an open string and differs from Rust InnerLoopPhase; fixture REFINE is a presentation label in a synthetic Draft scenario, not evidence of an existing Runtime REFINE wire code. Existing `schema_version: 1` is the legacy frontend schema, not automatic equivalence to proposed Desktop schema 1.0.

| Projection candidate | Required meaning (Draft fields, not Domain definitions) |
|---|---|
| TaskSnapshot | task_ref, display_id/title, task_status, inner_phase, iteration_ref/index, best_candidate_ref?, formal_outcome_ref?, accepted_brief_ref?, capabilities_ref, projection_revision |
| CandidateSnapshot | candidate_ref, task_ref, iteration_ref, artifact_refs, evaluation_refs, display_label, projection_revision; no UI-owned Viewed/Compare marker |
| ArtifactSnapshot | artifact_ref/canonical identity, media_type, availability, descriptor_refs by representation, verification summary, projection_revision |
| EvaluationSnapshot | evaluation_ref, candidate/artifact refs, decision, named axes and units, evidence refs, authority, optional confidence with documented meaning, projection_revision; missing score is not zero |
| IterationSnapshot | iteration_ref, task_ref, runtime index, candidate refs, refinement/evidence refs, projection_revision |
| OutcomeSnapshot | outcome_ref, task_ref, formal status, formally selected refs, provenance/evidence refs, projection_revision; absent is not failed |
| GraphSnapshot | graph_ref, canonical GraphRevision identity, plan refs, inspection nodes/edges, provenance/refinement refs, projection_revision; presentation revision is not GraphRevision |
| ProjectionDelta | observation scope, epoch, cursor, predecessor cursor, atomic transaction ID, typed upserts/removals with base/new projection revisions |
| CreativeCapabilities | supported command/media/features, constraints, availability/reason, scope and projection_revision; no raw provider secret |
| MediaDescriptor | See section 6; metadata/access descriptor only |

GraphRevision and ExecutionPlan references are Runtime facts, not client-generated compile instructions. Evidence/provenance references expose authorized projections, never DB handles. History/pagination must use server-issued continuation with stable query/snapshot scope; changing a filter starts a new observation, not a client reconstruction of Domain history.

Task status, phase, iteration, availability, connection and outcome remain separate axes. The fixture presentation uses ACTIVE / REFINE / iteration 3, but adapters must map actual runtime wire values explicitly with runtime-owner approval; casing and display text are not a new semantic enum. Unrecognized authoritative status/outcome values are preserved as unknown and inhibit dependent actions; never map unknown to FAILED/COMPLETED.

## 2. Observation and recovery

### Scope and counters

A projection key is `(entity kind, canonical entity id, projection kind)`. Its `projection_revision` is an opaque monotonically increasing counter within `(runtime_epoch, projection key)` and changes whenever the serialized authoritative projection changes. A GraphRevision identity is an independent immutable Domain identity; do not compare it numerically to projection_revision.

A stream cursor identifies a durable observation position within `(server_identity, runtime_epoch, subscription_scope_id)`. Scope includes principal/authorization context, task set, filters and negotiated schema. It is not a projection revision and cannot resume another scope. Cursor is opaque to the client; ordering/continuity is expressed by predecessor links rather than arithmetic assumptions. Runtime epoch changes whenever prior cursor validity cannot be guaranteed (at minimum after a non-durable restart). A durable epoch across restart is permitted only if replay guarantees are proven. Authentication scope changes invalidate cursors.

### Atomic observation session

Client requests `OpenObservation(scope, last_seen_cursor?, epoch?)` as one logical operation. `initial_snapshot` and `subscribe` may be convenience views of that session, never independently raced calls.

For new/full observation the server establishes a consistent canonical read cut and replay position C atomically, pins/registers delivery after C before releasing that cut, then sends SnapshotBegin, complete snapshot content, SnapshotEnd with snapshot_id, scope, epoch, cursor C, expected entity counts and integrity digest. Updates committed after C are buffered/replayed in order only after SnapshotEnd. Server must not assemble unrelated endpoint responses and call the result atomic. If the server cannot hold a cut/replay reservation, fail explicitly and restart; do not silently lose updates.

Client stages snapshot off the visible model, checks completeness/digest/schema/reference integrity and session generation, then swaps the whole bundle once. Only after successful atomic application is C the last_seen_cursor. Interdependent projections are either present or explicitly marked unavailable/deferred references, never accidental dangling references. Old view can remain visible as stale; it is not partly overwritten by an incomplete snapshot. Late messages from cancelled session generations are ignored.

For resume, server either acknowledges ReplayAccepted for the exact scope/epoch and sends all committed transactions after last_seen_cursor, or returns ResyncRequired(reason). Reasons include expired/pruned cursor, unknown scope, authorization change, epoch change or unavailable history. Full snapshot follows the same atomic path, not a partial fallback. A fresh process without durable cached complete state cannot use a saved cursor alone; request a full snapshot.

### Delta transactions, duplicates and gaps

One delta transaction is applied atomically across its projections, including removals/tombstones. Each mutation declares base_revision and new_revision; base must match locally staged state (or explicit creation). A transaction must follow the last applied cursor. Multi-frame transactions declare expected count/digest and commit marker; no partial transaction reaches UI. Negotiated bounded batch size applies.

Exact duplicate already-applied transaction/cursor with matching content is ignored. Same cursor with conflicting content is a protocol error. Maintain a bounded duplicate digest window; messages older than that window are not assumed equivalent—request resync. A missing predecessor, out-of-order transaction, invalid base revision, malformed required event, or unsupported state-changing event invalidates the current live session and requests a full snapshot. Do not advance cursor on receipt, queue insertion, or partial apply. Advance only after the entire validated transaction is published. No unbounded reorder buffer or guessed repair.

### Bounds and connection state

**Draft defaults:** 1 MiB maximum control frame; 8 MiB maximum staged snapshot/transaction bundle; receive queue at most 256 frames or 8 MiB, whichever is reached first. Limits include decoded allocation accounting; reject oversized declared lengths before allocating. Bootstrap fixtures must fit these bounds. Large graphs/history need explicit scoped/paged observations with completeness indicators, not silent truncation. Later revisions can negotiate lower bounds; raising ceilings requires measurement and review.

Overflow cancels the stream, drops unapplied staging, marks visible projection stale, preserves last completely applied cursor, and requests full resync with bounded backoff. Repeated overflow stops retry churn and reports scope/limit incompatibility. Background processing may coalesce publication of complete transactions, never drop arbitrary deltas. No blocking IPC, JSON decode or graph layout on the GPUI thread.

Client-local connection states: disconnected → negotiating → synchronizing → live; errors lead to stale/reconnecting or incompatible/unauthorized (manual correction). Last-known Task Status remains unchanged. Backoff/jitter for reconnect is safe; reconnect is not permission to replay commands. Local disconnect cancels observation only, not Task execution.

## 3. Commands and receipts

Draft command envelope: `command_id` (random globally unique client-generated id, retained across an allowed retry), command kind, typed target reference (or explicit creation scope), optional expected projection revision/epoch, payload, negotiated versions. Security principal is bound by authenticated transport, never trusted from payload. The server binds deduplication to `(principal, command_id)` and the canonical request digest including target/kind/payload/precondition.

Candidate command families: CreateTask, StopTask, ReuseTask. RefreshTask, RequestArtifact and RequestComparisonData are observation/query intents unless a negotiated command explicitly has a side effect. Do not invent runtime formal-selection commands from the UI's compare selection. Existing frontend create/stop validation is reused server-side, not replaced by Desktop rules.

Expected revision is required for mutations whose correctness depends on a viewed version; conflicts return current reference/revision and require re-evaluation, never last-write-wins. Creation uses an explicit scope rather than a fake task target. Stop may be defined by runtime as idempotent request intent without an expected revision; this must be declared in its command contract, not assumed by clients.

| Client/receipt state | Meaning |
|---|---|
| Prepared / not sent | Local intent only |
| Pending / sent | Delivery attempted; no claim of acceptance |
| Accepted | Server durably recorded validated intent/receipt, not semantic completion |
| Rejected | Definitive validation/authorization/conflict/unsupported result; no execution for that command |
| ResultUnknown | Response lost, timeout or crash; execution may have happened |
| CanonicalReflected | Authoritative projection/receipt links command_id to canonical effect; for Stop this still distinguishes stop-request state from terminal completion |

Server Draft guarantee: persist command digest and receipt atomically with the accepted effect/intent before acknowledging. Same ID and same digest returns original receipt/result; same ID and different digest returns CommandIdConflict. In-flight duplicate cannot execute twice. Receipt status lookup by command_id must be authorized. For asynchronous execution, accepted receipt persists the intent and effect/outcome reference when known. Runtime crash recovery must not duplicate work from an accepted intent.

Dedupe retention is negotiated and advertised as an expiry for lookup/replay safety; no retention duration is currently verified. On lost response, query receipt, refresh projections and show unknown meanwhile. Only retry the identical ID/envelope when the server explicitly guarantees dedupe is still retained and valid across the observed epoch; never generate a new ID to hide a lost response. Unknown/pruned receipt does not prove non-execution. If guarantee is absent/expired or server identity changes, do not automatically resend a mutating command. User reconciliation may create a new intentional request, clearly distinct from retry. Client cancellation/timeouts do not roll back server work. UI disable is not security control. Stop sent/accepted is never Stop completion.

## 4. Errors and capabilities

Draft structured errors: code, safe message, correlation/command ID, scope, optional retry_after, field errors, current revision and resync reason. Codes include Unauthorized, Forbidden, ValidationFailed, RevisionConflict, UnsupportedCommand, VersionIncompatible, ResyncRequired, QueueOverflow, MessageTooLarge, CommandIdConflict, ReceiptExpired, MediaUnavailable and InternalError. Transport failure is not a Task failure. Internal errors omit paths, secrets, raw prompts and stack internals from normal UI.

Capabilities express supported operations and constraints at server/task/artifact scope, not provider-name inference. Feature support differs from current permission/availability; both may restrict action. Unknown capability defaults disabled. Server rechecks authorization and validation on every command/media fetch even when advertised as available. Version enables interpretation of a message; capability determines whether an understood operation is offered.

## 5. Version negotiation

Separate `desktop_version` (client release), `cosd_version` (server build; existing adapter may identify itself without claiming to be cosd), `protocol_version` (envelope/interaction), `schema_version` (payload grammar/meaning). Complete application version equality is never required.

Draft protocol and schema versions are `major.minor`. Hello advertises supported ranges as `{major, min_minor, max_minor}` with inclusive bounds; multiple majors are separate entries, not one ambiguous semver expression. Initial fixture contract candidate is protocol 1.0/schema 1.0, not a claim of deployed v1 server support. Server chooses the highest mutually supported protocol and schema combination it can actually encode; client confirms before snapshots or commands. No intersection produces explicit VersionIncompatible and no side effects. The selected pair is fixed for the session; changing it requires a new observation. Advertise feature capabilities separately.

Minor additions may add optional fields with explicit defaults that preserve prior semantics. Emit fields/events only as supported by the negotiated schema; never add a required field or reinterpret a value in a minor update. Unknown noncritical fields are ignored (bounded parsing); unknown fields cannot bypass required-field validation. Unknown informational event with `affects_state=false` may be skipped while respecting its cursor continuity. Unknown state-changing event or unknown critical extension triggers incompatibility/resync, never silent cursor advancement. Unknown enum values remain an explicit Unknown(raw) presentation value and disable operations requiring understood semantics; unknown command discriminants are rejected, never executed as a default variant. If the selected schema cannot represent an authoritative state safely, server rejects that schema instead of lying by coercion.

Breaking semantics, removal, new mandatory fields, incompatible identity/framing or revision rules require a new major. Negotiate an old supported major through an explicit server adapter while migrating; never keep independently edited schemas. Compatibility tests must include old/new optional fields, unknown fields/enums/events, disjoint ranges, mismatched app versions with compatible protocol, and breaking-major refusal. Schema evolution is reviewed with contract → server → client → compatibility → migration impact, not by auto-bumping versions.

## 6. Media descriptors

Accepted pipeline: ArtifactSnapshot → MediaDescriptor → authorized media service → background decode → GPUI-owned GPU texture. Metadata, thumbnail, preview and original are distinct representations. No JSON + base64 images. No control-plane binary bytes. Complete media service/pipeline is Deferred beyond bootstrap.

Draft descriptor fields: opaque descriptor_id, artifact_ref, representation kind/version, media type, byte length/dimensions/duration where known, availability, verification state and expected digest/algorithm where meaningful, authorized locator/access_handle, server identity, audience/principal scope and expiry. Locator is a same-authority service endpoint or opaque handle, not a filesystem path or unrestricted URL capability. Client allows only negotiated server/scheme/origin; no arbitrary redirects, local file schemes, shell commands or metadata-driven executable actions. Server resolves handles and enforces task/artifact authorization again. Do not log signed URLs/tokens or persist credentials in plaintext.

Expired descriptor is refreshed through cos-client; missing/forbidden/pending/unsupported remain distinct states. Verify content identity/length/type and bounded decode policy before upload; digest mismatch quarantines/discards bytes and reports verification failure, never replaces canonical identity. External media remains untrusted even when transport-authenticated. Cache key includes canonical identity, representation and authorization scope, with independently bounded metadata/thumbnail/CPU/GPU budgets. GPUI texture ownership never leaks into protocol/client. OS file access for explicit import/export requires separate user-scoped permissions, not a MediaDescriptor escape hatch.

## 7. Proof boundaries

Fixture tests cover serialization, negotiation, snapshot/delta staging, gap/duplicate/overflow/restart-epoch logic and command unknown-result handling. They cannot establish atomic runtime snapshots, durable server dedupe, real authentication, task survival, restart recovery or actual media interoperability. Those need separate Runtime-owned implementation and integration evidence listed in [required changes](../integration/COS_RUNTIME_REQUIRED_CHANGES.md). No server implementation, transport, fixture file or Rust type is created by this document.
