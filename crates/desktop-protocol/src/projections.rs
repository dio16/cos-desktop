//! Read-only projection DTOs.
//!
//! Projections are a Desktop/COS contract, not copies of Domain objects. The
//! runtime validates identities, authorization, transitions and expected
//! revisions; Desktop cannot manufacture semantic identities, derive a winner
//! from a candidate list or infer a formal outcome. Missing values are
//! represented as `None` (or an explicit unknown/availability state) and never
//! as a synthesized success.

use serde::{Deserialize, Serialize};

use crate::ids::{
    ArtifactDigest, ArtifactId, AuthorityLabel, CandidateRef, CapabilitiesId, Counter,
    DescriptorId, EdgeId, EdgeKindLabel, EntityRef, EvaluationId, FormalSelectionRef, GraphRef,
    IterationId, LayerLabel, NodeId, OutcomeId, PhaseLabel, PlanRef, ProvenanceId, RefinementId,
    StopReasonLabel, TaskId,
};
use crate::macros::wire_enum;

wire_enum! {
    /// Wire task status vocabulary observed in the current runtime.
    ///
    /// Unrecognized statuses stay `Unknown(raw)` and MUST inhibit dependent
    /// actions; they are never mapped to `COMPLETED` or `FAILED`.
    pub enum TaskStatus {
        Assigned => "ASSIGNED",
        Active => "ACTIVE",
        Completed => "COMPLETED",
        Cancelled => "CANCELLED",
    }
}

impl TaskStatus {
    /// Whether this is a recognized terminal status.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    /// Whether this is the recognized active status.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}

wire_enum! {
    /// Artifact availability. States stay distinct; there is no implied retry.
    pub enum Availability {
        Available => "AVAILABLE",
        Staged => "STAGED",
        Pending => "PENDING",
        Missing => "MISSING",
        Forbidden => "FORBIDDEN",
        Failed => "FAILED",
        Quarantined => "QUARANTINED",
        Unsupported => "UNSUPPORTED",
    }
}

impl Availability {
    /// Whether content may be fetched for this artifact.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

wire_enum! {
    /// Verification state of an artifact or media representation.
    pub enum VerificationState {
        Unverified => "UNVERIFIED",
        Verified => "VERIFIED",
        Mismatch => "MISMATCH",
        Quarantined => "QUARANTINED",
    }
}

wire_enum! {
    /// Evaluation decision status vocabulary observed in the current runtime.
    pub enum EvaluationDecision {
        Pass => "PASS",
        Fail => "FAIL",
        Abstain => "ABSTAIN",
        Inconclusive => "INCONCLUSIVE",
    }
}

wire_enum! {
    /// Evidence record availability.
    pub enum EvidenceStatus {
        Available => "AVAILABLE",
        Missing => "MISSING",
        Forbidden => "FORBIDDEN",
    }
}

wire_enum! {
    /// Formal outcome status vocabulary observed in the current runtime.
    ///
    /// An absent outcome projection is not a failed outcome.
    pub enum FormalStatus {
        Completed => "COMPLETED",
        Partial => "PARTIAL",
        Failed => "FAILED",
        Cancelled => "CANCELLED",
    }
}

wire_enum! {
    /// Media representation kind.
    pub enum RepresentationKind {
        Metadata => "METADATA",
        Thumbnail => "THUMBNAIL",
        Preview => "PREVIEW",
        Original => "ORIGINAL",
    }
}

wire_enum! {
    /// Whether an operation/feature is offered.
    ///
    /// Feature support is separate from current permission/availability; both
    /// may restrict action. An unknown capability defaults disabled.
    pub enum SupportState {
        Supported => "SUPPORTED",
        Unsupported => "UNSUPPORTED",
        Unavailable => "UNAVAILABLE",
    }
}

impl SupportState {
    /// Only `SUPPORTED` enables an operation.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }
}

wire_enum! {
    /// Command families a client may send.
    pub enum CommandOperation {
        CreateTask => "CREATE_TASK",
        StopTask => "STOP_TASK",
        ReuseTask => "REUSE_TASK",
    }
}

/// Task projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    /// Canonical task identity.
    pub task_ref: TaskId,
    /// Human display id (e.g. `Task-184`); never a wire identifier.
    pub display_id: String,
    /// Human title.
    pub title: String,
    /// Task status axis.
    pub task_status: TaskStatus,
    /// Inner phase label axis (open vocabulary, presentation only).
    pub inner_phase: PhaseLabel,
    /// Current iteration reference, when known.
    #[serde(default)]
    pub iteration_ref: Option<IterationId>,
    /// Current runtime iteration index, when known.
    #[serde(default)]
    pub iteration_index: Option<u32>,
    /// Runtime best-so-far candidate reference, when the runtime projects one.
    #[serde(default)]
    pub best_candidate_ref: Option<CandidateRef>,
    /// Formal outcome reference, when a formal outcome exists.
    #[serde(default)]
    pub formal_outcome_ref: Option<OutcomeId>,
    /// Accepted brief artifact reference, when one exists.
    #[serde(default)]
    pub accepted_brief_ref: Option<ArtifactId>,
    /// Capabilities projection reference for this task.
    pub capabilities_ref: CapabilitiesId,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

/// Candidate projection. Carries no UI-owned viewed/compare marker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateSnapshot {
    /// Canonical candidate tuple.
    pub candidate_ref: CandidateRef,
    /// Owning task.
    pub task_ref: TaskId,
    /// Iteration the candidate belongs to.
    pub iteration_ref: IterationId,
    /// Artifacts produced for this candidate.
    #[serde(default)]
    pub artifact_refs: Vec<ArtifactId>,
    /// Evaluations that reference this candidate.
    #[serde(default)]
    pub evaluation_refs: Vec<EvaluationId>,
    /// Display label (e.g. `A`); never a wire identifier.
    pub display_label: String,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

/// Descriptor reference by representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDescriptorRef {
    /// Representation kind.
    pub representation: RepresentationKind,
    /// Descriptor identity.
    pub descriptor_id: DescriptorId,
}

/// Verification summary of an artifact or representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationSummary {
    /// Verification state.
    pub state: VerificationState,
    /// Digest algorithm, where meaningful.
    #[serde(default)]
    pub algorithm: Option<String>,
    /// Expected digest, where meaningful.
    #[serde(default)]
    pub expected_digest: Option<String>,
    /// Digest actually observed, where the server reports one.
    #[serde(default)]
    pub observed_digest: Option<String>,
}

/// Artifact projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactSnapshot {
    /// Canonical artifact reference.
    pub artifact_ref: ArtifactId,
    /// Canonical artifact digest; distinct from task/candidate identity even
    /// when candidates share bytes.
    pub artifact_digest: ArtifactDigest,
    /// Media type, when known.
    #[serde(default)]
    pub media_type: Option<String>,
    /// Availability state.
    pub availability: Availability,
    /// Descriptor references by representation.
    #[serde(default)]
    pub descriptor_refs: Vec<MediaDescriptorRef>,
    /// Verification summary, when the server reports one.
    #[serde(default)]
    pub verification: Option<VerificationSummary>,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

/// Named evaluation axis and unit. A missing score is `None`, never zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedScore {
    /// Axis name.
    pub name: String,
    /// Unit label (e.g. `score_0_100`).
    #[serde(default)]
    pub unit: Option<String>,
    /// Score value; `None` means not reported.
    #[serde(default)]
    pub value: Option<f64>,
}

/// Evidence reference with explicit availability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// Authorized evidence record reference.
    pub record_ref: EntityRef,
    /// Evidence availability.
    pub status: EvidenceStatus,
    /// Short summary.
    #[serde(default)]
    pub summary: Option<String>,
    /// Detail, when authorized.
    #[serde(default)]
    pub detail: Option<String>,
}

/// Evaluation projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationSnapshot {
    /// Evaluation identity.
    pub evaluation_ref: EvaluationId,
    /// Evaluated candidate tuple.
    pub candidate_ref: CandidateRef,
    /// Artifacts the evaluation applies to.
    #[serde(default)]
    pub artifact_refs: Vec<ArtifactId>,
    /// Decision status.
    pub decision: EvaluationDecision,
    /// Named axes and units.
    #[serde(default)]
    pub axes: Vec<NamedScore>,
    /// Evidence references.
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
    /// Evaluation authority label (open vocabulary).
    pub authority: AuthorityLabel,
    /// Optional confidence. Meaning is declared by the runtime; absence is not
    /// low confidence.
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

/// Iteration projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationSnapshot {
    /// Iteration identity.
    pub iteration_ref: IterationId,
    /// Owning task.
    pub task_ref: TaskId,
    /// Runtime iteration index.
    pub index: u32,
    /// Candidates observed in this iteration.
    #[serde(default)]
    pub candidate_refs: Vec<CandidateRef>,
    /// Refinement records for this iteration.
    #[serde(default)]
    pub refinement_refs: Vec<RefinementId>,
    /// Evidence references for this iteration.
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

/// Formal outcome projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeSnapshot {
    /// Outcome identity.
    pub outcome_ref: OutcomeId,
    /// Owning task.
    pub task_ref: TaskId,
    /// Formal status.
    pub formal_status: FormalStatus,
    /// Formally selected references.
    #[serde(default)]
    pub selected_refs: Vec<FormalSelectionRef>,
    /// Stop reason label (open vocabulary), when the runtime reports one.
    #[serde(default)]
    pub stop_reason: Option<StopReasonLabel>,
    /// Provenance records for this outcome.
    #[serde(default)]
    pub provenance_refs: Vec<ProvenanceId>,
    /// Evidence references for this outcome.
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

/// Graph inspection node (scripted projection, not a graph compiler input).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    /// Node identity.
    pub node_ref: NodeId,
    /// Display label.
    pub label: String,
    /// Layer label (open vocabulary), when known.
    #[serde(default)]
    pub layer: Option<LayerLabel>,
}

/// Graph inspection edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Edge identity.
    pub edge_ref: EdgeId,
    /// Source node.
    pub from: NodeId,
    /// Target node.
    pub to: NodeId,
    /// Edge kind label (open vocabulary), when known.
    #[serde(default)]
    pub kind: Option<EdgeKindLabel>,
}

/// Graph projection. `graph_ref` is the canonical GraphRevision identity; the
/// projection revision is an independent presentation counter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSnapshot {
    /// Canonical graph revision pointer.
    pub graph_ref: GraphRef,
    /// Execution plan references bound to this revision.
    #[serde(default)]
    pub plan_refs: Vec<PlanRef>,
    /// Inspection nodes.
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    /// Inspection edges.
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    /// Provenance records for this graph.
    #[serde(default)]
    pub provenance_refs: Vec<ProvenanceId>,
    /// Refinement records for this graph.
    #[serde(default)]
    pub refinement_refs: Vec<RefinementId>,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

/// Supported command and its reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCapability {
    /// Command family.
    pub operation: CommandOperation,
    /// Support state.
    pub support: SupportState,
    /// Reason when not supported.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Supported media representation and its reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapability {
    /// Representation kind.
    pub representation: RepresentationKind,
    /// Support state.
    pub support: SupportState,
    /// Reason when not supported.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Supported feature and its reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureCapability {
    /// Feature tag.
    pub feature: String,
    /// Support state.
    pub support: SupportState,
    /// Reason when not supported.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Named constraint advertised with capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityConstraint {
    /// Constraint name.
    pub name: String,
    /// Constraint value.
    pub value: String,
}

/// Scope a capabilities projection applies to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CapabilityScope {
    /// Server-wide.
    Server,
    /// Task-scoped.
    Task {
        /// Task the capabilities apply to.
        task_id: TaskId,
    },
    /// Artifact-scoped.
    Artifact {
        /// Artifact the capabilities apply to.
        artifact_id: ArtifactId,
    },
}

/// Creative capabilities projection. Carries no raw provider secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreativeCapabilities {
    /// Capabilities identity.
    pub capabilities_ref: CapabilitiesId,
    /// Scope.
    pub scope: CapabilityScope,
    /// Supported commands.
    #[serde(default)]
    pub commands: Vec<CommandCapability>,
    /// Supported media representations.
    #[serde(default)]
    pub media: Vec<MediaCapability>,
    /// Supported features.
    #[serde(default)]
    pub features: Vec<FeatureCapability>,
    /// Advertised constraints.
    #[serde(default)]
    pub constraints: Vec<CapabilityConstraint>,
    /// Overall availability.
    pub availability: SupportState,
    /// Reason when unavailable/unsupported.
    #[serde(default)]
    pub reason: Option<String>,
    /// Projection revision within the runtime epoch.
    pub projection_revision: Counter,
}

impl CreativeCapabilities {
    /// Explicit support for a command family.
    ///
    /// Returns `None` when the projection does not mention the command; callers
    /// MUST treat missing or unknown capabilities as disabled.
    pub fn command_support(&self, operation: &CommandOperation) -> Option<&SupportState> {
        self.commands
            .iter()
            .find(|capability| &capability.operation == operation)
            .map(|capability| &capability.support)
    }
}

/// Media descriptor: metadata/access descriptor only, never bytes.
///
/// The locator is a same-authority service endpoint or opaque handle, never a
/// filesystem path or unrestricted URL capability. Signed URLs/tokens must not
/// be logged or persisted in plaintext.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDescriptor {
    /// Opaque descriptor identity.
    pub descriptor_id: DescriptorId,
    /// Artifact the descriptor describes.
    pub artifact_ref: ArtifactId,
    /// Representation kind.
    pub representation: RepresentationKind,
    /// Representation version.
    #[serde(default = "default_representation_version")]
    pub representation_version: u32,
    /// Media type, when known.
    #[serde(default)]
    pub media_type: Option<String>,
    /// Byte length, when known.
    #[serde(default)]
    pub byte_length: Option<u64>,
    /// Pixel width, when known.
    #[serde(default)]
    pub width: Option<u32>,
    /// Pixel height, when known.
    #[serde(default)]
    pub height: Option<u32>,
    /// Duration in milliseconds, when known.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Availability state.
    pub availability: Availability,
    /// Verification summary, when meaningful.
    #[serde(default)]
    pub verification: Option<VerificationSummary>,
    /// Authorized opaque access handle or same-authority endpoint.
    #[serde(default)]
    pub access_handle: Option<String>,
    /// Server identity that issued the handle.
    #[serde(default)]
    pub server_identity: Option<String>,
    /// Audience/principal scope the handle is valid for.
    #[serde(default)]
    pub audience: Option<String>,
    /// Handle expiry (Unix milliseconds).
    #[serde(default)]
    pub expires_at_unix_ms: Option<u64>,
}

fn default_representation_version() -> u32 {
    1
}
