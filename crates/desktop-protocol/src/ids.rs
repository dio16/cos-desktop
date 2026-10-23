//! Opaque identifiers, decimal counters, cursors and generic entity references.
//!
//! Display labels are deliberately *not* identifiers. Every identity type here
//! wraps an opaque server-issued string; fixture-only synthetic identities use
//! the isolated `fixture:` namespace and must never be sent to a real server.

use std::fmt;
use std::str::FromStr;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::macros::{open_label, wire_enum};

/// Maximum byte length accepted for an opaque identifier, label or filter value.
pub const MAX_OPAQUE_BYTES: usize = 512;

/// Maximum digit count accepted for a decimal counter (fits in `u128`).
pub const MAX_COUNTER_DIGITS: usize = 39;

/// Validation error for opaque wire primitives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdError {
    message: String,
}

impl IdError {
    /// Create an error with a human-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for IdError {}

pub(crate) fn validate_opaque(raw: &str, what: &str) -> Result<(), IdError> {
    if raw.is_empty() {
        return Err(IdError::new(format!("{what} must not be empty")));
    }
    if raw.len() > MAX_OPAQUE_BYTES {
        return Err(IdError::new(format!(
            "{what} exceeds {MAX_OPAQUE_BYTES} bytes"
        )));
    }
    if raw.chars().any(char::is_control) {
        return Err(IdError::new(format!(
            "{what} must not contain control characters"
        )));
    }
    Ok(())
}

pub(crate) fn validate_wire_value(raw: &str) -> Result<(), IdError> {
    validate_opaque(raw, "wire value")
}

macro_rules! opaque_id {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis struct $name(String);

        impl $name {
            /// Create from an opaque wire string.
            pub fn new(raw: impl Into<String>) -> ::std::result::Result<Self, $crate::ids::IdError> {
                let raw = raw.into();
                $crate::ids::validate_opaque(&raw, stringify!($name))?;
                Ok(Self(raw))
            }

            /// Create a synthetic identifier in the isolated fixture namespace.
            pub fn fixture(local: impl AsRef<str>) -> Self {
                Self(format!("fixture:{}", local.as_ref()))
            }

            /// Borrow the opaque wire string.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Whether this identity is fixture-namespaced and therefore never
            /// acceptable as a real server entity.
            pub fn is_fixture(&self) -> bool {
                self.0.starts_with("fixture:")
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::ids::IdError;

            fn from_str(raw: &str) -> ::std::result::Result<Self, Self::Err> {
                Self::new(raw)
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let raw =
                    <::std::string::String as ::serde::Deserialize>::deserialize(deserializer)?;
                $crate::ids::validate_opaque(&raw, stringify!($name))
                    .map_err(::serde::de::Error::custom)?;
                Ok(Self(raw))
            }
        }
    };
}

opaque_id! {
    /// Canonical creative task identity.
    pub struct TaskId
}
opaque_id! {
    /// Runtime run identity.
    pub struct RunId
}
opaque_id! {
    /// Candidate identity within a run/iteration tuple.
    pub struct CandidateId
}
opaque_id! {
    /// Canonical artifact identity (a reference, not a digest).
    pub struct ArtifactId
}
opaque_id! {
    /// Canonical artifact digest; distinct from any task/candidate identity even
    /// when candidates share bytes.
    pub struct ArtifactDigest
}
opaque_id! {
    /// Evaluation identity.
    pub struct EvaluationId
}
opaque_id! {
    /// Iteration identity.
    pub struct IterationId
}
opaque_id! {
    /// Formal outcome identity.
    pub struct OutcomeId
}
opaque_id! {
    /// Graph identity (revision pointer is [`GraphRef`]).
    pub struct GraphId
}
opaque_id! {
    /// Immutable execution plan identity.
    pub struct PlanId
}
opaque_id! {
    /// Refinement record identity.
    pub struct RefinementId
}
opaque_id! {
    /// Provenance record identity.
    pub struct ProvenanceId
}
opaque_id! {
    /// Graph inspection node identity.
    pub struct NodeId
}
opaque_id! {
    /// Graph inspection edge identity.
    pub struct EdgeId
}
opaque_id! {
    /// Creative capabilities projection identity.
    pub struct CapabilitiesId
}
opaque_id! {
    /// Media descriptor identity.
    pub struct DescriptorId
}
opaque_id! {
    /// Snapshot bundle identity.
    pub struct SnapshotId
}
opaque_id! {
    /// Delta transaction identity.
    pub struct TransactionId
}
opaque_id! {
    /// Client-generated globally unique command identity.
    pub struct CommandId
}
opaque_id! {
    /// Correlation identity for diagnostics.
    pub struct CorrelationId
}
opaque_id! {
    /// Server-issued observation scope identity.
    pub struct ScopeId
}
opaque_id! {
    /// Server identity.
    pub struct ServerId
}
opaque_id! {
    /// Runtime epoch; changes when prior cursor validity cannot be guaranteed.
    pub struct EpochId
}
opaque_id! {
    /// Opaque durable observation position.
    pub struct Cursor
}
opaque_id! {
    /// Client-generated observation request correlation identity.
    pub struct ReadRequestId
}
opaque_id! {
    /// Runtime-issued authorized read context identity.
    pub struct ReadContextId
}
opaque_id! {
    /// Client-generated receipt lookup correlation identity.
    pub struct ReceiptReadId
}

/// Runtime-issued read authorization revision, encoded as a decimal string.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadRevision(Counter);

impl ReadRevision {
    /// Construct from a decimal counter.
    pub fn new(counter: Counter) -> Self {
        Self(counter)
    }

    /// Borrow the underlying decimal counter.
    pub fn as_counter(&self) -> &Counter {
        &self.0
    }

    /// Borrow the decimal revision string.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Serialize for ReadRevision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ReadRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Counter::deserialize(deserializer).map(Self)
    }
}

impl fmt::Display for ReadRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for ReadRevision {
    type Err = IdError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Counter::from_str(raw).map(Self)
    }
}
opaque_id! {
    /// Canonical integrity digest of a staged snapshot or transaction bundle.
    pub struct IntegrityDigest
}

impl IntegrityDigest {
    /// Construct from a crate-internal digest string already known to satisfy
    /// the opaque-string rules (hex rendering of a hash).
    pub(crate) fn new_unchecked(raw: String) -> Self {
        Self(raw)
    }
}
opaque_id! {
    /// Recipe identity used by an explicit creation scope.
    pub struct RecipeId
}
opaque_id! {
    /// Harness version identity referenced by a recipe.
    pub struct HarnessVersionId
}
opaque_id! {
    /// Recipe profile identity.
    pub struct ProfileId
}
opaque_id! {
    /// Generic canonical entity identity carried by [`EntityRef`].
    pub struct EntityId
}

impl CommandId {
    /// Generate a process-unique command identity.
    ///
    /// Bootstrap uses process id, wall-clock nanoseconds and a process counter:
    /// globally unique for fixture use, but predictable. A real transport should
    /// supply a CSPRNG-backed factory instead; the id must stay stable across an
    /// allowed retry and must never be regenerated to hide a lost response.
    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        Self(format!("cmd-{}-{nanos}-{sequence}", std::process::id()))
    }
}

/// Integer counter serialized as a decimal string.
///
/// Used for `projection_revision` and other monotonic counters so no
/// cross-language precision is lost. A counter is compared numerically within
/// one `(runtime_epoch, projection key)`; it is never a timestamp and never a
/// graph revision identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Counter(String);

impl Counter {
    /// Parse a non-empty decimal string.
    pub fn new(raw: impl Into<String>) -> Result<Self, IdError> {
        let raw = raw.into();
        if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(IdError::new("counter must be a non-empty decimal string"));
        }
        if raw.len() > MAX_COUNTER_DIGITS {
            return Err(IdError::new(format!(
                "counter exceeds {MAX_COUNTER_DIGITS} digits"
            )));
        }
        Ok(Self(raw))
    }

    /// Render an integer as a counter.
    pub fn from_u128(value: u128) -> Self {
        Self(value.to_string())
    }

    /// Borrow the decimal string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Numeric value when it fits in `u128`.
    pub fn to_u128(&self) -> Option<u128> {
        self.0.parse().ok()
    }

    /// Numerically greater-than comparison, or `None` when either side exceeds
    /// `u128` (in which case callers must treat monotonicity as unverified).
    pub fn numerically_gt(&self, other: &Counter) -> Option<bool> {
        Some(self.to_u128()? > other.to_u128()?)
    }
}

impl fmt::Display for Counter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Counter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Counter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Counter::new(raw).map_err(de::Error::custom)
    }
}

impl FromStr for Counter {
    type Err = IdError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::new(raw)
    }
}

wire_enum! {
    /// Kind of a referenced entity.
    pub enum EntityKind {
        Task => "TASK",
        Candidate => "CANDIDATE",
        Artifact => "ARTIFACT",
        Evaluation => "EVALUATION",
        Iteration => "ITERATION",
        Outcome => "OUTCOME",
        Graph => "GRAPH",
        Plan => "PLAN",
        Evidence => "EVIDENCE",
        Refinement => "REFINEMENT",
        Provenance => "PROVENANCE",
        Capabilities => "CAPABILITIES",
        MediaDescriptor => "MEDIA_DESCRIPTOR",
        GraphNode => "GRAPH_NODE",
        GraphEdge => "GRAPH_EDGE",
    }
}

wire_enum! {
    /// Kind of a projection carried by a delta or snapshot bundle.
    pub enum ProjectionKind {
        Task => "TASK",
        Candidate => "CANDIDATE",
        Artifact => "ARTIFACT",
        Evaluation => "EVALUATION",
        Iteration => "ITERATION",
        Outcome => "OUTCOME",
        Graph => "GRAPH",
        Capabilities => "CAPABILITIES",
    }
}

/// Generic entity reference: a kind plus an opaque server-issued identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    /// Entity kind.
    pub kind: EntityKind,
    /// Opaque canonical identity.
    pub id: EntityId,
}

open_label! {
    /// Task inner-loop phase label (open vocabulary; runtime-owned).
    ///
    /// The current wire phase is an open string and differs from the runtime's
    /// internal Rust phase enum. Desktop must not branch on this label; the
    /// fixture presentation value `REFINE` is a synthetic scenario label.
    pub struct PhaseLabel
}
open_label! {
    /// Evaluation authority label (open vocabulary; server-owned).
    pub struct AuthorityLabel
}
open_label! {
    /// Stop reason label (open vocabulary; runtime-owned).
    pub struct StopReasonLabel
}
open_label! {
    /// Graph inspection layer label (open vocabulary; runtime-owned).
    pub struct LayerLabel
}
open_label! {
    /// Graph inspection edge kind label (open vocabulary; runtime-owned).
    pub struct EdgeKindLabel
}

/// Observed candidate reference: the exact typed tuple used by the runtime.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CandidateRef {
    /// Owning task.
    pub task_id: TaskId,
    /// Run the candidate was produced in.
    pub run_id: RunId,
    /// Iteration index the candidate belongs to.
    pub iteration: u32,
    /// Candidate identity.
    pub candidate_id: CandidateId,
    /// Canonical artifact reference for the candidate bytes.
    pub artifact_ref: ArtifactId,
}

/// Formal selection reference: runtime-owned typed tuple.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FormalSelectionRef {
    /// Canonical artifact digest.
    pub artifact_digest: ArtifactDigest,
    /// Run the selection was made in.
    pub run_id: RunId,
    /// Evaluation that justified the selection.
    pub evaluation_id: EvaluationId,
    /// Iteration index.
    pub iteration: u32,
}

/// Exact graph revision pointer; never "latest".
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GraphRef {
    /// Graph identity.
    pub graph_id: GraphId,
    /// Canonical graph revision version.
    pub version: u64,
    /// Canonical graph revision digest.
    pub digest: ArtifactDigest,
}

/// Immutable execution plan reference.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlanRef {
    /// Plan identity.
    pub plan_id: PlanId,
    /// Plan digest.
    pub plan_digest: ArtifactDigest,
}
