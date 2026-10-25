//! Intervention strip derivation (DC-MP-INS3 / ADR-0011).
//!
//! Fixture/local presentation only. Version 1 carries no canonical issue collection:
//! every row is a typed, labelled **Desktop-local** condition derived from the
//! authorized observation (connection freshness, Stop/settlement presentation, and
//! unavailable/missing references). Nothing here invents Runtime severity or code,
//! starts an action, or changes a canonical projection.

use std::collections::BTreeMap;
use std::sync::Arc;

use app_core::{ObservationPublication, PublicationConnection, PublicationSource, SnapshotHandle};
use desktop_protocol::{
    ArtifactId, CandidateRef, CapabilitiesId, CommandId, EvaluationId, IterationId, OutcomeId,
    TaskId, TaskSnapshot, TaskStatus,
};

use crate::{StopDisplay, StopState};

/// Provenance of one row. Version 1 can only produce `DesktopLocal`; a future
/// Runtime-supplied issue would render verbatim under `RuntimeIssue` (ADR-0011 §1.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provenance {
    /// A typed, labelled Desktop-local condition.
    DesktopLocal,
}

impl Provenance {
    /// The provenance prefix every row carries.
    pub fn label(self) -> &'static str {
        match self {
            Self::DesktopLocal => "Desktop-local",
        }
    }
}

/// The typed action a local row may offer. All are read-only or local navigation;
/// none sends a canonical command or starts automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// Navigate to the surface/inspector that explains the condition.
    Inspect,
    /// Show the existing read-only receipt/Stop presentation.
    Details,
    /// Re-establish the local observation subscription; no canonical mutation.
    Reconnect,
}

impl Action {
    /// The action's display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Inspect => "Inspect",
            Self::Details => "Details",
            Self::Reconnect => "Reconnect",
        }
    }
}

/// The closed set of v1 local condition kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// The window has no authorized observation.
    ObservationUnavailable,
    /// Connection freshness (its own axis, never an issue).
    Connection,
    /// Stop/settlement presentation.
    Stop,
    /// A referenced identity is absent from the projection.
    MissingReference,
}

impl Kind {
    /// The kind's stable label.
    pub fn label(self) -> &'static str {
        match self {
            Self::ObservationUnavailable => "Observation",
            Self::Connection => "Connection",
            Self::Stop => "Stop",
            Self::MissingReference => "Reference",
        }
    }
}

/// Connection freshness states carried by a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConnectionKind {
    /// The visible projection may be stale.
    Stale,
    /// No session.
    Disconnected,
    /// Handshake in progress.
    Connecting,
    /// Bounded recovery in progress.
    Recovering,
    /// Authorization failure.
    Unauthorized,
    /// No mutually supported version pair.
    Incompatible,
}

/// One missing reference identity; typed so the ordering key never uses display text.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reference {
    /// Capabilities projection absent.
    Capabilities(CapabilitiesId),
    /// Iteration projection absent.
    Iteration(IterationId),
    /// Best-candidate projection absent.
    Candidate(CandidateRef),
    /// Formal outcome projection absent.
    Outcome(OutcomeId),
    /// Accepted brief artifact absent.
    Artifact(ArtifactId),
    /// Evaluation referenced by a candidate is absent.
    Evaluation(EvaluationId),
}

impl Reference {
    /// The reference kind's state word for the row label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Capabilities(_) => "capabilities",
            Self::Iteration(_) => "iteration",
            Self::Candidate(_) => "candidate",
            Self::Outcome(_) => "outcome",
            Self::Artifact(_) => "artifact",
            Self::Evaluation(_) => "evaluation",
        }
    }
}

/// The typed stable identity of one condition (tier 3 of the ratified ordering).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RowIdentity {
    /// Observation unavailable (blocked).
    ObservationUnavailable,
    /// Connection freshness.
    Connection(ConnectionKind),
    /// Stop/settlement state for one command.
    Stop(CommandId),
    /// Missing reference for one task.
    MissingReference {
        /// Owning task.
        task: TaskId,
        /// Missing reference identity.
        reference: Reference,
    },
}

/// One intervention row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Typed stable identity (never display text).
    pub identity: RowIdentity,
    /// Closed condition kind.
    pub kind: Kind,
    /// Task status used by tier 2; `None` for an unknown task.
    pub task_status: Option<TaskStatus>,
    /// Local provenance.
    pub provenance: Provenance,
    /// State-word label (no Runtime severity/code vocabulary).
    pub label: Arc<str>,
    /// Local detail (state words only).
    pub detail: Arc<str>,
    /// Read-only action offered, when one exists.
    pub action: Option<Action>,
}

impl Row {
    /// The complete accessible label: provenance prefix plus state words.
    pub fn display_label(&self) -> String {
        format!("{} · {}", self.provenance.label(), self.label)
    }
}

/// One display group: same-kind conditions collapse to one visible row plus a count
/// (ADR-0011 §2). The model keeps every member; grouping only shapes the presentation.
#[derive(Clone, Debug, PartialEq)]
pub struct StripGroup {
    /// The closed condition kind shared by every member.
    pub kind: Kind,
    /// The group's highest-priority member: the row displayed for the group.
    pub head: Row,
    /// Every member in priority order, the head included.
    pub members: Arc<[Row]>,
}

impl StripGroup {
    /// The number of conditions grouped behind the visible row (always at least one).
    pub fn count(&self) -> usize {
        self.members.len()
    }
}

/// The ordered strip: one visible group row per kind, with every condition still listed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    /// Every ordered, deduplicated condition; grouping never flattens or hides it.
    pub rows: Arc<[Row]>,
    /// One group per kind, ordered by the priority position of its head.
    pub groups: Arc<[StripGroup]>,
}

impl Strip {
    /// Whether no condition is active.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Conditions hidden behind their group's visible row while the list is collapsed.
    pub fn grouped_away(&self) -> usize {
        self.rows.len().saturating_sub(self.groups.len())
    }
}

/// Derived intervention state for one authorized observation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    rows: Arc<[Row]>,
}

impl Index {
    /// Derive v1 rows from the authorized observation.
    ///
    /// `task` is the selected task; `commands` is the same read-only Stop presentation
    /// the shell already renders. No canonical issue collection exists, so tier 1 of
    /// the ratified ordering is empty by construction.
    pub fn derive(
        publication: &ObservationPublication,
        snapshot: &SnapshotHandle,
        task: Option<&TaskSnapshot>,
        commands: &BTreeMap<CommandId, StopDisplay>,
    ) -> Self {
        if publication.source == PublicationSource::Unavailable {
            return Self::blocked();
        }
        let mut rows = Vec::new();
        let task_status = task.map(|task| task.task_status.clone());
        if let Some(row) = connection_row(publication, task_status.clone()) {
            rows.push(row);
        }
        rows.extend(
            commands
                .iter()
                .filter_map(|(identity, display)| stop_row(identity, display, task_status.clone())),
        );
        if let Some(task) = task {
            rows.extend(missing_reference_rows(snapshot, task, task_status));
        }
        Self {
            rows: order_rows(rows).into(),
        }
    }

    /// The blocked-observation strip: exactly one Desktop-local row.
    pub fn blocked() -> Self {
        Self {
            rows: vec![Row {
                identity: RowIdentity::ObservationUnavailable,
                kind: Kind::ObservationUnavailable,
                task_status: None,
                provenance: Provenance::DesktopLocal,
                label: "Observation unavailable".into(),
                detail: "No authorized observation is available in this window.".into(),
                action: Some(Action::Reconnect),
            }]
            .into(),
        }
    }

    /// Every ordered row (for tests and the expanded list).
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The display strip: one visible row per kind group, plus every member in priority order.
    ///
    /// Grouping is presentation only (ADR-0011 §2): `rows` keeps each typed identity and its
    /// priority position, and each group lists all of its members so the full list can be
    /// revealed on demand. The group's head is its highest-priority member.
    pub fn strip(&self) -> Strip {
        let mut grouped: Vec<(Kind, Row, Vec<Row>)> = Vec::new();
        for row in self.rows.iter() {
            match grouped.iter_mut().find(|(kind, _, _)| *kind == row.kind) {
                Some((_, _, members)) => members.push(row.clone()),
                None => grouped.push((row.kind, row.clone(), vec![row.clone()])),
            }
        }
        let groups: Vec<StripGroup> = grouped
            .into_iter()
            .map(|(kind, head, members)| StripGroup {
                kind,
                head,
                members: members.into(),
            })
            .collect();
        Strip {
            rows: Arc::clone(&self.rows),
            groups: groups.into(),
        }
    }
}

fn connection_row(
    publication: &ObservationPublication,
    task_status: Option<TaskStatus>,
) -> Option<Row> {
    let kind = if publication.stale {
        ConnectionKind::Stale
    } else {
        match publication.connection {
            PublicationConnection::Observing => return None,
            PublicationConnection::Disconnected => ConnectionKind::Disconnected,
            PublicationConnection::Connecting => ConnectionKind::Connecting,
            PublicationConnection::Recovering => ConnectionKind::Recovering,
            PublicationConnection::Unauthorized => ConnectionKind::Unauthorized,
            PublicationConnection::Incompatible => ConnectionKind::Incompatible,
        }
    };
    let (label, detail) = match kind {
        ConnectionKind::Stale => (
            "Connection stale",
            "The visible projection may be stale; observation is still local.",
        ),
        ConnectionKind::Disconnected => (
            "Disconnected",
            "No session is connected; observation is unavailable.",
        ),
        ConnectionKind::Connecting => ("Connecting", "A session handshake is in progress."),
        ConnectionKind::Recovering => (
            "Recovering",
            "Bounded recovery of the observation is in progress.",
        ),
        ConnectionKind::Unauthorized => (
            "Unauthorized",
            "Authorization failed; the observation requires correction.",
        ),
        ConnectionKind::Incompatible => (
            "Incompatible",
            "No mutually supported version pair is available.",
        ),
    };
    Some(Row {
        identity: RowIdentity::Connection(kind),
        kind: Kind::Connection,
        task_status,
        provenance: Provenance::DesktopLocal,
        label: label.into(),
        detail: detail.into(),
        action: Some(Action::Reconnect),
    })
}

fn stop_row(
    identity: &CommandId,
    display: &StopDisplay,
    task_status: Option<TaskStatus>,
) -> Option<Row> {
    // Only unsettled/uncertain or acceptance states need attention. Settled states
    // (canonical effect reflected) disappear; a prepared-but-not-sent intent is not a
    // request and is not an intervention.
    let relevant = matches!(
        display.state,
        StopState::AwaitingResponse
            | StopState::Accepted
            | StopState::Unknown
            | StopState::Unavailable
    );
    if !relevant {
        return None;
    }
    Some(Row {
        identity: RowIdentity::Stop(identity.clone()),
        kind: Kind::Stop,
        task_status,
        provenance: Provenance::DesktopLocal,
        label: Arc::clone(&display.label),
        detail: Arc::clone(&display.detail),
        action: Some(Action::Details),
    })
}

fn missing_reference_rows(
    snapshot: &SnapshotHandle,
    task: &TaskSnapshot,
    task_status: Option<TaskStatus>,
) -> Vec<Row> {
    let mut references = Vec::new();
    if !snapshot.capabilities.contains_key(&task.capabilities_ref) {
        references.push(Reference::Capabilities(task.capabilities_ref.clone()));
    }
    if let Some(identity) = task.iteration_ref.as_ref()
        && !matches!(
            snapshot.iterations.get(identity),
            Some(iteration) if iteration.task_ref == task.task_ref
        )
    {
        references.push(Reference::Iteration(identity.clone()));
    }
    if let Some(identity) = task.best_candidate_ref.as_ref()
        && !matches!(
            snapshot.candidates.get(identity),
            Some(candidate) if candidate.task_ref == task.task_ref
        )
    {
        references.push(Reference::Candidate(identity.clone()));
    }
    if let Some(identity) = task.formal_outcome_ref.as_ref()
        && !matches!(
            snapshot.outcomes.get(identity),
            Some(outcome) if outcome.task_ref == task.task_ref
        )
    {
        references.push(Reference::Outcome(identity.clone()));
    }
    if let Some(identity) = task.accepted_brief_ref.as_ref()
        && !snapshot.artifacts.contains_key(identity)
    {
        references.push(Reference::Artifact(identity.clone()));
    }
    for candidate in snapshot
        .candidates
        .values()
        .filter(|candidate| candidate.task_ref == task.task_ref)
    {
        for evaluation in &candidate.evaluation_refs {
            // Only a listed-but-absent evaluation is a missing reference. A candidate
            // with no evaluation references is merely unrequested, not a condition.
            if !snapshot.evaluations.contains_key(evaluation) {
                references.push(Reference::Evaluation(evaluation.clone()));
            }
        }
    }
    references
        .into_iter()
        .map(|reference| Row {
            identity: RowIdentity::MissingReference {
                task: task.task_ref.clone(),
                reference: reference.clone(),
            },
            kind: Kind::MissingReference,
            task_status: task_status.clone(),
            provenance: Provenance::DesktopLocal,
            label: format!("Reference {} missing", reference.label()).into(),
            detail: "A referenced identity is not present in the current authorized projection."
                .into(),
            action: Some(Action::Inspect),
        })
        .collect()
}

/// Tier 2 rank: ACTIVE < ASSIGNED < COMPLETED < CANCELLED < unknown last.
fn status_rank(status: Option<&TaskStatus>) -> u8 {
    match status {
        Some(TaskStatus::Active) => 0,
        Some(TaskStatus::Assigned) => 1,
        Some(TaskStatus::Completed) => 2,
        Some(TaskStatus::Cancelled) => 3,
        // Unrecognized statuses stay unknown and order last, like an absent task.
        Some(TaskStatus::Unknown(_)) | None => 4,
    }
}

/// Tier 2 then tier 3, with identical identities deduplicated to one row.
fn order_rows(rows: Vec<Row>) -> Vec<Row> {
    let mut rows = rows;
    rows.sort_by(|left, right| {
        status_rank(left.task_status.as_ref())
            .cmp(&status_rank(right.task_status.as_ref()))
            .then_with(|| left.identity.cmp(&right.identity))
    });
    rows.dedup_by(|left, right| left.identity == right.identity);
    rows
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use app_core::{
        ObservationPublication, PublicationConnection, PublicationSource, SnapshotHandle,
    };
    use desktop_protocol::{
        ArtifactId, CandidateId, CandidateRef, CapabilitiesId, CommandId, Counter, EvaluationId,
        IterationId, IterationSnapshot, OutcomeId, PhaseLabel, RunId, TaskId, TaskSnapshot,
        TaskStatus,
    };

    use super::{Action, Index, Kind, Provenance, Reference, Row, RowIdentity};

    fn task(status: TaskStatus) -> TaskSnapshot {
        TaskSnapshot {
            task_ref: TaskId::fixture("task/Task-184"),
            display_id: "Task-184".to_owned(),
            title: "Intervention task".to_owned(),
            task_status: status,
            inner_phase: PhaseLabel::new("REFINE").expect("valid phase"),
            iteration_ref: Some(IterationId::fixture("iteration/Task-184/1")),
            iteration_index: Some(1),
            best_candidate_ref: None,
            formal_outcome_ref: None,
            accepted_brief_ref: None,
            capabilities_ref: CapabilitiesId::fixture("capabilities/task"),
            projection_revision: Counter::from_u128(1),
        }
    }

    fn snapshot(task: &TaskSnapshot) -> SnapshotHandle {
        let mut snapshot: SnapshotHandle = Arc::default();
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        state.epoch = Some(desktop_protocol::EpochId::fixture("epoch/1"));
        state.tasks.insert(task.task_ref.clone(), task.clone());
        state.capabilities.insert(
            task.capabilities_ref.clone(),
            desktop_protocol::CreativeCapabilities {
                capabilities_ref: task.capabilities_ref.clone(),
                scope: desktop_protocol::CapabilityScope::Task {
                    task_id: task.task_ref.clone(),
                },
                commands: Vec::new(),
                media: Vec::new(),
                features: Vec::new(),
                constraints: Vec::new(),
                availability: desktop_protocol::SupportState::Supported,
                reason: None,
                projection_revision: Counter::from_u128(1),
            },
        );
        state.iterations.insert(
            IterationId::fixture("iteration/Task-184/1"),
            IterationSnapshot {
                iteration_ref: IterationId::fixture("iteration/Task-184/1"),
                task_ref: task.task_ref.clone(),
                index: 1,
                candidate_refs: Vec::new(),
                refinement_refs: Vec::new(),
                evidence_refs: Vec::new(),
                projection_revision: Counter::from_u128(1),
            },
        );
        snapshot
    }

    fn publication(connection: PublicationConnection, stale: bool) -> ObservationPublication {
        ObservationPublication {
            connection,
            source: PublicationSource::Fixture,
            stale,
            ..ObservationPublication::default()
        }
    }

    fn stop(state: super::StopState) -> super::StopDisplay {
        super::StopDisplay {
            state,
            label: format!("Stop {}", state as u8).into(),
            detail: "Local presentation only.".into(),
            manual_intervention: true,
        }
    }

    /// Three Stop conditions plus one connection row: four conditions across two kinds.
    fn grouped_strip() -> super::Strip {
        let task = task(TaskStatus::Active);
        let mut commands = BTreeMap::new();
        for (index, state) in [
            (1, super::StopState::Unknown),
            (2, super::StopState::AwaitingResponse),
            (3, super::StopState::Accepted),
        ] {
            commands.insert(
                CommandId::fixture(format!("command/stop-{index}")),
                stop(state),
            );
        }
        let index = Index::derive(
            &publication(PublicationConnection::Disconnected, false),
            &snapshot(&task),
            Some(&task),
            &commands,
        );
        index.strip()
    }

    #[test]
    fn v1_has_no_runtime_issue_provenance_and_only_read_only_actions() {
        // Exhaustive matches are the proof at this commit: a Runtime-issued row or a
        // destructive action would not compile without a new variant (ADR-0011 §1).
        fn provenance(value: Provenance) -> &'static str {
            match value {
                Provenance::DesktopLocal => "Desktop-local",
            }
        }
        fn action(value: Action) -> &'static str {
            match value {
                Action::Inspect => "Inspect",
                Action::Details => "Details",
                Action::Reconnect => "Reconnect",
            }
        }
        assert_eq!(provenance(Provenance::DesktopLocal), "Desktop-local");
        assert_eq!(action(Action::Inspect), "Inspect");
        assert_eq!(action(Action::Details), "Details");
        assert_eq!(action(Action::Reconnect), "Reconnect");
    }

    #[test]
    fn unknown_task_status_orders_last() {
        let rows = vec![
            Row {
                identity: RowIdentity::Connection(super::ConnectionKind::Stale),
                kind: Kind::Connection,
                task_status: Some(TaskStatus::Unknown("FUTURE".to_owned())),
                provenance: Provenance::DesktopLocal,
                label: "Connection stale".into(),
                detail: "d".into(),
                action: Some(Action::Reconnect),
            },
            Row {
                identity: RowIdentity::Stop(CommandId::fixture("command/stop")),
                kind: Kind::Stop,
                task_status: None,
                provenance: Provenance::DesktopLocal,
                label: "Stop outcome unknown".into(),
                detail: "d".into(),
                action: Some(Action::Details),
            },
            Row {
                identity: RowIdentity::MissingReference {
                    task: TaskId::fixture("task/Task-184"),
                    reference: Reference::Evaluation(EvaluationId::fixture("evaluation/a")),
                },
                kind: Kind::MissingReference,
                task_status: Some(TaskStatus::Active),
                provenance: Provenance::DesktopLocal,
                label: "Reference evaluation missing".into(),
                detail: "d".into(),
                action: Some(Action::Inspect),
            },
        ];
        let ordered = super::order_rows(rows);
        assert_eq!(
            ordered
                .iter()
                .map(|row| row.identity.clone())
                .collect::<Vec<_>>(),
            vec![
                RowIdentity::MissingReference {
                    task: TaskId::fixture("task/Task-184"),
                    reference: Reference::Evaluation(EvaluationId::fixture("evaluation/a")),
                },
                RowIdentity::Connection(super::ConnectionKind::Stale),
                RowIdentity::Stop(CommandId::fixture("command/stop")),
            ],
            "ACTIVE first, unknown/absent statuses last, identity within a rank"
        );
    }

    #[test]
    fn a_healthy_observation_has_an_empty_strip() {
        let task = task(TaskStatus::Active);
        let index = Index::derive(
            &publication(PublicationConnection::Observing, false),
            &snapshot(&task),
            Some(&task),
            &BTreeMap::new(),
        );
        let strip = index.strip();
        assert!(strip.is_empty());
        assert!(strip.groups.is_empty());
        assert!(strip.rows.is_empty());
    }

    #[test]
    fn a_blocked_observation_yields_exactly_one_local_row() {
        let task = task(TaskStatus::Active);
        let mut commands = BTreeMap::new();
        commands.insert(
            CommandId::fixture("command/stop"),
            stop(super::StopState::Unknown),
        );
        let publication = ObservationPublication {
            source: PublicationSource::Unavailable,
            connection: PublicationConnection::Disconnected,
            stale: true,
            ..ObservationPublication::default()
        };
        let index = Index::derive(&publication, &snapshot(&task), Some(&task), &commands);
        let strip = index.strip();
        assert_eq!(strip.rows.len(), 1);
        assert_eq!(strip.groups.len(), 1);
        let group = &strip.groups[0];
        assert_eq!(group.kind, Kind::ObservationUnavailable);
        assert_eq!(group.count(), 1);
        assert_eq!(group.head.identity, RowIdentity::ObservationUnavailable);
        assert_eq!(
            group.head.display_label(),
            "Desktop-local · Observation unavailable".to_owned()
        );
        assert_eq!(strip.grouped_away(), 0);
    }

    #[test]
    fn connection_state_is_a_local_row_and_never_a_runtime_issue() {
        let task = task(TaskStatus::Active);
        let index = Index::derive(
            &publication(PublicationConnection::Recovering, false),
            &snapshot(&task),
            Some(&task),
            &BTreeMap::new(),
        );
        let row = index
            .strip()
            .groups
            .first()
            .map(|group| group.head.clone())
            .expect("connection row");
        assert_eq!(row.kind, Kind::Connection);
        assert_eq!(row.provenance, Provenance::DesktopLocal);
        assert_eq!(row.display_label(), "Desktop-local · Recovering");
        assert!(row.detail.to_lowercase().contains("recovery"));
        for text in [row.display_label(), row.detail.to_string()] {
            for forbidden in ["Runtime", "severity", "code"] {
                assert!(!text.contains(forbidden), "no Runtime vocabulary: {text}");
            }
        }
        assert_eq!(row.action, Some(Action::Reconnect));
    }

    #[test]
    fn stale_and_disconnected_are_distinct_typed_rows() {
        let task = task(TaskStatus::Active);
        let index = Index::derive(
            &publication(PublicationConnection::Disconnected, true),
            &snapshot(&task),
            Some(&task),
            &BTreeMap::new(),
        );
        assert_eq!(index.rows().len(), 1);
        assert_eq!(
            index.rows()[0].identity,
            RowIdentity::Connection(super::ConnectionKind::Stale)
        );
    }

    #[test]
    fn ranking_is_status_then_typed_identity() {
        let active = task(TaskStatus::Active);
        let completed = task(TaskStatus::Completed);
        let mut rows = vec![
            Row {
                identity: RowIdentity::Connection(super::ConnectionKind::Stale),
                kind: Kind::Connection,
                task_status: Some(TaskStatus::Completed),
                provenance: Provenance::DesktopLocal,
                label: "Connection stale".into(),
                detail: "d".into(),
                action: Some(Action::Reconnect),
            },
            Row {
                identity: RowIdentity::MissingReference {
                    task: TaskId::fixture("task/Task-184"),
                    reference: Reference::Evaluation(EvaluationId::fixture("evaluation/a")),
                },
                kind: Kind::MissingReference,
                task_status: Some(TaskStatus::Active),
                provenance: Provenance::DesktopLocal,
                label: "Reference evaluation missing".into(),
                detail: "d".into(),
                action: Some(Action::Inspect),
            },
            Row {
                identity: RowIdentity::Stop(CommandId::fixture("command/stop")),
                kind: Kind::Stop,
                task_status: Some(TaskStatus::Active),
                provenance: Provenance::DesktopLocal,
                label: "Stop outcome unknown".into(),
                detail: "d".into(),
                action: Some(Action::Details),
            },
        ];
        let ordered = super::order_rows(std::mem::take(&mut rows));
        assert_eq!(
            ordered
                .iter()
                .map(|row| row.identity.clone())
                .collect::<Vec<_>>(),
            vec![
                RowIdentity::Stop(CommandId::fixture("command/stop")),
                RowIdentity::MissingReference {
                    task: TaskId::fixture("task/Task-184"),
                    reference: Reference::Evaluation(EvaluationId::fixture("evaluation/a")),
                },
                RowIdentity::Connection(super::ConnectionKind::Stale),
            ],
            "ACTIVE rows precede the COMPLETED row; identity orders within a rank"
        );
        let _ = (active, completed);
    }

    #[test]
    fn identical_conditions_dedupe_to_one_row() {
        let row = Row {
            identity: RowIdentity::Connection(super::ConnectionKind::Stale),
            kind: Kind::Connection,
            task_status: Some(TaskStatus::Active),
            provenance: Provenance::DesktopLocal,
            label: "Connection stale".into(),
            detail: "d".into(),
            action: Some(Action::Reconnect),
        };
        let ordered = super::order_rows(vec![row.clone(), row.clone()]);
        assert_eq!(ordered.len(), 1);
        let index = Index {
            rows: ordered.into(),
        };
        let strip = index.strip();
        assert_eq!(strip.rows.len(), 1);
        assert_eq!(strip.groups.len(), 1);
        assert_eq!(strip.groups[0].count(), 1);
        assert_eq!(strip.grouped_away(), 0);
    }

    #[test]
    fn same_kind_conditions_group_into_one_visible_row_with_a_count() {
        let strip = grouped_strip();
        assert_eq!(
            strip.rows.len(),
            4,
            "the model keeps three Stop conditions plus the connection condition"
        );
        assert_eq!(
            strip.groups.len(),
            2,
            "one visible row per kind, not one per condition"
        );
        assert_eq!(strip.grouped_away(), 2);
        assert_eq!(
            strip
                .groups
                .iter()
                .map(|group| group.kind)
                .collect::<Vec<_>>(),
            vec![Kind::Connection, Kind::Stop],
            "group order follows each head's priority position"
        );
        let stop_group = strip
            .groups
            .iter()
            .find(|group| group.kind == Kind::Stop)
            .expect("stop group");
        assert_eq!(
            stop_group.count(),
            3,
            "the count is the number of grouped conditions"
        );
        let first_stop = strip
            .rows
            .iter()
            .find(|row| row.kind == Kind::Stop)
            .expect("stop row");
        assert_eq!(
            stop_group.head.identity, first_stop.identity,
            "the displayed row is the group's highest-priority member"
        );
        let connection_group = strip
            .groups
            .iter()
            .find(|group| group.kind == Kind::Connection)
            .expect("connection group");
        assert_eq!(connection_group.count(), 1);
    }

    #[test]
    fn grouping_keeps_every_typed_identity_in_priority_order() {
        let strip = grouped_strip();
        let stop_group = strip
            .groups
            .iter()
            .find(|group| group.kind == Kind::Stop)
            .expect("stop group");
        let members: Vec<_> = stop_group
            .members
            .iter()
            .map(|row| row.identity.clone())
            .collect();
        let ordered: Vec<_> = strip
            .rows
            .iter()
            .filter(|row| row.kind == Kind::Stop)
            .map(|row| row.identity.clone())
            .collect();
        assert_eq!(
            members, ordered,
            "every identity stays, in the model's priority order"
        );
        assert_eq!(
            stop_group.head.identity, members[0],
            "the head is the first member in priority order"
        );
        let distinct: BTreeSet<_> = members.iter().cloned().collect();
        assert_eq!(distinct.len(), 3, "distinct identities are never flattened");
        assert!(
            members
                .iter()
                .all(|identity| matches!(identity, RowIdentity::Stop(_)))
        );
    }

    #[test]
    fn grouping_keeps_the_highest_priority_member_as_the_head() {
        let row = |command: &str, status: TaskStatus| Row {
            identity: RowIdentity::Stop(CommandId::fixture(command)),
            kind: Kind::Stop,
            task_status: Some(status),
            provenance: Provenance::DesktopLocal,
            label: format!("Stop {command}").into(),
            detail: "d".into(),
            action: Some(Action::Details),
        };
        // Insertion order disagrees with priority: the COMPLETED row is supplied first.
        let index = Index {
            rows: super::order_rows(vec![
                row("command/completed", TaskStatus::Completed),
                row("command/active", TaskStatus::Active),
            ])
            .into(),
        };
        let strip = index.strip();
        assert_eq!(strip.groups.len(), 1);
        assert_eq!(strip.groups[0].count(), 2);
        assert_eq!(
            strip.groups[0].head.identity,
            RowIdentity::Stop(CommandId::fixture("command/active")),
            "the displayed row is the highest-priority member, not the first inserted"
        );
        assert_eq!(strip.groups[0].head.task_status, Some(TaskStatus::Active));
    }

    #[test]
    fn the_same_projection_renders_the_same_order_every_time() {
        let task = task(TaskStatus::Active);
        let mut commands = BTreeMap::new();
        commands.insert(
            CommandId::fixture("command/stop"),
            stop(super::StopState::Unknown),
        );
        commands.insert(
            CommandId::fixture("command/stop-2"),
            stop(super::StopState::AwaitingResponse),
        );
        let publication = publication(PublicationConnection::Disconnected, false);
        let snapshot = snapshot(&task);
        let first = Index::derive(&publication, &snapshot, Some(&task), &commands);
        let second = Index::derive(&publication, &snapshot, Some(&task), &commands);
        assert_eq!(
            first
                .rows()
                .iter()
                .map(|row| row.identity.clone())
                .collect::<Vec<_>>(),
            second
                .rows()
                .iter()
                .map(|row| row.identity.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(first, second);
    }

    #[test]
    fn a_resolved_condition_disappears() {
        let mut task = task(TaskStatus::Active);
        task.formal_outcome_ref = Some(OutcomeId::fixture("outcome/main"));
        let publication = publication(PublicationConnection::Observing, false);
        let missing = snapshot(&task);
        let before = Index::derive(&publication, &missing, Some(&task), &BTreeMap::new());
        assert_eq!(before.rows().len(), 1);
        assert!(matches!(
            before.rows()[0].identity,
            RowIdentity::MissingReference {
                reference: Reference::Outcome(_),
                ..
            }
        ));

        let mut resolved = snapshot(&task);
        let state = Arc::get_mut(&mut resolved).expect("snapshot has one owner");
        state.outcomes.insert(
            OutcomeId::fixture("outcome/main"),
            desktop_protocol::OutcomeSnapshot {
                outcome_ref: OutcomeId::fixture("outcome/main"),
                task_ref: task.task_ref.clone(),
                formal_status: desktop_protocol::FormalStatus::Completed,
                selected_refs: Vec::new(),
                stop_reason: None,
                provenance_refs: Vec::new(),
                evidence_refs: Vec::new(),
                projection_revision: Counter::from_u128(1),
            },
        );
        let after = Index::derive(&publication, &resolved, Some(&task), &BTreeMap::new());
        assert!(after.rows().is_empty(), "a resolved cause must not linger");
    }

    #[test]
    fn a_missing_evaluation_reference_is_included_but_unrequested_is_not() {
        let mut task = task(TaskStatus::Active);
        task.best_candidate_ref = None;
        let requested = desktop_protocol::CandidateSnapshot {
            candidate_ref: CandidateRef {
                task_id: task.task_ref.clone(),
                run_id: RunId::fixture("run/Task-184/1"),
                iteration: 1,
                candidate_id: CandidateId::fixture("candidate/a"),
                artifact_ref: ArtifactId::fixture("artifact/a"),
            },
            task_ref: task.task_ref.clone(),
            iteration_ref: IterationId::fixture("iteration/Task-184/1"),
            artifact_refs: Vec::new(),
            evaluation_refs: vec![EvaluationId::fixture("evaluation/absent")],
            display_label: "A".to_owned(),
            projection_revision: Counter::from_u128(1),
        };
        let mut unrequested = requested.clone();
        unrequested.candidate_ref.candidate_id = CandidateId::fixture("candidate/b");
        unrequested.candidate_ref.artifact_ref = ArtifactId::fixture("artifact/b");
        unrequested.evaluation_refs = Vec::new();
        let mut snapshot = snapshot(&task);
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        state
            .candidates
            .insert(requested.candidate_ref.clone(), requested.clone());
        state
            .candidates
            .insert(unrequested.candidate_ref.clone(), unrequested);
        let index = Index::derive(
            &publication(PublicationConnection::Observing, false),
            &snapshot,
            Some(&task),
            &BTreeMap::new(),
        );
        assert_eq!(index.rows().len(), 1);
        assert_eq!(
            index.rows()[0].identity,
            RowIdentity::MissingReference {
                task: task.task_ref.clone(),
                reference: Reference::Evaluation(EvaluationId::fixture("evaluation/absent")),
            }
        );
    }

    #[test]
    fn stop_states_report_without_claiming_completion() {
        let task = task(TaskStatus::Active);
        let mut commands = BTreeMap::new();
        commands.insert(
            CommandId::fixture("command/accepted"),
            super::StopDisplay {
                state: super::StopState::Accepted,
                label: "Stop accepted".into(),
                detail: "Acceptance is not task completion".into(),
                manual_intervention: false,
            },
        );
        commands.insert(
            CommandId::fixture("command/reflected"),
            super::StopDisplay {
                state: super::StopState::CanonicalReflected,
                label: "Stop effect reflected".into(),
                detail: "Task completion remains independently observed".into(),
                manual_intervention: false,
            },
        );
        let index = Index::derive(
            &publication(PublicationConnection::Observing, false),
            &snapshot(&task),
            Some(&task),
            &commands,
        );
        assert_eq!(index.rows().len(), 1);
        let row = &index.rows()[0];
        assert_eq!(row.kind, Kind::Stop);
        assert_eq!(row.display_label(), "Desktop-local · Stop accepted");
        assert!(row.detail.contains("not task completion"));
        assert!(!row.display_label().contains("CANCELLED"));
        assert_eq!(row.action, Some(Action::Details));
    }

    #[test]
    fn v1_rows_offer_only_read_only_actions() {
        let task = task(TaskStatus::Active);
        let mut commands = BTreeMap::new();
        commands.insert(
            CommandId::fixture("command/stop"),
            stop(super::StopState::Unknown),
        );
        let index = Index::derive(
            &publication(PublicationConnection::Disconnected, false),
            &snapshot(&task),
            Some(&task),
            &commands,
        );
        for row in index.rows() {
            assert!(matches!(
                row.action,
                Some(Action::Inspect | Action::Details | Action::Reconnect)
            ));
            assert_eq!(row.provenance, Provenance::DesktopLocal);
            assert!(row.display_label().starts_with("Desktop-local · "));
        }
    }
}
