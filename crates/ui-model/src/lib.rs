//! Window-local interactions and display text. Read-only Runtime projections
//! arrive as complete publications from app-core; this crate never calls the client.

mod simple_casefold;
pub mod task_rail;
pub mod presentation {
    //! Window-local viewed/compare state (ADR-0010 D-1…D-9).
    //!
    //! This module owns only local presentation references. It never reads a
    //! canonical projection, never mutates one, and cannot issue a Runtime
    //! command: nothing here names app-core, a publication or a command payload.
    //! Task switching clears the references (D-9); an identity disappearing from
    //! the same task's projection keeps them and is rendered as unavailable
    //! (D-4/D-5).

    use desktop_protocol::{CandidateRef, TaskId};

    /// A local reference to a candidate, not an artifact identity or formal selection.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ViewedCandidate(pub CandidateRef);

    /// Which side of the local compare selection a change targets.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CompareSide {
        /// First compare side.
        A,
        /// Second compare side.
        B,
    }

    impl CompareSide {
        /// Index of this side inside the pair.
        pub fn index(self) -> usize {
            match self {
                Self::A => 0,
                Self::B => 1,
            }
        }

        /// Short label used by the UI and its accessible names.
        pub fn label(self) -> &'static str {
            match self {
                Self::A => "A",
                Self::B => "B",
            }
        }
    }

    /// What one explicit candidate selection did (ADR-0010 D-3).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SelectionEffect {
        /// Compare was active: only Viewed moved; the surface stayed on Compare.
        ViewedOnly,
        /// The first compare side (A) was filled; Compare is not active yet.
        CompareSideA,
        /// The pair completed; the surface must switch to Compare.
        CompareSideB,
        /// An explicit change-side action updated that side; Viewed did not move.
        Changed(CompareSide),
        /// The second plain selection repeated side A, so no pair was formed and
        /// Viewed moved to that same candidate (D-2/B1).
        DuplicateSelection,
        /// An explicit change-side action was refused because the candidate already
        /// occupies the other side; the pair is untouched (D-2/B1).
        DuplicateSide,
    }

    /// Which persisted reference the current projection accepted (ADR-0010 D-8).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct RestoreOutcome {
        /// The persisted viewed reference was present and applied.
        pub viewed_restored: bool,
        /// The persisted viewed reference was absent and was not applied.
        pub viewed_missing: bool,
        /// The persisted pair was complete and present, and was applied.
        pub pair_restored: bool,
        /// The persisted pair was absent or incomplete, so it was not applied.
        pub pair_missing: bool,
    }

    /// Window-local presentation state; publications never mutate it.
    #[derive(Debug, Default)]
    pub struct Presentation {
        viewed: Option<ViewedCandidate>,
        compare: Option<[ViewedCandidate; 2]>,
        /// Side A is filled but the pair is not complete yet (D-3).
        pending_side_a: Option<ViewedCandidate>,
        /// An explicit change-side action is armed for the next selection (D-3).
        changing_side: Option<CompareSide>,
        /// The task these references belong to; a task switch clears them (D-9).
        task_scope: Option<TaskId>,
        pub diagnostics_open: bool,
    }

    impl Presentation {
        /// Set Viewed directly (restore and non-dock callers). Dock selection must
        /// use [`Presentation::select`] so the pair transitions stay coherent.
        pub fn view(&mut self, candidate: ViewedCandidate) {
            self.viewed = Some(candidate);
        }

        /// The local viewed reference, present or not.
        pub fn viewed(&self) -> Option<&ViewedCandidate> {
            self.viewed.as_ref()
        }

        /// A complete compare pair, present or not.
        pub fn compare_pair(&self) -> Option<&[ViewedCandidate; 2]> {
            self.compare.as_ref()
        }

        /// One compare side, including a pending side A before the pair completes.
        pub fn compare_side(&self, side: CompareSide) -> Option<&ViewedCandidate> {
            match side {
                CompareSide::A => self
                    .compare
                    .as_ref()
                    .map(|pair| &pair[0])
                    .or(self.pending_side_a.as_ref()),
                CompareSide::B => self.compare.as_ref().map(|pair| &pair[1]),
            }
        }

        /// Set a complete pair (restore and non-dock callers).
        ///
        /// Returns false and applies nothing when both sides hold the same identity:
        /// a pair always names two different candidates (D-2/B1).
        pub fn compare(&mut self, pair: [ViewedCandidate; 2]) -> bool {
            if pair[0] == pair[1] {
                return false;
            }
            self.compare = Some(pair);
            self.pending_side_a = None;
            self.changing_side = None;
            true
        }

        /// One explicit dock selection (D-3).
        ///
        /// While Compare is inactive the first selection fills side A and the second
        /// completes side B; while Compare is active a plain selection moves Viewed
        /// only. An armed change-side action updates that side and leaves Viewed.
        pub fn select(&mut self, candidate: ViewedCandidate) -> SelectionEffect {
            if let Some(side) = self.changing_side.take() {
                // A change-side action must not produce an identical-identity pair.
                let other_identity = self.compare.as_ref().map(|pair| &pair[1 - side.index()].0);
                if other_identity.is_some_and(|other| *other == candidate.0) {
                    return SelectionEffect::DuplicateSide;
                }
                match side {
                    CompareSide::A => {
                        if let Some(pair) = self.compare.as_mut() {
                            pair[0] = candidate;
                            return SelectionEffect::Changed(CompareSide::A);
                        }
                        if self.pending_side_a.is_some() {
                            self.pending_side_a = Some(candidate);
                            return SelectionEffect::Changed(CompareSide::A);
                        }
                    }
                    CompareSide::B => {
                        if let Some(pair) = self.compare.as_mut() {
                            pair[1] = candidate;
                            return SelectionEffect::Changed(CompareSide::B);
                        }
                    }
                }
            }
            self.viewed = Some(candidate.clone());
            if self.compare.is_some() {
                return SelectionEffect::ViewedOnly;
            }
            if let Some(side_a) = &self.pending_side_a {
                if side_a.0 == candidate.0 {
                    // The same candidate cannot fill both sides: keep side A and the
                    // single-candidate state instead of forming `[A, A]` (D-2/B1).
                    return SelectionEffect::DuplicateSelection;
                }
                let side_a = self
                    .pending_side_a
                    .take()
                    .expect("side A exists while completing the pair");
                self.compare = Some([side_a, candidate]);
                return SelectionEffect::CompareSideB;
            }
            self.pending_side_a = Some(candidate);
            SelectionEffect::CompareSideA
        }

        /// Arm an explicit change-side action for the next selection (D-3/D-5).
        ///
        /// Returns false when that side does not exist, so a side is never created or
        /// replaced by a plain selection.
        pub fn arm_change(&mut self, side: CompareSide) -> bool {
            let available = match side {
                CompareSide::A => self.compare.is_some() || self.pending_side_a.is_some(),
                CompareSide::B => self.compare.is_some(),
            };
            if available {
                self.changing_side = Some(side);
            }
            available
        }

        /// The side an explicit change action is currently armed for.
        pub fn changing_side(&self) -> Option<CompareSide> {
            self.changing_side
        }

        /// Close Compare: keep Viewed, drop the pair and any pending side (D-7).
        pub fn close_compare(&mut self) -> bool {
            let had_pair = self.compare.take().is_some();
            let had_pending = self.pending_side_a.take().is_some();
            self.changing_side = None;
            had_pair || had_pending
        }

        /// Bind the references to a task without clearing them (startup restore).
        pub fn adopt_task(&mut self, task: TaskId) {
            if self.task_scope.is_none() {
                self.task_scope = Some(task);
            }
        }

        /// A task switch discards this task's references and starts at empty (D-9).
        ///
        /// Returns true when the task actually changed; a projection update inside the
        /// same task returns false and leaves the references untouched.
        pub fn reset_for_task(&mut self, task: TaskId) -> bool {
            if self.task_scope.as_ref() == Some(&task) {
                return false;
            }
            self.viewed = None;
            self.compare = None;
            self.pending_side_a = None;
            self.changing_side = None;
            self.task_scope = Some(task);
            true
        }

        /// The task these references belong to, when bound.
        pub fn task_scope(&self) -> Option<&TaskId> {
            self.task_scope.as_ref()
        }

        /// Apply persisted references with the REC3 identity rule (D-8).
        ///
        /// Viewed is applied only when present. The pair is all-or-nothing: both sides
        /// must be present, otherwise the pair is not applied and nothing is substituted.
        pub fn restore(
            &mut self,
            viewed: Option<&CandidateRef>,
            pair: Option<&[CandidateRef; 2]>,
            present: impl Fn(&CandidateRef) -> bool,
        ) -> RestoreOutcome {
            let mut outcome = RestoreOutcome::default();
            if let Some(reference) = viewed {
                if present(reference) {
                    self.viewed = Some(ViewedCandidate(reference.clone()));
                    outcome.viewed_restored = true;
                } else {
                    outcome.viewed_missing = true;
                }
            }
            if let Some(references) = pair {
                // An identical-identity pair is refused as unavailable, never repaired
                // into a different pair (D-2/B1).
                if references[0] == references[1] {
                    outcome.pair_missing = true;
                } else if present(&references[0]) && present(&references[1]) {
                    self.compare = Some([
                        ViewedCandidate(references[0].clone()),
                        ViewedCandidate(references[1].clone()),
                    ]);
                    self.pending_side_a = None;
                    self.changing_side = None;
                    outcome.pair_restored = true;
                } else {
                    outcome.pair_missing = true;
                }
            }
            outcome
        }

        /// Whether the held Viewed reference is present (D-4); `None` when empty.
        pub fn viewed_available(&self, present: impl Fn(&CandidateRef) -> bool) -> Option<bool> {
            self.viewed.as_ref().map(|viewed| present(&viewed.0))
        }

        /// Whether the held side is present (D-5); `None` when the side is empty.
        pub fn compare_side_available(
            &self,
            side: CompareSide,
            present: impl Fn(&CandidateRef) -> bool,
        ) -> Option<bool> {
            self.compare_side(side)
                .map(|candidate| present(&candidate.0))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{CompareSide, Presentation, SelectionEffect, ViewedCandidate};
        use desktop_protocol::{ArtifactId, CandidateId, CandidateRef, RunId, TaskId};

        fn candidate(label: &str) -> ViewedCandidate {
            ViewedCandidate(CandidateRef {
                task_id: TaskId::new("fixture:task/Task-184").expect("valid task identity"),
                run_id: RunId::new("fixture:run/Task-184/1").expect("valid run identity"),
                iteration: 3,
                candidate_id: CandidateId::new(format!("fixture:candidate/{label}"))
                    .expect("valid candidate"),
                artifact_ref: ArtifactId::new(format!("fixture:artifact/{label}"))
                    .expect("valid artifact"),
            })
        }

        fn other_candidate(label: &str) -> CandidateRef {
            candidate(label).0
        }

        fn present_in(existing: &[CandidateRef]) -> impl Fn(&CandidateRef) -> bool + '_ {
            move |reference| existing.contains(reference)
        }

        #[test]
        fn viewing_and_comparing_stay_independent() {
            let mut state = Presentation::default();
            let pair = [candidate("a"), candidate("b")];
            state.compare(pair.clone());
            state.view(candidate("c"));
            assert_eq!(state.compare_pair(), Some(&pair));
            assert_eq!(state.viewed(), Some(&candidate("c")));
        }

        #[test]
        fn with_no_persisted_reference_the_view_is_explicitly_empty() {
            let state = Presentation::default();
            assert_eq!(state.viewed(), None);
            assert_eq!(state.compare_pair(), None);
            assert_eq!(state.compare_side(CompareSide::A), None);
            assert_eq!(state.compare_side(CompareSide::B), None);
        }

        #[test]
        fn the_first_selection_fills_side_a_and_the_second_completes_compare() {
            let mut state = Presentation::default();
            assert_eq!(state.select(candidate("a")), SelectionEffect::CompareSideA);
            assert_eq!(state.viewed(), Some(&candidate("a")));
            assert_eq!(state.compare_pair(), None);
            assert_eq!(state.compare_side(CompareSide::A), Some(&candidate("a")));
            assert_eq!(state.select(candidate("b")), SelectionEffect::CompareSideB);
            assert_eq!(state.viewed(), Some(&candidate("b")));
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("a"), candidate("b")])
            );
        }

        #[test]
        fn while_compare_is_active_a_plain_selection_moves_viewed_only() {
            let mut state = Presentation::default();
            state.select(candidate("a"));
            state.select(candidate("b"));
            assert_eq!(state.select(candidate("c")), SelectionEffect::ViewedOnly);
            assert_eq!(state.viewed(), Some(&candidate("c")));
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("a"), candidate("b")]),
                "a plain selection must never replace a compare side"
            );
        }

        #[test]
        fn change_side_updates_that_side_without_moving_viewed() {
            let mut state = Presentation::default();
            state.select(candidate("a"));
            state.select(candidate("b"));
            state.select(candidate("c"));
            assert!(state.arm_change(CompareSide::A));
            assert_eq!(state.changing_side(), Some(CompareSide::A));
            assert_eq!(
                state.select(candidate("d")),
                SelectionEffect::Changed(CompareSide::A)
            );
            assert_eq!(state.viewed(), Some(&candidate("c")));
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("d"), candidate("b")])
            );
            assert!(state.arm_change(CompareSide::B));
            assert_eq!(
                state.select(candidate("e")),
                SelectionEffect::Changed(CompareSide::B)
            );
            assert_eq!(state.viewed(), Some(&candidate("c")));
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("d"), candidate("e")])
            );
        }

        #[test]
        fn selecting_the_same_candidate_twice_never_forms_a_pair() {
            let mut state = Presentation::default();
            assert_eq!(state.select(candidate("a")), SelectionEffect::CompareSideA);
            assert_eq!(
                state.select(candidate("a")),
                SelectionEffect::DuplicateSelection,
                "one candidate cannot fill both sides"
            );
            assert_eq!(state.viewed(), Some(&candidate("a")));
            assert_eq!(state.compare_pair(), None);
            assert_eq!(state.compare_side(CompareSide::A), Some(&candidate("a")));
            // A different second candidate still completes the pair normally.
            assert_eq!(state.select(candidate("b")), SelectionEffect::CompareSideB);
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("a"), candidate("b")])
            );
        }

        #[test]
        fn changing_a_side_to_the_other_side_identity_is_refused() {
            let mut state = Presentation::default();
            state.select(candidate("a"));
            state.select(candidate("b"));
            assert!(state.arm_change(CompareSide::A));
            assert_eq!(state.select(candidate("b")), SelectionEffect::DuplicateSide);
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("a"), candidate("b")]),
                "the refused change must leave the pair untouched"
            );
            assert_eq!(state.viewed(), Some(&candidate("b")));
            assert_eq!(state.changing_side(), None);
        }

        #[test]
        fn restore_refuses_an_identical_identity_pair() {
            let mut state = Presentation::default();
            let a = other_candidate("a");
            let duplicate = [a.clone(), a.clone()];
            let visible = [a];
            let present = present_in(&visible);
            let outcome = state.restore(None, Some(&duplicate), present);
            assert!(outcome.pair_missing);
            assert_eq!(state.compare_pair(), None);
            assert!(!state.compare([candidate("a"), candidate("a")]));
            assert_eq!(state.compare_pair(), None);
        }

        #[test]
        fn a_side_that_does_not_exist_cannot_be_armed() {
            let mut state = Presentation::default();
            assert!(!state.arm_change(CompareSide::B));
            state.select(candidate("a"));
            assert!(!state.arm_change(CompareSide::B));
            assert!(state.arm_change(CompareSide::A));
        }

        #[test]
        fn a_disappearing_viewed_identity_keeps_its_reference() {
            let mut state = Presentation::default();
            state.select(candidate("a"));
            assert_eq!(state.viewed_available(present_in(&[])), Some(false));
            assert_eq!(state.viewed(), Some(&candidate("a")));
            let later = [other_candidate("a")];
            assert_eq!(state.viewed_available(present_in(&later)), Some(true));
        }

        #[test]
        fn a_disappearing_compare_side_keeps_compare_and_its_reference() {
            let mut state = Presentation::default();
            state.select(candidate("a"));
            state.select(candidate("b"));
            assert_eq!(
                state.compare_side_available(CompareSide::B, present_in(&[other_candidate("a")])),
                Some(false)
            );
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("a"), candidate("b")])
            );
            let both = [other_candidate("a"), other_candidate("b")];
            assert_eq!(
                state.compare_side_available(CompareSide::B, present_in(&both)),
                Some(true)
            );
        }

        #[test]
        fn closing_compare_keeps_viewed_and_restarts_pair_creation() {
            let mut state = Presentation::default();
            state.select(candidate("a"));
            state.select(candidate("b"));
            assert!(state.close_compare());
            assert_eq!(state.compare_pair(), None);
            assert_eq!(state.viewed(), Some(&candidate("b")));
            assert_eq!(state.select(candidate("c")), SelectionEffect::CompareSideA);
            assert_eq!(state.compare_side(CompareSide::A), Some(&candidate("c")));
        }

        #[test]
        fn restore_applies_only_references_present_in_the_projection() {
            let mut state = Presentation::default();
            let viewed = other_candidate("v");
            let pair = [other_candidate("a"), other_candidate("b")];
            let visible = [other_candidate("v")];
            let present = present_in(&visible);
            let outcome = state.restore(Some(&viewed), Some(&pair), present);
            assert!(outcome.viewed_restored);
            assert!(outcome.pair_missing, "the pair is all-or-nothing");
            assert_eq!(state.viewed(), Some(&candidate("v")));
            assert_eq!(state.compare_pair(), None);
        }

        #[test]
        fn restore_applies_a_complete_pair_only_when_both_sides_are_present() {
            let mut state = Presentation::default();
            let viewed = other_candidate("v");
            let pair = [other_candidate("a"), other_candidate("b")];
            let all = [
                other_candidate("v"),
                other_candidate("a"),
                other_candidate("b"),
            ];
            let outcome = state.restore(Some(&viewed), Some(&pair), present_in(&all));
            assert!(outcome.viewed_restored);
            assert!(outcome.pair_restored);
            assert_eq!(state.viewed(), Some(&candidate("v")));
            assert_eq!(
                state.compare_pair(),
                Some(&[candidate("a"), candidate("b")])
            );
        }

        #[test]
        fn a_task_switch_clears_references_but_a_projection_update_does_not() {
            let mut state = Presentation::default();
            let task_a = TaskId::new("fixture:task/A").expect("valid task");
            let task_b = TaskId::new("fixture:task/B").expect("valid task");
            state.adopt_task(task_a.clone());
            state.select(candidate("a"));
            state.select(candidate("b"));
            assert!(!state.reset_for_task(task_a.clone()));
            assert_eq!(state.viewed(), Some(&candidate("b")));
            assert!(state.compare_pair().is_some());
            assert!(state.reset_for_task(task_b));
            assert_eq!(state.viewed(), None);
            assert_eq!(state.compare_pair(), None);
            assert_eq!(state.compare_side(CompareSide::A), None);
            assert_eq!(
                state.task_scope(),
                Some(&TaskId::new("fixture:task/B").expect("valid task"))
            );
        }
    }
}

use std::collections::BTreeMap;
use std::sync::Arc;

use app_core::{
    ObservationPublication, PublicationConnection, PublicationError, PublicationNotice,
    PublicationSource, SnapshotHandle,
};
use desktop_protocol::{
    CandidateRef, Counter, EpochId, EvaluationId, GraphId, GraphRef, IterationId, OutcomeId, TaskId,
};

/// One window's workbench: shared core ownership plus local presentation state.
pub struct Workbench {
    pub core: app_core::AppCore,
    pub presentation: presentation::Presentation,
}

/// Derivation identity that scopes reusable display records.
///
/// A generation never shares cached projection data with another observation
/// owner, and an epoch change invalidates every entity revision in the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationScope {
    pub session_generation: u64,
}

/// Immutable candidate presentation data derived from one authoritative row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateDisplay {
    pub identity: CandidateRef,
    pub source_revision: Counter,
    pub label: Arc<str>,
    /// Collision-free, length-delimited full candidate identity for UI element IDs.
    pub element_key: Arc<str>,
}

/// Immutable evaluation presentation data derived from one authoritative row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationDisplay {
    pub identity: EvaluationId,
    pub source_revision: Counter,
    pub lines: Arc<[Arc<str>]>,
}

/// Immutable outcome presentation data derived from one authoritative row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutcomeDisplay {
    pub identity: OutcomeId,
    pub source_revision: Counter,
    pub text: Arc<str>,
}

/// Immutable node presentation data derived from one authoritative graph row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNodeDisplay {
    /// Authoritative node identity.
    pub identity: desktop_protocol::NodeId,
    /// Authoritative semantic label; never derived from layout position.
    pub label: Arc<str>,
    /// Authoritative layer label, when provided.
    pub layer: Option<Arc<str>>,
}

/// Immutable edge presentation data derived from one authoritative graph row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphEdgeDisplay {
    /// Authoritative edge identity.
    pub identity: desktop_protocol::EdgeId,
    /// Authoritative source node identity.
    pub from: desktop_protocol::NodeId,
    /// Authoritative target node identity.
    pub to: desktop_protocol::NodeId,
    /// Source node's authoritative label from the same revision.
    pub from_label: Arc<str>,
    /// Target node's authoritative label from the same revision.
    pub to_label: Arc<str>,
    /// Authoritative edge kind label, when provided.
    pub kind: Option<Arc<str>>,
}

/// Immutable graph presentation data derived from one authoritative row.
///
/// `graph_ref` is the canonical revision identity (`graph_id` + `version` +
/// `digest`). `source_revision` is the independent projection revision used to
/// invalidate display rows; the two are never substituted for one another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphDisplay {
    pub graph_ref: GraphRef,
    pub source_revision: Counter,
    pub nodes: Arc<[Arc<GraphNodeDisplay>]>,
    pub edges: Arc<[Arc<GraphEdgeDisplay>]>,
}

/// Authoritative iteration group over the dock's candidate rows.
///
/// Grouping and order come from the runtime's iteration records: never from a
/// local sort, a score, or a locally inferred "current" iteration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateGroupDisplay {
    /// Authoritative iteration identity, when the runtime projects one.
    pub iteration: Option<IterationId>,
    /// Runtime iteration index of the authoritative record.
    pub index: Option<u32>,
    /// Full group label (`Iteration 3`), used where space allows.
    pub label: Arc<str>,
    /// Compact group label for the fixed-width dock gutter (`It 3`).
    pub short_label: Arc<str>,
    /// Index of this group's first row in the flattened dock row order.
    pub start: usize,
    /// Rows of this group, in dock row order.
    pub rows: Arc<[Arc<CandidateDisplay>]>,
}

/// Typed display families derived from one complete snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionDisplay {
    /// Owner-local marker for the complete snapshot that produced these records.
    pub source_revision: Counter,
    pub scope: DerivationScope,
    pub candidates: Arc<[Arc<CandidateDisplay>]>,
    /// Iteration groups covering every candidate row exactly once, in
    /// authoritative iteration order.
    pub candidate_groups: Arc<[CandidateGroupDisplay]>,
    /// Candidate identities the authoritative outcome projection formally selects.
    /// Display-only: never a local selection and never a command (ADR-0010 D-13).
    pub formal_candidates: Arc<[CandidateRef]>,
    pub evaluations: Arc<[Arc<EvaluationDisplay>]>,
    pub outcomes: Arc<[Arc<OutcomeDisplay>]>,
    pub graphs: Arc<[Arc<GraphDisplay>]>,
}

struct DisplayEntry<T> {
    revision: Counter,
    display: Arc<T>,
}

struct TaskDisplay {
    identity: TaskId,
    title: Arc<str>,
}
/// Per-consumer incremental projection derivation cache.
///
/// It is intended to be moved to a background executor with the publication.
/// The cache contains only rows present in its most recently derived snapshot.
#[derive(Default)]
pub struct DerivationCache {
    scope: Option<DerivationScope>,
    epoch: Option<EpochId>,
    /// Owner-local marker for the complete cached snapshot, not an entity revision.
    snapshot_revision: Option<Counter>,
    task: Option<DisplayEntry<TaskDisplay>>,
    axes: Option<(Counter, TaskId, StateAxes)>,
    candidates: BTreeMap<CandidateRef, DisplayEntry<CandidateDisplay>>,
    candidate_groups: Arc<[CandidateGroupDisplay]>,
    /// Authoritative iteration revisions the current groups were derived from.
    iteration_revisions: Vec<(IterationId, Counter)>,
    evaluations: BTreeMap<EvaluationId, DisplayEntry<EvaluationDisplay>>,
    outcomes: BTreeMap<OutcomeId, DisplayEntry<OutcomeDisplay>>,
    graphs: BTreeMap<GraphId, DisplayEntry<GraphDisplay>>,
    candidate_displays: Arc<[Arc<CandidateDisplay>]>,
    evaluation_displays: Arc<[Arc<EvaluationDisplay>]>,
    outcome_displays: Arc<[Arc<OutcomeDisplay>]>,
    graph_displays: Arc<[Arc<GraphDisplay>]>,
}

impl DerivationCache {
    /// Drop every reusable display record, including the generation/epoch scope.
    pub fn reset(&mut self) {
        self.scope = None;
        self.epoch = None;
        self.snapshot_revision = None;
        self.task = None;
        self.axes = None;
        self.candidates.clear();
        self.candidate_groups = Arc::from([]);
        self.iteration_revisions.clear();
        self.evaluations.clear();
        self.outcomes.clear();
        self.graphs.clear();
        self.candidate_displays = Arc::from([]);
        self.evaluation_displays = Arc::from([]);
        self.outcome_displays = Arc::from([]);
        self.graph_displays = Arc::from([]);
    }

    /// Derive all window display data from one complete authorized publication.
    ///
    /// `None` keeps the initial-task behavior; `Some` is authoritative and never
    /// falls back if that selected identity is no longer in the publication.
    /// Cancellation is checked before work and during projection traversal. A
    /// canceled or withheld derivation discards the cache before returning.
    pub fn derive(
        &mut self,
        publication: &ObservationPublication,
        selected_task: Option<&TaskId>,
        cancelled: impl Fn() -> bool,
    ) -> Option<WorkbenchText> {
        if cancelled() {
            self.reset();
            return None;
        }

        let mut text = WorkbenchText::from_publication(publication);
        if publication.source == PublicationSource::Unavailable {
            self.reset();
            text.title = "No Task observation".into();
            return Some(text);
        }
        if let Some(snapshot) = publication.snapshot.as_ref() {
            let Some(revision) = publication.revision.as_ref() else {
                self.reset();
                return None;
            };
            let scope = DerivationScope {
                session_generation: publication.session_generation,
            };
            let projection = self.derive_snapshot(snapshot, revision, scope, &cancelled)?;
            text.epoch = snapshot.epoch.clone();
            text.projection_revision = Some(revision.as_str().to_owned());
            text.artifacts = Arc::clone(&projection.candidates);
            text.candidate_groups = Arc::clone(&projection.candidate_groups);
            text.formal_candidates = Arc::clone(&projection.formal_candidates);
            text.evaluation = Arc::clone(&projection.evaluations);
            text.outcomes = Arc::clone(&projection.outcomes);
            text.graph = Arc::clone(&projection.graphs);

            if cancelled() {
                self.reset();
                return None;
            }
            let task = match selected_task {
                Some(identity) => snapshot.tasks.get(identity),
                None => snapshot.tasks.values().next(),
            };
            if let Some(task) = task {
                let matches_cached = self.task.as_ref().is_some_and(|cached| {
                    cached.display.identity == task.task_ref
                        && cached.revision == task.projection_revision
                });
                if !matches_cached {
                    self.task = None;
                }
                let cached = self.task.get_or_insert_with(|| DisplayEntry {
                    revision: task.projection_revision.clone(),
                    display: Arc::new(TaskDisplay {
                        identity: task.task_ref.clone(),
                        title: format!("{}  {}", task.display_id, task.title).into(),
                    }),
                });
                let display = &cached.display;
                text.task = Some(task.task_ref.clone());
                text.title = Arc::clone(&display.title);
                if self
                    .axes
                    .as_ref()
                    .is_some_and(|(cached_revision, cached_task, _)| {
                        cached_revision != revision || cached_task != &task.task_ref
                    })
                {
                    self.axes = None;
                }
                let (_, _, axes) = self.axes.get_or_insert_with(|| {
                    (
                        revision.clone(),
                        task.task_ref.clone(),
                        state_axes(task, snapshot, &text.connection),
                    )
                });
                text.axes = axes.clone();
                text.axes.connection = text.connection.as_str().into();
                (text.stop_enabled, text.stop_reason) = stop_availability(publication, task);
            } else {
                self.task = None;
                self.axes = None;
                text.title = "No Task observation".into();
            }
        } else {
            self.reset();
            text.title = "No Task observation".into();
        }

        for (id, view) in &publication.command_statuses {
            if cancelled() {
                self.reset();
                return None;
            }
            text.command_views
                .insert(id.clone(), StopDisplay::from_command(view));
        }
        if cancelled() {
            self.reset();
            return None;
        }
        Some(text)
    }

    /// Derive typed display records from a complete snapshot.
    ///
    /// Entity rows are reused only when their family, full identity and
    /// authoritative entity projection revision are unchanged. The returned
    /// `source_revision` tags the complete snapshot with its owner marker.
    pub fn derive_snapshot(
        &mut self,
        snapshot: &SnapshotHandle,
        revision: &Counter,
        scope: DerivationScope,
        cancelled: impl Fn() -> bool,
    ) -> Option<ProjectionDisplay> {
        if cancelled() {
            self.reset();
            return None;
        }
        if self.scope.as_ref() != Some(&scope) || self.epoch.as_ref() != snapshot.epoch.as_ref() {
            self.reset();
            self.scope = Some(scope.clone());
            self.epoch = snapshot.epoch.clone();
        }
        if self.snapshot_revision.as_ref() == Some(revision) {
            return Some(ProjectionDisplay {
                source_revision: revision.clone(),
                scope,
                candidates: Arc::clone(&self.candidate_displays),
                candidate_groups: Arc::clone(&self.candidate_groups),
                evaluations: Arc::clone(&self.evaluation_displays),
                outcomes: Arc::clone(&self.outcome_displays),
                graphs: Arc::clone(&self.graph_displays),
                formal_candidates: formal_candidate_refs(snapshot).into(),
            });
        }

        let mut canceled_during_prune = false;
        let mut candidate_displays_changed = false;
        self.candidates.retain(|identity, _| {
            if cancelled() {
                canceled_during_prune = true;
                true
            } else {
                let present = snapshot.candidates.contains_key(identity);
                candidate_displays_changed |= !present;
                present
            }
        });
        let mut evaluation_displays_changed = false;
        self.evaluations.retain(|identity, _| {
            if cancelled() {
                canceled_during_prune = true;
                true
            } else {
                let present = snapshot.evaluations.contains_key(identity);
                evaluation_displays_changed |= !present;
                present
            }
        });
        let mut outcome_displays_changed = false;
        self.outcomes.retain(|identity, _| {
            if cancelled() {
                canceled_during_prune = true;
                true
            } else {
                let present = snapshot.outcomes.contains_key(identity);
                outcome_displays_changed |= !present;
                present
            }
        });
        let mut graph_displays_changed = false;
        self.graphs.retain(|identity, _| {
            if cancelled() {
                canceled_during_prune = true;
                true
            } else {
                let present = snapshot.graphs.contains_key(identity);
                graph_displays_changed |= !present;
                present
            }
        });
        if canceled_during_prune || cancelled() {
            self.reset();
            return None;
        }

        for (identity, candidate) in &snapshot.candidates {
            if cancelled() {
                self.reset();
                return None;
            }
            match self.candidates.get(identity) {
                Some(cached) if cached.revision == candidate.projection_revision => {}
                _ => {
                    let display = Arc::new(CandidateDisplay {
                        identity: candidate.candidate_ref.clone(),
                        source_revision: candidate.projection_revision.clone(),
                        label: Arc::from(candidate.display_label.as_str()),
                        element_key: candidate_element_key(&candidate.candidate_ref).into(),
                    });
                    self.candidates.insert(
                        identity.clone(),
                        DisplayEntry {
                            revision: candidate.projection_revision.clone(),
                            display,
                        },
                    );
                    candidate_displays_changed = true;
                }
            }
        }

        for (identity, evaluation) in &snapshot.evaluations {
            if cancelled() {
                self.reset();
                return None;
            }
            match self.evaluations.get(identity) {
                Some(cached) if cached.revision == evaluation.projection_revision => {}
                _ => {
                    let Some(lines) = evaluation_lines(evaluation, &cancelled) else {
                        self.reset();
                        return None;
                    };
                    let display = Arc::new(EvaluationDisplay {
                        identity: evaluation.evaluation_ref.clone(),
                        source_revision: evaluation.projection_revision.clone(),
                        lines,
                    });
                    self.evaluations.insert(
                        identity.clone(),
                        DisplayEntry {
                            revision: evaluation.projection_revision.clone(),
                            display,
                        },
                    );
                    evaluation_displays_changed = true;
                }
            }
        }

        for (identity, outcome) in &snapshot.outcomes {
            if cancelled() {
                self.reset();
                return None;
            }
            match self.outcomes.get(identity) {
                Some(cached) if cached.revision == outcome.projection_revision => {}
                _ => {
                    let display = Arc::new(OutcomeDisplay {
                        identity: outcome.outcome_ref.clone(),
                        source_revision: outcome.projection_revision.clone(),
                        text: format!(
                            "Formal Outcome (Runtime projection): {}",
                            outcome.formal_status.as_str()
                        )
                        .into(),
                    });
                    self.outcomes.insert(
                        identity.clone(),
                        DisplayEntry {
                            revision: outcome.projection_revision.clone(),
                            display,
                        },
                    );
                    outcome_displays_changed = true;
                }
            }
        }

        for (identity, graph) in &snapshot.graphs {
            if cancelled() {
                self.reset();
                return None;
            }
            match self.graphs.get(identity) {
                Some(cached)
                    if cached.revision == graph.projection_revision
                        && cached.display.graph_ref == graph.graph_ref => {}
                _ => {
                    let labels = graph
                        .nodes
                        .iter()
                        .map(|node| (node.node_ref.clone(), node.label.as_str()))
                        .collect::<BTreeMap<_, _>>();
                    let mut nodes = Vec::with_capacity(graph.nodes.len());
                    for node in &graph.nodes {
                        if cancelled() {
                            self.reset();
                            return None;
                        }
                        nodes.push(Arc::new(GraphNodeDisplay {
                            identity: node.node_ref.clone(),
                            label: node.label.as_str().into(),
                            layer: node.layer.as_ref().map(|layer| layer.as_str().into()),
                        }));
                    }
                    let mut edges = Vec::with_capacity(graph.edges.len());
                    for edge in &graph.edges {
                        if cancelled() {
                            self.reset();
                            return None;
                        }
                        edges.push(Arc::new(GraphEdgeDisplay {
                            identity: edge.edge_ref.clone(),
                            from: edge.from.clone(),
                            to: edge.to.clone(),
                            from_label: labels
                                .get(&edge.from)
                                .copied()
                                .unwrap_or_else(|| edge.from.as_str())
                                .into(),
                            to_label: labels
                                .get(&edge.to)
                                .copied()
                                .unwrap_or_else(|| edge.to.as_str())
                                .into(),
                            kind: edge.kind.as_ref().map(|kind| kind.as_str().into()),
                        }));
                    }
                    let display = Arc::new(GraphDisplay {
                        graph_ref: graph.graph_ref.clone(),
                        source_revision: graph.projection_revision.clone(),
                        nodes: nodes.into(),
                        edges: edges.into(),
                    });
                    self.graphs.insert(
                        identity.clone(),
                        DisplayEntry {
                            revision: graph.projection_revision.clone(),
                            display,
                        },
                    );
                    graph_displays_changed = true;
                }
            }
        }

        if cancelled() {
            self.reset();
            return None;
        }
        let mut iteration_revisions = Vec::with_capacity(snapshot.iterations.len());
        for iteration in snapshot.iterations.values() {
            if cancelled() {
                self.reset();
                return None;
            }
            iteration_revisions.push((
                iteration.iteration_ref.clone(),
                iteration.projection_revision.clone(),
            ));
        }
        if candidate_displays_changed || self.iteration_revisions != iteration_revisions {
            // A previous grouped display is not an ordering authority. Always
            // start from the same source-key order as a fresh derivation.
            let mut base = Vec::with_capacity(self.candidates.len());
            for entry in self.candidates.values() {
                if cancelled() {
                    self.reset();
                    return None;
                }
                base.push(Arc::clone(&entry.display));
            }
            let Some(groups) = candidate_groups(&base, snapshot, &cancelled) else {
                self.reset();
                return None;
            };
            let mut flattened = Vec::with_capacity(base.len());
            for group in &groups {
                for row in group.rows.iter() {
                    if cancelled() {
                        self.reset();
                        return None;
                    }
                    flattened.push(Arc::clone(row));
                }
            }
            self.candidate_groups = groups.into();
            self.candidate_displays = flattened.into();
            self.iteration_revisions = iteration_revisions;
        }
        if evaluation_displays_changed {
            self.evaluation_displays = self
                .evaluations
                .values()
                .map(|entry| Arc::clone(&entry.display))
                .collect::<Vec<_>>()
                .into();
        }
        if outcome_displays_changed {
            self.outcome_displays = self
                .outcomes
                .values()
                .map(|entry| Arc::clone(&entry.display))
                .collect::<Vec<_>>()
                .into();
        }
        if graph_displays_changed {
            self.graph_displays = self
                .graphs
                .values()
                .map(|entry| Arc::clone(&entry.display))
                .collect::<Vec<_>>()
                .into();
        }
        if cancelled() {
            self.reset();
            return None;
        }
        self.snapshot_revision = Some(revision.clone());
        Some(ProjectionDisplay {
            source_revision: revision.clone(),
            scope,
            candidates: Arc::clone(&self.candidate_displays),
            candidate_groups: Arc::clone(&self.candidate_groups),
            evaluations: Arc::clone(&self.evaluation_displays),
            outcomes: Arc::clone(&self.outcome_displays),
            graphs: Arc::clone(&self.graph_displays),
            formal_candidates: formal_candidate_refs(snapshot).into(),
        })
    }
}

/// Independent, task-scoped presentation axes. Labels never imply an absent
/// projection is a negative terminal result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateAxes {
    pub task_status: Arc<str>,
    pub inner_phase: Arc<str>,
    pub iteration: Arc<str>,
    pub artifact_availability: Arc<str>,
    pub connection: Arc<str>,
    pub formal_outcome: Arc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopState {
    Prepared,
    AwaitingResponse,
    Accepted,
    Rejected,
    Unknown,
    CanonicalReflected,
    Failed,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopDisplay {
    pub state: StopState,
    pub label: Arc<str>,
    pub detail: Arc<str>,
    pub manual_intervention: bool,
}

impl StopDisplay {
    pub fn from_command(view: &app_core::CommandReadView) -> Self {
        use app_core::CommandPresentation as P;
        use desktop_protocol::ReceiptState as R;
        let state = match view.presentation {
            P::Prepared => StopState::Prepared,
            P::AwaitingResponse => StopState::AwaitingResponse,
            P::ReceiptAvailable => match view.receipt.as_ref().map(|receipt| &receipt.state) {
                Some(R::Accepted) => StopState::Accepted,
                Some(R::Rejected) => StopState::Rejected,
                Some(R::CanonicalReflected) => StopState::CanonicalReflected,
                Some(R::Unknown(_)) | None => StopState::Unknown,
            },
            P::CanonicalEffectObserved => StopState::CanonicalReflected,
            P::OutcomeUnconfirmed => StopState::Unknown,
            P::Unsupported => StopState::Unsupported,
            P::Failed => StopState::Failed,
        };
        let (label, detail, manual_intervention) = match state {
            StopState::Prepared => ("Stop prepared", "Not sent", false),
            StopState::AwaitingResponse => ("Stop awaiting response", "Possibly executed", true),
            StopState::Accepted => ("Stop accepted", "Acceptance is not task completion", false),
            StopState::Rejected => (
                "Stop rejected",
                "Task status remains independently observed",
                false,
            ),
            StopState::Unknown => (
                "Stop outcome unknown",
                "Possibly executed; manual intervention may be required",
                true,
            ),
            StopState::CanonicalReflected => (
                "Stop effect reflected",
                "Task completion remains independently observed",
                false,
            ),
            StopState::Failed => ("Stop failed", "No successful effect is implied", false),
            StopState::Unsupported => (
                "Stop unsupported",
                "Runtime does not support this operation",
                false,
            ),
            StopState::Unavailable => (
                "Stop unavailable",
                "Authorized receipt state is unavailable",
                true,
            ),
        };
        Self {
            state,
            label: label.into(),
            detail: detail.into(),
            manual_intervention,
        }
    }

    pub fn admission_error(error: &app_core::CommandReadError) -> Self {
        match error {
            app_core::CommandReadError::CapacityExceeded
            | app_core::CommandReadError::Client(PublicationError::InvalidCommand(
                app_core::InvalidCommandReason::CapacityExceeded,
            )) => Self::make(
                StopState::Unavailable,
                "Stop admission capacity unavailable",
                "No new Stop intent was admitted",
                false,
            ),
            _ => Self::unavailable(),
        }
    }
    pub fn retention_error(error: &app_core::CommandReadError) -> Self {
        match error {
            app_core::CommandReadError::CapacityExceeded
            | app_core::CommandReadError::Client(PublicationError::InvalidCommand(
                app_core::InvalidCommandReason::CapacityExceeded
                | app_core::InvalidCommandReason::ReferenceLimit,
            )) => Self::make(
                StopState::Unknown,
                "Stop receipt identity could not be retained",
                "Stop was admitted; it may have executed. Do not resend; manual intervention may be required",
                true,
            ),
            _ => Self::unavailable(),
        }
    }
    pub fn unavailable() -> Self {
        Self::make(
            StopState::Unavailable,
            "Stop unavailable",
            "Authorized receipt state is unavailable; manual intervention may be required",
            true,
        )
    }
    fn make(
        state: StopState,
        label: &'static str,
        detail: &'static str,
        manual_intervention: bool,
    ) -> Self {
        Self {
            state,
            label: label.into(),
            detail: detail.into(),
            manual_intervention,
        }
    }
}

/// Display-only data prepared from one complete publication.
#[derive(Default)]
pub struct WorkbenchText {
    pub title: Arc<str>,
    pub axes: StateAxes,
    pub stop_enabled: bool,
    pub stop_reason: Arc<str>,
    pub command_views: BTreeMap<desktop_protocol::CommandId, StopDisplay>,
    pub connection: String,
    pub source: String,
    pub mode: String,
    pub artifacts: Arc<[Arc<CandidateDisplay>]>,
    /// Authoritative iteration groups covering the dock rows.
    pub candidate_groups: Arc<[CandidateGroupDisplay]>,
    /// Candidate identities the runtime formally selects; display-only (ADR-0010 D-13).
    pub formal_candidates: Arc<[CandidateRef]>,
    pub evaluation: Arc<[Arc<EvaluationDisplay>]>,
    pub outcomes: Arc<[Arc<OutcomeDisplay>]>,
    pub graph: Arc<[Arc<GraphDisplay>]>,
    pub diagnostics: String,
    pub task: Option<TaskId>,
    /// Authoritative projection epoch, when a snapshot is admitted.
    pub epoch: Option<EpochId>,
    pub projection_revision: Option<String>,
    pub session_generation: u64,
    pub publication_version: u64,
}

impl WorkbenchText {
    fn from_publication(publication: &ObservationPublication) -> Self {
        let unavailable = publication.source == PublicationSource::Unavailable;
        let unknown = if unavailable {
            "Unavailable — source unavailable"
        } else {
            "Unknown"
        };
        Self {
            connection: connection_label(publication),
            source: source_label(publication),
            mode: mode_label(publication),
            session_generation: publication.session_generation,
            publication_version: publication.publication_version,
            diagnostics: diagnostics(publication),
            axes: StateAxes {
                task_status: unknown.into(),
                inner_phase: unknown.into(),
                iteration: unknown.into(),
                artifact_availability: unknown.into(),
                connection: connection_label(publication).into(),
                formal_outcome: unknown.into(),
            },
            stop_reason: if unavailable {
                "Stop unavailable — source unavailable".into()
            } else {
                "Stop unavailable — no current task observation".into()
            },
            ..Default::default()
        }
    }

    pub fn withheld() -> Self {
        Self {
            title: "No Task observation".into(),
            connection: "Data withheld — no current authorization".into(),
            source: "No verified source".into(),
            mode: "NO SOURCE / data withheld".into(),
            axes: StateAxes {
                task_status: "Unavailable — authorization unavailable".into(),
                inner_phase: "Unavailable — authorization unavailable".into(),
                iteration: "Unavailable — authorization unavailable".into(),
                artifact_availability: "Unavailable — authorization unavailable".into(),
                connection: "Data withheld — no current authorization".into(),
                formal_outcome: "Unavailable — authorization unavailable".into(),
            },
            stop_reason: "Stop unavailable — no current authorization".into(),
            ..Self::default()
        }
    }
}

fn state_axes(
    task: &desktop_protocol::TaskSnapshot,
    snapshot: &SnapshotHandle,
    connection: &str,
) -> StateAxes {
    use std::collections::BTreeSet;
    let mut artifact_ids = BTreeSet::new();
    if let Some(id) = task.accepted_brief_ref.as_ref() {
        artifact_ids.insert(id);
    }
    for candidate in snapshot
        .candidates
        .values()
        .filter(|c| c.task_ref == task.task_ref)
    {
        artifact_ids.insert(&candidate.candidate_ref.artifact_ref);
        artifact_ids.extend(candidate.artifact_refs.iter());
    }
    let reference_count = artifact_ids.len();
    let mut projected = 0;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for id in artifact_ids {
        if let Some(artifact) = snapshot.artifacts.get(id) {
            *counts.entry(artifact.availability.as_str()).or_default() += 1;
            projected += 1;
        }
    }
    let artifact_availability = if reference_count == 0 {
        "Unknown — no scoped artifact references".to_owned()
    } else if projected == 0 {
        format!(
            "Unknown — no scoped artifact availability projection ({reference_count} reference(s))"
        )
    } else {
        let mut states = counts
            .iter()
            .map(|(state, count)| format!("{state} ({count})"))
            .collect::<Vec<_>>();
        if projected < reference_count {
            states.push(format!(
                "Availability not projected ({})",
                reference_count - projected
            ));
        }
        states.join(", ")
    };
    let formal_outcome = task
        .formal_outcome_ref
        .as_ref()
        .and_then(|id| snapshot.outcomes.get(id))
        .filter(|outcome| outcome.task_ref == task.task_ref)
        .map(|outcome| outcome.formal_status.as_str().to_owned())
        .unwrap_or_else(|| "Unknown — no formal outcome projection".to_owned());
    StateAxes {
        task_status: task.task_status.as_str().into(),
        inner_phase: task.inner_phase.as_str().into(),
        iteration: task
            .iteration_index
            .map_or_else(|| "Unknown".to_owned(), |n| n.to_string())
            .into(),
        artifact_availability: artifact_availability.into(),
        connection: connection.into(),
        formal_outcome: formal_outcome.into(),
    }
}

fn stop_availability(
    publication: &ObservationPublication,
    task: &desktop_protocol::TaskSnapshot,
) -> (bool, Arc<str>) {
    use app_core::{PublicationConnection as C, PublicationSource as S};
    if publication.source == S::Unavailable || publication.snapshot.is_none() {
        return (
            false,
            "Stop unavailable — source or authorization unavailable".into(),
        );
    }
    if !matches!(publication.connection, C::Observing) {
        return (
            false,
            "Stop unavailable — no active authorized session".into(),
        );
    }
    let Some(snapshot) = publication.snapshot.as_ref() else {
        return (
            false,
            "Stop unavailable — capability projection unavailable".into(),
        );
    };
    let Some(capabilities) = snapshot.capabilities.get(&task.capabilities_ref) else {
        return (
            false,
            "Stop unavailable — capability projection unavailable".into(),
        );
    };
    if !capabilities.availability.is_supported() {
        return (
            false,
            "Stop unavailable — command capability unavailable".into(),
        );
    }
    match capabilities.command_support(&desktop_protocol::CommandOperation::StopTask) {
        Some(desktop_protocol::SupportState::Supported) => {}
        Some(desktop_protocol::SupportState::Unsupported) => {
            return (false, "Stop unsupported".into());
        }
        Some(desktop_protocol::SupportState::Unavailable)
        | Some(desktop_protocol::SupportState::Unknown(_))
        | None => {
            return (
                false,
                "Stop unavailable — command capability unknown".into(),
            );
        }
    }
    match task.task_status {
        desktop_protocol::TaskStatus::Active | desktop_protocol::TaskStatus::Assigned => {
            (true, "Stop task".into())
        }
        desktop_protocol::TaskStatus::Unknown(_) => {
            (false, "Stop unavailable — task status unknown".into())
        }
        _ => (false, "Stop unavailable — task is not active".into()),
    }
}

/// Display data cached with the exact publication identity it was derived from.
///
/// A publication identity is `(session_generation, publication_version)`.
pub struct DisplayCache {
    text: WorkbenchText,
    identity: Option<(u64, u64)>,
}

impl Default for DisplayCache {
    fn default() -> Self {
        Self {
            text: WorkbenchText::withheld(),
            identity: None,
        }
    }
}

impl DisplayCache {
    /// Whether derived authorized data is currently displayable.
    pub fn is_authorized(&self, live: Option<PublicationNotice>) -> bool {
        match (live, self.identity) {
            (Some(notice), Some(identity)) => {
                identity == (notice.session_generation, notice.publication_version)
            }
            _ => false,
        }
    }

    /// Install text only when both its own tag and the live publication tag agree.
    pub fn install(
        &mut self,
        identity: (u64, u64),
        text: WorkbenchText,
        live: Option<PublicationNotice>,
    ) -> bool {
        let live_identity =
            live.map(|notice| (notice.session_generation, notice.publication_version));
        let text_identity = (text.session_generation, text.publication_version);
        if live_identity != Some(identity) || text_identity != identity {
            // A stale completion must not invalidate a newer still-current view.
            self.revalidate(live);
            return false;
        }
        self.text = text;
        self.identity = Some(identity);
        true
    }

    /// Drop cached data; the next render shows an honest placeholder.
    pub fn invalidate(&mut self) {
        if self.identity.take().is_some() {
            self.text = WorkbenchText::withheld();
        }
    }

    /// Revalidate against the live record; stale text is replaced by a placeholder.
    pub fn revalidate(&mut self, live: Option<PublicationNotice>) {
        if self.is_authorized(live) {
            return;
        }
        if self.identity.is_some() {
            self.invalidate();
        }
    }

    /// The text that passed the most recent [`Self::revalidate`] call.
    pub fn text(&self) -> &WorkbenchText {
        &self.text
    }
}

fn candidate_element_key(candidate: &CandidateRef) -> String {
    let mut key = String::from("candidate:");
    push_key_component(&mut key, candidate.task_id.as_str());
    push_key_component(&mut key, candidate.run_id.as_str());
    push_key_component(&mut key, &candidate.iteration.to_string());
    push_key_component(&mut key, candidate.candidate_id.as_str());
    push_key_component(&mut key, candidate.artifact_ref.as_str());
    key
}

/// Group by authoritative task and iteration references. Numeric iteration
/// indices order groups inside a task; they never establish membership.
///
/// Temporary indices bound explicit membership and omissions to O(N log N + M
/// log N), rather than scanning every candidate for every reference/iteration.
/// Candidate identities named by the runtime's own formal selection references.
///
/// A formal selection carries `(run_id, iteration, evaluation_id)`; the evaluation names
/// the candidate it evaluated. The join is exact — a missing evaluation or a mismatched
/// run/iteration contributes nothing, so the dock never guesses and never derives a
/// "winner" from ordering or scores. The result is display-only (ADR-0010 D-13).
fn formal_candidate_refs(snapshot: &SnapshotHandle) -> Vec<CandidateRef> {
    let mut selected = std::collections::BTreeSet::new();
    for outcome in snapshot.outcomes.values() {
        for selection in &outcome.selected_refs {
            let Some(evaluation) = snapshot.evaluations.get(&selection.evaluation_id) else {
                continue;
            };
            let candidate = &evaluation.candidate_ref;
            if candidate.run_id != selection.run_id || candidate.iteration != selection.iteration {
                continue;
            }
            selected.insert(candidate.clone());
        }
    }
    selected.into_iter().collect()
}

fn candidate_groups(
    candidates: &[Arc<CandidateDisplay>],
    snapshot: &SnapshotHandle,
    cancelled: &impl Fn() -> bool,
) -> Option<Vec<CandidateGroupDisplay>> {
    let mut identities = BTreeMap::new();
    let mut by_iteration: BTreeMap<_, Vec<usize>> = BTreeMap::new();
    for (index, row) in candidates.iter().enumerate() {
        if cancelled() {
            return None;
        }
        identities.insert(&row.identity, index);
        if let Some(candidate) = snapshot.candidates.get(&row.identity) {
            by_iteration
                .entry((&candidate.task_ref, &candidate.iteration_ref))
                .or_default()
                .push(index);
        }
    }
    let mut ordered = BTreeMap::new();
    for iteration in snapshot.iterations.values() {
        if cancelled() {
            return None;
        }
        ordered.insert(
            (
                &iteration.task_ref,
                iteration.index,
                &iteration.iteration_ref,
            ),
            iteration,
        );
    }
    let mut groups = Vec::new();
    let mut assigned = vec![false; candidates.len()];
    let mut start = 0usize;
    for iteration in ordered.into_values() {
        if cancelled() {
            return None;
        }
        let mut rows = Vec::new();
        for reference in &iteration.candidate_refs {
            if cancelled() {
                return None;
            }
            let Some(&index) = identities.get(reference) else {
                continue;
            };
            let Some(candidate) = snapshot.candidates.get(reference) else {
                continue;
            };
            if !assigned[index]
                && candidate.task_ref == iteration.task_ref
                && candidate.iteration_ref == iteration.iteration_ref
            {
                assigned[index] = true;
                rows.push(Arc::clone(&candidates[index]));
            }
        }
        if let Some(omitted) = by_iteration.get(&(&iteration.task_ref, &iteration.iteration_ref)) {
            for &index in omitted {
                if cancelled() {
                    return None;
                }
                if !assigned[index] {
                    assigned[index] = true;
                    rows.push(Arc::clone(&candidates[index]));
                }
            }
        }
        if !rows.is_empty() {
            let len = rows.len();
            groups.push(CandidateGroupDisplay {
                iteration: Some(iteration.iteration_ref.clone()),
                index: Some(iteration.index),
                label: format!("Iteration {}", iteration.index).into(),
                short_label: format!("It {}", iteration.index).into(),
                start,
                rows: rows.into(),
            });
            start += len;
        }
    }
    let mut unattributed = Vec::new();
    for (index, row) in candidates.iter().enumerate() {
        if cancelled() {
            return None;
        }
        if !assigned[index] {
            unattributed.push(Arc::clone(row));
        }
    }
    if !unattributed.is_empty() {
        groups.push(CandidateGroupDisplay {
            iteration: None,
            index: None,
            label: "Iteration unavailable".into(),
            short_label: "It ?".into(),
            start,
            rows: unattributed.into(),
        });
    }
    Some(groups)
}

fn push_key_component(key: &mut String, value: &str) {
    use std::fmt::Write as _;
    let _ = write!(key, "{}:", value.len());
    key.push_str(value);
}

fn evaluation_lines(
    evaluation: &desktop_protocol::EvaluationSnapshot,
    cancelled: &impl Fn() -> bool,
) -> Option<Arc<[Arc<str>]>> {
    let mut lines = Vec::new();
    lines.push(format!(
        "Decision: {} (observation data, not evaluator output)",
        evaluation.decision.as_str()
    ));
    for axis in &evaluation.axes {
        if cancelled() {
            return None;
        }
        lines.push(format!(
            "{}: {} {}",
            axis.name,
            axis.value
                .map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
            axis.unit.as_deref().unwrap_or("")
        ));
    }
    for evidence in &evaluation.evidence_refs {
        if cancelled() {
            return None;
        }
        if let Some(summary) = &evidence.summary {
            lines.push(format!("Evidence: {summary}"));
        }
        if let Some(detail) = &evidence.detail {
            lines.push(detail.clone());
        }
    }
    Some(
        lines
            .into_iter()
            .map(Arc::<str>::from)
            .collect::<Vec<_>>()
            .into(),
    )
}

fn connection_label(publication: &ObservationPublication) -> String {
    let base = match publication.connection {
        PublicationConnection::Disconnected => "Disconnected",
        PublicationConnection::Connecting => "Connecting",
        PublicationConnection::Observing => "Observing",
        PublicationConnection::Recovering => "Recovering",
        PublicationConnection::Unauthorized => "Unauthorized",
        PublicationConnection::Incompatible => "Incompatible",
    };
    if publication.stale {
        format!("{base} · stale")
    } else {
        base.to_owned()
    }
}

fn source_label(publication: &ObservationPublication) -> String {
    match publication.source {
        PublicationSource::Runtime => "Runtime".to_owned(),
        PublicationSource::Fixture => "Fixture transcript".to_owned(),
        PublicationSource::Unavailable => "No verified source".to_owned(),
    }
}

fn mode_label(publication: &ObservationPublication) -> String {
    match publication.source {
        PublicationSource::Fixture => "FIXTURE / simulated data".to_owned(),
        PublicationSource::Runtime => "RUNTIME / observing".to_owned(),
        PublicationSource::Unavailable => "NO SOURCE / data withheld".to_owned(),
    }
}

fn diagnostics(publication: &ObservationPublication) -> String {
    let error = publication
        .last_error
        .as_ref()
        .map_or_else(|| "none".to_owned(), error_label);
    format!(
        "Desktop {} · {} · generation {} · publication {} · error {} · commands {}",
        env!("CARGO_PKG_VERSION"),
        source_label(publication),
        publication.session_generation,
        publication.publication_version,
        error,
        publication.command_statuses.len()
    )
}

fn error_label(error: &PublicationError) -> String {
    match error {
        PublicationError::Transport => "transport".to_owned(),
        PublicationError::Protocol => "protocol".to_owned(),
        PublicationError::VersionIncompatible => "version".to_owned(),
        PublicationError::Unauthorized => "unauthorized".to_owned(),
        PublicationError::VerificationPending => "verification pending".to_owned(),
        PublicationError::QueueOverflow => "queue overflow".to_owned(),
        PublicationError::MessageTooLarge => "message too large".to_owned(),
        PublicationError::NotConnected => "not connected".to_owned(),
        PublicationError::InvalidCommand(_) => "command rejected".to_owned(),
        PublicationError::Server(code) => format!("server {code:?}"),
        PublicationError::Fixture => "fixture".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;

    use super::{DerivationCache, DisplayCache, WorkbenchText};
    use app_core::{
        ObservationPublication, PublicationConnection, PublicationNotice, PublicationSource,
        SnapshotHandle,
    };
    use desktop_protocol::{
        ArtifactDigest, ArtifactId, AuthorityLabel, CandidateId, CandidateRef, CandidateSnapshot,
        CapabilitiesId, Counter, EvaluationDecision, EvaluationId, EvaluationSnapshot,
        FormalStatus, GraphId, GraphNode, GraphRef, GraphSnapshot, IterationId, IterationSnapshot,
        OutcomeId, OutcomeSnapshot, PhaseLabel, RunId, TaskId, TaskSnapshot, TaskStatus,
    };

    fn candidate_ref(local: &str) -> CandidateRef {
        CandidateRef {
            task_id: TaskId::fixture("task/Task-184"),
            run_id: RunId::fixture("run/Task-184/1"),
            iteration: 3,
            candidate_id: CandidateId::fixture(format!("candidate/{local}")),
            artifact_ref: ArtifactId::fixture(format!("artifact/{local}")),
        }
    }

    fn candidate(local: &str, label: &str, revision: u128) -> CandidateSnapshot {
        let identity = candidate_ref(local);
        CandidateSnapshot {
            task_ref: identity.task_id.clone(),
            iteration_ref: IterationId::fixture("iteration/Task-184/3"),
            candidate_ref: identity,
            artifact_refs: Vec::new(),
            evaluation_refs: Vec::new(),
            display_label: label.to_owned(),
            projection_revision: Counter::from_u128(revision),
        }
    }

    /// Construct a source-owned snapshot through the public snapshot handle and
    /// its public projection maps; no Domain model or private cache hooks.
    fn fixture_snapshot(
        title: &str,
        task_status: TaskStatus,
        task_revision: u128,
        candidates: Vec<CandidateSnapshot>,
        epoch: &str,
    ) -> SnapshotHandle {
        let mut snapshot: SnapshotHandle = Arc::default();
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        state.epoch = Some(desktop_protocol::EpochId::fixture(epoch));

        let task_ref = TaskId::fixture("task/Task-184");
        state.tasks.insert(
            task_ref.clone(),
            TaskSnapshot {
                task_ref: task_ref.clone(),
                display_id: "Task-184".to_owned(),
                title: title.to_owned(),
                task_status,
                inner_phase: PhaseLabel::new("BUILD").expect("valid phase"),
                iteration_ref: Some(IterationId::fixture("iteration/Task-184/3")),
                iteration_index: Some(3),
                best_candidate_ref: Some(candidate_ref("e")),
                formal_outcome_ref: Some(OutcomeId::fixture("outcome/main")),
                accepted_brief_ref: None,
                capabilities_ref: CapabilitiesId::fixture("capabilities/task"),
                projection_revision: Counter::from_u128(task_revision),
            },
        );
        state.capabilities.insert(
            CapabilitiesId::fixture("capabilities/task"),
            desktop_protocol::CreativeCapabilities {
                capabilities_ref: CapabilitiesId::fixture("capabilities/task"),
                scope: desktop_protocol::CapabilityScope::Task {
                    task_id: TaskId::fixture("task/Task-184"),
                },
                commands: vec![desktop_protocol::CommandCapability {
                    operation: desktop_protocol::CommandOperation::StopTask,
                    support: desktop_protocol::SupportState::Supported,
                    reason: None,
                }],
                media: Vec::new(),
                features: Vec::new(),
                constraints: Vec::new(),
                availability: desktop_protocol::SupportState::Supported,
                reason: None,
                projection_revision: Counter::from_u128(1),
            },
        );
        for candidate in candidates {
            state
                .candidates
                .insert(candidate.candidate_ref.clone(), candidate);
        }

        let eval_ref = EvaluationId::fixture("evaluation/main");
        state.evaluations.insert(
            eval_ref.clone(),
            EvaluationSnapshot {
                evaluation_ref: eval_ref,
                candidate_ref: candidate_ref("e"),
                artifact_refs: Vec::new(),
                decision: EvaluationDecision::Pass,
                axes: Vec::new(),
                evidence_refs: Vec::new(),
                authority: AuthorityLabel::new("RUNTIME").expect("valid authority"),
                confidence: None,
                projection_revision: Counter::from_u128(1),
            },
        );
        let outcome_ref = OutcomeId::fixture("outcome/main");
        state.outcomes.insert(
            outcome_ref.clone(),
            OutcomeSnapshot {
                outcome_ref,
                task_ref,
                formal_status: FormalStatus::Completed,
                selected_refs: Vec::new(),
                stop_reason: None,
                provenance_refs: Vec::new(),
                evidence_refs: Vec::new(),
                projection_revision: Counter::from_u128(1),
            },
        );
        let graph_id = GraphId::fixture("graph/main");
        state.graphs.insert(
            graph_id.clone(),
            GraphSnapshot {
                graph_ref: GraphRef {
                    graph_id,
                    version: 1,
                    digest: ArtifactDigest::fixture("digest/graph-main"),
                },
                plan_refs: Vec::new(),
                nodes: vec![GraphNode {
                    node_ref: desktop_protocol::NodeId::fixture("node/main"),
                    label: "observed graph node".to_owned(),
                    layer: None,
                }],
                edges: Vec::new(),
                provenance_refs: Vec::new(),
                refinement_refs: Vec::new(),
                projection_revision: Counter::from_u128(1),
            },
        );
        snapshot
    }

    fn publication(
        snapshot: SnapshotHandle,
        session_generation: u64,
        publication_version: u64,
        revision: u128,
    ) -> ObservationPublication {
        ObservationPublication {
            snapshot: Some(snapshot),
            revision: Some(Counter::from_u128(revision)),
            connection: PublicationConnection::Observing,
            source: PublicationSource::Fixture,
            stale: false,
            session_generation,
            publication_version,
            ..ObservationPublication::default()
        }
    }

    fn derive(cache: &mut DerivationCache, publication: &ObservationPublication) -> WorkbenchText {
        cache
            .derive(publication, None, || false)
            .expect("non-canceled publication derives")
    }

    fn dock_candidate(local: &str, label: &str, iteration: u32) -> CandidateSnapshot {
        let identity = CandidateRef {
            task_id: TaskId::fixture("task/Task-184"),
            run_id: RunId::fixture("run/Task-184/1"),
            iteration,
            candidate_id: CandidateId::fixture(format!("candidate/{local}")),
            artifact_ref: ArtifactId::fixture(format!("artifact/{local}")),
        };
        CandidateSnapshot {
            task_ref: identity.task_id.clone(),
            iteration_ref: IterationId::fixture(format!("iteration/Task-184/{iteration}")),
            candidate_ref: identity,
            artifact_refs: Vec::new(),
            evaluation_refs: Vec::new(),
            display_label: label.to_owned(),
            projection_revision: Counter::from_u128(1),
        }
    }

    fn dock_iteration(index: u32, candidate_refs: Vec<CandidateRef>) -> IterationSnapshot {
        IterationSnapshot {
            iteration_ref: IterationId::fixture(format!("iteration/Task-184/{index}")),
            task_ref: TaskId::fixture("task/Task-184"),
            index,
            candidate_refs,
            refinement_refs: Vec::new(),
            evidence_refs: Vec::new(),
            projection_revision: Counter::from_u128(1),
        }
    }

    fn dock_snapshot(
        candidates: Vec<CandidateSnapshot>,
        iterations: Vec<IterationSnapshot>,
    ) -> SnapshotHandle {
        let mut snapshot: SnapshotHandle = Arc::default();
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        state.epoch = Some(desktop_protocol::EpochId::fixture("epoch/1"));
        for candidate in candidates {
            state
                .candidates
                .insert(candidate.candidate_ref.clone(), candidate);
        }
        for iteration in iterations {
            state
                .iterations
                .insert(iteration.iteration_ref.clone(), iteration);
        }
        snapshot
    }

    #[test]
    fn iteration_groups_follow_authoritative_records_and_never_drop_rows() {
        let iteration_two = dock_iteration(2, vec![dock_candidate("e", "E", 2).candidate_ref]);
        let iteration_one = dock_iteration(
            1,
            vec![
                dock_candidate("b", "B", 1).candidate_ref,
                dock_candidate("a", "A", 1).candidate_ref,
            ],
        );
        let snapshot = dock_snapshot(
            vec![
                dock_candidate("a", "A", 1),
                dock_candidate("b", "B", 1),
                dock_candidate("e", "E", 2),
                dock_candidate("z", "Z", 9),
            ],
            // Inserted out of index order on purpose: the projection order is
            // the runtime's, not a local sort of the map.
            vec![iteration_two, iteration_one],
        );
        let mut cache = DerivationCache::default();
        let display = cache
            .derive_snapshot(
                &snapshot,
                &Counter::from_u128(4),
                super::DerivationScope {
                    session_generation: 1,
                },
                || false,
            )
            .expect("non-canceled snapshot derives");

        let groups = display.candidate_groups.as_ref();
        assert_eq!(groups.len(), 3, "two recorded iterations plus unattributed");
        assert_eq!(groups[0].index, Some(1));
        assert_eq!(groups[0].label.as_ref(), "Iteration 1");
        assert_eq!(groups[0].start, 0);
        assert_eq!(
            groups[0]
                .rows
                .iter()
                .map(|row| row.label.as_ref())
                .collect::<Vec<_>>(),
            vec!["B", "A"],
            "recorded candidate order is preserved"
        );
        assert_eq!(groups[1].index, Some(2));
        assert_eq!(groups[1].start, 2);
        assert_eq!(groups[2].index, None);
        assert_eq!(groups[2].label.as_ref(), "Iteration unavailable");
        assert_eq!(groups[2].start, 3);

        let flattened = display
            .candidates
            .iter()
            .map(|row| row.label.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(
            flattened,
            vec!["B", "A", "E", "Z"],
            "dock rows follow the authoritative iteration order and keep every candidate"
        );
    }

    #[test]
    fn iteration_group_membership_is_used_even_when_a_record_omits_a_candidate() {
        let membership = dock_candidate("a", "A", 1).candidate_ref.clone();
        let snapshot = dock_snapshot(
            vec![dock_candidate("a", "A", 1), dock_candidate("b", "B", 1)],
            vec![dock_iteration(1, vec![membership])],
        );
        let mut cache = DerivationCache::default();
        let display = cache
            .derive_snapshot(
                &snapshot,
                &Counter::from_u128(1),
                super::DerivationScope {
                    session_generation: 1,
                },
                || false,
            )
            .expect("non-canceled snapshot derives");
        assert_eq!(display.candidate_groups.len(), 1);
        assert_eq!(
            display.candidate_groups[0]
                .rows
                .iter()
                .map(|row| row.label.as_ref())
                .collect::<Vec<_>>(),
            vec!["A", "B"],
            "a candidate the record does not list stays in its iteration group"
        );
    }

    #[test]
    fn formal_markers_come_only_from_the_runtime_selection_refs() {
        use desktop_protocol::{ArtifactDigest, FormalSelectionRef, NodeId};

        use super::formal_candidate_refs;

        let mut snapshot: SnapshotHandle = Arc::default();
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        let a = candidate_ref("a");
        let b = candidate_ref("b");
        let eval_a = EvaluationId::fixture("evaluation/a");
        let eval_b = EvaluationId::fixture("evaluation/b");
        for (identity, evaluation, revision) in [
            (a.clone(), eval_a.clone(), 1_u128),
            (b.clone(), eval_b.clone(), 1_u128),
        ] {
            state.candidates.insert(
                identity.clone(),
                candidate(
                    identity
                        .candidate_id
                        .as_str()
                        .rsplit('/')
                        .next()
                        .unwrap_or("x"),
                    identity.candidate_id.as_str(),
                    revision,
                ),
            );
            state.evaluations.insert(
                evaluation.clone(),
                EvaluationSnapshot {
                    evaluation_ref: evaluation,
                    candidate_ref: identity,
                    artifact_refs: Vec::new(),
                    decision: EvaluationDecision::Pass,
                    axes: Vec::new(),
                    evidence_refs: Vec::new(),
                    authority: AuthorityLabel::new("RUNTIME").expect("valid authority"),
                    confidence: None,
                    projection_revision: Counter::from_u128(1),
                },
            );
        }
        state.outcomes.insert(
            OutcomeId::fixture("outcome/main"),
            OutcomeSnapshot {
                outcome_ref: OutcomeId::fixture("outcome/main"),
                task_ref: TaskId::fixture("task/Task-184"),
                formal_status: FormalStatus::Completed,
                selected_refs: vec![
                    FormalSelectionRef {
                        artifact_digest: ArtifactDigest::fixture("digest/a"),
                        run_id: RunId::fixture("run/Task-184/1"),
                        evaluation_id: eval_a.clone(),
                        iteration: 3,
                    },
                    // Same evaluation but a different run: never matched.
                    FormalSelectionRef {
                        artifact_digest: ArtifactDigest::fixture("digest/b"),
                        run_id: RunId::fixture("run/Task-184/2"),
                        evaluation_id: eval_b.clone(),
                        iteration: 3,
                    },
                    // No such evaluation: never matched.
                    FormalSelectionRef {
                        artifact_digest: ArtifactDigest::fixture("digest/unknown"),
                        run_id: RunId::fixture("run/Task-184/1"),
                        evaluation_id: EvaluationId::fixture("evaluation/unknown"),
                        iteration: 3,
                    },
                ],
                stop_reason: None,
                provenance_refs: Vec::new(),
                evidence_refs: Vec::new(),
                projection_revision: Counter::from_u128(1),
            },
        );
        let _ = NodeId::fixture("unused");
        assert_eq!(formal_candidate_refs(&snapshot), vec![a]);
    }

    #[test]
    fn the_timeline_shows_only_iterations_with_rows_and_never_synthesises_a_node() {
        let snapshot = dock_snapshot(
            vec![dock_candidate("a", "A", 1), dock_candidate("c", "C", 3)],
            vec![
                dock_iteration(1, vec![dock_candidate("a", "A", 1).candidate_ref]),
                // Iteration 2 is authoritative but carries no candidate rows: D-10
                // says it must not appear, and nothing may be synthesised for it.
                dock_iteration(2, Vec::new()),
                dock_iteration(3, vec![dock_candidate("c", "C", 3).candidate_ref]),
            ],
        );
        let mut cache = DerivationCache::default();
        let text = derive(&mut cache, &publication(snapshot, 1, 1, 7));
        let labels = text
            .candidate_groups
            .iter()
            .map(|group| group.label.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["Iteration 1", "Iteration 3"]);
        assert!(
            text.candidate_groups
                .iter()
                .all(|group| !group.label.to_lowercase().contains("outcome")),
            "the formal outcome belongs to the inspector, never to the timeline"
        );
        assert_eq!(text.formal_candidates.len(), 0);
    }

    #[test]
    fn the_dock_local_state_has_no_command_or_projection_surface() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub mod presentation {")
            .expect("presentation module");
        let end = source
            .find("use std::collections::BTreeMap;")
            .expect("module end");
        let module = &source[start..end];
        for forbidden in [
            "app_core",
            "AppCore",
            "ObservationHandle",
            "CommandPayload",
            "dispatch",
            "submit",
        ] {
            assert!(
                !module.contains(forbidden),
                "presentation state must not reach a command/projection surface: {forbidden}"
            );
        }
    }

    #[test]
    fn the_protocol_offers_no_formal_selection_command() {
        use desktop_protocol::{CommandPayload, StopTaskCommand};

        fn command_name(payload: &CommandPayload) -> &'static str {
            match payload {
                CommandPayload::CreateTask(_) => "CreateTask",
                CommandPayload::StopTask(_) => "StopTask",
                CommandPayload::ReuseTask(_) => "ReuseTask",
            }
        }

        // The exhaustive match is the proof at this commit: a formal-selection
        // command cannot exist without a new protocol variant, and no dock path can
        // issue one. The dock only ever mutates local presentation state.
        let stop = CommandPayload::StopTask(StopTaskCommand {
            task_id: TaskId::fixture("task/Task-184"),
            reason: None,
        });
        assert_eq!(command_name(&stop), "StopTask");
    }

    #[test]
    fn dock_candidates_follow_authoritative_iteration_order() {
        let a = dock_candidate("a", "A", 1);
        let mut b = dock_candidate("b", "B", 1);
        b.task_ref = TaskId::fixture("task/Other");
        b.candidate_ref.task_id = b.task_ref.clone();
        b.iteration_ref = IterationId::fixture("iteration/Other/1");
        let mut other_iteration = dock_iteration(1, Vec::new());
        other_iteration.task_ref = b.task_ref.clone();
        other_iteration.iteration_ref = b.iteration_ref.clone();
        let mut missing = dock_candidate("missing", "Unattributed", 1);
        missing.iteration_ref = IterationId::fixture("iteration/absent");
        let snapshot = dock_snapshot(
            vec![a.clone(), b.clone(), missing.clone()],
            vec![dock_iteration(1, Vec::new()), other_iteration],
        );
        let display = derive(
            &mut DerivationCache::default(),
            &publication(snapshot, 1, 1, 1),
        );
        let group = |iteration: &IterationId| {
            display
                .candidate_groups
                .iter()
                .find(|group| group.iteration.as_ref() == Some(iteration))
                .expect("authoritative iteration group")
        };
        assert_eq!(group(&a.iteration_ref).rows[0].identity, a.candidate_ref);
        assert_eq!(group(&a.iteration_ref).rows.len(), 1);
        assert_eq!(group(&b.iteration_ref).rows[0].identity, b.candidate_ref);
        assert_eq!(group(&b.iteration_ref).rows.len(), 1);
        let unavailable = display
            .candidate_groups
            .iter()
            .find(|group| group.iteration.is_none())
            .expect("explicit unattributed group");
        assert_eq!(unavailable.rows[0].identity, missing.candidate_ref);
        assert_eq!(unavailable.rows.len(), 1);
    }

    #[test]
    fn iteration_only_reorder_reuses_rows_and_epoch_reset_discards_old_groups() {
        let a = dock_candidate("a", "A", 1);
        let b = dock_candidate("b", "B", 2);
        let first = dock_snapshot(
            vec![a, b],
            vec![dock_iteration(1, Vec::new()), dock_iteration(2, Vec::new())],
        );
        let mut cache = DerivationCache::default();
        let before = derive(&mut cache, &publication(Arc::clone(&first), 1, 1, 1));
        let mut changed = first.as_ref().clone();
        let iteration = changed
            .iterations
            .get_mut(&IterationId::fixture("iteration/Task-184/2"))
            .expect("second iteration");
        iteration.index = 0;
        iteration.projection_revision = Counter::from_u128(2);
        let after = derive(&mut cache, &publication(Arc::new(changed), 1, 2, 2));
        assert_eq!(after.artifacts[0].label.as_ref(), "B");
        assert!(Arc::ptr_eq(&before.artifacts[1], &after.artifacts[0]));
        let empty = dock_snapshot(
            Vec::new(),
            vec![dock_iteration(1, Vec::new()), dock_iteration(2, Vec::new())],
        );
        let mut empty = empty.as_ref().clone();
        empty.epoch = Some(desktop_protocol::EpochId::fixture("epoch/fresh"));
        let fresh = derive(&mut cache, &publication(Arc::new(empty), 1, 3, 3));
        assert!(fresh.artifacts.is_empty());
        assert!(fresh.candidate_groups.is_empty());
    }

    #[test]
    fn explicit_large_membership_preserves_record_order_and_can_be_cancelled() {
        let rows = (0..10_000)
            .map(|index| dock_candidate(&format!("{index:05}"), &format!("C{index:05}"), 1))
            .collect::<Vec<_>>();
        let membership = rows
            .iter()
            .rev()
            .map(|row| row.candidate_ref.clone())
            .collect();
        let snapshot = dock_snapshot(rows, vec![dock_iteration(1, membership)]);
        let published = publication(snapshot, 1, 1, 1);
        let text = derive(&mut DerivationCache::default(), &published);
        assert_eq!(
            text.artifacts.first().expect("first").label.as_ref(),
            "C09999"
        );
        assert_eq!(
            text.artifacts.last().expect("last").label.as_ref(),
            "C00000"
        );
        assert_eq!(text.artifacts.len(), 10_000);
        // A deterministic cancellation budget, not a wall-clock performance SLA.
        // It allows source-row traversal, then expires during membership work.
        let work = Cell::new(0usize);
        let cancelled = || {
            work.set(work.get() + 1);
            work.get() > 30_100
        };
        assert!(
            DerivationCache::default()
                .derive(&published, None, cancelled)
                .is_none()
        );
    }

    #[test]
    fn iteration_omission_matches_fresh_order_and_reset_never_leaks_groups() {
        let a = dock_candidate("a", "A", 1);
        let b = dock_candidate("b", "B", 1);
        let snapshot = dock_snapshot(
            vec![a.clone(), b.clone()],
            vec![dock_iteration(1, vec![b.candidate_ref, a.candidate_ref])],
        );
        let mut cache = DerivationCache::default();
        let before = derive(&mut cache, &publication(Arc::clone(&snapshot), 1, 1, 1));
        let mut changed = snapshot.as_ref().clone();
        let iteration = changed.iterations.values_mut().next().expect("iteration");
        iteration.candidate_refs.clear();
        iteration.projection_revision = Counter::from_u128(2);
        let after = derive(&mut cache, &publication(Arc::new(changed.clone()), 1, 2, 2));
        let labels = after
            .artifacts
            .iter()
            .map(|row| row.label.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["A", "B"]);
        let fresh = derive(
            &mut DerivationCache::default(),
            &publication(Arc::new(changed.clone()), 1, 2, 2),
        );
        assert_eq!(after.artifacts, fresh.artifacts);
        assert!(Arc::ptr_eq(&before.artifacts[1], &after.artifacts[0]));
        changed.candidates.clear();
        changed.epoch = Some(desktop_protocol::EpochId::fixture("epoch/no-old-groups"));
        let empty = derive(&mut cache, &publication(Arc::new(changed), 1, 3, 3));
        assert!(empty.artifacts.is_empty());
        assert!(empty.candidate_groups.is_empty());
    }

    #[test]
    fn display_cache_withholds_content_when_the_authorization_fence_closes() {
        let mut derivation = DerivationCache::default();
        let mut display = DisplayCache::default();
        let published = publication(
            fixture_snapshot(
                "Task title",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Candidate E", 1)],
                "epoch/1",
            ),
            1,
            4,
            9,
        );
        let notice = PublicationNotice {
            session_generation: 1,
            publication_version: 4,
        };
        let text = derive(&mut derivation, &published);
        assert!(display.install((1, 4), text, Some(notice)));
        assert!(display.is_authorized(Some(notice)));

        display.revalidate(None);
        assert!(!display.is_authorized(None));
        assert_eq!(display.text().mode, "NO SOURCE / data withheld");
        assert_eq!(
            display.text().axes.task_status.as_ref(),
            "Unavailable — authorization unavailable"
        );
        assert_eq!(
            display.text().axes.artifact_availability.as_ref(),
            "Unavailable — authorization unavailable"
        );
        assert!(display.text().artifacts.is_empty());
    }

    #[test]
    fn derivation_exposes_newest_authorized_candidate_content_and_publication_tag() {
        let mut cache = DerivationCache::default();
        let first = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Candidate E", 1)],
                "epoch/1",
            ),
            7,
            12,
            20,
        );
        let _older = derive(&mut cache, &first);

        let newest = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Candidate E newest", 2)],
                "epoch/1",
            ),
            7,
            13,
            21,
        );
        let text = derive(&mut cache, &newest);
        assert_eq!(text.artifacts[0].label.as_ref(), "Candidate E newest");
        assert_eq!(text.artifacts[0].source_revision.as_str(), "2");
        assert_eq!(text.projection_revision.as_deref(), Some("21"));
        assert_eq!(text.session_generation, 7);
        assert_eq!(text.publication_version, 13);
    }

    #[test]
    fn task_only_publication_preserves_other_visible_projection_families() {
        let mut cache = DerivationCache::default();
        let first = publication(
            fixture_snapshot(
                "Task before",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Candidate E", 3)],
                "epoch/1",
            ),
            2,
            5,
            20,
        );
        let before = derive(&mut cache, &first);

        let task_only = publication(
            fixture_snapshot(
                "Task after",
                TaskStatus::Completed,
                2,
                vec![candidate("e", "Candidate E", 3)],
                "epoch/1",
            ),
            2,
            6,
            21,
        );
        let after = derive(&mut cache, &task_only);

        assert_eq!(after.title.as_ref(), "Task-184  Task after");
        assert_eq!(after.artifacts.as_ref(), before.artifacts.as_ref());
        assert_eq!(after.evaluation.as_ref(), before.evaluation.as_ref());
        assert_eq!(after.outcomes.as_ref(), before.outcomes.as_ref());
        assert_eq!(after.graph.as_ref(), before.graph.as_ref());
        assert_eq!(
            after.graph[0].graph_ref.graph_id.as_str(),
            "fixture:graph/main"
        );
        assert_eq!(
            after.graph[0].nodes[0].label.as_ref(),
            "observed graph node"
        );
        assert_eq!(after.session_generation, 2);
        assert_eq!(after.publication_version, 6);
    }

    fn add_graph(snapshot: &mut SnapshotHandle, graph: GraphSnapshot) {
        Arc::get_mut(snapshot)
            .expect("snapshot has one owner")
            .graphs
            .insert(graph.graph_ref.graph_id.clone(), graph);
    }

    fn flow_graph(
        version: u64,
        digest: &str,
        projection_revision: u128,
        nodes: &[(&str, &str, Option<&str>)],
        edges: &[(&str, &str, &str, Option<&str>)],
    ) -> GraphSnapshot {
        let graph_id = GraphId::fixture("graph/flow");
        GraphSnapshot {
            graph_ref: GraphRef {
                graph_id,
                version,
                digest: ArtifactDigest::fixture(digest),
            },
            plan_refs: Vec::new(),
            nodes: nodes
                .iter()
                .map(|(identity, label, layer)| GraphNode {
                    node_ref: desktop_protocol::NodeId::fixture(*identity),
                    label: (*label).to_owned(),
                    layer: layer.map(|layer| {
                        desktop_protocol::LayerLabel::new(layer).expect("valid layer")
                    }),
                })
                .collect(),
            edges: edges
                .iter()
                .map(|(identity, from, to, kind)| desktop_protocol::GraphEdge {
                    edge_ref: desktop_protocol::EdgeId::fixture(*identity),
                    from: desktop_protocol::NodeId::fixture(*from),
                    to: desktop_protocol::NodeId::fixture(*to),
                    kind: kind.map(|kind| {
                        desktop_protocol::EdgeKindLabel::new(kind).expect("valid edge kind")
                    }),
                })
                .collect(),
            provenance_refs: Vec::new(),
            refinement_refs: Vec::new(),
            projection_revision: Counter::from_u128(projection_revision),
        }
    }

    #[test]
    fn graph_display_preserves_exact_revision_identity_nodes_and_edges() {
        let mut snapshot =
            fixture_snapshot("Task", TaskStatus::Active, 1, Vec::new(), "epoch/graph");
        let graph = flow_graph(
            7,
            "digest/flow-7",
            11,
            &[("node/z", "Zeta", Some("GOAL")), ("node/a", "Alpha", None)],
            &[("edge/z-a", "node/z", "node/a", Some("FLOW"))],
        );
        add_graph(&mut snapshot, graph.clone());
        let mut cache = DerivationCache::default();
        let text = derive(&mut cache, &publication(snapshot, 1, 1, 1));
        let display = text
            .graph
            .iter()
            .find(|graph| graph.graph_ref.graph_id.as_str() == "fixture:graph/flow")
            .expect("flow graph is displayed");
        assert_eq!(
            text.epoch.as_ref().map(desktop_protocol::EpochId::as_str),
            Some("fixture:epoch/graph")
        );
        assert_eq!(display.graph_ref, graph.graph_ref);
        assert_eq!(display.source_revision, Counter::from_u128(11));
        assert_eq!(
            display
                .nodes
                .iter()
                .map(|node| (
                    node.identity.as_str(),
                    node.label.as_ref(),
                    node.layer.as_deref()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("fixture:node/z", "Zeta", Some("GOAL")),
                ("fixture:node/a", "Alpha", None),
            ]
        );
        assert_eq!(display.edges.len(), 1);
        let edge = &display.edges[0];
        assert_eq!(edge.identity.as_str(), "fixture:edge/z-a");
        assert_eq!(edge.from.as_str(), "fixture:node/z");
        assert_eq!(edge.to.as_str(), "fixture:node/a");
        assert_eq!(edge.from_label.as_ref(), "Zeta");
        assert_eq!(edge.to_label.as_ref(), "Alpha");
        assert_eq!(edge.kind.as_deref(), Some("FLOW"));
    }

    #[test]
    fn changed_graph_revision_rederives_even_at_the_same_projection_revision() {
        let mut snapshot =
            fixture_snapshot("Task", TaskStatus::Active, 1, Vec::new(), "epoch/graph");
        add_graph(
            &mut snapshot,
            flow_graph(1, "digest/flow-1", 4, &[("node/a", "Alpha", None)], &[]),
        );
        let mut cache = DerivationCache::default();
        let first_publication = publication(snapshot.clone(), 1, 1, 1);
        let first = derive(&mut cache, &first_publication);
        drop(first_publication);
        let first_graph = first
            .graph
            .iter()
            .find(|graph| graph.graph_ref.graph_id.as_str() == "fixture:graph/flow")
            .expect("first revision");
        assert_eq!(first_graph.graph_ref.version, 1);
        assert_eq!(first_graph.nodes[0].label.as_ref(), "Alpha");

        add_graph(
            &mut snapshot,
            flow_graph(
                2,
                "digest/flow-2",
                4,
                &[("node/a", "Alpha revised", None)],
                &[],
            ),
        );
        let second = derive(&mut cache, &publication(snapshot, 1, 1, 2));
        let second_graph = second
            .graph
            .iter()
            .find(|graph| graph.graph_ref.graph_id.as_str() == "fixture:graph/flow")
            .expect("second revision");
        assert_eq!(second_graph.graph_ref.version, 2);
        assert_eq!(
            second_graph.graph_ref.digest.as_str(),
            "fixture:digest/flow-2"
        );
        assert_eq!(second_graph.source_revision, Counter::from_u128(4));
        assert_eq!(second_graph.nodes[0].label.as_ref(), "Alpha revised");
    }

    #[test]
    fn selected_task_drives_title_axes_and_stop_at_the_same_publication_revision() {
        let mut snapshot = fixture_snapshot(
            "Initial task",
            TaskStatus::Active,
            1,
            vec![candidate("e", "Candidate E", 1)],
            "epoch/1",
        );
        let selected_id = TaskId::fixture("task/Task-999");
        let capabilities_ref = CapabilitiesId::fixture("capabilities/second");
        let selected_task = TaskSnapshot {
            task_ref: selected_id.clone(),
            display_id: "Task-999".to_owned(),
            title: "Second selected".to_owned(),
            task_status: TaskStatus::Completed,
            inner_phase: PhaseLabel::new("REVIEW").expect("valid phase"),
            iteration_ref: None,
            iteration_index: None,
            best_candidate_ref: None,
            formal_outcome_ref: None,
            accepted_brief_ref: None,
            capabilities_ref: capabilities_ref.clone(),
            projection_revision: Counter::from_u128(1),
        };
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        state.capabilities.insert(
            capabilities_ref.clone(),
            desktop_protocol::CreativeCapabilities {
                capabilities_ref,
                scope: desktop_protocol::CapabilityScope::Task {
                    task_id: selected_id.clone(),
                },
                commands: vec![desktop_protocol::CommandCapability {
                    operation: desktop_protocol::CommandOperation::StopTask,
                    support: desktop_protocol::SupportState::Supported,
                    reason: None,
                }],
                media: Vec::new(),
                features: Vec::new(),
                constraints: Vec::new(),
                availability: desktop_protocol::SupportState::Supported,
                reason: None,
                projection_revision: Counter::from_u128(1),
            },
        );
        state.tasks.insert(selected_id.clone(), selected_task);
        let publication = publication(snapshot, 3, 7, 21);

        let mut cache = DerivationCache::default();
        let initial = cache
            .derive(&publication, None, || false)
            .expect("initial task derives");
        assert_eq!(initial.task, Some(TaskId::fixture("task/Task-184")));
        assert_eq!(initial.axes.task_status.as_ref(), "ACTIVE");
        assert!(initial.stop_enabled);

        let selected = cache
            .derive(&publication, Some(&selected_id), || false)
            .expect("selected task derives");
        assert_eq!(selected.task.as_ref(), Some(&selected_id));
        assert_eq!(selected.title.as_ref(), "Task-999  Second selected");
        assert_eq!(selected.axes.task_status.as_ref(), "COMPLETED");
        assert_eq!(selected.axes.inner_phase.as_ref(), "REVIEW");
        assert_eq!(selected.axes.iteration.as_ref(), "Unknown");
        assert_eq!(
            selected.axes.formal_outcome.as_ref(),
            "Unknown — no formal outcome projection"
        );
        assert!(!selected.stop_enabled);
        assert_eq!(
            selected.stop_reason.as_ref(),
            "Stop unavailable — task is not active"
        );

        let missing_id = TaskId::fixture("task/removed");
        let missing = cache
            .derive(&publication, Some(&missing_id), || false)
            .expect("missing selected task yields unavailable context");
        assert_eq!(missing.task, None);
        assert_eq!(missing.title.as_ref(), "No Task observation");
        assert!(!missing.stop_enabled);
        assert_eq!(
            missing.stop_reason.as_ref(),
            "Stop unavailable — no current task observation"
        );
    }

    #[test]
    fn insertion_and_removal_keep_candidate_element_identity() {
        let viewed = candidate_ref("e");
        let mut cache = DerivationCache::default();

        let initial = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Candidate E", 1)],
                "epoch/1",
            ),
            3,
            1,
            1,
        );
        let first = derive(&mut cache, &initial);
        let initial_key = first.artifacts[0].element_key.clone();

        let inserted = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![
                    candidate("d", "Candidate D", 1),
                    candidate("e", "Candidate E", 1),
                ],
                "epoch/1",
            ),
            3,
            2,
            2,
        );
        let second = derive(&mut cache, &inserted);
        let viewed_row = second
            .artifacts
            .iter()
            .find(|row| row.identity == viewed)
            .expect("viewed identity remains present after insertion");
        assert_eq!(second.artifacts.len(), 2);
        assert!(
            second
                .artifacts
                .iter()
                .any(|row| row.identity == candidate_ref("d"))
        );
        assert_eq!(viewed_row.element_key.as_ref(), initial_key.as_ref());

        let removed = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Candidate E", 1)],
                "epoch/1",
            ),
            3,
            3,
            3,
        );
        let third = derive(&mut cache, &removed);
        assert_eq!(third.artifacts.len(), 1);
        assert_eq!(third.artifacts[0].identity, viewed);
    }

    #[test]
    fn generation_change_rederives_same_entity_revisions_in_same_epoch() {
        let mut cache = DerivationCache::default();
        let first = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Prior generation", 1)],
                "epoch/1",
            ),
            1,
            1,
            1,
        );
        let _ = derive(&mut cache, &first);

        let next_generation = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Current generation", 1)],
                "epoch/1",
            ),
            2,
            1,
            1,
        );
        let current = derive(&mut cache, &next_generation);
        assert_eq!(current.artifacts[0].label.as_ref(), "Current generation");
        assert_eq!(current.session_generation, 2);
    }

    #[test]
    fn epoch_change_rederives_same_entity_revisions_in_same_generation() {
        let mut cache = DerivationCache::default();
        let first = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Prior epoch", 1)],
                "epoch/1",
            ),
            2,
            1,
            1,
        );
        let _ = derive(&mut cache, &first);

        let next_epoch = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Current epoch", 1)],
                "epoch/2",
            ),
            2,
            2,
            1,
        );
        let current = derive(&mut cache, &next_epoch);
        assert_eq!(current.artifacts[0].label.as_ref(), "Current epoch");
        assert_eq!(current.publication_version, 2);
    }

    #[test]
    fn canceled_derivation_returns_no_result_and_next_job_shows_current_content() {
        let mut cache = DerivationCache::default();
        let first = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Prior content", 1)],
                "epoch/1",
            ),
            2,
            1,
            1,
        );
        let _ = derive(&mut cache, &first);

        let current = publication(
            fixture_snapshot(
                "Task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Current content", 2)],
                "epoch/1",
            ),
            2,
            2,
            2,
        );
        let cancel_after_start = Cell::new(false);
        assert!(
            cache
                .derive(&current, None, || cancel_after_start.replace(true))
                .is_none(),
            "canceled work cannot return a partial result"
        );
        let fresh = derive(&mut cache, &current);
        assert_eq!(fresh.artifacts[0].label.as_ref(), "Current content");
        assert_eq!(fresh.publication_version, 2);
    }

    #[test]
    fn stale_or_mismatched_install_does_not_clobber_current_authorized_text() {
        let mut derivation = DerivationCache::default();
        let mut display = DisplayCache::default();
        let current = publication(
            fixture_snapshot(
                "Current task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Current candidate", 2)],
                "epoch/1",
            ),
            4,
            9,
            2,
        );
        let current_text = derive(&mut derivation, &current);
        let current_notice = PublicationNotice {
            session_generation: 4,
            publication_version: 9,
        };
        assert!(display.install((4, 9), current_text, Some(current_notice)));

        let stale_publication = publication(
            fixture_snapshot(
                "Stale task",
                TaskStatus::Active,
                1,
                vec![candidate("e", "Stale candidate", 1)],
                "epoch/1",
            ),
            4,
            8,
            1,
        );
        let stale_text = derive(&mut derivation, &stale_publication);
        assert!(!display.install((4, 8), stale_text, Some(current_notice)));
        assert!(display.is_authorized(Some(current_notice)));
        assert_eq!(display.text().title.as_ref(), "Task-184  Current task");
        assert_eq!(
            display.text().artifacts[0].label.as_ref(),
            "Current candidate"
        );

        let mut mismatched_text = derive(&mut derivation, &current);
        mismatched_text.publication_version = 10;
        assert!(!display.install((4, 9), mismatched_text, Some(current_notice)));
        assert!(display.is_authorized(Some(current_notice)));
        assert_eq!(
            display.text().artifacts[0].label.as_ref(),
            "Current candidate"
        );
    }
    #[test]
    fn task_axes_use_selected_formal_reference_and_projected_artifact_availability() {
        let mut snapshot =
            fixture_snapshot("Task", TaskStatus::Completed, 1, Vec::new(), "epoch/axes");
        let state = Arc::get_mut(&mut snapshot).expect("fixture snapshot is uniquely owned");
        let task = state.tasks.values_mut().next().expect("fixture task");
        task.iteration_index = None;
        task.formal_outcome_ref = None;
        task.accepted_brief_ref = Some(ArtifactId::fixture("artifact/brief"));
        state.artifacts.insert(
            ArtifactId::fixture("artifact/brief"),
            desktop_protocol::ArtifactSnapshot {
                artifact_ref: ArtifactId::fixture("artifact/brief"),
                artifact_digest: ArtifactDigest::fixture("digest/brief"),
                media_type: None,
                availability: desktop_protocol::Availability::Staged,
                descriptor_refs: Vec::new(),
                verification: None,
                projection_revision: Counter::from_u128(2),
            },
        );
        let publication = publication(snapshot, 3, 5, 9);
        let text = derive(&mut DerivationCache::default(), &publication);
        assert_eq!(text.axes.task_status.as_ref(), "COMPLETED");
        assert_eq!(text.axes.iteration.as_ref(), "Unknown");
        assert_eq!(text.axes.artifact_availability.as_ref(), "STAGED (1)");
        assert_eq!(
            text.axes.formal_outcome.as_ref(),
            "Unknown — no formal outcome projection"
        );
        assert!(!text.stop_enabled);
    }

    #[test]
    fn unavailable_source_withholds_even_a_supplied_snapshot() {
        let snapshot = fixture_snapshot(
            "Private task",
            TaskStatus::Active,
            1,
            Vec::new(),
            "epoch/unavailable",
        );
        let mut publication = publication(snapshot, 1, 1, 1);
        publication.source = PublicationSource::Unavailable;
        let text = derive(&mut DerivationCache::default(), &publication);
        assert!(text.task.is_none());
        assert!(text.artifacts.is_empty());
        assert_eq!(
            text.axes.task_status.as_ref(),
            "Unavailable — source unavailable"
        );
        assert_eq!(
            text.stop_reason.as_ref(),
            "Stop unavailable — source unavailable"
        );
    }
    #[test]
    fn actual_formal_outcome_is_selected_by_the_task_reference() {
        let snapshot = fixture_snapshot("Task", TaskStatus::Active, 1, Vec::new(), "epoch/formal");
        let text = derive(
            &mut DerivationCache::default(),
            &publication(snapshot, 1, 1, 1),
        );
        assert_eq!(text.axes.formal_outcome.as_ref(), "COMPLETED");
    }

    #[test]
    fn disconnected_publication_does_not_change_task_status_axis() {
        let snapshot = fixture_snapshot(
            "Task",
            TaskStatus::Active,
            1,
            Vec::new(),
            "epoch/disconnected",
        );
        let mut publication = publication(snapshot, 1, 1, 1);
        publication.connection = PublicationConnection::Disconnected;
        let text = derive(&mut DerivationCache::default(), &publication);
        assert_eq!(text.axes.task_status.as_ref(), "ACTIVE");
        assert_eq!(text.axes.connection.as_ref(), "Disconnected");
    }

    #[test]
    fn accepted_and_canonical_reflected_receipts_are_not_task_completion() {
        let mut receipt = desktop_protocol::CommandReceipt {
            command_id: desktop_protocol::CommandId::fixture("command/stop"),
            operation: desktop_protocol::CommandOperation::StopTask,
            state: desktop_protocol::ReceiptState::Accepted,
            task_id: Some(TaskId::fixture("task/Task-184")),
            accepted_at: None,
            reject_reason: None,
            effect_refs: Vec::new(),
            dedupe_expires_at: None,
            message: Some("untrusted remote text".to_owned()),
        };
        let view = |receipt: desktop_protocol::CommandReceipt| app_core::CommandReadView {
            presentation: app_core::CommandPresentation::ReceiptAvailable,
            receipt: Some(receipt),
            issue: None,
        };
        let accepted = super::StopDisplay::from_command(&view(receipt.clone()));
        assert_eq!(accepted.state, super::StopState::Accepted);
        assert!(!accepted.label.contains("untrusted remote text"));
        let expired = super::StopDisplay::from_command(&app_core::CommandReadView {
            presentation: app_core::CommandPresentation::OutcomeUnconfirmed,
            receipt: Some(receipt.clone()),
            issue: None,
        });
        assert_eq!(expired.state, super::StopState::Unknown);
        assert!(expired.manual_intervention);

        receipt.state = desktop_protocol::ReceiptState::CanonicalReflected;
        let reflected = super::StopDisplay::from_command(&view(receipt));
        assert_eq!(reflected.state, super::StopState::CanonicalReflected);
    }
    #[test]
    fn a_missing_scoped_artifact_never_invents_an_availability_state() {
        let mut snapshot = fixture_snapshot(
            "Task",
            TaskStatus::Active,
            1,
            Vec::new(),
            "epoch/missing-artifact",
        );
        Arc::get_mut(&mut snapshot)
            .expect("fixture snapshot is uniquely owned")
            .tasks
            .values_mut()
            .next()
            .expect("fixture task")
            .accepted_brief_ref = Some(ArtifactId::fixture("artifact/not-projected"));
        let text = derive(
            &mut DerivationCache::default(),
            &publication(snapshot, 1, 1, 1),
        );
        assert!(text.axes.artifact_availability.contains("Unknown"));
        assert!(
            text.stop_enabled,
            "explicit task-scoped Stop support permits the action"
        );
    }
    #[test]
    fn unresolved_command_view_remains_unknown_and_possibly_executed() {
        let display = super::StopDisplay::from_command(&app_core::CommandReadView {
            presentation: app_core::CommandPresentation::OutcomeUnconfirmed,
            receipt: None,
            issue: None,
        });
        assert_eq!(display.state, super::StopState::Unknown);
        assert!(display.manual_intervention);
        assert_ne!(display.state, super::StopState::Accepted);
        assert_eq!(
            super::StopDisplay::admission_error(&app_core::CommandReadError::CapacityExceeded)
                .state,
            super::StopState::Unavailable
        );
        assert_eq!(
            super::StopDisplay::retention_error(&app_core::CommandReadError::CapacityExceeded)
                .state,
            super::StopState::Unknown
        );
        let ordinary_capacity =
            app_core::CommandReadError::Client(app_core::PublicationError::InvalidCommand(
                app_core::InvalidCommandReason::CapacityExceeded,
            ));
        assert_eq!(
            super::StopDisplay::admission_error(&ordinary_capacity).state,
            super::StopState::Unavailable
        );
        assert!(!super::StopDisplay::admission_error(&ordinary_capacity).manual_intervention);
        let reference_capacity =
            app_core::CommandReadError::Client(app_core::PublicationError::InvalidCommand(
                app_core::InvalidCommandReason::ReferenceLimit,
            ));
        assert_eq!(
            super::StopDisplay::retention_error(&reference_capacity).state,
            super::StopState::Unknown
        );
        assert!(super::StopDisplay::retention_error(&reference_capacity).manual_intervention);
    }
}
