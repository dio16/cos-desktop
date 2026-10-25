//! Inspector derivation for the viewed candidate (DC-MP-INS1 / MP-INS1).
//!
//! Fixture/local presentation only: every field is a read-only projection value.
//! Missing values stay missing (never zero, never FAIL, never inferred), evidence
//! bodies are never fetched, and the view is bound to the viewed candidate by
//! complete identity rather than by row position.

use std::collections::BTreeMap;
use std::sync::Arc;

use app_core::SnapshotHandle;
use desktop_protocol::{
    CandidateRef, CandidateSnapshot, Counter, EvaluationSnapshot, EvidenceRef, TaskSnapshot,
};

/// The five inspector sections in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    /// Why this artifact exists in the current task.
    Goal,
    /// Runtime evaluation values for the viewed candidate.
    Evaluation,
    /// Evidence summaries attached to those evaluations.
    Evidence,
    /// Refinement records for the candidate's iteration.
    Refinement,
    /// Formal outcome projection (task-scoped, never inferred).
    Outcome,
}

impl Section {
    /// Every section in display order.
    pub const ALL: [Self; 5] = [
        Self::Goal,
        Self::Evaluation,
        Self::Evidence,
        Self::Refinement,
        Self::Outcome,
    ];

    /// Stable display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Goal => "Goal",
            Self::Evaluation => "Evaluation",
            Self::Evidence => "Evidence",
            Self::Refinement => "Refinement",
            Self::Outcome => "Outcome",
        }
    }
}

/// One labelled projection value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// Field label.
    pub label: Arc<str>,
    /// Projected value text; absent values carry their own explicit text.
    pub value: Arc<str>,
}

impl Field {
    fn new(label: impl Into<Arc<str>>, value: impl Into<Arc<str>>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// One grouped record inside a section (an evaluation or an evidence summary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Optional heading, used when a section holds more than one record.
    pub heading: Option<Arc<str>>,
    /// Fields of this record, in projection order.
    pub fields: Arc<[Field]>,
}

/// One section with its explicit availability and disclosure revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionView {
    /// Section identity.
    pub id: Section,
    /// Whether this section has projected content.
    pub available: bool,
    /// Why the section has no content; `None` when available.
    pub absent_reason: Option<Arc<str>>,
    /// One-line state shown while collapsed.
    pub summary: Arc<str>,
    /// Revision key for disclosure state; never displayed.
    pub revision: Arc<str>,
    /// Records shown while expanded.
    pub entries: Arc<[Entry]>,
}

impl SectionView {
    fn available(
        id: Section,
        summary: impl Into<Arc<str>>,
        revision: impl Into<Arc<str>>,
        entries: Vec<Entry>,
    ) -> Self {
        Self {
            id,
            available: true,
            absent_reason: None,
            summary: summary.into(),
            revision: revision.into(),
            entries: entries.into(),
        }
    }

    fn absent(id: Section, reason: impl Into<Arc<str>>) -> Self {
        let reason = reason.into();
        Self {
            id,
            available: false,
            absent_reason: Some(Arc::clone(&reason)),
            summary: reason,
            revision: "absent".into(),
            entries: Arc::from([]),
        }
    }
}

/// The inspector bound to one viewed candidate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The viewed candidate this view is bound to.
    pub candidate: Option<CandidateRef>,
    /// All five sections in display order, each with an explicit state.
    pub sections: Arc<[SectionView]>,
}

/// Read-only inspector projection, keyed by complete candidate identity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    goal: Option<SectionView>,
    outcome: Option<SectionView>,
    candidates: BTreeMap<CandidateRef, CandidateSections>,
}

#[derive(Clone, Debug, PartialEq)]
struct CandidateSections {
    evaluation: SectionView,
    evidence: SectionView,
    refinement: SectionView,
}

impl Index {
    /// Derive the inspector index from one authorized snapshot and its selected task.
    ///
    /// Only the selected task's candidates are indexed, and only evaluations listed on
    /// the candidate projection are read; every evaluation is additionally checked for
    /// complete identity equality before its values are shown.
    pub fn derive(snapshot: &SnapshotHandle, task: Option<&TaskSnapshot>) -> Self {
        let mut index = Self::default();
        let Some(task) = task else {
            return index;
        };
        index.goal = Some(goal_section(task));
        index.outcome = outcome_section(snapshot, task);
        for candidate in snapshot
            .candidates
            .values()
            .filter(|candidate| candidate.task_ref == task.task_ref)
        {
            index.candidates.insert(
                candidate.candidate_ref.clone(),
                candidate_sections(snapshot, candidate),
            );
        }
        index
    }

    /// Bind the index to the viewed candidate by complete identity.
    pub fn view(&self, viewed: Option<&CandidateRef>) -> View {
        let mut sections = Vec::with_capacity(Section::ALL.len());
        sections.push(self.goal.clone().unwrap_or_else(|| {
            SectionView::absent(
                Section::Goal,
                "No Task projection is present in this observation.",
            )
        }));
        match viewed.and_then(|candidate| self.candidates.get(candidate)) {
            Some(candidate) => {
                sections.push(candidate.evaluation.clone());
                sections.push(candidate.evidence.clone());
                sections.push(candidate.refinement.clone());
            }
            None => {
                let reason: Arc<str> = if viewed.is_some() {
                    "The viewed candidate is not present in the current authorized projection."
                        .into()
                } else {
                    "No candidate is viewed.".into()
                };
                sections.push(SectionView::absent(
                    Section::Evaluation,
                    Arc::clone(&reason),
                ));
                sections.push(SectionView::absent(Section::Evidence, Arc::clone(&reason)));
                sections.push(SectionView::absent(Section::Refinement, reason));
            }
        }
        sections.push(self.outcome.clone().unwrap_or_else(|| {
            SectionView::absent(
                Section::Outcome,
                "No formal outcome is projected for this task.",
            )
        }));
        View {
            candidate: viewed.cloned(),
            sections: sections.into(),
        }
    }
}

/// Progressive-disclosure state for the inspector (DC-MP-INS1).
///
/// The state is scoped to the viewed candidate identity and keyed per section
/// revision. A new revision, a different identity, or a disappearance clears the
/// open set, so a returning identity never resumes stale disclosure as if current.
#[derive(Debug, Default)]
pub struct Disclosure {
    bound: Option<CandidateRef>,
    present: bool,
    open: std::collections::BTreeSet<(Section, Arc<str>)>,
}

impl Disclosure {
    /// Rebind after a projection/viewed update.
    ///
    /// Returns true when the previous open set was discarded (identity change or a
    /// disappearance).
    pub fn sync(&mut self, viewed: Option<&CandidateRef>, present: bool) -> bool {
        let identity_changed = self.bound.as_ref() != viewed;
        let disappeared = self.present && !present;
        if identity_changed || disappeared {
            self.open.clear();
        }
        self.bound = viewed.cloned();
        self.present = present;
        identity_changed || disappeared
    }

    /// Toggle one section at its current revision; returns whether it is now open.
    pub fn toggle(&mut self, section: Section, revision: &str) -> bool {
        let key = (section, Arc::<str>::from(revision));
        if self.open.remove(&key) {
            false
        } else {
            self.open.insert(key);
            true
        }
    }

    /// Whether this exact section revision is open (collapsed by default).
    pub fn is_open(&self, section: Section, revision: &str) -> bool {
        self.open.contains(&(section, Arc::<str>::from(revision)))
    }

    /// Drop every disclosure record.
    ///
    /// Used when observation is blocked: a later re-observation must start collapsed
    /// even when the identity and revision are unchanged (A1).
    pub fn invalidate(&mut self) {
        self.bound = None;
        self.present = false;
        self.open.clear();
    }

    /// Drop disclosure keys whose revision is no longer current.
    pub fn prune(&mut self, current: &[(Section, Arc<str>)]) {
        self.open.retain(|(section, revision)| {
            current
                .iter()
                .any(|(kept, current)| kept == section && current == revision)
        });
    }

    /// Number of open section/revision keys (bounded state for tests).
    pub fn open_count(&self) -> usize {
        self.open.len()
    }
}

fn goal_section(task: &TaskSnapshot) -> SectionView {
    let iteration = task.iteration_index.map_or_else(
        || Arc::<str>::from("not reported"),
        |index| Arc::<str>::from(index.to_string()),
    );
    let entries = vec![Entry {
        heading: None,
        fields: vec![
            Field::new("Task", task.display_id.clone()),
            Field::new("Title", task.title.clone()),
            Field::new("Iteration", iteration),
        ]
        .into(),
    }];
    SectionView::available(
        Section::Goal,
        format!("{} — {}", task.display_id, task.title),
        revision_key(task.projection_revision.clone()),
        entries,
    )
}

fn outcome_section(snapshot: &SnapshotHandle, task: &TaskSnapshot) -> Option<SectionView> {
    let outcome_ref = task.formal_outcome_ref.as_ref()?;
    let Some(outcome) = snapshot.outcomes.get(outcome_ref) else {
        return Some(SectionView::absent(
            Section::Outcome,
            "The formal outcome reference in this task is not present in this observation.",
        ));
    };
    let stop_reason = outcome.stop_reason.as_ref().map_or_else(
        || "not reported".to_owned(),
        |label| label.as_str().to_owned(),
    );
    let entries = vec![Entry {
        heading: None,
        fields: vec![
            Field::new("Formal status", outcome.formal_status.as_str()),
            Field::new("Stop reason", stop_reason),
            Field::new(
                "Formal selection records",
                format!("{} projected", outcome.selected_refs.len()),
            ),
        ]
        .into(),
    }];
    Some(SectionView::available(
        Section::Outcome,
        format!("Formal status: {}", outcome.formal_status.as_str()),
        revision_key(outcome.projection_revision.clone()),
        entries,
    ))
}

fn candidate_sections(
    snapshot: &SnapshotHandle,
    candidate: &CandidateSnapshot,
) -> CandidateSections {
    let mut evaluation_entries = Vec::new();
    let mut evidence_entries = Vec::new();
    let mut revisions = Vec::new();
    let mut decisions: Vec<String> = Vec::new();
    let mut missing_evaluations = 0usize;
    let mut mismatched_evaluations = 0usize;
    for evaluation_ref in &candidate.evaluation_refs {
        let Some(evaluation) = snapshot.evaluations.get(evaluation_ref) else {
            missing_evaluations += 1;
            continue;
        };
        if evaluation.candidate_ref != candidate.candidate_ref {
            // Never display another candidate's evaluation, even if the candidate
            // projection lists it: complete identity equality is the binding rule.
            mismatched_evaluations += 1;
            continue;
        }
        // The key must change when the referenced evaluation identity changes, even
        // if its revision number is unchanged (A2).
        revisions.push(format!(
            "{}@{}",
            evaluation.evaluation_ref.as_str(),
            evaluation.projection_revision.as_str()
        ));
        decisions.push(evaluation.decision.as_str().to_owned());
        evaluation_entries.push(evaluation_entry(evaluation, evaluation_entries.len()));
        for evidence in &evaluation.evidence_refs {
            evidence_entries.push(evidence_entry(evidence));
        }
    }

    let evaluation = if evaluation_entries.is_empty() {
        let reason: Arc<str> = if missing_evaluations > 0 {
            "The referenced evaluation records are not present in this observation.".into()
        } else if mismatched_evaluations > 0 {
            "The projected evaluation does not match the viewed candidate identity.".into()
        } else {
            "No evaluation is projected for the viewed candidate.".into()
        };
        SectionView::absent(Section::Evaluation, reason)
    } else {
        if missing_evaluations > 0 {
            evaluation_entries.push(Entry {
                heading: None,
                fields: vec![Field::new(
                    "Evaluation records not present",
                    format!("{missing_evaluations}"),
                )]
                .into(),
            });
        }
        if mismatched_evaluations > 0 {
            evaluation_entries.push(Entry {
                heading: None,
                fields: vec![Field::new(
                    "Evaluation identity mismatch",
                    format!("{mismatched_evaluations} projection(s) not displayed"),
                )]
                .into(),
            });
        }
        let decision_summary = decisions.join(", ");
        SectionView::available(
            Section::Evaluation,
            format!(
                "{} evaluation(s) · decision {decision_summary}",
                decisions.len()
            ),
            format!("evaluation:{}", revisions.join(",")),
            evaluation_entries,
        )
    };

    let evidence = if evidence_entries.is_empty() {
        SectionView::absent(
            Section::Evidence,
            "No evidence summary is projected for the viewed candidate.",
        )
    } else {
        SectionView::available(
            Section::Evidence,
            format!("{} evidence record(s) projected", evidence_entries.len()),
            format!("evidence:{}", revisions.join(",")),
            evidence_entries,
        )
    };

    let refinement = match snapshot.iterations.get(&candidate.iteration_ref) {
        Some(iteration) if !iteration.refinement_refs.is_empty() => {
            let entries = vec![Entry {
                heading: None,
                fields: vec![
                    Field::new(
                        "Refinement records",
                        format!("{} projected", iteration.refinement_refs.len()),
                    ),
                    Field::new(
                        "Reason",
                        "Not projected by this observation — only refinement references are projected.",
                    ),
                ]
                .into(),
            }];
            SectionView::available(
                Section::Refinement,
                format!(
                    "{} refinement record(s) · reason not projected",
                    iteration.refinement_refs.len()
                ),
                revision_key(iteration.projection_revision.clone()),
                entries,
            )
        }
        Some(_) => SectionView::absent(
            Section::Refinement,
            "No refinement is projected for this iteration.",
        ),
        None => SectionView::absent(
            Section::Refinement,
            "The candidate's iteration is not present in this observation.",
        ),
    };

    CandidateSections {
        evaluation,
        evidence,
        refinement,
    }
}

fn evaluation_entry(evaluation: &EvaluationSnapshot, index: usize) -> Entry {
    let confidence = evaluation
        .confidence
        .map_or_else(|| "not reported".to_owned(), |value| value.to_string());
    let mut fields = vec![
        Field::new("Decision", evaluation.decision.as_str()),
        Field::new("Authority", evaluation.authority.as_str()),
        Field::new("Confidence", confidence),
    ];
    if evaluation.axes.is_empty() {
        fields.push(Field::new("Axes", "None projected"));
    }
    for axis in &evaluation.axes {
        let unit = axis.unit.as_deref();
        let value = match (axis.value, unit) {
            (Some(value), Some(unit)) => format!("{value} {unit}"),
            (Some(value), None) => format!("{value} (unit not reported)"),
            (None, Some(unit)) => format!("not reported (unit {unit})"),
            (None, None) => "not reported".to_owned(),
        };
        fields.push(Field::new(axis.name.clone(), value));
    }
    Entry {
        heading: (index > 0).then(|| Arc::<str>::from(format!("Evaluation {}", index + 1))),
        fields: fields.into(),
    }
}

fn evidence_entry(evidence: &EvidenceRef) -> Entry {
    let summary = evidence
        .summary
        .clone()
        .unwrap_or_else(|| "not projected".to_owned());
    Entry {
        heading: None,
        fields: vec![
            Field::new("Summary", summary),
            Field::new("Availability", evidence.status.as_str()),
        ]
        .into(),
    }
}

fn revision_key(counter: Counter) -> Arc<str> {
    Arc::from(counter.as_str())
}

#[cfg(test)]
mod tests {
    use super::{Disclosure, Section};
    use desktop_protocol::{ArtifactId, CandidateId, CandidateRef, RunId, TaskId};

    fn candidate(label: &str) -> CandidateRef {
        CandidateRef {
            task_id: TaskId::new("fixture:task/Task-184").expect("valid task identity"),
            run_id: RunId::new("fixture:run/Task-184/1").expect("valid run identity"),
            iteration: 3,
            candidate_id: CandidateId::new(format!("fixture:candidate/{label}"))
                .expect("valid candidate"),
            artifact_ref: ArtifactId::new(format!("fixture:artifact/{label}"))
                .expect("valid artifact"),
        }
    }

    #[test]
    fn disclosure_is_collapsed_by_default() {
        let state = Disclosure::default();
        assert!(!state.is_open(Section::Evaluation, "1"));
        assert_eq!(state.open_count(), 0);
    }

    #[test]
    fn disclosure_updates_do_not_reuse_the_previous_revision() {
        let mut state = Disclosure::default();
        assert!(state.toggle(Section::Evaluation, "1"));
        assert!(state.is_open(Section::Evaluation, "1"));
        assert!(!state.is_open(Section::Evaluation, "2"));
        assert!(state.toggle(Section::Evaluation, "2"));
        assert!(state.is_open(Section::Evaluation, "2"));
        state.prune(&[(Section::Evaluation, "2".into())]);
        assert!(!state.is_open(Section::Evaluation, "1"));
        assert!(state.is_open(Section::Evaluation, "2"));
        assert_eq!(state.open_count(), 1);
    }

    #[test]
    fn a_disappearing_identity_starts_collapsed_on_reappearance() {
        let mut state = Disclosure::default();
        let viewed = candidate("a");
        assert!(state.sync(Some(&viewed), true));
        assert!(state.toggle(Section::Evidence, "1"));
        assert!(state.is_open(Section::Evidence, "1"));
        assert!(state.sync(Some(&viewed), false));
        assert!(!state.is_open(Section::Evidence, "1"));
        // Same identity and revision returning is not stale current state.
        state.sync(Some(&viewed), true);
        assert!(!state.is_open(Section::Evidence, "1"));
        assert_eq!(state.open_count(), 0);
    }

    #[test]
    fn a_blocked_observation_invalidates_disclosure_even_with_the_same_revision() {
        let mut state = Disclosure::default();
        let viewed = candidate("a");
        state.sync(Some(&viewed), true);
        assert!(state.toggle(Section::Evaluation, "1"));
        assert!(state.is_open(Section::Evaluation, "1"));
        // Authorization withdrawn: the panel invalidates instead of skipping sync.
        state.invalidate();
        // Re-authorized with the same identity and the same revision.
        state.sync(Some(&viewed), true);
        assert!(
            !state.is_open(Section::Evaluation, "1"),
            "re-observation after a blocked gap starts collapsed"
        );
        assert_eq!(state.open_count(), 0);
    }

    #[test]
    fn a_blocked_observation_still_starts_collapsed_after_identity_or_revision_changes() {
        let mut state = Disclosure::default();
        let a = candidate("a");
        let b = candidate("b");
        state.sync(Some(&a), true);
        assert!(state.toggle(Section::Evidence, "1"));
        state.invalidate();
        state.sync(Some(&b), true);
        assert!(!state.is_open(Section::Evidence, "1"));

        state.sync(Some(&b), true);
        assert!(state.toggle(Section::Evidence, "2"));
        state.invalidate();
        state.sync(Some(&b), true);
        assert!(!state.is_open(Section::Evidence, "2"));
        assert_eq!(state.open_count(), 0);
    }

    #[test]
    fn a_different_identity_clears_disclosure() {
        let mut state = Disclosure::default();
        let a = candidate("a");
        let b = candidate("b");
        state.sync(Some(&a), true);
        assert!(state.toggle(Section::Goal, "3"));
        assert!(state.sync(Some(&b), true));
        assert!(!state.is_open(Section::Goal, "3"));
        assert_eq!(state.open_count(), 0);
    }
}
