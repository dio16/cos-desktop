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
    CandidateRef, CandidateSnapshot, Counter, EvaluationSnapshot, EvidenceRef, TaskId, TaskSnapshot,
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
            Self::Goal => "目的",
            Self::Evaluation => "評価",
            Self::Evidence => "判断の根拠",
            Self::Refinement => "改善",
            Self::Outcome => "結果",
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
            SectionView::absent(Section::Goal, "この情報にはタスクの情報がありません。")
        }));
        match viewed.and_then(|candidate| self.candidates.get(candidate)) {
            Some(candidate) => {
                sections.push(candidate.evaluation.clone());
                sections.push(candidate.evidence.clone());
                sections.push(candidate.refinement.clone());
            }
            None => {
                let reason: Arc<str> = if viewed.is_some() {
                    "表示中の候補は、現在許可された情報に含まれていません。".into()
                } else {
                    "候補は選択されていません。".into()
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
                "このタスクの正式な結果は、受信した情報に含まれていません。",
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
    task: Option<TaskId>,
    bound: Option<CandidateRef>,
    present: bool,
    open: std::collections::BTreeSet<(Section, Arc<str>)>,
}

impl Disclosure {
    /// Rebind the owning Task before any view sync.
    ///
    /// A Task change clears every open record synchronously, so a switch that
    /// completes without an intermediate withheld frame cannot carry the previous
    /// Task's disclosure into a same-revision section of the next Task.
    pub fn sync_task(&mut self, task: Option<&TaskId>) -> bool {
        let changed = self.task.as_ref() != task;
        if changed {
            self.invalidate();
            self.task = task.cloned();
        }
        changed
    }

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
        self.task = None;
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
        || Arc::<str>::from("報告されていません"),
        |index| Arc::<str>::from(index.to_string()),
    );
    let entries = vec![Entry {
        heading: None,
        fields: vec![
            Field::new("タスク", task.display_id.clone()),
            Field::new("タイトル", task.title.clone()),
            Field::new("反復", iteration),
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
            "このタスクが参照する正式な結果は、この観測に含まれていません。",
        ));
    };
    let stop_reason = outcome.stop_reason.as_ref().map_or_else(
        || "報告されていません".to_owned(),
        |label| label.as_str().to_owned(),
    );
    let entries = vec![Entry {
        heading: None,
        fields: vec![
            Field::new("正式状態", formal_status_label(&outcome.formal_status)),
            Field::new("停止理由", stop_reason),
            Field::new(
                "正式選択の記録",
                format!("{}件を表示", outcome.selected_refs.len()),
            ),
        ]
        .into(),
    }];
    Some(SectionView::available(
        Section::Outcome,
        format!("正式状態: {}", formal_status_label(&outcome.formal_status)),
        revision_key(outcome.projection_revision.clone()),
        entries,
    ))
}

/// Japanese gloss plus the canonical token for a closed outcome vocabulary;
/// unrecognized values stay verbatim.
fn formal_status_label(status: &desktop_protocol::FormalStatus) -> String {
    use desktop_protocol::FormalStatus;
    match status {
        FormalStatus::Completed => "完了（COMPLETED）",
        FormalStatus::Partial => "一部完了（PARTIAL）",
        FormalStatus::Failed => "失敗（FAILED）",
        FormalStatus::Cancelled => "中止（CANCELLED）",
        FormalStatus::Unknown(raw) => raw.as_str(),
    }
    .to_owned()
}

/// Japanese gloss plus the canonical token for a closed evidence vocabulary;
/// unrecognized values stay verbatim.
fn evidence_status_label(status: &desktop_protocol::EvidenceStatus) -> String {
    use desktop_protocol::EvidenceStatus;
    match status {
        EvidenceStatus::Available => "取得可能（AVAILABLE）",
        EvidenceStatus::Missing => "見つかりません（MISSING）",
        EvidenceStatus::Forbidden => "許可されていません（FORBIDDEN）",
        EvidenceStatus::Unknown(raw) => raw.as_str(),
    }
    .to_owned()
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
        decisions.push(match &evaluation.decision {
            desktop_protocol::EvaluationDecision::Pass => "合格（PASS）".to_owned(),
            desktop_protocol::EvaluationDecision::Fail => "不合格（FAIL）".to_owned(),
            desktop_protocol::EvaluationDecision::Abstain => "判断保留（ABSTAIN）".to_owned(),
            desktop_protocol::EvaluationDecision::Inconclusive => {
                "結論に至らず（INCONCLUSIVE）".to_owned()
            }
            desktop_protocol::EvaluationDecision::Unknown(raw) => raw.clone(),
        });
        evaluation_entries.push(evaluation_entry(evaluation, evaluation_entries.len()));
        for evidence in &evaluation.evidence_refs {
            evidence_entries.push(evidence_entry(evidence));
        }
    }

    let evaluation = if evaluation_entries.is_empty() {
        let reason: Arc<str> = if missing_evaluations > 0 {
            "参照先の評価記録が、受信した情報に含まれていません。".into()
        } else if mismatched_evaluations > 0 {
            "表示された評価は、選択中の候補と一致しません。".into()
        } else {
            "選択中の候補に評価記録はありません。".into()
        };
        SectionView::absent(Section::Evaluation, reason)
    } else {
        if missing_evaluations > 0 {
            evaluation_entries.push(Entry {
                heading: None,
                fields: vec![Field::new(
                    "見つからない評価記録",
                    format!("{missing_evaluations}件"),
                )]
                .into(),
            });
        }
        if mismatched_evaluations > 0 {
            evaluation_entries.push(Entry {
                heading: None,
                fields: vec![Field::new(
                    "候補と一致しない評価記録",
                    format!("{}件は表示していません", mismatched_evaluations),
                )]
                .into(),
            });
        }
        SectionView::available(
            Section::Evaluation,
            {
                let decision_summary = decisions.join(", ");
                format!("{}件の評価 · 判断: {decision_summary}", decisions.len())
            },
            format!("evaluation:{}", revisions.join(",")),
            evaluation_entries,
        )
    };

    let evidence = if evidence_entries.is_empty() {
        SectionView::absent(Section::Evidence, "選択中の候補に判断の根拠はありません。")
    } else {
        SectionView::available(
            Section::Evidence,
            format!("判断の根拠 {}件を表示", evidence_entries.len()),
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
                        "改善記録",
                        format!("{}件を表示", iteration.refinement_refs.len()),
                    ),
                    Field::new(
                        "理由",
                        "この観測では改善内容は提供されず、参照情報のみが含まれています。",
                    ),
                ]
                .into(),
            }];
            SectionView::available(
                Section::Refinement,
                format!(
                    "改善記録 {}件 · 理由は未提供",
                    iteration.refinement_refs.len()
                ),
                revision_key(iteration.projection_revision.clone()),
                entries,
            )
        }
        Some(_) => SectionView::absent(Section::Refinement, "この反復には改善記録がありません。"),
        None => SectionView::absent(
            Section::Refinement,
            "候補の反復情報がこの観測にありません。",
        ),
    };

    CandidateSections {
        evaluation,
        evidence,
        refinement,
    }
}

fn evaluation_entry(evaluation: &EvaluationSnapshot, index: usize) -> Entry {
    let confidence = evaluation.confidence.map_or_else(
        || "報告されていません".to_owned(),
        |value| value.to_string(),
    );
    let mut fields = vec![
        Field::new(
            "判断",
            match &evaluation.decision {
                desktop_protocol::EvaluationDecision::Pass => "合格（PASS）",
                desktop_protocol::EvaluationDecision::Fail => "不合格（FAIL）",
                desktop_protocol::EvaluationDecision::Abstain => "判断保留（ABSTAIN）",
                desktop_protocol::EvaluationDecision::Inconclusive => {
                    "結論に至らず（INCONCLUSIVE）"
                }
                desktop_protocol::EvaluationDecision::Unknown(raw) => raw.as_str(),
            },
        ),
        Field::new("評価主体", evaluation.authority.as_str()),
        Field::new("確信度", confidence),
    ];
    if evaluation.axes.is_empty() {
        fields.push(Field::new("評価軸", "評価軸の情報はありません"));
    }
    for axis in &evaluation.axes {
        let unit = axis.unit.as_deref();
        let value = match (axis.value, unit) {
            (Some(value), Some(unit)) => format!("{value} {unit}"),
            (Some(value), None) => format!("{value}（単位は報告されていません）"),
            (None, Some(unit)) => format!("報告されていません（単位: {unit}）"),
            (None, None) => "報告されていません".to_owned(),
        };
        fields.push(Field::new(axis.name.clone(), value));
    }
    Entry {
        heading: (index > 0).then(|| Arc::<str>::from(format!("評価 {}", index + 1))),
        fields: fields.into(),
    }
}

fn evidence_entry(evidence: &EvidenceRef) -> Entry {
    let summary = evidence
        .summary
        .clone()
        .unwrap_or_else(|| "概要の情報はありません".to_owned());
    Entry {
        heading: None,
        fields: vec![
            Field::new("概要", summary),
            Field::new("利用可否", evidence_status_label(&evidence.status)),
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
    fn a_task_change_clears_disclosure_without_an_intermediate_frame() {
        let first = TaskId::fixture("task/first");
        let second = TaskId::fixture("task/second");
        let mut state = Disclosure::default();
        assert!(state.sync_task(Some(&first)));
        state.sync(None, false);
        assert!(state.toggle(Section::Outcome, "absent"));
        assert_eq!(state.open_count(), 1);

        // The switch completes before any withheld frame renders: the direct
        // A→B task change alone must clear the open set.
        assert!(state.sync_task(Some(&second)));
        assert_eq!(state.open_count(), 0);
        assert!(!state.is_open(Section::Outcome, "absent"));

        assert!(state.toggle(Section::Outcome, "absent"));
        assert!(!state.sync_task(Some(&second)));
        assert_eq!(state.open_count(), 1);
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
