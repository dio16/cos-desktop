//! Evidence-detail view model (DC-MP-INS2 / MP-INS2).
//!
//! Every row is derived from one authoritative EVALUATION projection. The
//! Runtime currently projects `record_ref` and `status`; `summary` and `detail`
//! remain optional in the contract and are commonly absent. An absent optional
//! value therefore renders as an explicit not-projected marker, never as empty
//! text, and never by fetching a body: the Desktop holds no evidence-body
//! authorization and this module performs no I/O.
//!
//! Detail materialization is on demand. Building the view copies only
//! references the publication already carried; row views exist only after a
//! caller requests the evaluation it shows. A cached detail is dropped as soon
//! as the authoritative evaluation revision or its complete candidate identity
//! changes, so a newer revision is never rendered beside an older detail.

use std::collections::BTreeMap;
use std::sync::Arc;

use desktop_protocol::{
    CandidateRef, Counter, EntityRef, EvaluationId, EvaluationSnapshot, EvidenceRef, EvidenceStatus,
};

/// Explicit marker for an omitted optional summary.
pub const SUMMARY_NOT_PROJECTED: &str = "概要は未投影";

/// Explicit marker for an omitted optional detail body.
pub const DETAIL_NOT_PROJECTED: &str = "詳細は未投影";

/// Upper bound of cached evaluation details in one window.
///
/// Only an explicitly requested evaluation is inserted; the bound keeps a long
/// session from retaining an unbounded detail table.
pub const MAX_EVIDENCE_DETAILS: usize = 8;

/// Per-record display state derived from the authoritative evidence status.
///
/// `PENDING` and `UNSUPPORTED` are not part of the frozen three-value evidence
/// vocabulary yet; the wire enum preserves them as unrecognized values, and
/// this view classifies exactly those two tokens instead of inventing a
/// default. Every other unrecognized value stays [`Self::Unknown`] verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvidenceDisplayState {
    /// The record is authorized and projected as retrievable.
    Available,
    /// The Runtime reports the record is not ready yet.
    Pending,
    /// The record does not exist.
    Missing,
    /// The current principal may not read the record.
    Forbidden,
    /// This Runtime/Desktop pair cannot present the record.
    Unsupported,
    /// Any other authoritative value, preserved verbatim.
    Unknown(Arc<str>),
}

impl EvidenceDisplayState {
    /// Map one projected status; never defaults an unknown value to available.
    pub fn from_wire(status: &EvidenceStatus) -> Self {
        match status {
            EvidenceStatus::Available => Self::Available,
            EvidenceStatus::Missing => Self::Missing,
            EvidenceStatus::Forbidden => Self::Forbidden,
            EvidenceStatus::Unknown(raw) if raw == "PENDING" => Self::Pending,
            EvidenceStatus::Unknown(raw) if raw == "UNSUPPORTED" => Self::Unsupported,
            EvidenceStatus::Unknown(raw) => Self::Unknown(Arc::from(raw.as_str())),
        }
    }

    /// Stable display label: Japanese gloss plus the canonical token.
    pub fn label(&self) -> Arc<str> {
        match self {
            Self::Available => Arc::from("取得可能（AVAILABLE）"),
            Self::Pending => Arc::from("準備中（PENDING）"),
            Self::Missing => Arc::from("見つかりません（MISSING）"),
            Self::Forbidden => Arc::from("許可されていません（FORBIDDEN）"),
            Self::Unsupported => Arc::from("未対応（UNSUPPORTED）"),
            Self::Unknown(raw) => Arc::clone(raw),
        }
    }
}

/// One authoritative evidence reference in a detail view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceRow {
    /// Referenced entity kind, exactly as projected.
    pub record_kind: Arc<str>,
    /// Referenced opaque identity, exactly as projected.
    pub record_id: Arc<str>,
    /// Mapped display state.
    pub state: EvidenceDisplayState,
    /// Display label of the state.
    pub state_label: Arc<str>,
    /// Projected summary, when the Runtime projects one.
    pub summary: Option<Arc<str>>,
    /// Projected detail body, when the Runtime projects one.
    pub detail: Option<Arc<str>>,
    /// Summary text or the explicit not-projected marker.
    pub summary_text: Arc<str>,
    /// Detail text or the explicit not-projected marker.
    pub detail_text: Arc<str>,
    /// One accessible label for the whole row.
    pub ax_label: Arc<str>,
}

/// One reference row as carried by the accepted projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceRecordSource {
    /// Authorized record reference (kind plus opaque identity).
    pub record_ref: EntityRef,
    /// Projected availability.
    pub status: EvidenceStatus,
    /// Optional projected summary.
    pub summary: Option<Arc<str>>,
    /// Optional projected detail body.
    pub detail: Option<Arc<str>>,
}

/// Evidence references that belong to one evaluation revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceGroupSource {
    /// Evaluation the rows were projected for.
    pub evaluation_ref: EvaluationId,
    /// Evaluation projection revision the rows are valid for.
    pub revision: Counter,
    /// Display heading for the group.
    pub heading: Arc<str>,
    /// Reference rows in projection order.
    pub records: Arc<[EvidenceRecordSource]>,
}

impl EvidenceGroupSource {
    /// Copy the evidence references of one identity-checked evaluation.
    pub fn from_evaluation(evaluation: &EvaluationSnapshot, ordinal: usize) -> Self {
        let records = evaluation
            .evidence_refs
            .iter()
            .map(source_record)
            .collect::<Vec<_>>();
        Self {
            evaluation_ref: evaluation.evaluation_ref.clone(),
            revision: evaluation.projection_revision.clone(),
            heading: Arc::from(format!("評価 {}", ordinal + 1)),
            records: records.into(),
        }
    }

    /// Stable disclosure key: one group is open only for this revision.
    pub fn disclosure_key(&self) -> Arc<str> {
        Arc::from(format!(
            "evidence-group:{}@{}",
            self.evaluation_ref.as_str(),
            self.revision.as_str()
        ))
    }

    /// Build the display detail for this revision. No value is fetched.
    fn detail(&self, candidate: &CandidateRef) -> EvidenceDetail {
        let rows = self.records.iter().map(row).collect::<Vec<_>>();
        let projected_summary_count = rows.iter().filter(|row| row.summary.is_some()).count();
        let projected_detail_count = rows.iter().filter(|row| row.detail.is_some()).count();
        let state_counts = state_counts(&rows);
        let ax_summary: Arc<str> = Arc::from(format!(
            "{}。根拠{}件。{}",
            self.heading,
            rows.len(),
            ax_counts(&state_counts)
        ));
        EvidenceDetail {
            candidate: candidate.clone(),
            evaluation_ref: self.evaluation_ref.clone(),
            evaluation_revision: self.revision.clone(),
            heading: Arc::clone(&self.heading),
            rows: rows.into(),
            projected_summary_count,
            projected_detail_count,
            ax_summary,
        }
    }
}

/// Evidence references already admitted for one candidate.
///
/// The source carries references only; it never carries a body and never
/// performs a fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceSource {
    /// Candidate every group was identity-checked against.
    pub candidate: CandidateRef,
    /// Evaluation groups in projection order.
    pub groups: Arc<[EvidenceGroupSource]>,
}

impl EvidenceSource {
    /// Build a source for one viewed candidate from already checked groups.
    pub fn new(candidate: CandidateRef, groups: Vec<EvidenceGroupSource>) -> Self {
        Self {
            candidate,
            groups: groups.into(),
        }
    }

    /// The group for one exact evaluation identity.
    pub fn group(&self, evaluation_ref: &EvaluationId) -> Option<&EvidenceGroupSource> {
        self.groups
            .iter()
            .find(|group| &group.evaluation_ref == evaluation_ref)
    }
}

/// One materialized evidence detail; only requested evaluations exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceDetail {
    /// Candidate the detail was built for.
    pub candidate: CandidateRef,
    /// Evaluation the rows belong to.
    pub evaluation_ref: EvaluationId,
    /// Evaluation revision the rows are valid for.
    pub evaluation_revision: Counter,
    /// Group heading.
    pub heading: Arc<str>,
    /// Detailed rows in projection order.
    pub rows: Arc<[EvidenceRow]>,
    /// Rows whose summary was projected.
    pub projected_summary_count: usize,
    /// Rows whose detail body was projected.
    pub projected_detail_count: usize,
    /// One accessible summary for the group.
    pub ax_summary: Arc<str>,
}

/// On-demand cache of evidence details keyed by exact evaluation identity.
///
/// [`Self::sync`] is the stale guard: a detail survives only while the current
/// source still carries the same candidate and the same evaluation revision.
/// [`Self::request`] is the only constructor of row views.
#[derive(Debug, Default)]
pub struct EvidenceDetailCache {
    entries: BTreeMap<EvaluationId, EvidenceDetail>,
}

impl EvidenceDetailCache {
    /// Drop every detail whose evaluation revision, candidate or group no
    /// longer matches the current source. Returns the number removed.
    pub fn sync(&mut self, source: Option<&EvidenceSource>) -> usize {
        let before = self.entries.len();
        self.entries.retain(|evaluation_ref, detail| {
            source.is_some_and(|source| {
                source.candidate == detail.candidate
                    && source
                        .group(evaluation_ref)
                        .is_some_and(|group| group.revision == detail.evaluation_revision)
            })
        });
        before - self.entries.len()
    }

    /// Materialize one evaluation's detail, replacing a stale revision.
    ///
    /// Returns `None` when the current source does not carry that evaluation;
    /// any previous detail for it is removed first, so a missing evaluation can
    /// never leave stale rows addressable.
    pub fn request(
        &mut self,
        source: &EvidenceSource,
        evaluation_ref: &EvaluationId,
    ) -> Option<&EvidenceDetail> {
        let Some(group) = source.group(evaluation_ref) else {
            self.entries.remove(evaluation_ref);
            return None;
        };
        let stale = self.entries.get(evaluation_ref).is_none_or(|detail| {
            detail.candidate != source.candidate
                || detail.evaluation_revision != group.revision
                || detail.evaluation_ref != *evaluation_ref
        });
        if stale {
            if !self.entries.contains_key(evaluation_ref)
                && self.entries.len() >= MAX_EVIDENCE_DETAILS
                && let Some(oldest) = self.entries.keys().next().cloned()
            {
                self.entries.remove(&oldest);
            }
            self.entries
                .insert(evaluation_ref.clone(), group.detail(&source.candidate));
        }
        self.entries.get(evaluation_ref)
    }

    /// The materialized detail for one evaluation, without building one.
    pub fn get(&self, evaluation_ref: &EvaluationId) -> Option<&EvidenceDetail> {
        self.entries.get(evaluation_ref)
    }

    /// Number of materialized details.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no detail has been materialized in this window.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop every materialized detail (authorization loss or task switch).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn source_record(evidence: &EvidenceRef) -> EvidenceRecordSource {
    EvidenceRecordSource {
        record_ref: evidence.record_ref.clone(),
        status: evidence.status.clone(),
        summary: evidence.summary.as_deref().map(Arc::from),
        detail: evidence.detail.as_deref().map(Arc::from),
    }
}

fn row(record: &EvidenceRecordSource) -> EvidenceRow {
    let state = EvidenceDisplayState::from_wire(&record.status);
    let state_label = state.label();
    let summary_text: Arc<str> = record
        .summary
        .as_deref()
        .map_or_else(|| Arc::from(SUMMARY_NOT_PROJECTED), Arc::from);
    let detail_text: Arc<str> = record
        .detail
        .as_deref()
        .map_or_else(|| Arc::from(DETAIL_NOT_PROJECTED), Arc::from);
    let record_kind: Arc<str> = Arc::from(record.record_ref.kind.as_str());
    let record_id: Arc<str> = Arc::from(record.record_ref.id.as_str());
    let ax_label: Arc<str> = Arc::from(format!(
        "根拠レコード {} {}。利用状態: {}。概要: {}。詳細: {}。",
        record_kind, record_id, state_label, summary_text, detail_text
    ));
    EvidenceRow {
        record_kind,
        record_id,
        state,
        state_label,
        summary: record.summary.clone(),
        detail: record.detail.clone(),
        summary_text,
        detail_text,
        ax_label,
    }
}

fn state_counts(rows: &[EvidenceRow]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(row.state_label.to_string()).or_insert(0) += 1;
    }
    counts
}

fn ax_counts(counts: &BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(label, count)| format!("{label} {count}件"))
        .collect::<Vec<_>>()
        .join("、")
}

#[cfg(test)]
mod tests {
    use super::{
        DETAIL_NOT_PROJECTED, EvidenceDetailCache, EvidenceDisplayState, EvidenceGroupSource,
        EvidenceSource, MAX_EVIDENCE_DETAILS, SUMMARY_NOT_PROJECTED,
    };
    use desktop_protocol::{
        AuthorityLabel, CandidateId, CandidateRef, Counter, EntityId, EntityKind, EntityRef,
        EvaluationDecision, EvaluationId, EvaluationSnapshot, EvidenceRef, EvidenceStatus, RunId,
        TaskId,
    };

    fn candidate() -> CandidateRef {
        CandidateRef {
            task_id: TaskId::fixture("task/Task-184"),
            run_id: RunId::fixture("run/Task-184/1"),
            iteration: 3,
            candidate_id: CandidateId::fixture("candidate/a"),
            artifact_ref: desktop_protocol::ArtifactId::fixture("artifact/a"),
        }
    }

    fn record(local: &str, status: EvidenceStatus) -> EvidenceRef {
        EvidenceRef {
            record_ref: EntityRef {
                kind: EntityKind::Evidence,
                id: EntityId::fixture(local),
            },
            status,
            summary: None,
            detail: None,
        }
    }

    fn evaluation(revision: u128, evidence: Vec<EvidenceRef>) -> EvaluationSnapshot {
        EvaluationSnapshot {
            evaluation_ref: EvaluationId::fixture("evaluation/a"),
            candidate_ref: candidate(),
            artifact_refs: Vec::new(),
            decision: EvaluationDecision::Pass,
            axes: Vec::new(),
            evidence_refs: evidence,
            authority: AuthorityLabel::new("RUNTIME").expect("valid authority"),
            confidence: None,
            projection_revision: Counter::from_u128(revision),
        }
    }

    fn source(revision: u128, evidence: Vec<EvidenceRef>) -> EvidenceSource {
        let evaluation = evaluation(revision, evidence);
        EvidenceSource::new(
            candidate(),
            vec![EvidenceGroupSource::from_evaluation(&evaluation, 0)],
        )
    }

    #[test]
    fn every_projected_status_maps_to_its_own_display_state() {
        let statuses = [
            (EvidenceStatus::Available, "取得可能（AVAILABLE）"),
            (
                EvidenceStatus::Unknown("PENDING".to_owned()),
                "準備中（PENDING）",
            ),
            (EvidenceStatus::Missing, "見つかりません（MISSING）"),
            (EvidenceStatus::Forbidden, "許可されていません（FORBIDDEN）"),
            (
                EvidenceStatus::Unknown("UNSUPPORTED".to_owned()),
                "未対応（UNSUPPORTED）",
            ),
        ];
        let evidence = statuses
            .iter()
            .enumerate()
            .map(|(index, (status, _))| record(&format!("evidence/{index}"), status.clone()))
            .collect();
        let source = source(1, evidence);
        let mut cache = EvidenceDetailCache::default();
        let detail = cache
            .request(&source, &EvaluationId::fixture("evaluation/a"))
            .expect("detail is built on request");
        assert_eq!(detail.rows.len(), statuses.len());
        for (row, (_, label)) in detail.rows.iter().zip(statuses.iter()) {
            assert_eq!(row.state_label.as_ref(), *label);
        }
        assert_eq!(detail.rows[0].state, EvidenceDisplayState::Available);
        assert_eq!(detail.rows[1].state, EvidenceDisplayState::Pending);
        assert_eq!(detail.rows[2].state, EvidenceDisplayState::Missing);
        assert_eq!(detail.rows[3].state, EvidenceDisplayState::Forbidden);
        assert_eq!(detail.rows[4].state, EvidenceDisplayState::Unsupported);
    }

    #[test]
    fn an_unrecognized_status_stays_verbatim_and_is_never_available() {
        let source = source(
            1,
            vec![record(
                "evidence/future",
                EvidenceStatus::Unknown("FUTURE_STATE".to_owned()),
            )],
        );
        let mut cache = EvidenceDetailCache::default();
        let detail = cache
            .request(&source, &EvaluationId::fixture("evaluation/a"))
            .expect("detail");
        assert_eq!(
            detail.rows[0].state,
            EvidenceDisplayState::Unknown("FUTURE_STATE".into())
        );
        assert_eq!(detail.rows[0].state_label.as_ref(), "FUTURE_STATE");
    }

    #[test]
    fn absent_summary_and_detail_are_explicit_not_projected_markers() {
        let source = source(1, vec![record("evidence/a", EvidenceStatus::Available)]);
        let mut cache = EvidenceDetailCache::default();
        let detail = cache
            .request(&source, &EvaluationId::fixture("evaluation/a"))
            .expect("detail");
        let row = &detail.rows[0];
        assert_eq!(row.summary, None);
        assert_eq!(row.detail, None);
        assert_eq!(row.summary_text.as_ref(), SUMMARY_NOT_PROJECTED);
        assert_eq!(row.detail_text.as_ref(), DETAIL_NOT_PROJECTED);
        assert!(row.ax_label.contains(SUMMARY_NOT_PROJECTED));
        assert!(row.ax_label.contains(DETAIL_NOT_PROJECTED));
        assert_eq!(detail.projected_summary_count, 0);
        assert_eq!(detail.projected_detail_count, 0);
    }

    #[test]
    fn a_projected_summary_is_shown_verbatim_and_detail_stays_honest() {
        let mut projected = record("evidence/a", EvidenceStatus::Forbidden);
        projected.summary = Some("metric evidence".to_owned());
        projected.detail = Some("authorized detail".to_owned());
        let source = source(1, vec![projected]);
        let mut cache = EvidenceDetailCache::default();
        let detail = cache
            .request(&source, &EvaluationId::fixture("evaluation/a"))
            .expect("detail");
        assert_eq!(detail.rows[0].summary_text.as_ref(), "metric evidence");
        assert_eq!(detail.rows[0].detail_text.as_ref(), "authorized detail");
        assert_eq!(detail.projected_detail_count, 1);
    }

    #[test]
    fn detail_is_materialized_only_when_requested() {
        let source = source(1, vec![record("evidence/a", EvidenceStatus::Available)]);
        let cache = EvidenceDetailCache::default();
        assert!(cache.is_empty(), "building the source must not build rows");
        let mut cache = cache;
        assert_eq!(cache.len(), 0);
        assert!(
            cache
                .request(&source, &EvaluationId::fixture("evaluation/a"))
                .is_some()
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_new_revision_discards_the_older_detail() {
        let first = source(1, vec![record("evidence/a", EvidenceStatus::Available)]);
        let second = source(2, vec![record("evidence/b", EvidenceStatus::Forbidden)]);
        let mut cache = EvidenceDetailCache::default();
        let old = cache
            .request(&first, &EvaluationId::fixture("evaluation/a"))
            .expect("first detail")
            .clone();
        assert_eq!(old.evaluation_revision, Counter::from_u128(1));

        // A render that observes the newer source drops the older detail even
        // before the group is opened again.
        assert_eq!(cache.sync(Some(&second)), 1);
        assert!(cache.get(&EvaluationId::fixture("evaluation/a")).is_none());

        let fresh = cache
            .request(&second, &EvaluationId::fixture("evaluation/a"))
            .expect("second detail");
        assert_eq!(fresh.evaluation_revision, Counter::from_u128(2));
        assert_eq!(fresh.rows[0].record_id.as_ref(), "fixture:evidence/b");
        assert_eq!(fresh.rows[0].state, EvidenceDisplayState::Forbidden);
    }

    #[test]
    fn a_request_for_a_newer_revision_replaces_without_a_sync() {
        let first = source(1, vec![record("evidence/a", EvidenceStatus::Available)]);
        let second = source(2, vec![record("evidence/b", EvidenceStatus::Missing)]);
        let mut cache = EvidenceDetailCache::default();
        cache.request(&first, &EvaluationId::fixture("evaluation/a"));
        let detail = cache
            .request(&second, &EvaluationId::fixture("evaluation/a"))
            .expect("replacement detail");
        assert_eq!(detail.evaluation_revision, Counter::from_u128(2));
        assert_eq!(detail.rows[0].state, EvidenceDisplayState::Missing);
    }

    #[test]
    fn a_vanished_evaluation_removes_its_detail() {
        let source = source(1, vec![record("evidence/a", EvidenceStatus::Available)]);
        let mut cache = EvidenceDetailCache::default();
        cache.request(&source, &EvaluationId::fixture("evaluation/a"));
        let empty = EvidenceSource::new(candidate(), Vec::new());
        assert_eq!(cache.sync(Some(&empty)), 1);
        assert!(
            cache
                .request(&empty, &EvaluationId::fixture("evaluation/a"))
                .is_none()
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn the_cache_is_bounded_and_deterministic() {
        let groups = (0..MAX_EVIDENCE_DETAILS + 2)
            .map(|index| {
                let mut evaluation = evaluation(
                    1,
                    vec![record(
                        &format!("evidence/{index}"),
                        EvidenceStatus::Available,
                    )],
                );
                evaluation.evaluation_ref = EvaluationId::fixture(format!("evaluation/{index}"));
                EvidenceGroupSource::from_evaluation(&evaluation, index)
            })
            .collect::<Vec<_>>();
        let many = EvidenceSource::new(candidate(), groups);
        let mut cache = EvidenceDetailCache::default();
        for group in many.groups.iter() {
            cache.request(&many, &group.evaluation_ref);
        }
        assert_eq!(cache.len(), MAX_EVIDENCE_DETAILS);
        // The first identity in the deterministic key order is evicted first;
        // the most recent request stays addressable.
        assert!(cache.get(&EvaluationId::fixture("evaluation/0")).is_none());
        let last = many.groups.last().expect("last group");
        assert!(cache.get(&last.evaluation_ref).is_some());
    }
}
