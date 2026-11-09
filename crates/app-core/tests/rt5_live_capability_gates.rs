//! MP-RT5 live capability-gate harness (test-only, ignored by default).
//!
//! `apps/desktop/devtools/runtime_integration/run_rt5.py` captures the
//! `CAPABILITIES` and `TASK` projections exactly as an isolated Runtime
//! published them over the Desktop observation protocol. This harness
//! deserializes those captured payloads with the production wire types, builds
//! the same [`ObservationPublication`] shape the Desktop derives, and records
//! the real `app_core::capability` gate decisions for each captured state.
//!
//! It is a measurement adapter, not a product feature and not a fixture
//! fallback: the payloads are the live Runtime output and the expectations are
//! the DC-MP-RT5 contract (unknown => disabled; a projected UNAVAILABLE =>
//! disabled with the projected reason; only a supported in-scope entry
//! enables).
//!
//! Environment:
//! - `RT5_GATE_CAPTURE`: capture document written by the Python harness.
//! - `RT5_GATE_DECISIONS_OUT`: file the harness writes the decisions to.
//!
//! Run with:
//! `cargo test -p app-core --test rt5_live_capability_gates -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use app_core::ObservationPublication;
use app_core::capability::{for_server, for_task};
use app_core::{PublicationConnection, PublicationSource};
use cos_client::ProjectionState;
use desktop_protocol::{ArtifactId, CommandOperation, Projection, RepresentationKind, TaskId};
use serde_json::{Value, json};

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("capture field {field} must be a string"))
}

fn bool_field(value: &Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("capture field {field} must be a bool"))
}

#[test]
#[ignore = "RT5 live harness: set RT5_GATE_CAPTURE and RT5_GATE_DECISIONS_OUT"]
fn rt5_live_capability_gates() {
    let capture_path =
        std::env::var("RT5_GATE_CAPTURE").expect("RT5_GATE_CAPTURE names the capture document");
    let decisions_path =
        std::env::var("RT5_GATE_DECISIONS_OUT").expect("RT5_GATE_DECISIONS_OUT names the output");
    let capture: Value = serde_json::from_str(
        &fs::read_to_string(&capture_path).expect("the capture document is readable"),
    )
    .expect("the capture document is JSON");

    let task_id = TaskId::new(text(&capture, "task_id")).expect("captured task identity is valid");
    let artifact_probe = ArtifactId::new(text(&capture, "artifact_probe"))
        .expect("captured artifact probe is a valid identity");
    let states = capture
        .get("states")
        .and_then(Value::as_array)
        .expect("the capture carries a states array");

    let mut decisions: Vec<Value> = Vec::new();
    for state in states {
        let label = text(state, "label");
        let task: Projection = serde_json::from_value(
            state
                .get("task")
                .cloned()
                .expect("state carries the TASK projection"),
        )
        .expect("the captured TASK payload decodes with the production wire type");
        let Projection::Task(task) = task else {
            panic!("{label}: expected a TASK projection")
        };
        assert_eq!(
            task.task_ref, task_id,
            "{label}: the TASK projection is the measured task"
        );
        let capabilities = match state.get("capabilities") {
            Some(Value::Null) | None => None,
            Some(value) => {
                let projection: Projection = serde_json::from_value(value.clone()).expect(
                    "the captured CAPABILITIES payload decodes with the production wire type",
                );
                let Projection::Capabilities(capabilities) = projection else {
                    panic!("{label}: expected a CAPABILITIES projection")
                };
                assert_eq!(
                    task.capabilities_ref.as_ref(),
                    Some(&capabilities.capabilities_ref),
                    "{label}: the task references the captured capability projection"
                );
                Some(capabilities)
            }
        };

        let mut projection_state = ProjectionState::default();
        projection_state
            .tasks
            .insert(task.task_ref.clone(), task.clone());
        if let Some(capabilities) = &capabilities {
            projection_state
                .capabilities
                .insert(capabilities.capabilities_ref.clone(), capabilities.clone());
        }
        let publication = ObservationPublication {
            snapshot: Some(Arc::new(projection_state)),
            revision: None,
            connection: PublicationConnection::Observing,
            source: PublicationSource::Runtime,
            stale: false,
            last_error: None,
            command_statuses: BTreeMap::new(),
            session_generation: 1,
            publication_version: 1,
        };

        let task_capability = for_task(&publication, &task_id);
        let server_capability = for_server(&publication);
        let stop = task_capability.gate_command(&CommandOperation::StopTask);
        let media = task_capability.gate_media(&RepresentationKind::Original, &artifact_probe);
        let create = server_capability.gate_command(&CommandOperation::CreateTask);
        let feature_decisions: Vec<Value> = capabilities
            .iter()
            .flat_map(|capabilities| capabilities.features.iter())
            .map(|feature| {
                let gate = task_capability.gate_feature(&feature.feature);
                json!({
                    "feature": feature.feature,
                    "projected_support": feature.support.as_str(),
                    "enabled": gate.enabled,
                    "code": gate.code.as_str(),
                    "reason": gate.reason.as_ref(),
                })
            })
            .collect();

        let expect = state.get("expect").expect("state carries expectations");
        for (name, gate, expected) in [
            ("stop", &stop, expect.get("stop").expect("stop expectation")),
            (
                "media",
                &media,
                expect.get("media").expect("media expectation"),
            ),
            (
                "create",
                &create,
                expect.get("create").expect("create expectation"),
            ),
        ] {
            assert_eq!(
                gate.enabled,
                bool_field(expected, "enabled"),
                "{label}/{name}: enabled decision"
            );
            assert_eq!(
                gate.code.as_str(),
                text(expected, "code"),
                "{label}/{name}: decision code"
            );
            if let Some(reason) = expected.get("reason").and_then(Value::as_str) {
                assert_eq!(gate.reason.as_ref(), reason, "{label}/{name}: reason");
            }
        }
        for feature in &feature_decisions {
            let expected = expect
                .get("features")
                .and_then(|features| features.get(text(feature, "feature")))
                .unwrap_or_else(|| {
                    panic!(
                        "{label}: missing expectation for feature {}",
                        text(feature, "feature")
                    )
                });
            assert_eq!(
                bool_field(feature, "enabled"),
                bool_field(expected, "enabled"),
                "{label}/feature {}: enabled decision",
                text(feature, "feature")
            );
            assert_eq!(
                text(feature, "code"),
                text(expected, "code"),
                "{label}/feature {}: decision code",
                text(feature, "feature")
            );
        }

        decisions.push(json!({
            "label": label,
            "has_capabilities_projection": capabilities.is_some(),
            "capabilities_ref": capabilities.as_ref().map(|value| value.capabilities_ref.as_str()),
            "projection_revision": capabilities.as_ref().map(|value| value.projection_revision.as_str()),
            "scope": capabilities.as_ref().map(scope_label),
            "availability": capabilities.as_ref().map(|value| value.availability.as_str()),
            "reason": capabilities.as_ref().and_then(|value| value.reason.clone()),
            "commands": capabilities.as_ref().map_or(0, |value| value.commands.len()),
            "media": capabilities.as_ref().map_or(0, |value| value.media.len()),
            "features": capabilities.as_ref().map(|value| value.features.iter().map(|feature| json!({
                "feature": feature.feature,
                "support": feature.support.as_str(),
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "constraints": capabilities.as_ref().map(|value| value.constraints.iter().map(|constraint| json!({
                "name": constraint.name,
                "value": constraint.value,
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "gates": {
                "stop": {
                    "enabled": stop.enabled,
                    "code": stop.code.as_str(),
                    "reason": stop.reason.as_ref(),
                },
                "media": {
                    "enabled": media.enabled,
                    "code": media.code.as_str(),
                    "reason": media.reason.as_ref(),
                },
                "create_server_scope": {
                    "enabled": create.enabled,
                    "code": create.code.as_str(),
                    "reason": create.reason.as_ref(),
                },
                "features": feature_decisions,
            },
        }));
    }

    let document = json!({
        "schema": "rt5-gate-decisions.v1",
        "source": "app_core::capability over captured live CAPABILITIES/TASK projections",
        "task_id": task_id.as_str(),
        "artifact_probe": artifact_probe.as_str(),
        "states": decisions,
    });
    fs::write(
        &decisions_path,
        serde_json::to_string_pretty(&document).expect("decisions serialize"),
    )
    .expect("the decisions file is writable");
    println!(
        "{}",
        serde_json::to_string_pretty(&document).expect("decisions serialize")
    );
}

fn scope_label(capabilities: &desktop_protocol::CreativeCapabilities) -> Value {
    match &capabilities.scope {
        desktop_protocol::CapabilityScope::Server => json!({"kind": "SERVER"}),
        desktop_protocol::CapabilityScope::Task { task_id } => {
            json!({"kind": "TASK", "task_id": task_id.as_str()})
        }
        desktop_protocol::CapabilityScope::Artifact { artifact_id } => {
            json!({"kind": "ARTIFACT", "artifact_id": artifact_id.as_str()})
        }
    }
}
