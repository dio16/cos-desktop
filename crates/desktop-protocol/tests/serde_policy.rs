//! Serde, vocabulary and bounds policy tests for the wire contract.

use desktop_protocol::{
    CandidateRef, CommandEnvelope, CommandId, CommandPayload, CommandTarget,
    CommandValidationError, Counter, DeltaTransaction, Limits, MAX_CONTROL_FRAME_BYTES,
    MAX_QUEUE_FRAMES, MAX_STAGED_BUNDLE_BYTES, Projection, ProjectionMutation, ServerMessage,
    ServerWire, StopTaskCommand, TaskId, TaskSnapshot, TaskStatus, UnknownMessage, Version,
    canonical_json, integrity_digest_of,
};

const TASK_JSON: &str = r#"{
  "task_ref": "fixture:task/Task-184",
  "display_id": "Task-184",
  "title": "新製品キービジュアル",
  "task_status": "ACTIVE",
  "inner_phase": "REFINE",
  "iteration_ref": "fixture:iteration/Task-184/3",
  "iteration_index": 3,
  "best_candidate_ref": {
    "task_id": "fixture:task/Task-184",
    "run_id": "fixture:run/Task-184/1",
    "iteration": 3,
    "candidate_id": "fixture:candidate/e",
    "artifact_ref": "fixture:artifact/e"
  },
  "formal_outcome_ref": null,
  "accepted_brief_ref": null,
  "capabilities_ref": "fixture:capabilities/Task-184",
  "projection_revision": "7"
}"#;

#[test]
fn task_snapshot_round_trips_and_keeps_axes_independent() {
    let task: TaskSnapshot = serde_json::from_str(TASK_JSON).expect("golden task parses");
    assert_eq!(task.display_id, "Task-184");
    assert_eq!(task.title, "新製品キービジュアル");
    assert_eq!(task.task_status, TaskStatus::Active);
    assert_eq!(task.inner_phase.as_str(), "REFINE");
    assert_eq!(task.iteration_index, Some(3));
    assert_eq!(task.task_ref.as_str(), "fixture:task/Task-184");
    assert_ne!(task.task_ref.as_str(), task.display_id);
    assert!(task.formal_outcome_ref.is_none());
    assert_eq!(task.projection_revision.as_str(), "7");

    let encoded = serde_json::to_string(&task).expect("task serializes");
    let reparsed: TaskSnapshot = serde_json::from_str(&encoded).expect("task reparses");
    assert_eq!(reparsed, task);
}

#[test]
fn integer_counters_stay_decimal_strings() {
    let task: TaskSnapshot = serde_json::from_str(TASK_JSON).expect("golden task parses");
    let value: serde_json::Value = serde_json::to_value(&task).expect("task serializes");
    assert_eq!(
        value["projection_revision"],
        serde_json::Value::String("7".to_owned())
    );
    assert!(Counter::new("1.5").is_err());
    assert!(Counter::new("-1").is_err());
    assert!(Counter::new("").is_err());
    assert_eq!(Counter::from_u128(42).as_str(), "42");
    assert_eq!(Counter::new("9").expect("valid").to_u128(), Some(9));
}

#[test]
fn unknown_enum_values_are_preserved_and_disable_actions() {
    let status = TaskStatus::from_wire("PAUSED");
    assert_eq!(status, TaskStatus::Unknown("PAUSED".to_owned()));
    assert!(!status.is_known());
    assert!(!status.is_terminal());
    assert!(!status.is_active());
    assert_eq!(status.as_str(), "PAUSED");

    let encoded = serde_json::to_string(&status).expect("status serializes");
    assert_eq!(encoded, "\"PAUSED\"");
    let decoded: TaskStatus = serde_json::from_str(&encoded).expect("status parses");
    assert_eq!(decoded, status);

    let json = TASK_JSON.replace("\"ACTIVE\"", "\"PAUSED\"");
    let task: TaskSnapshot = serde_json::from_str(&json).expect("unknown status parses");
    assert_eq!(task.task_status, TaskStatus::Unknown("PAUSED".to_owned()));
}

#[test]
fn unknown_fields_are_ignored_but_required_fields_are_enforced() {
    let with_extra = TASK_JSON.replace(
        "\"projection_revision\": \"7\"",
        "\"projection_revision\": \"7\", \"future_field\": {\"nested\": true}",
    );
    let task: TaskSnapshot = serde_json::from_str(&with_extra).expect("unknown field ignored");
    assert_eq!(task.projection_revision.as_str(), "7");

    let missing_required = TASK_JSON.replace("\"task_ref\": \"fixture:task/Task-184\",", "");
    assert!(serde_json::from_str::<TaskSnapshot>(&missing_required).is_err());
}

#[test]
fn candidate_reference_keeps_the_typed_tuple() {
    let task: TaskSnapshot = serde_json::from_str(TASK_JSON).expect("golden task parses");
    let candidate: &CandidateRef = task.best_candidate_ref.as_ref().expect("best candidate");
    assert_eq!(candidate.task_id, task.task_ref);
    assert_eq!(candidate.iteration, 3);
    assert_eq!(candidate.candidate_id.as_str(), "fixture:candidate/e");
    assert_eq!(candidate.artifact_ref.as_str(), "fixture:artifact/e");
    assert_ne!(
        candidate.artifact_ref.as_str(),
        candidate.candidate_id.as_str()
    );
}

#[test]
fn command_envelope_validation_keeps_creation_scoped() {
    let task_id = TaskId::fixture("task/Task-184");
    let valid = CommandEnvelope {
        command_id: CommandId::fixture("command/1"),
        target: CommandTarget::Task {
            task_id: task_id.clone(),
        },
        expected_revision: None,
        epoch: None,
        payload: CommandPayload::StopTask(StopTaskCommand {
            task_id: task_id.clone(),
            reason: None,
        }),
    };
    assert_eq!(valid.validate(), Ok(()));

    let mut mismatched = valid.clone();
    mismatched.target = CommandTarget::Task {
        task_id: TaskId::fixture("task/Other"),
    };
    assert!(mismatched.validate().is_err());

    let mut create_with_task_target = valid.clone();
    create_with_task_target.payload =
        CommandPayload::CreateTask(desktop_protocol::CreateTaskCommand {
            objective_text: "compose a key visual".to_owned(),
            text_prompt: None,
            reference_artifact_refs: Vec::new(),
            parameter_values: Vec::new(),
        });
    assert!(create_with_task_target.validate().is_err());

    let mut empty_objective = valid.clone();
    empty_objective.target = CommandTarget::Creation {
        recipe_ref: desktop_protocol::RecipeRef {
            recipe_id: desktop_protocol::RecipeId::fixture("recipe/1"),
            graph_ref: desktop_protocol::GraphRef {
                graph_id: desktop_protocol::GraphId::fixture("graph/1"),
                version: 1,
                digest: desktop_protocol::ArtifactDigest::fixture("digest/graph-1"),
            },
            source_harness_version_id: None,
            source_harness_digest: None,
            profile_ref: None,
            projection_digest: None,
        },
    };
    empty_objective.payload = CommandPayload::CreateTask(desktop_protocol::CreateTaskCommand {
        objective_text: String::new(),
        text_prompt: None,
        reference_artifact_refs: Vec::new(),
        parameter_values: Vec::new(),
    });
    assert!(empty_objective.validate().is_err());

    assert_eq!(
        serde_json::to_value(&valid).expect("envelope serializes")["payload"]["kind"],
        serde_json::Value::String("STOP_TASK".to_owned())
    );

    let error = CommandValidationError::new("x");
    assert_eq!(error.to_string(), "x");
}

#[test]
fn canonical_digest_is_deterministic_over_mutations() {
    let task: TaskSnapshot = serde_json::from_str(TASK_JSON).expect("golden task parses");
    let mutation = ProjectionMutation::Upsert {
        base_revision: None,
        projection: Projection::Task(task.clone()),
    };
    let first = integrity_digest_of(&vec![mutation.clone()]).expect("digest");
    let second = integrity_digest_of(&vec![mutation.clone()]).expect("digest");
    assert_eq!(first, second);
    assert!(first.as_str().starts_with("sha256:"));

    let mut changed = task;
    changed.title = "different".to_owned();
    let other = integrity_digest_of(&vec![ProjectionMutation::Upsert {
        base_revision: None,
        projection: Projection::Task(changed),
    }])
    .expect("digest");
    assert_ne!(first, other);
    assert!(
        !canonical_json(&vec![mutation])
            .expect("canonical")
            .is_empty()
    );
}

#[test]
fn unknown_server_message_kinds_default_to_state_changing() {
    let explicit: ServerMessage = serde_json::from_str(
        r#"{"kind":"FUTURE_EVENT","affects_state":false,"cursor":"fixture:cursor/9"}"#,
    )
    .expect("unknown message parses");
    match explicit {
        ServerMessage::Unknown(UnknownMessage {
            kind,
            affects_state,
            cursor,
        }) => {
            assert_eq!(kind, "FUTURE_EVENT");
            assert!(!affects_state);
            assert_eq!(cursor.expect("cursor").as_str(), "fixture:cursor/9");
        }
        other => panic!("expected unknown message, got {other:?}"),
    }

    let implicit: ServerMessage =
        serde_json::from_str(r#"{"kind":"FUTURE_STATE","cursor":"fixture:cursor/10"}"#)
            .expect("unknown message parses");
    match implicit {
        ServerMessage::Unknown(unknown) => assert!(unknown.affects_state),
        other => panic!("expected unknown message, got {other:?}"),
    }

    let known = ServerMessage::DeltaTransaction(DeltaTransaction {
        transaction_id: desktop_protocol::TransactionId::fixture("transaction/1"),
        epoch: desktop_protocol::EpochId::fixture("epoch/1"),
        cursor: desktop_protocol::Cursor::fixture("cursor/1"),
        predecessor: Some(desktop_protocol::Cursor::fixture("cursor/0")),
        mutation_count: 0,
        integrity_digest: integrity_digest_of(&Vec::<ProjectionMutation>::new()).expect("digest"),
        mutations: Vec::new(),
    });
    let wire = ServerWire::Frame(desktop_protocol::ServerFrame {
        protocol: Version::new(1, 0),
        schema: Version::new(1, 0),
        message: known.clone(),
    });
    let encoded = serde_json::to_string(&wire).expect("wire serializes");
    let decoded: ServerWire = serde_json::from_str(&encoded).expect("wire parses");
    assert_eq!(decoded, wire);
    assert_eq!(known.kind(), "DELTA_TRANSACTION");
}

#[test]
fn bounds_reject_oversized_declarations() {
    let defaults = Limits::DEFAULT;
    assert_eq!(defaults.max_frame_bytes, MAX_CONTROL_FRAME_BYTES);
    assert_eq!(defaults.max_bundle_bytes, MAX_STAGED_BUNDLE_BYTES);
    assert_eq!(defaults.max_queue_frames, MAX_QUEUE_FRAMES);
    assert!(defaults.frame_fits(MAX_CONTROL_FRAME_BYTES));
    assert!(!defaults.frame_fits(MAX_CONTROL_FRAME_BYTES + 1));
    assert!(!defaults.bundle_fits(MAX_STAGED_BUNDLE_BYTES + 1));

    let lowered = defaults.negotiate(Some(Limits {
        max_frame_bytes: 4096,
        max_bundle_bytes: 8192,
        max_queue_frames: 4,
        max_queue_bytes: 16_384,
    }));
    assert_eq!(lowered.max_frame_bytes, 4096);
    assert_eq!(lowered.max_queue_frames, 4);
    // A server can never raise local limits.
    let raised = defaults.negotiate(Some(Limits {
        max_frame_bytes: u64::MAX,
        max_bundle_bytes: u64::MAX,
        max_queue_frames: u32::MAX,
        max_queue_bytes: u64::MAX,
    }));
    assert_eq!(raised, defaults);
}
