//! Command honesty: acceptance is not completion, unsupported is explicit and
//! lost responses stay unknown.

mod common;

use common::*;
use cos_client::*;
use desktop_protocol::{
    AuthorizedReceiptLookup, ClientFrame, ClientMessage, ClientWire, CommandEnvelope, CommandId,
    CommandPayload, CommandTarget, ErrorCode, FormalStatus, ReceiptReadId, ReuseTaskCommand,
    StopTaskCommand, TaskStatus, Version,
};

fn stop_envelope(command_id: CommandId) -> CommandEnvelope {
    CommandEnvelope {
        command_id,
        target: CommandTarget::Task { task_id: task_id() },
        expected_revision: None,
        epoch: None,
        payload: CommandPayload::StopTask(StopTaskCommand {
            task_id: task_id(),
            reason: None,
        }),
    }
}

#[test]
fn stop_acceptance_is_not_stop_completion() {
    let mut mock = live_mock();
    let command_id = mock.stop_task(&task_id()).expect("stop sends");
    assert!(matches!(
        retained_state(&mock, &command_id),
        Some(CommandState::Pending { .. })
    ));

    // Only the acknowledgement frame is processed: the task must still be ACTIVE.
    let events = mock.pump_bounded(0, 1);
    assert!(matches!(
        retained_state(&mock, &command_id),
        Some(CommandState::Accepted(receipt)) if receipt.task_id.as_ref() == Some(&task_id())
    ));
    assert!(!has_error(&events));
    let task = mock.snapshot().task(&task_id()).cloned().expect("task");
    assert_eq!(
        task.task_status,
        TaskStatus::Active,
        "acceptance must not complete the task"
    );
    assert!(mock.snapshot().outcome_for_task(&task_id()).is_none());

    // The separately scripted canonical projection arrives later.
    mock.pump(0);
    let task = mock.snapshot().task(&task_id()).cloned().expect("task");
    assert_eq!(task.task_status, TaskStatus::Cancelled);
    assert_eq!(
        task.formal_outcome_ref.as_ref().map(|id| id.as_str()),
        Some("fixture:outcome/Task-184")
    );
    let outcome = mock
        .snapshot()
        .outcome_for_task(&task_id())
        .cloned()
        .expect("outcome");
    assert_eq!(outcome.formal_status, FormalStatus::Cancelled);
    assert_eq!(
        outcome.stop_reason.as_ref().map(|reason| reason.as_str()),
        Some("HUMAN_STOP")
    );
    match retained_state(&mock, &command_id) {
        Some(CommandState::CanonicalReflected(receipt)) => {
            assert_eq!(receipt.effect_refs.len(), 1);
            assert_eq!(
                receipt.effect_refs[0].id.as_str(),
                "fixture:outcome/Task-184"
            );
        }
        other => panic!("expected canonical reflection, got {other:?}"),
    }
}

#[test]
fn reuse_is_explicitly_unsupported_and_not_executed() {
    let mut mock = live_mock();
    let before = mock.snapshot();
    let command_id = mock
        .send_command(CommandEnvelope {
            command_id: CommandId::generate(),
            target: CommandTarget::Task { task_id: task_id() },
            expected_revision: None,
            epoch: None,
            payload: CommandPayload::ReuseTask(ReuseTaskCommand {
                source_task_id: task_id(),
            }),
        })
        .expect("reuse sends");
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::Error(ClientError::Server(error))
            if error.code == ErrorCode::UnsupportedCommand
    )));
    assert!(matches!(
        retained_state(&mock, &command_id),
        Some(CommandState::Unsupported { error }) if error.code == ErrorCode::UnsupportedCommand
    ));
    assert_eq!(
        *mock.snapshot(),
        *before,
        "unsupported command changed no state"
    );
}

#[test]
fn unknown_command_kinds_are_rejected_never_executed() {
    let mut mock = live_mock();
    let before = mock.snapshot();
    let raw = serde_json::json!({
        "kind": "FRAME",
        "protocol": { "major": 2, "minor": 0 },
        "schema": { "major": 2, "minor": 0 },
        "message": {
            "kind": "COMMAND",
            "command_id": "fixture:command/unknown-kind",
            "target": { "target": "TASK", "task_id": "fixture:task/Task-184" },
            "payload": { "kind": "FUTURE_COMMAND", "detail": "not implemented" }
        }
    });
    mock.mock_mut().inject_client_frame(raw.to_string());
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::Error(ClientError::Server(error))
            if error.code == ErrorCode::UnsupportedCommand
    )));
    assert_eq!(*mock.snapshot(), *before);
    assert_eq!(mock.connection(), &ConnectionState::Live);
}

#[test]
fn identical_command_id_replays_the_stored_response() {
    let mut mock = live_mock();
    let envelope = stop_envelope(CommandId::fixture("command/retry"));
    let command_id = mock
        .client_mut()
        .prepare_command(envelope)
        .expect("prepare");
    mock.client_mut()
        .send_prepared(&command_id)
        .expect("first send");
    let first = mock.pump(0);
    assert!(
        first
            .iter()
            .any(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
    );
    assert!(matches!(
        retained_state(&mock, &command_id),
        Some(CommandState::CanonicalReflected(_))
    ));

    // Retrying the identical id/envelope is allowed by the protocol; the server
    // replays the stored response and the duplicate transaction is ignored.
    mock.client_mut()
        .send_prepared(&command_id)
        .expect("identical retry");
    let second = mock.pump(0);
    assert!(
        second
            .iter()
            .any(|event| matches!(event, ClientEvent::DuplicateIgnored { .. }))
    );
    assert!(!has_error(&second));
    assert!(matches!(
        retained_state(&mock, &command_id),
        Some(CommandState::CanonicalReflected(_))
    ));
}

#[test]
fn reusing_a_command_id_with_different_content_is_rejected() {
    let mut mock = live_mock();
    let envelope = stop_envelope(CommandId::fixture("command/conflict"));
    let command_id = mock
        .client_mut()
        .prepare_command(envelope)
        .expect("prepare");
    mock.client_mut().send_prepared(&command_id).expect("send");
    mock.pump(0);

    let mut conflicting = stop_envelope(CommandId::fixture("command/conflict"));
    conflicting.payload = CommandPayload::StopTask(StopTaskCommand {
        task_id: task_id(),
        reason: Some("different content".to_owned()),
    });
    let frame = serde_json::to_string(&ClientWire::Frame(ClientFrame {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
        message: ClientMessage::Command(conflicting),
    }))
    .expect("frame serializes");
    mock.mock_mut().inject_client_frame(frame);
    let events = mock.pump(0);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::Error(ClientError::Server(error))
            if error.code == ErrorCode::CommandIdConflict
    )));
    assert!(matches!(
        retained_state(&mock, &command_id),
        Some(CommandState::Failed { error }) if error.code == ErrorCode::CommandIdConflict
    ));
}

#[test]
fn lost_responses_stay_unknown_instead_of_being_resolved() {
    let mut scenario = FixtureScenario::from_file(fixture_path()).expect("scenario");
    scenario
        .steps
        .retain(|step| !step.trigger.starts_with("on_command:STOP_TASK"));
    scenario.commands.insert(
        "STOP_TASK".to_owned(),
        CommandPolicy {
            default: CommandDefault::Silent,
        },
    );
    let mut mock = MockCosClient::from_scenario(scenario).expect("mock");
    mock.connect(0).expect("connect");
    mock.pump(0);

    // Timeout path.
    let timed_out = mock.stop_task(&task_id()).expect("stop sends");
    mock.pump(0);
    assert!(matches!(
        retained_state(&mock, &timed_out),
        Some(CommandState::Pending { .. })
    ));
    let events = mock.pump(31_000);
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::CommandUpdated {
            status: CommandState::ResultUnknown { .. },
            ..
        }
    )));
    assert!(matches!(
        retained_state(&mock, &timed_out),
        Some(CommandState::ResultUnknown { .. })
    ));

    // An authorized lookup reports Allowed/no receipt; the command stays unknown.
    let stamp = mock.mock().active_stamp().expect("authorized binding");
    mock.lookup_receipt_authorized(AuthorizedReceiptLookup {
        lookup_id: ReceiptReadId::new("receipt-read/unknown-1").expect("valid lookup identity"),
        stamp,
        command_id: timed_out.clone(),
    })
    .expect("lookup sends");
    mock.pump(31_001);
    assert!(matches!(
        retained_state(&mock, &timed_out),
        Some(CommandState::ResultUnknown { .. })
    ));

    // Disconnect path while pending.
    let pending = mock.stop_task(&task_id()).expect("stop sends");
    mock.pump(31_002);
    let events = mock.disconnect();
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::CommandUpdated { command_id, status: CommandState::ResultUnknown { reason, .. } }
            if command_id == &pending && reason.contains("disconnect")
    )));
    assert_eq!(mock.connection(), &ConnectionState::Disconnected);

    // The task itself is untouched by any of this.
    assert_eq!(
        mock.snapshot().task(&task_id()).expect("task").task_status,
        TaskStatus::Active
    );
}

#[test]
fn commands_require_a_live_session() {
    let mut mock = load_mock();
    let error = mock.stop_task(&task_id()).expect_err("not connected");
    assert!(
        matches!(error, ClientError::NotConnected(_)),
        "got {error:?}"
    );
}
