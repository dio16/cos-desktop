//! Transport-independent COS client: atomic observation, bounded recovery and
//! honest command receipts.
//!
//! The client is driven by explicit `pump` calls (no background threads, no
//! blocking). It owns protocol encoding, staging, duplicate/gap detection,
//! resync and session-generation cancellation. A consumer (app-core) runs the
//! pump loop on a background executor and renders the immutable
//! [`ProjectionState`] behind [`CosClient::snapshot`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use desktop_protocol::{
    AuthorizedCommandReceipt, AuthorizedReceiptLookup, ClientFrame, ClientMessage, ClientWire,
    CommandEnvelope, CommandId, CommandPayload, CommandReceipt, CommandTarget, Cursor,
    DeltaTransaction, EpochId, ErrorCode, HelloAccepted, HelloRequest, IntegrityDigest, Limits,
    ObservationScope, ObservationScopeRequest, OpenAuthorizedObservation, OpenObservation,
    Projection, ProjectionMutation, ProtocolError, ProtocolVersions, ReadGrant, ReadInvalidated,
    ReadInvalidationReason, ReadStamp, ReceiptReadOutcome, ReceiptReadResult, ReceiptState,
    RemoveTarget, ReplayAccepted, ResyncReason, ResyncRequired, ServerFrame, ServerId,
    ServerMessage, ServerWire, SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotEnd,
    SnapshotId, StopTaskCommand, TaskId, TransactionId, UnknownMessage, Version,
    VersionAdvertisement, VersionRange, canonical_json, integrity_digest_of,
};

use crate::error::ClientError;
use crate::projection::{ProjectionKey, ProjectionState};
use crate::read::{
    CommandLookup, CommandStatus, MAX_DISTINCT_RETAINED, MAX_PENDING_READS, MAX_RECENT,
    MAX_REFERENCES, MAX_TOMBSTONES, MAX_UNRESOLVED, ObservationControl, PendingReceiptRead,
    PublicationAccess, ReadBinding, ReadEvidence, ReadState, ReadUnavailable, ReadableCommands,
    RetentionClass, TOMBSTONE_REASON, capacity_error, next_read_request_id, next_receipt_read_id,
    tombstone_shape, unsettled,
};
use crate::transport::Transport as TransportTrait;
use crate::transport::TransportError;

/// Immutable projection snapshot handed to consumers.
pub type SnapshotHandle = Arc<ProjectionState>;

/// Client-local connection state. A stale connection keeps the last-known
/// canonical values; it never turns an ACTIVE task into a failed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// No session; local disconnect or before the first connect.
    Disconnected,
    /// Handshake in progress.
    Negotiating,
    /// Snapshot staging or replay in progress.
    Synchronizing,
    /// Session live; deltas may be applied.
    Live,
    /// Session invalidated; a bounded-backoff recovery is scheduled.
    Reconnecting {
        /// Consecutive recovery attempt.
        attempt: u32,
        /// Delay assigned when the recovery was scheduled.
        retry_in_ms: u64,
        /// Reason for the invalidation.
        reason: String,
    },
    /// No mutually supported versions or unrecoverable incompatibility;
    /// requires manual correction.
    Incompatible {
        /// Reason.
        reason: String,
    },
    /// Authorization failure; requires manual correction.
    Unauthorized {
        /// Reason.
        reason: String,
    },
}

impl ConnectionState {
    /// Short label for diagnostics/UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Negotiating => "negotiating",
            Self::Synchronizing => "synchronizing",
            Self::Live => "live",
            Self::Reconnecting { .. } => "reconnecting",
            Self::Incompatible { .. } => "incompatible",
            Self::Unauthorized { .. } => "unauthorized",
        }
    }

    /// Whether the session is live.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

/// Where the client's data comes from. Fixture mode must stay visible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientSource {
    /// Not yet known (before handshake).
    Unknown,
    /// Scripted fixture provider; never a real runtime connection.
    Fixture {
        /// Fixture scenario identity.
        scenario_id: String,
    },
    /// Real server identity from the handshake.
    Runtime {
        /// Server identity.
        server_identity: ServerId,
    },
}

/// Local command status.
///
/// `Accepted` means the server durably recorded validated intent — not
/// semantic completion. `CanonicalReflected` links the command to a canonical
/// effect; for Stop this still distinguishes stop-request state from terminal
/// completion. `ResultUnknown` is surfaced, never resolved into success or
/// failure by the client.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandState {
    /// Local intent only; not sent.
    Prepared,
    /// Delivery attempted; no claim of acceptance.
    Pending {
        /// Local send time.
        sent_at_ms: u64,
    },
    /// Server durably recorded the validated intent.
    Accepted(CommandReceipt),
    /// Definitive rejection; no execution for that command.
    Rejected(CommandReceipt),
    /// Authoritative projection/receipt links this command to a canonical effect.
    CanonicalReflected(CommandReceipt),
    /// Definitive failure without a receipt (structured server error).
    Failed {
        /// Server error.
        error: ProtocolError,
    },
    /// Response lost, timeout or crash; execution may have happened.
    ResultUnknown {
        /// Reason.
        reason: String,
        /// Receipt, if one was later recovered.
        receipt: Option<CommandReceipt>,
    },
    /// Command family explicitly unsupported by the server.
    Unsupported {
        /// Server error.
        error: ProtocolError,
    },
}

impl CommandState {
    /// Receipt associated with this status, if any.
    pub fn receipt(&self) -> Option<&CommandReceipt> {
        match self {
            Self::Accepted(receipt)
            | Self::Rejected(receipt)
            | Self::CanonicalReflected(receipt) => Some(receipt),
            Self::ResultUnknown { receipt, .. } => receipt.as_ref(),
            _ => None,
        }
    }

    /// Whether no further server response is expected for this command.
    pub fn is_settled(&self) -> bool {
        matches!(
            self,
            Self::Accepted(_)
                | Self::Rejected(_)
                | Self::CanonicalReflected(_)
                | Self::Failed { .. }
                | Self::Unsupported { .. }
        )
    }
}

/// Observable client event produced by [`CosClient::pump_bounded`].
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    /// Connection state changed.
    StateChanged(ConnectionState),
    /// Handshake completed with the selected pair.
    HandshakeCompleted {
        /// Selected protocol version.
        protocol: Version,
        /// Selected schema version.
        schema: Version,
        /// Server identity.
        server_identity: ServerId,
        /// Runtime epoch.
        epoch: EpochId,
    },
    /// A complete snapshot was verified and swapped in atomically.
    SnapshotApplied {
        /// Snapshot identity.
        snapshot_id: SnapshotId,
        /// Verified entity counts.
        counts: SnapshotCounts,
        /// Cursor of the consistent read cut.
        cursor: Cursor,
    },
    /// A delta transaction was validated and applied atomically.
    DeltaApplied {
        /// Transaction identity.
        transaction_id: TransactionId,
        /// Published cursor.
        cursor: Cursor,
    },
    /// An exact duplicate transaction was ignored.
    DuplicateIgnored {
        /// Transaction identity.
        transaction_id: TransactionId,
    },
    /// An unknown informational event was skipped with cursor continuity.
    InformationalEventSkipped {
        /// Unknown wire kind.
        kind: String,
    },
    /// Command status changed.
    CommandUpdated {
        /// Command identity.
        command_id: CommandId,
        /// New status.
        status: CommandState,
    },
    /// A full resync was requested.
    ResyncRequested {
        /// Reason.
        reason: ResyncReason,
    },
    /// Error surfaced to the consumer (also recorded as [`CosClient::last_error`]).
    Error(ClientError),
}

/// A bounded nonblocking pump result, including event-free presentation changes.
pub struct PumpReport {
    pub frames: usize,
    pub events: Vec<ClientEvent>,
    pub changed: bool,
    pub next_deadline_ms: Option<u64>,
}

/// Client configuration.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Client release identity.
    pub desktop_version: String,
    /// Advertised protocol ranges.
    pub protocol: Vec<VersionRange>,
    /// Advertised schema ranges.
    pub schema: Vec<VersionRange>,
    /// Advertised feature tags.
    pub features: Vec<String>,
    /// Time after which a pending command becomes `ResultUnknown`.
    pub command_timeout_ms: u64,
    /// Maximum consecutive recovery attempts before `Incompatible`.
    pub max_resync_attempts: u32,
    /// Maximum consecutive queue overflows before `Incompatible`.
    pub max_consecutive_overflows: u32,
    /// Deterministic jitter seed for backoff.
    pub backoff_seed: u64,
    /// Bounded duplicate digest window.
    pub duplicate_window: usize,
    /// Bounded memory of cancelled snapshot identities.
    pub cancelled_snapshot_memory: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            desktop_version: "cos-desktop/0.1.0".to_owned(),
            protocol: vec![VersionRange::new(2, 0, 0)],
            schema: vec![VersionRange::new(2, 0, 0)],
            features: Vec::new(),
            command_timeout_ms: 30_000,
            max_resync_attempts: 5,
            max_consecutive_overflows: 3,
            backoff_seed: 0x9E37_79B9_7F4A_7C15,
            duplicate_window: 64,
            cancelled_snapshot_memory: 8,
        }
    }
}

/// Object-safe client surface for app-core/UI consumers.
pub trait DesktopClient: Send {
    /// Immutable authoritative projection snapshot.
    fn snapshot(&self) -> SnapshotHandle;
    /// Current connection state.
    fn connection(&self) -> &ConnectionState;
    /// Whether the visible projection may be stale.
    fn is_stale(&self) -> bool;
    /// Data source (fixture or runtime).
    fn source(&self) -> ClientSource;
    /// Most recent error, if any.
    fn last_error(&self) -> Option<&ClientError>;
    /// Drive the session, returning observable events.
    fn pump(&mut self, now_ms: u64) -> Vec<ClientEvent>;
    /// Local disconnect: cancels observation only, never task execution.
    fn disconnect(&mut self) -> Vec<ClientEvent>;
    /// Request a full resync.
    fn request_resync(&mut self, reason: ResyncReason) -> Vec<ClientEvent>;
    /// Send a Stop request for a task.
    fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, ClientError>;
    /// Bounded background progress and exact next deadline.
    fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport;
    /// The single authoritative retained registry, not an authorized UI view.
    fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus>;
    fn lookup_command(&self, id: &CommandId) -> CommandLookup<'_>;
    fn retain_command(&mut self, id: &CommandId) -> Result<(), ClientError>;
    fn release_command(&mut self, id: &CommandId);
    fn begin_authorized_observation(
        &mut self,
        request: OpenAuthorizedObservation,
    ) -> Result<(), ClientError>;
    fn publication_access(&self) -> PublicationAccess<'_>;
    fn lookup_receipt_authorized(
        &mut self,
        request: AuthorizedReceiptLookup,
    ) -> Result<(), ClientError>;
    fn bind_observation_control(&mut self, control: ObservationControl);
    /// Build a fresh authorized observation request for a scope intent.
    fn observation_request(&self, scope: ObservationScopeRequest) -> OpenAuthorizedObservation;
    /// Adopt a transferred registry and its surface references.
    fn adopt_retained_registry(
        &mut self,
        records: BTreeMap<CommandId, CommandStatus>,
        references: BTreeMap<CommandId, usize>,
    ) -> Result<(), ClientError>;
}

struct Session {
    versions: ProtocolVersions,
    server_identity: ServerId,
    server_build: String,
    epoch: EpochId,
    dedupe_retention_ms: Option<u64>,
}

struct Staging {
    snapshot_id: SnapshotId,
    generation: u64,
    mutations: Vec<ProjectionMutation>,
    bundle_bytes: u64,
    next_part: u32,
}

struct AppliedTransaction {
    transaction_id: TransactionId,
    cursor: Cursor,
    digest: IntegrityDigest,
}

#[derive(Default)]
struct ReconnectPlan {
    attempt: u32,
    next_at_ms: Option<u64>,
    resume: bool,
    reason: Option<ResyncReason>,
}

/// Transport-independent COS client.
pub struct CosClient<T: TransportTrait> {
    transport: T,
    config: ClientConfig,
    limits: Limits,
    source: ClientSource,
    state: ConnectionState,
    generation: u64,
    now_ms: u64,
    session: Option<Session>,
    requested_scope: Option<ObservationScopeRequest>,
    scope: Option<ObservationScope>,
    resume_cursor: Option<Cursor>,
    projection: SnapshotHandle,
    projection_stale: bool,
    staging: Option<Staging>,
    last_seen_cursor: Option<Cursor>,
    applied: VecDeque<AppliedTransaction>,
    cancelled_snapshots: VecDeque<SnapshotId>,
    commands: BTreeMap<CommandId, CommandStatus>,
    reconnect: ReconnectPlan,
    consecutive_overflows: u32,
    last_error: Option<ClientError>,
    read: ReadState,
    changed: bool,
    settlement_sequence: u64,
    capacity_blocked: bool,
}

impl<T: TransportTrait> CosClient<T> {
    /// Create a client over a transport.
    pub fn new(transport: T, config: ClientConfig, limits: Limits, source: ClientSource) -> Self {
        Self {
            transport,
            config,
            limits,
            source,
            state: ConnectionState::Disconnected,
            generation: 0,
            now_ms: 0,
            session: None,
            requested_scope: None,
            scope: None,
            resume_cursor: None,
            projection: Arc::new(ProjectionState::default()),
            projection_stale: true,
            staging: None,
            last_seen_cursor: None,
            applied: VecDeque::new(),
            cancelled_snapshots: VecDeque::new(),
            commands: BTreeMap::new(),
            reconnect: ReconnectPlan::default(),
            consecutive_overflows: 0,
            last_error: None,
            read: ReadState::default(),
            changed: true,
            settlement_sequence: 0,
            capacity_blocked: false,
        }
    }

    /// Immutable authoritative projection snapshot.
    pub fn snapshot(&self) -> SnapshotHandle {
        Arc::clone(&self.projection)
    }

    /// Current connection state.
    pub fn connection(&self) -> &ConnectionState {
        &self.state
    }

    /// Whether the visible projection may be stale.
    pub fn is_stale(&self) -> bool {
        self.projection_stale
    }

    /// Data source.
    pub fn source(&self) -> ClientSource {
        self.source.clone()
    }

    /// Most recent error.
    pub fn last_error(&self) -> Option<&ClientError> {
        self.last_error.as_ref()
    }

    /// Server-issued scope of the current observation, when synchronized.
    pub fn scope(&self) -> Option<&ObservationScope> {
        self.scope.as_ref()
    }

    /// Last completely applied cursor.
    pub fn last_seen_cursor(&self) -> Option<&Cursor> {
        self.last_seen_cursor.as_ref()
    }

    /// Session runtime epoch.
    pub fn session_epoch(&self) -> Option<&EpochId> {
        self.session.as_ref().map(|session| &session.epoch)
    }

    /// Negotiated server identity, once the handshake completed.
    pub fn server_identity(&self) -> Option<&ServerId> {
        self.session
            .as_ref()
            .map(|session| &session.server_identity)
    }

    /// Server build label from the handshake.
    pub fn server_build(&self) -> Option<&str> {
        self.session
            .as_ref()
            .map(|session| session.server_build.as_str())
    }

    /// Advertised dedupe retention window, when the server guarantees one.
    ///
    /// Without this guarantee a lost response stays `ResultUnknown`; the
    /// client never auto-resends a mutating command.
    pub fn dedupe_retention_ms(&self) -> Option<u64> {
        self.session
            .as_ref()
            .and_then(|session| session.dedupe_retention_ms)
    }

    /// Negotiated limits.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Borrow the transport (fixture inspection, tests).
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Mutably borrow the transport.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// The single authoritative retained registry.
    ///
    /// A retention fact for the owning background executor, not an
    /// authorization-filtered view; consumers use [`Self::publication_access`].
    pub fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus> {
        &self.commands
    }

    /// Retain one surface reference to a command record.
    pub fn retain_command(&mut self, id: &CommandId) -> Result<(), ClientError> {
        if !self.commands.contains_key(id) {
            return Err(ClientError::InvalidCommand(format!(
                "command {id} is not retained"
            )));
        }
        if self.total_references() >= MAX_REFERENCES {
            return Err(ClientError::InvalidCommand(
                "CommandReferenceLimitExceeded".to_owned(),
            ));
        }
        if let Some(record) = self.commands.get_mut(id) {
            record.references += 1;
        }
        Ok(())
    }

    /// Release one surface reference to a command record.
    pub fn release_command(&mut self, id: &CommandId) {
        if let Some(record) = self.commands.get_mut(id) {
            record.references = record.references.saturating_sub(1);
        }
    }

    /// Number of identity-level read-evidence entries held right now.
    pub fn read_evidence_len(&self) -> usize {
        self.read.read_evidence.len()
    }

    /// Whether current evidence exists for one identity.
    pub fn read_evidence_has(&self, id: &CommandId) -> bool {
        self.read.read_evidence.contains_key(id)
    }

    /// Known command identities in order.
    pub fn command_ids(&self) -> Vec<CommandId> {
        self.commands.keys().cloned().collect()
    }

    /// Bind the actual close-only publication fence supplied by app-core.
    pub fn bind_observation_control(&mut self, control: ObservationControl) {
        self.transport.bind_observation_control(control.clone());
        self.read.control = Some(control);
    }

    /// Begin (or restart) observation with explicit authorized retention interest.
    ///
    /// Local scope intent closes the shared visibility fence synchronously
    /// before any queued work or new-context data.
    pub fn begin_authorized_observation(
        &mut self,
        request: OpenAuthorizedObservation,
    ) -> Result<(), ClientError> {
        // A new authorized observation cancels the previous session: scope,
        // principal and authentication intents all take this path.
        match self.state {
            ConnectionState::Disconnected
            | ConnectionState::Incompatible { .. }
            | ConnectionState::Unauthorized { .. }
            | ConnectionState::Reconnecting { .. }
            | ConnectionState::Negotiating
            | ConnectionState::Synchronizing
            | ConnectionState::Live => {}
        }
        let mut distinct = BTreeSet::new();
        if !request
            .retained_commands
            .iter()
            .all(|id| distinct.insert(id.clone()))
        {
            return Err(ClientError::InvalidCommand(
                "retained command identities must be distinct".to_owned(),
            ));
        }
        self.close_local_visibility();
        self.requested_scope = Some(request.observation.scope.clone());
        self.read.pending_retained = request.retained_commands.clone();
        self.read.pending_request = Some(request);
        self.read.request_sent = false;
        self.reconnect = ReconnectPlan::default();
        self.consecutive_overflows = 0;
        let mut events = Vec::new();
        self.begin_session(false, &mut events)
    }

    /// Connect (or reconnect) and open the observation session.
    ///
    /// Convenience wrapper over [`Self::begin_authorized_observation`] for
    /// trusted callers that already own the scope; it submits the current
    /// retained registry as the retention interest set.
    pub fn connect(
        &mut self,
        scope: ObservationScopeRequest,
        now_ms: u64,
    ) -> Result<(), ClientError> {
        self.now_ms = now_ms;
        let request = self.observation_request(scope);
        self.begin_authorized_observation(request)
    }

    /// Build a fresh authorized observation request for a scope intent.
    ///
    /// Request identities are client-generated, non-reused correlation values;
    /// the retention interest set is the current bounded registry.
    pub fn observation_request(&self, scope: ObservationScopeRequest) -> OpenAuthorizedObservation {
        OpenAuthorizedObservation {
            request_id: next_read_request_id(),
            observation: OpenObservation {
                scope,
                scope_id: None,
                last_seen_cursor: None,
                epoch: None,
            },
            retained_commands: self.commands.keys().cloned().collect(),
        }
    }

    /// Transfer the retained registry and its surface references.
    ///
    /// Owner replacement keeps the same records and the same outstanding surface
    /// references but drops all former-session decisions: nothing is readable
    /// until a fresh matching grant arrives, and the reference budget stays
    /// aligned with the credits the owner still holds.
    pub fn adopt_retained_registry(
        &mut self,
        records: BTreeMap<CommandId, CommandStatus>,
        references: BTreeMap<CommandId, usize>,
    ) -> Result<(), ClientError> {
        if !self.commands.is_empty() {
            return Err(ClientError::InvalidCommand(
                "retained registry is not empty".to_owned(),
            ));
        }
        if records.len() > MAX_DISTINCT_RETAINED {
            return Err(capacity_error());
        }
        let total_references: usize = references.values().sum();
        if total_references > MAX_REFERENCES
            || references.values().any(|count| *count > MAX_REFERENCES)
        {
            return Err(ClientError::InvalidCommand(
                "CommandReferenceLimitExceeded".to_owned(),
            ));
        }
        for (id, mut record) in records {
            record.decision = None;
            record.references = references.get(&id).copied().unwrap_or(0);
            match record.class {
                RetentionClass::Settled(sequence) => {
                    self.settlement_sequence = self.settlement_sequence.max(sequence);
                }
                RetentionClass::Ordinary | RetentionClass::Tombstone => {}
            }
            if unsettled(&record.status)
                && record.class == RetentionClass::Ordinary
                && self.ordinary_unsettled() >= MAX_UNRESOLVED
            {
                return Err(capacity_error());
            }
            self.commands.insert(id, record);
        }
        self.changed = true;
        Ok(())
    }

    /// Drive the session with the default frame budget.
    pub fn pump(&mut self, now_ms: u64) -> Vec<ClientEvent> {
        let budget = self.limits.max_queue_frames.max(1) as usize;
        self.pump_bounded(now_ms, budget)
    }

    /// Drive the session, processing at most `max_frames` inbound frames.
    pub fn pump_bounded(&mut self, now_ms: u64, max_frames: usize) -> Vec<ClientEvent> {
        self.pump_step(now_ms, max_frames).events
    }

    /// Bounded nonblocking progress for the background owner.
    pub fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport {
        self.changed = false;
        let mut events = Vec::new();
        self.now_ms = now_ms;
        self.expire_pending(now_ms, &mut events);
        self.expire_read_deadlines(now_ms, &mut events);
        if let ConnectionState::Reconnecting { .. } = self.state
            && self.reconnect.next_at_ms.is_some_and(|at| now_ms >= at)
        {
            let resume = self.reconnect.resume;
            if let Err(error) = self.begin_session(resume, &mut events) {
                self.handle_send_error(error, &mut events);
                return PumpReport {
                    frames: 0,
                    events,
                    changed: self.changed,
                    next_deadline_ms: self.next_deadline_ms(),
                };
            }
        }
        self.drive_automatic_reads(now_ms, &mut events);
        let mut frames = 0;
        for _ in 0..budget {
            match self.transport.poll() {
                Ok(Some(bytes)) => {
                    frames += 1;
                    if !self.handle_frame(&bytes, &mut events) {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    self.handle_transport_error(error, &mut events);
                    break;
                }
            }
        }
        self.reconcile_capacity();
        PumpReport {
            frames,
            events,
            changed: self.changed,
            next_deadline_ms: self.next_deadline_ms(),
        }
    }

    /// Validate and record a command without sending it.
    pub fn prepare_command(&mut self, envelope: CommandEnvelope) -> Result<CommandId, ClientError> {
        envelope
            .validate()
            .map_err(|error| ClientError::InvalidCommand(error.to_string()))?;
        let command_id = envelope.command_id.clone();
        if self.commands.contains_key(&command_id) {
            return Err(ClientError::InvalidCommand(format!(
                "command {command_id} was already prepared; inspect its status instead of resending a new intent"
            )));
        }
        let origin = self
            .read
            .binding
            .as_ref()
            .map(|binding| Arc::clone(&binding.context));
        let mut record = CommandStatus::prepared(envelope, origin);
        match self.take_slot(&record.status) {
            Ok(class) => record.class = class,
            Err(error) => {
                self.capacity_blocked = true;
                self.last_error = Some(error.clone());
                return Err(error);
            }
        }
        self.commands.insert(command_id.clone(), record);
        self.changed = true;
        Ok(command_id)
    }

    /// Send a previously prepared command.
    pub fn send_prepared(&mut self, command_id: &CommandId) -> Result<(), ClientError> {
        let envelope = {
            let record = self.commands.get(command_id).ok_or_else(|| {
                ClientError::InvalidCommand(format!("unknown command {command_id}"))
            })?;
            record.envelope.clone().ok_or_else(|| {
                ClientError::InvalidCommand(format!(
                    "command {command_id} has no retained envelope"
                ))
            })?
        };
        self.require_live()?;
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| ClientError::NotConnected("no negotiated session".to_owned()))?;
        let versions = session.versions;
        self.send_frame(versions, ClientMessage::Command(envelope))?;
        if let Some(record) = self.commands.get_mut(command_id) {
            record.status = CommandState::Pending {
                sent_at_ms: self.now_ms,
            };
        }
        Ok(())
    }

    /// Prepare and send a command.
    pub fn send_command(&mut self, envelope: CommandEnvelope) -> Result<CommandId, ClientError> {
        let command_id = self.prepare_command(envelope)?;
        self.send_prepared(&command_id)?;
        Ok(command_id)
    }

    /// Send a Stop request.
    ///
    /// Stop is a request intent; the runtime may define it as idempotent
    /// without an expected revision, so no revision precondition is attached.
    /// Acceptance is never completion.
    pub fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, ClientError> {
        let envelope = CommandEnvelope {
            command_id: CommandId::generate(),
            target: CommandTarget::Task {
                task_id: task_id.clone(),
            },
            expected_revision: None,
            epoch: self.session.as_ref().map(|session| session.epoch.clone()),
            payload: CommandPayload::StopTask(StopTaskCommand {
                task_id: task_id.clone(),
                reason: None,
            }),
        };
        self.send_command(envelope)
    }

    /// Request a full resync with bounded backoff.
    pub fn request_resync(&mut self, reason: ResyncReason) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        self.schedule_recovery(false, Some(reason), &mut events);
        events
    }

    /// Local disconnect: cancels observation only, never task execution.
    pub fn disconnect(&mut self) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        self.cancel_staging();
        self.withhold();
        self.transport.reset_stream();
        self.generation += 1;
        self.reconnect = ReconnectPlan::default();
        self.consecutive_overflows = 0;
        let pending: Vec<CommandId> = self
            .commands
            .iter()
            .filter(|(_, record)| matches!(&record.status, CommandState::Pending { .. }))
            .map(|(command_id, _)| command_id.clone())
            .collect();
        for command_id in pending {
            if let Some(record) = self.commands.get_mut(&command_id) {
                let receipt = record.status.receipt().cloned();
                record.status = CommandState::ResultUnknown {
                    reason:
                        "local disconnect while awaiting a receipt; execution may have happened"
                            .to_owned(),
                    receipt,
                };
                events.push(ClientEvent::CommandUpdated {
                    command_id,
                    status: record.status.clone(),
                });
            }
        }
        self.set_state(ConnectionState::Disconnected, &mut events);
        events
    }

    /// Mark pending commands older than the configured timeout as unknown.
    pub fn expire_pending(&mut self, now_ms: u64, events: &mut Vec<ClientEvent>) {
        let timeout = self.config.command_timeout_ms;
        let mut updates = Vec::new();
        for (command_id, record) in &mut self.commands {
            if let CommandState::Pending { sent_at_ms } = &record.status {
                let sent_at_ms = *sent_at_ms;
                if now_ms.saturating_sub(sent_at_ms) > timeout {
                    record.status = CommandState::ResultUnknown {
                        reason: "no receipt before timeout; execution may have happened".to_owned(),
                        receipt: None,
                    };
                    updates.push((command_id.clone(), record.status.clone()));
                }
            }
        }
        for (command_id, status) in updates {
            self.changed = true;
            events.push(ClientEvent::CommandUpdated { command_id, status });
        }
    }

    /// Validated publication evidence for the background owner.
    pub fn publication_access(&self) -> PublicationAccess<'_> {
        let Some(binding) = self.read.binding.as_ref() else {
            return PublicationAccess::Withheld {
                reason: if self.read.control.is_none() {
                    ReadUnavailable::AuthorizationUnavailable
                } else if self.read.closed_sequence != 0 {
                    ReadUnavailable::Superseded
                } else {
                    ReadUnavailable::AuthorizationUnavailable
                },
            };
        };
        if !matches!(self.state, ConnectionState::Live) {
            return PublicationAccess::Withheld {
                reason: ReadUnavailable::Synchronizing,
            };
        }
        PublicationAccess::Validated {
            stamp: binding.stamp.as_ref(),
            scope: &binding.scope,
            visibility_sequence: binding.visibility_sequence,
            readable_commands: ReadableCommands {
                records: self.commands.iter(),
                binding,
            },
            read_evidence: &self.read.read_evidence,
        }
    }

    /// Read the current local status of a command under the current binding.
    ///
    /// Authorization is checked separately: a consumer must hold a
    /// [`PublicationAccess::Validated`] view and apply the per-record decision
    /// before disclosure.
    pub fn lookup_command(&self, id: &CommandId) -> CommandLookup<'_> {
        match self.commands.get(id) {
            Some(record) => CommandLookup::Found(record),
            None => CommandLookup::NotRetained,
        }
    }

    /// Issue an authorized receipt lookup within the shared read budget.
    pub fn lookup_receipt_authorized(
        &mut self,
        request: AuthorizedReceiptLookup,
    ) -> Result<(), ClientError> {
        self.read.stalled.remove(&request.command_id);
        let retained_reference = self
            .commands
            .get(&request.command_id)
            .is_some_and(|record| record.references > 0);
        let now_ms = self.now_ms;
        self.send_lookup(&request, retained_reference, now_ms)
    }

    /// Withhold all projection/command data and close the shared fence.
    fn withhold(&mut self) {
        self.release_pending_reads();
        self.read.close_binding();
        self.changed = true;
    }

    /// Close the shared fence for a local scope/authorization intent.
    fn close_local_visibility(&mut self) -> u64 {
        self.withhold();
        self.read.closed_sequence
    }

    fn release_pending_reads(&mut self) {
        let retained: Vec<CommandId> = self
            .read
            .pending
            .iter()
            .filter(|(_, pending)| pending.retained_reference)
            .map(|(id, _)| id.clone())
            .collect();
        for id in retained {
            self.release_command(&id);
        }
        self.read.pending.clear();
    }

    fn total_references(&self) -> usize {
        self.commands.values().map(|record| record.references).sum()
    }

    fn ordinary_unsettled(&self) -> usize {
        self.commands
            .values()
            .filter(|record| record.class == RetentionClass::Ordinary)
            .count()
    }

    fn tombstone_count(&self) -> usize {
        self.commands
            .values()
            .filter(|record| record.class == RetentionClass::Tombstone)
            .count()
    }

    /// Reserve a capacity class for a state before any registry mutation.
    fn take_slot(&self, state: &CommandState) -> Result<RetentionClass, ClientError> {
        if unsettled(state) {
            if self.ordinary_unsettled() < MAX_UNRESOLVED {
                return Ok(RetentionClass::Ordinary);
            }
            if tombstone_shape(state) && self.tombstone_count() < MAX_TOMBSTONES {
                return Ok(RetentionClass::Tombstone);
            }
            return Err(capacity_error());
        }
        Ok(RetentionClass::Settled(0))
    }

    /// Recompute a record's capacity class after its state changed.
    fn refresh_class(&mut self, id: &CommandId) -> Result<(), ClientError> {
        let Some(class) = self.commands.get(id).map(|record| record.class) else {
            return Ok(());
        };
        let state = match self.commands.get(id) {
            Some(record) => record.status.clone(),
            None => return Ok(()),
        };
        if !unsettled(&state) {
            // Settled(0) is the unassigned placeholder: every settled record gets
            // its own settlement sequence so eviction order is deterministic.
            let needs_sequence = match class {
                RetentionClass::Settled(sequence) => sequence == 0,
                _ => true,
            };
            if needs_sequence {
                self.settlement_sequence += 1;
                let sequence = self.settlement_sequence;
                if let Some(record) = self.commands.get_mut(id) {
                    record.class = RetentionClass::Settled(sequence);
                }
            }
        } else if matches!(class, RetentionClass::Settled(_)) {
            let slot = self.take_slot(&state)?;
            if let Some(record) = self.commands.get_mut(id) {
                record.class = slot;
            }
        }
        self.prune_settled();
        Ok(())
    }

    /// Deterministic insertion-order eviction of zero-reference settled records.
    fn prune_settled(&mut self) {
        let mut settled: Vec<(u64, CommandId)> = self
            .commands
            .iter()
            .filter_map(|(id, record)| match record.class {
                RetentionClass::Settled(sequence) => Some((sequence, id.clone())),
                _ => None,
            })
            .collect();
        settled.sort();
        let mut settled_count = settled.len();
        let mut total = self.commands.len();
        for (_, id) in settled {
            if settled_count <= MAX_RECENT && total <= MAX_DISTINCT_RETAINED {
                break;
            }
            let removable = self
                .commands
                .get(&id)
                .is_some_and(|record| record.references == 0)
                && !self.read.pending.contains_key(&id);
            if !removable {
                continue;
            }
            self.commands.remove(&id);
            settled_count -= 1;
            total -= 1;
        }
    }

    /// Release tombstone slots that are no longer needed by genuine recovery.
    fn promote_tombstones(&mut self) {
        let candidates: Vec<CommandId> = self
            .commands
            .iter()
            .filter(|(_, record)| record.class == RetentionClass::Tombstone)
            .map(|(id, _)| id.clone())
            .collect();
        for id in candidates {
            if self.ordinary_unsettled() >= MAX_UNRESOLVED {
                break;
            }
            if let Some(record) = self.commands.get_mut(&id) {
                record.class = RetentionClass::Ordinary;
            }
        }
    }

    fn record_capacity_error(&mut self, events: &mut Vec<ClientEvent>) {
        let error = capacity_error();
        self.capacity_blocked = true;
        self.changed = true;
        events.push(ClientEvent::Error(error.clone()));
        self.last_error = Some(error);
    }

    fn reconcile_capacity(&mut self) {
        let is_capacity_error = |error: &ClientError| matches!(error, ClientError::InvalidCommand(message) if message == "CommandCapacityExceeded");
        if self.capacity_blocked {
            let recovered = self.ordinary_unsettled() < MAX_UNRESOLVED;
            let superseded = self
                .last_error
                .as_ref()
                .is_some_and(|error| !is_capacity_error(error));
            if recovered || superseded {
                self.capacity_blocked = false;
                if self.last_error.as_ref().is_some_and(is_capacity_error) {
                    self.last_error = None;
                    self.changed = true;
                }
            }
        }
    }

    fn next_deadline_ms(&self) -> Option<u64> {
        let mut next = self.reconnect.next_at_ms;
        for pending in self.read.pending.values() {
            next = Some(next.map_or(pending.deadline_ms, |at| at.min(pending.deadline_ms)));
        }
        next
    }

    fn expire_read_deadlines(&mut self, now_ms: u64, events: &mut Vec<ClientEvent>) {
        let expired: Vec<CommandId> = self
            .read
            .pending
            .iter()
            .filter(|(_, pending)| now_ms >= pending.deadline_ms)
            .map(|(id, _)| id.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        for id in expired {
            if let Some(pending) = self.read.pending.remove(&id)
                && pending.retained_reference
            {
                self.release_command(&id);
            }
            self.read.record_stalled(id);
            let error = ClientError::Protocol(
                "authorized receipt read timed out; the outcome remains unknown".to_owned(),
            );
            events.push(ClientEvent::Error(error.clone()));
            self.last_error = Some(error);
        }
        self.changed = true;
    }

    fn send_lookup(
        &mut self,
        request: &AuthorizedReceiptLookup,
        retained_reference: bool,
        now_ms: u64,
    ) -> Result<(), ClientError> {
        self.require_live()?;
        let Some(binding) = self.read.binding.as_ref() else {
            return Err(ClientError::NotConnected(
                "no current read binding".to_owned(),
            ));
        };
        if !binding.matches(&request.stamp) {
            return Err(ClientError::Protocol(
                "receipt lookup stamp is not the current binding".to_owned(),
            ));
        }
        if self.read.pending.contains_key(&request.command_id) {
            return Ok(());
        }
        if self.read.pending.len() >= MAX_PENDING_READS {
            return Err(capacity_error());
        }
        if retained_reference {
            self.retain_command(&request.command_id)?;
        }
        let versions = self
            .session
            .as_ref()
            .ok_or_else(|| ClientError::NotConnected("no negotiated session".to_owned()))?
            .versions;
        let deadline_ms = now_ms.saturating_add(self.config.command_timeout_ms);
        if let Err(error) = self.send_frame(versions, ClientMessage::LookupReceipt(request.clone()))
        {
            if retained_reference {
                self.release_command(&request.command_id);
            }
            return Err(error);
        }
        self.read.pending.insert(
            request.command_id.clone(),
            PendingReceiptRead {
                request: request.clone(),
                deadline_ms,
                retained_reference,
            },
        );
        self.changed = true;
        Ok(())
    }

    /// Fair, bounded automatic drain of receipt reads that lack current evidence.
    fn drive_automatic_reads(&mut self, now_ms: u64, events: &mut Vec<ClientEvent>) {
        if !matches!(self.state, ConnectionState::Live) || self.read.binding.is_none() {
            return;
        }
        self.promote_tombstones();
        if self.read.pending.len() >= MAX_PENDING_READS {
            return;
        }
        let ids: Vec<CommandId> = self.commands.keys().cloned().collect();
        if ids.is_empty() {
            return;
        }
        let start = self
            .read
            .automatic_cursor
            .as_ref()
            .and_then(|cursor| ids.iter().position(|id| id == cursor))
            .map_or(0, |index| index + 1);
        let ordered: Vec<CommandId> = ids[start.min(ids.len())..]
            .iter()
            .chain(ids[..start.min(ids.len())].iter())
            .cloned()
            .collect();
        let mut readable_now: Vec<CommandId> = Vec::new();
        for id in ordered {
            if self.read.pending.len() >= MAX_PENDING_READS {
                break;
            }
            if self.read.pending.contains_key(&id) || self.read.stalled.contains_key(&id) {
                continue;
            }
            let Some(stamp) = self
                .read
                .binding
                .as_ref()
                .map(|binding| Arc::clone(&binding.stamp))
            else {
                break;
            };
            let decision = self.commands.get(&id).map(|record| {
                (
                    record.server_derived(),
                    record.references > 0,
                    record.readable_with(stamp.as_ref()),
                )
            });
            let Some((server_derived, referenced, readable)) = decision else {
                continue;
            };
            if !server_derived {
                continue;
            }
            if readable {
                readable_now.push(id);
                continue;
            }
            // An explicit denial under the current binding is a settled answer, not
            // missing evidence: automatic reads must not re-query it.
            let denied_now = self
                .commands
                .get(&id)
                .is_some_and(|record| match &record.decision {
                    Some((evidence, false)) => evidence.as_ref() == stamp.as_ref(),
                    _ => false,
                })
                || matches!(
                    self.read.read_evidence.get(&id),
                    Some((_, ReadEvidence::Denied(held))) if held.as_ref() == stamp.as_ref()
                );
            if denied_now {
                continue;
            }
            if referenced && self.total_references() >= MAX_REFERENCES {
                continue;
            }
            let request = AuthorizedReceiptLookup {
                lookup_id: next_receipt_read_id(),
                stamp: stamp.as_ref().clone(),
                command_id: id.clone(),
            };
            self.read.automatic_cursor = Some(id.clone());
            if let Err(error) = self.send_lookup(&request, referenced, now_ms) {
                if matches!(error, ClientError::InvalidCommand(_)) {
                    break;
                }
                events.push(ClientEvent::Error(error.clone()));
                self.last_error = Some(error);
                break;
            }
        }
        for id in readable_now {
            if let Some(record) = self.commands.get_mut(&id)
                && record.needs_read
            {
                record.needs_read = false;
            }
        }
    }

    fn require_live(&self) -> Result<(), ClientError> {
        if matches!(self.state, ConnectionState::Live) {
            Ok(())
        } else {
            Err(ClientError::NotConnected(format!(
                "commands require a live session (state: {})",
                self.state.label()
            )))
        }
    }

    fn send_frame(
        &mut self,
        versions: ProtocolVersions,
        message: ClientMessage,
    ) -> Result<(), ClientError> {
        let frame = ClientFrame {
            protocol: versions.protocol,
            schema: versions.schema,
            message,
        };
        self.send_wire(&ClientWire::Frame(frame))
    }

    fn send_wire(&mut self, wire: &ClientWire) -> Result<(), ClientError> {
        let bytes =
            serde_json::to_vec(wire).map_err(|error| ClientError::Protocol(error.to_string()))?;
        let declared = bytes.len() as u64;
        if !self.limits.frame_fits(declared) {
            return Err(ClientError::MessageTooLarge {
                declared,
                limit: self.limits.max_frame_bytes,
            });
        }
        self.transport.send(bytes).map_err(|error| match error {
            TransportError::QueueOverflow => ClientError::QueueOverflow,
            TransportError::Disconnected => {
                ClientError::Transport("transport disconnected".to_owned())
            }
            TransportError::Io(message) => ClientError::Transport(message),
        })
    }

    fn begin_session(
        &mut self,
        resume: bool,
        events: &mut Vec<ClientEvent>,
    ) -> Result<(), ClientError> {
        if self.requested_scope.is_none() {
            return Err(ClientError::NotConnected(
                "no observation scope was requested".to_owned(),
            ));
        }
        self.generation += 1;
        self.cancel_staging();
        self.withhold();
        self.transport.reset_stream();
        self.session = None;
        self.resume_cursor = None;
        self.reconnect.resume = resume && self.last_seen_cursor.is_some();
        self.projection_stale = true;
        let hello = HelloRequest {
            desktop_version: self.config.desktop_version.clone(),
            protocol: self.config.protocol.clone(),
            schema: self.config.schema.clone(),
            features: self.config.features.clone(),
        };
        self.send_wire(&ClientWire::Hello(hello))?;
        self.set_state(ConnectionState::Negotiating, events);
        Ok(())
    }

    fn send_open_observation(
        &mut self,
        resume: bool,
        events: &mut Vec<ClientEvent>,
    ) -> Result<(), ClientError> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| ClientError::NotConnected("handshake has not completed".to_owned()))?;
        let versions = session.versions;
        let scope = self.requested_scope.clone().ok_or_else(|| {
            ClientError::NotConnected("no observation scope was requested".to_owned())
        })?;
        let resume = resume && self.last_seen_cursor.is_some();
        let scope_id = self.scope.as_ref().map(|scope| scope.scope_id.clone());
        let (last_seen_cursor, epoch) = if resume {
            (self.last_seen_cursor.clone(), Some(session.epoch.clone()))
        } else {
            (None, None)
        };
        // A new attempt always uses a fresh request identity: a former-session
        // grant must never be reusable.
        let request_id = if self.read.request_sent {
            next_read_request_id()
        } else {
            self.read
                .pending_request
                .as_ref()
                .map(|request| request.request_id.clone())
                .unwrap_or_else(next_read_request_id)
        };
        self.read.request_sent = true;
        let request = OpenAuthorizedObservation {
            request_id,
            observation: OpenObservation {
                scope,
                scope_id,
                last_seen_cursor: last_seen_cursor.clone(),
                epoch,
            },
            retained_commands: self.read.pending_retained.clone(),
        };
        self.send_frame(versions, ClientMessage::OpenObservation(request.clone()))?;
        self.read.pending_request = Some(request);
        self.resume_cursor = last_seen_cursor;
        self.set_state(ConnectionState::Synchronizing, events);
        Ok(())
    }

    fn handle_frame(&mut self, bytes: &[u8], events: &mut Vec<ClientEvent>) -> bool {
        let declared = bytes.len() as u64;
        if !self.limits.frame_fits(declared) {
            self.invalidate(
                ResyncReason::MessageTooLarge,
                ClientError::MessageTooLarge {
                    declared,
                    limit: self.limits.max_frame_bytes,
                },
                events,
            );
            return false;
        }
        let wire: ServerWire = match serde_json::from_slice(bytes) {
            Ok(wire) => wire,
            Err(error) => {
                self.invalidate(
                    ResyncReason::ProtocolViolation,
                    ClientError::Protocol(format!("malformed server frame: {error}")),
                    events,
                );
                return false;
            }
        };
        match wire {
            ServerWire::HelloAccepted(accepted) => self.handle_hello_accepted(accepted, events),
            ServerWire::HelloRejected(rejected) => {
                let message = format!(
                    "server rejected negotiation: {} ({})",
                    rejected.message, rejected.reason
                );
                self.fail_session(ClientError::VersionIncompatible(message), events);
                false
            }
            ServerWire::Frame(frame) => self.handle_server_frame(frame, events),
        }
    }

    fn handle_hello_accepted(
        &mut self,
        accepted: HelloAccepted,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        if !matches!(self.state, ConnectionState::Negotiating) {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("handshake accepted outside negotiation".to_owned()),
                events,
            );
            return false;
        }
        let selected = ProtocolVersions {
            protocol: accepted.protocol,
            schema: accepted.schema,
        };
        let advertised = VersionAdvertisement {
            protocol: self.config.protocol.clone(),
            schema: self.config.schema.clone(),
        };
        if !advertised.contains(selected) {
            self.fail_session(
                ClientError::VersionIncompatible(format!(
                    "server selected {}/{} outside the advertised ranges",
                    selected.protocol, selected.schema
                )),
                events,
            );
            return false;
        }
        self.limits = self.limits.negotiate(accepted.limits);
        if matches!(self.source, ClientSource::Unknown) {
            self.source = ClientSource::Runtime {
                server_identity: accepted.server_identity.clone(),
            };
        }
        self.session = Some(Session {
            versions: selected,
            server_identity: accepted.server_identity.clone(),
            server_build: accepted.server_build,
            epoch: accepted.epoch.clone(),
            dedupe_retention_ms: accepted.dedupe_retention_ms,
        });
        events.push(ClientEvent::HandshakeCompleted {
            protocol: selected.protocol,
            schema: selected.schema,
            server_identity: accepted.server_identity,
            epoch: accepted.epoch,
        });
        let resume = self.reconnect.resume;
        match self.send_open_observation(resume, events) {
            Ok(()) => true,
            Err(error) => {
                self.handle_send_error(error, events);
                false
            }
        }
    }

    fn handle_server_frame(&mut self, frame: ServerFrame, events: &mut Vec<ClientEvent>) -> bool {
        let Some(session) = self.session.as_ref() else {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("server frame before the handshake completed".to_owned()),
                events,
            );
            return false;
        };
        if frame.protocol != session.versions.protocol || frame.schema != session.versions.schema {
            self.fail_session(
                ClientError::VersionIncompatible(format!(
                    "server frame uses {}/{} but the session fixed {}/{}",
                    frame.protocol,
                    frame.schema,
                    session.versions.protocol,
                    session.versions.schema
                )),
                events,
            );
            return false;
        }
        let stamp_ok = self.stamp_is_current(frame.read_stamp.as_ref());
        match frame.message {
            ServerMessage::ReplayAccepted(payload) => {
                if !stamp_ok {
                    return self.reject_unauthorized("replay acknowledgement", events);
                }
                self.handle_replay_accepted(payload, events)
            }
            ServerMessage::SnapshotBegin(payload) => {
                if !stamp_ok {
                    return self.reject_unauthorized("snapshot begin", events);
                }
                self.handle_snapshot_begin(payload, events)
            }
            ServerMessage::SnapshotChunk(payload) => {
                if !stamp_ok {
                    return self.reject_unauthorized("snapshot chunk", events);
                }
                self.handle_snapshot_chunk(payload, events)
            }
            ServerMessage::SnapshotEnd(payload) => {
                if !stamp_ok {
                    return self.reject_unauthorized("snapshot end", events);
                }
                self.handle_snapshot_end(payload, events)
            }
            ServerMessage::DeltaTransaction(payload) => {
                if !stamp_ok {
                    return self.reject_unauthorized("delta transaction", events);
                }
                self.handle_delta(payload, events)
            }
            ServerMessage::Receipt(payload) => {
                if !stamp_ok {
                    return self.reject_unauthorized("command receipt", events);
                }
                self.handle_authorized_receipt(payload, events)
            }
            ServerMessage::ReadGrant(payload) => {
                // Grant, invalidation and receipt controls carry their own payload
                // binding: a duplicate envelope stamp that contradicts the payload
                // is invalid, never a second source of authority.
                if let Some(outer) = frame.read_stamp.as_ref()
                    && outer != &payload.stamp
                {
                    return self.reject_unauthorized(
                        "read grant with a conflicting envelope stamp",
                        events,
                    );
                }
                self.handle_read_grant(payload, events)
            }
            ServerMessage::ReadInvalidated(payload) => {
                if frame.read_stamp.is_some() && !stamp_ok {
                    return self.reject_unauthorized(
                        "read invalidation with a foreign envelope stamp",
                        events,
                    );
                }
                self.handle_read_invalidated(payload, events);
                true
            }
            ServerMessage::ReceiptReadResult(payload) => {
                if let Some(outer) = frame.read_stamp.as_ref()
                    && outer != &payload.stamp
                {
                    return self.reject_unauthorized(
                        "receipt read result with a conflicting envelope stamp",
                        events,
                    );
                }
                // The payload stamp is the binding for this control; its currency is
                // validated by the handler, and an absent envelope stamp is valid.
                self.handle_receipt_result(payload, events)
            }
            ServerMessage::ResyncRequired(payload) => self.handle_resync_required(payload, events),
            ServerMessage::Error(payload) => {
                // A state-bearing error settles a command: it needs the current
                // binding, never an implied permission.
                if payload.command_id.is_some() && !stamp_ok {
                    return self.reject_unauthorized("state-bearing error", events);
                }
                self.handle_protocol_error(payload, events)
            }
            ServerMessage::Unknown(payload) => self.handle_unknown(payload, events),
        }
    }

    fn handle_replay_accepted(
        &mut self,
        payload: ReplayAccepted,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        let requested = self.resume_cursor.as_ref();
        if !matches!(self.state, ConnectionState::Synchronizing)
            || requested != Some(&payload.cursor)
        {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(
                    "replay acknowledgement does not match the requested cursor".to_owned(),
                ),
                events,
            );
            return false;
        }
        self.resume_cursor = None;
        self.projection_stale = false;
        self.reconnect = ReconnectPlan::default();
        self.consecutive_overflows = 0;
        self.set_state(ConnectionState::Live, events);
        true
    }

    fn handle_resync_required(
        &mut self,
        payload: ResyncRequired,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        events.push(ClientEvent::ResyncRequested {
            reason: payload.reason.clone(),
        });
        self.reconnect.attempt += 1;
        self.reconnect.resume = false;
        self.resume_cursor = None;
        self.cancel_staging();
        if self.reconnect.attempt > self.config.max_resync_attempts {
            self.set_state(
                ConnectionState::Incompatible {
                    reason: format!("server keeps requiring resync ({})", payload.reason),
                },
                events,
            );
            return false;
        }
        match self.send_open_observation(false, events) {
            Ok(()) => true,
            Err(error) => {
                self.handle_send_error(error, events);
                false
            }
        }
    }

    fn handle_snapshot_begin(
        &mut self,
        payload: SnapshotBegin,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        if !matches!(self.state, ConnectionState::Synchronizing) {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("snapshot begin outside synchronization".to_owned()),
                events,
            );
            return false;
        }
        if let Some(staging) = self.staging.as_ref()
            && staging.snapshot_id == payload.snapshot_id
        {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("duplicate snapshot begin".to_owned()),
                events,
            );
            return false;
        }
        self.staging = Some(Staging {
            snapshot_id: payload.snapshot_id,
            generation: self.generation,
            mutations: Vec::new(),
            bundle_bytes: 0,
            next_part: 0,
        });
        true
    }

    fn handle_snapshot_chunk(
        &mut self,
        payload: SnapshotChunk,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        if self.cancelled_snapshots.contains(&payload.snapshot_id) {
            return true;
        }
        let violation = match &self.staging {
            None => Some("snapshot chunk without a begin"),
            Some(staging)
                if staging.snapshot_id != payload.snapshot_id
                    || staging.generation != self.generation =>
            {
                Some("snapshot chunk does not match the active bundle")
            }
            Some(staging) if payload.part_index != staging.next_part => {
                Some("out-of-order snapshot part")
            }
            Some(_) => None,
        };
        if let Some(message) = violation {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(message.to_owned()),
                events,
            );
            return false;
        }
        let part_bytes = match canonical_json(&payload.mutations) {
            Ok(bytes) => bytes.len() as u64,
            Err(error) => {
                self.invalidate(
                    ResyncReason::ProtocolViolation,
                    ClientError::Protocol(error.to_string()),
                    events,
                );
                return false;
            }
        };
        let accumulated = self
            .staging
            .as_ref()
            .map(|staging| staging.bundle_bytes)
            .unwrap_or(0)
            .saturating_add(part_bytes);
        if !self.limits.bundle_fits(accumulated) {
            self.invalidate(
                ResyncReason::MessageTooLarge,
                ClientError::MessageTooLarge {
                    declared: accumulated,
                    limit: self.limits.max_bundle_bytes,
                },
                events,
            );
            return false;
        }
        if let Some(staging) = self.staging.as_mut() {
            staging.bundle_bytes = accumulated;
            staging.next_part += 1;
            staging.mutations.extend(payload.mutations);
        }
        true
    }

    fn handle_snapshot_end(&mut self, payload: SnapshotEnd, events: &mut Vec<ClientEvent>) -> bool {
        if self.cancelled_snapshots.contains(&payload.snapshot_id) {
            return true;
        }
        let Some(staging) = self.staging.take() else {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("snapshot end without a begin".to_owned()),
                events,
            );
            return false;
        };
        if staging.snapshot_id != payload.snapshot_id || staging.generation != self.generation {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("snapshot end does not match the active bundle".to_owned()),
                events,
            );
            return false;
        }
        let epoch_matches = self
            .session
            .as_ref()
            .is_some_and(|session| session.epoch == payload.epoch);
        if !epoch_matches {
            self.invalidate(
                ResyncReason::EpochChanged,
                ClientError::Protocol("snapshot epoch differs from the session epoch".to_owned()),
                events,
            );
            return false;
        }
        if !self.scope_matches(&payload.scope) {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(
                    "snapshot scope does not match the requested scope".to_owned(),
                ),
                events,
            );
            return false;
        }
        let counts = SnapshotCounts::of(&staging.mutations);
        if counts != payload.expected {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("snapshot entity counts do not match".to_owned()),
                events,
            );
            return false;
        }
        let digest = match integrity_digest_of(&staging.mutations) {
            Ok(digest) => digest,
            Err(error) => {
                self.invalidate(
                    ResyncReason::ProtocolViolation,
                    ClientError::Protocol(error.to_string()),
                    events,
                );
                return false;
            }
        };
        if digest != payload.integrity_digest {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("snapshot integrity digest does not match".to_owned()),
                events,
            );
            return false;
        }
        match ProjectionState::from_snapshot(&payload.epoch, &staging.mutations) {
            Ok(state) => {
                self.projection = Arc::new(state);
                self.last_seen_cursor = Some(payload.cursor.clone());
                self.scope = Some(payload.scope.clone());
                self.projection_stale = false;
                self.reconnect = ReconnectPlan::default();
                self.consecutive_overflows = 0;
                self.resume_cursor = None;
                self.applied.clear();
                events.push(ClientEvent::SnapshotApplied {
                    snapshot_id: payload.snapshot_id,
                    counts,
                    cursor: payload.cursor.clone(),
                });
                self.set_state(ConnectionState::Live, events);
                true
            }
            Err(error) => {
                self.invalidate(
                    ResyncReason::ProtocolViolation,
                    ClientError::Protocol(format!("snapshot integrity check failed: {error}")),
                    events,
                );
                false
            }
        }
    }

    fn handle_delta(
        &mut self,
        transaction: DeltaTransaction,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        if !matches!(self.state, ConnectionState::Live) {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("delta received outside a live session".to_owned()),
                events,
            );
            return false;
        }
        let epoch_matches = self
            .session
            .as_ref()
            .is_some_and(|session| session.epoch == transaction.epoch);
        if !epoch_matches {
            self.schedule_recovery(false, Some(ResyncReason::EpochChanged), events);
            return false;
        }
        if let Some(previous) = self
            .applied
            .iter()
            .find(|applied| applied.transaction_id == transaction.transaction_id)
        {
            if previous.digest == transaction.integrity_digest
                && previous.cursor == transaction.cursor
            {
                events.push(ClientEvent::DuplicateIgnored {
                    transaction_id: transaction.transaction_id,
                });
                return true;
            }
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(
                    "conflicting duplicate transaction (same identity, different content)"
                        .to_owned(),
                ),
                events,
            );
            return false;
        }
        if self
            .applied
            .iter()
            .any(|applied| applied.cursor == transaction.cursor)
        {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(
                    "a different transaction already published this cursor".to_owned(),
                ),
                events,
            );
            return false;
        }
        if transaction.predecessor.as_ref() != self.last_seen_cursor.as_ref() {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(
                    "transaction does not follow the last applied cursor (gap, stale cursor or out-of-order delivery)"
                        .to_owned(),
                ),
                events,
            );
            return false;
        }
        if transaction.mutation_count as usize != transaction.mutations.len() {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(
                    "declared mutation count does not match the transaction".to_owned(),
                ),
                events,
            );
            return false;
        }
        let digest = match integrity_digest_of(&transaction.mutations) {
            Ok(digest) => digest,
            Err(error) => {
                self.invalidate(
                    ResyncReason::ProtocolViolation,
                    ClientError::Protocol(error.to_string()),
                    events,
                );
                return false;
            }
        };
        if digest != transaction.integrity_digest {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol("transaction integrity digest does not match".to_owned()),
                events,
            );
            return false;
        }
        let mut keys = BTreeSet::new();
        for mutation in &transaction.mutations {
            let key = mutation_key(mutation);
            if !keys.insert(key) {
                self.invalidate(
                    ResyncReason::ProtocolViolation,
                    ClientError::Protocol(
                        "a transaction mutates the same projection twice".to_owned(),
                    ),
                    events,
                );
                return false;
            }
        }
        let mut upserts: Vec<Projection> = Vec::new();
        let mut removals: Vec<RemoveTarget> = Vec::new();
        for mutation in &transaction.mutations {
            if let Err(error) = self.projection.validate_mutation(mutation) {
                self.invalidate(
                    ResyncReason::ProtocolViolation,
                    ClientError::Protocol(format!("mutation rejected: {error}")),
                    events,
                );
                return false;
            }
            match mutation {
                ProjectionMutation::Upsert { projection, .. } => upserts.push(projection.clone()),
                ProjectionMutation::Remove { target, .. } => removals.push(target.clone()),
            }
        }
        if let Err(error) = self
            .projection
            .validate_effective_state(&upserts, &removals)
        {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(format!("effective state rejected: {error}")),
                events,
            );
            return false;
        }
        {
            let state = Arc::make_mut(&mut self.projection);
            state.apply(&transaction.mutations);
        }
        // An applied delta changes the projection a publication exposes, and the
        // owner publishes only when the report says the client changed; a
        // re-delivered identical transaction is a duplicate and returns above
        // without marking one.
        self.changed = true;
        self.last_seen_cursor = Some(transaction.cursor.clone());
        self.applied.push_back(AppliedTransaction {
            transaction_id: transaction.transaction_id.clone(),
            cursor: transaction.cursor.clone(),
            digest,
        });
        while self.applied.len() > self.config.duplicate_window {
            self.applied.pop_front();
        }
        events.push(ClientEvent::DeltaApplied {
            transaction_id: transaction.transaction_id,
            cursor: transaction.cursor,
        });
        true
    }

    /// Whether a frame's stamp is the exact current binding.
    fn stamp_is_current(&self, stamp: Option<&ReadStamp>) -> bool {
        match (stamp, self.read.binding.as_ref()) {
            (Some(stamp), Some(binding)) => binding.matches(stamp),
            _ => false,
        }
    }

    /// Fail closed: withhold everything, then start bounded recovery.
    fn reject_unauthorized(&mut self, what: &str, events: &mut Vec<ClientEvent>) -> bool {
        self.withhold();
        self.invalidate(
            ResyncReason::ProtocolViolation,
            ClientError::Protocol(format!(
                "{what} arrived without the current authorization binding"
            )),
            events,
        );
        false
    }

    fn handle_read_grant(&mut self, grant: ReadGrant, events: &mut Vec<ClientEvent>) -> bool {
        if !matches!(self.state, ConnectionState::Synchronizing) {
            return self.reject_unauthorized("read grant outside synchronization", events);
        }
        let Some(session_epoch) = self.session.as_ref().map(|session| session.epoch.clone()) else {
            return self.reject_unauthorized("read grant before the handshake completed", events);
        };
        let Some(verified) = self.transport.authenticated_server() else {
            return self.reject_unauthorized("read grant on an unverified channel", events);
        };
        let Some(pending) = self.read.pending_request.clone() else {
            return self.reject_unauthorized("read grant without a pending request", events);
        };
        if grant.stamp.request_id != pending.request_id
            || grant.stamp.server_id != verified
            || grant.stamp.epoch != session_epoch
        {
            return self.reject_unauthorized("read grant binding mismatch", events);
        }
        if self
            .read
            .is_late_grant(&grant.stamp.context_id, &grant.stamp.read_revision)
        {
            return self.reject_unauthorized("late or equal read grant rejected", events);
        }
        let mut decided: BTreeSet<&CommandId> = BTreeSet::new();
        for decision in &grant.command_reads {
            if !decided.insert(&decision.command_id) {
                return self.reject_unauthorized("duplicate read decision", events);
            }
        }
        let requested: BTreeSet<&CommandId> = self.read.pending_retained.iter().collect();
        if decided != requested {
            return self.reject_unauthorized(
                "read decisions do not match the requested retained identities",
                events,
            );
        }
        if !self.scope_matches(&grant.scope) {
            return self.reject_unauthorized("granted scope does not match the request", events);
        }
        if grant.stamp.scope_id != grant.scope.scope_id {
            return self
                .reject_unauthorized("granted stamp does not describe the granted scope", events);
        }
        let sequence = self.read.current_sequence();
        let context = Arc::new(grant.stamp.context_id.clone());
        let stamp = Arc::new(grant.stamp.clone());
        for decision in &grant.command_reads {
            if let Some(record) = self.commands.get_mut(&decision.command_id) {
                record.decision = Some((Arc::clone(&stamp), decision.readable));
                record.needs_read = !decision.readable && record.server_derived();
            }
        }
        self.read.binding = Some(ReadBinding {
            stamp,
            context,
            scope: grant.scope,
            local_intents_readable: grant.local_intents_readable,
            visibility_sequence: sequence,
        });
        self.changed = true;
        true
    }

    fn handle_read_invalidated(&mut self, payload: ReadInvalidated, events: &mut Vec<ClientEvent>) {
        let reason = payload.reason;
        // Capture the binding identity before closing: only a notification that
        // matches the binding this session actually holds may move the floor or
        // drive recovery.
        let bound = self
            .read
            .binding
            .as_ref()
            .map(|binding| (binding.context.clone(), binding.stamp.request_id.clone()));
        // Detected invalidation always closes the shared fence first.
        self.withhold();
        let trusted = match &bound {
            Some((context, request_id)) => {
                context.as_ref() == &payload.context_id && request_id == &payload.request_id
            }
            None => false,
        };
        if !trusted {
            // A notification that does not match the binding this session held is
            // not usable evidence: stay closed, disclose nothing, and neither move
            // the floor nor start recovery from unverified data.
            events.push(ClientEvent::Error(ClientError::Protocol(
                "read invalidation does not match the current binding".to_owned(),
            )));
            self.changed = true;
            return;
        }
        // The floor only ever advances: a trusted notification moves it forward and
        // an older revision cannot regress it.
        self.read
            .raise_floor(payload.context_id.clone(), payload.read_revision.clone());
        let resync = match reason {
            ReadInvalidationReason::EpochChanged => ResyncReason::EpochChanged,
            _ => ResyncReason::Internal,
        };
        self.schedule_recovery(false, Some(resync), events);
    }

    fn handle_authorized_receipt(
        &mut self,
        authorized: AuthorizedCommandReceipt,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        if !authorized.permission.readable
            || authorized.permission.command_id != authorized.receipt.command_id
        {
            return self.reject_unauthorized(
                "command receipt without current positive permission",
                events,
            );
        }
        // The outer frame stamp alone is not authorization evidence: the receipt's
        // own stamp must be the current binding, or its state would change under a
        // stale authorization context.
        if !self.stamp_is_current(Some(&authorized.stamp)) {
            return self
                .reject_unauthorized("command receipt with a stale authorization stamp", events);
        }
        let evidence = Arc::new(authorized.stamp.clone());
        match self.apply_receipt(&authorized.receipt, Some(evidence), events) {
            Ok(()) => true,
            Err(_) => {
                self.record_capacity_error(events);
                true
            }
        }
    }

    /// Apply one receipt under pre-validated authorization evidence.
    fn apply_receipt(
        &mut self,
        receipt: &CommandReceipt,
        evidence: Option<Arc<ReadStamp>>,
        events: &mut Vec<ClientEvent>,
    ) -> Result<(), ClientError> {
        let tracked = self.commands.contains_key(&receipt.command_id);
        let target = match &receipt.state {
            ReceiptState::Accepted => CommandState::Accepted(receipt.clone()),
            ReceiptState::Rejected => CommandState::Rejected(receipt.clone()),
            ReceiptState::CanonicalReflected => CommandState::CanonicalReflected(receipt.clone()),
            ReceiptState::Unknown(raw) if tracked => CommandState::ResultUnknown {
                reason: format!("unrecognized receipt state {raw}"),
                receipt: Some(receipt.clone()),
            },
            ReceiptState::Unknown(_) => CommandState::ResultUnknown {
                reason: TOMBSTONE_REASON.to_owned(),
                receipt: Some(receipt.clone()),
            },
        };
        let class = match self
            .commands
            .get(&receipt.command_id)
            .map(|record| record.class)
        {
            Some(RetentionClass::Ordinary) if unsettled(&target) => RetentionClass::Ordinary,
            Some(RetentionClass::Tombstone) if unsettled(&target) => RetentionClass::Tombstone,
            _ => match self.take_slot(&target) {
                Ok(class) => class,
                Err(error) => {
                    // Ordinary capacity is full: an actual new untracked
                    // unsettled receipt takes a tombstone slot with its actual
                    // ResultUnknown state instead of failing, exactly as the
                    // contract's tombstone class describes. Both capacities full
                    // keeps the existing capacity error.
                    if tracked || !unsettled(&target) || self.tombstone_count() >= MAX_TOMBSTONES {
                        return Err(error);
                    }
                    RetentionClass::Tombstone
                }
            },
        };
        let stored = match class {
            RetentionClass::Tombstone => CommandState::ResultUnknown {
                reason: TOMBSTONE_REASON.to_owned(),
                receipt: Some(receipt.clone()),
            },
            _ => target.clone(),
        };
        match self.commands.get_mut(&receipt.command_id) {
            Some(record) => {
                record.status = stored.clone();
                record.class = class;
                record.needs_read = false;
                if let Some(evidence) = evidence {
                    record.decision = Some((evidence, true));
                }
            }
            None => {
                self.commands.insert(
                    receipt.command_id.clone(),
                    CommandStatus {
                        envelope: None,
                        status: stored.clone(),
                        origin: None,
                        decision: evidence.map(|evidence| (evidence, true)),
                        needs_read: false,
                        class,
                        references: 0,
                    },
                );
            }
        }
        self.read.read_evidence.remove(&receipt.command_id);
        self.refresh_class(&receipt.command_id)?;
        self.changed = true;
        events.push(ClientEvent::CommandUpdated {
            command_id: receipt.command_id.clone(),
            status: stored,
        });
        Ok(())
    }

    fn handle_receipt_result(
        &mut self,
        payload: ReceiptReadResult,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        let Some(binding) = self.read.binding.as_ref() else {
            // A reply after a close cannot disclose anything; the read is cancelled.
            return true;
        };
        if !binding.matches(&payload.stamp) {
            return self.reject_unauthorized("receipt read result with a stale stamp", events);
        }
        let Some(pending) = self.read.pending.get(&payload.command_id).cloned() else {
            return self.reject_unauthorized("uncorrelated receipt read result", events);
        };
        if pending.request.lookup_id != payload.lookup_id || pending.request.stamp != payload.stamp
        {
            return self
                .reject_unauthorized("receipt read result does not match its lookup", events);
        }
        if let Some(pending) = self.read.pending.remove(&payload.command_id)
            && pending.retained_reference
        {
            self.release_command(&payload.command_id);
        }
        self.read.stalled.remove(&payload.command_id);
        let evidence = Arc::new(payload.stamp.clone());
        match payload.outcome {
            ReceiptReadOutcome::Denied => {
                if let Some(record) = self.commands.get_mut(&payload.command_id) {
                    record.decision = Some((Arc::clone(&evidence), false));
                    record.needs_read = false;
                } else {
                    // The same public denial applies to an absent identity, and it
                    // discloses no retention or execution fact.
                    self.read.record_evidence(
                        payload.command_id.clone(),
                        ReadEvidence::Denied(evidence),
                    );
                }
                self.read.record_stalled(payload.command_id.clone());
                self.changed = true;
                true
            }
            ReceiptReadOutcome::Allowed {
                receipt: Some(receipt),
            } => {
                if receipt.command_id != payload.command_id {
                    return self.reject_unauthorized(
                        "receipt read result carries another command",
                        events,
                    );
                }
                match self.apply_receipt(&receipt, Some(evidence), events) {
                    Ok(()) => true,
                    Err(_) => {
                        self.record_capacity_error(events);
                        true
                    }
                }
            }
            ReceiptReadOutcome::Allowed { receipt: None } => {
                if self.commands.contains_key(&payload.command_id) {
                    if let Some(record) = self.commands.get_mut(&payload.command_id) {
                        record.decision = Some((Arc::clone(&evidence), true));
                        record.needs_read = false;
                    }
                } else {
                    self.read.record_evidence(
                        payload.command_id.clone(),
                        ReadEvidence::AllowedNoReceipt(evidence),
                    );
                }
                self.changed = true;
                true
            }
        }
    }

    fn handle_protocol_error(
        &mut self,
        error: ProtocolError,
        events: &mut Vec<ClientEvent>,
    ) -> bool {
        // Session-wide authorization failure invalidates read access and must not
        // settle a correlated command.
        if !matches!(error.code, ErrorCode::Unauthorized | ErrorCode::Forbidden)
            && let Some(command_id) = error.command_id.clone()
            && let Some(record) = self.commands.get_mut(&command_id)
        {
            record.status = match error.code {
                ErrorCode::UnsupportedCommand => CommandState::Unsupported {
                    error: error.clone(),
                },
                ErrorCode::ReceiptExpired => CommandState::ResultUnknown {
                    reason: "receipt expired or unknown; execution may have happened".to_owned(),
                    receipt: record.status.receipt().cloned(),
                },
                _ => CommandState::Failed {
                    error: error.clone(),
                },
            };
            events.push(ClientEvent::CommandUpdated {
                command_id,
                status: record.status.clone(),
            });
        }
        if let Some(command_id) = error.command_id.clone()
            && self.commands.contains_key(&command_id)
        {
            let _ = self.refresh_class(&command_id);
        }
        events.push(ClientEvent::Error(ClientError::server(error.clone())));
        self.last_error = Some(ClientError::server(error.clone()));
        match error.code {
            ErrorCode::VersionIncompatible => {
                self.fail_session(
                    ClientError::VersionIncompatible(error.message.clone()),
                    events,
                );
                false
            }
            ErrorCode::Unauthorized | ErrorCode::Forbidden => {
                self.withhold();
                self.cancel_staging();
                self.transport.reset_stream();
                self.projection_stale = true;
                self.reconnect = ReconnectPlan::default();
                self.set_state(
                    ConnectionState::Unauthorized {
                        reason: error.message.clone(),
                    },
                    events,
                );
                false
            }
            ErrorCode::ResyncRequired => {
                let reason = error
                    .resync_reason
                    .clone()
                    .unwrap_or(ResyncReason::Internal);
                self.schedule_recovery(false, Some(reason), events);
                false
            }
            ErrorCode::QueueOverflow => {
                self.handle_overflow(events);
                false
            }
            ErrorCode::MessageTooLarge => {
                self.schedule_recovery(false, Some(ResyncReason::MessageTooLarge), events);
                false
            }
            _ => true,
        }
    }

    fn handle_unknown(&mut self, unknown: UnknownMessage, events: &mut Vec<ClientEvent>) -> bool {
        if unknown.affects_state {
            self.invalidate(
                ResyncReason::UnsupportedStateEvent,
                ClientError::Protocol(format!(
                    "unsupported state-changing event {} requires resync",
                    unknown.kind
                )),
                events,
            );
            return false;
        }
        if !matches!(self.state, ConnectionState::Live) {
            self.invalidate(
                ResyncReason::ProtocolViolation,
                ClientError::Protocol(format!(
                    "informational event {} arrived outside a live session",
                    unknown.kind
                )),
                events,
            );
            return false;
        }
        if let Some(cursor) = unknown.cursor {
            self.last_seen_cursor = Some(cursor);
        }
        events.push(ClientEvent::InformationalEventSkipped { kind: unknown.kind });
        true
    }

    fn handle_transport_error(&mut self, error: TransportError, events: &mut Vec<ClientEvent>) {
        if error == TransportError::QueueOverflow {
            self.handle_overflow(events);
            return;
        }
        let client_error = match error {
            TransportError::Disconnected => {
                ClientError::Transport("transport disconnected".to_owned())
            }
            TransportError::Io(message) => ClientError::Transport(message),
            TransportError::QueueOverflow => ClientError::QueueOverflow,
        };
        events.push(ClientEvent::Error(client_error.clone()));
        self.last_error = Some(client_error);
        let was_live = matches!(self.state, ConnectionState::Live);
        let can_resume = was_live && self.last_seen_cursor.is_some() && self.session.is_some();
        if can_resume {
            self.schedule_recovery(true, None, events);
        } else {
            self.schedule_recovery(false, Some(ResyncReason::Internal), events);
        }
    }

    fn handle_send_error(&mut self, error: ClientError, events: &mut Vec<ClientEvent>) {
        events.push(ClientEvent::Error(error.clone()));
        self.last_error = Some(error);
        self.schedule_recovery(false, Some(ResyncReason::Internal), events);
    }

    fn handle_overflow(&mut self, events: &mut Vec<ClientEvent>) {
        self.consecutive_overflows += 1;
        let error = ClientError::QueueOverflow;
        events.push(ClientEvent::Error(error.clone()));
        self.last_error = Some(error);
        if self.consecutive_overflows >= self.config.max_consecutive_overflows {
            self.cancel_staging();
            self.withhold();
            self.transport.reset_stream();
            self.projection_stale = true;
            self.set_state(
                ConnectionState::Incompatible {
                    reason: "repeated receive-queue overflow; scope or limits incompatible"
                        .to_owned(),
                },
                events,
            );
            return;
        }
        self.schedule_recovery(false, Some(ResyncReason::QueueOverflow), events);
    }

    fn invalidate(
        &mut self,
        reason: ResyncReason,
        error: ClientError,
        events: &mut Vec<ClientEvent>,
    ) {
        events.push(ClientEvent::Error(error.clone()));
        self.last_error = Some(error);
        self.schedule_recovery(false, Some(reason), events);
    }

    fn schedule_recovery(
        &mut self,
        resume: bool,
        reason: Option<ResyncReason>,
        events: &mut Vec<ClientEvent>,
    ) {
        self.cancel_staging();
        self.withhold();
        self.transport.reset_stream();
        self.projection_stale = true;
        if self.requested_scope.is_none() || self.session.is_none() {
            self.set_state(ConnectionState::Disconnected, events);
            return;
        }
        self.reconnect.attempt += 1;
        self.reconnect.resume = resume && self.last_seen_cursor.is_some();
        self.reconnect.reason = reason.clone();
        let reason_text = reason
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "transport failure".to_owned());
        if self.reconnect.attempt > self.config.max_resync_attempts {
            self.set_state(
                ConnectionState::Incompatible {
                    reason: format!("recovery attempts exhausted after {reason_text}"),
                },
                events,
            );
            return;
        }
        let delay = backoff_ms(self.reconnect.attempt, self.config.backoff_seed);
        self.reconnect.next_at_ms = Some(self.now_ms.saturating_add(delay));
        self.set_state(
            ConnectionState::Reconnecting {
                attempt: self.reconnect.attempt,
                retry_in_ms: delay,
                reason: reason_text,
            },
            events,
        );
        if let Some(reason) = reason {
            events.push(ClientEvent::ResyncRequested { reason });
        }
    }

    fn fail_session(&mut self, error: ClientError, events: &mut Vec<ClientEvent>) {
        let reason = error.to_string();
        events.push(ClientEvent::Error(error.clone()));
        self.last_error = Some(error);
        self.cancel_staging();
        self.withhold();
        self.transport.reset_stream();
        self.projection_stale = true;
        self.reconnect = ReconnectPlan::default();
        self.set_state(ConnectionState::Incompatible { reason }, events);
    }

    fn scope_matches(&self, scope: &ObservationScope) -> bool {
        let Some(requested) = self.requested_scope.as_ref() else {
            return false;
        };
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        let requested_tasks: BTreeSet<&TaskId> = requested.task_ids.iter().collect();
        let scope_tasks: BTreeSet<&TaskId> = scope.task_ids.iter().collect();
        if requested_tasks != scope_tasks {
            return false;
        }
        let requested_filters: BTreeSet<(&str, &str)> = requested
            .filters
            .iter()
            .map(|filter| (filter.name.as_str(), filter.value.as_str()))
            .collect();
        let scope_filters: BTreeSet<(&str, &str)> = scope
            .filters
            .iter()
            .map(|filter| (filter.name.as_str(), filter.value.as_str()))
            .collect();
        if requested_filters != scope_filters {
            return false;
        }
        scope.protocol == session.versions.protocol && scope.schema == session.versions.schema
    }

    fn cancel_staging(&mut self) {
        if let Some(staging) = self.staging.take() {
            self.cancelled_snapshots.push_back(staging.snapshot_id);
            while self.cancelled_snapshots.len() > self.config.cancelled_snapshot_memory {
                self.cancelled_snapshots.pop_front();
            }
        }
    }

    fn set_state(&mut self, state: ConnectionState, events: &mut Vec<ClientEvent>) {
        if self.state == state {
            return;
        }
        self.state = state.clone();
        self.changed = true;
        events.push(ClientEvent::StateChanged(state));
    }
}

fn mutation_key(mutation: &ProjectionMutation) -> ProjectionKey {
    match mutation {
        ProjectionMutation::Upsert { projection, .. } => ProjectionKey::of_projection(projection),
        ProjectionMutation::Remove { target, .. } => ProjectionKey::of_remove_target(target),
    }
}

/// Deterministic exponential backoff with bounded jitter.
///
/// Base 500 ms doubling up to a 30 s cap, plus a deterministic 0..20% jitter
/// derived from the attempt and seed (no wall-clock sleeps, no RNG dependency).
pub fn backoff_ms(attempt: u32, seed: u64) -> u64 {
    const BASE_MS: u64 = 500;
    const CAP_MS: u64 = 30_000;
    let exponent = attempt.saturating_sub(1).min(6);
    let base = BASE_MS.saturating_mul(1u64 << exponent).min(CAP_MS);
    let jitter_span = (base / 5).max(1);
    let mut mixed = seed ^ u64::from(attempt).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    mixed ^= mixed >> 33;
    mixed = mixed.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    mixed ^= mixed >> 33;
    base + (mixed % jitter_span)
}

impl<T: TransportTrait + Send> DesktopClient for CosClient<T> {
    fn snapshot(&self) -> SnapshotHandle {
        CosClient::snapshot(self)
    }

    fn connection(&self) -> &ConnectionState {
        CosClient::connection(self)
    }

    fn is_stale(&self) -> bool {
        CosClient::is_stale(self)
    }

    fn source(&self) -> ClientSource {
        CosClient::source(self)
    }

    fn last_error(&self) -> Option<&ClientError> {
        CosClient::last_error(self)
    }

    fn pump(&mut self, now_ms: u64) -> Vec<ClientEvent> {
        CosClient::pump(self, now_ms)
    }

    fn disconnect(&mut self) -> Vec<ClientEvent> {
        CosClient::disconnect(self)
    }

    fn request_resync(&mut self, reason: ResyncReason) -> Vec<ClientEvent> {
        CosClient::request_resync(self, reason)
    }

    fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, ClientError> {
        CosClient::stop_task(self, task_id)
    }

    fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport {
        CosClient::pump_step(self, now_ms, budget)
    }

    fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus> {
        CosClient::command_statuses(self)
    }

    fn lookup_command(&self, id: &CommandId) -> CommandLookup<'_> {
        CosClient::lookup_command(self, id)
    }

    fn retain_command(&mut self, id: &CommandId) -> Result<(), ClientError> {
        CosClient::retain_command(self, id)
    }

    fn release_command(&mut self, id: &CommandId) {
        CosClient::release_command(self, id);
    }

    fn begin_authorized_observation(
        &mut self,
        request: OpenAuthorizedObservation,
    ) -> Result<(), ClientError> {
        CosClient::begin_authorized_observation(self, request)
    }

    fn publication_access(&self) -> PublicationAccess<'_> {
        CosClient::publication_access(self)
    }

    fn lookup_receipt_authorized(
        &mut self,
        request: AuthorizedReceiptLookup,
    ) -> Result<(), ClientError> {
        CosClient::lookup_receipt_authorized(self, request)
    }

    fn bind_observation_control(&mut self, control: ObservationControl) {
        CosClient::bind_observation_control(self, control);
    }

    fn observation_request(&self, scope: ObservationScopeRequest) -> OpenAuthorizedObservation {
        CosClient::observation_request(self, scope)
    }

    fn adopt_retained_registry(
        &mut self,
        records: BTreeMap<CommandId, CommandStatus>,
        references: BTreeMap<CommandId, usize>,
    ) -> Result<(), ClientError> {
        CosClient::adopt_retained_registry(self, records, references)
    }
}
