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
//! The Desktop credential is presented on exactly the authenticated Desktop
//! routes (observation open, fresh snapshot read, delta read, command submit,
//! receipt lookup, catalog and the task-scoped history read) because reads and
//! commands are bound to the issuing principal; the handshake route and every
//! other path never see it.
//!
//! `409 RESYNC_REQUIRED` is not an I/O failure: the Runtime's closed reason is
//! published as a `RESYNC_REQUIRED` frame and the client's own bounded recovery
//! re-opens the observation. A resumed open that the Runtime accepts carries a
//! fresh grant and a replay position; the transport publishes `READ_GRANT` then
//! a stamped `REPLAY_ACCEPTED` and keeps polling deltas from that cursor. The
//! three resume coordinates travel only as a complete triplet: the Runtime
//! refuses a partial one as invalid, so a fresh re-open that still knows its
//! previous scope identity is normalised to a fresh open, never a `422`. After
//! a delta-gap resync the next fresh open is served by one explicit
//! `GET /api/cos/desktop/snapshot` refresh for the still-granted scope (same
//! staging and identity rules as the open snapshot) before falling back to a
//! fresh observation open. The retained-command list is bounded at the frozen
//! contract maximum of 448 distinct identities. Every failure maps to a
//! [`TransportError`]; a malformed or oversized document publishes nothing.
//!
//! # Commands and receipts
//!
//! A `COMMAND` frame is submitted on the current read context
//! (`POST /api/cos/desktop/commands` with `X-Cos-Context`) and answered with one
//! `RECEIPT` frame carrying the Runtime's authorization stamp, so the client
//! applies it only when that stamp is its current binding. A `LOOKUP_RECEIPT`
//! frame maps to `POST /api/cos/desktop/commands/lookup` and is answered with
//! `RECEIPT_READ_RESULT` (allowed with/without a receipt, or denied). Refusals
//! become structured `ERROR` frames only for definitive command verdicts
//! (conflict, revision, validation, unsupported, expired receipt); session-wide
//! authorization, resync and internal failures stay uncorrelated. A lost or
//! failed exchange is a transport failure, so the command remains `ResultUnknown`
//! and is never resent automatically.
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

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use desktop_protocol::{
    AuthorizedCommandReceipt, AuthorizedReceiptLookup, ClientMessage, ClientWire, CommandEnvelope,
    Counter, Cursor, DeltaTransaction, EpochId, ErrorCode, HelloAccepted, Limits, ObservationScope,
    ObservationScopeRequest, OpenAuthorizedObservation, ProtocolError, ProtocolVersions,
    ReadContextId, ReadGrant, ReadRequestId, ReadStamp, ReceiptReadResult, ReplayAccepted,
    ResyncReason, ResyncRequired, ScopeId, ServerFrame, ServerId, ServerMessage, ServerWire,
    SnapshotBegin, SnapshotChunk, SnapshotEnd, Version,
};
use serde::{Deserialize, Serialize};

use crate::history::{
    HistoryDocument, HistoryFetchError, HistoryQuery, history_path, is_task_read_route,
};
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
/// `POST` route that submits one command envelope.
pub const COMMANDS_PATH: &str = "/api/cos/desktop/commands";
/// `POST` route that reads one authorized command receipt.
pub const COMMANDS_LOOKUP_PATH: &str = "/api/cos/desktop/commands/lookup";
/// `GET` route returning the authenticated Desktop catalog document.
pub const CATALOG_PATH: &str = "/api/cos/desktop/catalog";

/// Maximum time the catalog read may block its calling (owner) thread.
///
/// The REST exchange runs on the transport worker; this bound limits only the
/// caller's wait for the reply. It is deliberately short because the
/// observation owner is single-threaded and must not stall for the 30 s
/// handshake-class budget.
const CATALOG_FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// Authenticated Desktop catalog document.
///
/// Mirrors the Runtime `{schema_version, server, catalog}` wrapper and the
/// catalog recipe/parameter fields the Brief needs. The document is bounded by
/// [`ProtocolTransportConfig::max_response_bytes`]; unknown members are refused
/// so a schema change cannot be silently ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDocument {
    /// Document schema version (must be 1).
    pub schema_version: u32,
    /// Server identity and observation epoch that served the catalog.
    pub server: CatalogServer,
    /// Submittable recipe catalog.
    pub catalog: Catalog,
}

/// Server identity/epoch carried beside the authenticated catalog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogServer {
    /// Stable server identity.
    pub server_identity: String,
    /// Runtime epoch that served this catalog.
    pub epoch: String,
}

/// The catalog document body (same document the legacy builder serves).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// Catalog schema version.
    pub schema_version: u32,
    /// Whether at least one recipe can be submitted.
    pub can_submit: bool,
    /// Stable reasons when recipes are unavailable.
    pub reason_codes: Vec<String>,
    /// Submittable recipe options.
    pub recipes: Vec<CatalogRecipe>,
}

/// One Advanced control value carried by the catalog.
///
/// The Runtime control values are string, integer or boolean; an unknown JSON
/// value is a parse refusal rather than a silently coerced value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CatalogParameterValue {
    /// Text (also the canonical decimal text for a decimal control).
    Text(String),
    /// Signed integer.
    Integer(i64),
    /// Boolean.
    Boolean(bool),
}

/// Runtime input limits for one recipe.
///
/// The Runtime document may carry more keys; only the image bound is part of
/// the Desktop contract and unknown keys are ignored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogInputLimits {
    /// Maximum reference images accepted by the recipe.
    pub max_images: Option<u32>,
}

/// Runtime execution limits for one recipe.
///
/// The Desktop does not consume these values yet; the closed empty projection
/// keeps the wire parse bounded without inventing a second execution schema.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogExecutionLimits {}

/// One catalog recipe option.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRecipe {
    /// Exact recipe reference; copied verbatim into the creation target.
    pub recipe_ref: CatalogRecipeRef,
    /// Human label.
    pub label: String,
    /// Human description.
    pub description: String,
    /// Runtime-defined input limits.
    pub input_limits: CatalogInputLimits,
    /// Whether reference images are accepted.
    pub accepts_image_references: bool,
    /// Runtime-defined execution limits.
    pub execution_limits: CatalogExecutionLimits,
    /// Read-only constraint summary.
    pub constraint_summary: String,
    /// Advanced controls (editable flag per control).
    pub parameters: Vec<CatalogParameter>,
}

/// Recipe reference in the Runtime catalog projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRecipeRef {
    /// Recipe identity.
    pub recipe_id: String,
    /// Pinned harness version identity.
    #[serde(default)]
    pub source_harness_version_id: Option<String>,
    /// Pinned harness digest.
    #[serde(default)]
    pub source_harness_digest: Option<String>,
    /// Exact graph revision pointer.
    pub graph_ref: CatalogGraphRef,
    /// Selected profile, when any.
    #[serde(default)]
    pub profile_ref: Option<String>,
    /// Projection digest of the recipe option.
    #[serde(default)]
    pub projection_digest: Option<String>,
}

/// Graph pointer in the Runtime catalog projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogGraphRef {
    /// Graph identity.
    pub graph_id: String,
    /// Canonical graph revision version.
    pub graph_version: i64,
    /// Canonical graph revision digest.
    pub graph_digest: String,
}

/// One advanced catalog control.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogParameter {
    /// Control identity.
    pub id: String,
    /// Human label.
    pub label: String,
    /// Closed control kind (`text`, `integer`, `decimal`, `boolean`, `choice`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Default value (string, integer or boolean).
    pub value: CatalogParameterValue,
    /// Numeric/length bounds, when any.
    #[serde(default)]
    pub bounds: Option<CatalogParameterBounds>,
    /// Closed choice values, when a choice control.
    #[serde(default)]
    pub choices: Vec<CatalogParameterChoice>,
    /// Whether the control may be changed by the user.
    pub editable: bool,
}

/// Advanced control bounds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogParameterBounds {
    /// Minimum value text.
    #[serde(default)]
    pub min: Option<String>,
    /// Maximum value text.
    #[serde(default)]
    pub max: Option<String>,
    /// Maximum text length.
    #[serde(default)]
    pub max_length: Option<i64>,
}

/// One choice entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogParameterChoice {
    /// Wire value.
    pub value: String,
    /// Human label.
    pub label: String,
}

/// Frozen contract maximum of distinct retained command identities per open.
///
/// The protocol freezes the grant list at 448 (`docs/architecture/DESKTOP_PROTOCOL.md`),
/// derived from the registry bound rather than a new allowance. A client frame
/// above it is refused before any exchange.
pub const MAX_RETAINED_COMMANDS: usize = 448;

/// Longest Runtime safe message copied into a resync frame, in characters.
const MAX_RESYNC_MESSAGE_CHARS: usize = 512;

/// Routes that carry the Desktop credential.
///
/// Reads are bound to the issuing principal, so the observation open, the fresh
/// snapshot read, the delta read, the command submit and the receipt lookup all
/// present `Authorization: Bearer <token>`. The handshake route and every
/// non-protocol path must never see it: the credential exists only for these
/// exchanges. A query string is ignored, so `/deltas?cursor=…` is still the
/// delta route and nothing else can smuggle a credential through a crafted
/// query.
///
/// The composition root uses this to decide when a `401` is evidence that the
/// credential itself was rejected: a `401` from the handshake (which carries no
/// credential) says nothing about the stored credential and must not revoke it.
pub fn is_protocol_route(path: &str) -> bool {
    let route = path.split('?').next().unwrap_or(path);
    matches!(
        route,
        OBSERVATION_PATH
            | SNAPSHOT_PATH
            | DELTAS_PATH
            | COMMANDS_PATH
            | COMMANDS_LOOKUP_PATH
            | CATALOG_PATH
    ) || is_task_read_route(route)
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
    /// Desktop credential presented on the protocol routes (observation open,
    /// snapshot read, delta read, catalog read, command submit, receipt
    /// lookup); never on the handshake or another path.
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

    /// Fetch the authenticated Desktop catalog document.
    ///
    /// The REST transport runs the exchange on its worker thread and this call
    /// blocks the (background) caller until the bounded reply or timeout. It
    /// carries the Desktop credential but needs no observation context.
    pub fn fetch_catalog(&mut self) -> Result<CatalogDocument, TransportError> {
        fetch_catalog_with(&self.shared, &self.config)
    }

    /// Fetch one bounded authenticated task history page.
    ///
    /// The route is bound to the current observation read context, so the
    /// transport resolves that context when the request is enqueued. Without a
    /// live context the read is refused as unauthorized instead of being
    /// presented unauthenticated.
    pub fn fetch_history(
        &mut self,
        request: &HistoryQuery,
    ) -> Result<HistoryDocument, HistoryFetchError> {
        fetch_history_with(&self.shared, &self.config, request)
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

    fn fetch_catalog(&mut self) -> Result<CatalogDocument, TransportError> {
        fetch_catalog_with(&self.shared, &self.config)
    }

    fn fetch_history(
        &mut self,
        request: &HistoryQuery,
    ) -> Result<HistoryDocument, HistoryFetchError> {
        fetch_history_with(&self.shared, &self.config, request)
    }

    fn authenticated_server(&self) -> Option<ServerId> {
        lock(&self.shared).server.clone()
    }

    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        let wire: ClientWire = serde_json::from_slice(&frame)
            .map_err(|error| TransportError::Io(format!("malformed client frame: {error}")))?;
        let message = match wire {
            ClientWire::Hello(_) => None,
            ClientWire::Frame(frame) => {
                if frame.protocol != self.versions.protocol || frame.schema != self.versions.schema
                {
                    return Err(TransportError::Io(format!(
                        "client frame uses {}/{} but the REST transport serves {}/{}",
                        frame.protocol, frame.schema, self.versions.protocol, self.versions.schema
                    )));
                }
                Some(frame.message)
            }
        };
        {
            let mut state = lock(&self.shared);
            let generation = state.generation;
            let job = match message {
                None => Job::Hello { generation },
                Some(ClientMessage::OpenObservation(request)) => {
                    validate_open_request(&request)?;
                    Job::Open {
                        generation,
                        request,
                    }
                }
                Some(ClientMessage::Command(envelope)) => Job::Command {
                    generation,
                    envelope: Box::new(envelope),
                },
                Some(ClientMessage::LookupReceipt(request)) => Job::Lookup {
                    generation,
                    request: Box::new(request),
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
            state.refresh = None;
            state.catalog_reply = None;
            state.history_reply = None;
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
    /// Granted scope held for one explicit snapshot refresh after a delta gap.
    refresh: Option<RefreshSnapshot>,
    /// Reply slot for one catalog fetch; the worker fills it directly.
    catalog_reply: Option<Result<CatalogDocument, TransportError>>,
    /// Reply slot for one history fetch; the worker fills it directly.
    history_reply: Option<Result<HistoryDocument, HistoryFetchError>>,
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
            refresh: None,
            catalog_reply: None,
            history_reply: None,
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
    Command {
        generation: u64,
        envelope: Box<CommandEnvelope>,
    },
    Lookup {
        generation: u64,
        request: Box<AuthorizedReceiptLookup>,
    },
    Catalog {
        generation: u64,
    },
    History {
        generation: u64,
        request: Box<HistoryQuery>,
        context: ReadContextId,
    },
}

impl Job {
    fn generation(&self) -> u64 {
        match self {
            Self::Hello { generation }
            | Self::Open { generation, .. }
            | Self::Command { generation, .. }
            | Self::Lookup { generation, .. }
            | Self::Catalog { generation }
            | Self::History { generation, .. } => *generation,
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
    scope: ObservationScope,
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
    Command {
        generation: u64,
        envelope: Box<CommandEnvelope>,
    },
    Lookup {
        generation: u64,
        request: Box<AuthorizedReceiptLookup>,
    },
    Catalog,
    History {
        request: Box<HistoryQuery>,
        context: ReadContextId,
    },
    Poll(PollSession),
}

fn lock(shared: &SharedState) -> MutexGuard<'_, Shared> {
    shared.0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Frozen-contract admission for one observation open.
///
/// The list is bounded at [`MAX_RETAINED_COMMANDS`] distinct identities; a
/// duplicate or oversized list is refused before the executor is involved, so
/// no partial or truncated request reaches the Runtime.
fn validate_open_request(request: &OpenAuthorizedObservation) -> Result<(), TransportError> {
    let retained = request.retained_commands.as_slice();
    if retained.len() > MAX_RETAINED_COMMANDS {
        return Err(TransportError::Io(format!(
            "observation request carries {} retained command identities; the contract bounds the list at {MAX_RETAINED_COMMANDS}",
            retained.len()
        )));
    }
    let mut unique = HashSet::with_capacity(retained.len());
    if retained.iter().any(|command| !unique.insert(command)) {
        return Err(TransportError::Io(
            "observation request repeats a retained command identity".to_owned(),
        ));
    }
    Ok(())
}

/// All-or-none resume coordinates.
///
/// The Runtime contract requires the scope identity, the last completely applied
/// cursor and the expected epoch together; a partial triplet is refused as an
/// invalid request (422) because a lone scope identity is not a resume request.
/// The Protocol wire carries the three fields independently, and an ordinary
/// fresh re-open still knows its previous scope id, so this adapter normalises a
/// partial triplet to a fresh open instead of forwarding an invalid resume.
fn resume_coordinates(
    observation: &desktop_protocol::OpenObservation,
) -> (Option<&ScopeId>, Option<&Cursor>, Option<&EpochId>) {
    match (
        observation.scope_id.as_ref(),
        observation.last_seen_cursor.as_ref(),
        observation.epoch.as_ref(),
    ) {
        (Some(scope_id), Some(cursor), Some(epoch)) => (Some(scope_id), Some(cursor), Some(epoch)),
        _ => (None, None, None),
    }
}

/// A granted scope whose next fresh open may be served by the snapshot route.
struct RefreshSnapshot {
    scope: ObservationScope,
    context: ReadContextId,
    stamp: ReadStamp,
}

/// Whether an open request asks for the same scope the grant describes.
fn scope_request_matches(request: &ObservationScopeRequest, granted: &ObservationScope) -> bool {
    let requested_tasks: HashSet<&desktop_protocol::TaskId> = request.task_ids.iter().collect();
    let granted_tasks: HashSet<&desktop_protocol::TaskId> = granted.task_ids.iter().collect();
    requested_tasks == granted_tasks
        && request.filters.len() == granted.filters.len()
        && request
            .filters
            .iter()
            .all(|filter| granted.filters.contains(filter))
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

/// Blocking catalog fetch over the worker thread (no observation context needed).
fn fetch_catalog_with(
    shared: &SharedState,
    config: &ProtocolTransportConfig,
) -> Result<CatalogDocument, TransportError> {
    let deadline = Instant::now() + CATALOG_FETCH_TIMEOUT;
    {
        let mut state = lock(shared);
        if state.closed {
            return Err(TransportError::Disconnected);
        }
        state.catalog_reply = None;
        let generation = state.generation;
        enqueue_job(
            &mut state,
            Job::Catalog { generation },
            config.max_pending_requests,
        )?;
    }
    shared.1.notify_all();
    loop {
        let mut state = lock(shared);
        if let Some(outcome) = state.catalog_reply.take() {
            return outcome;
        }
        if state.closed {
            return Err(TransportError::Disconnected);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(TransportError::Io("catalog request timed out".to_owned()));
        }
        let (_state, _timeout) = shared
            .1
            .wait_timeout(state, remaining.min(Duration::from_millis(50)))
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// Blocking history fetch over the worker thread (bound to the live read
/// context, which the caller never supplies and cannot fabricate).
fn fetch_history_with(
    shared: &SharedState,
    config: &ProtocolTransportConfig,
    request: &HistoryQuery,
) -> Result<HistoryDocument, HistoryFetchError> {
    let deadline = Instant::now() + Duration::from_secs(30);
    {
        let mut state = lock(shared);
        if state.closed {
            return Err(HistoryFetchError::Unavailable);
        }
        // The route is context-bound: without a live grant the Runtime would
        // answer 401, so the read is refused locally with that same class.
        let Some(context) = state.poll.as_ref().map(|poll| poll.context.clone()) else {
            return Err(HistoryFetchError::Unauthorized);
        };
        state.history_reply = None;
        let generation = state.generation;
        enqueue_job(
            &mut state,
            Job::History {
                generation,
                request: Box::new(request.clone()),
                context,
            },
            config.max_pending_requests,
        )
        .map_err(|_| HistoryFetchError::Unavailable)?;
    }
    shared.1.notify_all();
    loop {
        let mut state = lock(shared);
        if let Some(outcome) = state.history_reply.take() {
            return outcome;
        }
        if state.closed {
            return Err(HistoryFetchError::Unavailable);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HistoryFetchError::Unavailable);
        }
        let (_state, _timeout) = shared
            .1
            .wait_timeout(state, remaining.min(Duration::from_millis(50)))
            .unwrap_or_else(PoisonError::into_inner);
    }
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
                        Job::Command {
                            generation,
                            envelope,
                        } => WorkerAction::Command {
                            generation,
                            envelope,
                        },
                        Job::Lookup {
                            generation,
                            request,
                        } => WorkerAction::Lookup {
                            generation,
                            request,
                        },
                        Job::Catalog { .. } => WorkerAction::Catalog,
                        Job::History {
                            generation: _,
                            request,
                            context,
                        } => WorkerAction::History { request, context },
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
            WorkerAction::Catalog => {
                // A catalog read is credential-bound, not session-bound: it is
                // served even while no observation grant is open, but never
                // after the transport is closed.
                let outcome = get_catalog(&mut executor, &config);
                let mut state = lock(&shared);
                if state.closed {
                    continue;
                }
                state.catalog_reply = Some(outcome);
                shared.1.notify_all();
            }
            WorkerAction::History { request, context } => {
                // A history read is bound to the observation read context that
                // authorized it. A stream reset after the request was enqueued
                // invalidates that binding: the read is answered as unauthorized
                // instead of being presented under a context the Runtime no
                // longer holds.
                let generation = lock(&shared).generation;
                let outcome = get_history(&mut executor, &config, &request, &context);
                let mut state = lock(&shared);
                if state.closed {
                    continue;
                }
                if generation != state.generation {
                    state.history_reply = Some(Err(HistoryFetchError::Unauthorized));
                } else {
                    state.history_reply = Some(outcome);
                }
                shared.1.notify_all();
            }
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
                // After a delta-gap resync the next fresh open of the same scope
                // is served by one explicit snapshot refresh under the held grant.
                // Any refusal or identity mismatch falls through to a fresh open,
                // which mints a new grant.
                let refresh = {
                    let mut state = lock(&shared);
                    if state.closed || generation != state.generation {
                        continue;
                    }
                    if resume_coordinates(&request.observation).1.is_some() {
                        state.refresh = None;
                        None
                    } else {
                        state.refresh.take().filter(|refresh| {
                            scope_request_matches(&request.observation.scope, &refresh.scope)
                        })
                    }
                };
                if let Some(refresh) = refresh
                    && let Ok(Some(snapshot)) = get_snapshot(&mut executor, &config, &refresh)
                {
                    let mut state = lock(&shared);
                    if state.closed || generation != state.generation {
                        continue;
                    }
                    let identity_matches = state.hello.as_ref().is_some_and(|hello| {
                        snapshot.end.epoch == hello.epoch
                            && snapshot.end.scope.scope_id == refresh.scope.scope_id
                    });
                    if identity_matches {
                        match apply_snapshot_refresh(&mut state, &snapshot, &refresh) {
                            Ok(()) => {}
                            Err(error) => state.pending_error = Some(error),
                        }
                        wake_after(state);
                        continue;
                    }
                    // A refresh document describing another identity is never
                    // published; the fresh open below recovers.
                }
                let outcome = post_open(&mut executor, &config, versions, &request);
                let mut state = lock(&shared);
                if state.closed || generation != state.generation {
                    continue;
                }
                match outcome {
                    Ok(OpenOutcome::Document(document)) => match apply_open(
                        &mut state,
                        &document,
                        resume_coordinates(&request.observation).1,
                    ) {
                        Ok(()) => {}
                        Err(error) => state.pending_error = Some(error),
                    },
                    Ok(OpenOutcome::Resync(payload)) => {
                        state.poll = None;
                        match encode_frame(
                            versions.protocol,
                            versions.schema,
                            None,
                            ServerMessage::ResyncRequired(payload),
                        ) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                    }
                    Err(error) => state.pending_error = Some(error),
                }
                wake_after(state);
            }
            WorkerAction::Command {
                generation,
                envelope,
            } => {
                // A command is bound to the current read context; without one it
                // is refused rather than presented unauthenticated.
                let binding = {
                    let state = lock(&shared);
                    if state.closed || generation != state.generation {
                        continue;
                    }
                    state
                        .poll
                        .as_ref()
                        .map(|poll| (poll.context.clone(), poll.stamp.clone()))
                };
                let Some((context, stamp)) = binding else {
                    let mut state = lock(&shared);
                    if !state.closed && generation == state.generation {
                        state.pending_error = Some(TransportError::Io(
                            "command requires a live observation binding".to_owned(),
                        ));
                    }
                    continue;
                };
                let outcome = post_command(&mut executor, &config, &envelope, &context);
                let mut state = lock(&shared);
                if state.closed || generation != state.generation {
                    continue;
                }
                match outcome {
                    Ok(CommandOutcome::Receipt(receipt)) => {
                        let read_stamp = Some(receipt.stamp.clone());
                        match encode_frame(
                            versions.protocol,
                            versions.schema,
                            read_stamp,
                            ServerMessage::Receipt(*receipt),
                        ) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                    }
                    Ok(CommandOutcome::Error(error)) => {
                        // A definitive command refusal is state-bearing: it carries
                        // the current binding stamp so the client may settle it.
                        let read_stamp = error.command_id.is_some().then(|| stamp.clone());
                        match encode_frame(
                            versions.protocol,
                            versions.schema,
                            read_stamp,
                            ServerMessage::Error(error),
                        ) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                    }
                    Err(error) => state.pending_error = Some(error),
                }
                wake_after(state);
            }
            WorkerAction::Lookup {
                generation,
                request,
            } => {
                let outcome = post_lookup(&mut executor, &config, &request);
                let mut state = lock(&shared);
                if state.closed || generation != state.generation {
                    continue;
                }
                match outcome {
                    Ok(LookupOutcome::Result(result)) => {
                        let read_stamp = Some(result.stamp.clone());
                        match encode_frame(
                            versions.protocol,
                            versions.schema,
                            read_stamp,
                            ServerMessage::ReceiptReadResult(*result),
                        ) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                    }
                    Ok(LookupOutcome::Error(error)) => {
                        let read_stamp = error.command_id.is_some().then(|| request.stamp.clone());
                        match encode_frame(
                            versions.protocol,
                            versions.schema,
                            read_stamp,
                            ServerMessage::Error(error),
                        ) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                    }
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
                    // The Runtime states why replay is not possible. The reason
                    // is published as a protocol frame and the client's bounded
                    // recovery re-opens the observation; no cursor is fabricated.
                    Ok(DeltaOutcome::Resync(payload)) => {
                        state.refresh = Some(RefreshSnapshot {
                            scope: poll.scope.clone(),
                            context: poll.context.clone(),
                            stamp: poll.stamp.clone(),
                        });
                        state.poll = None;
                        match encode_frame(
                            poll.protocol,
                            poll.schema,
                            None,
                            ServerMessage::ResyncRequired(payload),
                        ) {
                            Ok(frame) => push_inbound(&mut state, frame),
                            Err(error) => state.pending_error = Some(error),
                        }
                        wake_after(state);
                    }
                    // A transport failure is not a resync decision: publish
                    // nothing and let the client's recovery decide.
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
fn apply_open(
    state: &mut Shared,
    document: &OpenDocument,
    requested_resume: Option<&Cursor>,
) -> Result<(), TransportError> {
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
    let Some(grant) = document.grant.as_ref() else {
        return Err(TransportError::Io(
            "observation document carries no read grant".to_owned(),
        ));
    };
    let protocol = document.server.protocol;
    let schema = document.server.schema;
    let stamp = Some(grant.stamp.clone());
    match (&document.snapshot, &document.replay) {
        (Some(snapshot), None) => {
            let mut frames = Vec::with_capacity(3 + snapshot.chunks.len());
            frames.push(encode_frame(
                protocol,
                schema,
                None,
                ServerMessage::ReadGrant(grant.clone()),
            )?);
            frames.push(encode_frame(
                protocol,
                schema,
                stamp.clone(),
                ServerMessage::SnapshotBegin(snapshot.begin.clone()),
            )?);
            for chunk in &snapshot.chunks {
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
                ServerMessage::SnapshotEnd(snapshot.end.clone()),
            )?);
            for frame in frames {
                push_inbound(state, frame);
            }
            state.poll = Some(PollSession {
                generation: state.generation,
                protocol,
                schema,
                cursor: snapshot.end.cursor.clone(),
                context: grant.stamp.context_id.clone(),
                stamp: grant.stamp.clone(),
                scope: grant.scope.clone(),
                first: true,
            });
            Ok(())
        }
        (None, Some(replay)) => {
            let Some(requested) = requested_resume else {
                return Err(TransportError::Io(
                    "replay acceptance arrived without a resume request".to_owned(),
                ));
            };
            if &replay.cursor != requested {
                return Err(TransportError::Io(
                    "replay acceptance cursor differs from the requested cursor".to_owned(),
                ));
            }
            let frames = [
                encode_frame(
                    protocol,
                    schema,
                    None,
                    ServerMessage::ReadGrant(grant.clone()),
                )?,
                encode_frame(
                    protocol,
                    schema,
                    stamp,
                    ServerMessage::ReplayAccepted(replay.clone()),
                )?,
            ];
            for frame in frames {
                push_inbound(state, frame);
            }
            state.poll = Some(PollSession {
                generation: state.generation,
                protocol,
                schema,
                cursor: replay.cursor.clone(),
                context: grant.stamp.context_id.clone(),
                stamp: grant.stamp.clone(),
                scope: grant.scope.clone(),
                first: true,
            });
            Ok(())
        }
        _ => Err(TransportError::Io(
            "observation document must carry exactly one snapshot or replay".to_owned(),
        )),
    }
}

/// Publish one fresh snapshot read under the still-current grant binding.
///
/// The snapshot route returns only the bundle; the client keeps the binding the
/// last `READ_GRANT` installed, so the frames carry that same stamp and no new
/// grant is invented.
fn apply_snapshot_refresh(
    state: &mut Shared,
    snapshot: &SnapshotDocument,
    refresh: &RefreshSnapshot,
) -> Result<(), TransportError> {
    let Some(hello) = state.hello.as_ref() else {
        return Err(TransportError::Io(
            "snapshot refresh arrived before an accepted handshake".to_owned(),
        ));
    };
    let protocol = hello.protocol;
    let schema = hello.schema;
    let stamp = Some(refresh.stamp.clone());
    let mut frames = Vec::with_capacity(2 + snapshot.chunks.len());
    frames.push(encode_frame(
        protocol,
        schema,
        stamp.clone(),
        ServerMessage::SnapshotBegin(snapshot.begin.clone()),
    )?);
    for chunk in &snapshot.chunks {
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
        ServerMessage::SnapshotEnd(snapshot.end.clone()),
    )?);
    for frame in frames {
        push_inbound(state, frame);
    }
    state.poll = Some(PollSession {
        generation: state.generation,
        protocol,
        schema,
        cursor: snapshot.end.cursor.clone(),
        context: refresh.context.clone(),
        stamp: refresh.stamp.clone(),
        scope: snapshot.end.scope.clone(),
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
    /// Server-issued scope identity from the previous session, when resuming.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_id: Option<&'a ScopeId>,
    /// Last completely applied cursor, when resuming.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_cursor: Option<&'a Cursor>,
    /// Expected Runtime epoch, when resuming.
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<&'a EpochId>,
    retained_commands: &'a [desktop_protocol::CommandId],
}

#[derive(Deserialize)]
struct OpenDocument {
    schema_version: u32,
    server: HelloAccepted,
    #[serde(default)]
    grant: Option<ReadGrant>,
    #[serde(default)]
    snapshot: Option<SnapshotDocument>,
    #[serde(default)]
    replay: Option<ReplayAccepted>,
}

#[derive(Deserialize)]
struct SnapshotDocument {
    begin: SnapshotBegin,
    #[serde(default)]
    chunks: Vec<SnapshotChunk>,
    end: SnapshotEnd,
}

/// Document served by the explicit fresh-snapshot read (bundle only, no grant).
#[derive(Deserialize)]
struct SnapshotRefreshDocument {
    schema_version: u32,
    snapshot: SnapshotDocument,
}

#[derive(Deserialize)]
struct DeltasDocument {
    schema_version: u32,
    #[serde(default)]
    transaction: Option<DeltaTransaction>,
}

/// One authorized command receipt document (`schema_version` optional envelope).
#[derive(Deserialize)]
struct CommandReceiptDocument {
    #[serde(default = "document_schema_version")]
    schema_version: u32,
    #[serde(flatten)]
    receipt: AuthorizedCommandReceipt,
}

/// One authorized receipt-lookup result document (`result` is the wrapper key).
#[derive(Deserialize)]
struct ReceiptReadDocument {
    #[serde(default = "document_schema_version")]
    schema_version: u32,
    result: ReceiptReadResult,
}

fn document_schema_version() -> u32 {
    OBSERVATION_DOCUMENT_SCHEMA_VERSION
}

enum OpenOutcome {
    Document(Box<OpenDocument>),
    Resync(ResyncRequired),
}

enum CommandOutcome {
    Receipt(Box<AuthorizedCommandReceipt>),
    Error(ProtocolError),
}

enum LookupOutcome {
    Result(Box<ReceiptReadResult>),
    Error(ProtocolError),
}

enum DeltaOutcome {
    Empty,
    Transaction(Box<DeltaTransaction>),
    Resync(ResyncRequired),
}

fn get_catalog<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
) -> Result<CatalogDocument, TransportError> {
    let mut headers = vec![("accept".to_owned(), "application/json".to_owned())];
    authorize(config, &mut headers, CATALOG_PATH);
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Get,
            path: CATALOG_PATH.to_owned(),
            headers,
            body: Vec::new(),
        })
        .map_err(|message| TransportError::Io(format!("catalog request failed: {message}")))?;
    // The byte bound applies to every response, error documents included.
    bound_response(config, &response, "catalog")?;
    // Authentication/authorization refusals keep their taxonomy: they are not
    // generic transport I/O failures and must reach the caller as such.
    match response.status {
        401 => return Err(TransportError::Unauthorized),
        403 => return Err(TransportError::Forbidden),
        _ => {}
    }
    if !is_success(response.status) {
        return Err(http_error("catalog", &response));
    }
    let document: CatalogDocument = serde_json::from_slice(&response.body)
        .map_err(|error| TransportError::Io(format!("malformed catalog document: {error}")))?;
    if document.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION
        || document.catalog.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION
    {
        return Err(TransportError::Io(format!(
            "unsupported catalog document schema version {}",
            document.schema_version
        )));
    }
    Ok(document)
}

/// One authenticated task history page over the plain-loopback transport.
///
/// The request carries the Desktop credential and the live `X-Cos-Context`
/// header; the answer is bounded before parsing, matched to the requested task
/// and read context, and validated against the closed keyset contract. Every
/// failure maps to a value-free category and no response body is echoed.
fn get_history<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
    request: &HistoryQuery,
    context: &ReadContextId,
) -> Result<HistoryDocument, HistoryFetchError> {
    let path = history_path(request);
    let mut headers = vec![
        ("accept".to_owned(), "application/json".to_owned()),
        ("x-cos-context".to_owned(), context.as_str().to_owned()),
    ];
    authorize(config, &mut headers, &path);
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Get,
            path,
            headers,
            body: Vec::new(),
        })
        .map_err(|_| HistoryFetchError::Unavailable)?;
    if response.body.len() as u64 > config.max_response_bytes {
        return Err(HistoryFetchError::Oversized);
    }
    if !is_success(response.status) {
        return Err(HistoryFetchError::from_http(
            response.status,
            error_code(&response).as_deref(),
        ));
    }
    let document: HistoryDocument =
        serde_json::from_slice(&response.body).map_err(|_| HistoryFetchError::Malformed)?;
    document.validate()?;
    if document.task_ref != request.task_ref().as_str()
        || document.stamp.context_id != context.as_str()
    {
        // A page for another task or read binding is never accepted as this
        // request's answer.
        return Err(HistoryFetchError::Malformed);
    }
    Ok(document)
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
) -> Result<OpenOutcome, TransportError> {
    let (scope_id, last_seen_cursor, epoch) = resume_coordinates(&request.observation);
    let body = OpenRequestBody {
        schema_version: OBSERVATION_DOCUMENT_SCHEMA_VERSION,
        request_id: &request.request_id,
        protocol: versions.protocol,
        schema: versions.schema,
        scope: &request.observation.scope,
        scope_id,
        last_seen_cursor,
        epoch,
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
        if let Some(payload) = resync_required(&response) {
            return Ok(OpenOutcome::Resync(payload));
        }
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
    Ok(OpenOutcome::Document(Box::new(document)))
}

/// One explicit fresh-snapshot read under a still-granted scope and context.
///
/// `Ok(None)` means the Runtime refused the refresh (`401`/`403`/`409` or any
/// other non-success), so the caller falls back to a fresh observation open;
/// a malformed success document is a transport failure.
fn get_snapshot<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
    refresh: &RefreshSnapshot,
) -> Result<Option<SnapshotDocument>, TransportError> {
    let path = format!(
        "{SNAPSHOT_PATH}?scope={}",
        encode_query_value(refresh.scope.scope_id.as_str())
    );
    let mut headers = vec![
        ("accept".to_owned(), "application/json".to_owned()),
        (
            "x-cos-context".to_owned(),
            refresh.context.as_str().to_owned(),
        ),
    ];
    authorize(config, &mut headers, &path);
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Get,
            path,
            headers,
            body: Vec::new(),
        })
        .map_err(|message| TransportError::Io(format!("snapshot refresh failed: {message}")))?;
    bound_response(config, &response, "snapshot refresh")?;
    if !is_success(response.status) {
        return Ok(None);
    }
    let document: SnapshotRefreshDocument =
        serde_json::from_slice(&response.body).map_err(|error| {
            TransportError::Io(format!("malformed snapshot refresh document: {error}"))
        })?;
    if document.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION {
        return Err(TransportError::Io(format!(
            "unsupported snapshot refresh schema version {}",
            document.schema_version
        )));
    }
    Ok(Some(document.snapshot))
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
        if let Some(payload) = resync_required(&response) {
            return Ok(DeltaOutcome::Resync(payload));
        }
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

/// Submit one command envelope on the current read context.
fn post_command<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
    envelope: &CommandEnvelope,
    context: &ReadContextId,
) -> Result<CommandOutcome, TransportError> {
    let body = serde_json::to_vec(envelope)
        .map_err(|error| TransportError::Io(format!("cannot encode command: {error}")))?;
    let mut headers = vec![
        ("accept".to_owned(), "application/json".to_owned()),
        ("content-type".to_owned(), "application/json".to_owned()),
        ("x-cos-context".to_owned(), context.as_str().to_owned()),
    ];
    authorize(config, &mut headers, COMMANDS_PATH);
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Post,
            path: COMMANDS_PATH.to_owned(),
            headers,
            body,
        })
        .map_err(|message| TransportError::Io(format!("command request failed: {message}")))?;
    bound_response(config, &response, "command")?;
    if !is_success(response.status) {
        return Ok(CommandOutcome::Error(protocol_refusal(
            "command",
            &response,
            Some(envelope.command_id.clone()),
            refusal_is_definitive,
        )));
    }
    let document: CommandReceiptDocument = serde_json::from_slice(&response.body)
        .map_err(|error| TransportError::Io(format!("malformed command document: {error}")))?;
    if document.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION {
        return Err(TransportError::Io(format!(
            "unsupported command document schema version {}",
            document.schema_version
        )));
    }
    Ok(CommandOutcome::Receipt(Box::new(document.receipt)))
}

/// Read one authorized command receipt under the lookup's own binding.
fn post_lookup<E: ProtocolHttp>(
    executor: &mut E,
    config: &ProtocolTransportConfig,
    request: &AuthorizedReceiptLookup,
) -> Result<LookupOutcome, TransportError> {
    let body = serde_json::to_vec(request)
        .map_err(|error| TransportError::Io(format!("cannot encode receipt lookup: {error}")))?;
    let mut headers = vec![
        ("accept".to_owned(), "application/json".to_owned()),
        ("content-type".to_owned(), "application/json".to_owned()),
        (
            "x-cos-context".to_owned(),
            request.stamp.context_id.as_str().to_owned(),
        ),
    ];
    authorize(config, &mut headers, COMMANDS_LOOKUP_PATH);
    let response = executor
        .execute(HttpRequest {
            method: HttpMethod::Post,
            path: COMMANDS_LOOKUP_PATH.to_owned(),
            headers,
            body,
        })
        .map_err(|message| TransportError::Io(format!("receipt lookup failed: {message}")))?;
    bound_response(config, &response, "receipt lookup")?;
    if !is_success(response.status) {
        return Ok(LookupOutcome::Error(protocol_refusal(
            "receipt lookup",
            &response,
            Some(request.command_id.clone()),
            lookup_refusal_is_definitive,
        )));
    }
    let document: ReceiptReadDocument = serde_json::from_slice(&response.body)
        .map_err(|error| TransportError::Io(format!("malformed receipt read document: {error}")))?;
    if document.schema_version != OBSERVATION_DOCUMENT_SCHEMA_VERSION {
        return Err(TransportError::Io(format!(
            "unsupported receipt read document schema version {}",
            document.schema_version
        )));
    }
    Ok(LookupOutcome::Result(Box::new(document.result)))
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

/// Stable Runtime error code from one bounded error document, when present.
fn error_code(response: &HttpResponse) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| {
            value
                .get("code")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

/// One `RESYNC_REQUIRED` refusal, mapped to the protocol's closed reason.
///
/// The Runtime states the stable code; a recognized `reason` is preserved, and
/// an absent or unrecognized one falls back to `HISTORY_UNAVAILABLE` (the
/// closed meaning: this history cannot be replayed). The safe `detail` text is
/// bounded and control-free; nothing else from the body is copied.
fn resync_required(response: &HttpResponse) -> Option<ResyncRequired> {
    if error_code(response).as_deref() != Some("RESYNC_REQUIRED") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&response.body).ok()?;
    let reason = value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(ResyncReason::from_wire)
        .filter(ResyncReason::is_known)
        .unwrap_or(ResyncReason::HistoryUnavailable);
    let message = value
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .map(safe_message);
    Some(ResyncRequired { reason, message })
}

/// Bound one Runtime safe message for a protocol frame.
fn safe_message(raw: &str) -> String {
    raw.chars()
        .filter(|character| !character.is_control())
        .take(MAX_RESYNC_MESSAGE_CHARS)
        .collect()
}

fn http_error(what: &str, response: &HttpResponse) -> TransportError {
    // Only the status and the stable code are classified; the body text never
    // reaches a message, so a diagnostic cannot leak a Runtime detail string.
    let code = error_code(response);
    match (response.status, code) {
        (422, Some(code)) => TransportError::Io(format!("{what} rejected: HTTP 422 {code}")),
        (422, None) => TransportError::Io(format!("{what} rejected: HTTP 422")),
        (status, Some(code)) => {
            TransportError::Io(format!("{what} rejected: HTTP {status} {code}"))
        }
        (status, None) => TransportError::Io(format!("{what} rejected: HTTP {status}")),
    }
}

/// Stable `ErrorCode` for a Runtime refusal code, normalizing legacy spellings.
fn map_error_code(raw: &str) -> ErrorCode {
    match raw {
        "INTERNAL" => ErrorCode::InternalError,
        // The Runtime seals read contexts separately; the protocol client only
        // knows the session-wide Unauthorized class, and closing the binding is
        // the correct response to a rejected context.
        "UNAUTHORIZED_CONTEXT" => ErrorCode::Unauthorized,
        other => ErrorCode::from_wire(other),
    }
}

/// Whether a refusal is a definitive verdict the client may settle a command on.
///
/// Session-wide authorization, resync and internal failures are not verdicts:
/// the command stays unresolved and no state-bearing error is fabricated.
fn refusal_is_definitive(code: &ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::CommandIdConflict
            | ErrorCode::RevisionConflict
            | ErrorCode::ValidationFailed
            | ErrorCode::UnsupportedCommand
            | ErrorCode::ReceiptExpired
            | ErrorCode::MediaUnavailable
    )
}

/// A receipt lookup refusal only settles the command when the receipt expired.
fn lookup_refusal_is_definitive(code: &ErrorCode) -> bool {
    matches!(code, ErrorCode::ReceiptExpired)
}

/// Turn one bounded Runtime refusal into the protocol's structured error.
///
/// Only the stable code, the bounded safe detail and the optional structured
/// fields are copied; the raw body never reaches a message. `command_id` is
/// attached only when the code is a definitive verdict (`is_definitive`), so a
/// resync/internal failure cannot masquerade as a command failure.
fn protocol_refusal(
    what: &str,
    response: &HttpResponse,
    command_id: Option<desktop_protocol::CommandId>,
    is_definitive: fn(&ErrorCode) -> bool,
) -> ProtocolError {
    let value: Option<serde_json::Value> = serde_json::from_slice(&response.body).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_str)
        .map(map_error_code)
        .unwrap_or(ErrorCode::InternalError);
    let message = value
        .as_ref()
        .and_then(|value| value.get("detail"))
        .and_then(serde_json::Value::as_str)
        .map(safe_message)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| format!("{what} rejected: HTTP {}", response.status));
    let resync_reason = value
        .as_ref()
        .and_then(|value| value.get("reason"))
        .and_then(serde_json::Value::as_str)
        .map(ResyncReason::from_wire)
        .filter(ResyncReason::is_known);
    let current_revision = value
        .as_ref()
        .and_then(|value| value.get("current_revision"))
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| Counter::new(raw).ok());
    let retry_after_ms = value
        .as_ref()
        .and_then(|value| value.get("retry_after_ms"))
        .and_then(serde_json::Value::as_u64);
    ProtocolError {
        code: code.clone(),
        message,
        correlation_id: None,
        command_id: command_id.filter(|_| is_definitive(&code)),
        scope_id: None,
        retry_after_ms,
        field_errors: Vec::new(),
        current_revision,
        resync_reason,
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
    //! RC-03: reads and commands are principal-bound, so the credential is
    //! presented on the protocol routes and nowhere else.

    use super::{
        CATALOG_PATH, COMMANDS_LOOKUP_PATH, COMMANDS_PATH, DELTAS_PATH, OBSERVATION_PATH,
        ProtocolTransportConfig, SERVER_PATH, SNAPSHOT_PATH, authorize, is_protocol_route,
    };

    #[test]
    fn the_credential_is_presented_on_exactly_the_protocol_routes() {
        for route in [
            OBSERVATION_PATH,
            SNAPSHOT_PATH,
            DELTAS_PATH,
            COMMANDS_PATH,
            COMMANDS_LOOKUP_PATH,
            CATALOG_PATH,
        ] {
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
            "/api/cos/desktop/commands-extra",
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

    #[test]
    fn the_authenticated_catalog_document_parses_the_owner_shape() {
        use super::CatalogDocument;

        let raw = r#"{
            "schema_version": 1,
            "server": {"server_identity": "cos-runtime-orchestrator", "epoch": "epoch"},
            "catalog": {
                "schema_version": 1,
                "can_submit": true,
                "reason_codes": [],
                "recipes": [{
                    "recipe_ref": {
                        "recipe_id": "recipe-1",
                        "source_harness_version_id": "hvid_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "source_harness_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "graph_ref": {"graph_id": "graph-1", "graph_version": 7, "graph_digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
                        "profile_ref": null,
                        "projection_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    },
                    "label": "Still",
                    "description": "One still image.",
                    "input_limits": {"max_images": 2},
                    "accepts_image_references": true,
                    "execution_limits": {},
                    "constraint_summary": "tasteful",
                    "parameters": [{
                        "id": "seed",
                        "label": "Seed",
                        "type": "integer",
                        "value": 7,
                        "bounds": {"min": "1", "max": "99", "max_length": null},
                        "choices": [],
                        "editable": true
                    }]
                }]
            }
        }"#;
        let document: CatalogDocument =
            serde_json::from_str(raw).expect("authenticated catalog document");
        assert_eq!(document.schema_version, 1);
        assert_eq!(document.server.server_identity, "cos-runtime-orchestrator");
        assert!(document.catalog.can_submit);
        let recipe = &document.catalog.recipes[0];
        assert_eq!(recipe.recipe_ref.graph_ref.graph_version, 7);
        assert_eq!(recipe.parameters[0].kind, "integer");
        assert_eq!(
            recipe.parameters[0].value,
            super::CatalogParameterValue::Integer(7)
        );
    }
}
