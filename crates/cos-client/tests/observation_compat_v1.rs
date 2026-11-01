//! MP-RT1: observation v1 compatibility tests over recorded Runtime responses.
//!
//! `fixtures/desktop-observation-v1/` is a byte-identical copy of the Runtime's
//! recording (`C:\AI\orchestrator\fixtures\desktop-observation-v1`, Runtime
//! commit `4b3e377d`). The replay stub serves those recorded documents by
//! method/path and drives the real [`CosClient`] through
//! [`ProtocolTransport`], exactly as it runs against the live adapter.
//!
//! Two session-correlation fields cannot be replayed verbatim and are re-echoed
//! the way the server contract defines them, never fabricated into content:
//!
//! - `grant.stamp.request_id`: the server echoes the client's request id, so the
//!   stub echoes the replayed client's id (the recording holds the recorder's),
//! - a `deltas-changed` transaction was captured against a **second** server
//!   copy, so its `epoch`/`predecessor` binding is rebound to the replayed
//!   session. The mutations, `mutation_count`, `integrity_digest`, transaction
//!   id and successor cursor stay recorded; the digest covers only the
//!   mutations, so rebinding the envelope cannot change what is verified.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cos_client::{
    ClientConfig, ClientError, ClientEvent, ClientSource, ConnectionState, CosClient, DELTAS_PATH,
    HttpMethod, HttpRequest, HttpResponse, OBSERVATION_PATH, ProtocolHttp, ProtocolTransport,
    ProtocolTransportConfig, PublicationAccess, ReadUnavailable, SERVER_PATH,
};
use desktop_protocol::{
    DeltaTransaction, HelloAccepted, Limits, ProjectionMutation, ProtocolVersions, ReadGrant,
    SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotEnd, Version, projection_bundle_digest,
};
use serde::Deserialize;

type CompatClient = CosClient<ProtocolTransport<ReplayHttp>>;

// --- recorded fixtures ----------------------------------------------------

/// A recorded document: its HTTP status and the verbatim JSON body.
#[derive(Clone)]
struct Recorded {
    status: u16,
    body: serde_json::Value,
}

fn recorded(name: &str) -> Recorded {
    let path = format!(
        "{}/fixtures/desktop-observation-v1/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("recorded fixture {name} is missing: {error}"));
    let value: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("recorded fixture {name} is not JSON: {error}"));
    assert_eq!(
        value["name"],
        serde_json::json!(name),
        "fixture {name} carries its scenario name"
    );
    let status = value["response"]["status"]
        .as_u64()
        .unwrap_or_else(|| panic!("fixture {name} declares a status")) as u16;
    Recorded {
        status,
        body: value["body"].clone(),
    }
}

/// The recorded copies must stay byte-identical to the Runtime recording; the
/// digests are the ones the Runtime's README publishes.
#[test]
fn recorded_fixture_copies_are_byte_identical_to_the_runtime_recording() {
    let expected = [
        (
            "hello.json",
            "b6c9fad7aa136ae24d9cf901f29d89fb9056665dfa93c6043d20756409ec1cc0",
        ),
        (
            "observation-grant.json",
            "01deb64d5e551e37c23e87d610db569001a1eb8cbe404445d93ce3be63a6a91b",
        ),
        (
            "snapshot.json",
            "616eba0dd8913877d68dba742c6b79f1d88358f406c8dffc3d7a66e86385b734",
        ),
        (
            "deltas-none.json",
            "946ae7fdf95ce2c79d8580c82f6678451a1eae70ce4821a134b4004e0f0f34a7",
        ),
        (
            "deltas-changed.json",
            "324f9f7d61a34cd2769ebcb3a4615cd5816b94a6ed639240c3a1a3ac4802976b",
        ),
        (
            "deltas-resync.json",
            "ff00e0a4edb19754f88b1d7c10b80e57c92c17bdd460f737c6cb0db6a5c3c298",
        ),
        (
            "context-missing.json",
            "5b42c018d039cc0cdc7e77a90e5fba5766d3daec3b02c64c1d3e313851d6b8fa",
        ),
        (
            "observation-unauthorized.json",
            "4ee1e255e4171afb7fb9c8f752908a1804e35de5a5f4b07348c733fe4161c88e",
        ),
        (
            "version-rejected.json",
            "28c3eb650ee6f6b5587dc81a8c2989162eccb066431a64113db14b4730a953ba",
        ),
    ];
    for (file, digest) in expected {
        let path = format!(
            "{}/fixtures/desktop-observation-v1/{file}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("recorded fixture {file} is missing: {error}"));
        assert_eq!(
            desktop_protocol::sha256_digest(&bytes).as_str(),
            format!("sha256:{digest}"),
            "recorded fixture {file} is an unmodified copy"
        );
    }
}

// --- replaying executor ----------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenReply {
    Grant,
    Unauthorized,
    VersionRejected,
}

#[derive(Clone)]
struct DeltasReply {
    recorded: Recorded,
    /// Rebind `epoch`/`predecessor` to the replayed session (the recording was
    /// captured on a second server copy).
    rebind: bool,
}

#[derive(Clone)]
struct Session {
    epoch: String,
    cursor: String,
}

struct ReplayState {
    calls: Vec<String>,
}

struct ReplayHttp {
    hello: Recorded,
    grant: Recorded,
    open_reply: OpenReply,
    unauthorized: Recorded,
    version_rejected: Recorded,
    deltas: VecDeque<DeltasReply>,
    session: Option<Session>,
    state: Arc<Mutex<ReplayState>>,
}

impl ReplayHttp {
    fn new(
        open_reply: OpenReply,
        deltas: Vec<DeltasReply>,
        state: Arc<Mutex<ReplayState>>,
    ) -> Self {
        Self {
            hello: recorded("hello"),
            grant: recorded("observation-grant"),
            open_reply,
            unauthorized: recorded("observation-unauthorized"),
            version_rejected: recorded("version-rejected"),
            deltas: deltas.into(),
            session: None,
            state,
        }
    }
}

impl ProtocolHttp for ReplayHttp {
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        let method = match request.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        };
        self.state
            .lock()
            .expect("replay state")
            .calls
            .push(format!("{method} {}", request.path));
        match (request.method, request.path.as_str()) {
            (HttpMethod::Get, SERVER_PATH) => Ok(as_response(&self.hello)),
            (HttpMethod::Post, OBSERVATION_PATH) => match self.open_reply {
                OpenReply::Grant => {
                    let request_body: serde_json::Value = serde_json::from_slice(&request.body)
                        .map_err(|error| format!("replayed open body: {error}"))?;
                    // The server echoes the client's request id, so a replay must
                    // echo the replayed client's id instead of the recorded one.
                    let mut document = self.grant.body.clone();
                    document["grant"]["stamp"]["request_id"] = request_body["request_id"].clone();
                    let epoch = document["server"]["epoch"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    let cursor = document["snapshot"]["end"]["cursor"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    self.session = Some(Session { epoch, cursor });
                    Ok(json_response(self.grant.status, document))
                }
                OpenReply::Unauthorized => Ok(as_response(&self.unauthorized)),
                OpenReply::VersionRejected => Ok(as_response(&self.version_rejected)),
            },
            (HttpMethod::Get, path) if path.starts_with(DELTAS_PATH) => {
                let reply = if self.deltas.len() > 1 {
                    self.deltas.pop_front().expect("queued delta reply")
                } else {
                    self.deltas
                        .front()
                        .expect("a delta reply is configured")
                        .clone()
                };
                let mut body = reply.recorded.body.clone();
                if reply.rebind {
                    let session = self.session.as_ref().expect("a session precedes a delta");
                    body["transaction"]["epoch"] = serde_json::json!(session.epoch);
                    body["transaction"]["predecessor"] = serde_json::json!(session.cursor);
                }
                Ok(json_response(reply.recorded.status, body))
            }
            _ => Ok(HttpResponse {
                status: 404,
                body: b"{}".to_vec(),
            }),
        }
    }
}

fn as_response(recorded: &Recorded) -> HttpResponse {
    json_response(recorded.status, recorded.body.clone())
}

fn json_response(status: u16, body: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        body: serde_json::to_vec(&body).expect("recorded body encodes"),
    }
}

// --- typed views of the recorded bodies -----------------------------------

#[derive(Deserialize)]
struct GrantDocument {
    schema_version: u32,
    server: HelloAccepted,
    grant: ReadGrant,
    snapshot: SnapshotDocument,
}

#[derive(Deserialize)]
struct SnapshotDocument {
    #[allow(dead_code)]
    begin: SnapshotBegin,
    chunks: Vec<SnapshotChunk>,
    end: SnapshotEnd,
}

#[derive(Deserialize)]
struct DeltasDocument {
    schema_version: u32,
    transaction: Option<DeltaTransaction>,
}

fn grant_document() -> GrantDocument {
    serde_json::from_value(recorded("observation-grant").body).expect("recorded grant decodes")
}

fn recorded_snapshot_mutations(snapshot: &SnapshotDocument) -> Vec<ProjectionMutation> {
    snapshot
        .chunks
        .iter()
        .flat_map(|chunk| chunk.mutations.clone())
        .collect()
}

fn recorded_delta() -> DeltaTransaction {
    let document: DeltasDocument =
        serde_json::from_value(recorded("deltas-changed").body).expect("recorded delta decodes");
    assert_eq!(document.schema_version, 1);
    document
        .transaction
        .expect("deltas-changed carries a transaction")
}

// --- client driver ---------------------------------------------------------

fn scope() -> desktop_protocol::ObservationScopeRequest {
    desktop_protocol::ObservationScopeRequest {
        task_ids: Vec::new(),
        filters: vec![desktop_protocol::ScopeFilter {
            name: "view".to_owned(),
            value: "all".to_owned(),
        }],
    }
}

fn client(
    open_reply: OpenReply,
    deltas: Vec<DeltasReply>,
    interval: Duration,
) -> (CompatClient, Arc<Mutex<ReplayState>>) {
    let state = Arc::new(Mutex::new(ReplayState { calls: Vec::new() }));
    let replay = ReplayHttp::new(open_reply, deltas, Arc::clone(&state));
    let transport = ProtocolTransport::new(
        replay,
        ProtocolTransportConfig {
            delta_poll_interval: interval,
            ..ProtocolTransportConfig::default()
        },
        ProtocolVersions {
            protocol: Version::new(2, 0),
            schema: Version::new(2, 0),
        },
    );
    (
        CosClient::new(
            transport,
            ClientConfig::default(),
            Limits::DEFAULT,
            ClientSource::Unknown,
        ),
        state,
    )
}

fn live(client: &CompatClient) -> bool {
    matches!(client.connection(), ConnectionState::Live)
}

fn wait_for(
    client: &mut CompatClient,
    now: &mut u64,
    mut condition: impl FnMut(&CompatClient) -> bool,
) -> Vec<ClientEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        *now += 10;
        events.extend(client.pump(*now));
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

fn pump_for(
    client: &mut CompatClient,
    now: &mut u64,
    span: Duration,
    step: u64,
) -> Vec<ClientEvent> {
    let deadline = Instant::now() + span;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        *now += step;
        events.extend(client.pump(*now));
        std::thread::sleep(Duration::from_millis(1));
    }
    events
}

fn transport_error_containing(events: &[ClientEvent], needle: &str) -> bool {
    events.iter().any(|event| {
        matches!(event, ClientEvent::Error(ClientError::Transport(message)) if message.contains(needle))
    })
}

fn snapshot_applied(events: &[ClientEvent]) -> Option<SnapshotCounts> {
    events.iter().find_map(|event| match event {
        ClientEvent::SnapshotApplied { counts, .. } => Some(*counts),
        _ => None,
    })
}

fn delta_applied(events: &[ClientEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::DeltaApplied {
                transaction_id,
                cursor,
            } => Some((transaction_id.to_string(), cursor.to_string())),
            _ => None,
        })
        .collect()
}

fn calls_matching(state: &Arc<Mutex<ReplayState>>, prefix: &str) -> usize {
    state
        .lock()
        .expect("replay state")
        .calls
        .iter()
        .filter(|call| call.starts_with(prefix))
        .count()
}

// --- accepted combination --------------------------------------------------

/// The recorded real grant reaches Live, publishes the recorded snapshot and
/// leaves no dangling reference — run-history and run-less tasks both included.
#[test]
fn recorded_observation_grant_reaches_live_with_the_real_snapshot() {
    let grant = grant_document();
    let expected_tasks = grant.snapshot.end.expected.tasks;
    let (mut client, _state) = client(
        OpenReply::Grant,
        vec![DeltasReply {
            recorded: recorded("deltas-none"),
            rebind: false,
        }],
        Duration::from_millis(10),
    );
    client.connect(scope(), 0).expect("observation opens");
    let mut now = 0;
    let events = wait_for(&mut client, &mut now, live);

    assert!(live(&client), "the recorded grant must reach Live");
    assert_eq!(
        snapshot_applied(&events).map(|counts| counts.tasks),
        Some(expected_tasks),
        "the published counts are the recorded end.expected"
    );
    assert_eq!(
        client.snapshot().tasks.len(),
        expected_tasks as usize,
        "every recorded task is published"
    );
    assert_eq!(
        client
            .last_seen_cursor()
            .map(desktop_protocol::Cursor::as_str),
        Some(grant.snapshot.end.cursor.as_str()),
        "the server-issued snapshot cursor is retained"
    );
    assert_eq!(
        client.server_identity(),
        Some(&grant.server.server_identity),
        "the authenticated server is the recorded one"
    );
    assert!(
        matches!(
            client.publication_access(),
            PublicationAccess::Validated { .. }
        ),
        "the validated publication is available"
    );
    // The recorded grant carries the v1 authorization surface: no readable
    // command decisions and no local intents.
    assert!(
        grant.grant.command_reads.is_empty(),
        "the recorded grant authorizes no command"
    );
    assert!(
        !grant.grant.local_intents_readable,
        "the recorded grant projects no local intents"
    );
    assert_eq!(
        grant.grant.stamp.server_id, grant.server.server_identity,
        "the grant is bound to the recorded server"
    );

    // Both recorded shapes are accepted: 37 tasks with a run (iteration index)
    // and 2 without any run axes.
    let with_run = client
        .snapshot()
        .tasks
        .values()
        .filter(|task| task.iteration_index.is_some())
        .count();
    assert_eq!(with_run, 37, "run-history tasks are published");
    assert_eq!(
        client.snapshot().tasks.len() - with_run,
        2,
        "run-less tasks are published without invented axes"
    );
    // v1 projects TASK only and the recorded rows declare no reference, so the
    // snapshot cannot carry a dangling one.
    assert!(
        client.snapshot().tasks.values().all(|task| {
            task.iteration_ref.is_none()
                && task.best_candidate_ref.is_none()
                && task.formal_outcome_ref.is_none()
                && task.accepted_brief_ref.is_none()
                && task.capabilities_ref.is_none()
        }),
        "no recorded task declares an optional reference"
    );
}

/// A recorded `{"transaction": null}` poll changes nothing and keeps the cursor.
#[test]
fn recorded_delta_none_keeps_the_snapshot_cursor() {
    let grant = grant_document();
    let (mut client, state) = client(
        OpenReply::Grant,
        vec![DeltasReply {
            recorded: recorded("deltas-none"),
            rebind: false,
        }],
        Duration::from_millis(10),
    );
    client.connect(scope(), 0).expect("observation opens");
    let mut now = 0;
    let events = wait_for(&mut client, &mut now, live);
    let events = {
        let mut all = events;
        all.extend(pump_for(
            &mut client,
            &mut now,
            Duration::from_millis(50),
            10,
        ));
        all
    };

    assert!(delta_applied(&events).is_empty(), "null applies nothing");
    assert_eq!(
        client
            .last_seen_cursor()
            .map(desktop_protocol::Cursor::as_str),
        Some(grant.snapshot.end.cursor.as_str()),
        "the cursor does not move without a transaction"
    );
    assert_eq!(client.snapshot().tasks.len(), 39);
    assert!(
        calls_matching(&state, &format!("GET {DELTAS_PATH}")) >= 1,
        "the recorded null poll was served"
    );
}

/// The recorded DeltaTransaction applies atomically: the key revision advances,
/// the successor cursor becomes the retained one and no resync happens.
#[test]
fn recorded_delta_changed_applies_the_recorded_transaction() {
    let delta = recorded_delta();
    let mutated = delta.mutations[0]
        .clone()
        .new_revision()
        .expect("the recorded upsert declares a new revision")
        .clone();
    let (mut client, _state) = client(
        OpenReply::Grant,
        vec![
            DeltasReply {
                recorded: recorded("deltas-changed"),
                rebind: true,
            },
            DeltasReply {
                recorded: recorded("deltas-none"),
                rebind: false,
            },
        ],
        Duration::from_millis(10),
    );
    client.connect(scope(), 0).expect("observation opens");
    let mut now = 0;
    let events = wait_for(&mut client, &mut now, |client| {
        client
            .snapshot()
            .tasks
            .values()
            .any(|task| task.projection_revision == mutated)
    });

    let applied = delta_applied(&events);
    assert_eq!(applied.len(), 1, "exactly one recorded transaction applies");
    assert_eq!(applied[0].0, delta.transaction_id.to_string());
    assert_eq!(
        applied[0].1,
        delta.cursor.to_string(),
        "the successor cursor is retained"
    );
    assert_eq!(
        client
            .last_seen_cursor()
            .map(desktop_protocol::Cursor::as_str),
        Some(delta.cursor.as_str())
    );
    assert_eq!(
        client.snapshot().tasks.len(),
        39,
        "no other projection moves"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ClientEvent::ResyncRequested { .. })),
        "an applied delta never triggers a resync"
    );
    let snapshot = client.snapshot();
    let updated = snapshot
        .tasks
        .values()
        .find(|task| task.projection_revision == mutated)
        .expect("the recorded key is present");
    assert_eq!(
        updated.task_status,
        desktop_protocol::TaskStatus::Cancelled,
        "the recorded mutation content is applied verbatim"
    );
}

/// A recorded `409 RESYNC_REQUIRED` surfaces a transport failure; the client
/// re-observes on its own, without a fabricated cursor or mutation.
#[test]
fn recorded_delta_resync_reobserves_without_fabrication() {
    let grant = grant_document();
    let snapshot_cursor = grant.snapshot.end.cursor.clone();
    let (mut client, state) = client(
        OpenReply::Grant,
        vec![
            // The first poll is the recorded `null`; the second is the 409.
            DeltasReply {
                recorded: recorded("deltas-none"),
                rebind: false,
            },
            DeltasReply {
                recorded: recorded("deltas-resync"),
                rebind: false,
            },
            // After the resync the re-observed session polls a recorded null and
            // stays Live instead of replaying the rejection forever.
            DeltasReply {
                recorded: recorded("deltas-none"),
                rebind: false,
            },
        ],
        Duration::from_millis(10),
    );
    client.connect(scope(), 0).expect("observation opens");
    let mut now = 0;
    let mut events = wait_for(&mut client, &mut now, live);

    // The 409 reaches the client as a transport failure, not as a frame.
    let error_events = wait_for(&mut client, &mut now, |client| {
        client.last_error().is_some_and(|error| {
            matches!(error, ClientError::Transport(message) if message.contains("RESYNC_REQUIRED"))
        })
    });
    events.extend(error_events);
    assert!(
        transport_error_containing(&events, "RESYNC_REQUIRED"),
        "the recorded 409 is surfaced as a transport failure"
    );
    // No cursor was fabricated and nothing was applied from the 409 body.
    assert_eq!(
        client
            .last_seen_cursor()
            .map(desktop_protocol::Cursor::as_str),
        Some(snapshot_cursor.as_str()),
        "the client keeps the last applied cursor"
    );
    assert!(
        delta_applied(&events).is_empty(),
        "the 409 body applies no mutation"
    );
    let relationship = client
        .snapshot()
        .tasks
        .values()
        .find(|task| task.task_ref.as_str() == "task-0b23811426af4a86a3fed9dc67b21947")
        .map(|task| {
            (
                task.projection_revision.to_string(),
                task.task_status.clone(),
            )
        });
    assert_eq!(
        relationship,
        Some(("3".to_owned(), desktop_protocol::TaskStatus::Completed)),
        "the projection still holds the snapshot content"
    );

    // The client's own recovery re-opens the observation (second recorded POST).
    let recovery = wait_for(&mut client, &mut now, live);
    events.extend(recovery);
    assert!(
        live(&client),
        "the client returns to Live after re-observing"
    );
    assert_eq!(
        calls_matching(&state, &format!("POST {OBSERVATION_PATH}")),
        2,
        "recovery re-opens the observation instead of guessing a cursor"
    );
    assert_eq!(
        client
            .last_seen_cursor()
            .map(desktop_protocol::Cursor::as_str),
        Some(snapshot_cursor.as_str()),
        "the re-observed snapshot restores the server-issued cursor"
    );
    assert!(
        delta_applied(&events).is_empty(),
        "no delta was ever applied"
    );
}

// --- rejection conditions --------------------------------------------------

/// A recorded `401 UNAUTHORIZED_CONTEXT` on a read withholds the publication.
#[test]
fn recorded_context_missing_withholds_the_publication() {
    let (mut client, _state) = client(
        OpenReply::Grant,
        vec![
            DeltasReply {
                recorded: recorded("deltas-none"),
                rebind: false,
            },
            DeltasReply {
                recorded: recorded("context-missing"),
                rebind: false,
            },
        ],
        // Give the test a moment of Live before the 401 poll.
        Duration::from_millis(80),
    );
    client.connect(scope(), 0).expect("observation opens");
    let mut now = 0;
    let _live_events = wait_for(&mut client, &mut now, live);
    assert!(matches!(
        client.publication_access(),
        PublicationAccess::Validated { .. }
    ));

    let events = wait_for(&mut client, &mut now, |client| {
        !matches!(
            client.publication_access(),
            PublicationAccess::Validated { .. }
        ) && client.last_error().is_some_and(|error| {
            matches!(error, ClientError::Transport(message) if message.contains("UNAUTHORIZED_CONTEXT"))
        })
    });
    assert!(
        transport_error_containing(&events, "UNAUTHORIZED_CONTEXT"),
        "the recorded 401 is surfaced"
    );
    match client.publication_access() {
        PublicationAccess::Withheld { reason } => assert_eq!(
            reason,
            ReadUnavailable::AuthorizationUnavailable,
            "the publication is withheld for authorization, not silently kept"
        ),
        PublicationAccess::Validated { .. } => panic!("an invalidated context must withhold"),
    }
    assert!(!live(&client), "a rejected context leaves the live state");
    // The last-known projection is retained but not published.
    assert_eq!(client.snapshot().tasks.len(), 39);
}

/// A recorded 401 for the observation POST publishes nothing.
#[test]
fn recorded_observation_unauthorized_publishes_nothing() {
    let (mut client, state) = client(
        OpenReply::Unauthorized,
        vec![DeltasReply {
            recorded: recorded("deltas-none"),
            rebind: false,
        }],
        Duration::from_millis(10),
    );
    client.connect(scope(), 0).expect("observation opens");
    let mut now = 0;
    let events = pump_for(&mut client, &mut now, Duration::from_millis(500), 1000);

    assert!(
        transport_error_containing(&events, "HTTP 401"),
        "the recorded 401 is surfaced"
    );
    assert!(
        snapshot_applied(&events).is_none(),
        "no snapshot is published"
    );
    assert!(client.snapshot().tasks.is_empty(), "no data is exposed");
    assert!(!live(&client), "the client must not claim a live session");
    match client.publication_access() {
        PublicationAccess::Withheld { reason } => assert_eq!(
            reason,
            ReadUnavailable::AuthorizationUnavailable,
            "the surface reports an unavailable authorization"
        ),
        PublicationAccess::Validated { .. } => panic!("an unauthorized open publishes nothing"),
    }
    assert!(
        calls_matching(&state, &format!("POST {OBSERVATION_PATH}")) >= 1,
        "the recorded POST rejection was exercised"
    );
}

/// A recorded `422 UNSUPPORTED_PROTOCOL` fails closed with no data.
#[test]
fn recorded_version_rejected_fails_closed() {
    let (mut client, _state) = client(
        OpenReply::VersionRejected,
        vec![DeltasReply {
            recorded: recorded("deltas-none"),
            rebind: false,
        }],
        Duration::from_millis(10),
    );
    client.connect(scope(), 0).expect("observation opens");
    let mut now = 0;
    let events = pump_for(&mut client, &mut now, Duration::from_millis(500), 1000);

    assert!(
        transport_error_containing(&events, "UNSUPPORTED_PROTOCOL"),
        "the recorded 422 is surfaced with its reason"
    );
    assert!(
        snapshot_applied(&events).is_none(),
        "no snapshot is published"
    );
    assert!(client.snapshot().tasks.is_empty(), "no data is exposed");
    assert!(
        matches!(client.connection(), ConnectionState::Incompatible { .. }),
        "bounded recovery ends in the incompatible fail-closed state, got {:?}",
        client.connection()
    );
    match client.publication_access() {
        PublicationAccess::Withheld { reason } => assert_eq!(
            reason,
            ReadUnavailable::AuthorizationUnavailable,
            "the surface stays withheld"
        ),
        PublicationAccess::Validated { .. } => panic!("a rejected version publishes nothing"),
    }
}

/// The recorded documents themselves reproduce the Runtime's counts and
/// CanonicalV1 bundle digests (the Desktop encoder verifies real server output).
#[test]
fn recorded_documents_reproduce_the_server_digests_and_counts() {
    let grant = grant_document();
    let grant_mutations = recorded_snapshot_mutations(&grant.snapshot);
    assert_eq!(
        SnapshotCounts::of(&grant_mutations),
        grant.snapshot.end.expected,
        "the grant snapshot counts are reproduced"
    );
    assert_eq!(
        projection_bundle_digest(&grant_mutations).expect("canonical digest"),
        grant.snapshot.end.integrity_digest,
        "the grant snapshot digest is reproduced"
    );
    assert_eq!(grant.schema_version, 1);
    assert_eq!(grant.snapshot.end.epoch, grant.server.epoch);

    let snapshot_record: GrantDocumentTail = {
        #[derive(Deserialize)]
        struct SnapshotOnly {
            schema_version: u32,
            snapshot: SnapshotDocument,
        }
        let document: SnapshotOnly =
            serde_json::from_value(recorded("snapshot").body).expect("recorded snapshot decodes");
        assert_eq!(document.schema_version, 1);
        GrantDocumentTail {
            snapshot: document.snapshot,
        }
    };
    let snapshot_mutations = recorded_snapshot_mutations(&snapshot_record.snapshot);
    assert_eq!(
        SnapshotCounts::of(&snapshot_mutations),
        snapshot_record.snapshot.end.expected
    );
    assert_eq!(
        projection_bundle_digest(&snapshot_mutations).expect("canonical digest"),
        snapshot_record.snapshot.end.integrity_digest
    );

    let delta = recorded_delta();
    assert_eq!(delta.mutation_count as usize, delta.mutations.len());
    assert_eq!(
        projection_bundle_digest(&delta.mutations).expect("canonical digest"),
        delta.integrity_digest,
        "the recorded transaction digest is reproduced"
    );
}

/// Local view used only by the snapshot-document assertion above.
struct GrantDocumentTail {
    snapshot: SnapshotDocument,
}
