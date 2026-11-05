//! Wire compatibility: what a peer may omit, what it may add, and what must be refused.
//!
//! The observation-v1 matrix itself needs a real Runtime (it remains `NOT_RUN`). What can be pinned
//! without one is the shape of the contract each message declares: an **optional** field may be
//! omitted, a **required** field may not, and an **unknown** field added by a newer peer is ignored
//! rather than fatal. That is what lets two releases talk to each other.

use desktop_protocol::versions::{HelloAccepted, HelloRejected, HelloRequest, NegotiationFailure};
use desktop_protocol::{EpochId, ServerId, Version};

fn rejected_json(reason: &str) -> String {
    format!(
        r#"{{"code": "VERSION_INCOMPATIBLE", "message": "no shared protocol",
             "reason": "{reason}",
             "protocol": [{{"major": 1, "min_minor": 0, "max_minor": 3}}]}}"#
    )
}

fn hello_json(extra: &str, omit_protocol: bool) -> String {
    let protocol = if omit_protocol {
        ""
    } else {
        r#""protocol": [{"major": 1, "min_minor": 0, "max_minor": 3}], "#
    };
    format!(
        r#"{{ {protocol}"schema": [{{"major": 2, "min_minor": 0, "max_minor": 1}}],
             "desktop_version": "0.1.0"{extra} }}"#
    )
}

#[test]
fn a_required_handshake_field_cannot_be_omitted() {
    let missing = hello_json("", true);
    assert!(
        serde_json::from_str::<HelloRequest>(&missing).is_err(),
        "a handshake without its protocol ranges must not deserialize"
    );

    let complete = hello_json("", false);
    let request: HelloRequest =
        serde_json::from_str(&complete).expect("the complete handshake deserializes");
    assert_eq!(request.protocol.len(), 1);
    assert_eq!(request.schema.len(), 1);
}

#[test]
fn an_optional_handshake_field_may_be_omitted_or_present() {
    // `features` is declared `#[serde(default)]`: both shapes must land on the same value.
    let without = hello_json("", false);
    let with = hello_json(r#", "features": ["observation-v1"]"#, false);
    let a: HelloRequest = serde_json::from_str(&without).expect("without optional field");
    let b: HelloRequest = serde_json::from_str(&with).expect("with optional field");
    assert!(a.features.is_empty());
    assert_eq!(b.features, vec!["observation-v1".to_string()]);
    // Everything else is equal, so the optional field is the only difference.
    assert_eq!(a.protocol, b.protocol);
    assert_eq!(a.schema, b.schema);
    assert_eq!(a.desktop_version, b.desktop_version);
}

#[test]
fn an_unknown_field_from_a_newer_peer_is_ignored() {
    // Forward compatibility: a newer peer adds a field the desktop does not know. Dropping the
    // frame would make every release skew fatal.
    let newer = hello_json(r#", "capabilities": {"observation": 2}"#, false);
    let request: HelloRequest =
        serde_json::from_str(&newer).expect("an unknown field must not be fatal");
    assert_eq!(request.protocol.len(), 1);

    let accepted = format!(
        r#"{{"protocol": {{"major": 1, "minor": 3}},
             "schema": {{"major": 2, "minor": 1}},
             "server_identity": "{}",
             "server_build": "runtime-test",
             "epoch": "{}",
             "future_field": [1, 2, 3]}}"#,
        ServerId::fixture("server").as_str(),
        EpochId::fixture("epoch").as_str()
    );
    let accepted: HelloAccepted =
        serde_json::from_str(&accepted).expect("an unknown field must not be fatal");
    assert_eq!(accepted.protocol, Version::new(1, 3));
    assert!(accepted.dedupe_retention_ms.is_none());
    assert!(accepted.limits.is_none());
}

#[test]
fn a_rejection_carries_whether_it_could_be_negotiated() {
    // The rejection path is part of the contract too: a version mismatch must arrive as a typed
    // reason, not as an empty frame.
    let rejected = rejected_json("NO_PROTOCOL_INTERSECTION");
    let rejected: HelloRejected =
        serde_json::from_str(&rejected).expect("the rejection deserializes");
    assert_eq!(rejected.reason, NegotiationFailure::NoProtocolIntersection);
    // A reason this release does not know must not fail the frame; it lands on `Unknown` so the
    // rejection can still be shown and logged.
    let future = rejected_json("SOME_FUTURE_REASON");
    let future: HelloRejected =
        serde_json::from_str(&future).expect("an unknown reason must not be fatal");
    assert!(matches!(future.reason, NegotiationFailure::Unknown(_)));
    // The schema list is optional and was omitted.
    assert!(rejected.schema.is_empty());
    assert_eq!(rejected.protocol.len(), 1);
}
