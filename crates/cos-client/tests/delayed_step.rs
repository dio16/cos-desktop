//! Fixture deadlines must wake the real owner without polling while idle.

mod common;

use common::fixture_path;
use cos_client::{ConnectionState, FixtureScenario, MockCosClient};

#[test]
fn delayed_work_reports_a_wake_and_disconnect_cancels_it() {
    let mut scenario = FixtureScenario::from_file(fixture_path()).expect("scenario loads");
    let mut step = scenario
        .steps
        .iter()
        .find(|step| step.id == "delta-evaluation-e")
        .cloned()
        .expect("evaluation delta step");
    step.trigger = "after_ms:10000".to_owned();
    scenario.steps = vec![step];

    let mut mock = MockCosClient::from_scenario(scenario).expect("fixture loads");
    mock.connect(0).expect("connect");
    let initial = mock.pump_step(0, 64);
    assert_eq!(mock.connection(), &ConnectionState::Live);
    assert!(
        initial
            .next_deadline_ms
            .is_some_and(|at| at > 0 && at <= 10000),
        "a delayed fixture must provide the owner a bounded wake"
    );
    mock.disconnect();
    assert_eq!(mock.connection(), &ConnectionState::Disconnected);
    assert_eq!(mock.pump_step(1, 64).next_deadline_ms, None);
}

#[test]
fn invalid_delays_and_rearmed_explicit_transaction_identities_are_rejected() {
    let original = FixtureScenario::from_file(fixture_path()).expect("scenario loads");
    for trigger in [
        "after_ms:0",
        "after_ms:86400001",
        "after_ms:18446744073709551615",
    ] {
        let mut scenario = original.clone();
        scenario.steps[0].trigger = trigger.to_owned();
        assert!(scenario.validate().is_err(), "invalid trigger {trigger}");
    }
    let step = original
        .steps
        .iter()
        .find(|step| step.id == "delta-evaluation-e")
        .cloned()
        .expect("delta step");
    for field in ["transaction_id", "cursor", "predecessor"] {
        let mut value = serde_json::to_value(&original).expect("fixture serializes");
        let mut delayed = serde_json::to_value(&step).expect("step serializes");
        delayed["trigger"] = "after_ms:10000".into();
        delayed["actions"][0]["delta"][field] = "fixture:explicit".into();
        value["steps"] = serde_json::json!([delayed]);
        assert!(
            FixtureScenario::from_json(&value.to_string()).is_err(),
            "explicit {field}"
        );
    }
}
