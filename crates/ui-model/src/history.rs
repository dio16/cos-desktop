//! History timeline view model and local reopen (DC-MP-NAV3).
//!
//! The rows are the Runtime-served run-level records app-core publishes; this
//! module adds display text only. It never derives history from iteration
//! numbers, never turns an unloaded page into an empty one, and never names a
//! client or a command: `reopen` restores window-local references and cannot
//! mutate canonical selection, outcome or task status.
//!
//! Label rules:
//! * `Idle` is an explicit "not loaded yet" state, distinct from `Empty`;
//! * a bounded load keeps its remaining, unloaded pages visible as a bound, not
//!   as an authoritative end;
//! * absent projected values render as [`UNKNOWN`], never as a plausible value.

use std::sync::Arc;

use app_core::{HistoryEntry, HistoryError, HistoryPublication, HistoryRequest, HistoryStatus};
use desktop_protocol::{CandidateRef, TaskId};

use crate::presentation::{Presentation, ViewedCandidate};

/// Display marker for a value the Runtime did not project.
pub const UNKNOWN: &str = "不明";

/// Display status of one task's history (mirrors [`HistoryStatus`] with view
/// labels; kept separate so presentation wording never redefines the contract).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryLoadState {
    /// Never requested for the current context.
    #[default]
    Idle,
    /// A read is in flight.
    Loading,
    /// A page was loaded and more pages exist.
    Loaded,
    /// The Runtime served an authoritative empty history.
    Empty,
    /// Every page was loaded.
    End,
    /// The read failed with a value-free category.
    Error(HistoryError),
    /// The credential/context cannot read this task's history.
    Unauthorized,
}

impl HistoryLoadState {
    /// Whether the shown rows are an authoritative served result.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Loaded | Self::Empty | Self::End)
    }

    /// Fixed status label for the surface.
    pub fn label(self, has_rows: bool) -> Arc<str> {
        Arc::from(match self {
            Self::Idle => "履歴はまだ読み込まれていません",
            Self::Loading if has_rows => "さらに古い履歴を読み込み中…",
            Self::Loading => "履歴を読み込み中…",
            Self::Loaded => "履歴を表示しています（さらに古い履歴があります）",
            Self::Empty => "このタスクには実行履歴がありません",
            Self::End => "最新の履歴をすべて表示しています",
            Self::Unauthorized => "履歴を表示する許可がありません",
            Self::Error(error) => match error {
                HistoryError::NotFound => "このタスクの履歴が見つかりません",
                HistoryError::Conflict => "観測を開き直す必要があります",
                HistoryError::InvalidRequest => "履歴の読み込み位置が無効です",
                HistoryError::Malformed => "履歴の内容を読み取れません",
                HistoryError::Oversized => "履歴の応答が大きすぎます",
                HistoryError::Unavailable => "履歴を取得できません",
                HistoryError::Unsupported => "この環境では履歴を取得できません",
            },
        })
    }
}

/// One rendered timeline row; only run-level fields plus display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRow {
    /// Position in the loaded timeline (newest first).
    pub index: usize,
    /// Iteration index label; the projected value, never derived from position.
    pub iteration_label: Arc<str>,
    /// Inner phase label.
    pub phase_label: Arc<str>,
    /// Run identity label.
    pub run_label: Arc<str>,
    /// Plan identity label, or [`UNKNOWN`].
    pub plan_label: Arc<str>,
    /// Creation timestamp label.
    pub created_label: Arc<str>,
    /// Runtime record reference.
    pub record_ref: Arc<str>,
    /// Accessible name of the row.
    pub accessible_label: Arc<str>,
    /// Accessible name of the row's reopen action.
    pub reopen_label: Arc<str>,
    /// Iteration the row reopens; `None` when the value cannot be addressed.
    pub reopen_iteration: Option<u64>,
}

impl HistoryRow {
    fn from_entry(index: usize, entry: &HistoryEntry) -> Self {
        let iteration_label: Arc<str> = Arc::from(entry.iteration.to_string());
        let plan_label: Arc<str> = entry.plan_id.clone().unwrap_or_else(|| Arc::from(UNKNOWN));
        let accessible_label: Arc<str> = Arc::from(format!(
            "反復 {}。フェーズ {}。実行 {}。計画 {}。作成 {}。記録 {}。",
            iteration_label,
            entry.phase,
            entry.run_id,
            plan_label,
            entry.created_at,
            entry.record_ref
        ));
        let reopen_label: Arc<str> = Arc::from(format!(
            "この時点を開く（反復 {}）。ローカル表示のみを復元します",
            iteration_label
        ));
        Self {
            index,
            iteration_label,
            phase_label: Arc::clone(&entry.phase),
            run_label: Arc::clone(&entry.run_id),
            plan_label,
            created_label: Arc::clone(&entry.created_at),
            record_ref: Arc::clone(&entry.record_ref),
            accessible_label,
            reopen_label,
            reopen_iteration: Some(entry.iteration),
        }
    }
}

/// One task's rendered history surface state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryView {
    /// Task the surface currently describes, when a request named one.
    pub task: Option<Arc<str>>,
    /// Display status.
    pub state: HistoryLoadState,
    /// Status label for the surface.
    pub status_label: Arc<str>,
    /// Loaded timeline rows, newest first.
    pub rows: Vec<HistoryRow>,
    /// Whether the bounded load-older action is offered.
    pub can_load_older: bool,
    /// Label of the load-older action or of the reason it is not offered.
    pub load_older_label: Arc<str>,
    /// Whether the entry ceiling stopped further appends.
    pub at_bound: bool,
}

impl HistoryView {
    /// Render one published history view.
    ///
    /// The continuation action is offered only when the Runtime actually issued
    /// a cursor, the view still has more pages and the bounded entry ceiling has
    /// not been reached. Otherwise the label states the exact reason, so
    /// unloaded older pages are never presented as an end.
    pub fn from_publication(publication: &HistoryPublication) -> Self {
        let state = match publication.status {
            HistoryStatus::Idle => HistoryLoadState::Idle,
            HistoryStatus::Loading => HistoryLoadState::Loading,
            HistoryStatus::Loaded => HistoryLoadState::Loaded,
            HistoryStatus::Empty => HistoryLoadState::Empty,
            HistoryStatus::End => HistoryLoadState::End,
            HistoryStatus::Unauthorized => HistoryLoadState::Unauthorized,
            HistoryStatus::Error(error) => HistoryLoadState::Error(error),
        };
        let rows: Vec<HistoryRow> = publication
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| HistoryRow::from_entry(index, entry))
            .collect();
        let can_load_older = matches!(state, HistoryLoadState::Loaded | HistoryLoadState::Loading)
            && publication.has_more
            && publication.next_cursor.is_some()
            && !publication.at_bound;
        let load_older_label: Arc<str> = if can_load_older {
            Arc::from("さらに古い履歴を読み込む")
        } else if publication.at_bound {
            Arc::from("表示上限（200件）に達しました。これより古い履歴は未読み込みです")
        } else if matches!(state, HistoryLoadState::Loaded | HistoryLoadState::Loading)
            && publication.has_more
        {
            // A further page without a cursor is a contract violation the client
            // already refuses; here it is stated rather than hidden.
            Arc::from("続きの読み込み位置を取得できません")
        } else if matches!(state, HistoryLoadState::Idle) {
            Arc::from("履歴はまだ読み込まれていません")
        } else {
            Arc::from("これより古い履歴はありません")
        };
        Self {
            task: publication
                .task_ref
                .as_ref()
                .map(|task| Arc::from(task.as_str())),
            state,
            status_label: state.label(!rows.is_empty()),
            rows,
            can_load_older,
            load_older_label,
            at_bound: publication.at_bound,
        }
    }

    /// The bounded load-older request for this view, when the action is offered.
    pub fn older_request(&self, publication: &HistoryPublication) -> Option<HistoryRequest> {
        let task = publication.task_ref.clone()?;
        self.can_load_older
            .then(|| HistoryRequest::older(publication, task))
            .flatten()
    }
}

/// The local view one history row reopens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReopenTarget {
    /// Task the row belongs to.
    pub task: TaskId,
    /// Iteration index the row recorded.
    pub iteration: u64,
}

/// What one reopen changed in the window-local presentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReopenOutcome {
    /// Whether the local task scope changed.
    pub task_selected: bool,
    /// Whether a candidate of the target iteration became the viewed candidate.
    pub viewed_restored: bool,
    /// Whether the target iteration has no observed candidate.
    pub iteration_unavailable: bool,
    /// Whether the existing compare pair was kept (both sides still observed).
    pub compare_restored: bool,
    /// Whether a compare side is no longer observed and stays unavailable (D-4/D-5).
    pub compare_unavailable: bool,
}

impl ReopenOutcome {
    /// Fixed notice for the surface.
    pub fn label(&self) -> &'static str {
        if self.iteration_unavailable {
            "この時点の候補は現在の観測にありません。表示のみ切り替えました"
        } else if self.viewed_restored && self.compare_unavailable {
            "この時点を開きました。比較の一方は現在の観測にありません"
        } else if self.viewed_restored {
            "この時点のローカル表示を復元しました"
        } else {
            "表示できる候補がありません"
        }
    }
}

/// Reopen one history row as a **local view** only.
///
/// This is presentation state and nothing else: the task scope, the viewed
/// candidate and the existing compare pair are applied through
/// [`Presentation`], which cannot name a command, a projection or a canonical
/// selection. A target iteration with no observed candidate restores nothing
/// synthetic; an existing compare pair is kept (D-4/D-5) and reported as
/// unavailable when a side is no longer observed.
pub fn reopen_local_view(
    presentation: &mut Presentation,
    target: &ReopenTarget,
    candidates: &[CandidateRef],
) -> ReopenOutcome {
    let mut outcome = ReopenOutcome {
        task_selected: presentation.reset_for_task(target.task.clone()),
        ..ReopenOutcome::default()
    };
    let belongs = |candidate: &CandidateRef| candidate.task_id == target.task;
    let matches_iteration =
        |candidate: &CandidateRef| u64::from(candidate.iteration) == target.iteration;
    // Restore the same viewed candidate when it still belongs to this iteration
    // and is observed; otherwise the first observed candidate of that iteration.
    let viewed = presentation.viewed().map(|viewed| viewed.0.clone());
    let restored = viewed
        .filter(|candidate| {
            belongs(candidate) && matches_iteration(candidate) && candidates.contains(candidate)
        })
        .or_else(|| {
            candidates
                .iter()
                .find(|candidate| belongs(candidate) && matches_iteration(candidate))
                .cloned()
        });
    match restored {
        Some(candidate) => {
            presentation.view(ViewedCandidate(candidate));
            outcome.viewed_restored = true;
        }
        None => outcome.iteration_unavailable = true,
    }
    if let Some(pair) = presentation.compare_pair() {
        let available = pair
            .iter()
            .all(|side| belongs(&side.0) && candidates.contains(&side.0));
        if available {
            outcome.compare_restored = true;
        } else {
            // The local reference is kept and rendered unavailable (D-4/D-5); it
            // is never replaced with another candidate by a reopen.
            outcome.compare_unavailable = true;
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::{
        HistoryLoadState, HistoryView, ReopenOutcome, ReopenTarget, UNKNOWN, reopen_local_view,
    };
    use crate::presentation::{CompareSide, Presentation, ViewedCandidate};
    use app_core::HistoryCursor;
    use app_core::{HistoryEntry, HistoryError, HistoryPublication, HistoryStatus};
    use desktop_protocol::{ArtifactId, CandidateId, CandidateRef, RunId, TaskId};
    use std::sync::Arc;

    fn task(label: &str) -> TaskId {
        TaskId::new(format!("task/{label}")).expect("task")
    }

    fn candidate(label: &str, iteration: u32) -> CandidateRef {
        CandidateRef {
            task_id: task("Task-184"),
            run_id: RunId::new(format!("run/{label}")).expect("run"),
            iteration,
            candidate_id: CandidateId::new(format!("candidate/{label}")).expect("candidate"),
            artifact_ref: ArtifactId::new(format!("artifact/{label}")).expect("artifact"),
        }
    }

    fn entry(iteration: u64, plan: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            record_ref: format!("record/{iteration}").into(),
            run_id: format!("run/{iteration}").into(),
            iteration,
            phase: "REFINE".into(),
            plan_id: plan.map(Arc::from),
            created_at: "2026-11-08T00:00:00Z".into(),
        }
    }

    fn publication(
        status: HistoryStatus,
        entries: Vec<HistoryEntry>,
        next: Option<&str>,
        at_bound: bool,
    ) -> HistoryPublication {
        HistoryPublication {
            task_ref: Some(task("Task-184")),
            status,
            entries: Arc::from(entries),
            has_more: next.is_some(),
            next_cursor: next.map(|cursor| HistoryCursor::new(cursor).expect("cursor")),
            at_bound,
            sequence: 1,
        }
    }

    #[test]
    fn an_unloaded_page_is_not_rendered_as_an_empty_history() {
        let view = HistoryView::from_publication(&HistoryPublication::default());
        assert_eq!(view.state, HistoryLoadState::Idle);
        assert!(view.rows.is_empty());
        assert_eq!(
            view.status_label.as_ref(),
            "履歴はまだ読み込まれていません",
            "an unloaded page has its own label"
        );
        assert_ne!(
            view.status_label.as_ref(),
            "このタスクには実行履歴がありません"
        );
        assert!(!view.can_load_older);
    }

    #[test]
    fn an_authoritative_empty_history_has_its_own_label() {
        let view = HistoryView::from_publication(&publication(
            HistoryStatus::Empty,
            Vec::new(),
            None,
            false,
        ));
        assert_eq!(view.state, HistoryLoadState::Empty);
        assert_eq!(
            view.status_label.as_ref(),
            "このタスクには実行履歴がありません"
        );
    }

    #[test]
    fn rows_carry_only_run_level_fields_and_explicit_unknown_values() {
        let view = HistoryView::from_publication(&publication(
            HistoryStatus::End,
            vec![entry(3, None)],
            None,
            false,
        ));
        let row = &view.rows[0];
        assert_eq!(row.iteration_label.as_ref(), "3");
        assert_eq!(row.plan_label.as_ref(), UNKNOWN);
        assert_eq!(row.phase_label.as_ref(), "REFINE");
        assert_eq!(row.run_label.as_ref(), "run/3");
        assert_eq!(row.record_ref.as_ref(), "record/3");
        assert!(row.reopen_label.contains("反復 3"));
        assert!(row.accessible_label.contains("計画 不明"));
        assert_eq!(row.reopen_iteration, Some(3));
    }

    #[test]
    fn a_continuation_offers_load_older_only_with_a_server_cursor() {
        let view = HistoryView::from_publication(&publication(
            HistoryStatus::Loaded,
            vec![entry(3, Some("plan/3"))],
            Some("cursor-1"),
            false,
        ));
        assert!(view.can_load_older);
        assert_eq!(view.load_older_label.as_ref(), "さらに古い履歴を読み込む");
        let served = publication(
            HistoryStatus::Loaded,
            vec![entry(3, None)],
            Some("cursor-1"),
            false,
        );
        let request = view.older_request(&served).expect("a bounded continuation");
        assert_eq!(request.query.cursor().map(|c| c.as_str()), Some("cursor-1"));

        let ended = HistoryView::from_publication(&publication(
            HistoryStatus::End,
            vec![entry(3, None)],
            None,
            false,
        ));
        assert!(!ended.can_load_older);
        assert_eq!(
            ended.load_older_label.as_ref(),
            "これより古い履歴はありません"
        );
    }

    #[test]
    fn the_entry_ceiling_states_the_bound_instead_of_an_end() {
        let view = HistoryView::from_publication(&publication(
            HistoryStatus::Loaded,
            vec![entry(3, None)],
            Some("cursor-1"),
            true,
        ));
        assert!(view.at_bound);
        assert!(!view.can_load_older);
        assert!(view.load_older_label.contains("表示上限（200件）"));
        assert!(view.load_older_label.contains("未読み込み"));
    }

    #[test]
    fn error_and_unauthorized_states_have_distinct_explicit_labels() {
        let unauthorized = HistoryView::from_publication(&publication(
            HistoryStatus::Unauthorized,
            Vec::new(),
            None,
            false,
        ));
        assert_eq!(
            unauthorized.status_label.as_ref(),
            "履歴を表示する許可がありません"
        );
        let conflict = HistoryView::from_publication(&publication(
            HistoryStatus::Error(HistoryError::Conflict),
            Vec::new(),
            None,
            false,
        ));
        assert_eq!(
            conflict.status_label.as_ref(),
            "観測を開き直す必要があります"
        );
        let malformed = HistoryView::from_publication(&publication(
            HistoryStatus::Error(HistoryError::Malformed),
            Vec::new(),
            None,
            false,
        ));
        assert_eq!(
            malformed.status_label.as_ref(),
            "履歴の内容を読み取れません"
        );
    }

    #[test]
    fn reopen_restores_the_observed_candidate_of_the_iteration() {
        let mut presentation = Presentation::default();
        let candidates = vec![candidate("a", 3), candidate("b", 4)];
        let outcome = reopen_local_view(
            &mut presentation,
            &ReopenTarget {
                task: task("Task-184"),
                iteration: 3,
            },
            &candidates,
        );
        assert!(outcome.task_selected);
        assert!(outcome.viewed_restored);
        assert!(!outcome.iteration_unavailable);
        assert_eq!(presentation.viewed().expect("viewed").0, candidate("a", 3));
        assert_eq!(outcome.label(), "この時点のローカル表示を復元しました");
    }

    #[test]
    fn reopen_does_not_invent_a_candidate_for_an_unobserved_iteration() {
        let mut presentation = Presentation::default();
        let candidates = vec![candidate("b", 4)];
        let outcome = reopen_local_view(
            &mut presentation,
            &ReopenTarget {
                task: task("Task-184"),
                iteration: 9,
            },
            &candidates,
        );
        assert!(!outcome.viewed_restored);
        assert!(outcome.iteration_unavailable);
        assert!(presentation.viewed().is_none(), "nothing is synthesized");
        assert!(outcome.label().contains("現在の観測にありません"));
    }

    #[test]
    fn reopen_keeps_a_compare_pair_only_while_both_sides_are_observed() {
        let mut presentation = Presentation::default();
        let a = candidate("a", 3);
        let b = candidate("b", 3);
        presentation.adopt_task(task("Task-184"));
        assert!(presentation.compare([ViewedCandidate(a.clone()), ViewedCandidate(b.clone())]));
        assert_eq!(
            presentation
                .compare_side(CompareSide::A)
                .map(|side| &side.0),
            Some(&a)
        );

        let outcome = reopen_local_view(
            &mut presentation,
            &ReopenTarget {
                task: task("Task-184"),
                iteration: 3,
            },
            &[a.clone(), b.clone()],
        );
        assert!(outcome.compare_restored);
        assert!(!outcome.compare_unavailable);
        assert_eq!(presentation.compare_pair().expect("pair").len(), 2);
        assert!(!outcome.task_selected, "the same task is not re-selected");

        // One side disappears from the current observation: the local reference
        // is kept (D-4/D-5) and reported unavailable, never replaced.
        let outcome = reopen_local_view(
            &mut presentation,
            &ReopenTarget {
                task: task("Task-184"),
                iteration: 3,
            },
            &[a],
        );
        assert!(outcome.compare_unavailable);
        assert_eq!(presentation.compare_pair().expect("pair").len(), 2);
        assert!(outcome.label().contains("比較の一方"));
    }

    #[test]
    fn reopen_switches_the_local_task_scope_and_clears_cross_task_references() {
        let mut presentation = Presentation::default();
        let old = candidate("old", 1);
        presentation.adopt_task(task("Task-Else"));
        presentation.view(ViewedCandidate(old));
        let outcome = reopen_local_view(
            &mut presentation,
            &ReopenTarget {
                task: task("Task-184"),
                iteration: 2,
            },
            &[candidate("new", 2)],
        );
        assert!(outcome.task_selected);
        assert_eq!(
            presentation.task_scope(),
            Some(&task("Task-184")),
            "the local task scope follows the reopened row"
        );
        assert_eq!(
            presentation.viewed().expect("viewed").0,
            candidate("new", 2)
        );
    }

    #[test]
    fn a_reopen_outcome_reports_only_local_presentation_changes() {
        // The outcome type has no canonical field at all: status, selection or
        // outcome cannot be expressed, so a reopen cannot mutate them.
        let outcome = ReopenOutcome {
            task_selected: true,
            viewed_restored: true,
            iteration_unavailable: false,
            compare_restored: true,
            compare_unavailable: false,
        };
        assert_eq!(outcome.label(), "この時点のローカル表示を復元しました");
    }

    /// §69 regression: every History state the user sees keeps an understandable
    /// Japanese label. A rename or a new surface that leaves only an English or
    /// acronym label fails here instead of shipping silently.
    #[test]
    fn every_user_facing_history_state_stays_japanese() {
        fn has_japanese(text: &str) -> bool {
            text.chars()
                .any(|character| matches!(character as u32, 0x3040..=0x30ff | 0x4e00..=0x9fff))
        }
        let states = [
            HistoryLoadState::Idle,
            HistoryLoadState::Loading,
            HistoryLoadState::Loaded,
            HistoryLoadState::Empty,
            HistoryLoadState::End,
            HistoryLoadState::Unauthorized,
            HistoryLoadState::Error(HistoryError::NotFound),
            HistoryLoadState::Error(HistoryError::Conflict),
            HistoryLoadState::Error(HistoryError::InvalidRequest),
            HistoryLoadState::Error(HistoryError::Malformed),
            HistoryLoadState::Error(HistoryError::Oversized),
            HistoryLoadState::Error(HistoryError::Unavailable),
            HistoryLoadState::Error(HistoryError::Unsupported),
        ];
        for state in states {
            for has_rows in [false, true] {
                let label = state.label(has_rows);
                assert!(
                    has_japanese(&label),
                    "history state {state:?} has no Japanese label: {label}"
                );
            }
        }
    }
}
