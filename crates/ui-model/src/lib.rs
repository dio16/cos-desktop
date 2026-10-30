//! Window-local interactions and display text. Read-only Runtime projections
//! arrive as complete publications from app-core; this crate never calls the client.

pub mod inspector;
pub mod intervention;
pub mod palette;
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
            text.title = "タスク情報がありません".into();
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
            text.inspector = inspector::Index::derive(snapshot, task);
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
                text.title = "タスク情報がありません".into();
            }
        } else {
            self.reset();
            text.title = "タスク情報がありません".into();
        }

        for (id, view) in &publication.command_statuses {
            if cancelled() {
                self.reset();
                return None;
            }
            text.command_views
                .insert(id.clone(), StopDisplay::from_command(view));
        }
        text.interventions = match publication.snapshot.as_ref() {
            Some(snapshot) => {
                let task = match selected_task {
                    Some(identity) => snapshot.tasks.get(identity),
                    None => snapshot.tasks.values().next(),
                };
                // The strip does not trust this map's provenance: `Index::derive` re-proves each
                // Stop command's family and target task from its own receipt, so one task's Stop
                // state can never be attributed to another task's rows.
                intervention::Index::derive(publication, snapshot, task, &text.command_views)
            }
            None => intervention::Index::blocked(),
        };
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
                            "正式な結果（Runtimeの情報）: {}",
                            match &outcome.formal_status {
                                desktop_protocol::FormalStatus::Completed => "完了（COMPLETED）",
                                desktop_protocol::FormalStatus::Partial => "一部完了（PARTIAL）",
                                desktop_protocol::FormalStatus::Failed => "失敗（FAILED）",
                                desktop_protocol::FormalStatus::Cancelled => "中止（CANCELLED）",
                                desktop_protocol::FormalStatus::Unknown(raw) => raw.as_str(),
                            }
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
            StopState::Prepared => ("停止要求を準備しました", "まだ送信していません", false),
            StopState::AwaitingResponse => {
                ("停止要求の応答待ち", "実行された可能性があります", true)
            }
            StopState::Accepted => (
                "停止要求を受け付けました",
                "受け付けられたことはタスク完了を意味しません",
                false,
            ),
            StopState::Rejected => (
                "停止要求は拒否されました",
                "タスク状態は別途観測されます",
                false,
            ),
            StopState::Unknown => (
                "停止要求の結果は不明です",
                "実行された可能性があります。手動対応が必要な場合があります",
                true,
            ),
            StopState::CanonicalReflected => (
                "停止の効果を確認しました",
                "タスク完了状態は別途観測されます",
                false,
            ),
            StopState::Failed => (
                "停止要求に失敗しました",
                "効果があったとは判断できません",
                false,
            ),
            StopState::Unsupported => (
                "停止操作は未対応です",
                "実行環境はこの操作をサポートしていません",
                false,
            ),
            StopState::Unavailable => (
                "停止状態を確認できません",
                "許可された受領状態を取得できません",
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
                "停止要求を受け付けられません",
                "新しい停止要求は受け付けられていません",
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
                "停止要求の識別情報を保持できません",
                "要求は受け付け済みで、実行された可能性があります。再送せず、必要に応じて手動で対応してください",
                true,
            ),
            _ => Self::unavailable(),
        }
    }
    pub fn unavailable() -> Self {
        Self::make(
            StopState::Unavailable,
            "停止状態を確認できません",
            "許可された受領状態を取得できません。手動対応が必要な場合があります",
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
    /// Identity-bound inspector projection for the viewed candidate (DC-MP-INS1).
    pub inspector: inspector::Index,
    /// Typed Desktop-local intervention conditions (DC-MP-INS3 / ADR-0011).
    pub interventions: intervention::Index,
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
            "利用できません — 情報源を利用できません"
        } else {
            "不明"
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
                "停止状態を確認できません — 情報源を利用できません".into()
            } else {
                "停止状態を確認できません — 現在のタスク観測がありません".into()
            },
            interventions: if unavailable {
                intervention::Index::blocked()
            } else {
                intervention::Index::default()
            },
            ..Default::default()
        }
    }

    pub fn withheld() -> Self {
        Self {
            title: "タスク情報はありません".into(),
            connection: "データは非公開 — 現在の許可がありません".into(),
            source: "検証済みの情報源がありません".into(),
            mode: "情報源なし / データ非公開".into(),
            axes: StateAxes {
                task_status: "利用できません — 許可情報を取得できません".into(),
                inner_phase: "利用できません — 許可情報を取得できません".into(),
                iteration: "利用できません — 許可情報を取得できません".into(),
                artifact_availability: "利用できません — 許可情報を取得できません".into(),
                connection: "データは非公開 — 現在の許可がありません".into(),
                formal_outcome: "利用できません — 許可情報を取得できません".into(),
            },
            stop_reason: "停止状態を確認できません — 現在の許可がありません".into(),
            interventions: intervention::Index::blocked(),
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
            let label = match &artifact.availability {
                desktop_protocol::Availability::Available => "取得可能（AVAILABLE）",
                desktop_protocol::Availability::Staged => "準備済み（STAGED）",
                desktop_protocol::Availability::Pending => "保留中（PENDING）",
                desktop_protocol::Availability::Missing => "見つかりません（MISSING）",
                desktop_protocol::Availability::Forbidden => "許可されていません（FORBIDDEN）",
                desktop_protocol::Availability::Failed => "失敗（FAILED）",
                desktop_protocol::Availability::Quarantined => "隔離中（QUARANTINED）",
                desktop_protocol::Availability::Unsupported => "未対応（UNSUPPORTED）",
                desktop_protocol::Availability::Unknown(raw) => raw.as_str(),
            };
            *counts.entry(label).or_default() += 1;
            projected += 1;
        }
    }
    let artifact_availability = if reference_count == 0 {
        "不明 — 対象タスクに成果物の参照がありません".to_owned()
    } else if projected == 0 {
        format!("不明 — 成果物の利用可否を受け取れていません（参照数: {reference_count}）")
    } else {
        let mut states = counts
            .iter()
            .map(|(state, count)| format!("{state}（{count}件）"))
            .collect::<Vec<_>>();
        if projected < reference_count {
            states.push(format!(
                "利用可否の情報がありません（{}件）",
                reference_count - projected
            ));
        }
        states.join("、")
    };
    let formal_outcome = task
        .formal_outcome_ref
        .as_ref()
        .and_then(|id| snapshot.outcomes.get(id))
        .filter(|outcome| outcome.task_ref == task.task_ref)
        .map(|outcome| match &outcome.formal_status {
            desktop_protocol::FormalStatus::Completed => "完了（COMPLETED）".to_owned(),
            desktop_protocol::FormalStatus::Partial => "一部完了（PARTIAL）".to_owned(),
            desktop_protocol::FormalStatus::Failed => "失敗（FAILED）".to_owned(),
            desktop_protocol::FormalStatus::Cancelled => "中止（CANCELLED）".to_owned(),
            desktop_protocol::FormalStatus::Unknown(raw) => raw.clone(),
        })
        .unwrap_or_else(|| "不明 — 正式な結果の情報を受け取れていません".to_owned());
    StateAxes {
        task_status: match &task.task_status {
            desktop_protocol::TaskStatus::Assigned => "割り当て済み（ASSIGNED）",
            desktop_protocol::TaskStatus::Active => "実行中（ACTIVE）",
            desktop_protocol::TaskStatus::Completed => "完了（COMPLETED）",
            desktop_protocol::TaskStatus::Cancelled => "中止（CANCELLED）",
            desktop_protocol::TaskStatus::Unknown(raw) => raw.as_str(),
        }
        .into(),
        inner_phase: task.inner_phase.as_str().into(),
        iteration: task
            .iteration_index
            .map_or_else(|| "不明".to_owned(), |n| format!("第{n}回"))
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
            "停止状態を確認できません — 情報源または許可状態がありません".into(),
        );
    }
    if !matches!(publication.connection, C::Observing) {
        return (
            false,
            "停止状態を確認できません — 許可されたセッションが観測中ではありません".into(),
        );
    }
    let Some(snapshot) = publication.snapshot.as_ref() else {
        return (
            false,
            "停止状態を確認できません — 操作可否の情報がありません".into(),
        );
    };
    let Some(capabilities) = snapshot.capabilities.get(&task.capabilities_ref) else {
        return (
            false,
            "停止状態を確認できません — 操作可否の情報がありません".into(),
        );
    };
    if !capabilities.availability.is_supported() {
        return (
            false,
            "停止状態を確認できません — コマンドの利用可否がありません".into(),
        );
    }
    match capabilities.command_support(&desktop_protocol::CommandOperation::StopTask) {
        Some(desktop_protocol::SupportState::Supported) => {}
        Some(desktop_protocol::SupportState::Unsupported) => {
            return (false, "停止操作は未対応です".into());
        }
        Some(desktop_protocol::SupportState::Unavailable)
        | Some(desktop_protocol::SupportState::Unknown(_))
        | None => {
            return (
                false,
                "停止状態を確認できません — コマンドの対応状況が不明です".into(),
            );
        }
    }
    match task.task_status {
        desktop_protocol::TaskStatus::Active | desktop_protocol::TaskStatus::Assigned => {
            (true, "タスクを停止".into())
        }
        desktop_protocol::TaskStatus::Unknown(_) => (
            false,
            "停止状態を確認できません — タスク状態が不明です".into(),
        ),
        _ => (
            false,
            "停止状態を確認できません — タスクは実行中ではありません".into(),
        ),
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
                label: format!("第{}回", iteration.index).into(),
                short_label: format!("反復 {}", iteration.index).into(),
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
            label: "反復情報なし".into(),
            short_label: "反復 ?".into(),

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
        "判断: {}（観測データ。評価者の出力ではありません）",
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
                .map_or_else(|| "利用不可".to_owned(), |value| value.to_string()),
            axis.unit.as_deref().unwrap_or("")
        ));
    }
    for evidence in &evaluation.evidence_refs {
        if cancelled() {
            return None;
        }
        if let Some(summary) = &evidence.summary {
            lines.push(format!("証拠: {summary}"));
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
        PublicationConnection::Disconnected => "未接続",
        PublicationConnection::Connecting => "接続中",
        PublicationConnection::Observing => "情報を受信中",
        PublicationConnection::Recovering => "再接続中",
        PublicationConnection::Unauthorized => "認証されていません",
        PublicationConnection::Incompatible => "互換性がありません",
    };
    if publication.stale {
        format!("{base} · 古い情報の可能性")
    } else {
        base.to_owned()
    }
}
fn source_label(publication: &ObservationPublication) -> String {
    match publication.source {
        PublicationSource::Runtime => "Runtime".to_owned(),
        PublicationSource::Fixture => "テスト用記録".to_owned(),
        PublicationSource::Unavailable => "検証済みの情報源がありません".to_owned(),
    }
}
fn mode_label(publication: &ObservationPublication) -> String {
    match publication.source {
        PublicationSource::Fixture => "テストデータ / 模擬表示".to_owned(),
        PublicationSource::Runtime => "Runtime / 情報を受信中".to_owned(),
        PublicationSource::Unavailable => "情報源なし / データ非公開".to_owned(),
    }
}
fn diagnostics(publication: &ObservationPublication) -> String {
    let error = publication
        .last_error
        .as_ref()
        .map_or_else(|| "なし".to_owned(), error_label);
    format!(
        "Desktop {} · {} · 世代 {} · 公開 {} · エラー {} · コマンド {}件",
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
        PublicationError::Transport => "通信".to_owned(),
        PublicationError::Protocol => "プロトコル".to_owned(),
        PublicationError::VersionIncompatible => "バージョン".to_owned(),
        PublicationError::Unauthorized => "認証エラー".to_owned(),
        PublicationError::VerificationPending => "検証待ち".to_owned(),
        PublicationError::QueueOverflow => "キュー上限超過".to_owned(),
        PublicationError::MessageTooLarge => "メッセージが大きすぎます".to_owned(),
        PublicationError::NotConnected => "未接続".to_owned(),
        PublicationError::InvalidCommand(_) => "コマンド拒否".to_owned(),
        PublicationError::Server(code) => format!("サーバー {code:?}"),
        PublicationError::Fixture => "テストデータ".to_owned(),
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
        CapabilitiesId, Counter, EntityId, EntityKind, EntityRef, EvaluationDecision, EvaluationId,
        EvaluationSnapshot, EvidenceRef, EvidenceStatus, FormalStatus, GraphId, GraphNode,
        GraphRef, GraphSnapshot, IterationId, IterationSnapshot, NamedScore, OutcomeId,
        OutcomeSnapshot, PhaseLabel, RunId, TaskId, TaskSnapshot, TaskStatus,
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

    fn inspector_candidate(
        local: &str,
        label: &str,
        iteration: u32,
        evaluations: &[EvaluationId],
    ) -> CandidateSnapshot {
        let mut candidate = dock_candidate(local, label, iteration);
        candidate.evaluation_refs = evaluations.to_vec();
        candidate
    }

    fn inspector_evaluation(
        local: &str,
        candidate: &CandidateRef,
        decision: EvaluationDecision,
        value: Option<f64>,
        confidence: Option<f64>,
        evidence: Vec<EvidenceRef>,
        revision: u128,
    ) -> EvaluationSnapshot {
        EvaluationSnapshot {
            evaluation_ref: EvaluationId::fixture(local),
            candidate_ref: candidate.clone(),
            artifact_refs: vec![candidate.artifact_ref.clone()],
            decision,
            axes: vec![NamedScore {
                name: "visual_quality".to_owned(),
                unit: Some("score_0_100".to_owned()),
                value,
            }],
            evidence_refs: evidence,
            authority: AuthorityLabel::new("HARNESS").expect("valid authority"),
            confidence,
            projection_revision: Counter::from_u128(revision),
        }
    }

    fn evidence(local: &str, status: EvidenceStatus, summary: Option<&str>) -> EvidenceRef {
        EvidenceRef {
            record_ref: EntityRef {
                kind: EntityKind::Evidence,
                id: EntityId::fixture(local),
            },
            status,
            summary: summary.map(str::to_owned),
            detail: None,
        }
    }

    fn inspector_snapshot(
        candidates: Vec<CandidateSnapshot>,
        iterations: Vec<IterationSnapshot>,
        evaluations: Vec<EvaluationSnapshot>,
        outcomes: Vec<OutcomeSnapshot>,
        formal_outcome_ref: Option<OutcomeId>,
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
        for evaluation in evaluations {
            state
                .evaluations
                .insert(evaluation.evaluation_ref.clone(), evaluation);
        }
        for outcome in outcomes {
            state.outcomes.insert(outcome.outcome_ref.clone(), outcome);
        }
        let task_ref = TaskId::fixture("task/Task-184");
        state.tasks.insert(
            task_ref.clone(),
            TaskSnapshot {
                task_ref: task_ref.clone(),
                display_id: "Task-184".to_owned(),
                title: "Inspector task".to_owned(),
                task_status: TaskStatus::Active,
                inner_phase: PhaseLabel::new("REFINE").expect("valid phase"),
                iteration_ref: Some(IterationId::fixture("iteration/Task-184/1")),
                iteration_index: Some(1),
                best_candidate_ref: None,
                formal_outcome_ref,
                accepted_brief_ref: None,
                capabilities_ref: CapabilitiesId::fixture("capabilities/task"),
                projection_revision: Counter::from_u128(1),
            },
        );
        snapshot
    }

    fn field_value(section: &super::inspector::SectionView, label: &str) -> Option<String> {
        section
            .entries
            .iter()
            .flat_map(|entry| entry.fields.iter())
            .find(|field| field.label.as_ref() == label)
            .map(|field| field.value.to_string())
    }

    #[test]
    fn the_inspector_binds_the_viewed_candidate_by_complete_identity() {
        let a = inspector_candidate("a", "A", 1, &[EvaluationId::fixture("evaluation/a")]);
        let z = inspector_candidate("z", "Z", 1, &[EvaluationId::fixture("evaluation/z")]);
        let evaluate_a = inspector_evaluation(
            "evaluation/a",
            &a.candidate_ref,
            EvaluationDecision::Fail,
            Some(12.0),
            Some(0.2),
            vec![],
            1,
        );
        let evaluate_z = inspector_evaluation(
            "evaluation/z",
            &z.candidate_ref,
            EvaluationDecision::Pass,
            Some(88.0),
            Some(0.81),
            vec![],
            1,
        );
        let mut cache = DerivationCache::default();
        // Both candidates share one iteration group; the viewed candidate is second.
        let first = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![a.clone(), z.clone()],
                    vec![dock_iteration(
                        1,
                        vec![a.candidate_ref.clone(), z.candidate_ref.clone()],
                    )],
                    vec![evaluate_a.clone(), evaluate_z.clone()],
                    vec![],
                    None,
                ),
                1,
                1,
                1,
            ),
        );
        assert_eq!(
            first.candidate_groups[0].rows[0].identity, a.candidate_ref,
            "the first projection places the viewed candidate second"
        );
        let view_z = first.inspector.view(Some(&z.candidate_ref));
        assert!(
            view_z.sections[1]
                .entries
                .iter()
                .flat_map(|entry| entry.fields.iter())
                .any(|field| field.value.contains("PASS"))
        );
        assert_eq!(
            field_value(&view_z.sections[1], "visual_quality"),
            Some("88 score_0_100".into())
        );

        // The projection reorders: the viewed candidate moves to the first group and
        // the other candidate moves to a later iteration. The binding must not migrate.
        let mut moved_a = dock_iteration(2, vec![a.candidate_ref.clone()]);
        moved_a.iteration_ref = IterationId::fixture("iteration/Task-184/2");
        let mut moved_a_candidate = a.clone();
        moved_a_candidate.iteration_ref = moved_a.iteration_ref.clone();
        let second = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![moved_a_candidate, z.clone()],
                    vec![dock_iteration(1, vec![z.candidate_ref.clone()]), moved_a],
                    vec![evaluate_a, evaluate_z],
                    vec![],
                    None,
                ),
                1,
                2,
                2,
            ),
        );
        assert_eq!(
            second.candidate_groups[0].rows[0].identity, z.candidate_ref,
            "the dock order really changed"
        );
        let reordered = second.inspector.view(Some(&z.candidate_ref));
        assert!(
            reordered.sections[1]
                .entries
                .iter()
                .flat_map(|entry| entry.fields.iter())
                .any(|field| field.value.contains("PASS")),
            "the inspector must still show the viewed candidate's evaluation"
        );
        assert_eq!(
            field_value(&reordered.sections[1], "visual_quality"),
            Some("88 score_0_100".into())
        );
    }

    #[test]
    fn a_missing_score_and_confidence_are_never_rendered_as_zero_or_fail() {
        let candidate = inspector_candidate("c", "C", 1, &[EvaluationId::fixture("evaluation/c")]);
        let evaluate = inspector_evaluation(
            "evaluation/c",
            &candidate.candidate_ref,
            EvaluationDecision::Abstain,
            None,
            None,
            vec![],
            1,
        );
        let mut cache = DerivationCache::default();
        let text = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![candidate.clone()],
                    vec![dock_iteration(1, vec![candidate.candidate_ref.clone()])],
                    vec![evaluate],
                    vec![],
                    None,
                ),
                1,
                1,
                1,
            ),
        );
        let view = text.inspector.view(Some(&candidate.candidate_ref));
        let evaluation = &view.sections[1];
        assert!(
            evaluation
                .entries
                .iter()
                .flat_map(|entry| entry.fields.iter())
                .any(|field| field.value.contains("ABSTAIN"))
        );
        assert!(
            evaluation
                .entries
                .iter()
                .flat_map(|entry| entry.fields.iter())
                .all(|field| field.value.as_ref() != "0" && field.value.as_ref() != "FAIL"),
            "an absent value is never zero or FAIL"
        );
    }

    #[test]
    fn an_absent_outcome_is_an_explicit_absent_state_not_a_failure() {
        let candidate = inspector_candidate("a", "A", 1, &[]);
        let mut cache = DerivationCache::default();
        let text = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![candidate.clone()],
                    vec![dock_iteration(1, vec![candidate.candidate_ref.clone()])],
                    vec![],
                    vec![],
                    None,
                ),
                1,
                1,
                1,
            ),
        );
        let view = text.inspector.view(Some(&candidate.candidate_ref));
        let outcome = &view.sections[4];
        assert!(!outcome.available);
        assert!(outcome.absent_reason.is_some());
        assert!(outcome.entries.is_empty());
        assert!(
            !outcome.summary.contains("FAILED"),
            "an absent outcome is not failure"
        );
    }

    #[test]
    fn evidence_retains_the_reported_missing_availability() {
        let candidate = inspector_candidate("c", "C", 1, &[EvaluationId::fixture("evaluation/c")]);
        let evaluate = inspector_evaluation(
            "evaluation/c",
            &candidate.candidate_ref,
            EvaluationDecision::Abstain,
            None,
            None,
            vec![evidence("evidence/c", EvidenceStatus::Missing, None)],
            1,
        );
        let mut cache = DerivationCache::default();
        let text = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![candidate.clone()],
                    vec![dock_iteration(1, vec![candidate.candidate_ref.clone()])],
                    vec![evaluate],
                    vec![],
                    None,
                ),
                1,
                1,
                1,
            ),
        );
        let view = text.inspector.view(Some(&candidate.candidate_ref));
        let evidence = &view.sections[2];
        assert!(evidence.available);
        assert_eq!(
            field_value(evidence, "概要"),
            Some("概要の情報はありません".into())
        );
        assert_eq!(
            field_value(evidence, "利用可否"),
            Some("見つかりません（MISSING）".into())
        );
    }

    #[test]
    fn a_refinement_reason_is_explicitly_absent_when_only_references_are_projected() {
        let candidate = inspector_candidate("a", "A", 1, &[]);
        let mut iteration = dock_iteration(1, vec![candidate.candidate_ref.clone()]);
        iteration.refinement_refs = vec![desktop_protocol::RefinementId::fixture("refinement/1")];
        let text = derive(
            &mut DerivationCache::default(),
            &publication(
                inspector_snapshot(
                    vec![candidate.clone()],
                    vec![iteration],
                    vec![],
                    vec![],
                    None,
                ),
                1,
                1,
                1,
            ),
        );
        let view = text.inspector.view(Some(&candidate.candidate_ref));
        let refinement = &view.sections[3];
        assert!(refinement.available);
        assert_eq!(
            field_value(refinement, "改善記録"),
            Some("1件を表示".into())
        );
        assert!(
            field_value(refinement, "理由")
                .unwrap_or_default()
                .contains("参照情報のみ"),
            "a reason absent from the projection is stated as absent"
        );
    }

    #[test]
    fn inspector_section_revisions_follow_the_projection_revision() {
        let candidate = inspector_candidate("a", "A", 1, &[EvaluationId::fixture("evaluation/a")]);
        let mut cache = DerivationCache::default();
        let first = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![candidate.clone()],
                    vec![dock_iteration(1, vec![candidate.candidate_ref.clone()])],
                    vec![inspector_evaluation(
                        "evaluation/a",
                        &candidate.candidate_ref,
                        EvaluationDecision::Pass,
                        Some(50.0),
                        Some(0.5),
                        vec![],
                        1,
                    )],
                    vec![],
                    None,
                ),
                1,
                1,
                1,
            ),
        );
        let before = first
            .inspector
            .view(Some(&candidate.candidate_ref))
            .sections[1]
            .revision
            .clone();
        let second = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![candidate.clone()],
                    vec![dock_iteration(1, vec![candidate.candidate_ref.clone()])],
                    vec![inspector_evaluation(
                        "evaluation/a",
                        &candidate.candidate_ref,
                        EvaluationDecision::Pass,
                        Some(50.0),
                        Some(0.5),
                        vec![],
                        2,
                    )],
                    vec![],
                    None,
                ),
                1,
                2,
                2,
            ),
        );
        let after = &second
            .inspector
            .view(Some(&candidate.candidate_ref))
            .sections[1];
        assert_ne!(before, after.revision);
    }

    #[test]
    fn a_referenced_evaluation_identity_change_invalidates_the_disclosure_key() {
        use super::inspector::{Disclosure, Section};

        let candidate = inspector_candidate("a", "A", 1, &[EvaluationId::fixture("evaluation/a")]);
        let mut cache = DerivationCache::default();
        let first = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![candidate.clone()],
                    vec![dock_iteration(1, vec![candidate.candidate_ref.clone()])],
                    vec![inspector_evaluation(
                        "evaluation/a",
                        &candidate.candidate_ref,
                        EvaluationDecision::Pass,
                        Some(50.0),
                        Some(0.5),
                        vec![evidence(
                            "evidence/a",
                            EvidenceStatus::Available,
                            Some("summary a"),
                        )],
                        1,
                    )],
                    vec![],
                    None,
                ),
                1,
                1,
                1,
            ),
        );
        // Same candidate identity, same revision number, different evaluation identity.
        let swapped = inspector_candidate("a", "A", 1, &[EvaluationId::fixture("evaluation/b")]);
        let second = derive(
            &mut cache,
            &publication(
                inspector_snapshot(
                    vec![swapped.clone()],
                    vec![dock_iteration(1, vec![swapped.candidate_ref.clone()])],
                    vec![inspector_evaluation(
                        "evaluation/b",
                        &swapped.candidate_ref,
                        EvaluationDecision::Pass,
                        Some(50.0),
                        Some(0.5),
                        vec![evidence(
                            "evidence/b",
                            EvidenceStatus::Available,
                            Some("summary b"),
                        )],
                        1,
                    )],
                    vec![],
                    None,
                ),
                1,
                2,
                2,
            ),
        );
        let before = first
            .inspector
            .view(Some(&candidate.candidate_ref))
            .sections[1]
            .revision
            .clone();
        let after = second
            .inspector
            .view(Some(&candidate.candidate_ref))
            .sections[1]
            .revision
            .clone();
        assert_ne!(
            before, after,
            "the evaluation identity participates in the disclosure key"
        );
        assert_ne!(
            first
                .inspector
                .view(Some(&candidate.candidate_ref))
                .sections[2]
                .revision,
            second
                .inspector
                .view(Some(&candidate.candidate_ref))
                .sections[2]
                .revision,
            "the evidence key follows the same composition"
        );
        let mut disclosure = Disclosure::default();
        disclosure.sync(Some(&candidate.candidate_ref), true);
        assert!(disclosure.toggle(Section::Evaluation, &before));
        // Re-observation keeps the same candidate and revision number; the changed
        // evaluation identity must not inherit the old open state.
        disclosure.sync(Some(&candidate.candidate_ref), true);
        assert!(!disclosure.is_open(Section::Evaluation, &after));
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
        let indices = text
            .candidate_groups
            .iter()
            .map(|group| group.index)
            .collect::<Vec<_>>();
        assert_eq!(indices, vec![Some(1), Some(3)]);
        assert_eq!(text.formal_candidates.len(), 0);
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
        assert!(display.text().task.is_none());
        assert!(!display.text().title.contains("Task title"));
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
        assert!(initial.axes.task_status.contains("ACTIVE"));
        assert!(initial.stop_enabled);

        let selected = cache
            .derive(&publication, Some(&selected_id), || false)
            .expect("selected task derives");
        assert_eq!(selected.task.as_ref(), Some(&selected_id));
        assert_eq!(selected.title.as_ref(), "Task-999  Second selected");
        assert!(selected.axes.task_status.contains("COMPLETED"));
        assert_eq!(selected.axes.inner_phase.as_ref(), "REVIEW");
        assert!(!selected.stop_enabled);

        let missing_id = TaskId::fixture("task/removed");
        let missing = cache
            .derive(&publication, Some(&missing_id), || false)
            .expect("missing selected task yields unavailable context");
        assert_eq!(missing.task, None);
        assert!(!missing.stop_enabled);
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
        assert!(text.axes.task_status.contains("COMPLETED"));
        assert!(text.axes.artifact_availability.contains("STAGED"));
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
        assert!(!text.stop_enabled);
    }
    #[test]
    fn actual_formal_outcome_is_selected_by_the_task_reference() {
        let snapshot = fixture_snapshot("Task", TaskStatus::Active, 1, Vec::new(), "epoch/formal");
        let text = derive(
            &mut DerivationCache::default(),
            &publication(snapshot, 1, 1, 1),
        );
        assert!(text.axes.formal_outcome.contains("COMPLETED"));
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
        let connected = derive(&mut DerivationCache::default(), &publication);
        publication.connection = PublicationConnection::Disconnected;
        let text = derive(&mut DerivationCache::default(), &publication);
        assert_eq!(text.axes.task_status, connected.axes.task_status);
        assert_ne!(text.axes.connection, connected.axes.connection);
    }

    #[test]
    fn japanese_presentation_preserves_unknown_runtime_values_and_disables_stop() {
        let mut snapshot = fixture_snapshot(
            "Runtime-provided title",
            TaskStatus::Unknown("VENDOR_STATUS_V2".to_owned()),
            1,
            Vec::new(),
            "epoch/unknown-values",
        );
        let state = Arc::get_mut(&mut snapshot).expect("fixture snapshot is uniquely owned");
        let artifact = ArtifactId::fixture("artifact/unknown-availability");
        let task = state
            .tasks
            .get_mut(&TaskId::fixture("task/Task-184"))
            .expect("fixture task");
        task.inner_phase = PhaseLabel::new("VENDOR_PHASE_V2").expect("valid open phase");
        task.accepted_brief_ref = Some(artifact.clone());
        state.artifacts.insert(
            artifact.clone(),
            desktop_protocol::ArtifactSnapshot {
                artifact_ref: artifact,
                artifact_digest: ArtifactDigest::fixture("digest/unknown-availability"),
                media_type: None,
                availability: desktop_protocol::Availability::Unknown(
                    "VENDOR_AVAILABILITY_V2".to_owned(),
                ),
                descriptor_refs: Vec::new(),
                verification: None,
                projection_revision: Counter::from_u128(1),
            },
        );
        state
            .outcomes
            .get_mut(&OutcomeId::fixture("outcome/main"))
            .expect("fixture outcome")
            .formal_status = FormalStatus::Unknown("VENDOR_OUTCOME_V2".to_owned());

        let text = derive(
            &mut DerivationCache::default(),
            &publication(snapshot, 1, 1, 1),
        );
        assert_eq!(text.axes.task_status.as_ref(), "VENDOR_STATUS_V2");
        assert_eq!(text.axes.inner_phase.as_ref(), "VENDOR_PHASE_V2");
        assert_eq!(text.axes.formal_outcome.as_ref(), "VENDOR_OUTCOME_V2");
        assert!(
            text.axes
                .artifact_availability
                .contains("VENDOR_AVAILABILITY_V2")
        );
        assert!(
            !text.stop_enabled,
            "unknown task status cannot enable a command"
        );
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
        assert!(
            accepted.detail.contains("タスク完了を意味しません"),
            "an accepted receipt states that acceptance is not task completion: {}",
            accepted.detail
        );
        assert!(!accepted.manual_intervention);
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
        for state in [
            "AVAILABLE",
            "STAGED",
            "PENDING",
            "MISSING",
            "FORBIDDEN",
            "FAILED",
            "QUARANTINED",
            "UNSUPPORTED",
        ] {
            assert!(!text.axes.artifact_availability.contains(state));
        }
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
