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
    COMMANDS_LOOKUP_PATH, COMMANDS_PATH, ClientConfig, ClientEvent, ClientSource, CommandLookup,
    CommandState, ConnectionState, CosClient, DELTAS_PATH, HttpMethod, HttpRequest, HttpResponse,
    OBSERVATION_PATH, ProtocolHttp, ProtocolTransport, ProtocolTransportConfig, PublicationAccess,
    SERVER_PATH, SNAPSHOT_PATH, Transport, TransportError,
};
use desktop_protocol::{
    ArtifactDigest, AuthorizedReceiptLookup, ClientFrame, ClientMessage, ClientWire,
    CommandEnvelope, CommandId, CommandOperation, CommandPayload, CommandTarget, Counter,
    CreateTaskCommand, Cursor, DeltaTransaction, EpochId, ErrorCode, GraphId, GraphRef,
    HelloAccepted, HelloRequest, Limits, ObservationScope, ObservationScopeRequest,
    OpenAuthorizedObservation, OpenObservation, PhaseLabel, Projection, ProjectionMutation,
    ProtocolVersions, ReadContextId, ReadGrant, ReadRequestId, ReadRevision, ReadStamp,
    ReceiptReadId, RecipeId, RecipeRef, ResyncReason, ScopeFilter, ScopeId, ServerId,
    ServerMessage, ServerWire, SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotEnd,
    SnapshotId, StopTaskCommand, TaskId, TaskSnapshot, TaskStatus, TransactionId, Version,
    VersionRange, projection_bundle_digest,
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
    /// Answer the next observation open with a `409 RESYNC_REQUIRED` document.
    resync_open: bool,
    /// Answer a resume open with a replay acceptance instead of a snapshot.
    replay_open: bool,
    /// Reason copied into the next `409 RESYNC_REQUIRED` document.
    resync_reason: Option<String>,
    /// Answer the next fresh-snapshot read with `401 UNAUTHORIZED_CONTEXT`.
    refresh_refused: bool,
    /// Fresh-snapshot reads served, and the scope/context the grant described.
    refresh_served: u32,
    granted_scope: Option<ObservationScope>,
    granted_context: Option<ReadContextId>,
    granted_stamp: Option<ReadStamp>,
    /// Serve one delta transaction on the next successful poll.
    delta: bool,
    /// Next command response: a receipt document override, or a refusal.
    command_receipt: Option<serde_json::Value>,
    command_error: Option<(u16, Vec<u8>)>,
    /// Stamp a receipt with a foreign context to prove it is withheld.
    foreign_receipt_stamp: bool,
    /// Receipt-lookup outcome mode and next refusal override.
    lookup_mode: LookupMode,
    lookup_error: Option<(u16, Vec<u8>)>,
    next_session: u64,
    next_snapshot: u64,
    cursor: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LookupMode {
    AllowedSome,
    AllowedNone,
    Denied,
}

impl ServerState {
    fn new() -> Self {
        Self {
            calls: Vec::new(),
            threads: Vec::new(),
            gate: None,
            resync_polls: 0,
            resync_open: false,
            replay_open: false,
            resync_reason: None,
            refresh_refused: false,
            refresh_served: 0,
            granted_scope: None,
            granted_context: None,
            granted_stamp: None,
            delta: false,
            command_receipt: None,
            command_error: None,
            foreign_receipt_stamp: false,
            lookup_mode: LookupMode::AllowedNone,
            lookup_error: None,
            next_session: 0,
            next_snapshot: 0,
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
                if state.resync_open {
                    state.resync_open = false;
                    return Ok(resync_error(state.resync_reason.take()));
                }
                if state.replay_open && body.get("last_seen_cursor").is_some() {
                    state.replay_open = false;
                    return Ok(json_response(replay_document(&mut state, &body)));
                }
                Ok(json_response(open_document(&mut state, &body)))
            }
            (HttpMethod::Get, path) if path.starts_with(SNAPSHOT_PATH) => {
                if state.refresh_refused {
                    state.refresh_refused = false;
                    return Ok(HttpResponse {
                        status: 401,
                        body: br#"{"code":"UNAUTHORIZED_CONTEXT","detail":"context expired"}"#
                            .to_vec(),
                    });
                }
                Ok(json_response(snapshot_refresh_document(&mut state, path)))
            }
            (HttpMethod::Post, COMMANDS_PATH) => {
                if let Some((status, body)) = state.command_error.take() {
                    return Ok(HttpResponse { status, body });
                }
                let body: serde_json::Value = serde_json::from_slice(&request.body)
                    .map_err(|error| format!("command body: {error}"))?;
                let document = state
                    .command_receipt
                    .take()
                    .unwrap_or_else(|| command_receipt_document(&state, &body));
                Ok(json_response(document))
            }
            (HttpMethod::Post, COMMANDS_LOOKUP_PATH) => {
                if let Some((status, body)) = state.lookup_error.take() {
                    return Ok(HttpResponse { status, body });
                }
                let body: serde_json::Value = serde_json::from_slice(&request.body)
                    .map_err(|error| format!("lookup body: {error}"))?;
                Ok(json_response(lookup_result_document(&state, &body)))
            }
            (HttpMethod::Get, path) if path.starts_with(DELTAS_PATH) => {
                if state.resync_polls > 0 {
                    state.resync_polls -= 1;
                    return Ok(resync_error(state.resync_reason.take()));
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

/// An authorized command receipt echoing the request's identity and the grant.
fn command_receipt_document(state: &ServerState, body: &serde_json::Value) -> serde_json::Value {
    let command_id = body["command_id"].as_str().unwrap_or("command-unknown");
    let mut stamp = serde_json::to_value(state.granted_stamp.clone().expect("granted stamp"))
        .expect("stamp encodes");
    if state.foreign_receipt_stamp {
        stamp["context_id"] = serde_json::Value::String("context-foreign".to_owned());
    }
    let mut receipt = serde_json::json!({
        "command_id": command_id,
        "operation": match body["payload"]["kind"].as_str() {
            Some("CREATE_TASK") => "CREATE_TASK",
            Some("REUSE_TASK") => "REUSE_TASK",
            _ => "STOP_TASK",
        },
        "state": "ACCEPTED",
    });
    if let Some(task_id) = body["target"]["task_id"].as_str() {
        receipt["task_id"] = serde_json::Value::String(task_id.to_owned());
    }
    serde_json::json!({
        "schema_version": 1,
        "stamp": stamp,
        "permission": {"command_id": command_id, "readable": true},
        "receipt": receipt,
    })
}

/// One receipt-lookup result echoing the lookup's exact binding and identity.
fn lookup_result_document(state: &ServerState, body: &serde_json::Value) -> serde_json::Value {
    let command_id = body["command_id"].as_str().unwrap_or("command-unknown");
    let lookup_id = body["lookup_id"].as_str().unwrap_or("lookup-unknown");
    let stamp = body["stamp"].clone();
    let outcome = match state.lookup_mode {
        LookupMode::Denied => serde_json::json!({"outcome": "DENIED"}),
        LookupMode::AllowedNone => serde_json::json!({"outcome": "ALLOWED", "receipt": null}),
        LookupMode::AllowedSome => serde_json::json!({
            "outcome": "ALLOWED",
            "receipt": {
                "command_id": command_id,
                "operation": "STOP_TASK",
                "state": "ACCEPTED",
                "task_id": "task/Task-184",
            },
        }),
    };
    serde_json::json!({
        "schema_version": 1,
        "result": {
            "lookup_id": lookup_id,
            "stamp": stamp,
            "command_id": command_id,
            "outcome": outcome,
        },
    })
}

/// One closed Runtime resync refusal document.
fn resync_error(reason: Option<String>) -> HttpResponse {
    let mut value = serde_json::json!({
        "code": "RESYNC_REQUIRED",
        "detail": "history cannot be replayed",
    });
    if let Some(reason) = reason {
        value["reason"] = serde_json::Value::String(reason);
    }
    HttpResponse {
        status: 409,
        body: serde_json::to_vec(&value).expect("resync body"),
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

/// One session the stub minted, with the grant the Desktop client must accept.
struct GrantedSession {
    session: u64,
    scope: ObservationScope,
    stamp: ReadStamp,
}

fn mint_grant(state: &mut ServerState, body: &serde_json::Value) -> GrantedSession {
    let request_id = body["request_id"].as_str().expect("request id").to_owned();
    let protocol: Version =
        serde_json::from_value(body["protocol"].clone()).expect("request protocol");
    let schema: Version = serde_json::from_value(body["schema"].clone()).expect("request schema");
    let scope_request: ObservationScopeRequest =
        serde_json::from_value(body["scope"].clone()).expect("request scope");
    state.next_session += 1;
    let session = state.next_session;
    let scope = ObservationScope {
        scope_id: ScopeId::new(format!("scope-{session}")).expect("scope id"),
        task_ids: scope_request.task_ids.clone(),
        filters: scope_request.filters.clone(),
        protocol,
        schema,
    };
    let context_id = ReadContextId::new(format!("context-{session}")).expect("context id");
    let stamp = ReadStamp {
        request_id: ReadRequestId::new(request_id).expect("request id"),
        server_id: ServerId::new(SERVER_IDENTITY).expect("server id"),
        epoch: EpochId::new(EPOCH).expect("epoch"),
        context_id: context_id.clone(),
        read_revision: ReadRevision::new(Counter::new("1").expect("counter")),
        scope_id: scope.scope_id.clone(),
    };
    state.granted_scope = Some(scope.clone());
    state.granted_context = Some(context_id);
    state.granted_stamp = Some(stamp.clone());
    GrantedSession {
        session,
        scope,
        stamp,
    }
}

fn grant_value(session: &GrantedSession) -> serde_json::Value {
    serde_json::to_value(ReadGrant {
        stamp: session.stamp.clone(),
        scope: session.scope.clone(),
        local_intents_readable: false,
        command_reads: Vec::new(),
    })
    .expect("grant encodes")
}

/// One complete atomic snapshot document for a granted scope.
fn snapshot_value(
    state: &mut ServerState,
    scope: &ObservationScope,
    title: &str,
) -> serde_json::Value {
    state.next_snapshot += 1;
    let snapshot_id =
        SnapshotId::new(format!("snapshot-{}", state.next_snapshot)).expect("snapshot id");
    let cursor = format!("cursor-{}", state.next_snapshot);
    state.cursor.clone_from(&cursor);
    let mutations = task_mutations(title, "1");
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
    serde_json::json!({
        "begin": serde_json::to_value(begin).expect("begin encodes"),
        "chunks": chunks
            .iter()
            .map(|chunk| serde_json::to_value(chunk).expect("chunk encodes"))
            .collect::<Vec<_>>(),
        "end": serde_json::to_value(end).expect("end encodes"),
    })
}

fn open_document(state: &mut ServerState, body: &serde_json::Value) -> serde_json::Value {
    let session = mint_grant(state, body);
    let title = format!("REST task {}", session.session);
    let snapshot = snapshot_value(state, &session.scope, &title);
    serde_json::json!({
        "schema_version": 1,
        "server": serde_json::to_value(hello()).expect("hello encodes"),
        "grant": grant_value(&session),
        "snapshot": snapshot,
    })
}

/// One resume acceptance: a fresh grant plus the requested replay position.
fn replay_document(state: &mut ServerState, body: &serde_json::Value) -> serde_json::Value {
    let session = mint_grant(state, body);
    let requested = body["last_seen_cursor"]
        .as_str()
        .expect("requested resume cursor")
        .to_owned();
    state.cursor.clone_from(&requested);
    serde_json::json!({
        "schema_version": 1,
        "server": serde_json::to_value(hello()).expect("hello encodes"),
        "grant": grant_value(&session),
        "replay": {"cursor": requested},
    })
}

/// One explicit fresh-snapshot read for the scope the grant already authorized.
fn snapshot_refresh_document(state: &mut ServerState, path: &str) -> serde_json::Value {
    state.refresh_served += 1;
    let scope = state.granted_scope.clone().expect("granted scope");
    let requested = path
        .split("scope=")
        .nth(1)
        .expect("the refresh names its scope");
    assert_eq!(requested, scope.scope_id.as_str(), "scope query identity");
    let title = format!("REST task refreshed {}", state.refresh_served);
    let snapshot = snapshot_value(state, &scope, &title);
    serde_json::json!({
        "schema_version": 1,
        "snapshot": snapshot,
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

fn next_frame<E: ProtocolHttp>(transport: &mut ProtocolTransport<E>) -> Vec<u8> {
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

/// A client that reached Live over the stub, with the credential configured.
fn live_authorized_client() -> (
    CosClient<ProtocolTransport<StubHttp>>,
    Arc<Mutex<ServerState>>,
) {
    let (executor, state) = StubHttp::new();
    let config = ProtocolTransportConfig {
        bearer_token: Some("local-test-token".to_owned()),
        ..config()
    };
    let transport = ProtocolTransport::new(executor, config, versions());
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
    drive_until(&mut client, |client| {
        matches!(client.connection(), ConnectionState::Live)
    });
    (client, state)
}

fn command_state(
    client: &CosClient<ProtocolTransport<StubHttp>>,
    command_id: &CommandId,
) -> Option<CommandState> {
    match client.lookup_command(command_id) {
        CommandLookup::Found(record) => Some(record.state().clone()),
        CommandLookup::NotRetained => None,
    }
}

fn current_stamp(client: &CosClient<ProtocolTransport<StubHttp>>) -> ReadStamp {
    match client.publication_access() {
        PublicationAccess::Validated { stamp, .. } => stamp.clone(),
        PublicationAccess::Withheld { reason } => panic!("publication withheld: {reason:?}"),
    }
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

/// (f) A delta-gap resync becomes a protocol frame; the client's bounded
/// recovery is served by one explicit snapshot refresh under the same grant.
#[test]
fn delta_gap_resync_is_a_frame_and_the_next_open_refreshes_the_snapshot() {
    let (executor, state) = StubHttp::new();
    {
        let mut state = state.lock().expect("stub state");
        state.resync_polls = 1;
        state.resync_reason = Some("HISTORY_UNAVAILABLE".to_owned());
    }
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
        task_title(client).as_deref() == Some("REST task refreshed 1")
    });
    assert!(matches!(client.connection(), ConnectionState::Live));
    assert!(
        events.iter().any(|event| matches!(
            event,
            ClientEvent::ResyncRequested {
                reason: ResyncReason::HistoryUnavailable
            }
        )),
        "the client receives the closed resync reason, not an I/O failure"
    );
    let state = state.lock().expect("stub state");
    assert_eq!(
        state.calls_to(HttpMethod::Post, OBSERVATION_PATH),
        1,
        "the refresh must not mint a second observation session"
    );
    let refresh_calls: Vec<&HttpRequest> = state
        .calls
        .iter()
        .filter(|call| call.method == HttpMethod::Get && call.path.starts_with(SNAPSHOT_PATH))
        .collect();
    assert_eq!(refresh_calls.len(), 1, "one explicit snapshot refresh");
    assert!(
        refresh_calls[0].path.contains("scope=scope-1"),
        "the refresh names the granted scope: {}",
        refresh_calls[0].path
    );
    assert!(
        refresh_calls[0]
            .headers
            .iter()
            .any(|(name, value)| name == "x-cos-context" && value == "context-1"),
        "the refresh presents the grant context"
    );
}

/// (f2) A refused refresh falls back to a fresh observation open.
#[test]
fn refused_snapshot_refresh_falls_back_to_a_fresh_open() {
    let (executor, state) = StubHttp::new();
    {
        let mut state = state.lock().expect("stub state");
        state.resync_polls = 1;
        state.refresh_refused = true;
    }
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

    drive_until(&mut client, |client| {
        task_title(client).as_deref() == Some("REST task 2")
    });
    assert!(matches!(client.connection(), ConnectionState::Live));
    let state = state.lock().expect("stub state");
    assert_eq!(
        state.calls_to(HttpMethod::Post, OBSERVATION_PATH),
        2,
        "the refusal recovers through a fresh grant and snapshot"
    );
    let refresh_calls = state
        .calls
        .iter()
        .filter(|call| call.method == HttpMethod::Get && call.path.starts_with(SNAPSHOT_PATH))
        .count();
    assert_eq!(refresh_calls, 1, "exactly one refresh was attempted");
}

/// (f3) Resume coordinates from the client reach the Runtime request body.
#[test]
fn resume_coordinates_reach_the_observation_body() {
    let (mut transport, state) = transport();
    transport.send(hello_frame()).expect("hello enqueues");
    assert_eq!(
        message_kind(&decoded(&next_frame(&mut transport))),
        "HELLO_ACCEPTED"
    );
    let mut request = open_request("resume-wire");
    request.observation.scope_id = Some(ScopeId::new("scope-7").expect("scope id"));
    request.observation.last_seen_cursor = Some(Cursor::new("cursor-7").expect("cursor"));
    request.observation.epoch = Some(EpochId::new(EPOCH).expect("epoch"));
    transport.send(open_frame(request)).expect("open enqueues");
    let body = wait_for_open_body(&state);
    assert_eq!(body["scope_id"], "scope-7");
    assert_eq!(body["last_seen_cursor"], "cursor-7");
    assert_eq!(body["epoch"], EPOCH);
}

/// (f4) A fresh open keeps the pre-resume body shape byte-for-byte.
#[test]
fn fresh_open_body_omits_resume_coordinates() {
    let (mut transport, state) = transport();
    transport.send(hello_frame()).expect("hello enqueues");
    assert_eq!(
        message_kind(&decoded(&next_frame(&mut transport))),
        "HELLO_ACCEPTED"
    );
    transport
        .send(open_frame(open_request("fresh-wire")))
        .expect("open enqueues");
    let body = wait_for_open_body(&state);
    assert!(body.get("scope_id").is_none());
    assert!(body.get("last_seen_cursor").is_none());
    assert!(body.get("epoch").is_none());
}

/// (f4b) A partial resume triplet is normalised to a fresh open, never a 422.
#[test]
fn partial_resume_triplet_is_normalised_to_a_fresh_open() {
    let scope_id = ScopeId::new("scope-7").expect("scope id");
    let cursor = Cursor::new("cursor-7").expect("cursor");
    let epoch = EpochId::new(EPOCH).expect("epoch");
    let partial = [
        (Some(scope_id.clone()), None, None),
        (None, Some(cursor.clone()), None),
        (None, None, Some(epoch.clone())),
        (Some(scope_id.clone()), Some(cursor), None),
        (Some(scope_id), None, Some(epoch)),
    ];
    for (index, (scope, cursor, epoch)) in partial.into_iter().enumerate() {
        let (mut transport, state) = transport();
        transport.send(hello_frame()).expect("hello enqueues");
        assert_eq!(
            message_kind(&decoded(&next_frame(&mut transport))),
            "HELLO_ACCEPTED"
        );
        let mut request = open_request(&format!("partial-{index}"));
        request.observation.scope_id = scope;
        request.observation.last_seen_cursor = cursor;
        request.observation.epoch = epoch;
        transport.send(open_frame(request)).expect("open enqueues");
        let body = wait_for_open_body(&state);
        assert!(body.get("scope_id").is_none(), "case {index}: {body}");
        assert!(
            body.get("last_seen_cursor").is_none(),
            "case {index}: {body}"
        );
        assert!(body.get("epoch").is_none(), "case {index}: {body}");
    }
}

fn wait_for_open_body(state: &Arc<Mutex<ServerState>>) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let body = state
            .lock()
            .expect("stub state")
            .calls
            .iter()
            .rev()
            .find_map(|call| {
                (call.method == HttpMethod::Post && call.path == OBSERVATION_PATH)
                    .then(|| serde_json::from_slice::<serde_json::Value>(&call.body).ok())
                    .flatten()
            });
        if let Some(body) = body {
            return body;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("timed out waiting for the observation request body");
}

/// (f5) A replay acceptance publishes a stamped `REPLAY_ACCEPTED` frame.
#[test]
fn replay_acceptance_is_published_as_a_stamped_frame() {
    let (mut transport, state) = transport();
    state.lock().expect("stub state").replay_open = true;
    transport.send(hello_frame()).expect("hello enqueues");
    assert_eq!(
        message_kind(&decoded(&next_frame(&mut transport))),
        "HELLO_ACCEPTED"
    );
    let mut request = open_request("resume-replay");
    request.observation.scope_id = Some(ScopeId::new("scope-7").expect("scope id"));
    request.observation.last_seen_cursor = Some(Cursor::new("cursor-7").expect("cursor"));
    request.observation.epoch = Some(EpochId::new(EPOCH).expect("epoch"));
    transport.send(open_frame(request)).expect("open enqueues");

    let grant = decoded(&next_frame(&mut transport));
    let grant_stamp = match grant {
        ServerWire::Frame(frame) => match frame.message {
            ServerMessage::ReadGrant(grant) => {
                assert!(frame.read_stamp.is_none());
                grant.stamp
            }
            other => panic!("expected READ_GRANT, got {:?}", other.kind()),
        },
        other => panic!("expected a server frame, got {other:?}"),
    };
    let replay = decoded(&next_frame(&mut transport));
    match replay {
        ServerWire::Frame(frame) => {
            assert_eq!(frame.read_stamp.as_ref(), Some(&grant_stamp));
            match frame.message {
                ServerMessage::ReplayAccepted(payload) => {
                    assert_eq!(payload.cursor.as_str(), "cursor-7")
                }
                other => panic!("expected REPLAY_ACCEPTED, got {:?}", other.kind()),
            }
        }
        other => panic!("expected a server frame, got {other:?}"),
    }
}

/// (f6) A Runtime resync refusal maps to the protocol frame and its reason.
#[test]
fn resync_refusal_maps_to_a_frame_with_its_closed_reason() {
    let (mut transport, state) = transport();
    {
        let mut state = state.lock().expect("stub state");
        state.resync_open = true;
        state.resync_reason = Some("EPOCH_CHANGED".to_owned());
    }
    transport.send(hello_frame()).expect("hello enqueues");
    assert_eq!(
        message_kind(&decoded(&next_frame(&mut transport))),
        "HELLO_ACCEPTED"
    );
    transport
        .send(open_frame(open_request("resync-open")))
        .expect("open enqueues");
    match decoded(&next_frame(&mut transport)) {
        ServerWire::Frame(frame) => {
            assert!(frame.read_stamp.is_none());
            match frame.message {
                ServerMessage::ResyncRequired(payload) => {
                    assert_eq!(payload.reason, ResyncReason::EpochChanged);
                    assert_eq!(
                        payload.message.as_deref(),
                        Some("history cannot be replayed")
                    );
                }
                other => panic!("expected RESYNC_REQUIRED, got {:?}", other.kind()),
            }
        }
        other => panic!("expected a server frame, got {other:?}"),
    }
}

/// (f7) An absent or unrecognized reason falls back to a closed meaning.
#[test]
fn resync_without_a_recognized_reason_falls_back_closed() {
    for reason in [None, Some("BRAND_NEW_REASON".to_owned())] {
        let (mut transport, state) = transport();
        {
            let mut state = state.lock().expect("stub state");
            state.resync_open = true;
            state.resync_reason = reason;
        }
        transport.send(hello_frame()).expect("hello enqueues");
        assert_eq!(
            message_kind(&decoded(&next_frame(&mut transport))),
            "HELLO_ACCEPTED"
        );
        transport
            .send(open_frame(open_request("resync-fallback")))
            .expect("open enqueues");
        match decoded(&next_frame(&mut transport)) {
            ServerWire::Frame(frame) => match frame.message {
                ServerMessage::ResyncRequired(payload) => {
                    assert_eq!(payload.reason, ResyncReason::HistoryUnavailable)
                }
                other => panic!("expected RESYNC_REQUIRED, got {:?}", other.kind()),
            },
            other => panic!("expected a server frame, got {other:?}"),
        }
    }
}

/// (f8) The retained-command list is bounded at the frozen 448 identities.
#[test]
fn retained_commands_above_the_contract_bound_are_refused() {
    let (mut transport, state) = transport();
    let mut oversized = open_request("retained-449");
    oversized.retained_commands = (0..449)
        .map(|index| CommandId::new(format!("command-{index}")).expect("command id"))
        .collect();
    let error = transport
        .send(open_frame(oversized))
        .expect_err("449 retained identities are over the contract bound");
    match error {
        TransportError::Io(message) => assert!(message.contains("448"), "{message}"),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(
        state.lock().expect("stub state").calls.is_empty(),
        "nothing reached the executor"
    );

    let mut repeated = open_request("retained-duplicate");
    repeated.retained_commands = vec![
        CommandId::new("command-1").expect("command id"),
        CommandId::new("command-1").expect("command id"),
    ];
    let error = transport
        .send(open_frame(repeated))
        .expect_err("a duplicate retained identity is refused");
    match error {
        TransportError::Io(message) => assert!(message.contains("repeats"), "{message}"),
        other => panic!("unexpected error: {other:?}"),
    }

    let mut allowed = open_request("retained-448");
    allowed.retained_commands = (0..448)
        .map(|index| CommandId::new(format!("command-{index}")).expect("command id"))
        .collect();
    transport
        .send(open_frame(allowed))
        .expect("448 distinct identities are admitted");
}

/// (f9) A Runtime 422 is classified by status and code without its body text.
#[test]
fn runtime_422_is_classified_without_its_body_text() {
    let hello = json_http(server_document(&ServerState::new()));
    let rejected = http(
        422,
        br#"{"code":"UNSUPPORTED_PROTOCOL","detail":"SECRET-MARKER"}"#.to_vec(),
    );
    let mut transport = scripted(vec![hello, rejected]);
    transport.send(hello_frame()).expect("hello enqueues");
    assert_eq!(
        message_kind(&decoded(&next_frame(&mut transport))),
        "HELLO_ACCEPTED"
    );
    transport
        .send(open_frame(open_request("rejected-open")))
        .expect("open enqueues");
    match next_error(&mut transport) {
        TransportError::Io(message) => {
            assert!(message.contains("422"), "{message}");
            assert!(message.contains("UNSUPPORTED_PROTOCOL"), "{message}");
            assert!(!message.contains("SECRET-MARKER"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
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

/// RC-03: reads are principal-bound, so the Desktop credential is presented on
/// the observation open and the delta read, and never on the handshake.
#[test]
fn the_credential_is_presented_on_the_protocol_reads_only() {
    let (executor, state) = StubHttp::new();
    let config = ProtocolTransportConfig {
        bearer_token: Some("local-test-token".to_owned()),
        delta_poll_interval: Duration::from_millis(1),
        ..config()
    };
    let mut transport = ProtocolTransport::new(executor, config, versions());
    transport.send(hello_frame()).expect("hello enqueues");
    let frame = next_frame(&mut transport);
    assert_eq!(message_kind(&decoded(&frame)), "HELLO_ACCEPTED");
    transport
        .send(open_frame(open_request("request-1")))
        .expect("open enqueues");
    let kinds: Vec<String> = std::iter::from_fn(|| Some(next_frame(&mut transport)))
        .take(4)
        .map(|frame| message_kind(&decoded(&frame)))
        .collect();
    assert_eq!(
        kinds,
        [
            "READ_GRANT",
            "SNAPSHOT_BEGIN",
            "SNAPSHOT_CHUNK",
            "SNAPSHOT_END"
        ]
    );

    // Wait until the first delta poll has been issued.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let polled = state
            .lock()
            .expect("stub state")
            .calls
            .iter()
            .any(|call| call.path.starts_with(DELTAS_PATH));
        if polled {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let calls = state.lock().expect("stub state").calls.clone();
    let credential = |call: &HttpRequest| -> Option<String> {
        call.headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone())
    };
    let handshake = calls
        .iter()
        .find(|call| call.path == SERVER_PATH)
        .expect("handshake request");
    assert_eq!(
        credential(handshake),
        None,
        "the handshake route never carries the credential"
    );
    let open = calls
        .iter()
        .find(|call| call.path == OBSERVATION_PATH)
        .expect("observation open request");
    assert_eq!(
        credential(open).as_deref(),
        Some("Bearer local-test-token"),
        "the observation open presents the credential"
    );
    let deltas = calls
        .iter()
        .find(|call| call.path.starts_with(DELTAS_PATH))
        .expect("delta read request");
    assert_eq!(
        credential(deltas).as_deref(),
        Some("Bearer local-test-token"),
        "the delta read presents the credential"
    );
}

/// (i) A malformed client frame is an explicit error, never silence.
#[test]
fn malformed_client_frame_is_an_explicit_error() {
    let (mut transport, state) = transport();
    let error = transport
        .send(b"{\"kind\":\"FRAME\"}".to_vec())
        .expect_err("a malformed client frame is rejected");
    assert!(
        matches!(&error, TransportError::Io(message) if message.contains("malformed client frame")),
        "{error:?}"
    );
    assert!(state.lock().expect("stub state").calls.is_empty());
}

/// (i2) A command frame from another version pair is refused before any exchange.
#[test]
fn command_with_another_version_pair_is_refused() {
    let (mut transport, state) = transport();
    let mut frame: serde_json::Value = serde_json::from_slice(&command_frame()).expect("frame");
    frame["protocol"] = serde_json::json!({"major": 1, "minor": 0});
    let error = transport
        .send(serde_json::to_vec(&frame).expect("frame encodes"))
        .expect_err("foreign versions are refused");
    assert!(
        matches!(&error, TransportError::Io(message) if message.contains("REST transport serves")),
        "{error:?}"
    );
    assert!(state.lock().expect("stub state").calls.is_empty());
}

// --- S2/3-REPAIR regression tests -----------------------------------------

/// Scripted executor: consumes canned responses in order, then repeats the last.
struct ScriptedHttp {
    responses: Vec<HttpResponse>,
}

impl ProtocolHttp for ScriptedHttp {
    fn execute(&mut self, _request: HttpRequest) -> Result<HttpResponse, String> {
        if self.responses.len() > 1 {
            Ok(self.responses.remove(0))
        } else {
            Ok(self.responses[0].clone())
        }
    }
}

fn http(status: u16, body: Vec<u8>) -> HttpResponse {
    HttpResponse { status, body }
}

fn json_http(value: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        body: serde_json::to_vec(&value).expect("response encodes"),
    }
}

/// A 4xx/5xx document far wider than the configured response bound.
fn oversized_error(status: u16) -> HttpResponse {
    HttpResponse {
        status,
        body: vec![b'{'; 8192],
    }
}

fn scripted(responses: Vec<HttpResponse>) -> ProtocolTransport<ScriptedHttp> {
    let config = ProtocolTransportConfig {
        delta_poll_interval: Duration::from_millis(1),
        max_response_bytes: 4096,
        ..ProtocolTransportConfig::default()
    };
    ProtocolTransport::new(ScriptedHttp { responses }, config, versions())
}

fn next_error<E: ProtocolHttp>(transport: &mut ProtocolTransport<E>) -> TransportError {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match transport.poll() {
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => return error,
        }
    }
    panic!("timed out waiting for a transport error");
}

fn open_document_for(request_id: &str) -> serde_json::Value {
    let mut state = ServerState::new();
    let body = serde_json::json!({
        "request_id": request_id,
        "protocol": {"major": 2, "minor": 0},
        "schema": {"major": 2, "minor": 0},
        "scope": {"task_ids": [], "filters": [{"name": "view", "value": "all"}]},
    });
    open_document(&mut state, &body)
}

/// A-01: `{:?}` never prints the bearer token or the authorization value.
#[test]
fn debug_output_redacts_transport_credentials() {
    const TOKEN: &str = "token-that-must-never-leak";

    let config = ProtocolTransportConfig {
        bearer_token: Some(TOKEN.to_owned()),
        ..ProtocolTransportConfig::default()
    };
    let rendered = format!("{config:?}");
    assert!(!rendered.contains(TOKEN), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
    assert!(
        rendered.contains("delta_poll_interval"),
        "non-secret fields stay visible: {rendered}"
    );

    let request = HttpRequest {
        method: HttpMethod::Post,
        path: "/api/cos/desktop/observation".to_owned(),
        headers: vec![
            ("accept".to_owned(), "application/json".to_owned()),
            ("authorization".to_owned(), format!("Bearer {TOKEN}")),
        ],
        body: b"{}".to_vec(),
    };
    let rendered = format!("{request:?}");
    assert!(!rendered.contains(TOKEN), "{rendered}");
    assert!(rendered.contains("Bearer <redacted>"), "{rendered}");
    assert!(rendered.contains("application/json"), "{rendered}");

    // A non-bearer authorization value is redacted as well.
    let basic = HttpRequest {
        method: HttpMethod::Get,
        path: "/api/cos/desktop/server".to_owned(),
        headers: vec![("authorization".to_owned(), format!("Basic {TOKEN}"))],
        body: Vec::new(),
    };
    let rendered = format!("{basic:?}");
    assert!(!rendered.contains(TOKEN), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
}

/// A-02: a reset stream must forget the old stream's verified server identity.
#[test]
fn reset_stream_forgets_the_authenticated_server() {
    let (mut transport, _state) = transport();
    transport.send(hello_frame()).expect("hello enqueues");
    let frame = next_frame(&mut transport);
    assert_eq!(message_kind(&decoded(&frame)), "HELLO_ACCEPTED");
    assert!(transport.authenticated_server().is_some());

    transport.reset_stream();
    assert!(
        transport.authenticated_server().is_none(),
        "a new stream must re-authenticate before any server statement is trusted"
    );
    // The old stream's frames are gone with it.
    assert!(matches!(transport.poll(), Ok(None)));
}

/// A-03: the byte bound applies to error documents before any JSON parsing.
#[test]
fn oversized_error_documents_are_bounded_before_parsing() {
    // Handshake path.
    let mut handshake = scripted(vec![oversized_error(500)]);
    handshake.send(hello_frame()).expect("hello enqueues");
    let error = next_error(&mut handshake);
    assert!(
        matches!(&error, TransportError::Io(message) if message.contains("bounded")),
        "{error:?}"
    );

    // Observation-open path.
    let hello = server_document(&ServerState::new());
    let mut open = scripted(vec![json_http(hello), oversized_error(422)]);
    open.send(hello_frame()).expect("hello enqueues");
    let frame = next_frame(&mut open);
    assert_eq!(message_kind(&decoded(&frame)), "HELLO_ACCEPTED");
    open.send(open_frame(open_request("request-1")))
        .expect("open enqueues");
    let error = next_error(&mut open);
    assert!(
        matches!(&error, TransportError::Io(message) if message.contains("bounded")),
        "{error:?}"
    );

    // Delta-poll path.
    let hello = server_document(&ServerState::new());
    let mut deltas = scripted(vec![
        json_http(hello),
        json_http(open_document_for("request-1")),
        oversized_error(500),
    ]);
    deltas.send(hello_frame()).expect("hello enqueues");
    let frame = next_frame(&mut deltas);
    assert_eq!(message_kind(&decoded(&frame)), "HELLO_ACCEPTED");
    deltas
        .send(open_frame(open_request("request-1")))
        .expect("open enqueues");
    let kinds: Vec<String> = std::iter::from_fn(|| Some(next_frame(&mut deltas)))
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
    let error = next_error(&mut deltas);
    assert!(
        matches!(&error, TransportError::Io(message) if message.contains("bounded")),
        "{error:?}"
    );
}

/// A-03 control: a small error document still reaches the status classifier.
#[test]
fn small_error_documents_still_report_their_status_and_code() {
    let mut transport = {
        let config = ProtocolTransportConfig {
            max_response_bytes: 4096,
            ..ProtocolTransportConfig::default()
        };
        let responses = vec![http(500, br#"{"code":"DIGEST_UNAVAILABLE"}"#.to_vec())];
        ProtocolTransport::new(ScriptedHttp { responses }, config, versions())
    };
    transport.send(hello_frame()).expect("hello enqueues");
    let error = next_error(&mut transport);
    assert!(
        matches!(
            &error,
            TransportError::Io(message)
                if message.contains("HTTP 500") && message.contains("DIGEST_UNAVAILABLE")
        ),
        "{error:?}"
    );
}

// --- RT4 command and receipt channel ---------------------------------------

/// (c1) A stop command reaches the Runtime and its authorized receipt settles it.
#[test]
fn command_submission_posts_the_envelope_and_applies_a_receipt() {
    let (mut client, state) = live_authorized_client();
    let task_id = TaskId::new("task/Task-184").expect("task id");
    let command_id = client.stop_task(&task_id).expect("command queued");

    let events = drive_until(&mut client, |client| {
        matches!(
            command_state(client, &command_id),
            Some(CommandState::Accepted(_))
        )
    });
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::CommandUpdated { command_id: updated, .. } if updated == &command_id
    )));

    let state = state.lock().expect("stub state");
    let calls: Vec<&HttpRequest> = state
        .calls
        .iter()
        .filter(|call| call.method == HttpMethod::Post && call.path == COMMANDS_PATH)
        .collect();
    assert_eq!(calls.len(), 1, "one command exchange");
    let body: serde_json::Value = serde_json::from_slice(&calls[0].body).expect("command body");
    assert_eq!(body["command_id"], command_id.as_str());
    assert_eq!(body["payload"]["kind"], "STOP_TASK");
    assert!(
        calls[0]
            .headers
            .iter()
            .any(|(name, value)| name == "authorization" && value == "Bearer local-test-token"),
        "the command carries the credential"
    );
    assert!(
        calls[0]
            .headers
            .iter()
            .any(|(name, value)| name == "x-cos-context" && value == "context-1"),
        "the command is bound to the current read context"
    );
}

/// (c2) A receipt whose stamp is not the client's binding is withheld.
#[test]
fn receipt_with_a_foreign_stamp_is_withheld() {
    let (mut client, state) = live_authorized_client();
    state.lock().expect("stub state").foreign_receipt_stamp = true;
    let command_id = client
        .stop_task(&TaskId::new("task/Task-184").expect("task id"))
        .expect("command queued");
    drive_until(&mut client, |client| {
        matches!(
            client.publication_access(),
            PublicationAccess::Withheld { .. }
        )
    });
    assert!(
        !matches!(
            command_state(&client, &command_id),
            Some(CommandState::Accepted(_))
        ),
        "a foreign stamp never settles the command"
    );
    assert!(client.last_error().is_some());
}

/// (c3) Allowed{Some}, Allowed{None} and Denied are distinct outcomes.
#[test]
fn receipt_lookup_outcomes_are_distinct() {
    // Allowed with a receipt.
    let (mut client, state) = live_authorized_client();
    state.lock().expect("stub state").lookup_mode = LookupMode::AllowedSome;
    let command_id = CommandId::new("command-lookup-some").expect("command id");
    client
        .lookup_receipt_authorized(AuthorizedReceiptLookup {
            lookup_id: ReceiptReadId::new("lookup-1").expect("lookup id"),
            stamp: current_stamp(&client),
            command_id: command_id.clone(),
        })
        .expect("lookup sent");
    drive_until(&mut client, |client| {
        matches!(
            command_state(client, &command_id),
            Some(CommandState::Accepted(_))
        )
    });

    // Allowed without a retained receipt: no lifecycle fact is fabricated.
    for (mode, label) in [
        (LookupMode::AllowedNone, "allowed-none"),
        (LookupMode::Denied, "denied"),
    ] {
        let (mut client, state) = live_authorized_client();
        state.lock().expect("stub state").lookup_mode = mode;
        let command_id = CommandId::new(format!("command-lookup-{label}")).expect("command id");
        client
            .lookup_receipt_authorized(AuthorizedReceiptLookup {
                lookup_id: ReceiptReadId::new(format!("lookup-{label}")).expect("lookup id"),
                stamp: current_stamp(&client),
                command_id: command_id.clone(),
            })
            .expect("lookup sent");
        let mut now = 0;
        for _ in 0..50 {
            now += 10;
            let _ = client.pump(now);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            command_state(&client, &command_id).is_none(),
            "{label}: no record is invented"
        );
        assert!(client.last_error().is_none(), "{label}: no error");
    }
}

/// (c4) Definitive refusals settle the command and preserve the current revision.
#[test]
fn definitive_refusals_settle_the_command() {
    for (body, expected) in [
        (
            serde_json::json!({"code": "COMMAND_ID_CONFLICT", "detail": "same id differs"}),
            ErrorCode::CommandIdConflict,
        ),
        (
            serde_json::json!({
                "code": "REVISION_CONFLICT",
                "detail": "stale revision",
                "current_revision": "7",
            }),
            ErrorCode::RevisionConflict,
        ),
    ] {
        let (mut client, state) = live_authorized_client();
        state.lock().expect("stub state").command_error =
            Some((409, serde_json::to_vec(&body).expect("refusal body")));
        let command_id = client
            .stop_task(&TaskId::new("task/Task-184").expect("task id"))
            .expect("command queued");
        let expected_code = expected.clone();
        drive_until(&mut client, |client| {
            matches!(
                command_state(client, &command_id),
                Some(CommandState::Failed { error }) if error.code == expected_code
            )
        });
        if expected == ErrorCode::RevisionConflict {
            match command_state(&client, &command_id) {
                Some(CommandState::Failed { error }) => assert_eq!(
                    error.current_revision,
                    Some(Counter::new("7").expect("counter"))
                ),
                other => panic!("expected a failed command, got {other:?}"),
            }
        }
    }
}

/// (c5) A lost command response keeps ResultUnknown; the command is never resent.
#[test]
fn lost_command_response_never_resends_the_command() {
    let (mut client, state) = live_authorized_client();
    state.lock().expect("stub state").command_error = Some((
        500,
        br#"{"code":"INTERNAL","detail":"lost response"}"#.to_vec(),
    ));
    let command_id = client
        .stop_task(&TaskId::new("task/Task-184").expect("task id"))
        .expect("command queued");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut now = 0;
    let mut saw_error = false;
    while Instant::now() < deadline && !saw_error {
        now += 10;
        saw_error = client
            .pump(now)
            .iter()
            .any(|event| matches!(event, ClientEvent::Error(_)));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(saw_error, "the client observes the lost response");
    drive_until(&mut client, |client| {
        matches!(client.connection(), ConnectionState::Live)
    });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        state
            .lock()
            .expect("stub state")
            .calls_to(HttpMethod::Post, COMMANDS_PATH),
        1,
        "a mutating command is never retried automatically"
    );
    assert!(
        !matches!(
            command_state(&client, &command_id),
            Some(CommandState::Failed { .. })
        ),
        "a lost response is not a definitive failure"
    );
}

/// (c6) An oversized command response is refused by the byte bound.
#[test]
fn oversized_command_response_is_refused_before_parsing() {
    let (executor, state) = StubHttp::new();
    let config = ProtocolTransportConfig {
        max_response_bytes: 8_000,
        bearer_token: Some("local-test-token".to_owned()),
        ..config()
    };
    let transport = ProtocolTransport::new(executor, config, versions());
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
    drive_until(&mut client, |client| {
        matches!(client.connection(), ConnectionState::Live)
    });
    state.lock().expect("stub state").command_error = Some((200, vec![b'x'; 9_000]));
    client
        .stop_task(&TaskId::new("task/Task-184").expect("task id"))
        .expect("command queued");
    drive_until(&mut client, |client| {
        client.last_error().is_some_and(|error| {
            matches!(error, cos_client::ClientError::Transport(message) if message.contains("exceeds"))
        })
    });
}

/// (c7) The create-task command family travels the same channel.
#[test]
fn create_task_command_reaches_the_runtime() {
    let (mut client, state) = live_authorized_client();
    let command_id = CommandId::new("command-create-1").expect("command id");
    let envelope = CommandEnvelope {
        command_id: command_id.clone(),
        target: CommandTarget::Creation {
            recipe_ref: RecipeRef {
                recipe_id: RecipeId::new("recipe-1").expect("recipe id"),
                graph_ref: GraphRef {
                    graph_id: GraphId::new("graph-1").expect("graph id"),
                    version: 1,
                    digest: ArtifactDigest::new(format!("sha256:{}", "a".repeat(64)))
                        .expect("digest"),
                },
                source_harness_version_id: None,
                source_harness_digest: None,
                profile_ref: None,
                projection_digest: None,
            },
        },
        expected_revision: None,
        epoch: None,
        payload: CommandPayload::CreateTask(CreateTaskCommand {
            objective_text: "observe the transport".to_owned(),
            text_prompt: None,
            reference_artifact_refs: Vec::new(),
            parameter_values: Vec::new(),
        }),
    };
    client.send_command(envelope).expect("command queued");
    drive_until(&mut client, |client| {
        matches!(
            command_state(client, &command_id),
            Some(CommandState::Accepted(receipt)) if receipt.operation == CommandOperation::CreateTask
        )
    });
    let state = state.lock().expect("stub state");
    let call = state
        .calls
        .iter()
        .find(|call| call.method == HttpMethod::Post && call.path == COMMANDS_PATH)
        .expect("command exchange");
    let body: serde_json::Value = serde_json::from_slice(&call.body).expect("command body");
    assert_eq!(body["target"]["target"], "CREATION");
    assert_eq!(body["payload"]["kind"], "CREATE_TASK");
    assert_eq!(body["payload"]["objective_text"], "observe the transport");
}

/// (c8) A FORBIDDEN command refusal withholds the session, not the command.
#[test]
fn forbidden_command_refusal_withholds_the_session() {
    let (mut client, state) = live_authorized_client();
    state.lock().expect("stub state").command_error =
        Some((403, br#"{"code":"FORBIDDEN"}"#.to_vec()));
    client
        .stop_task(&TaskId::new("task/Task-184").expect("task id"))
        .expect("command queued");
    drive_until(&mut client, |client| {
        matches!(
            client.publication_access(),
            PublicationAccess::Withheld { .. }
        )
    });
    assert!(matches!(
        client.connection(),
        ConnectionState::Unauthorized { .. }
    ));
}

/// (c9) UNAUTHORIZED_CONTEXT is classified as the session-wide Unauthorized class.
#[test]
fn unauthorized_context_is_classified_as_session_unauthorized() {
    let (mut client, state) = live_authorized_client();
    state.lock().expect("stub state").command_error = Some((
        401,
        br#"{"code":"UNAUTHORIZED_CONTEXT","detail":"context expired"}"#.to_vec(),
    ));
    client
        .stop_task(&TaskId::new("task/Task-184").expect("task id"))
        .expect("command queued");
    drive_until(&mut client, |client| {
        matches!(client.connection(), ConnectionState::Unauthorized { .. })
    });
}
