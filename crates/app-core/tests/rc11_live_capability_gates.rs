//! RC-11 live capability-gate harness (test-only, ignored by default).
//!
//! `apps/desktop/devtools/runtime_integration/run_rc11.py` captures the
//! `CAPABILITIES` and `TASK` projections exactly as an isolated Runtime
//! published them over the Desktop observation protocol. This harness
//! deserializes those captured payloads with the production wire types, builds
//! the same [`ObservationPublication`] shape the Desktop derives, and records
//! the real `app_core::capability` gate decisions for every command/media
//! entry the RC-11 extension projects.
//!
//! It is a measurement adapter, not a product feature and not a fixture
//! fallback: the payloads are the live Runtime output and the expectations are
//! the DC-MP-RT5 contract (only a supported in-scope entry enables; every
//! other state is disabled with the projected or gate reason).
//!
//! Environment:
//! - `RC11_GATE_CAPTURE`: capture document written by the Python harness.
//! - `RC11_GATE_DECISIONS_OUT`: file the harness writes the decisions to.
//!
//! Run with:
//! `cargo test -p app-core --test rc11_live_capability_gates -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use app_core::ObservationPublication;
use app_core::capability::{CapabilityGate, for_server, for_task};
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

fn gate_json(gate: &CapabilityGate) -> Value {
    json!({
        "enabled": gate.enabled,
        "code": gate.code.as_str(),
        "reason": gate.reason.as_ref(),
    })
}

fn assert_gate(label: &str, name: &str, gate: &CapabilityGate, expected: &Value) {
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

#[test]
#[ignore = "RC-11 live harness: set RC11_GATE_CAPTURE and RC11_GATE_DECISIONS_OUT"]
fn rc11_live_capability_gates() {
    let capture_path =
        std::env::var("RC11_GATE_CAPTURE").expect("RC11_GATE_CAPTURE names the capture document");
    let decisions_path =
        std::env::var("RC11_GATE_DECISIONS_OUT").expect("RC11_GATE_DECISIONS_OUT names the output");
    let capture: Value = serde_json::from_str(
        &fs::read_to_string(&capture_path).expect("the capture document is readable"),
    )
    .expect("the capture document is JSON");

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
        let task_id: TaskId = task.task_ref.clone();
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
        projection_state.tasks.insert(task_id.clone(), task);
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
        let gates = [
            (
                "stop",
                task_capability.gate_command(&CommandOperation::StopTask),
            ),
            (
                "reuse",
                task_capability.gate_command(&CommandOperation::ReuseTask),
            ),
            (
                "create_server_scope",
                server_capability.gate_command(&CommandOperation::CreateTask),
            ),
            (
                "media_original",
                task_capability.gate_media(&RepresentationKind::Original, &artifact_probe),
            ),
            (
                "media_thumbnail",
                task_capability.gate_media(&RepresentationKind::Thumbnail, &artifact_probe),
            ),
            (
                "media_preview",
                task_capability.gate_media(&RepresentationKind::Preview, &artifact_probe),
            ),
            (
                "media_metadata",
                task_capability.gate_media(&RepresentationKind::Metadata, &artifact_probe),
            ),
        ];

        let expect = state.get("expect").expect("state carries expectations");
        for (name, gate) in &gates {
            assert_gate(
                label,
                name,
                gate,
                expect
                    .get(*name)
                    .unwrap_or_else(|| panic!("{label}: missing expectation for {name}")),
            );
        }

        let mut gate_object = serde_json::Map::new();
        for (name, gate) in &gates {
            gate_object.insert((*name).to_owned(), gate_json(gate));
        }
        decisions.push(json!({
            "label": label,
            "task_id": task_id.as_str(),
            "has_capabilities_projection": capabilities.is_some(),
            "capabilities_ref": capabilities.as_ref().map(|value| value.capabilities_ref.as_str()),
            "projection_revision": capabilities.as_ref().map(|value| value.projection_revision.as_str()),
            "scope": capabilities.as_ref().map(scope_label),
            "availability": capabilities.as_ref().map(|value| value.availability.as_str()),
            "reason": capabilities.as_ref().and_then(|value| value.reason.clone()),
            "commands": capabilities.as_ref().map(|value| serde_json::to_value(&value.commands).expect("commands serialize")),
            "media": capabilities.as_ref().map(|value| serde_json::to_value(&value.media).expect("media serialize")),
            "gates": Value::Object(gate_object),
        }));
    }

    let document = json!({
        "schema": "rc11-gate-decisions.v1",
        "source": "app_core::capability over captured live CAPABILITIES/TASK projections",
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
