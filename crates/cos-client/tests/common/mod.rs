//! Shared test helpers.

#![allow(dead_code)]

use cos_client::{ClientEvent, CommandLookup, CommandState, ConnectionState, MockCosClient};
use desktop_protocol::{
    CommandId, DeltaTransaction, ServerFrame, ServerMessage, ServerWire, TaskId, Version,
};

/// Path of the committed Task-184 fixture.
pub fn fixture_path() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/task-184.json").to_owned()
}

/// Load the fixture provider without connecting.
pub fn load_mock() -> MockCosClient {
    MockCosClient::from_fixture_file(fixture_path()).expect("fixture loads")
}

/// Connect, drain initialization and assert the session is live.
pub fn live_mock() -> MockCosClient {
    let mut mock = load_mock();
    mock.connect(0).expect("connect");
    mock.pump(0);
    assert_eq!(
        mock.connection(),
        &ConnectionState::Live,
        "one pump must drain the scripted initialization"
    );
    mock
}

/// Primary task identity of the fixture.
pub fn task_id() -> TaskId {
    TaskId::fixture("task/Task-184")
}

/// Actual retained lifecycle state, without any authorization filtering.
pub fn retained_state(mock: &MockCosClient, command_id: &CommandId) -> Option<CommandState> {
    match mock.lookup_command(command_id) {
        CommandLookup::Found(record) => Some(record.state().clone()),
        CommandLookup::NotRetained => None,
    }
}

/// Encode a server frame with the negotiated versions and an explicit null stamp.
///
/// The fixture provider stamps state-bearing frames it injects from its active
/// binding, so this helper is used for transcripts that must stay unstamped.
pub fn server_frame(message: ServerMessage) -> String {
    serde_json::to_string(&ServerWire::Frame(ServerFrame {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
        read_stamp: None,
        message,
    }))
    .expect("server frame serializes")
}

/// Encode a delta transaction frame.
pub fn delta_frame(transaction: DeltaTransaction) -> String {
    server_frame(ServerMessage::DeltaTransaction(transaction))
}

/// Whether any event is an error.
pub fn has_error(events: &[ClientEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ClientEvent::Error(_)))
}
