//! REST protocol transport for the COS Runtime desktop observation endpoints.
//!
//! [`CosClient`](crate::CosClient) speaks encoded [`ClientWire`]/[`ServerWire`]
//! JSON frames through [`Transport`]; the Runtime serves the same protocol over
//! REST routes. This module adapts the two without teaching `cos-client` any
//! HTTP details: an injected [`ProtocolHttp`] executor performs the blocking
//! exchange, and one background thread turns the server documents into protocol
//! frames.
//!
//! # Session flow
//!
//! `CosClient` opens a session in two steps: it sends [`ClientWire::Hello`] and
//! waits for `HELLO_ACCEPTED`, then sends `OPEN_OBSERVATION` and waits for the
//! grant and snapshot. The transport mirrors that exactly:
//!
//! 1. on `Hello`, `GET /api/cos/desktop/server` yields the server-issued
//!    handshake, which is forwarded verbatim as a `HELLO_ACCEPTED` frame. The
//!    client negotiates against its own advertisement; the transport never
//!    edits the server's values,
//! 2. on `OPEN_OBSERVATION`, `POST /api/cos/desktop/observation` carries that
//!    request (`Authorization: Bearer` when configured) and its document is
//!    forwarded in order as `READ_GRANT`, `SNAPSHOT_BEGIN`, `SNAPSHOT_CHUNK…`
//!    and `SNAPSHOT_END`. The document's `server` report must equal the accepted
//!    hello, otherwise nothing is published and the stream fails closed,
//! 3. after the snapshot, `GET /api/cos/desktop/deltas?cursor=…` is polled at
//!    the configured interval (with the grant's `X-Cos-Context` header and the
//!    same `Authorization: Bearer` credential) while the stream generation is
//!    current. `"transaction": null` publishes nothing, and a delta transaction
//!    is forwarded as `DELTA_TRANSACTION`.
//!
//! The Desktop credential is presented on exactly the three protocol routes
//! (observation open, snapshot read, delta read) because reads are bound to the
//! issuing principal; the handshake route and every other path never see it.
//!
//! `409 RESYNC_REQUIRED` publishes no frame and fabricates no cursor: the
//! stream stops and the client's own recovery re-opens the observation. Every
//! failure maps to a [`TransportError`]; a malformed or oversized document
//! publishes nothing.
//!
//! # Streams, generations and bounds
//!
//! `send` never blocks: it decodes the frame and queues the exchange for the
//! background thread (a newer observation request supersedes a queued older one
//! of the same generation), and rejects any other client message explicitly.
//! `poll` returns one bounded inbound frame per call and `Ok(None)` when the
//! queue is empty. `reset_stream` drops queued frames and advances the
//! generation, so an in-flight response for the old stream is discarded. The
//! inbound queue, the job queue and the accepted response size are bounded; no
//! call panics.

use std::collections::VecDeque;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use desktop_protocol::{
    ClientMessage, ClientWire, Cursor, DeltaTransaction, HelloAccepted, Limits,
    ObservationScopeRequest, OpenAuthorizedObservation, ProtocolVersions, ReadContextId, ReadGrant,
    ReadRequestId, ReadStamp, ServerFrame, ServerId, ServerMessage, ServerWire, SnapshotBegin,
    SnapshotChunk, SnapshotEnd, Version,
};
use serde::{Deserialize, Serialize};

use crate::read::ObservationControl;
use crate::transport::{Transport, TransportError};

/// Observation document envelope version served by the REST endpoints.
pub const OBSERVATION_DOCUMENT_SCHEMA_VERSION: u32 = 1;
/// `GET` route returning the server-issued handshake.
pub const SERVER_PATH: &str = "/api/cos/desktop/server";
/// `POST` route that grants one observation session and its snapshot.
pub const OBSERVATION_PATH: &str = "/api/cos/desktop/observation";
/// `GET` route returning a fresh snapshot document for one scope.
pub const SNAPSHOT_PATH: &str = "/api/cos/desktop/snapshot";
/// `GET` route that polls one delta transaction after a cursor.
pub const DELTAS_PATH: &str = "/api/cos/desktop/deltas";

/// Routes that carry the Desktop credential.
///
/// Reads are bound to the issuing principal, so the observation open, the
/// snapshot read and the delta read all present `Authorization: Bearer <token>`.
/// The handshake route and every non-protocol path must never see it: the
/// credential exists only for these three exchanges. A query string is ignored,
/// so `/deltas?cursor=…` is still the delta route and nothing else can smuggle a
/// credential through a crafted query.
///
/// The composition root uses this to decide when a `401` is evidence that the
/// credential itself was rejected: a `401` from the handshake (which carries no
/// credential) says nothing about the stored credential and must not revoke it.
pub fn is_protocol_route(path: &str) -> bool {
    let route = path.split('?').next().unwrap_or(path);
    matches!(route, OBSERVATION_PATH | SNAPSHOT_PATH | DELTAS_PATH)
}

/// Append the credential to a protocol request's headers, when one is configured.
fn authorize(config: &ProtocolTransportConfig, headers: &mut Vec<(String, String)>, path: &str) {
    if !is_protocol_route(path) {
        return;
    }
    if let Some(token) = &config.bearer_token {
        headers.push(("authorization".to_owned(), format!("Bearer {token}")));
    }
}

/// One bounded HTTP exchange requested by the transport.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Path plus query string; the executor prefixes the configured origin.
    pub path: String,
    /// Lower-case header name/value pairs.
    pub headers: Vec<(String, String)>,
    /// Request body bytes; empty for `GET`.
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Credentials must never reach a log line, diagnostic or panic message
        // through `{:?}`: an `Authorization` header value is redacted while the
        // header name and every other field stay visible.
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), redacted_header_value(name, value)))
            .collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("headers", &headers)
            .field("body", &self.body)
            .finish()
    }
}

/// Header value rendered for diagnostics; authorization credentials are hidden.
fn redacted_header_value<'a>(name: &str, value: &'a str) -> &'a str {
    if !name.eq_ignore_ascii_case("authorization") {
        return value;
    }
    match value.split_once(' ') {
        Some((scheme, _rest)) if scheme.eq_ignore_ascii_case("bearer") => "Bearer <redacted>",
        _ => "<redacted>",
    }
}

/// HTTP method of one protocol exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    /// Read one resource.
    Get,
    /// Open one resource.
    Post,
}

/// Bounded HTTP response returned by the executor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body bytes; the transport bounds and parses it.
    pub body: Vec<u8>,
}

/// Injected blocking HTTP executor.
///
/// The binary implements this over its concrete client and with its own request
/// timeout; the transport never performs I/O itself. Implementations must be
/// `Send` because the transport moves them to its background thread.
pub trait ProtocolHttp: Send {
    /// Perform one blocking exchange. Errors carry a safe message only.
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String>;
}

/// Transport configuration.
#[derive(Clone)]
pub struct ProtocolTransportConfig {
    /// Desktop credential presented on the three protocol routes (observation
    /// open, snapshot read, delta read); never on the handshake or another path.
    pub bearer_token: Option<String>,
    /// Interval between delta polls after the initial snapshot.
    pub delta_poll_interval: Duration,
    /// Maximum accepted response-document size in bytes.
    pub max_response_bytes: u64,
    /// Bounded number of pending protocol jobs.
    pub max_pending_requests: usize,
    /// Bounded inbound frame queue.
    pub queue_limits: Limits,
}

impl fmt::Debug for ProtocolTransportConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The bearer token is a credential: `{:?}` must never print it into a
        // log line, diagnostic report or panic message.
        f.debug_struct("ProtocolTransportConfig")
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("delta_poll_interval", &self.delta_poll_interval)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("max_pending_requests", &self.max_pending_requests)
            .field("queue_limits", &self.queue_limits)
            .finish()
    }
}

impl Default for ProtocolTransportConfig {
    fn default() -> Self {
        let limits = Limits::DEFAULT;
        Self {
            bearer_token: None,
            delta_poll_interval: Duration::from_millis(250),
            // One document carries the whole snapshot plus the grant report;
            // bound it by the staged bundle ceiling plus one control frame of
            // envelope overhead.
            max_response_bytes: limits
                .max_bundle_bytes
                .saturating_add(limits.max_frame_bytes),
            max_pending_requests: 4,
            queue_limits: limits,
        }
    }
}

/// One protocol transport over the injected REST executor.
pub struct ProtocolTransport<E: ProtocolHttp> {
    shared: SharedState,
    config: ProtocolTransportConfig,
    versions: ProtocolVersions,
    worker: Option<JoinHandle<()>>,
    _executor: PhantomData<fn() -> E>,
}

impl<E: ProtocolHttp + 'static> ProtocolTransport<E> {
    /// Start the transport and its single background thread.
    ///
    /// A thread that cannot start leaves the transport permanently failed
    /// closed: `poll` reports the spawn error instead of silently delivering
    /// nothing.
    pub fn new(executor: E, config: ProtocolTransportConfig, versions: ProtocolVersions) -> Self {
        let shared: SharedState =
            Arc::new((Mutex::new(Shared::new(config.queue_limits)), Condvar::new()));
        let worker = {
            let shared = Arc::clone(&shared);
            let worker_config = config.clone();
            thread::Builder::new()
                .name("cos-protocol-transport".to_owned())
                .spawn(move || run_worker(executor, worker_config, versions, shared))
                .ok()
        };
        if worker.is_none() {
            let mut state = lock(&shared);
            state.pending_error = Some(TransportError::Io(
                "protocol transport background thread could not start".to_owned(),
            ));
        }
        Self {
            shared,
            config,
            versions,
            worker,
            _executor: PhantomData,
        }
    }

    /// Current stream generation (diagnostics/tests only).
    pub fn generation(&self) -> u64 {
        lock(&self.shared).generation
    }
}

impl<E: ProtocolHttp> Drop for ProtocolTransport<E> {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.shared);
            state.closed = true;
        }
        self.shared.1.notify_all();
        // The worker observes `closed` at its next wakeup. Joining would block
        // Drop on the injected executor's own request timeout, so the handle is
        // released and the thread exits on its own.
        self.worker.take();
    }
}

impl<E: ProtocolHttp> Transport for ProtocolTransport<E> {
    fn bind_observation_control(&mut self, control: ObservationControl) {
        lock(&self.shared).control = Some(control);
    }

    fn authenticated_server(&self) -> Option<ServerId> {
        lock(&self.shared).server.clone()
    }

    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        let wire: ClientWire = serde_json::from_slice(&frame)
            .map_err(|error| TransportError::Io(format!("malformed client frame: {error}")))?;
        let open = match wire {
            ClientWire::Hello(_) => None,
            ClientWire::Frame(frame) => {
                let ClientMessage::OpenObservation(request) = frame.message else {
                    return Err(TransportError::Io(
                        "unsupported client frame: the REST transport carries observation opens only"
                            .to_owned(),
                    ));
                };
                if frame.protocol != self.versions.protocol || frame.schema != self.versions.schema
                {
                    return Err(TransportError::Io(format!(
                        "client frame uses {}/{} but the REST transport serves {}/{}",
                        frame.protocol, frame.schema, self.versions.protocol, self.versions.schema
                    )));
                }
                Some(request)
            }
        };
        {
            let mut state = lock(&self.shared);
            let generation = state.generation;
            let job = match open {
                None => Job::Hello { generation },
                Some(request) => Job::Open {
                    generation,
                    request,
                },
            };
            enqueue_job(&mut state, job, self.config.max_pending_requests)?;
        }
        self.shared.1.notify_all();
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        let mut state = lock(&self.shared);
        if state.overflowed {
            state.overflowed = false;
            state.inbound.clear();
            state.inbound_bytes = 0;
            return Err(TransportError::QueueOverflow);
        }
        if let Some(frame) = state.inbound.pop_front() {
            state.inbound_bytes = state.inbound_bytes.saturating_sub(frame.len() as u64);
            return Ok(Some(frame));
        }
        if let Some(error) = state.pending_error.take() {
            return Err(error);
        }
        Ok(None)
    }

    fn reset_stream(&mut self) {
        {
            let mut state = lock(&self.shared);
            state.generation += 1;
            state.inbound.clear();
            state.inbound_bytes = 0;
            state.overflowed = false;
            state.pending_error = None;
            state.jobs.clear();
            state.poll = None;
            // The old stream's verified handshake and server identity belong to
            // that stream: a new stream must re-authenticate before the client
            // may treat any server statement as verified.
            state.hello = None;
            state.server = None;
        }
        self.shared.1.notify_all();
    }
}

type SharedState = Arc<(Mutex<Shared>, Condvar)>;

struct Shared {
    generation: u64,
    closed: bool,
    inbound: VecDeque<Vec<u8>>,
    inbound_bytes: u64,
    overflowed: bool,
    pending_error: Option<TransportError>,
    jobs: VecDeque<Job>,
    poll: Option<PollSession>,
    /// Latest server-issued handshake accepted for this stream.
    hello: Option<HelloAccepted>,
    /// Server identity verified by that handshake.
    server: Option<ServerId>,
    control: Option<ObservationControl>,
    limits: Limits,
}

impl Shared {
    fn new(limits: Limits) -> Self {
        Self {
            generation: 0,
            closed: false,
            inbound: VecDeque::new(),
            inbound_bytes: 0,
            overflowed: false,
            pending_error: None,
            jobs: VecDeque::new(),
            poll: None,
            hello: None,
            server: None,
            control: None,
            limits,
        }
    }
}

enum Job {
    Hello {
        generation: u64,
    },
    Open {
        generation: u64,
        request: OpenAuthorizedObservation,
    },
}

impl Job {
    fn generation(&self) -> u64 {
        match self {
            Self::Hello { generation } | Self::Open { generation, .. } => *generation,
        }
    }
}

#[derive(Clone)]
struct PollSession {
    generation: u64,
    protocol: Version,
    schema: Version,
    cursor: Cursor,
    context: ReadContextId,
    stamp: ReadStamp,
    /// Whether the first poll still has to happen immediately.
    first: bool,
}

enum WorkerAction {
    Hello {
        generation: u64,
    },
    Open {
        generation: u64,
        request: OpenAuthorizedObservation,
    },
    Poll(PollSession),
}

fn lock(shared: &SharedState) -> MutexGuard<'_, Shared> {
    shared.0.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait<'a>(shared: &'a SharedState, state: MutexGuard<'a, Shared>) -> MutexGuard<'a, Shared> {
    shared.1.wait(state).unwrap_or_else(PoisonError::into_inner)
}

fn push_inbound(state: &mut Shared, frame: Vec<u8>) {
    let frames_full = state.inbound.len() as u64 >= u64::from(state.limits.max_queue_frames);
    let bytes_full =
        state.inbound_bytes.saturating_add(frame.len() as u64) > state.limits.max_queue_bytes;
    if frames_full || bytes_full {
        state.overflowed = true;
        return;
    }
    state.inbound_bytes = state.inbound_bytes.saturating_add(frame.len() as u64);
    state.inbound.push_back(frame);
}

fn enqueue_job(state: &mut Shared, job: Job, capacity: usize) -> Result<(), TransportError> {
    // A newer request supersedes queued work for the same generation: a repeated
    // handshake replaces the queued one, and a newer observation open replaces a
    // queued open. A handshake and an open coexist because the open depends on
    // the handshake being accepted.
    let generation = job.generation();
    let replacing_hello = matches!(job, Job::Hello { .. });
    let replacing_open = matches!(job, Job::Open { .. });
    state.jobs.retain(|queued| {
        if queued.generation() != generation {
            return true;
        }
        if replacing_hello {
            return !matches!(queued, Job::Hello { .. });
        }
        if replacing_open {
            return !matches!(queued, Job::Open { .. });
        }
        true
    });
    if state.jobs.len() >= capacity {
        return Err(TransportError::QueueOverflow);
    }
    state.jobs.push_back(job);
    Ok(())
}

fn run_worker<E: ProtocolHttp>(
    mut executor: E,
    config: ProtocolTransportConfig,
    versions: ProtocolVersions,
    shared: SharedState,
) {
    loop {
        let action = {
            let mut state = lock(&shared);
            loop {
                if state.closed {
                    return;
                }
                if let Some(job) = state.jobs.pop_front() {
                    break match job {
                        Job::Hello { generation } => WorkerAction::Hello { generation },
                        Job::Open {
                            generation,
                            request,
                        } => WorkerAction::Open {
                            generation,
                            request,
                        },
                    };
                }
                if let Some(poll) = state.poll.clone() {
                    if poll.generation == state.generation {
                        break WorkerAction::Poll(poll);
                    }
                    state.poll = None;
                    continue;
                }
                state = wait(&shared, state);
            }
        };
        match action {
            WorkerAction::Hello { generation } => {
                let outcome = get_server(&mut executor, &config);
                let mut state = lock(&shared);
                if state.closed || generation != state.generation {
                    continue;
                }
                match outcome {
                    Ok(hello) => {
                        state.server = Some(hello.server_identity.clone());
                        state.hello = Some(hello.clone());
                        match encode_hello(&hello) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                    }
                    Err(error) => state.pending_error = Some(error),
                }
                wake_after(state);
            }
            WorkerAction::Open {
                generation,
                request,
            } => {
                let outcome = post_open(&mut executor, &config, versions, &request);
                let mut state = lock(&shared);
                if state.closed || generation != state.generation {
                    continue;
                }
                match outcome {
                    Ok(document) => match apply_open(&mut state, &document) {
                        Ok(()) => {}
                        Err(error) => state.pending_error = Some(error),
                    },
                    Err(error) => state.pending_error = Some(error),
                }
                wake_after(state);
            }
            WorkerAction::Poll(poll) => {
                let mut state = lock(&shared);
                let immediate = take_first_poll(&mut state, &poll);
                if !immediate {
                    let (next, _timeout) = shared
                        .1
                        .wait_timeout(state, config.delta_poll_interval)
                        .unwrap_or_else(PoisonError::into_inner);
                    state = next;
                }
                if state.closed || poll.generation != state.generation {
                    state.poll = None;
                    continue;
                }
                if !state.jobs.is_empty() {
                    continue;
                }
                drop(state);
                let outcome = get_deltas(&mut executor, &config, &poll);
                let mut state = lock(&shared);
                if state.closed || poll.generation != state.generation {
                    state.poll = None;
                    continue;
                }
                match outcome {
                    Ok(DeltaOutcome::Empty) => {}
                    Ok(DeltaOutcome::Transaction(transaction)) => {
                        if let Some(current) = state.poll.as_mut() {
                            current.cursor = transaction.cursor.clone();
                        }
                        match encode_frame(
                            poll.protocol,
                            poll.schema,
                            Some(poll.stamp.clone()),
                            ServerMessage::DeltaTransaction(*transaction),
                        ) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                        wake_after(state);
                    }
                    // The endpoint reports the stable code only; publishing no
                    // frame leaves the decision to the client's own recovery,
                    // which re-opens the observation.
                    Err(error) => {
                        state.poll = None;
                        state.pending_error = Some(error);
                        wake_after(state);
                    }
                }
            }
        }
    }
}

/// Wake the observation pump after releasing the shared lock.
fn wake_after(state: MutexGuard<'_, Shared>) {
    let control = state.control.clone();
    drop(state);
    if let Some(control) = control {
        control.wake.wake_by_ref();
    }
}

/// Whether this poll must run without waiting; clears the session's first flag.
fn take_first_poll(state: &mut Shared, poll: &PollSession) -> bool {
    match state.poll.as_mut() {
        Some(current) if current.generation == poll.generation => {
            std::mem::take(&mut current.first)
        }
        _ => true,
    }
}

/// Publish one observation document after the accepted handshake.
fn apply_open(state: &mut Shared, document: &OpenDocument) -> Result<(), TransportError> {
    let Some(hello) = state.hello.as_ref() else {
        return Err(TransportError::Io(
            "observation document arrived before an accepted handshake".to_owned(),
        ));
    };
    if document.server != *hello {
        return Err(TransportError::Io(
            "observation document handshake differs from the accepted hello".to_owned(),
        ));
    }
    let protocol = document.server.protocol;
    let schema = document.server.schema;
    let stamp = Some(document.grant.stamp.clone());
    let mut frames = Vec::with_capacity(3 + document.snapshot.chunks.len());
    frames.push(encode_frame(
        protocol,
        schema,
        None,
        ServerMessage::ReadGrant(document.grant.clone()),
    )?);
    frames.push(encode_frame(
        protocol,
        schema,
        stamp.clone(),
        ServerMessage::SnapshotBegin(document.snapshot.begin.clone()),
    )?);
    for chunk in &document.snapshot.chunks {
        frames.push(encode_frame(
            protocol,
            schema,
            stamp.clone(),
            ServerMessage::SnapshotChunk(chunk.clone()),
        )?);
    }
    frames.push(encode_frame(
        protocol,
        schema,
        stamp,
        ServerMessage::SnapshotEnd(document.snapshot.end.clone()),
    )?);
    for frame in frames {
        push_inbound(state, frame);
    }
    state.poll = Some(PollSession {
        generation: state.generation,
        protocol,
        schema,
        cursor: document.snapshot.end.cursor.clone(),
        context: document.grant.stamp.context_id.clone(),
        stamp: document.grant.stamp.clone(),
        first: true,
    });
    Ok(())
}

fn encode_frame(
    protocol: Version,
    schema: Version,
    read_stamp: Option<ReadStamp>,
    message: ServerMessage,
) -> Result<Vec<u8>, TransportError> {
    encode_wire(&ServerWire::Frame(ServerFrame {
        protocol,
        schema,
        read_stamp,
        message,
    }))
}

fn encode_hello(hello: &HelloAccepted) -> Result<Vec<u8>, TransportError> {
    encode_wire(&ServerWire::HelloAccepted(hello.clone()))
}

fn encode_wire(wire: &ServerWire) -> Result<Vec<u8>, TransportError> {
    serde_json::to_vec(wire)
        .map_err(|error| TransportError::Io(format!("cannot encode server frame: {error}")))
}

// --- REST exchanges -------------------------------------------------------

#[derive(Deserialize)]
struct ServerDocument {
    schema_version: u32,
    server: HelloAccepted,
}

#[derive(Serialize)]
struct OpenRequestBody<'a> {
    schema_version: u32,
    request_id: &'a ReadRequestId,
    protocol: Version,
    schema: Version,
    scope: &'a ObservationScopeRequest,
    retained_commands: &'a [desktop_protocol::CommandId],
}

#[derive(Deserialize)]
struct OpenDocument {
    schema_version: u32,
    server: HelloAccepted,
    grant: ReadGrant,
    snapshot: SnapshotDocument,
}

#[derive(Deserialize)]
struct SnapshotDocument {
    begin: SnapshotBegin,
    #[serde(default)]
    chunks: Vec<SnapshotChunk>,
    end: SnapshotEnd,
}

#[derive(Deserialize)]
struct DeltasDocument {
    schema_version: u32,
    #[serde(default)]
    transaction: Option<DeltaTransaction>,
}

enum DeltaOutcome {
    Empty,
    Transaction(Box<DeltaTransaction>),
}

fn get_server<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
) -> Result<HelloAccepted, TransportError> {
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Get,
            path: SERVER_PATH.to_owned(),
            headers: vec![("accept".to_owned(), "application/json".to_owned())],
            body: Vec::new(),
        })
        .map_err(|message| TransportError::Io(format!("server handshake failed: {message}")))?;
    // The byte bound applies to every response, error documents included: an
    // oversized body must never reach the JSON parser or the error classifier.
    bound_response(config, &response, "server handshake")?;
    if !is_success(response.status) {
        return Err(http_error("server handshake", &response));
    }
    let document: ServerDocument = serde_json::from_slice(&response.body)
        .map_err(|error| TransportError::Io(format!("malformed server document: {error}")))?;
    if document.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION {
        return Err(TransportError::Io(format!(
            "unsupported server document schema version {}",
            document.schema_version
        )));
    }
    Ok(document.server)
}

fn post_open<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
    versions: ProtocolVersions,
    request: &OpenAuthorizedObservation,
) -> Result<OpenDocument, TransportError> {
    let body = OpenRequestBody {
        schema_version: OBSERVATION_DOCUMENT_SCHEMA_VERSION,
        request_id: &request.request_id,
        protocol: versions.protocol,
        schema: versions.schema,
        scope: &request.observation.scope,
        retained_commands: &request.retained_commands,
    };
    let body = serde_json::to_vec(&body).map_err(|error| {
        TransportError::Io(format!("cannot encode observation request: {error}"))
    })?;
    let mut headers = vec![
        ("accept".to_owned(), "application/json".to_owned()),
        ("content-type".to_owned(), "application/json".to_owned()),
    ];
    authorize(config, &mut headers, OBSERVATION_PATH);
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Post,
            path: OBSERVATION_PATH.to_owned(),
            headers,
            body,
        })
        .map_err(|message| TransportError::Io(format!("observation request failed: {message}")))?;
    // The byte bound applies to every response, error documents included: an
    // oversized body must never reach the JSON parser or the error classifier.
    bound_response(config, &response, "observation open")?;
    if !is_success(response.status) {
        return Err(http_error("observation open", &response));
    }
    let document: OpenDocument = serde_json::from_slice(&response.body)
        .map_err(|error| TransportError::Io(format!("malformed observation document: {error}")))?;
    if document.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION {
        return Err(TransportError::Io(format!(
            "unsupported observation document schema version {}",
            document.schema_version
        )));
    }
    Ok(document)
}

fn get_deltas<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
    poll: &PollSession,
) -> Result<DeltaOutcome, TransportError> {
    let path = format!(
        "{DELTAS_PATH}?cursor={}",
        encode_query_value(poll.cursor.as_str())
    );
    let mut headers = vec![
        ("accept".to_owned(), "application/json".to_owned()),
        ("x-cos-context".to_owned(), poll.context.as_str().to_owned()),
    ];
    authorize(config, &mut headers, &path);
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Get,
            path,
            headers,
            body: Vec::new(),
        })
        .map_err(|message| TransportError::Io(format!("delta request failed: {message}")))?;
    // The byte bound applies to every response, error documents included: an
    // oversized body must never reach the JSON parser or the error classifier.
    bound_response(config, &response, "delta poll")?;
    if !is_success(response.status) {
        return Err(http_error("delta poll", &response));
    }
    let document: DeltasDocument = serde_json::from_slice(&response.body)
        .map_err(|error| TransportError::Io(format!("malformed delta document: {error}")))?;
    if document.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION {
        return Err(TransportError::Io(format!(
            "unsupported delta document schema version {}",
            document.schema_version
        )));
    }
    Ok(match document.transaction {
        Some(transaction) => DeltaOutcome::Transaction(Box::new(transaction)),
        None => DeltaOutcome::Empty,
    })
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

fn bound_response(
    config: &ProtocolTransportConfig,
    response: &HttpResponse,
    what: &str,
) -> Result<(), TransportError> {
    if response.body.len() as u64 > config.max_response_bytes {
        return Err(TransportError::Io(format!(
            "{what} response of {} bytes exceeds the bounded {}-byte limit",
            response.body.len(),
            config.max_response_bytes
        )));
    }
    Ok(())
}

fn http_error(what: &str, response: &HttpResponse) -> TransportError {
    let code = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| {
            value
                .get("code")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    match code {
        Some(code) => {
            TransportError::Io(format!("{what} rejected: HTTP {} {code}", response.status))
        }
        None => TransportError::Io(format!("{what} rejected: HTTP {}", response.status)),
    }
}

fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let high = char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0');
                let low = char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0');
                out.push('%');
                out.push(high.to_ascii_uppercase());
                out.push(low.to_ascii_uppercase());
            }
        }
    }
    out
}

#[cfg(test)]
mod credential_route_tests {
    //! RC-03: reads are principal-bound, so the credential is presented on the
    //! three protocol routes and nowhere else.

    use super::{
        DELTAS_PATH, OBSERVATION_PATH, ProtocolTransportConfig, SERVER_PATH, SNAPSHOT_PATH,
        authorize, is_protocol_route,
    };

    #[test]
    fn the_credential_is_presented_on_exactly_the_three_protocol_routes() {
        for route in [OBSERVATION_PATH, SNAPSHOT_PATH, DELTAS_PATH] {
            assert!(is_protocol_route(route), "{route} is a protocol route");
        }
        // A query string belongs to the route, not to another path.
        assert!(is_protocol_route(&format!("{DELTAS_PATH}?cursor=cursor-1")));
        assert!(is_protocol_route(&format!("{SNAPSHOT_PATH}?scope=scope-1")));
        for other in [
            SERVER_PATH,
            "/api/cos/desktop/other",
            "/api/cos/desktop/deltas-extra",
            "/api/cos/desktop/snapshot/extra",
            "/",
        ] {
            assert!(!is_protocol_route(other), "{other} is not a protocol route");
        }

        let config = ProtocolTransportConfig {
            bearer_token: Some("local-test-token".to_owned()),
            ..ProtocolTransportConfig::default()
        };
        let mut headers = Vec::new();
        authorize(
            &config,
            &mut headers,
            &format!("{DELTAS_PATH}?cursor=cursor-1"),
        );
        assert_eq!(
            headers,
            vec![(
                "authorization".to_owned(),
                "Bearer local-test-token".to_owned()
            )]
        );

        let mut headers = Vec::new();
        authorize(&config, &mut headers, SERVER_PATH);
        assert!(headers.is_empty(), "the handshake never carries it");

        let mut headers = Vec::new();
        authorize(
            &ProtocolTransportConfig::default(),
            &mut headers,
            OBSERVATION_PATH,
        );
        assert!(headers.is_empty(), "no configured token means no header");
    }
}
