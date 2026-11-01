//! Protocol transport tests over a stub REST executor.
//!
//! The stub reproduces the three Runtime endpoints byte-for-byte
//! (`GET /api/cos/desktop/server`, `POST /api/cos/desktop/observation`,
//! `GET /api/cos/desktop/deltas`), so the transport is exercised exactly as it
//! would run against the real adapter, and a real `CosClient` is driven from
//! `OPEN_OBSERVATION` to a published snapshot and an applied delta.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use cos_client::{
    ClientConfig, ClientEvent, ClientSource, ConnectionState, CosClient, DELTAS_PATH, HttpMethod,
    HttpRequest, HttpResponse, OBSERVATION_PATH, ProtocolHttp, ProtocolTransport,
    ProtocolTransportConfig, SERVER_PATH, Transport, TransportError,
};
use desktop_protocol::{
    ClientFrame, ClientMessage, ClientWire, CommandEnvelope, CommandId, CommandPayload,
    CommandTarget, Counter, Cursor, DeltaTransaction, EpochId, HelloAccepted, HelloRequest, Limits,
    ObservationScope, ObservationScopeRequest, OpenAuthorizedObservation, OpenObservation,
    PhaseLabel, Projection, ProjectionMutation, ProtocolVersions, ReadContextId, ReadGrant,
    ReadRequestId, ReadRevision, ReadStamp, ScopeFilter, ScopeId, ServerId, ServerWire,
    SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotEnd, SnapshotId, StopTaskCommand, TaskId,
    TaskSnapshot, TaskStatus, TransactionId, Version, VersionRange, projection_bundle_digest,
};

const SERVER_IDENTITY: &str = "runtime-server";
const EPOCH: &str = "epoch-1";

type Gate = Arc<(Mutex<bool>, Condvar)>;

struct ServerState {
    calls: Vec<HttpRequest>,
    threads: Vec<ThreadId>,
    /// One-shot block installed before an exchange to prove `send` never waits.
    gate: Option<Gate>,
    /// Remaining delta polls answered with `409 RESYNC_REQUIRED`.
    resync_polls: u32,
    /// Serve one delta transaction on the next successful poll.
    delta: bool,
    next_session: u64,
    cursor: String,
}

impl ServerState {
    fn new() -> Self {
        Self {
            calls: Vec::new(),
            threads: Vec::new(),
            gate: None,
            resync_polls: 0,
            delta: false,
            next_session: 0,
            cursor: String::new(),
        }
    }

    fn calls_to(&self, method: HttpMethod, path: &str) -> usize {
        self.calls
            .iter()
            .filter(|call| call.method == method && call.path == path)
            .count()
    }
}

#[derive(Clone)]
struct StubHttp {
    state: Arc<Mutex<ServerState>>,
}

impl StubHttp {
    fn new() -> (Self, Arc<Mutex<ServerState>>) {
        let state = Arc::new(Mutex::new(ServerState::new()));
        (
            Self {
                state: Arc::clone(&state),
            },
            state,
        )
    }
}

impl ProtocolHttp for StubHttp {
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        let gate = {
            let mut state = self.state.lock().expect("stub state");
            state.calls.push(request.clone());
            state.threads.push(std::thread::current().id());
            state.gate.take()
        };
        if let Some(gate) = gate {
            let mut open = gate.0.lock().expect("gate");
            while !*open {
                open = gate.1.wait(open).expect("gate wait");
            }
        }
        let mut state = self.state.lock().expect("stub state");
        match (request.method, request.path.as_str()) {
            (HttpMethod::Get, SERVER_PATH) => Ok(json_response(server_document(&state))),
            (HttpMethod::Post, OBSERVATION_PATH) => {
                let body: serde_json::Value = serde_json::from_slice(&request.body)
                    .map_err(|error| format!("request body: {error}"))?;
                Ok(json_response(open_document(&mut state, &body)))
            }
            (HttpMethod::Get, path) if path.starts_with(DELTAS_PATH) => {
                if state.resync_polls > 0 {
                    state.resync_polls -= 1;
                    return Ok(HttpResponse {
                        status: 409,
                        body: br#"{"code":"RESYNC_REQUIRED"}"#.to_vec(),
                    });
                }
                let transaction = if state.delta {
                    state.delta = false;
                    Some(delta_transaction(&state))
                } else {
                    None
                };
                Ok(json_response(serde_json::json!({
                    "schema_version": 1,
                    "transaction": transaction,
                })))
            }
            _ => Ok(HttpResponse {
                status: 404,
                body: b"{}".to_vec(),
            }),
        }
    }
}

fn json_response(value: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        body: serde_json::to_vec(&value).expect("response encodes"),
    }
}

fn hello() -> HelloAccepted {
    HelloAccepted {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
        server_identity: ServerId::new(SERVER_IDENTITY).expect("server id"),
        server_build: "orchestrator-runtime/test".to_owned(),
        epoch: EpochId::new(EPOCH).expect("epoch"),
        dedupe_retention_ms: None,
        limits: None,
    }
}

fn server_document(_state: &ServerState) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "server": serde_json::to_value(hello()).expect("hello encodes"),
    })
}

fn task_mutations(title: &str, revision: &str) -> Vec<ProjectionMutation> {
    vec![ProjectionMutation::Upsert {
        base_revision: (revision != "1").then(|| Counter::new("1").expect("base revision")),
        projection: Projection::Task(TaskSnapshot {
            task_ref: TaskId::new("task/Task-184").expect("task id"),
            display_id: Some("Task-184".to_owned()),
            title: title.to_owned(),
            task_status: TaskStatus::Active,
            inner_phase: Some(PhaseLabel::new("REFINE").expect("phase")),
            iteration_ref: None,
            iteration_index: None,
            best_candidate_ref: None,
            formal_outcome_ref: None,
            accepted_brief_ref: None,
            capabilities_ref: None,
            projection_revision: Counter::new(revision).expect("revision"),
        }),
    }]
}

fn open_document(state: &mut ServerState, body: &serde_json::Value) -> serde_json::Value {
    let request_id = body["request_id"].as_str().expect("request id").to_owned();
    let protocol: Version =
        serde_json::from_value(body["protocol"].clone()).expect("request protocol");
    let schema: Version = serde_json::from_value(body["schema"].clone()).expect("request schema");
    let scope_request: ObservationScopeRequest =
        serde_json::from_value(body["scope"].clone()).expect("request scope");
    state.next_session += 1;
    let session = state.next_session;
    let scope_id = format!("scope-{session}");
    let context_id = format!("context-{session}");
    let snapshot_id = SnapshotId::new(format!("snapshot-{session}")).expect("snapshot id");
    let cursor = format!("cursor-{session}");
    let title = format!("REST task {session}");
    state.cursor.clone_from(&cursor);
    let scope = ObservationScope {
        scope_id: ScopeId::new(scope_id).expect("scope id"),
        task_ids: scope_request.task_ids.clone(),
        filters: scope_request.filters.clone(),
        protocol,
        schema,
    };
    let mutations = task_mutations(&title, "1");
    let begin = SnapshotBegin {
        snapshot_id: snapshot_id.clone(),
    };
    let chunks = [SnapshotChunk {
        snapshot_id: snapshot_id.clone(),
        part_index: 0,
        mutations: mutations.clone(),
    }];
    let end = SnapshotEnd {
        snapshot_id,
        scope: scope.clone(),
        epoch: EpochId::new(EPOCH).expect("epoch"),
        cursor: Cursor::new(&cursor).expect("cursor"),
        expected: SnapshotCounts::of(&mutations),
        integrity_digest: projection_bundle_digest(&mutations).expect("digest"),
    };
    let grant = ReadGrant {
        stamp: ReadStamp {
            request_id: ReadRequestId::new(request_id).expect("request id"),
            server_id: ServerId::new(SERVER_IDENTITY).expect("server id"),
            epoch: EpochId::new(EPOCH).expect("epoch"),
            context_id: ReadContextId::new(context_id).expect("context id"),
            read_revision: ReadRevision::new(Counter::new("1").expect("counter")),
            scope_id: scope.scope_id.clone(),
        },
        scope,
        local_intents_readable: false,
        command_reads: Vec::new(),
    };
    serde_json::json!({
        "schema_version": 1,
        "server": serde_json::to_value(hello()).expect("hello encodes"),
        "grant": serde_json::to_value(grant).expect("grant encodes"),
        "snapshot": {
            "begin": serde_json::to_value(begin).expect("begin encodes"),
            "chunks": chunks
                .iter()
                .map(|chunk| serde_json::to_value(chunk).expect("chunk encodes"))
                .collect::<Vec<_>>(),
            "end": serde_json::to_value(end).expect("end encodes"),
        },
    })
}

fn delta_transaction(state: &ServerState) -> DeltaTransaction {
    let mutations = task_mutations("REST task updated", "2");
    DeltaTransaction {
        transaction_id: TransactionId::new("transaction-1").expect("transaction id"),
        epoch: EpochId::new(EPOCH).expect("epoch"),
        cursor: Cursor::new(format!("cursor-delta-{}", state.next_session)).expect("cursor"),
        predecessor: Some(Cursor::new(&state.cursor).expect("predecessor")),
        mutation_count: mutations.len() as u32,
        integrity_digest: projection_bundle_digest(&mutations).expect("digest"),
        mutations,
    }
}

fn hello_frame() -> Vec<u8> {
    serde_json::to_vec(&ClientWire::Hello(HelloRequest {
        desktop_version: "cos-desktop/test".to_owned(),
        protocol: vec![VersionRange::new(2, 0, 0)],
        schema: vec![VersionRange::new(2, 0, 0)],
        features: Vec::new(),
    }))
    .expect("hello frame")
}

fn open_request(request_id: &str) -> OpenAuthorizedObservation {
    OpenAuthorizedObservation {
        request_id: ReadRequestId::new(request_id).expect("request id"),
        observation: OpenObservation {
            scope: ObservationScopeRequest {
                task_ids: Vec::new(),
                filters: vec![ScopeFilter {
                    name: "view".to_owned(),
                    value: "all".to_owned(),
                }],
            },
            scope_id: None,
            last_seen_cursor: None,
            epoch: None,
        },
        retained_commands: Vec::new(),
    }
}

fn open_frame(request: OpenAuthorizedObservation) -> Vec<u8> {
    serde_json::to_vec(&ClientWire::Frame(ClientFrame {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
        message: ClientMessage::OpenObservation(request),
    }))
    .expect("open frame")
}

fn command_frame() -> Vec<u8> {
    let task_id = TaskId::new("task/Task-184").expect("task id");
    serde_json::to_vec(&ClientWire::Frame(ClientFrame {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
        message: ClientMessage::Command(CommandEnvelope {
            command_id: CommandId::new("command-1").expect("command id"),
            target: CommandTarget::Task {
                task_id: task_id.clone(),
            },
            expected_revision: None,
            epoch: None,
            payload: CommandPayload::StopTask(StopTaskCommand {
                task_id,
                reason: None,
            }),
        }),
    }))
    .expect("command frame")
}

fn versions() -> ProtocolVersions {
    ProtocolVersions {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
    }
}

fn config() -> ProtocolTransportConfig {
    ProtocolTransportConfig {
        delta_poll_interval: Duration::from_millis(1),
        ..ProtocolTransportConfig::default()
    }
}

fn transport() -> (ProtocolTransport<StubHttp>, Arc<Mutex<ServerState>>) {
    let (executor, state) = StubHttp::new();
    let transport = ProtocolTransport::new(executor, config(), versions());
    (transport, state)
}

fn client(transport: ProtocolTransport<StubHttp>) -> CosClient<ProtocolTransport<StubHttp>> {
    CosClient::new(
        transport,
        ClientConfig::default(),
        Limits::DEFAULT,
        ClientSource::Unknown,
    )
}

fn next_frame(transport: &mut ProtocolTransport<StubHttp>) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match transport.poll() {
            Ok(Some(frame)) => return frame,
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => panic!("unexpected transport error: {error:?}"),
        }
    }
    panic!("timed out waiting for a frame");
}

fn decoded(frame: &[u8]) -> ServerWire {
    serde_json::from_slice(frame).expect("server wire decodes")
}

fn message_kind(wire: &ServerWire) -> String {
    match wire {
        ServerWire::HelloAccepted(_) => "HELLO_ACCEPTED".to_owned(),
        ServerWire::HelloRejected(_) => "HELLO_REJECTED".to_owned(),
        ServerWire::Frame(frame) => frame.message.kind().to_owned(),
    }
}

fn drive_until<F>(
    client: &mut CosClient<ProtocolTransport<StubHttp>>,
    mut condition: F,
) -> Vec<ClientEvent>
where
    F: FnMut(&CosClient<ProtocolTransport<StubHttp>>) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    let mut now = 0u64;
    while Instant::now() < deadline {
        now += 10;
        events.extend(client.pump(now));
        if condition(client) {
            return events;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "timed out driving the client; state {:?}, last error {:?}",
        client.connection(),
        client.last_error()
    );
}

fn task_title(client: &CosClient<ProtocolTransport<StubHttp>>) -> Option<String> {
    client
        .snapshot()
        .tasks
        .values()
        .next()
        .map(|task| task.title.clone())
}

/// (a) `send` enqueues and returns while the executor is still blocked.
#[test]
fn send_never_waits_for_the_executor() {
    let (mut transport, state) = transport();
    let main_thread = std::thread::current().id();
    let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
    state.lock().expect("stub state").gate = Some(Arc::clone(&gate));

    transport.send(hello_frame()).expect("hello enqueues");

    // The call returned while the executor is parked on the gate; release it.
    {
        let mut open = gate.0.lock().expect("gate");
        *open = true;
        gate.1.notify_all();
    }
    let frame = next_frame(&mut transport);
    assert_eq!(message_kind(&decoded(&frame)), "HELLO_ACCEPTED");
    let state = state.lock().expect("stub state");
    assert!(!state.threads.is_empty());
    assert!(
        state.threads.iter().all(|thread| *thread != main_thread),
        "the executor runs only on the transport thread"
    );
}

/// (b) The handshake, grant and snapshot frames arrive in protocol order.
#[test]
fn frames_arrive_in_protocol_order() {
    let (mut transport, state) = transport();
    transport.send(hello_frame()).expect("hello enqueues");
    let hello = decoded(&next_frame(&mut transport));
    assert!(matches!(hello, ServerWire::HelloAccepted(_)));

    transport
        .send(open_frame(open_request("request-1")))
        .expect("open enqueues");
    let kinds: Vec<String> = std::iter::from_fn(|| Some(next_frame(&mut transport)))
        .take(4)
        .map(|frame| message_kind(&decoded(&frame)))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "READ_GRANT".to_owned(),
            "SNAPSHOT_BEGIN".to_owned(),
            "SNAPSHOT_CHUNK".to_owned(),
            "SNAPSHOT_END".to_owned(),
        ]
    );

    let state = state.lock().expect("stub state");
    assert_eq!(state.calls_to(HttpMethod::Get, SERVER_PATH), 1);
    assert_eq!(state.calls_to(HttpMethod::Post, OBSERVATION_PATH), 1);
    let post = state
        .calls
        .iter()
        .find(|call| call.method == HttpMethod::Post)
        .expect("observation post");
    let body: serde_json::Value = serde_json::from_slice(&post.body).expect("request body");
    assert_eq!(body["schema_version"], serde_json::json!(1));
    assert_eq!(body["request_id"], serde_json::json!("request-1"));
    assert_eq!(
        body["protocol"],
        serde_json::json!({"major": 2, "minor": 0})
    );
    assert_eq!(body["schema"], serde_json::json!({"major": 2, "minor": 0}));
}

/// (c) The server identity is unknown until the handshake is accepted.
#[test]
fn authenticated_server_follows_the_handshake() {
    let (mut transport, _state) = transport();
    assert!(transport.authenticated_server().is_none());
    transport.send(hello_frame()).expect("hello enqueues");
    let _ = next_frame(&mut transport);
    assert_eq!(
        transport.authenticated_server(),
        Some(ServerId::new(SERVER_IDENTITY).expect("server id"))
    );
}

/// (d) A real client reaches a published snapshot through the transport.
#[test]
fn cos_client_reaches_a_published_snapshot_through_the_rest_transport() {
    let (transport, state) = transport();
    let mut client = client(transport);
    let request = client.observation_request(ObservationScopeRequest {
        task_ids: Vec::new(),
        filters: vec![ScopeFilter {
            name: "view".to_owned(),
            value: "all".to_owned(),
        }],
    });
    client
        .begin_authorized_observation(request)
        .expect("observation begins");

    let events = drive_until(&mut client, |client| {
        matches!(client.connection(), ConnectionState::Live)
    });
    assert!(matches!(client.connection(), ConnectionState::Live));
    assert_eq!(task_title(&client), Some("REST task 1".to_owned()));
    assert_eq!(
        client.server_identity(),
        Some(&ServerId::new(SERVER_IDENTITY).expect("server id"))
    );
    assert_eq!(
        client
            .last_seen_cursor()
            .map(desktop_protocol::Cursor::as_str),
        Some("cursor-1")
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::HandshakeCompleted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::SnapshotApplied { .. }))
    );

    let state = state.lock().expect("stub state");
    assert_eq!(state.calls_to(HttpMethod::Get, SERVER_PATH), 1);
    assert_eq!(state.calls_to(HttpMethod::Post, OBSERVATION_PATH), 1);
    assert!(
        state
            .calls
            .iter()
            .any(|call| call.path.starts_with(DELTAS_PATH))
    );
}

/// (e) A delta served by the delta poll is validated and applied.
#[test]
fn cos_client_applies_a_delta_from_the_delta_poll() {
    let (executor, state) = StubHttp::new();
    state.lock().expect("stub state").delta = true;
    let transport = ProtocolTransport::new(executor, config(), versions());
    let mut client = client(transport);
    client
        .begin_authorized_observation(client.observation_request(ObservationScopeRequest {
            task_ids: Vec::new(),
            filters: vec![ScopeFilter {
                name: "view".to_owned(),
                value: "all".to_owned(),
            }],
        }))
        .expect("observation begins");

    let events = drive_until(&mut client, |client| {
        task_title(client).as_deref() == Some("REST task updated")
    });
    assert_eq!(
        client.last_seen_cursor().map(|c| c.as_str()),
        Some("cursor-delta-1")
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
    );

    let delta_calls = {
        let state = state.lock().expect("stub state");
        state
            .calls
            .iter()
            .filter(|call| call.path.starts_with(DELTAS_PATH))
            .count()
    };
    assert!(delta_calls >= 1);
}

/// (f) `409 RESYNC_REQUIRED` publishes no frame and leaves resync to the client.
#[test]
fn delta_poll_409_leaves_resync_to_the_client() {
    let (executor, state) = StubHttp::new();
    state.lock().expect("stub state").resync_polls = 1;
    let transport = ProtocolTransport::new(executor, config(), versions());
    let mut client = client(transport);
    client
        .begin_authorized_observation(client.observation_request(ObservationScopeRequest {
            task_ids: Vec::new(),
            filters: vec![ScopeFilter {
                name: "view".to_owned(),
                value: "all".to_owned(),
            }],
        }))
        .expect("observation begins");

    let events = drive_until(&mut client, |client| {
        task_title(client).as_deref() == Some("REST task 2")
    });
    assert!(matches!(client.connection(), ConnectionState::Live));
    assert!(
        events.iter().any(|event| matches!(
            event,
            ClientEvent::Error(cos_client::ClientError::Transport(message))
                if message.contains("RESYNC_REQUIRED")
        )),
        "the client observes the transport failure"
    );
    let state = state.lock().expect("stub state");
    assert_eq!(
        state.calls_to(HttpMethod::Post, OBSERVATION_PATH),
        2,
        "the client re-opens the observation instead of guessing a cursor"
    );
    assert_eq!(task_title(&client), Some("REST task 2".to_owned()));
}

/// (g) The bounded inbound queue reports an overflow instead of growing.
#[test]
fn bounded_inbound_queue_reports_overflow() {
    let (executor, _state) = StubHttp::new();
    let mut config = config();
    config.queue_limits.max_queue_frames = 2;
    let mut transport = ProtocolTransport::new(executor, config, versions());

    transport.send(hello_frame()).expect("hello enqueues");
    transport
        .send(open_frame(open_request("request-1")))
        .expect("open enqueues");

    let mut frames = 0;
    let mut overflowed = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match transport.poll() {
            Ok(Some(_)) => frames += 1,
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
            Err(TransportError::QueueOverflow) => {
                overflowed = true;
                break;
            }
            Err(error) => panic!("unexpected transport error: {error:?}"),
        }
    }
    assert!(overflowed, "the bounded queue reports its overflow");
    assert!(frames <= 2, "at most the queue bound was delivered");
}

/// (h) `reset_stream` drops frames and discards the stale generation.
#[test]
fn reset_stream_discards_the_stale_generation() {
    let (mut transport, state) = transport();
    let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
    state.lock().expect("stub state").gate = Some(Arc::clone(&gate));
    let before = transport.generation();

    transport.send(hello_frame()).expect("hello enqueues");
    // Wait until the executor is inside the stale exchange.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !state.lock().expect("stub state").calls.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    transport.reset_stream();
    assert_eq!(transport.generation(), before + 1);
    {
        let mut open = gate.0.lock().expect("gate");
        *open = true;
        gate.1.notify_all();
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(matches!(transport.poll(), Ok(None)));
    assert!(
        transport.authenticated_server().is_none(),
        "a discarded handshake never authenticates the stream"
    );
}

/// (i) A non-observation client message is an explicit error, never silence.
#[test]
fn unsupported_client_frame_is_an_explicit_error() {
    let (mut transport, state) = transport();
    let error = transport
        .send(command_frame())
        .expect_err("commands are not carried by this transport");
    assert!(
        matches!(&error, TransportError::Io(message) if message.contains("unsupported client frame")),
        "{error:?}"
    );
    assert!(state.lock().expect("stub state").calls.is_empty());
}
