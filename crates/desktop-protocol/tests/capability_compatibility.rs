//! The capability projection's compatibility contract.
//!
//! The rule the contract states is the one worth testing: **a capability the projection does not
//! mention, or a value this release does not recognize, leaves the operation disabled.** Getting
//! that wrong turns an unknown future capability into a permitted command, which is the one failure
//! mode this layer exists to prevent.

use desktop_protocol::projections::{
    CommandOperation, CreativeCapabilities, MediaDescriptor, SupportState,
};

fn capabilities_json(commands: &str, availability: &str) -> String {
    format!(
        r#"{{"capabilities_ref": "capabilities-1",
             "scope": {{"kind": "SERVER"}},
             "commands": [{commands}],
             "availability": "{availability}",
             "projection_revision": "3"}}"#
    )
}

#[test]
fn an_unmentioned_command_is_not_available() {
    // The projection lists one command; every other operation must be absent rather than defaulted
    // to supported.
    let json = capabilities_json(
        r#"{"operation": "STOP_TASK", "support": "SUPPORTED"}"#,
        "SUPPORTED",
    );
    let capabilities: CreativeCapabilities =
        serde_json::from_str(&json).expect("the capabilities projection deserializes");
    assert_eq!(
        capabilities.command_support(&CommandOperation::StopTask),
        Some(&SupportState::Supported)
    );
    assert_eq!(
        capabilities.command_support(&CommandOperation::CreateTask),
        None,
        "an unmentioned command must not be reported as supported"
    );
    // The optional lists were omitted and default to empty, which is the same as "nothing else".
    assert!(capabilities.media.is_empty());
    assert!(capabilities.features.is_empty());
    assert!(capabilities.constraints.is_empty());
    assert!(capabilities.reason.is_none());
}

#[test]
fn an_unsupported_command_is_reported_as_unsupported_not_missing() {
    // Present-but-unsupported and absent are different answers, and both keep the operation off.
    let json = capabilities_json(
        r#"{"operation": "CREATE_TASK", "support": "UNSUPPORTED"}"#,
        "SUPPORTED",
    );
    let capabilities: CreativeCapabilities =
        serde_json::from_str(&json).expect("the capabilities projection deserializes");
    let support = capabilities
        .command_support(&CommandOperation::CreateTask)
        .expect("the projection mentions the command");
    assert_eq!(support, &SupportState::Unsupported);
    assert!(!support.is_supported());
}

#[test]
fn an_unrecognized_capability_value_is_preserved_and_disables_the_operation() {
    // A newer Runtime may advertise a state this release has never seen. It must not fail the frame
    // and it must not read as supported.
    let json = capabilities_json(
        r#"{"operation": "CREATE_TASK", "support": "SUPPORTED_FOR_FUTURE_SCOPE"}"#,
        "SUPPORTED",
    );
    let capabilities: CreativeCapabilities =
        serde_json::from_str(&json).expect("an unknown support value must not fail the frame");
    let support = capabilities
        .command_support(&CommandOperation::CreateTask)
        .expect("the projection mentions the command");
    assert!(
        matches!(support, SupportState::Unknown(raw) if raw == "SUPPORTED_FOR_FUTURE_SCOPE"),
        "the unrecognized value must be preserved verbatim: {support:?}"
    );
    assert!(!support.is_known(), "an unrecognized value is not known");
    assert!(!support.is_supported());
    // And the value round-trips through **JSON**, not only through the string helper: a value this
    // release does not know must survive serialization verbatim rather than being rewritten into a
    // known one or refused on the way back in.
    assert_eq!(support.as_str(), "SUPPORTED_FOR_FUTURE_SCOPE");
    let wire = serde_json::to_string(support).expect("an unknown value serializes");
    assert_eq!(wire, "\"SUPPORTED_FOR_FUTURE_SCOPE\"");
    let again: SupportState = serde_json::from_str(&wire).expect("an unknown value deserializes");
    assert_eq!(&again, support);
    assert!(!again.is_supported());
}

#[test]
fn an_unrecognized_command_operation_is_kept_separate_from_the_known_ones() {
    // A future operation must not collide with a known one, and asking for a known operation must
    // not accidentally match it.
    let json = capabilities_json(
        r#"{"operation": "REUSE_TASK_FOR_FUTURE_SCOPE", "support": "SUPPORTED"}"#,
        "SUPPORTED",
    );
    let capabilities: CreativeCapabilities =
        serde_json::from_str(&json).expect("the capabilities projection deserializes");
    let listed: Vec<&CommandOperation> = capabilities
        .commands
        .iter()
        .map(|capability| &capability.operation)
        .collect();
    assert_eq!(listed.len(), 1);
    assert!(
        !listed[0].is_known(),
        "REUSE_TASK_FOR_FUTURE_SCOPE is not a known operation"
    );
    assert!(!listed[0].is_known());
    // Asking for the known operation of that kind must not match the future one.
    assert_eq!(
        capabilities.command_support(&CommandOperation::ReuseTask),
        None,
        "a future operation must not answer for the known one"
    );
}

#[test]
fn a_media_descriptor_defaults_its_representation_version_and_tolerates_unknown_extras() {
    // `representation_version` has a real default (1), and a descriptor from a newer peer may carry
    // fields this release does not know. Serde **ignores** an unknown field rather than keeping it, so
    // what is pinned here is that the frame still parses and nothing is invented for it.
    let json = r#"{"descriptor_id": "descriptor-1",
                   "artifact_ref": "artifact-1",
                   "representation": "THUMBNAIL",
                   "availability": "AVAILABLE",
                   "future_field": {"key": "value"}}"#;
    let descriptor: MediaDescriptor =
        serde_json::from_str(json).expect("the descriptor deserializes");
    assert_eq!(descriptor.representation_version, 1);
    assert_eq!(descriptor.descriptor_id.as_str(), "descriptor-1");
    assert_eq!(descriptor.artifact_ref.as_str(), "artifact-1");
    // Everything the peer did not state stays absent rather than being invented.
    assert!(descriptor.media_type.is_none());
    assert!(descriptor.byte_length.is_none());
    assert!(descriptor.width.is_none());
    assert!(descriptor.access_handle.is_none());
    assert!(descriptor.verification.is_none());
    // The required availability still arrives.
    assert!(descriptor.availability.is_available());

    // A missing availability is not tolerated: it is the field that decides whether pixels may be
    // requested at all.
    let without_availability = r#"{"descriptor_id": "descriptor-1",
                                   "artifact_ref": "artifact-1",
                                   "representation": "THUMBNAIL"}"#;
    assert!(serde_json::from_str::<MediaDescriptor>(without_availability).is_err());
}
