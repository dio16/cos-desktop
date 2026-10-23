//! Scripted fixture provider behind the same client boundary.
//!
//! `MockCosClient` is an explicit fixture: it serves scripted snapshots, delta
//! transactions, receipts and errors through the same transport interface a
//! real adapter would implement. It never falls back to fixture mode
//! automatically, never reimplements Domain transitions and never fabricates
//! completion. Every identity is `fixture:`-namespaced (validated at load).

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::path::Path;

use desktop_protocol::{
    CommandEnvelope, CommandId, CommandOperation, CommandPayload, CommandReceipt, CommandTarget,
    EpochId, ErrorCode, HelloAccepted, HelloRejected, HelloRequest, Limits, ObservationScope,
    ObservationScopeRequest, OpenObservation, ProtocolError, ProtocolVersions, ReceiptLookup,
    ReplayAccepted, ResyncReason, ResyncRequired, ServerFrame, ServerMessage, ServerWire,
    SnapshotBegin, SnapshotChunk, SnapshotEnd, TaskId, Version, VersionAdvertisement,
    integrity_digest_of, negotiate_session,
};
use serde_json::Value;

use crate::client::{
    ClientConfig, ClientEvent, ClientSource, CommandStatus, ConnectionState, CosClient,
    DesktopClient, SnapshotHandle,
};
use crate::error::ClientError;
use crate::fixture::{
    CommandDefault, FixtureError, FixtureReceipt, FixtureRuntime, FixtureScenario, FixtureStep,
    ObservationMode, Trigger, parse_trigger,
};
use crate::transport::{Transport, TransportError};

/// Scripted byte transport and synthetic server.
///
/// The bounded receive queue is enforced here (it simulates the client-side
/// socket buffer): pushing beyond the negotiated frame/byte limits flags an
/// overflow that the client observes as a cancelled stream.
pub struct MockTransport {
    scenario: FixtureScenario,
    runtime: FixtureRuntime,
    limits: Limits,
    inbox: VecDeque<Vec<u8>>,
    inbox_bytes: u64,
    overflowed: bool,
    disconnect_pending: bool,
    sent: Vec<Vec<u8>>,
    selected: Option<ProtocolVersions>,
    observation_count: u32,
    after_open_steps: VecDeque<FixtureStep>,
    command_steps: BTreeMap<String, VecDeque<FixtureStep>>,
    manual_steps: BTreeMap<String, FixtureStep>,
    receipts: BTreeMap<CommandId, CommandReceipt>,
    command_digests: BTreeMap<CommandId, desktop_protocol::IntegrityDigest>,
    command_responses: BTreeMap<CommandId, Vec<Vec<u8>>>,
}

impl MockTransport {
    /// Build a transport for a validated scenario.
    pub fn new(scenario: FixtureScenario) -> Result<Self, FixtureError> {
        let mut after_open_steps = VecDeque::new();
        let mut command_steps: BTreeMap<String, VecDeque<FixtureStep>> = BTreeMap::new();
        let mut manual_steps = BTreeMap::new();
        for step in &scenario.steps {
            match parse_trigger(&step.trigger) {
                Some(Trigger::AfterOpen) => after_open_steps.push_back(step.clone()),
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
            command_steps,
            manual_steps,
            receipts: BTreeMap::new(),
            command_digests: BTreeMap::new(),
            command_responses: BTreeMap::new(),
        })
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
        self.runtime.set_epoch(epoch);
    }

    /// Raw client frames recorded by the provider.
    pub fn sent_frames(&self) -> &[Vec<u8>] {
        &self.sent
    }

    /// Number of frames currently queued for the client.
    pub fn inbox_len(&self) -> usize {
        self.inbox.len()
    }

    /// Number of observation requests served.
    pub fn observation_count(&self) -> u32 {
        self.observation_count
    }

    /// Queue a raw JSON frame verbatim (malformed/unknown/oversize tests).
    pub fn inject_raw(&mut self, raw: impl AsRef<str>) {
        self.enqueue_bytes(raw.as_ref().as_bytes().to_vec());
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
            Some("FRAME") => self.handle_frame(value),
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
            Some("OPEN_OBSERVATION") => match serde_json::from_value::<OpenObservation>(message) {
                Ok(open) => self.handle_open(open),
                Err(error) => {
                    self.enqueue_error(
                        ErrorCode::ValidationFailed,
                        format!("malformed observation request: {error}"),
                        None,
                    );
                }
            },
            Some("COMMAND") => self.handle_command_value(message),
            Some("LOOKUP_RECEIPT") => match serde_json::from_value::<ReceiptLookup>(message) {
                Ok(lookup) => self.handle_lookup(lookup),
                Err(error) => {
                    self.enqueue_error(
                        ErrorCode::ValidationFailed,
                        format!("malformed receipt lookup: {error}"),
                        None,
                    );
                }
            },
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

    fn handle_open(&mut self, open: OpenObservation) {
        if self.selected.is_none() {
            self.enqueue_error(
                ErrorCode::ValidationFailed,
                "observation requested before the handshake completed",
                None,
            );
            return;
        }
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
            protocol: Version::new(1, 0),
            schema: Version::new(1, 0),
        });
        ObservationScope {
            scope_id: self.scenario.scope.scope_id.clone(),
            task_ids: self.scenario.scope.task_ids.clone(),
            filters: self.scenario.scope.filters.clone(),
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

    fn handle_lookup(&mut self, lookup: ReceiptLookup) {
        match self.receipts.get(&lookup.command_id) {
            Some(receipt) => {
                self.enqueue_message(&ServerMessage::Receipt(receipt.clone()));
            }
            None => {
                self.enqueue_error(
                    ErrorCode::ReceiptExpired,
                    "no receipt is retained for this command",
                    Some(lookup.command_id.clone()),
                );
            }
        }
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
            protocol: Version::new(1, 0),
            schema: Version::new(1, 0),
        });
        let wire = ServerWire::Frame(ServerFrame {
            protocol: versions.protocol,
            schema: versions.schema,
            message: message.clone(),
        });
        Ok(serde_json::to_vec(&wire)?)
    }

    fn receipt_frame(&self, receipt: &CommandReceipt) -> Vec<u8> {
        self.message_frame(&ServerMessage::Receipt(receipt.clone()))
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
        let frames = self.inbox.len() as u64 + 1;
        let total_bytes = self.inbox_bytes.saturating_add(bytes.len() as u64);
        if frames > u64::from(self.limits.max_queue_frames)
            || total_bytes > self.limits.max_queue_bytes
        {
            self.overflowed = true;
            return;
        }
        self.inbox_bytes = total_bytes;
        self.inbox.push_back(bytes);
    }
}

impl Transport for MockTransport {
    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        self.sent.push(frame.clone());
        self.handle_client_frame(&frame);
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        if self.disconnect_pending {
            self.disconnect_pending = false;
            return Err(TransportError::Disconnected);
        }
        if self.overflowed {
            self.overflowed = false;
            self.inbox.clear();
            self.inbox_bytes = 0;
            return Err(TransportError::QueueOverflow);
        }
        let frame = self.inbox.pop_front();
        if let Some(frame) = &frame {
            self.inbox_bytes = self.inbox_bytes.saturating_sub(frame.len() as u64);
        }
        Ok(frame)
    }

    fn reset_stream(&mut self) {
        self.inbox.clear();
        self.inbox_bytes = 0;
        self.overflowed = false;
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
    pub fn command_status(&self, command_id: &CommandId) -> Option<&CommandStatus> {
        self.client.command_status(command_id)
    }

    /// Look up a receipt.
    pub fn lookup_receipt(&mut self, command_id: &CommandId) -> Result<(), ClientError> {
        self.client.lookup_receipt(command_id)
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

    fn command_status(&self, command_id: &CommandId) -> Option<&CommandStatus> {
        MockCosClient::command_status(self, command_id)
    }
}
