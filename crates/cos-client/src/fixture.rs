//! File-loaded scripted fixture scenario.
//!
//! A scenario is data, not a runtime: it seeds a synthetic projection state,
//! scripts deltas/receipts/errors and declares command outcomes. The provider
//! never reimplements Domain transitions, never fabricates a winner and never
//! turns an arbitrary command into success. Every synthetic identity must use
//! the isolated `fixture:` namespace, which is validated at load time.

use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use desktop_protocol::{
    CommandId, Cursor, EpochId, IntegrityDigest, Limits, Projection, ProjectionMutation,
    ProtocolError, ReadContextId, ReadInvalidationReason, ReadRevision, ReceiptState, RejectReason,
    ResyncReason, ScopeFilter, ScopeId, ServerId, SnapshotCounts, SnapshotId, TaskId,
    TransactionId, VersionRange, integrity_digest_of,
};
use serde::{Deserialize, Serialize};

use crate::projection::ProjectionState;

/// Fixture definition or loading failure.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// The fixture file could not be read.
    #[error("fixture file {path} could not be read: {source}")]
    Io {
        /// Path that failed.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The fixture JSON is invalid.
    #[error("fixture JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The scenario does not declare explicit fixture mode.
    #[error("fixture scenario must set \"fixture\": true: {0}")]
    NotFixture(String),
    /// The scenario is structurally invalid.
    #[error("fixture scenario is invalid: {0}")]
    Invalid(String),
}

/// Fixture scenario loaded from JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureScenario {
    /// Scenario identity.
    pub scenario_id: String,
    /// Must be `true`; fixture mode is explicit, never a fallback.
    #[serde(default)]
    pub fixture: bool,
    /// Marks authored terminal projections that prove UI handling but are not
    /// real runtime evidence.
    #[serde(default)]
    pub simulated: bool,
    /// Synthetic server identity.
    pub server_identity: ServerId,
    /// Synthetic runtime epoch.
    pub epoch: EpochId,
    /// Handshake advertisement.
    pub hello: FixtureHello,
    /// Observation scope.
    pub scope: FixtureScope,
    /// Resume policy for a second observation request.
    #[serde(default)]
    pub observation_mode: ObservationMode,
    /// Session bounds (may only lower the contract defaults).
    #[serde(default)]
    pub limits: Option<Limits>,
    /// Bounded replay journal capacity.
    #[serde(default)]
    pub journal_capacity: Option<usize>,
    /// Entities per generated snapshot chunk.
    #[serde(default)]
    pub snapshot_chunk_entities: Option<usize>,
    /// Seed state served as the first snapshot.
    pub initial_state: FixtureState,
    /// Scripted steps.
    #[serde(default)]
    pub steps: Vec<FixtureStep>,
    /// Per-command default outcome when no step answers.
    #[serde(default)]
    pub commands: std::collections::BTreeMap<String, CommandPolicy>,
    /// Scripted authorized read handoff (fixture transcript, not authenticated Runtime).
    #[serde(default)]
    pub read_authorization: Option<FixtureReadAuthorization>,
}

/// Scripted authorized read handoff for one fixture context.
///
/// The provider answers an authorized open request with this context's grant and
/// stamps every later state-bearing frame with the same binding. It is a
/// deterministic transcript, never a claim of authenticated Runtime behavior.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureReadAuthorization {
    /// Runtime-issued read context identity.
    pub context_id: ReadContextId,
    /// Monotonic read revision, as a decimal string.
    pub read_revision: ReadRevision,
    /// Whether local lifecycle intents from this context are readable.
    #[serde(default = "default_true")]
    pub local_intents_readable: bool,
    /// Retained identities this context denies.
    #[serde(default)]
    pub denied_commands: Vec<CommandId>,
    /// Identities with no retained server receipt (Allowed/no receipt).
    #[serde(default)]
    pub absent_commands: Vec<CommandId>,
    /// Whether an open request is answered with a grant.
    #[serde(default = "default_true")]
    pub grant_on_open: bool,
    /// Whether authorized receipt lookups are answered; a transcript that must
    /// stay pending answers them itself.
    #[serde(default = "default_true")]
    pub answer_lookups: bool,
}

fn default_true() -> bool {
    true
}

/// Handshake advertisement of the synthetic server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureHello {
    /// Server build label.
    pub server_build: String,
    /// Supported protocol ranges.
    pub protocol: Vec<VersionRange>,
    /// Supported schema ranges.
    pub schema: Vec<VersionRange>,
    /// Advertised dedupe retention window.
    #[serde(default)]
    pub dedupe_retention_ms: Option<u64>,
}

/// Observation scope of the synthetic server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureScope {
    /// Server-issued scope identity.
    pub scope_id: ScopeId,
    /// Observed task set.
    pub task_ids: Vec<TaskId>,
    /// Server-interpreted filters.
    #[serde(default)]
    pub filters: Vec<ScopeFilter>,
}

/// Resume policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationMode {
    /// Attempt journal replay; fall back to resync when the cursor is unknown.
    #[default]
    Replay,
    /// Always require a full resync.
    ResyncRequired,
    /// Always serve a fresh snapshot.
    Snapshot,
}

/// Seed projection state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FixtureState {
    /// Tasks.
    #[serde(default)]
    pub tasks: Vec<desktop_protocol::TaskSnapshot>,
    /// Candidates.
    #[serde(default)]
    pub candidates: Vec<desktop_protocol::CandidateSnapshot>,
    /// Artifacts.
    #[serde(default)]
    pub artifacts: Vec<desktop_protocol::ArtifactSnapshot>,
    /// Evaluations.
    #[serde(default)]
    pub evaluations: Vec<desktop_protocol::EvaluationSnapshot>,
    /// Iterations.
    #[serde(default)]
    pub iterations: Vec<desktop_protocol::IterationSnapshot>,
    /// Outcomes.
    #[serde(default)]
    pub outcomes: Vec<desktop_protocol::OutcomeSnapshot>,
    /// Graphs.
    #[serde(default)]
    pub graphs: Vec<desktop_protocol::GraphSnapshot>,
    /// Capabilities.
    #[serde(default)]
    pub capabilities: Vec<desktop_protocol::CreativeCapabilities>,
}

impl FixtureState {
    /// Snapshot creations for this seed state.
    pub fn mutations(&self) -> Vec<ProjectionMutation> {
        let mut mutations = Vec::new();
        let mut push = |projection: Projection| {
            mutations.push(ProjectionMutation::Upsert {
                base_revision: None,
                projection,
            });
        };
        for task in &self.tasks {
            push(Projection::Task(task.clone()));
        }
        for candidate in &self.candidates {
            push(Projection::Candidate(candidate.clone()));
        }
        for artifact in &self.artifacts {
            push(Projection::Artifact(artifact.clone()));
        }
        for evaluation in &self.evaluations {
            push(Projection::Evaluation(evaluation.clone()));
        }
        for iteration in &self.iterations {
            push(Projection::Iteration(iteration.clone()));
        }
        for outcome in &self.outcomes {
            push(Projection::Outcome(outcome.clone()));
        }
        for graph in &self.graphs {
            push(Projection::Graph(graph.clone()));
        }
        for capabilities in &self.capabilities {
            push(Projection::Capabilities(capabilities.clone()));
        }
        mutations
    }
}

/// Scripted step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureStep {
    /// Step identity (used by manual delivery and diagnostics).
    pub id: String,
    /// Trigger: `after_open`, `after_ms:<1..=86400000>`, `after_ms_once:<1..=86400000>`,
    /// `manual` or `on_command:<KIND>`. The one-shot form only schedules invalidation.
    pub trigger: String,
    /// Actions delivered in order.
    #[serde(default)]
    pub actions: Vec<FixtureAction>,
}

/// One scripted action; a step may combine several.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FixtureAction {
    /// Emit a delta transaction.
    #[serde(default)]
    pub delta: Option<FixtureDelta>,
    /// Emit a receipt (command identity/operation filled by the provider).
    #[serde(default)]
    pub receipt: Option<FixtureReceipt>,
    /// Emit a structured error (command identity filled when applicable).
    #[serde(default)]
    pub error: Option<ProtocolError>,
    /// Emit a raw JSON frame verbatim (malformed/unknown/oversize tests).
    #[serde(default)]
    pub raw: Option<serde_json::Value>,
    /// Emit a resync-required message.
    #[serde(default)]
    pub resync_required: Option<ResyncReason>,
    /// Invalidate the current read binding with a newer revision.
    #[serde(default)]
    pub read_invalidated: Option<ReadInvalidationReason>,
    /// Answer an outstanding authorized receipt lookup from the script.
    #[serde(default)]
    pub receipt_read: Option<FixtureReceiptRead>,
}

/// Scripted answer for one authorized receipt lookup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureReceiptRead {
    /// Scoped outcome: `denied`, `allowed` (retained receipt) or `absent`.
    pub outcome: FixtureReceiptOutcome,
    /// Command identity to answer; the pending lookup when omitted.
    #[serde(default)]
    pub command_id: Option<CommandId>,
}

/// Scripted receipt-read outcome class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureReceiptOutcome {
    /// Denied without disclosing receipt or execution facts.
    Denied,
    /// Allowed with the retained server receipt.
    Allowed,
    /// Allowed with no retained server receipt.
    Absent,
}

/// Scripted delta content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureDelta {
    /// Explicit transaction identity (auto-assigned when absent).
    #[serde(default)]
    pub transaction_id: Option<TransactionId>,
    /// Explicit cursor (auto-assigned when absent).
    #[serde(default)]
    pub cursor: Option<Cursor>,
    /// Explicit predecessor (defaults to the current head).
    #[serde(default)]
    pub predecessor: Option<Cursor>,
    /// Mutations.
    pub mutations: Vec<ProjectionMutation>,
}

/// Scripted receipt template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureReceipt {
    /// Receipt state.
    pub state: ReceiptState,
    /// Command family; filled from the triggering command when omitted.
    #[serde(default)]
    pub operation: Option<desktop_protocol::CommandOperation>,
    /// Task the command targeted or created.
    #[serde(default)]
    pub task_id: Option<TaskId>,
    /// Acceptance timestamp.
    #[serde(default)]
    pub accepted_at: Option<String>,
    /// Rejection reason.
    #[serde(default)]
    pub reject_reason: Option<RejectReason>,
    /// Canonical effect references.
    #[serde(default)]
    pub effect_refs: Vec<desktop_protocol::EntityRef>,
    /// Safe message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Per-command default outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandPolicy {
    /// Default outcome when no scripted step answers the command.
    #[serde(default)]
    pub default: CommandDefault,
}

/// Default command outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandDefault {
    /// Synthesize an accepted receipt (intent recorded; never completion).
    Accepted,
    /// Explicitly unsupported command family.
    #[default]
    Unsupported,
    /// No response (lost-response/unknown-result testing).
    Silent,
}

impl FixtureScenario {
    /// Load a scenario from a file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, FixtureError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| FixtureError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_json(&text)
    }

    /// Parse a scenario from JSON text.
    pub fn from_json(text: &str) -> Result<Self, FixtureError> {
        let scenario: Self = serde_json::from_str(text)?;
        scenario.validate()?;
        Ok(scenario)
    }

    /// Observation request for this scenario.
    pub fn observation_request(&self) -> desktop_protocol::ObservationScopeRequest {
        desktop_protocol::ObservationScopeRequest {
            task_ids: self.scope.task_ids.clone(),
            filters: self.scope.filters.clone(),
        }
    }

    /// First observed task identity.
    pub fn primary_task_id(&self) -> Option<&TaskId> {
        self.scope.task_ids.first()
    }

    /// Effective authorized read context for this transcript.
    ///
    /// A scenario without an explicit context is still a trusted fixture
    /// producer; the synthesized context stays visibly fixture-namespaced.
    pub fn read_authorization(&self) -> FixtureReadAuthorization {
        self.read_authorization
            .clone()
            .unwrap_or_else(|| FixtureReadAuthorization {
                context_id: ReadContextId::new(format!(
                    "fixture:read-context/{}",
                    self.scenario_id
                ))
                .expect("scenario-derived fixture context identity is valid"),
                read_revision: ReadRevision::new(
                    desktop_protocol::Counter::new("1").expect("1 is a valid counter"),
                ),
                local_intents_readable: true,
                denied_commands: Vec::new(),
                absent_commands: Vec::new(),
                grant_on_open: true,
                answer_lookups: true,
            })
    }

    /// Validate fixture isolation, references and script structure.
    pub fn validate(&self) -> Result<(), FixtureError> {
        if !self.fixture {
            return Err(FixtureError::NotFixture(self.scenario_id.clone()));
        }
        require_fixture("server_identity", self.server_identity.as_str())?;
        require_fixture("epoch", self.epoch.as_str())?;
        require_fixture("scope.scope_id", self.scope.scope_id.as_str())?;
        if self.scope.task_ids.is_empty() {
            return Err(FixtureError::Invalid(
                "scope.task_ids must observe at least one task".to_owned(),
            ));
        }
        if self.hello.protocol.is_empty()
            || self.hello.schema.is_empty()
            || !self.hello.protocol.iter().all(VersionRange::is_well_formed)
            || !self.hello.schema.iter().all(VersionRange::is_well_formed)
        {
            return Err(FixtureError::Invalid(
                "hello protocol/schema ranges must be non-empty and well-formed".to_owned(),
            ));
        }
        if let Some(limits) = self.limits
            && (limits.max_frame_bytes == 0
                || limits.max_bundle_bytes == 0
                || limits.max_queue_frames == 0
                || limits.max_queue_bytes == 0)
        {
            return Err(FixtureError::Invalid("limits must be non-zero".to_owned()));
        }
        if self.steps.len() > 1024 {
            return Err(FixtureError::Invalid(
                "at most 1024 fixture steps are allowed".to_owned(),
            ));
        }
        let mut step_ids = std::collections::BTreeSet::new();
        for step in &self.steps {
            if !step_ids.insert(step.id.as_str()) {
                return Err(FixtureError::Invalid(format!(
                    "duplicate step id {}",
                    step.id
                )));
            }
            let trigger = parse_trigger(&step.trigger);
            match &trigger {
                Some(
                    Trigger::AfterOpen
                    | Trigger::AfterDelay(_)
                    | Trigger::AfterDelayOnce(_)
                    | Trigger::Manual,
                ) => {}
                Some(Trigger::OnCommand(operation)) => {
                    let known = matches!(
                        operation.as_str(),
                        "CREATE_TASK" | "STOP_TASK" | "REUSE_TASK"
                    );
                    if !known {
                        return Err(FixtureError::Invalid(format!(
                            "step {} triggers on unknown command {}",
                            step.id, operation
                        )));
                    }
                }
                None => {
                    return Err(FixtureError::Invalid(format!(
                        "step {} has an invalid trigger {}",
                        step.id, step.trigger
                    )));
                }
            }
            if matches!(trigger, Some(Trigger::AfterDelayOnce(_)))
                && (step.actions.len() != 1
                    || step.actions.iter().any(|action| {
                        action.read_invalidated.is_none()
                            || action.delta.is_some()
                            || action.receipt.is_some()
                            || action.error.is_some()
                            || action.raw.is_some()
                            || action.resync_required.is_some()
                            || action.receipt_read.is_some()
                    }))
            {
                return Err(FixtureError::Invalid(
                    "after_ms_once must contain exactly one read_invalidated action".to_owned(),
                ));
            }
            for action in &step.actions {
                if let Some(delta) = &action.delta {
                    if matches!(
                        trigger,
                        Some(Trigger::AfterDelay(_) | Trigger::AfterDelayOnce(_))
                    ) && (delta.transaction_id.is_some()
                        || delta.cursor.is_some()
                        || delta.predecessor.is_some())
                    {
                        return Err(FixtureError::Invalid(
                            "delayed deltas cannot pin transaction_id, cursor or predecessor"
                                .to_owned(),
                        ));
                    }
                    if let Some(transaction_id) = &delta.transaction_id {
                        require_fixture("step delta transaction_id", transaction_id.as_str())?;
                    }
                    if let Some(cursor) = &delta.cursor {
                        require_fixture("step delta cursor", cursor.as_str())?;
                    }
                    if let Some(predecessor) = &delta.predecessor {
                        require_fixture("step delta predecessor", predecessor.as_str())?;
                    }
                    for mutation in &delta.mutations {
                        validate_mutation_ids(mutation)?;
                    }
                }
                if let Some(receipt) = &action.receipt {
                    if let Some(task_id) = &receipt.task_id {
                        require_fixture("receipt task_id", task_id.as_str())?;
                    }
                    for effect in &receipt.effect_refs {
                        require_fixture("receipt effect_ref id", effect.id.as_str())?;
                    }
                }
                if let Some(error) = &action.error
                    && let Some(command_id) = &error.command_id
                {
                    require_fixture("step error command_id", command_id.as_str())?;
                }
                if let Some(read) = &action.receipt_read
                    && let Some(command_id) = &read.command_id
                {
                    require_fixture("step receipt_read command_id", command_id.as_str())?;
                }
            }
        }
        if let Some(read) = &self.read_authorization {
            require_fixture("read_authorization.context_id", read.context_id.as_str())?;
            for command_id in read.denied_commands.iter().chain(&read.absent_commands) {
                require_fixture("read_authorization command_id", command_id.as_str())?;
            }
        }
        for kind in self.commands.keys() {
            if !matches!(kind.as_str(), "CREATE_TASK" | "STOP_TASK" | "REUSE_TASK") {
                return Err(FixtureError::Invalid(format!(
                    "commands declares unknown command kind {kind}"
                )));
            }
        }
        let mutations = self.initial_state.mutations();
        ProjectionState::from_snapshot(&self.epoch, &mutations).map_err(|error| {
            FixtureError::Invalid(format!("initial_state is inconsistent: {error}"))
        })?;
        Ok(())
    }
}

fn require_fixture(what: &str, value: &str) -> Result<(), FixtureError> {
    if value.starts_with("fixture:") {
        Ok(())
    } else {
        Err(FixtureError::Invalid(format!(
            "{what} must use the isolated fixture: namespace (got {value}); display labels and real identities are never fixture identifiers"
        )))
    }
}

fn validate_mutation_ids(mutation: &ProjectionMutation) -> Result<(), FixtureError> {
    let mut ids: Vec<String> = Vec::new();
    match mutation {
        ProjectionMutation::Upsert { projection, .. } => match projection {
            Projection::Task(snapshot) => {
                ids.push(snapshot.task_ref.as_str().to_owned());
                if let Some(iteration) = &snapshot.iteration_ref {
                    ids.push(iteration.as_str().to_owned());
                }
                if let Some(candidate) = &snapshot.best_candidate_ref {
                    push_candidate_ids(&mut ids, candidate);
                }
                if let Some(outcome) = &snapshot.formal_outcome_ref {
                    ids.push(outcome.as_str().to_owned());
                }
                if let Some(brief) = &snapshot.accepted_brief_ref {
                    ids.push(brief.as_str().to_owned());
                }
                ids.push(snapshot.capabilities_ref.as_str().to_owned());
            }
            Projection::Candidate(snapshot) => {
                push_candidate_ids(&mut ids, &snapshot.candidate_ref);
                ids.push(snapshot.task_ref.as_str().to_owned());
                ids.push(snapshot.iteration_ref.as_str().to_owned());
                for artifact in &snapshot.artifact_refs {
                    ids.push(artifact.as_str().to_owned());
                }
                for evaluation in &snapshot.evaluation_refs {
                    ids.push(evaluation.as_str().to_owned());
                }
            }
            Projection::Artifact(snapshot) => {
                ids.push(snapshot.artifact_ref.as_str().to_owned());
                ids.push(snapshot.artifact_digest.as_str().to_owned());
                for descriptor in &snapshot.descriptor_refs {
                    ids.push(descriptor.descriptor_id.as_str().to_owned());
                }
            }
            Projection::Evaluation(snapshot) => {
                ids.push(snapshot.evaluation_ref.as_str().to_owned());
                push_candidate_ids(&mut ids, &snapshot.candidate_ref);
                for artifact in &snapshot.artifact_refs {
                    ids.push(artifact.as_str().to_owned());
                }
                for evidence in &snapshot.evidence_refs {
                    ids.push(evidence.record_ref.id.as_str().to_owned());
                }
            }
            Projection::Iteration(snapshot) => {
                ids.push(snapshot.iteration_ref.as_str().to_owned());
                ids.push(snapshot.task_ref.as_str().to_owned());
                for candidate in &snapshot.candidate_refs {
                    push_candidate_ids(&mut ids, candidate);
                }
                for refinement in &snapshot.refinement_refs {
                    ids.push(refinement.as_str().to_owned());
                }
                for evidence in &snapshot.evidence_refs {
                    ids.push(evidence.record_ref.id.as_str().to_owned());
                }
            }
            Projection::Outcome(snapshot) => {
                ids.push(snapshot.outcome_ref.as_str().to_owned());
                ids.push(snapshot.task_ref.as_str().to_owned());
                for selected in &snapshot.selected_refs {
                    ids.push(selected.artifact_digest.as_str().to_owned());
                    ids.push(selected.run_id.as_str().to_owned());
                    ids.push(selected.evaluation_id.as_str().to_owned());
                }
                for provenance in &snapshot.provenance_refs {
                    ids.push(provenance.as_str().to_owned());
                }
                for evidence in &snapshot.evidence_refs {
                    ids.push(evidence.record_ref.id.as_str().to_owned());
                }
            }
            Projection::Graph(snapshot) => {
                ids.push(snapshot.graph_ref.graph_id.as_str().to_owned());
                ids.push(snapshot.graph_ref.digest.as_str().to_owned());
                for plan in &snapshot.plan_refs {
                    ids.push(plan.plan_id.as_str().to_owned());
                    ids.push(plan.plan_digest.as_str().to_owned());
                }
                for node in &snapshot.nodes {
                    ids.push(node.node_ref.as_str().to_owned());
                }
                for edge in &snapshot.edges {
                    ids.push(edge.edge_ref.as_str().to_owned());
                    ids.push(edge.from.as_str().to_owned());
                    ids.push(edge.to.as_str().to_owned());
                }
                for provenance in &snapshot.provenance_refs {
                    ids.push(provenance.as_str().to_owned());
                }
                for refinement in &snapshot.refinement_refs {
                    ids.push(refinement.as_str().to_owned());
                }
            }
            Projection::Capabilities(snapshot) => {
                ids.push(snapshot.capabilities_ref.as_str().to_owned());
            }
        },
        ProjectionMutation::Remove { target, .. } => match target {
            desktop_protocol::RemoveTarget::Task { task_id } => {
                ids.push(task_id.as_str().to_owned());
            }
            desktop_protocol::RemoveTarget::Candidate { candidate_ref } => {
                push_candidate_ids(&mut ids, candidate_ref);
            }
            desktop_protocol::RemoveTarget::Artifact { artifact_id } => {
                ids.push(artifact_id.as_str().to_owned());
            }
            desktop_protocol::RemoveTarget::Evaluation { evaluation_id } => {
                ids.push(evaluation_id.as_str().to_owned());
            }
            desktop_protocol::RemoveTarget::Iteration { iteration_id } => {
                ids.push(iteration_id.as_str().to_owned());
            }
            desktop_protocol::RemoveTarget::Outcome { outcome_id } => {
                ids.push(outcome_id.as_str().to_owned());
            }
            desktop_protocol::RemoveTarget::Graph { graph_id } => {
                ids.push(graph_id.as_str().to_owned());
            }
            desktop_protocol::RemoveTarget::Capabilities { capabilities_id } => {
                ids.push(capabilities_id.as_str().to_owned());
            }
        },
    }
    for id in ids {
        if !id.starts_with("fixture:") {
            return Err(FixtureError::Invalid(format!(
                "scripted mutation uses a non-fixture identity {id}"
            )));
        }
    }
    Ok(())
}

fn push_candidate_ids(ids: &mut Vec<String>, candidate: &desktop_protocol::CandidateRef) {
    ids.push(candidate.task_id.as_str().to_owned());
    ids.push(candidate.run_id.as_str().to_owned());
    ids.push(candidate.candidate_id.as_str().to_owned());
    ids.push(candidate.artifact_ref.as_str().to_owned());
}

/// Parsed step trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Deliver once, immediately after the first snapshot completes.
    AfterOpen,
    /// Deliver after each open grant; a new binding re-arms the step.
    AfterDelay(Duration),
    /// Deliver one invalidation at most once during the entire transcript.
    AfterDelayOnce(Duration),
    /// Deliver when the named command kind arrives.
    OnCommand(String),
    /// Deliver only when a test requests it.
    Manual,
}

/// Parse a trigger string.
pub fn parse_trigger(raw: &str) -> Option<Trigger> {
    if raw == "after_open" {
        return Some(Trigger::AfterOpen);
    }
    if raw == "manual" {
        return Some(Trigger::Manual);
    }
    for (prefix, once) in [("after_ms:", false), ("after_ms_once:", true)] {
        if let Some(milliseconds) = raw
            .strip_prefix(prefix)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=86_400_000).contains(value))
        {
            let delay = Duration::from_millis(milliseconds);
            return Some(if once {
                Trigger::AfterDelayOnce(delay)
            } else {
                Trigger::AfterDelay(delay)
            });
        }
    }
    raw.strip_prefix("on_command:")
        .filter(|kind| !kind.is_empty())
        .map(|kind| Trigger::OnCommand(kind.to_owned()))
}

/// Snapshot bundle generated from the current fixture state.
#[derive(Clone, Debug)]
pub struct SnapshotBundle {
    /// Snapshot identity.
    pub snapshot_id: SnapshotId,
    /// Consistent read-cut cursor.
    pub cursor: Cursor,
    /// Mutation parts.
    pub parts: Vec<Vec<ProjectionMutation>>,
    /// Entity counts.
    pub counts: SnapshotCounts,
    /// Integrity digest over all parts.
    pub digest: IntegrityDigest,
}

#[derive(Debug)]
struct JournalEntry {
    transaction_id: TransactionId,
    cursor: Cursor,
    predecessor: Option<Cursor>,
    mutations: Vec<ProjectionMutation>,
    digest: IntegrityDigest,
}

/// Fixture projection accumulator: the scripted server-side state used to
/// generate coherent snapshots and bounded replay.
#[derive(Debug)]
pub struct FixtureRuntime {
    state: ProjectionState,
    epoch: EpochId,
    head: Cursor,
    snapshot_cursor: Cursor,
    sequence: u64,
    snapshot_sequence: u64,
    journal: VecDeque<JournalEntry>,
    journal_capacity: usize,
    journal_pruned: bool,
    chunk_entities: usize,
}

impl FixtureRuntime {
    /// Build a runtime from a validated scenario.
    pub fn new(scenario: &FixtureScenario) -> Result<Self, FixtureError> {
        let mutations = scenario.initial_state.mutations();
        let state = ProjectionState::from_snapshot(&scenario.epoch, &mutations)
            .map_err(|error| FixtureError::Invalid(error.to_string()))?;
        let head = Cursor::fixture("cursor/0");
        Ok(Self {
            state,
            epoch: scenario.epoch.clone(),
            head: head.clone(),
            snapshot_cursor: head,
            sequence: 1,
            snapshot_sequence: 1,
            journal: VecDeque::new(),
            journal_capacity: scenario.journal_capacity.unwrap_or(32).max(1),
            journal_pruned: false,
            chunk_entities: scenario.snapshot_chunk_entities.unwrap_or(32).max(1),
        })
    }

    /// Current accumulated projection state.
    pub fn state(&self) -> &ProjectionState {
        &self.state
    }

    /// Current epoch.
    pub fn epoch(&self) -> &EpochId {
        &self.epoch
    }

    /// Simulate a runtime restart epoch change.
    pub fn set_epoch(&mut self, epoch: EpochId) {
        self.epoch = epoch;
    }

    /// Generate a complete snapshot from the current state.
    pub fn snapshot_bundle(&mut self) -> Result<SnapshotBundle, FixtureError> {
        let mutations = self.state.snapshot_mutations();
        let counts = SnapshotCounts::of(&mutations);
        let digest = integrity_digest_of(&mutations)
            .map_err(|error| FixtureError::Invalid(error.to_string()))?;
        let snapshot_id = SnapshotId::fixture(format!("snapshot/{}", self.snapshot_sequence));
        self.snapshot_sequence += 1;
        let cursor = self.next_cursor();
        self.snapshot_cursor = cursor.clone();
        self.head = cursor.clone();
        self.journal.clear();
        self.journal_pruned = false;
        let mut parts = Vec::new();
        let mut current = Vec::new();
        for mutation in mutations {
            current.push(mutation);
            if current.len() >= self.chunk_entities {
                parts.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            parts.push(current);
        }
        Ok(SnapshotBundle {
            snapshot_id,
            cursor,
            parts,
            counts,
            digest,
        })
    }

    /// Commit one scripted delta, or rebuild an already-committed one.
    ///
    /// Re-delivery of a known transaction identity emits the authored content
    /// unchanged without touching the accumulator (duplicate/conflict testing).
    pub fn commit_delta(
        &mut self,
        delta: &FixtureDelta,
    ) -> Result<desktop_protocol::DeltaTransaction, FixtureError> {
        let transaction_id = match &delta.transaction_id {
            Some(transaction_id) => transaction_id.clone(),
            None => TransactionId::fixture(format!("transaction/{}", self.sequence)),
        };
        let digest = integrity_digest_of(&delta.mutations)
            .map_err(|error| FixtureError::Invalid(error.to_string()))?;
        if let Some(existing) = self
            .journal
            .iter()
            .find(|entry| entry.transaction_id == transaction_id)
        {
            let cursor = delta
                .cursor
                .clone()
                .unwrap_or_else(|| existing.cursor.clone());
            let predecessor = delta
                .predecessor
                .clone()
                .or_else(|| existing.predecessor.clone())
                .or_else(|| Some(self.snapshot_cursor.clone()));
            return Ok(desktop_protocol::DeltaTransaction {
                transaction_id,
                epoch: self.epoch.clone(),
                cursor,
                predecessor,
                mutation_count: delta.mutations.len() as u32,
                integrity_digest: digest,
                mutations: delta.mutations.clone(),
            });
        }
        let cursor = match &delta.cursor {
            Some(cursor) => cursor.clone(),
            None => self.next_cursor(),
        };
        let predecessor = delta
            .predecessor
            .clone()
            .or_else(|| Some(self.head.clone()));
        for mutation in &delta.mutations {
            self.state.validate_mutation(mutation).map_err(|error| {
                FixtureError::Invalid(format!(
                    "scripted delta {transaction_id} is not applicable: {error}"
                ))
            })?;
        }
        self.state.apply(&delta.mutations);
        self.head = cursor.clone();
        self.journal.push_back(JournalEntry {
            transaction_id: transaction_id.clone(),
            cursor: cursor.clone(),
            predecessor: predecessor.clone(),
            mutations: delta.mutations.clone(),
            digest: digest.clone(),
        });
        while self.journal.len() > self.journal_capacity {
            self.journal.pop_front();
            self.journal_pruned = true;
        }
        Ok(desktop_protocol::DeltaTransaction {
            transaction_id,
            epoch: self.epoch.clone(),
            cursor,
            predecessor,
            mutation_count: delta.mutations.len() as u32,
            integrity_digest: digest,
            mutations: delta.mutations.clone(),
        })
    }

    /// Transactions committed after a cursor, when replay is still possible.
    pub fn replay_after(&self, cursor: &Cursor) -> Option<Vec<desktop_protocol::DeltaTransaction>> {
        if cursor == &self.head {
            return Some(Vec::new());
        }
        if let Some(position) = self
            .journal
            .iter()
            .position(|entry| &entry.cursor == cursor)
        {
            return Some(
                self.journal
                    .iter()
                    .skip(position + 1)
                    .map(|entry| self.entry_to_transaction(entry))
                    .collect(),
            );
        }
        if cursor == &self.snapshot_cursor && !self.journal_pruned {
            return Some(
                self.journal
                    .iter()
                    .map(|entry| self.entry_to_transaction(entry))
                    .collect(),
            );
        }
        None
    }

    fn entry_to_transaction(&self, entry: &JournalEntry) -> desktop_protocol::DeltaTransaction {
        desktop_protocol::DeltaTransaction {
            transaction_id: entry.transaction_id.clone(),
            epoch: self.epoch.clone(),
            cursor: entry.cursor.clone(),
            predecessor: entry.predecessor.clone(),
            mutation_count: entry.mutations.len() as u32,
            integrity_digest: entry.digest.clone(),
            mutations: entry.mutations.clone(),
        }
    }

    fn next_cursor(&mut self) -> Cursor {
        let cursor = Cursor::fixture(format!("cursor/{}", self.sequence));
        self.sequence += 1;
        cursor
    }
}
