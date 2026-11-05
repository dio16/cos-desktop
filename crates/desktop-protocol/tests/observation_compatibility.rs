//! The observation messages' compatibility contract, field by field.
//!
//! Each row states one field of one message: whether a peer may omit it, what the default is when
//! it does, and what the release does with it. The planning matrix for observation v1 against a
//! real Runtime is still `NOT_RUN`; this pins what the **wire shape** promises, which is what two
//! releases have to agree on before any bytes are exchanged.

use desktop_protocol::messages::{
    ObservationScopeRequest, OpenObservation, ReplayAccepted, ResyncRequired, ScopeFilter,
    UnknownMessage,
};
use desktop_protocol::{Cursor, ScopeId};

#[test]
fn a_resume_coordinate_may_be_omitted_and_then_means_the_first_session() {
    // Opening observation: the three resume coordinates are optional; with none of them the frame
    // describes a fresh session rather than a resumption.
    let fresh = r#"{"scope": {"task_ids": [], "filters": []}}"#;
    let open: OpenObservation = serde_json::from_str(fresh).expect("a fresh open deserializes");
    assert!(open.scope_id.is_none());
    assert!(open.last_seen_cursor.is_none());
    assert!(open.epoch.is_none());
    assert!(open.scope.task_ids.is_empty());
    assert!(open.scope.filters.is_empty());

    // A resumption carries them, and they survive the round trip unchanged.
    let resuming = format!(
        r#"{{"scope": {{"task_ids": [], "filters": []}},
             "scope_id": "{}",
             "last_seen_cursor": "cursor-41",
             "epoch": "{}"}}"#,
        ScopeId::fixture("scope").as_str(),
        desktop_protocol::EpochId::fixture("epoch").as_str()
    );
    let open: OpenObservation = serde_json::from_str(&resuming).expect("a resume deserializes");
    assert_eq!(
        open.scope_id.as_ref().map(ScopeId::as_str),
        Some(ScopeId::fixture("scope").as_str())
    );
    assert_eq!(
        open.last_seen_cursor.as_ref().map(Cursor::as_str),
        Some("cursor-41")
    );
    assert!(open.epoch.is_some());
}

#[test]
fn the_scope_filter_list_is_optional_but_the_task_list_is_required() {
    // `filters` is declared `#[serde(default)]`; `task_ids` is not. A frame that omits the optional
    // list is a wider request, not an invalid one; a frame that omits the required list is invalid.
    let without_filters = r#"{"task_ids": []}"#;
    let scope: ObservationScopeRequest =
        serde_json::from_str(without_filters).expect("the optional filter list may be omitted");
    assert!(scope.filters.is_empty());

    let without_tasks = r#"{"filters": []}"#;
    assert!(
        serde_json::from_str::<ObservationScopeRequest>(without_tasks).is_err(),
        "a scope without its task list must not deserialize"
    );

    // A filter entry itself has no optional parts.
    assert!(serde_json::from_str::<ScopeFilter>(r#"{"name": "layer"}"#).is_err());
    let filter: ScopeFilter =
        serde_json::from_str(r#"{"name": "layer", "value": "domain"}"#).expect("both parts");
    assert_eq!(filter.name, "layer");
    assert_eq!(filter.value, "domain");
}

#[test]
fn an_unrecognized_server_message_defaults_to_affecting_state() {
    // The safe default matters: an unknown event that might change authoritative state must be
    // treated as if it does, so a newer peer cannot silently desynchronise the desktop.
    let without_flag = r#"{"kind": "observation-v2.unknown"}"#;
    let unknown: UnknownMessage =
        serde_json::from_str(without_flag).expect("an unknown message deserializes");
    assert!(
        unknown.affects_state,
        "an unknown message must default to affecting state"
    );
    assert!(unknown.cursor.is_none());

    // A peer that says otherwise is believed, and the cursor it declares is kept.
    let declared =
        r#"{"kind": "observation-v2.unknown", "affects_state": false, "cursor": "cursor-7"}"#;
    let unknown: UnknownMessage = serde_json::from_str(declared).expect("the declared form");
    assert!(!unknown.affects_state);
    assert_eq!(
        unknown.cursor.as_ref().map(Cursor::as_str),
        Some("cursor-7")
    );
}

#[test]
fn a_resync_and_a_replay_acknowledgement_carry_what_the_desktop_needs() {
    // A resync must say why, and its human message is optional.
    let bare = r#"{"reason": "RETENTION_EXPIRED"}"#;
    let resync: ResyncRequired = serde_json::from_str(bare).expect("a bare resync deserializes");
    assert!(resync.message.is_none());

    // Without a reason there is nothing to act on, so it is not a valid frame.
    assert!(serde_json::from_str::<ResyncRequired>("{}").is_err());

    // A replay acknowledgement must name the cursor it starts after.
    let accepted: ReplayAccepted =
        serde_json::from_str(r#"{"cursor": "cursor-12"}"#).expect("a replay ack");
    assert_eq!(accepted.cursor.as_str(), "cursor-12");
    assert!(serde_json::from_str::<ReplayAccepted>("{}").is_err());
}

#[test]
fn a_newer_peer_may_add_fields_to_observation_messages() {
    // Forward compatibility on the observation path: extra fields are ignored, and the known ones
    // still land.
    let with_extras = r#"{"task_ids": [], "filters": [], "future_axis": {"depth": 3}}"#;
    let scope: ObservationScopeRequest =
        serde_json::from_str(with_extras).expect("an extra field must not be fatal");
    assert!(scope.task_ids.is_empty());

    let open_with_extra = r#"{"scope": {"task_ids": [], "filters": []}, "future_flag": true}"#;
    let open: OpenObservation =
        serde_json::from_str(open_with_extra).expect("an extra field must not be fatal");
    assert!(open.scope_id.is_none());
}
