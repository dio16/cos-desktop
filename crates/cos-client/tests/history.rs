//! Wire tests for the authenticated bounded task history read (DC-MP-NAV3).
//!
//! The stub reproduces the Runtime observation open plus the task history route
//! so the real `CosClient` + plain-loopback `ProtocolTransport` are exercised:
//! the request must carry `Authorization` and `X-Cos-Context`, the page must be
//! bounded and closed-parsed, and every status class must map to its value-free
//! category. No test fabricates a page in fixture mode.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cos_client::{
    ClientConfig, ClientSource, ConnectionState, CosClient, DELTAS_PATH, DesktopClient,
    HistoryCursor, HistoryDocument, HistoryFetchError, HistoryItem, HistoryQuery, HttpMethod,
    HttpRequest, HttpResponse, OBSERVATION_PATH, ProtocolHttp, ProtocolTransport,
    ProtocolTransportConfig, SERVER_PATH,
};
use desktop_protocol::{
    Counter, Cursor, EpochId, HelloAccepted, Limits, ObservationScope, ObservationScopeRequest,
    OpenAuthorizedObservation, OpenObservation, PhaseLabel, Projection, ProjectionMutation,
    ProtocolVersions, ReadContextId, ReadGrant, ReadRequestId, ReadRevision, ReadStamp,
    ScopeFilter, ScopeId, ServerId, SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotEnd,
    SnapshotId, TaskId, TaskSnapshot, TaskStatus, Version, projection_bundle_digest,
};

const SERVER_IDENTITY: &str = "runtime-server";
const EPOCH: &str = "epoch-1";
const TASK: &str = "task/Task-184";

/// One scripted history answer.
#[derive(Clone)]
enum HistoryAnswer {
    /// A complete document served with `200`.
    Document(serde_json::Value),
    /// A raw status/body pair (refusals, undecodable and oversized bodies).
    Raw { status: u16, body: Vec<u8> },
}

struct StubState {
    calls: Vec<HttpRequest>,
    history: Option<HistoryAnswer>,
    granted_context: Option<ReadContextId>,
    session: u64,
}

impl StubState {
    fn history_calls(&self) -> Vec<&HttpRequest> {
        self.calls
            .iter()
            .filter(|call| {
                call.path.starts_with("/api/cos/desktop/tasks/")
                    && call
                        .path
                        .split('?')
                        .next()
                        .is_some_and(|route| route.ends_with("/history"))
            })
            .collect()
    }
}

#[derive(Clone)]
struct StubHttp {
    state: Arc<Mutex<StubState>>,
}

impl StubHttp {
    fn new() -> (Self, Arc<Mutex<StubState>>) {
        let state = Arc::new(Mutex::new(StubState {
            calls: Vec::new(),
            history: None,
            granted_context: None,
            session: 0,
        }));
        (
            Self {
                state: Arc::clone(&state),
            },
            state,
        )
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

fn server_document() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "server": serde_json::to_value(hello()).expect("hello encodes"),
    })
}

fn task_mutations() -> Vec<ProjectionMutation> {
    vec![ProjectionMutation::Upsert {
        base_revision: None,
        projection: Projection::Task(TaskSnapshot {
            task_ref: TaskId::new(TASK).expect("task id"),
            display_id: Some("Task-184".to_owned()),
            title: "REST task".to_owned(),
            task_status: TaskStatus::Active,
            inner_phase: Some(PhaseLabel::new("REFINE").expect("phase")),
            iteration_ref: None,
            iteration_index: None,
            best_candidate_ref: None,
            formal_outcome_ref: None,
            accepted_brief_ref: None,
            capabilities_ref: None,
            projection_revision: Counter::new("1").expect("revision"),
        }),
    }]
}

fn open_document(state: &mut StubState, body: &serde_json::Value) -> serde_json::Value {
    state.session += 1;
    let session = state.session;
    let request_id = body["request_id"].as_str().expect("request id").to_owned();
    let protocol: Version =
        serde_json::from_value(body["protocol"].clone()).expect("request protocol");
    let schema: Version = serde_json::from_value(body["schema"].clone()).expect("request schema");
    let scope_request: ObservationScopeRequest =
        serde_json::from_value(body["scope"].clone()).expect("request scope");
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
    state.granted_context = Some(context_id);
    let grant = ReadGrant {
        stamp,
        scope: scope.clone(),
        local_intents_readable: false,
        command_reads: Vec::new(),
    };
    let mutations = task_mutations();
    let snapshot_id = SnapshotId::new(format!("snapshot-{session}")).expect("snapshot id");
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
        scope,
        epoch: EpochId::new(EPOCH).expect("epoch"),
        cursor: Cursor::new(format!("cursor-{session}")).expect("cursor"),
        expected: SnapshotCounts::of(&mutations),
        integrity_digest: projection_bundle_digest(&mutations).expect("digest"),
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

impl ProtocolHttp for StubHttp {
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        let mut state = self.state.lock().expect("stub state");
        state.calls.push(request.clone());
        match (request.method, request.path.as_str()) {
            (HttpMethod::Get, SERVER_PATH) => Ok(json_response(server_document())),
            (HttpMethod::Post, OBSERVATION_PATH) => {
                let body: serde_json::Value = serde_json::from_slice(&request.body)
                    .map_err(|error| format!("request body: {error}"))?;
                Ok(json_response(open_document(&mut state, &body)))
            }
            (HttpMethod::Get, path) if path.starts_with(DELTAS_PATH) => {
                Ok(json_response(serde_json::json!({
                    "schema_version": 1,
                    "transaction": null,
                })))
            }
            (HttpMethod::Get, path)
                if path.starts_with("/api/cos/desktop/tasks/")
                    && path
                        .split('?')
                        .next()
                        .is_some_and(|route| route.ends_with("/history")) =>
            {
                match state.history.clone() {
                    Some(HistoryAnswer::Document(document)) => Ok(json_response(document)),
                    Some(HistoryAnswer::Raw { status, body }) => Ok(HttpResponse { status, body }),
                    None => Ok(HttpResponse {
                        status: 404,
                        body: br#"{"code":"NOT_FOUND"}"#.to_vec(),
                    }),
                }
            }
            _ => Ok(HttpResponse {
                status: 404,
                body: b"{}".to_vec(),
            }),
        }
    }
}

fn versions() -> ProtocolVersions {
    ProtocolVersions {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
    }
}

fn config(max_response_bytes: u64) -> ProtocolTransportConfig {
    ProtocolTransportConfig {
        bearer_token: Some("test-token".to_owned()),
        delta_poll_interval: Duration::from_millis(1),
        max_response_bytes,
        ..ProtocolTransportConfig::default()
    }
}

fn new_client() -> (
    CosClient<ProtocolTransport<StubHttp>>,
    Arc<Mutex<StubState>>,
) {
    let (executor, state) = StubHttp::new();
    let transport = ProtocolTransport::new(
        executor,
        config(ProtocolTransportConfig::default().max_response_bytes),
        versions(),
    );
    let client = CosClient::new(
        transport,
        ClientConfig::default(),
        Limits::DEFAULT,
        ClientSource::Unknown,
    );
    (client, state)
}

fn open_request() -> OpenAuthorizedObservation {
    OpenAuthorizedObservation {
        request_id: ReadRequestId::new("request-1").expect("request id"),
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

/// Drive the client to a live observation so a history read has a context.
fn connect(client: &mut CosClient<ProtocolTransport<StubHttp>>) {
    client
        .begin_authorized_observation(open_request())
        .expect("observation request accepted");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut now = 0u64;
    while Instant::now() < deadline {
        now += 10;
        client.pump(now);
        if client.connection() == &ConnectionState::Live {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("client did not reach a live observation");
}

fn task() -> TaskId {
    TaskId::new(TASK).expect("task id")
}

fn item(iteration: u64) -> HistoryItem {
    HistoryItem {
        task_ref: TASK.to_owned(),
        record_ref: format!("run-{iteration}:{iteration}"),
        run_id: format!("run-{iteration}"),
        iteration,
        phase: "REFINE".to_owned(),
        plan_id: String::new(),
        created_at: "2026-11-08T00:00:00Z".to_owned(),
    }
}

fn document(
    items: Vec<HistoryItem>,
    next_cursor: Option<&str>,
    has_more: bool,
) -> serde_json::Value {
    let context = "context-1";
    serde_json::json!({
        "schema_version": 1,
        "server": {"server_identity": SERVER_IDENTITY, "epoch": EPOCH},
        "stamp": {
            "request_id": "request-1",
            "server_id": SERVER_IDENTITY,
            "epoch": EPOCH,
            "context_id": context,
            "read_revision": "1",
            "scope_id": "scope-1",
        },
        "task_ref": TASK,
        "items": items,
        "next_cursor": next_cursor,
        "has_more": has_more,
    })
}

fn serve(state: &Arc<Mutex<StubState>>, answer: HistoryAnswer) {
    state.lock().expect("stub state").history = Some(answer);
}

#[test]
fn one_page_reads_with_bearer_context_and_parses_the_closed_document() {
    let (mut client, state) = new_client();
    connect(&mut client);
    serve(
        &state,
        HistoryAnswer::Document(document(vec![item(2), item(1)], Some("cursor-next"), true)),
    );
    let query = HistoryQuery::first(task());
    let page: HistoryDocument = client.fetch_history(&query).expect("page served");

    assert_eq!(page.schema_version, 1);
    assert_eq!(page.task_ref, TASK);
    assert_eq!(page.server.server_identity, SERVER_IDENTITY);
    assert_eq!(page.stamp.context_id, "context-1");
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].iteration, 2);
    assert_eq!(page.items[0].record_ref, "run-2:2");
    assert_eq!(page.items[0].plan_id, "", "an absent plan stays absent");
    assert_eq!(page.next_cursor.as_deref(), Some("cursor-next"));
    assert!(page.has_more);
    assert!(!page.is_empty_history());

    let calls = state.lock().expect("stub state");
    let history = calls.history_calls();
    assert_eq!(history.len(), 1);
    let call = history[0];
    assert_eq!(call.method, HttpMethod::Get);
    assert!(
        call.path
            .starts_with("/api/cos/desktop/tasks/task%2FTask-184/history?limit=20"),
        "the task reference is percent-encoded and the page is bounded: {}",
        call.path
    );
    let headers = |name: &str| {
        call.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        headers("authorization").as_deref(),
        Some("Bearer test-token"),
        "the configured credential is presented"
    );
    assert_eq!(
        headers("x-cos-context").as_deref(),
        Some("context-1"),
        "the live read context is presented"
    );
}

#[test]
fn an_empty_page_is_authoritative_and_offers_no_continuation() {
    let (mut client, state) = new_client();
    connect(&mut client);
    let mut empty = document(Vec::new(), None, false);
    if let Some(object) = empty.as_object_mut() {
        // The Runtime omits `next_cursor` when no continuation remains.
        object.remove("next_cursor");
    }
    serve(&state, HistoryAnswer::Document(empty));
    let page = client
        .fetch_history(&HistoryQuery::first(task()))
        .expect("empty page served");
    assert!(page.items.is_empty());
    assert!(!page.has_more);
    assert_eq!(page.next_cursor, None);
    assert!(page.is_empty_history());
    assert_eq!(
        HistoryQuery::first(task())
            .continuation(&page)
            .expect("no continuation"),
        None
    );
}

#[test]
fn the_server_cursor_is_used_verbatim_for_the_next_page() {
    let (mut client, state) = new_client();
    connect(&mut client);
    serve(
        &state,
        HistoryAnswer::Document(document(vec![item(3)], Some("u+next/page="), true)),
    );
    let first = HistoryQuery::first(task());
    let page = client.fetch_history(&first).expect("first page");
    let next = first
        .continuation(&page)
        .expect("continuation parses")
        .expect("a continuation exists");

    serve(
        &state,
        HistoryAnswer::Document(document(vec![item(2)], None, false)),
    );
    let last = client.fetch_history(&next).expect("second page");
    assert!(!last.has_more);
    assert_eq!(
        next.continuation(&last).expect("end has no continuation"),
        None
    );

    let calls = state.lock().expect("stub state");
    let history = calls.history_calls();
    assert_eq!(history.len(), 2);
    assert!(
        history[1].path.ends_with("&cursor=u%2Bnext%2Fpage%3D"),
        "the opaque cursor travels encoded, not reinterpreted: {}",
        history[1].path
    );
}

#[test]
fn a_bad_limit_or_cursor_is_refused_before_any_exchange() {
    let (mut client, state) = new_client();
    connect(&mut client);
    assert_eq!(
        HistoryQuery::new(task(), 21, None),
        Err(HistoryFetchError::InvalidRequest)
    );
    assert_eq!(
        HistoryQuery::new(task(), 0, None),
        Err(HistoryFetchError::InvalidRequest)
    );
    // A cursor is only representable once it satisfies the bound, so no caller
    // can hand the transport an empty, control-bearing or oversized one.
    assert_eq!(
        HistoryCursor::new("x".repeat(4096)),
        Err(HistoryFetchError::InvalidRequest)
    );
    assert_eq!(
        HistoryCursor::new(String::new()),
        Err(HistoryFetchError::InvalidRequest)
    );
    let before = state.lock().expect("stub state").history_calls().len();
    assert_eq!(before, 0, "no invalid request reached the transport");
}

#[test]
fn no_live_read_context_refuses_the_read_as_unauthorized() {
    let (mut client, state) = new_client();
    let error = client
        .fetch_history(&HistoryQuery::first(task()))
        .expect_err("no context");
    assert_eq!(error, HistoryFetchError::Unauthorized);
    assert!(state.lock().expect("stub state").history_calls().is_empty());
}

#[test]
fn every_closed_refusal_class_maps_without_copying_the_body() {
    let cases = [
        (401, "UNAUTHORIZED_CONTEXT", HistoryFetchError::Unauthorized),
        (403, "PRINCIPAL_MISMATCH", HistoryFetchError::Forbidden),
        (404, "NOT_FOUND", HistoryFetchError::NotFound),
        (409, "RESYNC_REQUIRED", HistoryFetchError::Conflict),
        (422, "VALIDATION_FAILED", HistoryFetchError::InvalidRequest),
        (413, "JSON_RESPONSE_TOO_LARGE", HistoryFetchError::Oversized),
        (500, "INTERNAL", HistoryFetchError::Unavailable),
    ];
    for (status, code, expected) in cases {
        let (mut client, state) = new_client();
        connect(&mut client);
        serve(
            &state,
            HistoryAnswer::Raw {
                status,
                body: serde_json::to_vec(&serde_json::json!({
                    "code": code,
                    "detail": "a runtime detail that must not be copied",
                }))
                .expect("refusal encodes"),
            },
        );
        let error = client
            .fetch_history(&HistoryQuery::first(task()))
            .expect_err("refused");
        assert_eq!(error, expected, "HTTP {status} {code}");
        let rendered = error.to_string();
        assert!(
            !rendered.contains("a runtime detail"),
            "no server detail is copied into the failure class"
        );
    }
}

#[test]
fn malformed_truncated_and_foreign_pages_are_refused() {
    // Undecodable body.
    let (mut client, state) = new_client();
    connect(&mut client);
    serve(
        &state,
        HistoryAnswer::Raw {
            status: 200,
            body: b"not a page".to_vec(),
        },
    );
    assert_eq!(
        client.fetch_history(&HistoryQuery::first(task())),
        Err(HistoryFetchError::Malformed)
    );

    // A page that claims more without a continuation is a truncation, never an
    // authoritative end.
    let (mut client, state) = new_client();
    connect(&mut client);
    let mut truncated = document(vec![item(1)], None, true);
    truncated["next_cursor"] = serde_json::Value::Null;
    serve(&state, HistoryAnswer::Document(truncated));
    assert_eq!(
        client.fetch_history(&HistoryQuery::first(task())),
        Err(HistoryFetchError::Malformed)
    );

    // A page for another task is not accepted for this request.
    let (mut client, state) = new_client();
    connect(&mut client);
    let mut foreign = document(vec![item(1)], None, false);
    foreign["task_ref"] = serde_json::Value::String("task/Other".to_owned());
    serve(&state, HistoryAnswer::Document(foreign));
    assert_eq!(
        client.fetch_history(&HistoryQuery::first(task())),
        Err(HistoryFetchError::Malformed)
    );

    // A page for another read context is not accepted either.
    let (mut client, state) = new_client();
    connect(&mut client);
    let mut foreign = document(vec![item(1)], None, false);
    foreign["stamp"]["context_id"] = serde_json::Value::String("context-foreign".to_owned());
    serve(&state, HistoryAnswer::Document(foreign));
    assert_eq!(
        client.fetch_history(&HistoryQuery::first(task())),
        Err(HistoryFetchError::Malformed)
    );

    // A closed document with an unknown member cannot be silently ignored.
    let (mut client, state) = new_client();
    connect(&mut client);
    let mut unknown = document(vec![item(1)], None, false);
    unknown["unexpected"] = serde_json::Value::Bool(true);
    serve(&state, HistoryAnswer::Document(unknown));
    assert_eq!(
        client.fetch_history(&HistoryQuery::first(task())),
        Err(HistoryFetchError::Malformed)
    );
}

#[test]
fn an_oversized_page_is_refused_before_parsing() {
    let (executor, state) = StubHttp::new();
    let transport = ProtocolTransport::new(executor, config(64 * 1024), versions());
    let mut client = CosClient::new(
        transport,
        ClientConfig::default(),
        Limits::DEFAULT,
        ClientSource::Unknown,
    );
    connect(&mut client);
    let oversized = vec![b'x'; 64 * 1024 + 1];
    serve(
        &state,
        HistoryAnswer::Raw {
            status: 200,
            body: oversized,
        },
    );
    assert_eq!(
        client.fetch_history(&HistoryQuery::first(task())),
        Err(HistoryFetchError::Oversized)
    );
}

#[test]
fn the_fixture_provider_refuses_history_instead_of_synthesizing_a_page() {
    let mut mock =
        cos_client::MockCosClient::from_fixture_str(include_str!("../fixtures/task-184.json"))
            .expect("fixture loads");
    assert_eq!(
        mock.fetch_history(&HistoryQuery::first(task())),
        Err(HistoryFetchError::Unsupported)
    );
}
