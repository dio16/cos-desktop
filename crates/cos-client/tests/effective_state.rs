//! MP-COR1 / DC-MP-COR1 — delta effective-state integrity.
//!
//! Regression slice for the aggregate validator that is applied to a
//! transaction's *effective* state (current state + upserts − removals) before
//! anything is published.
//!
//! Every test names the plausible bug it catches and its independent oracle.
//! The oracles are the reviewed `task-184` fixture identities plus the
//! published contract DC-MP-COR1 (dangling references and reciprocal
//! iteration↔candidate ownership must be rejected; a legal rewire must be
//! accepted; a rejection must not publish partially). They are derived from
//! the contract and the fixture, never copied from
//! `ProjectionState::validate_effective_state`.

mod common;

use common::*;
use cos_client::{ClientEvent, ConnectionState, MockCosClient, ProjectionState};
use desktop_protocol::{
    ArtifactId, CandidateId, CandidateSnapshot, Counter, Cursor, DeltaTransaction, EpochId,
    EvaluationId, IterationId, IterationSnapshot, Projection, ProjectionMutation, RemoveTarget,
    TaskSnapshot, TransactionId, integrity_digest_of,
};

/// Encode a transaction that starts or continues the opaque fixture cursor
/// chain. `predecessor` is always the cursor observed from the client, so the
/// tests never invent an ordering for an opaque position.
fn transaction(
    id: &str,
    cursor: &str,
    predecessor: Option<Cursor>,
    mutations: Vec<ProjectionMutation>,
) -> DeltaTransaction {
    DeltaTransaction {
        transaction_id: TransactionId::fixture(id),
        epoch: EpochId::fixture("epoch/1"),
        cursor: Cursor::fixture(cursor),
        predecessor,
        mutation_count: mutations.len() as u32,
        integrity_digest: integrity_digest_of(&mutations).expect("transaction digest"),
        mutations,
    }
}

fn upsert(base: &str, projection: Projection) -> ProjectionMutation {
    ProjectionMutation::Upsert {
        base_revision: Some(Counter::new(base).expect("counter")),
        projection,
    }
}

fn removal(base: &str, target: RemoveTarget) -> ProjectionMutation {
    ProjectionMutation::Remove {
        base_revision: Counter::new(base).expect("counter"),
        target,
    }
}

fn task(state: &ProjectionState) -> TaskSnapshot {
    state.task(&task_id()).cloned().expect("fixture task")
}

/// Fixture candidate addressed by its local identity (`fixture:candidate/<x>`).
fn candidate(state: &ProjectionState, local: &str) -> CandidateSnapshot {
    let wanted = CandidateId::fixture(local);
    state
        .candidates
        .values()
        .find(|snapshot| snapshot.candidate_ref.candidate_id == wanted)
        .cloned()
        .expect("fixture candidate")
}

fn iteration(state: &ProjectionState, local: &str) -> IterationSnapshot {
    state
        .iterations
        .get(&IterationId::fixture(local))
        .cloned()
        .expect("fixture iteration")
}

fn delta_applied(events: &[ClientEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
}

/// Structural oracle for the contract's reciprocal ownership rule: every
/// candidate listed by an iteration must point back at that iteration, and
/// every candidate's iteration must exist. Written from DC-MP-COR1, not from
/// the production validator.
fn assert_reciprocal_iteration_ownership(state: &ProjectionState) {
    for iter in state.iterations.values() {
        assert!(
            state.tasks.contains_key(&iter.task_ref),
            "iteration {} references an absent task",
            iter.iteration_ref
        );
        for listed in &iter.candidate_refs {
            let owned = state
                .candidates
                .get(listed)
                .expect("a listed candidate must exist");
            assert_eq!(
                owned.iteration_ref, iter.iteration_ref,
                "iteration {} lists candidate {} owned by {}",
                iter.iteration_ref, listed.candidate_id, owned.iteration_ref
            );
        }
    }
    for snapshot in state.candidates.values() {
        assert!(
            state.iterations.contains_key(&snapshot.iteration_ref),
            "candidate {} references an absent iteration {}",
            snapshot.candidate_ref.candidate_id,
            snapshot.iteration_ref
        );
    }
}

/// Observable rejection contract shared by scenarios 1, 2 and 4: an error
/// event, a session that is no longer live, an unchanged published projection
/// and an unchanged applied cursor.
fn assert_rejected_without_side_effects(
    mock: &MockCosClient,
    before: &ProjectionState,
    cursor_before: &Cursor,
) {
    assert!(
        !mock.connection().is_live(),
        "a rejected transaction must leave the live state"
    );
    assert!(
        matches!(mock.connection(), ConnectionState::Reconnecting { .. }),
        "a rejected transaction must schedule bounded recovery"
    );
    assert!(
        mock.is_stale(),
        "the last valid projection must be marked stale"
    );
    assert_eq!(
        mock.snapshot().as_ref(),
        before,
        "a rejected transaction must never partially publish"
    );
    assert_eq!(
        mock.client().last_seen_cursor(),
        Some(cursor_before),
        "a rejected transaction must not advance the applied cursor"
    );
}

#[test]
fn removing_an_artifact_still_referenced_by_a_candidate_is_rejected() {
    // Catches: repro A — an artifact removal is published even though an
    // unchanged candidate still lists it, leaving a dangling `artifact_refs`
    // entry that the snapshot path would never accept.
    // Oracle: DC-MP-COR1 rejection contract plus the reviewed fixture tuple
    // `candidate/a`, whose `artifact_refs` is exactly `[artifact/a]`;
    // `fixture:artifact/a` is therefore an unchanged holder reference.
    let mut mock = live_mock();
    let before = mock.snapshot();
    let cursor_before = mock.client().last_seen_cursor().cloned().expect("cursor");

    let artifact = ArtifactId::fixture("artifact/a");
    assert!(
        candidate(&before, "candidate/a")
            .artifact_refs
            .contains(&artifact),
        "fixture precondition: candidate/a references artifact/a"
    );
    let mutations = vec![removal(
        "1",
        RemoveTarget::Artifact {
            artifact_id: artifact.clone(),
        },
    )];

    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/cor1-dangling-artifact",
        "cursor/cor1-dangling",
        Some(cursor_before.clone()),
        mutations,
    )));
    let events = mock.pump(0);

    assert!(
        has_error(&events),
        "a dangling artifact reference must surface an error event"
    );
    assert!(
        !delta_applied(&events),
        "a rejected transaction must not publish"
    );
    assert_rejected_without_side_effects(&mock, &before, &cursor_before);

    // The next pump performs the bounded resync; the rejected removal is never
    // resurrected from a staged buffer.
    let events = mock.pump(1_000_000);
    assert!(
        mock.connection().is_live(),
        "bounded recovery must return to Live"
    );
    assert!(!has_error(&events));
    assert!(
        mock.snapshot().artifacts.contains_key(&artifact),
        "the rejected removal must not survive the resync"
    );
}

#[test]
fn reciprocal_iteration_candidate_mismatch_inside_one_transaction_is_rejected() {
    // Catches: repro B — two upserts that are individually admissible against
    // the current state but describe an inconsistent effective state: the
    // candidate moves to iteration 1 while iteration 3 starts listing it, so
    // the reciprocal ownership rule is broken.
    // Oracle: DC-MP-COR1 aggregate rule + fixture identities
    // `candidate/c` (iteration/Task-184/2, revision 3) and
    // `iteration/Task-184/3` (revision 5).
    let mut mock = live_mock();
    let before = mock.snapshot();
    let cursor_before = mock.client().last_seen_cursor().cloned().expect("cursor");

    let mut moved = candidate(&before, "candidate/c");
    moved.iteration_ref = IterationId::fixture("iteration/Task-184/1");
    moved.projection_revision = Counter::new("4").expect("counter");

    let mut listing = iteration(&before, "iteration/Task-184/3");
    listing
        .candidate_refs
        .push(candidate(&before, "candidate/c").candidate_ref.clone());
    listing.projection_revision = Counter::new("6").expect("counter");

    let mutations = vec![
        upsert("3", Projection::Candidate(moved)),
        upsert("5", Projection::Iteration(listing)),
    ];

    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/cor1-reciprocal-mismatch",
        "cursor/cor1-reciprocal",
        Some(cursor_before.clone()),
        mutations,
    )));
    let events = mock.pump(0);

    assert!(
        has_error(&events),
        "an inconsistent effective iteration/candidate pair must be rejected"
    );
    assert!(
        !delta_applied(&events),
        "a rejected transaction must not publish"
    );
    assert_rejected_without_side_effects(&mock, &before, &cursor_before);
}

/// The legal transaction-local rewire used by the order-independence test:
/// move `candidate/e` from the removed `iteration/Task-184/3` into
/// `iteration/Task-184/2` and re-point the task at the surviving iteration.
fn rewire_mutations(state: &ProjectionState) -> Vec<ProjectionMutation> {
    let mut moved = candidate(state, "candidate/e");
    moved.iteration_ref = IterationId::fixture("iteration/Task-184/2");
    moved.projection_revision = Counter::new("6").expect("counter");

    let mut listing = iteration(state, "iteration/Task-184/2");
    listing
        .candidate_refs
        .push(candidate(state, "candidate/e").candidate_ref.clone());
    listing.projection_revision = Counter::new("5").expect("counter");

    let mut current = task(state);
    current.iteration_ref = Some(IterationId::fixture("iteration/Task-184/2"));
    current.iteration_index = Some(2);
    current.projection_revision = Counter::new("9").expect("counter");

    vec![
        removal(
            "5",
            RemoveTarget::Iteration {
                iteration_id: IterationId::fixture("iteration/Task-184/3"),
            },
        ),
        upsert("5", Projection::Candidate(moved)),
        upsert("4", Projection::Iteration(listing)),
        upsert("8", Projection::Task(current)),
    ]
}

fn run_rewire(reversed: bool) -> (ProjectionState, Cursor) {
    let mut mock = live_mock();
    let cursor_before = mock.client().last_seen_cursor().cloned().expect("cursor");

    let mut mutations = rewire_mutations(&mock.snapshot());
    if reversed {
        mutations.reverse();
    }
    mock.mock_mut().inject_raw(delta_frame(transaction(
        if reversed {
            "transaction/cor1-rewire-reversed"
        } else {
            "transaction/cor1-rewire"
        },
        if reversed {
            "cursor/cor1-rewire-reversed"
        } else {
            "cursor/cor1-rewire"
        },
        Some(cursor_before),
        mutations,
    )));
    let events = mock.pump(0);
    assert!(
        delta_applied(&events),
        "the legal rewire must be published: {events:?}"
    );
    assert!(
        !has_error(&events),
        "no error on a legal rewire: {events:?}"
    );
    assert!(mock.connection().is_live());

    let state = mock.snapshot().as_ref().clone();
    let cursor = mock.client().last_seen_cursor().cloned().expect("cursor");
    (state, cursor)
}

#[test]
fn a_transaction_local_iteration_rewire_is_accepted_in_any_mutation_order() {
    // Catches: an over-strict effective-state validator that rejects a legal
    // rewire (removing the old iteration and upserting the candidate with the
    // new `iteration_ref`), or whose verdict depends on mutation order.
    // Oracle: DC-MP-COR1 acceptance rule + the reviewed fixture tuples; the
    // resulting state is checked with the independent reciprocal-ownership
    // probe, not with the production validator.
    let forward = run_rewire(false);
    let reversed = run_rewire(true);

    assert_eq!(
        forward.0, reversed.0,
        "the published state must not depend on mutation order"
    );
    assert_ne!(
        forward.1, reversed.1,
        "each order publishes its own transaction cursor"
    );

    let state = &forward.0;
    assert!(
        !state
            .iterations
            .contains_key(&IterationId::fixture("iteration/Task-184/3")),
        "the removed iteration must be gone"
    );
    assert_eq!(
        candidate(state, "candidate/e").iteration_ref,
        IterationId::fixture("iteration/Task-184/2"),
        "the moved candidate must point at the surviving iteration"
    );
    assert_eq!(
        task(state).iteration_ref,
        Some(IterationId::fixture("iteration/Task-184/2")),
        "the task must follow its candidate"
    );
    assert_reciprocal_iteration_ownership(state);
}

#[test]
fn a_rejected_transaction_never_partially_publishes_and_the_session_recovers() {
    // Catches: staged or partially applied publication — some of the rejected
    // transaction becoming observable, or the rejection poisoning the session
    // so that later valid transactions are no longer admitted.
    // Oracle: DC-MP-COR1 rejection contract + the reviewed fixture identities
    // (task best candidate `candidate/e`, `evaluation/e` revision 2 after the
    // scripted after-open deltas). The accepted transaction is the reviewed
    // `candidate/b` (revision 2).
    let mut mock = live_mock();
    let before = mock.snapshot();
    let cursor_before = mock.client().last_seen_cursor().cloned().expect("cursor");

    // Orphan `candidate/c` from the iteration that lists it, without touching
    // that iteration: admissible per mutation, inconsistent in effect.
    let mut orphaned = candidate(&before, "candidate/c");
    orphaned.iteration_ref = IterationId::fixture("iteration/Task-184/1");
    orphaned.projection_revision = Counter::new("4").expect("counter");

    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/cor1-orphan",
        "cursor/cor1-orphan",
        Some(cursor_before.clone()),
        vec![upsert("3", Projection::Candidate(orphaned))],
    )));
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(!delta_applied(&events));
    assert_rejected_without_side_effects(&mock, &before, &cursor_before);

    // The previously published state is still fully observable through the
    // public snapshot API, including the projection the transaction tried to
    // replace and projections it never touched.
    let visible = mock.snapshot();
    assert_eq!(
        task(&visible)
            .best_candidate_ref
            .as_ref()
            .expect("best candidate")
            .candidate_id,
        CandidateId::fixture("candidate/e"),
        "the pre-rejection best candidate must remain observable"
    );
    assert_eq!(
        visible
            .evaluations
            .get(&EvaluationId::fixture("evaluation/e"))
            .expect("evaluation/e")
            .projection_revision
            .as_str(),
        "2",
        "the after-open evaluation revision must remain observable"
    );
    let orphan = candidate(&visible, "candidate/c");
    assert_eq!(
        visible
            .candidates
            .get(&orphan.candidate_ref)
            .expect("candidate/c")
            .iteration_ref,
        IterationId::fixture("iteration/Task-184/2"),
        "the attempted re-point must not be observable"
    );

    // Bounded recovery returns the session to Live with the reviewed state.
    let events = mock.pump(1_000_000);
    assert!(mock.connection().is_live());
    assert!(!has_error(&events));
    assert_eq!(
        mock.snapshot().as_ref(),
        before.as_ref(),
        "the resync must restore exactly the reviewed state"
    );

    // The same session still admits and applies a later valid transaction.
    let cursor_after_resync = mock.client().last_seen_cursor().cloned().expect("cursor");
    let mut relabelled = candidate(&mock.snapshot(), "candidate/b");
    relabelled.display_label = "B (cor1)".to_owned();
    relabelled.projection_revision = Counter::new("3").expect("counter");
    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/cor1-legal-after-rejection",
        "cursor/cor1-legal",
        Some(cursor_after_resync),
        vec![upsert("2", Projection::Candidate(relabelled))],
    )));
    let events = mock.pump(0);
    assert!(
        delta_applied(&events),
        "a valid transaction after a rejection must still be admitted: {events:?}"
    );
    assert!(!has_error(&events));
    assert_eq!(
        mock.client().last_seen_cursor(),
        Some(&Cursor::fixture("cursor/cor1-legal"))
    );
    assert_eq!(
        candidate(&mock.snapshot(), "candidate/b").display_label,
        "B (cor1)"
    );
}
