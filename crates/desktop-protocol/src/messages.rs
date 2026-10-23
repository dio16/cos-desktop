//! Wire messages: envelopes, observation session, snapshots, delta
//! transactions, commands and receipts.
//!
//! Envelopes carry the negotiated protocol/schema versions. Unknown *message*
//! kinds deserialize to [`ServerMessage::Unknown`] and are handled by the
//! client's policy: an informational event (`affects_state = false`) may be
//! skipped while respecting cursor continuity; an unknown state-changing event
//! requires resync. An unknown `affects_state` value defaults to `true`
//! (state-changing) because assuming otherwise would silently lose state.

use std::fmt;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::errors::{ProtocolError, RejectReason, ResyncReason};
use crate::ids::{
    ArtifactDigest, ArtifactId, CommandId, Counter, Cursor, EpochId, GraphId, HarnessVersionId,
    IntegrityDigest, IterationId, OutcomeId, ProfileId, ProjectionKind, RecipeId, ScopeId,
    SnapshotId, TaskId, TransactionId,
};
use crate::macros::wire_enum;
use crate::projections::{
    ArtifactSnapshot, CandidateSnapshot, CreativeCapabilities, EvaluationSnapshot, GraphSnapshot,
    IterationSnapshot, OutcomeSnapshot, TaskSnapshot,
};
use crate::versions::{HelloAccepted, HelloRejected, HelloRequest, Version};

/// Client wire frame.
// Wire DTO enums are transient parse/match values; boxing variants would
// allocate on every frame and obscure the contract shape.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientWire {
    /// Pre-session handshake; carries advertised ranges instead of a negotiated
    /// pair.
    Hello(HelloRequest),
    /// Post-handshake frame with the negotiated versions.
    Frame(ClientFrame),
}

/// Client frame carrying the negotiated versions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientFrame {
    /// Negotiated protocol version.
    pub protocol: Version,
    /// Negotiated schema version.
    pub schema: Version,
    /// Message body.
    pub message: ClientMessage,
}

/// Client-to-server messages after the handshake.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientMessage {
    /// Open (or resume) one atomic observation session.
    OpenObservation(OpenObservation),
    /// Send one command.
    Command(CommandEnvelope),
    /// Look up the receipt for a previously sent command.
    LookupReceipt(ReceiptLookup),
}

/// Open or resume an observation session as one logical operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenObservation {
    /// Requested scope.
    pub scope: ObservationScopeRequest,
    /// Server-issued scope identity from the previous session, when resuming.
    #[serde(default)]
    pub scope_id: Option<ScopeId>,
    /// Last completely applied cursor, when resuming.
    #[serde(default)]
    pub last_seen_cursor: Option<Cursor>,
    /// Expected runtime epoch, when resuming.
    #[serde(default)]
    pub epoch: Option<EpochId>,
}

/// Client-requested observation scope. The security principal is bound by the
/// authenticated transport, never trusted from this payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationScopeRequest {
    /// Observed task set.
    pub task_ids: Vec<TaskId>,
    /// Additional server-interpreted filters.
    #[serde(default)]
    pub filters: Vec<ScopeFilter>,
}

/// Server-interpreted scope filter entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeFilter {
    /// Filter name.
    pub name: String,
    /// Filter value.
    pub value: String,
}

/// Receipt lookup by command identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptLookup {
    /// Command identity to look up.
    pub command_id: CommandId,
}

/// Server wire frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServerWire {
    /// Handshake accepted with the selected pair.
    HelloAccepted(HelloAccepted),
    /// Handshake rejected; no snapshot or command may follow.
    HelloRejected(HelloRejected),
    /// Post-handshake frame with the negotiated versions.
    Frame(ServerFrame),
}

/// Server frame carrying the negotiated versions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerFrame {
    /// Negotiated protocol version.
    pub protocol: Version,
    /// Negotiated schema version.
    pub schema: Version,
    /// Message body.
    pub message: ServerMessage,
}

/// Server-to-client messages after the handshake.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerMessage {
    /// Resume accepted for the exact scope/epoch; committed transactions follow.
    ReplayAccepted(ReplayAccepted),
    /// Resume refused; a fresh atomic snapshot is required.
    ResyncRequired(ResyncRequired),
    /// Snapshot bundle begins.
    SnapshotBegin(SnapshotBegin),
    /// Snapshot bundle part.
    SnapshotChunk(SnapshotChunk),
    /// Snapshot bundle ends; verifies completeness and integrity.
    SnapshotEnd(SnapshotEnd),
    /// One atomic delta transaction.
    DeltaTransaction(DeltaTransaction),
    /// Receipt (initial response or pushed canonical reflection).
    Receipt(CommandReceipt),
    /// Structured error.
    Error(ProtocolError),
    /// Unrecognized message kind, preserved for policy handling.
    Unknown(UnknownMessage),
}

impl ServerMessage {
    /// Wire kind of this message.
    pub fn kind(&self) -> &str {
        match self {
            Self::ReplayAccepted(_) => "REPLAY_ACCEPTED",
            Self::ResyncRequired(_) => "RESYNC_REQUIRED",
            Self::SnapshotBegin(_) => "SNAPSHOT_BEGIN",
            Self::SnapshotChunk(_) => "SNAPSHOT_CHUNK",
            Self::SnapshotEnd(_) => "SNAPSHOT_END",
            Self::DeltaTransaction(_) => "DELTA_TRANSACTION",
            Self::Receipt(_) => "RECEIPT",
            Self::Error(_) => "ERROR",
            Self::Unknown(unknown) => unknown.kind.as_str(),
        }
    }
}

fn tagged<T: Serialize>(kind: &str, payload: &T) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(payload)?;
    if let Value::Object(map) = &mut value {
        map.insert("kind".to_owned(), Value::String(kind.to_owned()));
    }
    Ok(value)
}

impl Serialize for ServerMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::ReplayAccepted(payload) => tagged("REPLAY_ACCEPTED", payload),
            Self::ResyncRequired(payload) => tagged("RESYNC_REQUIRED", payload),
            Self::SnapshotBegin(payload) => tagged("SNAPSHOT_BEGIN", payload),
            Self::SnapshotChunk(payload) => tagged("SNAPSHOT_CHUNK", payload),
            Self::SnapshotEnd(payload) => tagged("SNAPSHOT_END", payload),
            Self::DeltaTransaction(payload) => tagged("DELTA_TRANSACTION", payload),
            Self::Receipt(payload) => tagged("RECEIPT", payload),
            Self::Error(payload) => tagged("ERROR", payload),
            Self::Unknown(payload) => serde_json::to_value(payload),
        }
        .map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ServerMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::custom("server message is missing its kind"))?
            .to_owned();
        match kind.as_str() {
            "REPLAY_ACCEPTED" => serde_json::from_value(value)
                .map(Self::ReplayAccepted)
                .map_err(de::Error::custom),
            "RESYNC_REQUIRED" => serde_json::from_value(value)
                .map(Self::ResyncRequired)
                .map_err(de::Error::custom),
            "SNAPSHOT_BEGIN" => serde_json::from_value(value)
                .map(Self::SnapshotBegin)
                .map_err(de::Error::custom),
            "SNAPSHOT_CHUNK" => serde_json::from_value(value)
                .map(Self::SnapshotChunk)
                .map_err(de::Error::custom),
            "SNAPSHOT_END" => serde_json::from_value(value)
                .map(Self::SnapshotEnd)
                .map_err(de::Error::custom),
            "DELTA_TRANSACTION" => serde_json::from_value(value)
                .map(Self::DeltaTransaction)
                .map_err(de::Error::custom),
            "RECEIPT" => serde_json::from_value(value)
                .map(Self::Receipt)
                .map_err(de::Error::custom),
            "ERROR" => serde_json::from_value(value)
                .map(Self::Error)
                .map_err(de::Error::custom),
            _ => {
                let affects_state = value
                    .get("affects_state")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let cursor = match value.get("cursor") {
                    Some(raw) => Some(
                        serde_json::from_value::<Cursor>(raw.clone()).map_err(de::Error::custom)?,
                    ),
                    None => None,
                };
                Ok(Self::Unknown(UnknownMessage {
                    kind,
                    affects_state,
                    cursor,
                }))
            }
        }
    }
}

/// Unrecognized server message preserved for policy handling.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnknownMessage {
    /// Wire kind.
    pub kind: String,
    /// Whether the event may change authoritative state. Defaults to `true`
    /// when the server omits it.
    #[serde(default = "default_affects_state")]
    pub affects_state: bool,
    /// Cursor position this event consumed, when it declares one.
    #[serde(default)]
    pub cursor: Option<Cursor>,
}

fn default_affects_state() -> bool {
    true
}

/// Resume acknowledgement for the exact scope/epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayAccepted {
    /// Cursor the replay starts after; must equal the requested cursor.
    pub cursor: Cursor,
}

/// Explicit resync request with a reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncRequired {
    /// Why replay is not possible.
    pub reason: ResyncReason,
    /// Safe message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Snapshot bundle start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotBegin {
    /// Snapshot identity.
    pub snapshot_id: SnapshotId,
}

/// Snapshot bundle part. Mutations are creations (`base_revision` is `null`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotChunk {
    /// Snapshot identity this part belongs to.
    pub snapshot_id: SnapshotId,
    /// Zero-based part index.
    pub part_index: u32,
    /// Projection creations.
    pub mutations: Vec<ProjectionMutation>,
}

/// Snapshot bundle end with completeness and integrity declarations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEnd {
    /// Snapshot identity.
    pub snapshot_id: SnapshotId,
    /// Scope the snapshot was taken in.
    pub scope: ObservationScope,
    /// Runtime epoch.
    pub epoch: EpochId,
    /// Replay position of the consistent read cut.
    pub cursor: Cursor,
    /// Expected entity counts per projection kind.
    pub expected: SnapshotCounts,
    /// Integrity digest over the canonical encoding of all staged mutations.
    pub integrity_digest: IntegrityDigest,
}

/// Entity counts declared by a snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCounts {
    /// Tasks.
    pub tasks: u32,
    /// Candidates.
    pub candidates: u32,
    /// Artifacts.
    pub artifacts: u32,
    /// Evaluations.
    pub evaluations: u32,
    /// Iterations.
    pub iterations: u32,
    /// Outcomes.
    pub outcomes: u32,
    /// Graphs.
    pub graphs: u32,
    /// Capabilities.
    pub capabilities: u32,
}

impl SnapshotCounts {
    /// Counts implied by a staged mutation list.
    pub fn of(mutations: &[ProjectionMutation]) -> Self {
        let mut counts = Self::default();
        for mutation in mutations {
            if let ProjectionMutation::Upsert { projection, .. } = mutation {
                match projection {
                    Projection::Task(_) => counts.tasks += 1,
                    Projection::Candidate(_) => counts.candidates += 1,
                    Projection::Artifact(_) => counts.artifacts += 1,
                    Projection::Evaluation(_) => counts.evaluations += 1,
                    Projection::Iteration(_) => counts.iterations += 1,
                    Projection::Outcome(_) => counts.outcomes += 1,
                    Projection::Graph(_) => counts.graphs += 1,
                    Projection::Capabilities(_) => counts.capabilities += 1,
                }
            }
        }
        counts
    }

    /// Total entity count.
    pub fn total(&self) -> u32 {
        self.tasks
            + self.candidates
            + self.artifacts
            + self.evaluations
            + self.iterations
            + self.outcomes
            + self.graphs
            + self.capabilities
    }
}

/// Server-issued observation scope identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationScope {
    /// Server-issued scope identity; cursors cannot resume another scope.
    pub scope_id: ScopeId,
    /// Observed task set.
    pub task_ids: Vec<TaskId>,
    /// Server-interpreted filters.
    #[serde(default)]
    pub filters: Vec<ScopeFilter>,
    /// Negotiated protocol version.
    pub protocol: Version,
    /// Negotiated schema version.
    pub schema: Version,
}

/// One atomic delta transaction.
///
/// `predecessor` links to the previously applied cursor; ordering is expressed
/// by these links, never by arithmetic on the opaque cursor. `mutation_count`
/// and `integrity_digest` are verified before any mutation is applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaTransaction {
    /// Transaction identity.
    pub transaction_id: TransactionId,
    /// Runtime epoch.
    pub epoch: EpochId,
    /// Cursor this transaction publishes.
    pub cursor: Cursor,
    /// Cursor this transaction follows; `None` only for a stream start without
    /// a snapshot (not used by the observation session).
    #[serde(default)]
    pub predecessor: Option<Cursor>,
    /// Declared number of mutations.
    pub mutation_count: u32,
    /// Integrity digest over the canonical encoding of `mutations`.
    pub integrity_digest: IntegrityDigest,
    /// Mutations applied atomically.
    pub mutations: Vec<ProjectionMutation>,
}

/// One typed projection mutation.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProjectionMutation {
    /// Create or replace a projection.
    Upsert {
        /// Expected current revision; `None` requires the projection to be absent.
        #[serde(default)]
        base_revision: Option<Counter>,
        /// Projection content carrying the new revision.
        projection: Projection,
    },
    /// Remove a projection (tombstone).
    Remove {
        /// Expected current revision.
        base_revision: Counter,
        /// Removal target.
        target: RemoveTarget,
    },
}

impl ProjectionMutation {
    /// New revision declared by an upsert, if any.
    pub fn new_revision(&self) -> Option<&Counter> {
        match self {
            Self::Upsert { projection, .. } => Some(projection.revision()),
            Self::Remove { .. } => None,
        }
    }

    /// Projection kind touched by this mutation.
    pub fn projection_kind(&self) -> ProjectionKind {
        match self {
            Self::Upsert { projection, .. } => projection.kind(),
            Self::Remove { target, .. } => target.kind(),
        }
    }
}

/// Projection payload tagged by kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "projection", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Projection {
    /// Task projection.
    Task(TaskSnapshot),
    /// Candidate projection.
    Candidate(CandidateSnapshot),
    /// Artifact projection.
    Artifact(ArtifactSnapshot),
    /// Evaluation projection.
    Evaluation(EvaluationSnapshot),
    /// Iteration projection.
    Iteration(IterationSnapshot),
    /// Outcome projection.
    Outcome(OutcomeSnapshot),
    /// Graph projection.
    Graph(GraphSnapshot),
    /// Capabilities projection.
    Capabilities(CreativeCapabilities),
}

impl Projection {
    /// Projection kind.
    pub fn kind(&self) -> ProjectionKind {
        match self {
            Self::Task(_) => ProjectionKind::Task,
            Self::Candidate(_) => ProjectionKind::Candidate,
            Self::Artifact(_) => ProjectionKind::Artifact,
            Self::Evaluation(_) => ProjectionKind::Evaluation,
            Self::Iteration(_) => ProjectionKind::Iteration,
            Self::Outcome(_) => ProjectionKind::Outcome,
            Self::Graph(_) => ProjectionKind::Graph,
            Self::Capabilities(_) => ProjectionKind::Capabilities,
        }
    }

    /// Projection revision carried by this payload.
    pub fn revision(&self) -> &Counter {
        match self {
            Self::Task(snapshot) => &snapshot.projection_revision,
            Self::Candidate(snapshot) => &snapshot.projection_revision,
            Self::Artifact(snapshot) => &snapshot.projection_revision,
            Self::Evaluation(snapshot) => &snapshot.projection_revision,
            Self::Iteration(snapshot) => &snapshot.projection_revision,
            Self::Outcome(snapshot) => &snapshot.projection_revision,
            Self::Graph(snapshot) => &snapshot.projection_revision,
            Self::Capabilities(snapshot) => &snapshot.projection_revision,
        }
    }
}

/// Typed removal target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RemoveTarget {
    /// Remove a task.
    Task {
        /// Task identity.
        task_id: TaskId,
    },
    /// Remove a candidate.
    Candidate {
        /// Candidate tuple.
        candidate_ref: crate::ids::CandidateRef,
    },
    /// Remove an artifact.
    Artifact {
        /// Artifact identity.
        artifact_id: ArtifactId,
    },
    /// Remove an evaluation.
    Evaluation {
        /// Evaluation identity.
        evaluation_id: crate::ids::EvaluationId,
    },
    /// Remove an iteration.
    Iteration {
        /// Iteration identity.
        iteration_id: IterationId,
    },
    /// Remove an outcome.
    Outcome {
        /// Outcome identity.
        outcome_id: OutcomeId,
    },
    /// Remove a graph.
    Graph {
        /// Graph identity.
        graph_id: GraphId,
    },
    /// Remove a capabilities projection.
    Capabilities {
        /// Capabilities identity.
        capabilities_id: crate::ids::CapabilitiesId,
    },
}

impl RemoveTarget {
    /// Projection kind removed by this target.
    pub fn kind(&self) -> ProjectionKind {
        match self {
            Self::Task { .. } => ProjectionKind::Task,
            Self::Candidate { .. } => ProjectionKind::Candidate,
            Self::Artifact { .. } => ProjectionKind::Artifact,
            Self::Evaluation { .. } => ProjectionKind::Evaluation,
            Self::Iteration { .. } => ProjectionKind::Iteration,
            Self::Outcome { .. } => ProjectionKind::Outcome,
            Self::Graph { .. } => ProjectionKind::Graph,
            Self::Capabilities { .. } => ProjectionKind::Capabilities,
        }
    }
}

/// Command envelope.
///
/// The security principal is bound by the authenticated transport, never from
/// this payload. `expected_revision` is required for mutations whose
/// correctness depends on a viewed version; conflicts are never resolved
/// last-write-wins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    /// Client-generated globally unique command identity, retained across an
    /// allowed retry.
    pub command_id: CommandId,
    /// Typed target or explicit creation scope.
    pub target: CommandTarget,
    /// Optional expected projection revision precondition.
    #[serde(default)]
    pub expected_revision: Option<Counter>,
    /// Optional expected runtime epoch precondition.
    #[serde(default)]
    pub epoch: Option<EpochId>,
    /// Command payload.
    pub payload: CommandPayload,
}

/// Command target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandTarget {
    /// Existing task.
    Task {
        /// Task identity.
        task_id: TaskId,
    },
    /// Explicit creation scope; never a fake task target.
    Creation {
        /// Recipe reference for the creation.
        recipe_ref: RecipeRef,
    },
}

/// Recipe reference used by a creation scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeRef {
    /// Recipe identity.
    pub recipe_id: RecipeId,
    /// Pinned graph revision.
    pub graph_ref: crate::ids::GraphRef,
    /// Source harness version identity, when pinned.
    #[serde(default)]
    pub source_harness_version_id: Option<HarnessVersionId>,
    /// Source harness digest, when pinned.
    #[serde(default)]
    pub source_harness_digest: Option<ArtifactDigest>,
    /// Profile reference, when selected.
    #[serde(default)]
    pub profile_ref: Option<ProfileId>,
    /// Projection digest of the recipe, when supplied.
    #[serde(default)]
    pub projection_digest: Option<ArtifactDigest>,
}

/// Command payload families.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandPayload {
    /// Create a task in an explicit scope.
    CreateTask(CreateTaskCommand),
    /// Stop an existing task (request intent; acceptance is not completion).
    StopTask(StopTaskCommand),
    /// Reuse an existing task's context (runtime-owned semantics).
    ReuseTask(ReuseTaskCommand),
}

/// Create-task payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTaskCommand {
    /// Objective text (1..=8000 characters).
    pub objective_text: String,
    /// Optional text prompt.
    #[serde(default)]
    pub text_prompt: Option<String>,
    /// Reference artifact identities.
    #[serde(default)]
    pub reference_artifact_refs: Vec<ArtifactId>,
    /// Parameter values.
    #[serde(default)]
    pub parameter_values: Vec<NamedParameter>,
}

/// Stop-task payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopTaskCommand {
    /// Task to stop.
    pub task_id: TaskId,
    /// Optional reason text.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Reuse-task payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReuseTaskCommand {
    /// Source task whose accepted context is reused.
    pub source_task_id: TaskId,
}

/// Named parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedParameter {
    /// Parameter name.
    pub name: String,
    /// Parameter value.
    pub value: ParameterValue,
}

/// Typed parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ParameterValue {
    /// Text value.
    Text(String),
    /// Integer value.
    Integer(i64),
    /// Decimal value.
    Decimal(f64),
    /// Boolean value.
    Boolean(bool),
    /// Choice value.
    Choice(String),
}

/// Command envelope validation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandValidationError {
    message: String,
}

impl CommandValidationError {
    /// Create an error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CommandValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandValidationError {}

impl CommandEnvelope {
    /// Validate target/payload consistency and creation preconditions.
    pub fn validate(&self) -> Result<(), CommandValidationError> {
        match &self.payload {
            CommandPayload::CreateTask(create) => {
                if !matches!(self.target, CommandTarget::Creation { .. }) {
                    return Err(CommandValidationError::new(
                        "CREATE_TASK requires an explicit creation target",
                    ));
                }
                if self.expected_revision.is_some() {
                    return Err(CommandValidationError::new(
                        "CREATE_TASK must not carry an expected projection revision",
                    ));
                }
                let length = create.objective_text.chars().count();
                if length == 0 || length > 8000 {
                    return Err(CommandValidationError::new(
                        "CREATE_TASK objective_text must be 1..=8000 characters",
                    ));
                }
            }
            CommandPayload::StopTask(stop) => match &self.target {
                CommandTarget::Task { task_id } if task_id == &stop.task_id => {}
                _ => {
                    return Err(CommandValidationError::new(
                        "STOP_TASK requires a task target matching its payload task",
                    ));
                }
            },
            CommandPayload::ReuseTask(reuse) => match &self.target {
                CommandTarget::Task { task_id } if task_id == &reuse.source_task_id => {}
                _ => {
                    return Err(CommandValidationError::new(
                        "REUSE_TASK requires a task target matching its source task",
                    ));
                }
            },
        }
        Ok(())
    }
}

wire_enum! {
    /// Receipt state recorded by the server.
    ///
    /// `Accepted` means the server durably recorded validated intent, not
    /// semantic completion. `CanonicalReflected` links the command to a
    /// canonical effect; for Stop this still distinguishes stop-request state
    /// from terminal completion.
    pub enum ReceiptState {
        Accepted => "ACCEPTED",
        Rejected => "REJECTED",
        CanonicalReflected => "CANONICAL_REFLECTED",
    }
}

/// Command receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    /// Command identity.
    pub command_id: CommandId,
    /// Command family.
    pub operation: crate::projections::CommandOperation,
    /// Receipt state.
    pub state: ReceiptState,
    /// Task the command targeted or created, when known.
    #[serde(default)]
    pub task_id: Option<TaskId>,
    /// Server acceptance timestamp, when reported.
    #[serde(default)]
    pub accepted_at: Option<String>,
    /// Definitive rejection reason for rejected receipts.
    #[serde(default)]
    pub reject_reason: Option<RejectReason>,
    /// Canonical effect references linked to the command.
    #[serde(default)]
    pub effect_refs: Vec<crate::ids::EntityRef>,
    /// Advertised dedupe retention expiry, when the server guarantees one.
    #[serde(default)]
    pub dedupe_expires_at: Option<String>,
    /// Safe message.
    #[serde(default)]
    pub message: Option<String>,
}
