//! Deterministic clock boundary tests drive the real client/publication seam.
//! The private clock override only schedules races; expected revisions, delivery
//! counts, cancellation and trace records are independent contract oracles.

use super::*;
use crate::fixture::FixtureAction;
use desktop_protocol::EvaluationId;
use std::fs::{File, OpenOptions};
use std::sync::atomic::{AtomicU64, Ordering};

fn scenario() -> FixtureScenario {
    FixtureScenario::from_json(include_str!("../../fixtures/task-184.json")).expect("fixture")
}

fn delta_step() -> FixtureStep {
    let original = scenario();
    let mut step = original
        .steps
        .into_iter()
        .find(|step| step.id == "delta-evaluation-e")
        .expect("delta");
    step.trigger = "after_ms:500".to_owned();
    step
}

fn client(steps: Vec<FixtureStep>) -> (MockCosClient, Instant) {
    let mut scenario = scenario();
    scenario.steps = steps;
    let mut mock = MockCosClient::from_scenario(scenario).expect("mock");
    let start = Instant::now();
    mock.mock_mut().fixture_now_override = Some(start);
    mock.connect(0).expect("connect");
    mock.pump_step(0, 64);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    (mock, start)
}

fn at(mock: &mut MockCosClient, start: Instant, milliseconds: u64, budget: usize) -> PumpReport {
    mock.mock_mut().fixture_now_override = Some(start + Duration::from_millis(milliseconds));
    mock.pump_step(milliseconds, budget)
}

fn delta_count(report: &PumpReport) -> usize {
    report
        .events
        .iter()
        .filter(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
        .count()
}

fn evaluation_revision(mock: &MockCosClient) -> String {
    mock.snapshot().evaluations[&EvaluationId::fixture("evaluation/e")]
        .projection_revision
        .as_str()
        .to_owned()
}

#[test]
fn early_wakes_cannot_deliver_and_the_due_delivery_stops_idle_wakes() {
    let (mut mock, start) = client(vec![delta_step()]);
    for milliseconds in [0, 1, 200, 499] {
        let report = at(&mut mock, start, milliseconds, 64);
        assert_eq!(delta_count(&report), 0);
        assert_eq!(evaluation_revision(&mock), "1");
        assert_eq!(report.next_deadline_ms, Some(500));
    }
    let due = at(&mut mock, start, 500, 64);
    assert_eq!(delta_count(&due), 1);
    assert_eq!(evaluation_revision(&mock), "2");
    assert_eq!(due.next_deadline_ms, None);
    let idle = at(&mut mock, start, 501, 64);
    assert_eq!(delta_count(&idle), 0);
    assert_eq!(idle.next_deadline_ms, None);
    assert!(
        !idle
            .events
            .iter()
            .any(|event| matches!(event, ClientEvent::DuplicateIgnored { .. }))
    );
}

fn empty_delta() -> FixtureAction {
    serde_json::from_value(serde_json::json!({"delta": {"mutations": []}})).expect("delta action")
}

#[test]
fn sixty_four_and_sixty_five_frames_drain_without_losing_the_followup_wake() {
    for count in [64, 65] {
        let step = FixtureStep {
            id: "batch".to_owned(),
            trigger: "after_ms:500".to_owned(),
            actions: vec![empty_delta(); count],
        };
        let (mut mock, start) = client(vec![step]);
        let first = at(&mut mock, start, 500, 64);
        assert_eq!(delta_count(&first), 64);
        assert_eq!(
            first.next_deadline_ms,
            if count == 65 { Some(501) } else { None }
        );
        let second = at(&mut mock, start, 501, 64);
        assert_eq!(delta_count(&second), count - 64);
        assert_eq!(second.next_deadline_ms, None);
        assert_eq!(mock.connection(), &ConnectionState::Live);
    }
}

#[test]
fn equal_deadlines_deliver_both_steps_once() {
    let steps = ["first", "second"]
        .into_iter()
        .map(|id| FixtureStep {
            id: id.to_owned(),
            trigger: "after_ms:500".to_owned(),
            actions: vec![empty_delta()],
        })
        .collect();
    let (mut mock, start) = client(steps);
    assert_eq!(delta_count(&at(&mut mock, start, 499, 64)), 0);
    assert_eq!(delta_count(&at(&mut mock, start, 500, 64)), 2);
    assert_eq!(delta_count(&at(&mut mock, start, 501, 64)), 0);
}

#[test]
fn cancelled_binding_cannot_deliver_and_a_fresh_grant_rearms_from_its_own_clock() {
    let (mut mock, start) = client(vec![delta_step()]);
    at(&mut mock, start, 100, 64);
    mock.disconnect();
    let closed = at(&mut mock, start, 600, 64);
    assert_eq!(delta_count(&closed), 0);
    assert_eq!(closed.next_deadline_ms, None);
    assert_eq!(evaluation_revision(&mock), "1");
    mock.connect(600).expect("fresh connect");
    let opened = at(&mut mock, start, 600, 64);
    assert_eq!(opened.next_deadline_ms, Some(1100));
    assert_eq!(delta_count(&at(&mut mock, start, 1099, 64)), 0);
    assert_eq!(delta_count(&at(&mut mock, start, 1100, 64)), 1);
    assert_eq!(evaluation_revision(&mock), "2");
}

#[test]
fn scope_replacement_cancels_old_work_without_cancelling_the_new_schedule() {
    let (mut mock, start) = client(vec![delta_step()]);
    at(&mut mock, start, 200, 64);
    let mut scope = mock.mock().scenario().observation_request();
    scope.task_ids.push(TaskId::fixture("task/another"));
    mock.control().serve_scope(scope.clone());
    let request = mock.client().observation_request(scope);
    mock.begin_authorized_observation(request)
        .expect("replacement");
    assert_eq!(at(&mut mock, start, 200, 64).next_deadline_ms, Some(700));
    assert_eq!(delta_count(&at(&mut mock, start, 500, 64)), 0);
    assert_eq!(delta_count(&at(&mut mock, start, 700, 64)), 1);
}

#[test]
fn epoch_change_cancels_the_previous_booking() {
    let (mut mock, start) = client(vec![delta_step()]);
    mock.mock_mut().set_epoch(EpochId::fixture("epoch/next"));
    let old = at(&mut mock, start, 500, 64);
    assert_eq!(delta_count(&old), 0);
    assert_eq!(old.next_deadline_ms, None);
    assert_eq!(evaluation_revision(&mock), "1");
}

#[test]
fn one_shot_invalidation_recovers_once_and_remains_stable_after_new_grants() {
    let step = FixtureStep {
        id: "revoke-once".to_owned(),
        trigger: "after_ms_once:500".to_owned(),
        actions: vec![FixtureAction {
            read_invalidated: Some(ReadInvalidationReason::GrantRevoked),
            ..FixtureAction::default()
        }],
    };
    let (mut mock, start) = client(vec![step]);
    at(&mut mock, start, 500, 64);
    assert!(matches!(
        mock.publication_access(),
        PublicationAccess::Withheld { .. }
    ));
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
    let recovery = at(&mut mock, start, 10000, 64);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert!(matches!(
        mock.publication_access(),
        PublicationAccess::Validated { .. }
    ));
    assert_eq!(recovery.next_deadline_ms, None);
    assert_eq!(at(&mut mock, start, 20000, 64).next_deadline_ms, None);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert_eq!(mock.mock().observation_count(), 2);
}

struct TraceFile(std::path::PathBuf);
impl TraceFile {
    fn create() -> (Self, File) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cos-fixture-trace-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("new trace");
        (Self(path), file)
    }
    fn records(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(&self.0).expect("trace read");
        assert!(text.ends_with('\n'), "trace ends in a complete record");
        text.lines()
            .map(|line| serde_json::from_str(line).expect("trace record"))
            .collect()
    }
}
impl Drop for TraceFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn a_dispatched_stop_is_recorded_with_its_actual_identity_and_payload_kind() {
    let (path, file) = TraceFile::create();
    let (mut mock, start) = client(Vec::new());
    mock.mock_mut().trace = FixtureTrace::with_file(file);
    let task = mock.primary_task().expect("primary task").clone();
    let command_id = mock.stop_task(&task).expect("stop dispatched");
    at(&mut mock, start, 1, 64);
    let records = path.records();
    let command = records
        .iter()
        .find(|record| record["event"] == "command")
        .expect("command record");
    assert_eq!(command["command_id"], command_id.as_str());
    assert_eq!(command["payload_kind"], "STOP_TASK");
    assert_eq!(command["pid"], u64::from(std::process::id()));
    assert!(
        command["execution_id"]
            .as_str()
            .is_some_and(|execution| !execution.is_empty())
    );
    assert!(
        command["sequence"]
            .as_u64()
            .is_some_and(|sequence| sequence >= 1)
    );
    assert!(
        command["monotone_ns"]
            .as_str()
            .is_some_and(|nanos| nanos.parse::<u128>().is_ok())
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["event"] == "command")
            .count(),
        1,
        "one dispatch is one recorded command"
    );
    // The env-gated trace and the test-only counter must observe the same
    // dispatch point with the same identity and payload kind.
    assert_eq!(
        mock.control().sent_commands(),
        vec![(command_id.as_str().to_owned(), "STOP_TASK".to_owned())]
    );
    for (index, record) in records.iter().enumerate() {
        assert_eq!(
            record["sequence"],
            index as u64 + 1,
            "complete ordered trace"
        );
    }
}

#[test]
fn overflow_never_claims_a_partial_batch_was_delivered() {
    let (path, file) = TraceFile::create();
    let mut scenario = scenario();
    let mut limits = Limits::DEFAULT;
    limits.max_queue_frames = 64;
    scenario.limits = Some(limits);
    scenario.steps = vec![FixtureStep {
        id: "too-many".to_owned(),
        trigger: "after_ms:500".to_owned(),
        actions: vec![empty_delta(); 65],
    }];
    let mut mock = MockCosClient::from_scenario(scenario).expect("mock");
    let start = Instant::now();
    mock.mock_mut().fixture_now_override = Some(start);
    mock.mock_mut().trace = FixtureTrace::with_file(file);
    mock.connect(0).expect("connect");
    mock.pump_step(0, 64);
    let report = at(&mut mock, start, 500, 64);
    assert_eq!(delta_count(&report), 0);
    assert!(matches!(
        mock.connection(),
        ConnectionState::Reconnecting { .. }
    ));
    let records = path.records();
    assert!(
        records
            .iter()
            .any(|record| record["event"] == "materialized" && record["frame_count"] == 65)
    );
    assert!(!records.iter().any(|record| record["event"] == "delivered"));
}

#[test]
fn trace_write_failure_does_not_prevent_the_real_client_delta() {
    let (mut mock, start) = client(vec![delta_step()]);
    let read_only = File::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/task-184.json"
    ))
    .expect("read-only file");
    mock.mock_mut().trace = FixtureTrace::with_file(read_only);
    let report = at(&mut mock, start, 500, 64);
    assert_eq!(delta_count(&report), 1);
    assert_eq!(evaluation_revision(&mock), "2");
    assert_eq!(mock.connection(), &ConnectionState::Live);
}

#[test]
fn delivery_trace_is_complete_unique_and_matches_the_consumed_transaction() {
    let (path, file) = TraceFile::create();
    let (mut mock, start) = client(vec![delta_step()]);
    mock.mock_mut().trace = FixtureTrace::with_file(file);
    let report = at(&mut mock, start, 500, 64);
    let transaction = report
        .events
        .iter()
        .find_map(|event| match event {
            ClientEvent::DeltaApplied { transaction_id, .. } => Some(transaction_id.as_str()),
            _ => None,
        })
        .expect("consumed transaction");
    let records = path.records();
    for (index, record) in records.iter().enumerate() {
        assert_eq!(
            record["sequence"],
            index as u64 + 1,
            "complete ordered trace"
        );
    }
    let delivered = records
        .iter()
        .find(|record| record["event"] == "delivered")
        .expect("delivery record");
    let materialized = records
        .iter()
        .find(|record| record["event"] == "materialized")
        .expect("materialization record");
    assert!(materialized["sequence"].as_u64() < delivered["sequence"].as_u64());
    assert_eq!(delivered["frame_count"], 1);
    assert_eq!(delivered["session_serial"], 1);
    assert_eq!(delivered["transactions"], serde_json::json!([transaction]));
    let observed = records
        .iter()
        .find(|record| record["event"] == "consumer_state")
        .expect("observed client state");
    assert_eq!(observed["consumer_access"], "validated");
    assert_eq!(observed["connection"], "live");
    assert_eq!(evaluation_revision(&mock), "2");
}

#[test]
fn an_already_due_step_with_no_frame_budget_reports_a_positive_progress_wait() {
    let (mut mock, start) = client(vec![delta_step()]);
    let deferred = at(&mut mock, start, 500, 0);
    assert_eq!(delta_count(&deferred), 0);
    assert_eq!(deferred.next_deadline_ms, Some(501));
    assert_eq!(evaluation_revision(&mock), "1");
    let delivered = at(&mut mock, start, 500, 64);
    assert_eq!(delta_count(&delivered), 1);
    assert_eq!(delivered.next_deadline_ms, None);
}

#[test]
fn a_repeatable_step_uses_a_new_transaction_on_a_fresh_grant() {
    let step = FixtureStep {
        id: "repeat".to_owned(),
        trigger: "after_ms:500".to_owned(),
        actions: vec![empty_delta()],
    };
    let (mut mock, start) = client(vec![step]);
    let first = at(&mut mock, start, 500, 64);
    let first_cursor = first
        .events
        .iter()
        .find_map(|event| match event {
            ClientEvent::DeltaApplied { cursor, .. } => Some(cursor.clone()),
            _ => None,
        })
        .expect("first cursor");
    assert_eq!(delta_count(&first), 1);
    mock.connect(500).expect("new grant");
    let reopened = at(&mut mock, start, 500, 64);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert_eq!(reopened.next_deadline_ms, Some(1000));
    let second = at(&mut mock, start, 1000, 64);
    assert_eq!(delta_count(&second), 1);
    let second_cursor = second
        .events
        .iter()
        .find_map(|event| match event {
            ClientEvent::DeltaApplied { cursor, .. } => Some(cursor.clone()),
            _ => None,
        })
        .expect("second cursor");
    assert_ne!(second_cursor, first_cursor);
    assert!(
        !second
            .events
            .iter()
            .any(|event| matches!(event, ClientEvent::DuplicateIgnored { .. }))
    );
}

#[test]
fn an_earlier_real_read_deadline_is_not_hidden_by_the_fixture_hint() {
    let mut scenario = scenario();
    scenario
        .read_authorization
        .as_mut()
        .expect("authorization")
        .answer_lookups = false;
    let mut step = delta_step();
    step.trigger = "after_ms:86400000".to_owned();
    scenario.steps = vec![step];
    let mut mock = MockCosClient::from_scenario(scenario).expect("fixture");
    let start = Instant::now();
    mock.mock_mut().fixture_now_override = Some(start);
    mock.connect(0).expect("connect");
    mock.pump_step(0, 64);
    mock.stop_task(&TaskId::fixture("task/Task-184"))
        .expect("stop");
    mock.pump_step(0, 64);
    let scope = mock.mock().scenario().observation_request();
    let mut request = mock.client().observation_request(scope);
    request.retained_commands.clear();
    mock.begin_authorized_observation(request).expect("rebind");
    mock.pump_step(0, 64);
    let report = at(&mut mock, start, 1, 64);
    assert!(
        report
            .next_deadline_ms
            .is_some_and(|at| at > 1 && at < 86400000)
    );
}

#[test]
fn fixture_step_bound_rejects_an_unbounded_schedule_before_client_construction() {
    let mut bounded = scenario();
    bounded.steps = (0..1024)
        .map(|index| FixtureStep {
            id: format!("bounded-{index}"),
            trigger: "after_ms:500".to_owned(),
            actions: vec![empty_delta()],
        })
        .collect();
    bounded.validate().expect("bounded scenario admitted");
    bounded.steps.push(FixtureStep {
        id: "over-bound".to_owned(),
        trigger: "after_ms:500".to_owned(),
        actions: vec![empty_delta()],
    });
    assert!(matches!(
        MockCosClient::from_scenario(bounded),
        Err(FixtureError::Invalid(_))
    ));
}
