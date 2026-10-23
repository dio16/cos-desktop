//! Scripted fixture provider behind the same client boundary.
//!
//! `MockCosClient` is an explicit fixture: it serves scripted snapshots, delta
//! transactions, receipts and errors through the same transport interface a
//! real adapter would implement. It never falls back to fixture mode
//! automatically, never reimplements Domain transitions and never fabricates
//! completion. Every identity is `fixture:`-namespaced (validated at load).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use desktop_protocol::{
    AuthorizedCommandReceipt, AuthorizedReceiptLookup, CommandEnvelope, CommandId,
    CommandOperation, CommandPayload, CommandReadDecision, CommandReceipt, CommandTarget, EpochId,
    ErrorCode, HelloAccepted, HelloRejected, HelloRequest, Limits, ObservationScope,
    ObservationScopeRequest, OpenAuthorizedObservation, ProtocolError, ProtocolVersions, ReadGrant,
    ReadInvalidated, ReadInvalidationReason, ReadRequestId, ReadRevision, ReadStamp,
    ReceiptReadOutcome, ReceiptReadResult, ReplayAccepted, ResyncReason, ResyncRequired,
    ServerFrame, ServerId, ServerMessage, ServerWire, SnapshotBegin, SnapshotChunk, SnapshotEnd,
    TaskId, Version, VersionAdvertisement, integrity_digest_of, negotiate_session,
};
use serde_json::Value;

use crate::client::{
    ClientConfig, ClientEvent, ClientSource, ConnectionState, CosClient, DesktopClient, PumpReport,
    SnapshotHandle,
};
use crate::error::ClientError;
use crate::fixture::{
    CommandDefault, FixtureError, FixtureReadAuthorization, FixtureReceipt, FixtureReceiptOutcome,
    FixtureRuntime, FixtureScenario, FixtureStep, ObservationMode, Trigger, parse_trigger,
};
use crate::read::{CommandLookup, CommandStatus, PublicationAccess};
use crate::transport::{Transport, TransportError};

mod deadline_trace;
use deadline_trace::FixtureTrace;

/// One steered fixture action, applied on the provider's next poll.
enum MockCommand {
    /// Inject a raw JSON frame (stamped from the active binding when omitted).
    Inject(String),
    /// Inject a raw JSON frame verbatim, preserving a missing or null stamp.
    InjectVerbatim(String),
    /// Inject a typed server message, optionally stamped from the active binding.
    Message {
        message: Box<ServerMessage>,
        stamped: bool,
    },
    /// Inject a typed server message with an explicit (possibly stale) stamp.
    StampedMessage {
        message: Box<ServerMessage>,
        stamp: Box<ReadStamp>,
    },
    /// Make the next poll report a transport disconnect.
    Disconnect,
    /// Simulate a runtime restart with a new epoch.
    SetEpoch(EpochId),
    /// Deliver one scripted manual step.
    DeliverStep(String),
    /// Deny a command identity for later authorized receipt reads.
    DenyCommand(String),
    /// Serve later grants with a stamp that names another scope identity.
    GrantStampScope(String),
    /// Arm a blueprint violation the provider injects into every later grant.
    GrantFlaw(GrantFlaw),
    /// Stop injecting a blueprint violation into later grants.
    ClearGrantFlaw,
    /// Answer the outstanding authorized lookup with an explicit receipt.
    AnswerLookup(Option<Box<CommandReceipt>>),
    /// Answer the outstanding authorized lookup with a scoped denial.
    DenyLookup,
    /// Serve later grants with a different observation scope.
    ServeScope(Box<ObservationScopeRequest>),
}

/// A deliberate blueprint violation the fixture provider injects into a grant
/// it issues while the client is still synchronizing.
///
/// Each variant is an otherwise well-formed grant whose sole contradiction the
/// client must refuse without opening a binding. It exists so a transcript can
/// drive the refusal paths directly, instead of relying on the provider's clean
/// grant being malformed by accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantFlaw {
    /// Omit one requested identity's read decision.
    MissingDecision,
    /// Answer an identity the client never requested.
    ExtraDecision,
    /// Answer one requested identity more than once.
    DuplicateDecision,
    /// Stamp the grant with a server identity other than the authenticated one.
    ForeignServer,
}

#[derive(Default)]
struct MockShared {
    commands: VecDeque<MockCommand>,
    active_stamp: Option<ReadStamp>,
    sent_frames: usize,
    lookups: usize,
    /// Command dispatches the provider actually received, as (command id, payload kind).
    command_sends: Vec<(String, String)>,
}

/// Shared control channel for a scripted fixture transcript.
///
/// It only enqueues steered actions and reports counters; the provider applies
/// them on its next poll, so a window or owner never waits on fixture I/O.
#[derive(Clone, Default)]
pub struct MockControl {
    state: Arc<Mutex<MockShared>>,
}

impl MockControl {
    /// Inject a raw JSON frame, stamped from the active binding when omitted.
    pub fn inject_raw(&self, raw: impl Into<String>) {
        self.push(MockCommand::Inject(raw.into()));
    }

    /// Inject a typed server message, stamped from the active binding.
    pub fn inject_message(&self, message: ServerMessage) {
        self.push(MockCommand::Message {
            message: Box::new(message),
            stamped: true,
        });
    }

    /// Inject a typed server message without any frame stamp.
    pub fn inject_unstamped_message(&self, message: ServerMessage) {
        self.push(MockCommand::Message {
            message: Box::new(message),
            stamped: false,
        });
    }

    /// Inject a typed state-bearing message carrying an explicit stamp.
    pub fn inject_message_with_stamp(&self, message: ServerMessage, stamp: ReadStamp) {
        self.push(MockCommand::StampedMessage {
            message: Box::new(message),
            stamp: Box::new(stamp),
        });
    }

    /// Inject a raw JSON frame verbatim, preserving a missing or null stamp.
    pub fn inject_raw_unstamped(&self, raw: impl Into<String>) {
        self.push(MockCommand::InjectVerbatim(raw.into()));
    }

    /// Report a transport disconnect on the next poll.
    pub fn disconnect(&self) {
        self.push(MockCommand::Disconnect);
    }

    /// Simulate a runtime restart with a new epoch.
    pub fn set_epoch(&self, epoch: EpochId) {
        self.push(MockCommand::SetEpoch(epoch));
    }

    /// Deny an identity for later authorized receipt reads.
    pub fn deny_command(&self, id: impl Into<String>) {
        self.push(MockCommand::DenyCommand(id.into()));
    }

    /// Serve later grants with a stamp that names another scope identity, so the
    /// client must refuse the contradiction before any binding opens.
    pub fn grant_stamp_scope(&self, scope_id: impl Into<String>) {
        self.push(MockCommand::GrantStampScope(scope_id.into()));
    }

    /// Arm a blueprint violation injected into every later issued grant, so the
    /// client is delivered a grant that is well-formed except for one
    /// contradiction and must stay withheld.
    pub fn grant_flaw(&self, flaw: GrantFlaw) {
        self.push(MockCommand::GrantFlaw(flaw));
    }

    /// Stop injecting a blueprint violation into later grants.
    pub fn clear_grant_flaw(&self) {
        self.push(MockCommand::ClearGrantFlaw);
    }

    /// Answer the outstanding authorized lookup, for transcripts that keep lookups
    /// pending until the test decides the outcome.
    pub fn answer_pending_lookup(&self, receipt: Option<CommandReceipt>) {
        self.push(MockCommand::AnswerLookup(receipt.map(Box::new)));
    }

    /// Answer the outstanding authorized lookup with a scoped denial.
    pub fn answer_pending_lookup_denied(&self) {
        self.push(MockCommand::DenyLookup);
    }

    /// Deliver one scripted manual step.
    pub fn deliver_step(&self, id: impl Into<String>) {
        self.push(MockCommand::DeliverStep(id.into()));
    }

    /// Serve later grant frames with a different observation scope.
    ///
    /// A transcript otherwise answers every open request with the scope it was
    /// built for. This test-only seam moves the provider onto the exact scope a
    /// caller names, so a client that requested a new scope can be answered with
    /// a grant that matches it (or, while unset, must stay refused).
    pub fn serve_scope(&self, scope: ObservationScopeRequest) {
        self.push(MockCommand::ServeScope(Box::new(scope)));
    }

    /// Binding the provider currently serves data under.
    pub fn active_stamp(&self) -> Option<ReadStamp> {
        self.state
            .lock()
            .expect("fixture control lock")
            .active_stamp
            .clone()
    }

    /// Number of client frames the provider has consumed.
    pub fn sent_frames(&self) -> usize {
        self.state.lock().expect("fixture control lock").sent_frames
    }

    /// Number of authorized receipt lookups the provider has received.
    pub fn lookups_seen(&self) -> usize {
        self.state.lock().expect("fixture control lock").lookups
    }

    /// Command dispatches the provider actually received, in delivery order.
    ///
    /// Fixture-side observation for per-identity re-send assertions: unlike the
    /// total frame count, this excludes reauthorization and lookup frames, so a
    /// duplicate `StopTask` dispatch is directly visible.
    pub fn sent_commands(&self) -> Vec<(String, String)> {
        self.state
            .lock()
            .expect("fixture control lock")
            .command_sends
            .clone()
    }

    fn push(&self, command: MockCommand) {
        self.state
            .lock()
            .expect("fixture control lock")
            .commands
            .push_back(command);
    }
}

/// Scripted byte transport and synthetic server.
///
/// The bounded receive queue is enforced here (it simulates the client-side
/// socket buffer): pushing beyond the negotiated frame/byte limits flags an
/// overflow that the client observes as a cancelled stream.
pub struct MockTransport {
    scenario: FixtureScenario,
    runtime: FixtureRuntime,
    limits: Limits,
    inbox: VecDeque<QueuedFixtureFrame>,
    inbox_bytes: u64,
    overflowed: bool,
    disconnect_pending: bool,
    sent: Vec<Vec<u8>>,
    selected: Option<ProtocolVersions>,
    observation_count: u32,
    after_open_steps: VecDeque<FixtureStep>,
    after_delay_steps: Vec<(Duration, FixtureStep, bool)>,
    delayed_steps: Vec<ScheduledFixtureStep>,
    session_serial: u64,
    consumed_once: BTreeSet<String>,
    trace: FixtureTrace,
    #[cfg(test)]
    fixture_now_override: Option<Instant>,
    command_steps: BTreeMap<String, VecDeque<FixtureStep>>,
    manual_steps: BTreeMap<String, FixtureStep>,
    /// Test-only contradiction: the grant stamp names another scope identity.
    grant_stamp_scope: Option<String>,
    /// Test-only override: scope served to later grants when set.
    served_scope: Option<ObservationScopeRequest>,
    /// Test-only blueprint violation injected into every later issued grant.
    grant_flaw: Option<GrantFlaw>,
    receipts: BTreeMap<CommandId, CommandReceipt>,
    command_digests: BTreeMap<CommandId, desktop_protocol::IntegrityDigest>,
    command_responses: BTreeMap<CommandId, Vec<Vec<u8>>>,
    active_read: Option<ActiveRead>,
    read_revision: ReadRevision,
    authorization: FixtureReadAuthorization,
    pending_lookup: Option<AuthorizedReceiptLookup>,
    observation_control: Option<crate::ObservationControl>,
    shared: Arc<Mutex<MockShared>>,
}

/// Authorized binding the fixture provider currently serves data under.
#[derive(Clone)]
struct ActiveRead {
    stamp: ReadStamp,
    denied: BTreeSet<CommandId>,
    absent: BTreeSet<CommandId>,
}

struct QueuedFixtureFrame {
    bytes: Vec<u8>,
    session_serial: Option<u64>,
}

struct ScheduledFixtureStep {
    release_at: Instant,
    session_serial: u64,
    once: bool,
    step: FixtureStep,
}

#[derive(Clone, Copy)]
enum ScheduleCancelReason {
    Replaced,
    Disconnect,
    ResetStream,
    AuthorizationChanged,
    ScopeChanged,
    EpochChanged,
    Capacity,
    Closed,
}

impl ScheduleCancelReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Replaced => "replaced",
            Self::Disconnect => "disconnect",
            Self::ResetStream => "reset_stream",
            Self::AuthorizationChanged => "authorization_changed",
            Self::ScopeChanged => "scope_changed",
            Self::EpochChanged => "epoch_changed",
            Self::Capacity => "capacity",
            Self::Closed => "closed",
        }
    }
}

impl MockTransport {
    /// Build a transport for a validated scenario.
    pub fn new(scenario: FixtureScenario) -> Result<Self, FixtureError> {
        if scenario.steps.len() > 1024 {
            return Err(FixtureError::Invalid(
                "at most 1024 fixture steps are allowed".to_owned(),
            ));
        }
        let mut after_open_steps = VecDeque::new();
        let mut after_delay_steps = Vec::new();
        let mut command_steps: BTreeMap<String, VecDeque<FixtureStep>> = BTreeMap::new();
        let mut manual_steps = BTreeMap::new();
        for step in &scenario.steps {
            match parse_trigger(&step.trigger) {
                Some(Trigger::AfterOpen) => after_open_steps.push_back(step.clone()),
                Some(Trigger::AfterDelay(delay)) => {
                    after_delay_steps.push((delay, step.clone(), false));
                }
                Some(Trigger::AfterDelayOnce(delay)) => {
                    after_delay_steps.push((delay, step.clone(), true));
                }
                Some(Trigger::OnCommand(kind)) => {
                    command_steps
                        .entry(kind)
                        .or_default()
                        .push_back(step.clone());
                }
                Some(Trigger::Manual) => {
                    manual_steps.insert(step.id.clone(), step.clone());
                }
                None => {
                    return Err(FixtureError::Invalid(format!(
                        "step {} has an invalid trigger {}",
                        step.id, step.trigger
                    )));
                }
            }
        }
        let limits = scenario.limits.unwrap_or(Limits::DEFAULT);
        let runtime = FixtureRuntime::new(&scenario)?;
        let authorization = scenario.read_authorization();
        let shared: Arc<Mutex<MockShared>> = Arc::new(Mutex::new(MockShared::default()));
        Ok(Self {
            scenario,
            runtime,
            limits,
            inbox: VecDeque::new(),
            inbox_bytes: 0,
            overflowed: false,
            disconnect_pending: false,
            sent: Vec::new(),
            selected: None,
            observation_count: 0,
            after_open_steps,
            after_delay_steps,
            delayed_steps: Vec::new(),
            session_serial: 0,
            consumed_once: BTreeSet::new(),
            trace: FixtureTrace::new(),
            #[cfg(test)]
            fixture_now_override: None,
            command_steps,
            manual_steps,
            grant_stamp_scope: None,
            served_scope: None,
            grant_flaw: None,
            receipts: BTreeMap::new(),
            command_digests: BTreeMap::new(),
            command_responses: BTreeMap::new(),
            active_read: None,
            read_revision: authorization.read_revision.clone(),
            authorization,
            pending_lookup: None,
            observation_control: None,
            shared,
        })
    }

    /// Shared control channel for this transcript.
    pub fn control(&self) -> MockControl {
        MockControl {
            state: Arc::clone(&self.shared),
        }
    }

    /// Scenario definition.
    pub fn scenario(&self) -> &FixtureScenario {
        &self.scenario
    }

    /// Fixture projection accumulator.
    pub fn runtime(&self) -> &FixtureRuntime {
        &self.runtime
    }

    /// Current synthetic epoch.
    pub fn epoch(&self) -> &EpochId {
        self.runtime.epoch()
    }

    /// Simulate a runtime restart with a new epoch.
    pub fn set_epoch(&mut self, epoch: EpochId) {
        self.cancel_schedule(ScheduleCancelReason::EpochChanged);
        self.runtime.set_epoch(epoch);
    }

    /// Raw client frames recorded by the provider.
    pub fn sent_frames(&self) -> &[Vec<u8>] {
        &self.sent
    }

    /// Binding the provider currently serves data under, when a grant is open.
    pub fn active_stamp(&self) -> Option<ReadStamp> {
        self.active_read.as_ref().map(|active| active.stamp.clone())
    }

    /// Bump the scripted read revision for late/equal grant transcripts.
    pub fn set_read_revision(&mut self, revision: ReadRevision) {
        self.read_revision = revision;
    }

    /// Whether a command is denied by the current fixture context.
    pub fn read_denies(&self, command_id: &CommandId) -> bool {
        self.authorization.denied_commands.contains(command_id)
    }

    /// Number of frames currently queued for the client.
    pub fn inbox_len(&self) -> usize {
        self.inbox.len()
    }

    /// Number of observation requests served.
    pub fn observation_count(&self) -> u32 {
        self.observation_count
    }

    /// Apply steered transcript actions queued by the test control channel.
    fn apply_control(&mut self) {
        loop {
            let command = {
                let Ok(mut shared) = self.shared.lock() else {
                    return;
                };
                shared.commands.pop_front()
            };
            let Some(command) = command else {
                return;
            };
            match command {
                MockCommand::Inject(raw) => self.inject_raw(raw),
                MockCommand::InjectVerbatim(raw) => self.inject_raw_unstamped(raw),
                MockCommand::Message { message, stamped } => {
                    let frame = if stamped {
                        self.message_frame(&message)
                    } else {
                        self.unstamped_frame(&message)
                    };
                    match frame {
                        Ok(bytes) => self.enqueue_bytes(bytes),
                        Err(error) => {
                            self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
                        }
                    }
                }
                MockCommand::StampedMessage { message, stamp } => {
                    match self.frame_with_stamp(&message, Some(*stamp)) {
                        Ok(bytes) => self.enqueue_bytes(bytes),
                        Err(error) => {
                            self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
                        }
                    }
                }
                MockCommand::Disconnect => self.simulate_disconnect(),
                MockCommand::SetEpoch(epoch) => self.set_epoch(epoch),
                MockCommand::DenyCommand(raw) => {
                    if let Ok(command_id) = CommandId::new(raw)
                        && !self.authorization.denied_commands.contains(&command_id)
                    {
                        self.authorization.denied_commands.push(command_id);
                    }
                }
                MockCommand::GrantStampScope(scope_id) => self.grant_stamp_scope = Some(scope_id),
                MockCommand::ServeScope(scope) => {
                    self.cancel_schedule(ScheduleCancelReason::ScopeChanged);
                    self.served_scope = Some(*scope);
                }
                MockCommand::GrantFlaw(flaw) => self.grant_flaw = Some(flaw),
                MockCommand::ClearGrantFlaw => self.grant_flaw = None,
                MockCommand::DenyLookup => {
                    if let Some(lookup) = self.pending_lookup.clone() {
                        self.enqueue_message(&ServerMessage::ReceiptReadResult(
                            ReceiptReadResult {
                                lookup_id: lookup.lookup_id,
                                stamp: lookup.stamp,
                                command_id: lookup.command_id,
                                outcome: ReceiptReadOutcome::Denied,
                            },
                        ));
                    }
                }
                MockCommand::AnswerLookup(receipt) => {
                    if let Some(lookup) = self.pending_lookup.clone() {
                        self.enqueue_message(&ServerMessage::ReceiptReadResult(
                            ReceiptReadResult {
                                lookup_id: lookup.lookup_id,
                                stamp: lookup.stamp,
                                command_id: lookup.command_id,
                                outcome: ReceiptReadOutcome::Allowed {
                                    receipt: receipt.map(|receipt| *receipt),
                                },
                            },
                        ));
                    }
                }
                MockCommand::DeliverStep(id) => {
                    if let Err(error) = self.deliver_step(&id) {
                        self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
                    }
                }
            }
        }
    }

    /// Queue a raw JSON frame for the client.
    ///
    /// State-bearing fixture frames are stamped with the provider's active
    /// binding, exactly as a real server would; a transcript that must stay
    /// unstamped uses [`Self::inject_raw_unstamped`].
    pub fn inject_raw(&mut self, raw: impl AsRef<str>) {
        let bytes = match self.stamp_frame(raw.as_ref()) {
            Some(bytes) => bytes,
            None => raw.as_ref().as_bytes().to_vec(),
        };
        self.enqueue_bytes(bytes);
    }

    /// Queue a raw JSON frame verbatim, preserving a missing or null stamp.
    pub fn inject_raw_unstamped(&mut self, raw: impl AsRef<str>) {
        let bytes = raw.as_ref().as_bytes().to_vec();
        self.enqueue_bytes(bytes);
    }

    /// Attach the active binding to a state-bearing frame that carries none.
    fn stamp_frame(&self, raw: &str) -> Option<Vec<u8>> {
        let active = self.active_read.as_ref()?;
        let mut value: Value = serde_json::from_str(raw).ok()?;
        if value.get("kind").and_then(Value::as_str) != Some("FRAME") {
            return None;
        }
        let message = value.get_mut("message")?;
        let kind = message
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let state_bearing = matches!(
            kind.as_deref(),
            Some(
                "REPLAY_ACCEPTED"
                    | "SNAPSHOT_BEGIN"
                    | "SNAPSHOT_CHUNK"
                    | "SNAPSHOT_END"
                    | "DELTA_TRANSACTION"
                    | "RECEIPT"
            )
        );
        if !state_bearing {
            return None;
        }
        let root = value.as_object_mut()?;
        if root.get("read_stamp").is_some_and(|stamp| !stamp.is_null()) {
            return None;
        }
        root.insert(
            "read_stamp".to_owned(),
            serde_json::to_value(active.stamp.clone()).ok()?,
        );
        serde_json::to_vec(&value).ok()
    }

    /// Feed a raw client frame as if the client had sent it.
    ///
    /// Tests may craft frames the typed client would refuse to build (unknown
    /// command kinds, stale epochs); the provider responds exactly as it would
    /// to a real peer.
    pub fn inject_client_frame(&mut self, raw: impl AsRef<str>) {
        let bytes = raw.as_ref().as_bytes().to_vec();
        self.sent.push(bytes.clone());
        self.handle_client_frame(&bytes);
    }

    /// Make the next poll report a transport disconnect (recovery tests).
    pub fn simulate_disconnect(&mut self) {
        self.cancel_schedule(ScheduleCancelReason::Disconnect);
        self.disconnect_pending = true;
    }

    /// Queue one scripted manual step; repeatable by design.
    pub fn deliver_step(&mut self, id: &str) -> Result<(), FixtureError> {
        let step =
            self.manual_steps.get(id).cloned().ok_or_else(|| {
                FixtureError::Invalid(format!("no manual fixture step named {id}"))
            })?;
        let frames = self.materialize_step(&step, None)?;
        for frame in frames {
            self.enqueue_bytes(frame);
        }
        Ok(())
    }

    fn fixture_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = self.fixture_now_override {
            return now;
        }
        Instant::now()
    }

    fn fixture_hint(&self) -> Option<Duration> {
        if self
            .inbox
            .iter()
            .any(|frame| frame.session_serial == Some(self.session_serial))
        {
            return Some(Duration::from_millis(1));
        }
        let now = self.fixture_now();
        self.delayed_steps
            .iter()
            .map(|step| step.release_at.saturating_duration_since(now))
            .min()
    }

    fn cancel_schedule(&mut self, reason: ScheduleCancelReason) {
        while let Some(step) = self.delayed_steps.pop() {
            self.trace.record(
                step.session_serial,
                &step.step.id,
                "cancelled",
                &[],
                Some(reason.as_str()),
                None,
            );
        }
        let serial = self.session_serial;
        let mut dropped_bytes = 0;
        self.inbox.retain(|frame| {
            let keep = frame.session_serial != Some(serial);
            if !keep {
                dropped_bytes += frame.bytes.len() as u64;
            }
            keep
        });
        self.inbox_bytes = self.inbox_bytes.saturating_sub(dropped_bytes);
    }

    /// Materialization is not delivery: reserve the complete batch before tracing
    /// and making any owned frame consumable.
    fn enqueue_due_steps(&mut self) {
        let now = self.fixture_now();
        let mut index = 0;
        while index < self.delayed_steps.len() {
            if self.delayed_steps[index].release_at > now {
                index += 1;
                continue;
            }
            let scheduled = self.delayed_steps.remove(index);
            if scheduled.session_serial != self.session_serial {
                self.trace.record(
                    scheduled.session_serial,
                    &scheduled.step.id,
                    "cancelled",
                    &[],
                    Some(ScheduleCancelReason::Replaced.as_str()),
                    None,
                );
                continue;
            }
            match self.materialize_step(&scheduled.step, None) {
                Ok(frames) => {
                    if scheduled.once {
                        self.consumed_once.insert(scheduled.step.id.clone());
                    }
                    self.trace.record(
                        scheduled.session_serial,
                        &scheduled.step.id,
                        "materialized",
                        &frames,
                        None,
                        None,
                    );
                    let bytes = frames
                        .iter()
                        .fold(0_u64, |sum, frame| sum.saturating_add(frame.len() as u64));
                    if self.inbox.len().saturating_add(frames.len())
                        > self.limits.max_queue_frames as usize
                        || self.inbox_bytes.saturating_add(bytes) > self.limits.max_queue_bytes
                        || self.inbox.try_reserve(frames.len()).is_err()
                    {
                        self.trace.fail();
                        self.overflowed = true;
                        return;
                    }
                    self.trace.record(
                        scheduled.session_serial,
                        &scheduled.step.id,
                        "delivered",
                        &frames,
                        None,
                        None,
                    );
                    self.inbox_bytes += bytes;
                    self.inbox
                        .extend(frames.into_iter().map(|bytes| QueuedFixtureFrame {
                            bytes,
                            session_serial: Some(scheduled.session_serial),
                        }));
                }
                Err(error) => {
                    self.trace.fail();
                    self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
                }
            }
        }
    }

    fn handle_client_frame(&mut self, frame: &[u8]) {
        let value: Value = match serde_json::from_slice(frame) {
            Ok(value) => value,
            Err(error) => {
                self.enqueue_error(
                    ErrorCode::ValidationFailed,
                    format!("malformed client frame: {error}"),
                    None,
                );
                return;
            }
        };
        match value.get("kind").and_then(Value::as_str) {
            Some("HELLO") => match serde_json::from_value::<HelloRequest>(value) {
                Ok(hello) => self.handle_hello(hello),
                Err(error) => {
                    self.enqueue_error(
                        ErrorCode::ValidationFailed,
                        format!("malformed hello: {error}"),
                        None,
                    );
                }
            },
            Some("FRAME") => {
                // Fixture-side observation of actual command dispatches: a test can
                // assert per-identity that no command was re-sent, which a total
                // frame count cannot show once reauthorization adds lookups.
                if let Some(message) = value.get("message")
                    && message.get("kind").and_then(Value::as_str) == Some("COMMAND")
                {
                    let id = message
                        .get("command_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let payload = message
                        .pointer("/payload/kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if let Ok(mut shared) = self.shared.lock() {
                        shared.command_sends.push((id.clone(), payload.clone()));
                    }
                    // Same seam, same evidence: the provider records the exact
                    // identity it received into the ordered run trace, so a native
                    // run's Send count is never inferred from the screen.
                    self.trace
                        .record_command(self.session_serial, &id, &payload);
                }
                self.handle_frame(value);
            }
            Some(other) => {
                self.enqueue_error(
                    ErrorCode::UnsupportedCommand,
                    format!("unknown client wire kind {other}"),
                    None,
                );
            }
            None => {
                self.enqueue_error(
                    ErrorCode::ValidationFailed,
                    "client frame is missing its kind",
                    None,
                );
            }
        }
    }

    fn handle_frame(&mut self, value: Value) {
        let Some(versions) = self.selected else {
            self.enqueue_error(
                ErrorCode::ValidationFailed,
                "frame received before the handshake completed",
                None,
            );
            return;
        };
        let protocol: Option<Version> = value
            .get("protocol")
            .cloned()
            .and_then(|raw| serde_json::from_value(raw).ok());
        let schema: Option<Version> = value
            .get("schema")
            .cloned()
            .and_then(|raw| serde_json::from_value(raw).ok());
        if protocol != Some(versions.protocol) || schema != Some(versions.schema) {
            self.enqueue_error(
                ErrorCode::VersionIncompatible,
                "frame versions do not match the negotiated session",
                None,
            );
            return;
        }
        let message = value.get("message").cloned().unwrap_or(Value::Null);
        let kind = message
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match kind.as_deref() {
            Some("OPEN_OBSERVATION") => {
                match serde_json::from_value::<OpenAuthorizedObservation>(message) {
                    Ok(open) => self.handle_open(open),
                    Err(error) => {
                        self.enqueue_error(
                            ErrorCode::ValidationFailed,
                            format!("malformed observation request: {error}"),
                            None,
                        );
                    }
                }
            }
            Some("COMMAND") => self.handle_command_value(message),
            Some("LOOKUP_RECEIPT") => {
                match serde_json::from_value::<AuthorizedReceiptLookup>(message) {
                    Ok(lookup) => self.handle_lookup(lookup),
                    Err(error) => {
                        self.enqueue_error(
                            ErrorCode::ValidationFailed,
                            format!("malformed receipt lookup: {error}"),
                            None,
                        );
                    }
                }
            }
            Some(other) => {
                self.enqueue_error(
                    ErrorCode::UnsupportedCommand,
                    format!("unknown client message kind {other}"),
                    None,
                );
            }
            None => {
                self.enqueue_error(
                    ErrorCode::ValidationFailed,
                    "client message is missing its kind",
                    None,
                );
            }
        }
    }

    fn handle_hello(&mut self, hello: HelloRequest) {
        let client_advertisement = hello.advertisement();
        let server_advertisement = VersionAdvertisement {
            protocol: self.scenario.hello.protocol.clone(),
            schema: self.scenario.hello.schema.clone(),
        };
        match negotiate_session(&client_advertisement, &server_advertisement) {
            Ok(versions) => {
                self.selected = Some(versions);
                let accepted = HelloAccepted {
                    protocol: versions.protocol,
                    schema: versions.schema,
                    server_identity: self.scenario.server_identity.clone(),
                    server_build: self.scenario.hello.server_build.clone(),
                    epoch: self.runtime.epoch().clone(),
                    dedupe_retention_ms: self.scenario.hello.dedupe_retention_ms,
                    limits: Some(self.limits),
                };
                self.enqueue_wire(&ServerWire::HelloAccepted(accepted));
            }
            Err(failure) => {
                let rejected = HelloRejected {
                    code: ErrorCode::VersionIncompatible,
                    message: format!("no mutually supported version ({failure})"),
                    reason: failure,
                    protocol: server_advertisement.protocol,
                    schema: server_advertisement.schema,
                };
                self.enqueue_wire(&ServerWire::HelloRejected(rejected));
            }
        }
    }

    fn handle_open(&mut self, open: OpenAuthorizedObservation) {
        self.cancel_schedule(ScheduleCancelReason::Replaced);
        if self.selected.is_none() {
            self.enqueue_error(
                ErrorCode::ValidationFailed,
                "observation requested before the handshake completed",
                None,
            );
            return;
        }
        let OpenAuthorizedObservation {
            request_id,
            observation: open,
            retained_commands,
        } = open;
        self.observation_count += 1;
        if let Some(epoch) = &open.epoch
            && epoch != self.runtime.epoch()
        {
            self.enqueue_resync(ResyncReason::EpochChanged, "runtime epoch changed");
            return;
        }
        if let Some(scope_id) = &open.scope_id
            && scope_id != &self.scenario.scope.scope_id
        {
            self.enqueue_resync(ResyncReason::UnknownScope, "observation scope is unknown");
            return;
        }
        if self.authorization.grant_on_open {
            let grant = self.issue_grant(&request_id, &retained_commands);
            if let Err(error) = self.schedule_after_delay_steps() {
                self.trace.fail();
                self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
                return;
            }
            self.enqueue_message(&ServerMessage::ReadGrant(grant));
        } else {
            self.active_read = None;
            self.trace
                .record(self.session_serial, "", "withheld", &[], None, None);
        }
        if let Some(cursor) = &open.last_seen_cursor {
            match self.scenario.observation_mode {
                ObservationMode::ResyncRequired => {
                    self.enqueue_resync(
                        ResyncReason::ExpiredCursor,
                        "replay history is unavailable for this scope",
                    );
                    return;
                }
                ObservationMode::Replay => match self.runtime.replay_after(cursor) {
                    Some(transactions) => {
                        self.enqueue_message(&ServerMessage::ReplayAccepted(ReplayAccepted {
                            cursor: cursor.clone(),
                        }));
                        for transaction in transactions {
                            self.enqueue_message(&ServerMessage::DeltaTransaction(transaction));
                        }
                        return;
                    }
                    None => {
                        self.enqueue_resync(
                            ResyncReason::ExpiredCursor,
                            "cursor is no longer replayable",
                        );
                        return;
                    }
                },
                ObservationMode::Snapshot => {}
            }
        }
        self.enqueue_snapshot();
        if self.observation_count == 1 {
            let steps: Vec<FixtureStep> = self.after_open_steps.drain(..).collect();
            for step in &steps {
                match self.materialize_step(step, None) {
                    Ok(frames) => {
                        for frame in frames {
                            self.enqueue_bytes(frame);
                        }
                    }
                    Err(error) => {
                        self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
                    }
                }
            }
        }
    }

    /// A new grant cancels its predecessor and re-arms only repeatable steps.
    fn schedule_after_delay_steps(&mut self) -> Result<(), FixtureError> {
        self.session_serial = self
            .session_serial
            .checked_add(1)
            .ok_or_else(|| FixtureError::Invalid("fixture session serial exhausted".to_owned()))?;
        let issued_at = self.fixture_now();
        self.trace
            .record(self.session_serial, "", "grant", &[], None, None);
        for (delay, step, once) in &self.after_delay_steps {
            if *once && self.consumed_once.contains(&step.id) {
                continue;
            }
            let release_at = issued_at.checked_add(*delay).ok_or_else(|| {
                FixtureError::Invalid("fixture deadline cannot be represented".to_owned())
            })?;
            self.delayed_steps.push(ScheduledFixtureStep {
                release_at,
                session_serial: self.session_serial,
                once: *once,
                step: step.clone(),
            });
            self.trace
                .record(self.session_serial, &step.id, "armed", &[], None, None);
        }
        Ok(())
    }

    /// Issue the fixture grant for one authorized open request.
    ///
    /// When a [`GrantFlaw`] is armed, the issued grant carries exactly that one
    /// contradiction: well-formed in every other respect, it is the sole reason
    /// the client must refuse it.
    fn issue_grant(&mut self, request_id: &ReadRequestId, retained: &[CommandId]) -> ReadGrant {
        let stamp = self.current_stamp(request_id);
        let mut command_reads: Vec<CommandReadDecision> = retained
            .iter()
            .map(|command_id| CommandReadDecision {
                command_id: command_id.clone(),
                readable: !self.authorization.denied_commands.contains(command_id),
            })
            .collect();
        let mut stamp = stamp;
        match self.grant_flaw {
            None => {}
            Some(GrantFlaw::MissingDecision) => {
                command_reads.pop();
            }
            Some(GrantFlaw::ExtraDecision) => {
                command_reads.push(CommandReadDecision {
                    command_id: CommandId::fixture("command/unrequested"),
                    readable: true,
                });
            }
            Some(GrantFlaw::DuplicateDecision) => {
                if let Some(first) = command_reads.first().cloned() {
                    command_reads.push(first);
                }
            }
            Some(GrantFlaw::ForeignServer) => {
                stamp.server_id = ServerId::fixture("server/foreign");
            }
        }
        self.active_read = Some(ActiveRead {
            stamp: stamp.clone(),
            denied: self.authorization.denied_commands.iter().cloned().collect(),
            absent: self.authorization.absent_commands.iter().cloned().collect(),
        });
        if let Ok(mut shared) = self.shared.lock() {
            shared.active_stamp = Some(stamp.clone());
        }
        ReadGrant {
            stamp,
            scope: self.issued_scope(),
            local_intents_readable: self.authorization.local_intents_readable,
            command_reads,
        }
    }

    fn current_stamp(&self, request_id: &ReadRequestId) -> ReadStamp {
        ReadStamp {
            request_id: request_id.clone(),
            server_id: self.scenario.server_identity.clone(),
            epoch: self.runtime.epoch().clone(),
            context_id: self.authorization.context_id.clone(),
            read_revision: self.read_revision.clone(),
            scope_id: match &self.grant_stamp_scope {
                Some(scope_id) => desktop_protocol::ScopeId::fixture(scope_id.clone()),
                None => self.scenario.scope.scope_id.clone(),
            },
        }
    }

    /// Script a close-only invalidation with a strictly newer revision.
    fn invalidate_read(&mut self, reason: ReadInvalidationReason) -> Result<Vec<u8>, FixtureError> {
        let Some(active) = self.active_read.take() else {
            return Err(FixtureError::Invalid(
                "no active read binding to invalidate".to_owned(),
            ));
        };
        self.cancel_schedule(ScheduleCancelReason::AuthorizationChanged);
        if let Ok(mut shared) = self.shared.lock() {
            shared.active_stamp = None;
        }
        let floor = self
            .read_revision
            .as_counter()
            .to_u128()
            .and_then(|revision| revision.checked_add(1))
            .ok_or_else(|| FixtureError::Invalid("fixture read revision exhausted".to_owned()))?;
        let fresh = floor
            .checked_add(1)
            .ok_or_else(|| FixtureError::Invalid("fixture read revision exhausted".to_owned()))?;
        // The invalidation establishes a floor. The scripted producer's next
        // grant must be strictly newer, never the equal revision the client rejects.
        self.read_revision = ReadRevision::new(desktop_protocol::Counter::from_u128(fresh));
        self.message_frame(&ServerMessage::ReadInvalidated(ReadInvalidated {
            request_id: active.stamp.request_id.clone(),
            context_id: active.stamp.context_id.clone(),
            read_revision: ReadRevision::new(desktop_protocol::Counter::from_u128(floor)),
            reason,
        }))
    }

    /// Answer the outstanding authorized lookup from the script.
    fn scripted_receipt_read(
        &mut self,
        script: &crate::fixture::FixtureReceiptRead,
    ) -> Result<Vec<u8>, FixtureError> {
        let Some(pending) = self.pending_lookup.clone() else {
            return Err(FixtureError::Invalid(
                "no pending receipt lookup to answer".to_owned(),
            ));
        };
        let command_id = script
            .command_id
            .clone()
            .unwrap_or_else(|| pending.command_id.clone());
        if command_id != pending.command_id {
            return Err(FixtureError::Invalid(
                "scripted receipt read answers another command".to_owned(),
            ));
        }
        let outcome = match script.outcome {
            FixtureReceiptOutcome::Denied => ReceiptReadOutcome::Denied,
            FixtureReceiptOutcome::Absent => ReceiptReadOutcome::Allowed { receipt: None },
            FixtureReceiptOutcome::Allowed => ReceiptReadOutcome::Allowed {
                receipt: self.receipts.get(&command_id).cloned(),
            },
        };
        self.message_frame(&ServerMessage::ReceiptReadResult(ReceiptReadResult {
            lookup_id: pending.lookup_id,
            stamp: pending.stamp,
            command_id,
            outcome,
        }))
    }

    fn enqueue_snapshot(&mut self) {
        let bundle = match self.runtime.snapshot_bundle() {
            Ok(bundle) => bundle,
            Err(error) => {
                self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
                return;
            }
        };
        let scope = self.issued_scope();
        self.enqueue_message(&ServerMessage::SnapshotBegin(SnapshotBegin {
            snapshot_id: bundle.snapshot_id.clone(),
        }));
        for (index, part) in bundle.parts.iter().enumerate() {
            self.enqueue_message(&ServerMessage::SnapshotChunk(SnapshotChunk {
                snapshot_id: bundle.snapshot_id.clone(),
                part_index: index as u32,
                mutations: part.clone(),
            }));
        }
        self.enqueue_message(&ServerMessage::SnapshotEnd(SnapshotEnd {
            snapshot_id: bundle.snapshot_id,
            scope,
            epoch: self.runtime.epoch().clone(),
            cursor: bundle.cursor,
            expected: bundle.counts,
            integrity_digest: bundle.digest,
        }));
    }

    fn issued_scope(&self) -> ObservationScope {
        let versions = self.selected.unwrap_or(ProtocolVersions {
            protocol: Version::new(2, 0),
            schema: Version::new(2, 0),
        });
        let task_ids = self
            .served_scope
            .as_ref()
            .map(|scope| scope.task_ids.clone())
            .unwrap_or_else(|| self.scenario.scope.task_ids.clone());
        let filters = self
            .served_scope
            .as_ref()
            .map(|scope| scope.filters.clone())
            .unwrap_or_else(|| self.scenario.scope.filters.clone());
        ObservationScope {
            scope_id: self.scenario.scope.scope_id.clone(),
            task_ids,
            filters,
            protocol: versions.protocol,
            schema: versions.schema,
        }
    }

    fn handle_command_value(&mut self, message: Value) {
        let payload_kind = message
            .get("payload")
            .and_then(|payload| payload.get("kind"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        match payload_kind.as_deref() {
            Some("CREATE_TASK") | Some("STOP_TASK") | Some("REUSE_TASK") => {}
            Some(other) => {
                let command_id = message
                    .get("command_id")
                    .cloned()
                    .and_then(|raw| serde_json::from_value::<CommandId>(raw).ok());
                self.enqueue_error(
                    ErrorCode::UnsupportedCommand,
                    format!("unknown command kind {other}"),
                    command_id,
                );
                return;
            }
            None => {
                self.enqueue_error(
                    ErrorCode::ValidationFailed,
                    "command is missing its payload kind",
                    None,
                );
                return;
            }
        }
        match serde_json::from_value::<CommandEnvelope>(message) {
            Ok(envelope) => self.handle_command(envelope),
            Err(error) => {
                self.enqueue_error(
                    ErrorCode::ValidationFailed,
                    format!("malformed command envelope: {error}"),
                    None,
                );
            }
        }
    }

    fn handle_command(&mut self, envelope: CommandEnvelope) {
        if let Err(error) = envelope.validate() {
            self.enqueue_error(
                ErrorCode::ValidationFailed,
                error.to_string(),
                Some(envelope.command_id.clone()),
            );
            return;
        }
        let digest = match integrity_digest_of(&envelope) {
            Ok(digest) => digest,
            Err(error) => {
                self.enqueue_error(
                    ErrorCode::InternalError,
                    error.to_string(),
                    Some(envelope.command_id.clone()),
                );
                return;
            }
        };
        if let Some(existing) = self.command_digests.get(&envelope.command_id) {
            if existing == &digest {
                let responses = self
                    .command_responses
                    .get(&envelope.command_id)
                    .cloned()
                    .unwrap_or_default();
                for response in responses {
                    self.enqueue_bytes(response);
                }
            } else {
                self.enqueue_error(
                    ErrorCode::CommandIdConflict,
                    "the same command id was reused with different content",
                    Some(envelope.command_id.clone()),
                );
            }
            return;
        }
        self.command_digests
            .insert(envelope.command_id.clone(), digest);
        let operation = operation_of(&envelope.payload);
        let kind = operation.as_str().to_owned();
        let steps: Vec<FixtureStep> = match self.command_steps.get_mut(&kind) {
            Some(queue) => queue.drain(..).collect(),
            None => Vec::new(),
        };
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut delivered_receipt = false;
        for step in &steps {
            match self.materialize_step(step, Some(&envelope)) {
                Ok(step_frames) => {
                    if step.actions.iter().any(|action| action.receipt.is_some()) {
                        delivered_receipt = true;
                    }
                    frames.extend(step_frames);
                }
                Err(error) => {
                    frames.push(self.error_frame(
                        ErrorCode::InternalError,
                        error.to_string(),
                        Some(envelope.command_id.clone()),
                    ));
                }
            }
        }
        if !delivered_receipt {
            let default = self
                .scenario
                .commands
                .get(&kind)
                .map(|policy| policy.default)
                .unwrap_or(CommandDefault::Unsupported);
            match default {
                CommandDefault::Accepted => {
                    let receipt = self.synthesized_receipt(&envelope, operation);
                    frames.push(self.receipt_frame(&receipt));
                }
                CommandDefault::Unsupported => {
                    frames.push(self.error_frame(
                        ErrorCode::UnsupportedCommand,
                        format!("command family {kind} is not supported by this fixture"),
                        Some(envelope.command_id.clone()),
                    ));
                }
                CommandDefault::Silent => {}
            }
        }
        self.command_responses
            .insert(envelope.command_id.clone(), frames.clone());
        for frame in frames {
            self.enqueue_bytes(frame);
        }
    }

    fn handle_lookup(&mut self, lookup: AuthorizedReceiptLookup) {
        let Some(active) = self.active_read.as_ref() else {
            self.enqueue_error(
                ErrorCode::Unauthorized,
                "no active read binding is bound to this channel",
                None,
            );
            return;
        };
        if lookup.stamp != active.stamp {
            self.enqueue_error(
                ErrorCode::ValidationFailed,
                "receipt lookup stamp is not the current binding",
                None,
            );
            return;
        }
        if let Ok(mut shared) = self.shared.lock() {
            shared.lookups += 1;
        }
        self.pending_lookup = Some(lookup.clone());
        if !self.authorization.answer_lookups {
            // The transcript answers this lookup itself.
            return;
        }
        let outcome = if active.denied.contains(&lookup.command_id)
            || self
                .authorization
                .denied_commands
                .contains(&lookup.command_id)
        {
            ReceiptReadOutcome::Denied
        } else if active.absent.contains(&lookup.command_id) {
            ReceiptReadOutcome::Allowed { receipt: None }
        } else {
            match self.receipts.get(&lookup.command_id) {
                Some(receipt) => ReceiptReadOutcome::Allowed {
                    receipt: Some(receipt.clone()),
                },
                None => ReceiptReadOutcome::Allowed { receipt: None },
            }
        };
        self.enqueue_message(&ServerMessage::ReceiptReadResult(ReceiptReadResult {
            lookup_id: lookup.lookup_id,
            stamp: lookup.stamp,
            command_id: lookup.command_id,
            outcome,
        }));
    }

    fn materialize_step(
        &mut self,
        step: &FixtureStep,
        command: Option<&CommandEnvelope>,
    ) -> Result<Vec<Vec<u8>>, FixtureError> {
        let mut frames = Vec::new();
        for action in &step.actions {
            if let Some(delta) = &action.delta {
                let transaction = self.runtime.commit_delta(delta)?;
                frames.push(self.message_frame(&ServerMessage::DeltaTransaction(transaction))?);
            }
            if let Some(template) = &action.receipt {
                let receipt = self.materialize_receipt(template, command, &step.id);
                frames.push(self.receipt_frame(&receipt));
            }
            if let Some(error) = &action.error {
                let mut error = error.clone();
                if error.command_id.is_none() {
                    error.command_id = command.map(|envelope| envelope.command_id.clone());
                }
                frames.push(self.message_frame(&ServerMessage::Error(error))?);
            }
            if let Some(raw) = &action.raw {
                frames.push(serde_json::to_vec(raw)?);
            }
            if let Some(reason) = &action.resync_required {
                frames.push(self.message_frame(&ServerMessage::ResyncRequired(
                    ResyncRequired {
                        reason: reason.clone(),
                        message: Some(format!("scripted resync from step {}", step.id)),
                    },
                ))?);
            }
            if let Some(reason) = &action.read_invalidated {
                frames.push(self.invalidate_read(reason.clone())?);
            }
            if let Some(script) = &action.receipt_read {
                frames.push(self.scripted_receipt_read(script)?);
            }
        }
        Ok(frames)
    }

    fn materialize_receipt(
        &mut self,
        template: &FixtureReceipt,
        command: Option<&CommandEnvelope>,
        step_id: &str,
    ) -> CommandReceipt {
        let (command_id, operation) = match command {
            Some(envelope) => (envelope.command_id.clone(), operation_of(&envelope.payload)),
            None => (
                CommandId::fixture(format!("command/{step_id}")),
                template
                    .operation
                    .clone()
                    .unwrap_or(CommandOperation::Unknown("UNKNOWN".to_owned())),
            ),
        };
        let receipt = CommandReceipt {
            command_id: command_id.clone(),
            operation,
            state: template.state.clone(),
            task_id: template.task_id.clone(),
            accepted_at: template.accepted_at.clone(),
            reject_reason: template.reject_reason.clone(),
            effect_refs: template.effect_refs.clone(),
            dedupe_expires_at: None,
            message: template.message.clone(),
        };
        self.receipts.insert(command_id, receipt.clone());
        receipt
    }

    fn synthesized_receipt(
        &mut self,
        envelope: &CommandEnvelope,
        operation: CommandOperation,
    ) -> CommandReceipt {
        let task_id = match &envelope.target {
            CommandTarget::Task { task_id } => Some(task_id.clone()),
            CommandTarget::Creation { .. } => None,
        };
        let receipt = CommandReceipt {
            command_id: envelope.command_id.clone(),
            operation,
            state: desktop_protocol::ReceiptState::Accepted,
            task_id,
            accepted_at: None,
            reject_reason: None,
            effect_refs: Vec::new(),
            dedupe_expires_at: None,
            message: Some(
                "fixture acceptance: validated intent recorded, not semantic completion".to_owned(),
            ),
        };
        self.receipts
            .insert(envelope.command_id.clone(), receipt.clone());
        receipt
    }

    fn enqueue_message(&mut self, message: &ServerMessage) {
        match self.message_frame(message) {
            Ok(frame) => self.enqueue_bytes(frame),
            Err(error) => {
                self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
            }
        }
    }

    fn message_frame(&self, message: &ServerMessage) -> Result<Vec<u8>, FixtureError> {
        let versions = self.selected.unwrap_or(ProtocolVersions {
            protocol: Version::new(2, 0),
            schema: Version::new(2, 0),
        });
        let wire = ServerWire::Frame(ServerFrame {
            protocol: versions.protocol,
            schema: versions.schema,
            read_stamp: message_stamp(message, self.active_read.as_ref()),
            message: message.clone(),
        });
        Ok(serde_json::to_vec(&wire)?)
    }

    /// Frame for a typed message with no authorization stamp at all.
    fn unstamped_frame(&self, message: &ServerMessage) -> Result<Vec<u8>, FixtureError> {
        self.frame_with_stamp(message, None)
    }

    /// Frame for a typed message with an explicit stamp.
    fn frame_with_stamp(
        &self,
        message: &ServerMessage,
        read_stamp: Option<ReadStamp>,
    ) -> Result<Vec<u8>, FixtureError> {
        let versions = self.selected.unwrap_or(ProtocolVersions {
            protocol: Version::new(2, 0),
            schema: Version::new(2, 0),
        });
        let wire = ServerWire::Frame(ServerFrame {
            protocol: versions.protocol,
            schema: versions.schema,
            read_stamp,
            message: message.clone(),
        });
        Ok(serde_json::to_vec(&wire)?)
    }

    fn receipt_frame(&self, receipt: &CommandReceipt) -> Vec<u8> {
        let Some(active) = self.active_read.as_ref() else {
            // An unverified or unauthorized channel cannot disclose a receipt.
            return Vec::new();
        };
        if active.denied.contains(&receipt.command_id) {
            // Denied carries no receipt, retention or execution fact.
            return Vec::new();
        }
        let authorized = AuthorizedCommandReceipt {
            stamp: active.stamp.clone(),
            permission: CommandReadDecision {
                command_id: receipt.command_id.clone(),
                readable: true,
            },
            receipt: receipt.clone(),
        };
        self.message_frame(&ServerMessage::Receipt(authorized))
            .unwrap_or_default()
    }

    fn enqueue_wire(&mut self, wire: &ServerWire) {
        match serde_json::to_vec(wire) {
            Ok(frame) => self.enqueue_bytes(frame),
            Err(error) => {
                self.enqueue_error(ErrorCode::InternalError, error.to_string(), None);
            }
        }
    }

    fn enqueue_error(
        &mut self,
        code: ErrorCode,
        message: impl Into<String>,
        command_id: Option<CommandId>,
    ) {
        let frame = self.error_frame(code, message.into(), command_id);
        self.enqueue_bytes(frame);
    }

    fn error_frame(
        &self,
        code: ErrorCode,
        message: String,
        command_id: Option<CommandId>,
    ) -> Vec<u8> {
        let error = ProtocolError {
            code,
            message,
            correlation_id: None,
            command_id,
            scope_id: None,
            retry_after_ms: None,
            field_errors: Vec::new(),
            current_revision: None,
            resync_reason: None,
        };
        self.message_frame(&ServerMessage::Error(error))
            .unwrap_or_default()
    }

    fn enqueue_resync(&mut self, reason: ResyncReason, message: &str) {
        self.enqueue_message(&ServerMessage::ResyncRequired(ResyncRequired {
            reason,
            message: Some(message.to_owned()),
        }));
    }

    fn enqueue_bytes(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let frames = self.inbox.len() as u64 + 1;
        let total_bytes = self.inbox_bytes.saturating_add(bytes.len() as u64);
        if frames > u64::from(self.limits.max_queue_frames)
            || total_bytes > self.limits.max_queue_bytes
        {
            self.overflowed = true;
            return;
        }
        self.inbox_bytes = total_bytes;
        self.inbox.push_back(QueuedFixtureFrame {
            bytes,
            session_serial: None,
        });
    }
}

/// Stamp carried by a state-bearing fixture frame under the active binding.
fn message_stamp(message: &ServerMessage, active: Option<&ActiveRead>) -> Option<ReadStamp> {
    match message {
        ServerMessage::ReplayAccepted(_)
        | ServerMessage::SnapshotBegin(_)
        | ServerMessage::SnapshotChunk(_)
        | ServerMessage::SnapshotEnd(_)
        | ServerMessage::DeltaTransaction(_)
        | ServerMessage::Receipt(_) => active.map(|active| active.stamp.clone()),
        // A state-bearing error settles a command, so it carries the binding too.
        ServerMessage::Error(error) if error.command_id.is_some() => {
            active.map(|active| active.stamp.clone())
        }
        ServerMessage::ResyncRequired(_)
        | ServerMessage::ReadGrant(_)
        | ServerMessage::ReadInvalidated(_)
        | ServerMessage::ReceiptReadResult(_)
        | ServerMessage::Error(_)
        | ServerMessage::Unknown(_) => None,
    }
}

impl Transport for MockTransport {
    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        self.sent.push(frame.clone());
        if let Ok(mut shared) = self.shared.lock() {
            shared.sent_frames += 1;
        }
        self.handle_client_frame(&frame);
        Ok(())
    }

    // Poll is the sole delivery authority; a reported deadline is only a wake hint.
    fn poll(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        self.apply_control();
        if self.disconnect_pending {
            self.cancel_schedule(ScheduleCancelReason::Disconnect);
        } else {
            self.enqueue_due_steps();
        }
        if self.disconnect_pending {
            self.disconnect_pending = false;
            return Err(TransportError::Disconnected);
        }
        if self.overflowed {
            self.cancel_schedule(ScheduleCancelReason::Capacity);
            self.overflowed = false;
            self.inbox.clear();
            self.inbox_bytes = 0;
            return Err(TransportError::QueueOverflow);
        }
        let frame = self.inbox.pop_front();
        if let Some(frame) = &frame {
            self.inbox_bytes = self.inbox_bytes.saturating_sub(frame.bytes.len() as u64);
        }
        Ok(frame.map(|frame| frame.bytes))
    }

    fn reset_stream(&mut self) {
        self.cancel_schedule(ScheduleCancelReason::ResetStream);
        // Only the stream is reset. The scripted producer keeps the binding it
        // is serving, so a client that re-opens under the current context is
        // answered from that same binding instead of a cleared one.
        self.inbox.clear();
        self.inbox_bytes = 0;
        self.overflowed = false;
    }

    fn bind_observation_control(&mut self, control: crate::ObservationControl) {
        self.observation_control = Some(control);
    }

    fn authenticated_server(&self) -> Option<ServerId> {
        // Fixture identity: visibly synthetic, never a claim of real auth.
        Some(self.scenario.server_identity.clone())
    }
}

fn operation_of(payload: &CommandPayload) -> CommandOperation {
    match payload {
        CommandPayload::CreateTask(_) => CommandOperation::CreateTask,
        CommandPayload::StopTask(_) => CommandOperation::StopTask,
        CommandPayload::ReuseTask(_) => CommandOperation::ReuseTask,
    }
}

/// Fixture provider: a [`CosClient`] over the scripted transport.
pub struct MockCosClient {
    client: CosClient<MockTransport>,
    scenario_id: String,
    primary_task: Option<TaskId>,
    traced_consumer: Option<(&'static str, &'static str, u64)>,
}

impl MockCosClient {
    /// Load a scenario file and build the provider.
    pub fn from_fixture_file(path: impl AsRef<Path>) -> Result<Self, FixtureError> {
        let scenario = FixtureScenario::from_file(path)?;
        Self::from_scenario(scenario)
    }

    /// Parse a scenario string and build the provider.
    pub fn from_fixture_str(text: &str) -> Result<Self, FixtureError> {
        let scenario = FixtureScenario::from_json(text)?;
        Self::from_scenario(scenario)
    }

    /// Build the provider from a scenario value (tests may adjust it first).
    pub fn from_scenario(scenario: FixtureScenario) -> Result<Self, FixtureError> {
        Self::from_scenario_with_config(scenario, ClientConfig::default())
    }

    /// Build the provider with an explicit client configuration.
    pub fn from_scenario_with_config(
        scenario: FixtureScenario,
        config: ClientConfig,
    ) -> Result<Self, FixtureError> {
        scenario.validate()?;
        let limits = scenario.limits.unwrap_or(Limits::DEFAULT);
        let scenario_id = scenario.scenario_id.clone();
        let primary_task = scenario.primary_task_id().cloned();
        let transport = MockTransport::new(scenario)?;
        let client = CosClient::new(
            transport,
            config,
            limits,
            ClientSource::Fixture {
                scenario_id: scenario_id.clone(),
            },
        );
        Ok(Self {
            client,
            scenario_id,
            primary_task,
            traced_consumer: None,
        })
    }

    /// Scenario identity.
    pub fn scenario_id(&self) -> &str {
        &self.scenario_id
    }

    /// Primary observed task identity.
    pub fn primary_task(&self) -> Option<&TaskId> {
        self.primary_task.as_ref()
    }

    /// Connect and open the scripted observation.
    pub fn connect(&mut self, now_ms: u64) -> Result<(), ClientError> {
        let scope: ObservationScopeRequest =
            self.client.transport().scenario().observation_request();
        self.client.connect(scope, now_ms)
    }

    /// Drive the session with the default frame budget.
    pub fn pump(&mut self, now_ms: u64) -> Vec<ClientEvent> {
        self.client.pump(now_ms)
    }

    /// Drive the session with an explicit frame budget.
    pub fn pump_bounded(&mut self, now_ms: u64, max_frames: usize) -> Vec<ClientEvent> {
        self.client.pump_bounded(now_ms, max_frames)
    }

    /// Immutable authoritative projection snapshot.
    pub fn snapshot(&self) -> SnapshotHandle {
        self.client.snapshot()
    }

    /// Current connection state.
    pub fn connection(&self) -> &ConnectionState {
        self.client.connection()
    }

    /// Whether the visible projection may be stale.
    pub fn is_stale(&self) -> bool {
        self.client.is_stale()
    }

    /// Data source; always [`ClientSource::Fixture`] for this provider.
    pub fn source(&self) -> ClientSource {
        self.client.source()
    }

    /// Most recent error.
    pub fn last_error(&self) -> Option<&ClientError> {
        self.client.last_error()
    }

    /// Send a Stop request.
    pub fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, ClientError> {
        self.client.stop_task(task_id)
    }

    /// Send an arbitrary prepared envelope.
    pub fn send_command(&mut self, envelope: CommandEnvelope) -> Result<CommandId, ClientError> {
        self.client.send_command(envelope)
    }

    /// Local command status.
    pub fn lookup_command(&self, command_id: &CommandId) -> CommandLookup<'_> {
        self.client.lookup_command(command_id)
    }

    /// The single authoritative retained registry.
    pub fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus> {
        self.client.command_statuses()
    }

    /// Number of identity-level read-evidence entries held right now.
    pub fn read_evidence_len(&self) -> usize {
        self.client.read_evidence_len()
    }

    /// Whether current evidence exists for one identity.
    pub fn read_evidence_has(&self, id: &CommandId) -> bool {
        self.client.read_evidence_has(id)
    }

    /// Bounded background progress with event-free change reporting.
    pub fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport {
        let mut report = self.client.pump_step(now_ms, budget);
        if let Some(delay) = self.client.transport().fixture_hint() {
            let milliseconds = delay.as_nanos().div_ceil(1_000_000).max(1);
            if let Ok(milliseconds) = u64::try_from(milliseconds) {
                let at = now_ms.saturating_add(milliseconds);
                report.next_deadline_ms =
                    Some(report.next_deadline_ms.map_or(at, |old| old.min(at)));
            }
        }
        if self.client.transport().trace.enabled() {
            let access = match self.client.publication_access() {
                PublicationAccess::Validated { .. } => "validated",
                PublicationAccess::Withheld { .. } => "withheld",
            };
            let connection = self.client.connection().label();
            let serial = self.client.transport().session_serial;
            let state = (access, connection, serial);
            if report.changed || self.traced_consumer != Some(state) {
                self.client.transport_mut().trace.record(
                    serial,
                    "",
                    "consumer_state",
                    &[],
                    None,
                    Some((access, connection)),
                );
                self.traced_consumer = Some(state);
            }
        }
        report
    }

    /// Validated publication evidence.
    pub fn publication_access(&self) -> PublicationAccess<'_> {
        self.client.publication_access()
    }

    /// Begin observation with explicit authorized retention interest.
    pub fn begin_authorized_observation(
        &mut self,
        request: OpenAuthorizedObservation,
    ) -> Result<(), ClientError> {
        self.client.begin_authorized_observation(request)
    }

    /// Issue an authorized receipt lookup.
    pub fn lookup_receipt_authorized(
        &mut self,
        request: AuthorizedReceiptLookup,
    ) -> Result<(), ClientError> {
        self.client.lookup_receipt_authorized(request)
    }

    /// Bind the shared close-only publication fence.
    pub fn bind_observation_control(&mut self, control: crate::ObservationControl) {
        self.client.bind_observation_control(control);
    }

    /// Request a full resync.
    pub fn request_resync(&mut self, reason: ResyncReason) -> Vec<ClientEvent> {
        self.client.request_resync(reason)
    }

    /// Local disconnect (cancels observation only).
    pub fn disconnect(&mut self) -> Vec<ClientEvent> {
        self.client.disconnect()
    }

    /// Borrow the underlying client.
    pub fn client(&self) -> &CosClient<MockTransport> {
        &self.client
    }

    /// Mutably borrow the underlying client.
    pub fn client_mut(&mut self) -> &mut CosClient<MockTransport> {
        &mut self.client
    }

    /// Borrow the scripted transport (injection, epoch control, manual steps).
    pub fn mock(&self) -> &MockTransport {
        self.client.transport()
    }

    /// Mutably borrow the scripted transport.
    pub fn mock_mut(&mut self) -> &mut MockTransport {
        self.client.transport_mut()
    }

    /// Shared control channel for this transcript.
    ///
    /// Steered actions are applied on the provider's next poll, so a steered
    /// fixture never blocks or mutates the client from another thread.
    pub fn control(&self) -> MockControl {
        self.client.transport().control()
    }
}

impl fmt::Debug for MockCosClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockCosClient")
            .field("scenario_id", &self.scenario_id)
            .field("connection", self.client.connection())
            .finish_non_exhaustive()
    }
}

impl DesktopClient for MockCosClient {
    fn snapshot(&self) -> SnapshotHandle {
        MockCosClient::snapshot(self)
    }

    fn connection(&self) -> &ConnectionState {
        MockCosClient::connection(self)
    }

    fn is_stale(&self) -> bool {
        MockCosClient::is_stale(self)
    }

    fn source(&self) -> ClientSource {
        MockCosClient::source(self)
    }

    fn last_error(&self) -> Option<&ClientError> {
        MockCosClient::last_error(self)
    }

    fn pump(&mut self, now_ms: u64) -> Vec<ClientEvent> {
        MockCosClient::pump(self, now_ms)
    }

    fn disconnect(&mut self) -> Vec<ClientEvent> {
        MockCosClient::disconnect(self)
    }

    fn request_resync(&mut self, reason: ResyncReason) -> Vec<ClientEvent> {
        MockCosClient::request_resync(self, reason)
    }

    fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, ClientError> {
        MockCosClient::stop_task(self, task_id)
    }

    fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport {
        MockCosClient::pump_step(self, now_ms, budget)
    }

    fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus> {
        self.client.command_statuses()
    }

    fn lookup_command(&self, id: &CommandId) -> CommandLookup<'_> {
        MockCosClient::lookup_command(self, id)
    }

    fn retain_command(&mut self, id: &CommandId) -> Result<(), ClientError> {
        self.client.retain_command(id)
    }

    fn release_command(&mut self, id: &CommandId) {
        self.client.release_command(id);
    }

    fn begin_authorized_observation(
        &mut self,
        request: OpenAuthorizedObservation,
    ) -> Result<(), ClientError> {
        MockCosClient::begin_authorized_observation(self, request)
    }

    fn publication_access(&self) -> PublicationAccess<'_> {
        MockCosClient::publication_access(self)
    }

    fn lookup_receipt_authorized(
        &mut self,
        request: AuthorizedReceiptLookup,
    ) -> Result<(), ClientError> {
        MockCosClient::lookup_receipt_authorized(self, request)
    }

    fn bind_observation_control(&mut self, control: crate::ObservationControl) {
        MockCosClient::bind_observation_control(self, control);
    }

    fn observation_request(&self, scope: ObservationScopeRequest) -> OpenAuthorizedObservation {
        self.client.observation_request(scope)
    }

    fn adopt_retained_registry(
        &mut self,
        records: BTreeMap<CommandId, CommandStatus>,
        references: BTreeMap<CommandId, usize>,
    ) -> Result<(), ClientError> {
        self.client.adopt_retained_registry(records, references)
    }
}

impl Drop for MockTransport {
    fn drop(&mut self) {
        self.cancel_schedule(ScheduleCancelReason::Closed);
        self.trace
            .record(self.session_serial, "", "closed", &[], None, None);
    }
}

#[cfg(test)]
mod deadline_tests;
