//! Recovery semantics: duplicates, gaps, stale cursors, overflow, epoch
//! invalidation, session-generation cancellation and real scripted resync.

mod common;

use common::*;
use cos_client::*;
use desktop_protocol::{
    ClientFrame, ClientMessage, ClientWire, Counter, Cursor, DeltaTransaction, EpochId, ErrorCode,
    EvaluationId, IntegrityDigest, MAX_CONTROL_FRAME_BYTES, ObservationScopeRequest,
    OpenAuthorizedObservation, OpenObservation, Projection, ProjectionMutation, ReadRequestId,
    ResyncReason, SnapshotChunk, SnapshotId, TaskStatus, TransactionId, Version,
    integrity_digest_of,
};

fn transaction(
    id: &str,
    cursor: &str,
    predecessor: Option<Cursor>,
    mutations: Vec<ProjectionMutation>,
    epoch: &str,
) -> DeltaTransaction {
    DeltaTransaction {
        transaction_id: TransactionId::fixture(id),
        epoch: EpochId::fixture(epoch),
        cursor: Cursor::fixture(cursor),
        predecessor,
        mutation_count: mutations.len() as u32,
        integrity_digest: integrity_digest_of(&mutations).expect("digest"),
        mutations,
    }
}

#[test]
fn duplicate_transactions_are_ignored_and_conflicts_request_resync() {
    let mut scenario = FixtureScenario::from_file(fixture_path()).expect("scenario");
    let mut delta = {
        let step = scenario
            .steps
            .iter()
            .find(|step| step.id == "delta-best-candidate")
            .expect("step");
        step.actions[0].delta.clone().expect("delta")
    };
    delta.transaction_id = Some(TransactionId::fixture("transaction/best"));
    for step in &mut scenario.steps {
        if step.id == "delta-best-candidate" {
            step.actions[0].delta = Some(delta.clone());
        }
    }
    scenario.steps.push(FixtureStep {
        id: "duplicate-best".to_owned(),
        trigger: "manual".to_owned(),
        actions: vec![FixtureAction {
            delta: Some(delta.clone()),
            ..FixtureAction::default()
        }],
    });
    let mut conflicting = delta;
    match &mut conflicting.mutations[0] {
        ProjectionMutation::Upsert {
            projection: Projection::Task(task),
            ..
        } => task.title = "conflicting content".to_owned(),
        other => panic!("unexpected mutation {other:?}"),
    }
    scenario.steps.push(FixtureStep {
        id: "conflicting-best".to_owned(),
        trigger: "manual".to_owned(),
        actions: vec![FixtureAction {
            delta: Some(conflicting),
            ..FixtureAction::default()
        }],
    });

    let mut mock = MockCosClient::from_scenario(scenario).expect("mock");
    mock.connect(0).expect("connect");
    mock.pump(0);
    assert_eq!(mock.connection(), &ConnectionState::Live);

    mock.mock_mut()
        .deliver_step("duplicate-best")
        .expect("deliver duplicate");
    let events = mock.pump(0);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::DuplicateIgnored { .. }))
    );
    assert!(!has_error(&events));
    assert_eq!(mock.connection(), &ConnectionState::Live);

    mock.mock_mut()
        .deliver_step("conflicting-best")
        .expect("deliver conflict");
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
    assert!(mock.is_stale());

    // Real resync: the provider regenerates a complete snapshot from its
    // accumulated state, and the client swaps it in atomically.
    let events = mock.pump(1_000_000);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert!(!mock.is_stale());
    assert!(!has_error(&events));
    assert_eq!(
        mock.snapshot().task(&task_id()).expect("task").title,
        "新製品キービジュアル"
    );
}

#[test]
fn gaps_and_stale_cursors_require_a_full_resync() {
    let mut mock = live_mock();
    let cursor_before = mock.client().last_seen_cursor().cloned().expect("cursor");

    // Gap: the predecessor is not the last applied cursor.
    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/gap",
        "cursor/40",
        Some(Cursor::fixture("cursor/0")),
        Vec::new(),
        "epoch/1",
    )));
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::ResyncRequested { reason } if reason == &ResyncReason::ProtocolViolation
    )));
    assert_eq!(
        mock.client().last_seen_cursor(),
        Some(&cursor_before),
        "the last completely applied cursor must be preserved"
    );
    assert!(mock.is_stale());

    mock.pump(1_000_000);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    let recovered_cursor = mock.client().last_seen_cursor().cloned().expect("cursor");
    assert_ne!(recovered_cursor, cursor_before);

    // Stale cursor: a known but superseded position.
    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/stale",
        "cursor/41",
        Some(Cursor::fixture("cursor/1")),
        Vec::new(),
        "epoch/1",
    )));
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
}

#[test]
fn base_revision_and_digest_violations_are_rejected_atomically() {
    let mut mock = live_mock();
    let evaluation_ref = EvaluationId::fixture("evaluation/e");

    let mut evaluation = mock
        .snapshot()
        .evaluations
        .get(&evaluation_ref)
        .cloned()
        .expect("evaluation E");
    evaluation.projection_revision = Counter::new("3").expect("counter");
    let stale_base = vec![ProjectionMutation::Upsert {
        base_revision: Some(Counter::new("1").expect("counter")),
        projection: Projection::Evaluation(evaluation),
    }];
    let predecessor = mock.client().last_seen_cursor().cloned();
    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/stale-base",
        "cursor/42",
        predecessor,
        stale_base,
        "epoch/1",
    )));
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
    assert_eq!(
        mock.snapshot()
            .evaluations
            .get(&evaluation_ref)
            .expect("evaluation E")
            .projection_revision
            .as_str(),
        "2",
        "a rejected transaction must not be partially applied"
    );

    mock.pump(1_000_000);
    assert_eq!(mock.connection(), &ConnectionState::Live);

    // Digest mismatch: valid chain, wrong integrity declaration.
    let predecessor = mock.client().last_seen_cursor().cloned();
    let mut bad_digest = transaction(
        "transaction/bad-digest",
        "cursor/43",
        predecessor,
        Vec::new(),
        "epoch/1",
    );
    bad_digest.integrity_digest = IntegrityDigest::fixture("digest/not-the-content");
    mock.mock_mut().inject_raw(delta_frame(bad_digest));
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
}

#[test]
fn overflow_cancels_the_stream_preserves_cursor_and_recovers() {
    let mut mock = live_mock();
    let cursor_before = mock.client().last_seen_cursor().cloned().expect("cursor");
    for _ in 0..300 {
        mock.mock_mut().inject_raw(r#"{"kind":"PADDING"}"#);
    }
    let events = mock.pump(0);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::Error(ClientError::QueueOverflow)))
    );
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
    assert!(mock.is_stale());
    assert_eq!(mock.client().last_seen_cursor(), Some(&cursor_before));

    mock.pump(1_000_000);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert!(!mock.is_stale());
    assert_eq!(
        mock.snapshot().task(&task_id()).expect("task").task_status,
        TaskStatus::Active
    );
}

#[test]
fn repeated_overflow_reports_scope_incompatibility() {
    let mut mock = live_mock();
    for _ in 0..3 {
        for _ in 0..300 {
            mock.mock_mut().inject_raw(r#"{"kind":"PADDING"}"#);
        }
        mock.pump(0);
    }
    assert!(matches!(
        mock.connection(),
        ConnectionState::Incompatible { .. }
    ));
    let error = mock.last_error().expect("error");
    assert!(matches!(error, ClientError::QueueOverflow), "got {error:?}");
}

#[test]
fn session_generation_cancellation_ignores_late_chunks() {
    let mut mock = load_mock();
    mock.connect(0).expect("connect");
    mock.pump_bounded(0, 3); // hello acknowledgement + read grant + snapshot begin: staging started

    mock.disconnect();
    assert_eq!(mock.connection(), &ConnectionState::Disconnected);
    mock.connect(0).expect("reconnect");
    mock.pump(0);
    assert_eq!(mock.connection(), &ConnectionState::Live);

    // A late chunk from the cancelled generation is ignored silently.
    mock.mock_mut().inject_raw(server_frame(
        desktop_protocol::ServerMessage::SnapshotChunk(SnapshotChunk {
            snapshot_id: SnapshotId::fixture("snapshot/1"),
            part_index: 99,
            mutations: Vec::new(),
        }),
    ));
    let events = mock.pump(0);
    assert!(
        !has_error(&events),
        "late generation must not surface an error"
    );
    assert_eq!(mock.connection(), &ConnectionState::Live);

    // An unknown snapshot identity is still a protocol violation.
    mock.mock_mut().inject_raw(server_frame(
        desktop_protocol::ServerMessage::SnapshotChunk(SnapshotChunk {
            snapshot_id: SnapshotId::fixture("snapshot/999"),
            part_index: 0,
            mutations: Vec::new(),
        }),
    ));
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
}

#[test]
fn local_disconnect_preserves_the_task_and_reconnect_resynchronizes() {
    let mut mock = live_mock();
    let task = mock.snapshot().task(&task_id()).cloned().expect("task");
    mock.disconnect();
    assert_eq!(mock.connection(), &ConnectionState::Disconnected);
    assert_eq!(mock.snapshot().task(&task_id()), Some(&task));
    assert_eq!(task.task_status, TaskStatus::Active);

    mock.connect(0).expect("reconnect");
    mock.pump(0);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    let task_after = mock.snapshot().task(&task_id()).cloned().expect("task");
    assert_eq!(task_after.task_status, TaskStatus::Active);
    assert_eq!(task_after.title, task.title);
}

#[test]
fn transport_loss_resumes_from_the_last_applied_cursor() {
    let mut mock = load_mock();
    mock.connect(0).expect("connect");
    // hello acknowledgement + read grant + snapshot begin + 4 chunks + snapshot
    // end: the snapshot lands, both deltas stay queued.
    mock.pump_bounded(0, 8);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    let cursor = mock.client().last_seen_cursor().cloned().expect("cursor");
    assert_eq!(
        mock.snapshot()
            .task(&task_id())
            .expect("task")
            .projection_revision
            .as_str(),
        "7"
    );

    mock.mock_mut().simulate_disconnect();
    let events = mock.pump(0);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::Error(ClientError::Transport(_))))
    );
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));

    let events = mock.pump(1_000_000);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::DeltaApplied { .. })),
        "the provider replays the missing transaction"
    );
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert_eq!(
        mock.snapshot()
            .task(&task_id())
            .expect("task")
            .projection_revision
            .as_str(),
        "8"
    );
    assert_ne!(mock.client().last_seen_cursor(), Some(&cursor));
}

#[test]
fn pruned_history_requires_a_full_resync() {
    let mut scenario = FixtureScenario::from_file(fixture_path()).expect("scenario");
    scenario.journal_capacity = Some(1);
    let mut mock = MockCosClient::from_scenario(scenario).expect("mock");
    mock.connect(0).expect("connect");
    // hello acknowledgement + read grant + snapshot begin + 4 chunks + snapshot end.
    mock.pump_bounded(0, 8);
    assert_eq!(mock.connection(), &ConnectionState::Live);

    mock.mock_mut().simulate_disconnect();
    mock.pump(0);
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));

    let events = mock.pump(1_000_000);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::ResyncRequested { reason } if reason == &ResyncReason::ExpiredCursor
    )));
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert_eq!(
        mock.snapshot()
            .task(&task_id())
            .expect("task")
            .projection_revision
            .as_str(),
        "8",
        "the resynced snapshot reflects the accumulated state"
    );
}

#[test]
fn unknown_events_follow_informational_and_state_changing_policies() {
    let mut mock = live_mock();
    let informational = serde_json::json!({
        "kind": "FRAME",
        "protocol": { "major": 2, "minor": 0 },
        "schema": { "major": 2, "minor": 0 },
        "message": {
            "kind": "FUTURE_INFO",
            "affects_state": false,
            "cursor": "fixture:cursor/30"
        }
    });
    mock.mock_mut().inject_raw(informational.to_string());
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::InformationalEventSkipped { kind } if kind == "FUTURE_INFO"
    )));
    assert_eq!(
        mock.client()
            .last_seen_cursor()
            .map(|cursor| cursor.as_str()),
        Some("fixture:cursor/30"),
        "skipping an informational event preserves cursor continuity"
    );

    // The next transaction chains from the skipped position and applies cleanly.
    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/after-info",
        "cursor/31",
        Some(Cursor::fixture("cursor/30")),
        Vec::new(),
        "epoch/1",
    )));
    let events = mock.pump(0);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
    );
    assert!(!has_error(&events));

    // An unknown state-changing event requires resync.
    let state_changing = serde_json::json!({
        "kind": "FRAME",
        "protocol": { "major": 2, "minor": 0 },
        "schema": { "major": 2, "minor": 0 },
        "message": { "kind": "FUTURE_STATE", "cursor": "fixture:cursor/32" }
    });
    mock.mock_mut().inject_raw(state_changing.to_string());
    let events = mock.pump(0);
    assert!(has_error(&events));
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::ResyncRequested { reason } if reason == &ResyncReason::UnsupportedStateEvent
    )));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
}

#[test]
fn legacy_1_0_only_peer_is_refused_before_any_data_or_command() {
    let mut scenario = FixtureScenario::from_file(fixture_path()).expect("scenario");
    // The 2.0 client must refuse a peer that only speaks the replaced 1.0 pair.
    scenario.hello.protocol = vec![desktop_protocol::VersionRange::new(1, 0, 0)];
    scenario.hello.schema = vec![desktop_protocol::VersionRange::new(1, 0, 0)];
    let mut mock = MockCosClient::from_scenario(scenario).expect("mock");
    mock.connect(0).expect("connect");
    let events = mock.pump(0);
    assert!(
        events.iter().any(|event| matches!(
            event,
            ClientEvent::Error(ClientError::VersionIncompatible(_))
        )),
        "a 1.0-only peer is refused explicitly"
    );
    assert!(matches!(
        mock.connection(),
        ConnectionState::Incompatible { .. }
    ));
    assert_eq!(
        mock.mock().sent_frames().len(),
        1,
        "only the hello may be sent; no observation request or command follows"
    );
}

#[test]
fn oversized_frames_are_rejected_before_parsing() {
    let mut mock = live_mock();
    let oversized = "x".repeat(MAX_CONTROL_FRAME_BYTES as usize + 1);
    mock.mock_mut().inject_raw(oversized);
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::Error(ClientError::MessageTooLarge { .. })
    )));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
}

#[test]
fn negotiation_rejection_is_terminal_and_side_effect_free() {
    let mut scenario = FixtureScenario::from_file(fixture_path()).expect("scenario");
    scenario.hello.protocol = vec![desktop_protocol::VersionRange::new(3, 0, 0)];
    scenario.hello.schema = vec![desktop_protocol::VersionRange::new(3, 0, 0)];
    let mut mock = MockCosClient::from_scenario(scenario).expect("mock");
    mock.connect(0).expect("connect");
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::Error(ClientError::VersionIncompatible(_))
    )));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Incompatible { .. }
    ));
    assert_eq!(
        mock.mock().sent_frames().len(),
        1,
        "only the hello may be sent; no snapshot or command follows"
    );
    assert_eq!(mock.mock().observation_count(), 0);
    assert!(mock.snapshot().tasks.is_empty());
}

#[test]
fn epoch_changes_invalidate_the_session_and_resynchronize() {
    let mut mock = live_mock();

    // A delta from another epoch invalidates the live session.
    let predecessor = mock.client().last_seen_cursor().cloned();
    mock.mock_mut().inject_raw(delta_frame(transaction(
        "transaction/epoch",
        "cursor/60",
        predecessor,
        Vec::new(),
        "epoch/2",
    )));
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::ResyncRequested { reason } if reason == &ResyncReason::EpochChanged
    )));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
    mock.pump(1_000_000);
    assert_eq!(mock.connection(), &ConnectionState::Live);

    // A stale epoch on a resume request is refused by the provider, and the
    // client falls back to the atomic full-snapshot path.
    let stale_open = ClientWire::Frame(ClientFrame {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
        message: ClientMessage::OpenObservation(OpenAuthorizedObservation {
            request_id: ReadRequestId::new("read-req/stale-epoch").expect("valid request identity"),
            observation: OpenObservation {
                scope: ObservationScopeRequest {
                    task_ids: vec![task_id()],
                    filters: Vec::new(),
                },
                scope_id: None,
                last_seen_cursor: mock.client().last_seen_cursor().cloned(),
                epoch: Some(EpochId::fixture("epoch/0")),
            },
            retained_commands: Vec::new(),
        }),
    });
    mock.mock_mut()
        .inject_client_frame(serde_json::to_string(&stale_open).expect("frame"));
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::ResyncRequested { reason } if reason == &ResyncReason::EpochChanged
    )));
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert_eq!(
        mock.snapshot().task(&task_id()).expect("task").task_status,
        TaskStatus::Active
    );
}

#[test]
fn unauthorized_errors_stop_automatic_recovery() {
    let mut mock = live_mock();
    mock.mock_mut()
        .inject_raw(server_frame(desktop_protocol::ServerMessage::Error(
            desktop_protocol::ProtocolError {
                code: ErrorCode::Forbidden,
                message: "principal is not authorized for this task".to_owned(),
                correlation_id: None,
                command_id: None,
                scope_id: None,
                retry_after_ms: None,
                field_errors: Vec::new(),
                current_revision: None,
                resync_reason: None,
            },
        )));
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::Error(ClientError::Server(error)) if error.code == ErrorCode::Forbidden
    )));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Unauthorized { .. }
    ));
    let before = mock.snapshot();
    mock.pump(1_000_000);
    assert!(matches!(
        mock.connection(),
        ConnectionState::Unauthorized { .. }
    ));
    assert_eq!(
        *mock.snapshot(),
        *before,
        "no silent recovery after authorization loss"
    );
}
