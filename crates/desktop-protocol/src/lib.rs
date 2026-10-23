//! # desktop-protocol
//!
//! Versioned Desktop/COS wire contract: projection DTOs, envelopes, snapshot
//! and delta framing, command envelopes, receipts, errors, negotiation and
//! bounds.
//!
//! Status: bootstrap Draft implementation of `docs/architecture/DESKTOP_PROTOCOL.md`.
//! These types are the machine-readable contract source; the runtime validates
//! identities, authorization, transitions and expected revisions behind an
//! adapter. Fixture-only synthetic identities use the isolated `fixture:`
//! namespace and are never acceptable as real server entities.
//!
//! Rules encoded here:
//!
//! - Integer counters are decimal strings ([`Counter`]); opaque identities are
//!   distinct types, never display labels.
//! - Unrecognized enum values deserialize to `Unknown(raw)` and disable
//!   dependent operations instead of being coerced.
//! - Unknown message kinds deserialize to [`ServerMessage::Unknown`]; an
//!   unknown state-changing event must trigger resync (client policy).
//! - Staged bundles are verified with [`integrity_digest_of`] before
//!   application.
//! - Bounds ([`Limits`]) reject oversized declared lengths before allocating.

pub mod bounds;
pub mod canonical;
pub mod errors;
pub mod ids;
mod macros;
pub mod messages;
pub mod projections;
pub mod versions;

pub use bounds::{
    Limits, MAX_CONTROL_FRAME_BYTES, MAX_QUEUE_BYTES, MAX_QUEUE_FRAMES, MAX_STAGED_BUNDLE_BYTES,
};
pub use canonical::{CanonicalError, canonical_json, integrity_digest_of, sha256_digest};
pub use errors::{ErrorCode, FieldError, ProtocolError, RejectReason, ResyncReason};
pub use ids::{
    ArtifactDigest, ArtifactId, AuthorityLabel, CandidateId, CandidateRef, CapabilitiesId,
    CommandId, CorrelationId, Counter, Cursor, DescriptorId, EdgeId, EdgeKindLabel, EntityId,
    EntityKind, EntityRef, EpochId, EvaluationId, FormalSelectionRef, GraphId, GraphRef,
    HarnessVersionId, IdError, IntegrityDigest, IterationId, LayerLabel, MAX_COUNTER_DIGITS,
    MAX_OPAQUE_BYTES, NodeId, OutcomeId, PhaseLabel, PlanId, PlanRef, ProfileId, ProjectionKind,
    ProvenanceId, RecipeId, RefinementId, RunId, ScopeId, ServerId, SnapshotId, StopReasonLabel,
    TaskId, TransactionId,
};
pub use messages::{
    ClientFrame, ClientMessage, ClientWire, CommandEnvelope, CommandPayload, CommandReceipt,
    CommandTarget, CommandValidationError, CreateTaskCommand, DeltaTransaction, NamedParameter,
    ObservationScope, ObservationScopeRequest, OpenObservation, ParameterValue, Projection,
    ProjectionMutation, ReceiptLookup, ReceiptState, RecipeRef, RemoveTarget, ReplayAccepted,
    ResyncRequired, ReuseTaskCommand, ScopeFilter, ServerFrame, ServerMessage, ServerWire,
    SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotEnd, StopTaskCommand, UnknownMessage,
};
pub use projections::{
    ArtifactSnapshot, Availability, CandidateSnapshot, CapabilityConstraint, CapabilityScope,
    CommandCapability, CommandOperation, CreativeCapabilities, EvaluationDecision,
    EvaluationSnapshot, EvidenceRef, EvidenceStatus, FeatureCapability, FormalStatus, GraphEdge,
    GraphNode, GraphSnapshot, IterationSnapshot, MediaCapability, MediaDescriptor,
    MediaDescriptorRef, NamedScore, OutcomeSnapshot, RepresentationKind, SupportState,
    TaskSnapshot, TaskStatus, VerificationState, VerificationSummary,
};
pub use versions::{
    HelloAccepted, HelloRejected, HelloRequest, NegotiationFailure, ProtocolVersions, Version,
    VersionAdvertisement, VersionRange, negotiate, negotiate_session,
};
