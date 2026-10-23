//! End-to-end fixture observation: atomic snapshot, scripted deltas and the
//! authoritative projections app-core renders.

mod common;

use common::*;
use cos_client::*;
use desktop_protocol::{
    ArtifactId, Availability, CandidateRef, ClientFrame, ClientMessage, ClientWire,
    CommandOperation, Counter, Cursor, DeltaTransaction, EpochId, EvaluationDecision, EvaluationId,
    EvidenceStatus, IterationId, Projection, ProjectionMutation, SnapshotChunk, SnapshotId,
    SupportState, TaskStatus, TransactionId, Version, integrity_digest_of,
};

#[test]
fn one_pump_drains_initialization_into_authoritative_state() {
    let mut mock = load_mock();
    assert_eq!(
        mock.source(),
        ClientSource::Fixture {
            scenario_id: "task-184-refine".to_owned()
        }
    );
    assert!(mock.is_stale());
    mock.connect(0).expect("connect");
    let events = mock.pump(0);

    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert!(!mock.is_stale());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::SnapshotApplied { .. })),
        "snapshot must be applied in the initialization pump"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
            .count(),
        2,
        "both scripted after-open deltas must land in the same pump"
    );
    assert!(mock.last_error().is_none());

    let snapshot = mock.snapshot();
    let task = snapshot.task(&task_id()).expect("task projection");
    assert_eq!(task.display_id, "Task-184");
    assert_eq!(task.title, "新製品キービジュアル");
    assert_eq!(task.task_status, TaskStatus::Active);
    assert_eq!(task.inner_phase.as_str(), "REFINE");
    assert_eq!(task.iteration_index, Some(3));
    assert_eq!(task.projection_revision.as_str(), "8");
    assert_eq!(task.formal_outcome_ref, None);
    assert!(
        snapshot.outcome_for_task(&task_id()).is_none(),
        "an absent outcome is not a failed outcome"
    );

    let primary = task_id();
    let labels: Vec<&str> = snapshot
        .candidates_for_task(&primary)
        .map(|candidate| candidate.display_label.as_str())
        .collect();
    assert_eq!(labels, vec!["A", "B", "C", "D", "E"]);
    let best = task.best_candidate_ref.as_ref().expect("best candidate");
    assert_eq!(best.candidate_id.as_str(), "fixture:candidate/e");

    let evaluation = snapshot
        .evaluations
        .get(&EvaluationId::fixture("evaluation/e"))
        .expect("evaluation E");
    assert_eq!(evaluation.projection_revision.as_str(), "2");
    assert_eq!(evaluation.decision, EvaluationDecision::Pass);
    assert_eq!(evaluation.axes[0].value, Some(89.0));
    assert_eq!(evaluation.axes[0].unit.as_deref(), Some("score_0_100"));
    assert_eq!(
        evaluation.evidence_refs[0].status,
        EvidenceStatus::Available
    );
    assert_eq!(evaluation.confidence, Some(0.83));

    let abstained = snapshot
        .evaluations
        .get(&EvaluationId::fixture("evaluation/c"))
        .expect("evaluation C");
    assert_eq!(abstained.decision, EvaluationDecision::Abstain);
    assert_eq!(abstained.axes[0].value, None, "a missing score is not zero");

    let iteration = snapshot
        .iterations
        .get(&IterationId::fixture("iteration/Task-184/3"))
        .expect("iteration 3");
    assert_eq!(iteration.index, 3);
    assert_eq!(iteration.refinement_refs.len(), 1);

    let graph = snapshot.graphs.values().next().expect("graph");
    let node_labels: Vec<&str> = graph.nodes.iter().map(|node| node.label.as_str()).collect();
    assert_eq!(
        node_labels,
        vec![
            "Goal",
            "Context",
            "Capability",
            "Generation",
            "Evaluation",
            "Select",
            "Feedback",
            "Refine",
            "Outcome"
        ]
    );
    assert_eq!(graph.edges.len(), 8);
    assert_eq!(graph.graph_ref.version, 3);

    let capabilities = snapshot.capabilities.values().next().expect("capabilities");
    assert_eq!(
        capabilities.command_support(&CommandOperation::StopTask),
        Some(&SupportState::Supported)
    );
    assert_eq!(
        capabilities.command_support(&CommandOperation::CreateTask),
        Some(&SupportState::Unsupported)
    );
    assert_eq!(capabilities.availability, SupportState::Supported);

    let artifact = snapshot
        .artifacts
        .get(&ArtifactId::fixture("artifact/e"))
        .expect("artifact E");
    assert_eq!(artifact.availability, Availability::Available);
    assert_eq!(artifact.descriptor_refs.len(), 2);

    assert_eq!(
        mock.client().session_epoch().expect("epoch").as_str(),
        "fixture:epoch/1"
    );
    assert!(mock.client().last_seen_cursor().is_some());
    assert_eq!(
        mock.client().scope().expect("scope").scope_id.as_str(),
        "fixture:scope/task-184"
    );
}

#[test]
fn authoritative_best_and_outcome_never_follow_local_or_candidate_order() {
    let mut mock = live_mock();
    let task = mock.snapshot().task(&task_id()).cloned().expect("task");
    let best_before = task.best_candidate_ref.clone().expect("best");
    assert_eq!(best_before.candidate_id.as_str(), "fixture:candidate/e");

    // A server-side delta introduces a brand-new candidate with a higher score.
    // The client must apply it without deriving a new best-so-far or outcome.
    let artifact = Projection::Artifact(desktop_protocol::ArtifactSnapshot {
        artifact_ref: ArtifactId::fixture("artifact/f"),
        artifact_digest: desktop_protocol::ArtifactDigest::fixture("digest/artifact-f"),
        media_type: Some("image/png".to_owned()),
        availability: Availability::Available,
        descriptor_refs: Vec::new(),
        verification: None,
        projection_revision: Counter::new("1").expect("counter"),
    });
    let candidate = Projection::Candidate(desktop_protocol::CandidateSnapshot {
        candidate_ref: CandidateRef {
            task_id: task_id(),
            run_id: desktop_protocol::RunId::fixture("run/Task-184/1"),
            iteration: 3,
            candidate_id: desktop_protocol::CandidateId::fixture("candidate/f"),
            artifact_ref: ArtifactId::fixture("artifact/f"),
        },
        task_ref: task_id(),
        iteration_ref: IterationId::fixture("iteration/Task-184/3"),
        artifact_refs: vec![ArtifactId::fixture("artifact/f")],
        evaluation_refs: Vec::new(),
        display_label: "F".to_owned(),
        projection_revision: Counter::new("1").expect("counter"),
    });
    let evaluation = Projection::Evaluation(desktop_protocol::EvaluationSnapshot {
        evaluation_ref: EvaluationId::fixture("evaluation/f"),
        candidate_ref: match &candidate {
            Projection::Candidate(snapshot) => snapshot.candidate_ref.clone(),
            _ => unreachable!(),
        },
        artifact_refs: vec![ArtifactId::fixture("artifact/f")],
        decision: EvaluationDecision::Pass,
        axes: vec![desktop_protocol::NamedScore {
            name: "visual_quality".to_owned(),
            unit: Some("score_0_100".to_owned()),
            value: Some(99.0),
        }],
        evidence_refs: Vec::new(),
        authority: desktop_protocol::AuthorityLabel::new("HARNESS").expect("label"),
        confidence: Some(0.9),
        projection_revision: Counter::new("1").expect("counter"),
    });
    let mutations = vec![
        ProjectionMutation::Upsert {
            base_revision: None,
            projection: artifact,
        },
        ProjectionMutation::Upsert {
            base_revision: None,
            projection: candidate,
        },
        ProjectionMutation::Upsert {
            base_revision: None,
            projection: evaluation,
        },
    ];
    let transaction = DeltaTransaction {
        transaction_id: TransactionId::fixture("transaction/candidate-f"),
        epoch: EpochId::fixture("epoch/1"),
        cursor: Cursor::fixture("cursor/50"),
        predecessor: mock.client().last_seen_cursor().cloned(),
        mutation_count: mutations.len() as u32,
        integrity_digest: integrity_digest_of(&mutations).expect("digest"),
        mutations,
    };
    mock.mock_mut().inject_raw(delta_frame(transaction));
    let events = mock.pump(0);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
    );
    assert!(!has_error(&events));

    let snapshot = mock.snapshot();
    assert_eq!(
        snapshot.candidates_for_task(&task_id()).count(),
        6,
        "the new candidate is authoritative state"
    );
    let task_after = snapshot.task(&task_id()).expect("task");
    assert_eq!(
        task_after.best_candidate_ref, task.best_candidate_ref,
        "best-so-far is never derived from candidate content or scores"
    );
    assert_eq!(task_after.formal_outcome_ref, None);
    assert!(snapshot.outcome_for_task(&task_id()).is_none());
}

#[test]
fn display_labels_and_fixture_identities_stay_distinct() {
    let mock = live_mock();
    let snapshot = mock.snapshot();
    let candidate = snapshot
        .candidates
        .get(&CandidateRef {
            task_id: task_id(),
            run_id: desktop_protocol::RunId::fixture("run/Task-184/1"),
            iteration: 3,
            candidate_id: desktop_protocol::CandidateId::fixture("candidate/e"),
            artifact_ref: ArtifactId::fixture("artifact/e"),
        })
        .expect("candidate E");
    assert_eq!(candidate.display_label, "E");
    assert_ne!(
        candidate.display_label,
        candidate.candidate_ref.candidate_id.as_str()
    );
    assert!(candidate.candidate_ref.candidate_id.is_fixture());
    assert!(candidate.task_ref.is_fixture());
}

#[test]
fn fixture_namespace_is_enforced_at_load() {
    let text = std::fs::read_to_string(fixture_path()).expect("fixture file");
    let real_identity = text.replace("\"fixture:task/Task-184\"", "\"task/Task-184\"");
    let error =
        MockCosClient::from_fixture_str(&real_identity).expect_err("real identity rejected");
    assert!(matches!(error, FixtureError::Invalid(_)), "got {error:?}");

    let not_fixture = text.replace("\"fixture\": true", "\"fixture\": false");
    let error = MockCosClient::from_fixture_str(&not_fixture).expect_err("fixture mode required");
    assert!(
        matches!(error, FixtureError::NotFixture(_)),
        "got {error:?}"
    );
}

#[test]
fn fixture_loading_failure_is_explicit() {
    let error = MockCosClient::from_fixture_file("does/not/exist.json").expect_err("missing file");
    assert!(matches!(error, FixtureError::Io { .. }), "got {error:?}");
}

#[test]
fn client_frames_carry_the_negotiated_versions() {
    let mut mock = live_mock();
    let sent = mock.mock().sent_frames();
    let hello: ClientWire =
        serde_json::from_slice(sent.first().expect("hello frame")).expect("hello parses");
    assert!(
        matches!(hello, ClientWire::Hello(_)),
        "first frame is the hello"
    );

    let open: ClientWire =
        serde_json::from_slice(sent.get(1).expect("open frame")).expect("open parses");
    match open {
        ClientWire::Frame(ClientFrame {
            protocol,
            schema,
            message,
        }) => {
            assert_eq!(protocol, Version::new(2, 0));
            assert_eq!(schema, Version::new(2, 0));
            assert!(matches!(message, ClientMessage::OpenObservation(_)));
        }
        other => panic!("unexpected wire {other:?}"),
    }

    mock.stop_task(&task_id()).expect("stop sends");
    let last = mock.mock().sent_frames().last().expect("command frame");
    let command: ClientWire = serde_json::from_slice(last).expect("command parses");
    match command {
        ClientWire::Frame(ClientFrame { message, .. }) => match message {
            ClientMessage::Command(envelope) => {
                assert!(!envelope.command_id.as_str().is_empty());
            }
            other => panic!("unexpected message {other:?}"),
        },
        other => panic!("unexpected wire {other:?}"),
    }

    let chunk = server_frame(desktop_protocol::ServerMessage::SnapshotChunk(
        SnapshotChunk {
            snapshot_id: SnapshotId::fixture("snapshot/1"),
            part_index: 0,
            mutations: Vec::new(),
        },
    ));
    assert!(chunk.contains("SNAPSHOT_CHUNK"));
}

/// A delta that lands after the session is already live must mark the report
/// changed: the owner publishes only when `PumpReport::changed` says the client
/// changed, so an unmarked delta would leave the previous publication
/// installed. A re-delivered identical transaction is a duplicate and must not
/// publish either.
#[test]
fn a_delta_after_the_initial_pump_marks_the_report_changed() {
    let mut mock = live_mock();
    let idle = mock.pump_step(0, 64);
    assert!(!idle.changed, "an idle live pump reports no change");

    let mut task = mock
        .snapshot()
        .task(&task_id())
        .cloned()
        .expect("fixture task");
    let base = task.projection_revision.clone();
    task.title = "delta after publication".to_owned();
    task.projection_revision = Counter::new("9").expect("counter");
    let mutations = vec![ProjectionMutation::Upsert {
        base_revision: Some(base),
        projection: Projection::Task(task),
    }];
    let transaction = DeltaTransaction {
        transaction_id: TransactionId::fixture("transaction/after-open"),
        epoch: EpochId::fixture("epoch/1"),
        cursor: Cursor::fixture("cursor/after-open"),
        predecessor: mock.client().last_seen_cursor().cloned(),
        mutation_count: mutations.len() as u32,
        integrity_digest: integrity_digest_of(&mutations).expect("digest"),
        mutations,
    };
    let frame = delta_frame(transaction);
    mock.mock_mut().inject_raw(frame.clone());
    let report = mock.pump_step(0, 64);
    assert!(
        report.changed,
        "an applied delta marks the report changed: {:?}",
        report.events
    );
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, ClientEvent::DeltaApplied { .. })),
        "the delta applies: {:?}",
        report.events
    );
    assert_eq!(
        mock.snapshot().task(&task_id()).expect("task").title,
        "delta after publication"
    );

    mock.mock_mut().inject_raw(frame);
    let duplicate = mock.pump_step(0, 64);
    assert!(
        duplicate
            .events
            .iter()
            .any(|event| matches!(event, ClientEvent::DuplicateIgnored { .. })),
        "the identical replay is a duplicate"
    );
    assert!(
        !duplicate.changed,
        "an ignored duplicate must not publish a new version"
    );
}
