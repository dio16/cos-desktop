//! Runtime reconnect: rebuild one observation client after the session was lost.
//!
//! A Runtime restart is not a Desktop restart. The Runtime may be down longer
//! than the client's bounded recovery budget, and it comes back under a new epoch
//! (and, in tests, possibly a new origin) with a credential registry that no
//! longer knows the session's credential. The measured defect is that a terminal
//! client state had no in-process recovery: the explicit `再接続する` action only
//! re-subscribed locally.
//!
//! This module is the composition-root half of the fix (app-core owns the
//! *when*, see [`app_core::reconnect`]): one attempt re-reads the discovery
//! descriptor, re-resolves the endpoint and credential, runs the bounded
//! credential preparation and builds a completely fresh client. Nothing here
//! reads a credential from a previous attempt, and nothing renders, logs or
//! returns a token value.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use app_core::ReconnectUnavailable;
use cos_client::DesktopClient;

use crate::app::Mode;
use crate::credential_issuance;
use crate::credentials::CredentialStore;
use crate::discovery;
use crate::runtime::{self, LaunchPlan, RuntimeTeardown, RuntimeUnavailable};

/// One bounded credential exchange for a reconnect attempt.
///
/// The binary supplies the real `ureq` exchange; tests supply a scripted one, so
/// the discovery-refresh and credential-reissue sequence is testable without a
/// listening Runtime.
pub(crate) type ExchangeFactory =
    dyn Fn(&str) -> Box<dyn credential_issuance::CredentialExchange + Send> + Send + Sync;

/// Rebuilds the observation client for one reconnect attempt.
pub(crate) struct RuntimeReconnector {
    mode: Mode,
    /// The explicit endpoint override, when the process was started with one.
    endpoint_override: Option<String>,
    /// The per-user discovery descriptor path, when the process uses discovery.
    discovery_path: Option<PathBuf>,
    store: Arc<dyn CredentialStore>,
    /// The process-wide teardown slot: a reconnect replaces the credential the
    /// exit handshake must revoke.
    teardown: Arc<Mutex<Option<RuntimeTeardown>>>,
    exchange: Box<ExchangeFactory>,
}

impl RuntimeReconnector {
    pub(crate) fn new(
        mode: Mode,
        endpoint_override: Option<String>,
        discovery_path: Option<PathBuf>,
        store: Arc<dyn CredentialStore>,
        teardown: Arc<Mutex<Option<RuntimeTeardown>>>,
        exchange: Box<ExchangeFactory>,
    ) -> Self {
        Self {
            mode,
            endpoint_override,
            discovery_path,
            store,
            teardown,
            exchange,
        }
    }

    /// The production exchange: one bounded `ureq` request per credential call.
    pub(crate) fn ureq_exchange() -> Box<ExchangeFactory> {
        Box::new(|origin: &str| {
            Box::new(credential_issuance::UreqExchange::with_timeout(
                origin,
                runtime::REAUTH_BUDGET,
            )) as Box<dyn credential_issuance::CredentialExchange + Send>
        })
    }

    /// Resolve the origin for this attempt.
    ///
    /// With an explicit endpoint override the descriptor is not consulted (the
    /// operator's endpoint wins, exactly as at startup). Otherwise the per-user
    /// descriptor is re-read, so a restarted Runtime that advertises a new epoch,
    /// pid or port is picked up without a Desktop restart.
    fn rediscover(&self) -> Result<String, ReconnectUnavailable> {
        if let Some(endpoint) = self.endpoint_override.as_deref() {
            let trimmed = endpoint.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_owned());
            }
        }
        let Some(path) = self.discovery_path.as_ref() else {
            return Err(ReconnectUnavailable::Discovery);
        };
        match discovery::discover(path) {
            Ok(descriptor) => {
                // Same value-free fields the startup line reports; the descriptor
                // path, origin and credential are never logged.
                eprintln!(
                    "Runtime rediscovered: server_identity={} epoch={} pid={}",
                    descriptor.server_identity(),
                    descriptor.epoch(),
                    descriptor.pid()
                );
                Ok(descriptor.origin().to_owned())
            }
            Err(error) => {
                eprintln!("Runtime rediscovery unavailable: {}", error.label());
                Err(ReconnectUnavailable::Discovery)
            }
        }
    }

    /// One complete discovery + authorization + client-build attempt.
    pub(crate) fn reconnect(&self) -> Result<Box<dyn DesktopClient>, ReconnectUnavailable> {
        let endpoint = self.rediscover()?;
        // A fresh client is built from the same resolution the startup path uses;
        // the token value never leaves this boundary.
        let token = std::env::var(runtime::TOKEN_ENV).ok();
        let settings = match runtime::plan(
            &self.mode,
            Some(&endpoint),
            token.as_deref(),
            self.store.as_ref(),
        ) {
            LaunchPlan::Runtime(settings) => settings,
            LaunchPlan::RuntimeUnavailable { reason, .. } => {
                return Err(plan_unavailable(reason));
            }
            // The reconnector is only built for a real Runtime session; a fixture
            // plan here is a composition error and never a silent fallback.
            LaunchPlan::Fixture { .. } => return Err(ReconnectUnavailable::Discovery),
        };
        // Bounded reauthorization inside the attempt: the restarted Runtime may no
        // longer know the stored credential, and the issuance secret is re-read
        // here (never retained) so a new credential can be issued. A failure is
        // reported and then left to the client's own bounded 401 recovery.
        if let Some(origin) = runtime::validated_origin(settings.origin()) {
            let secret = std::env::var(credential_issuance::ISSUANCE_SECRET_ENV).ok();
            let mut exchange = (self.exchange)(&origin);
            match credential_issuance::prepare(
                &origin,
                token.as_deref(),
                secret.as_deref(),
                self.store.as_ref(),
                &mut *exchange,
                SystemTime::now(),
            ) {
                Ok(_) => eprintln!("Runtime credential prepared for the reconnect attempt"),
                Err(error) => eprintln!(
                    "Runtime credential preparation unavailable: {}",
                    error.label()
                ),
            }
        }
        match runtime::observation(settings, Arc::clone(&self.store)) {
            Ok(observation) => {
                if let Ok(mut slot) = self.teardown.lock() {
                    *slot = observation.teardown;
                }
                Ok(observation.client)
            }
            Err(reason) => {
                eprintln!("Runtime reconnect client unavailable: {}", reason.label());
                Err(ReconnectUnavailable::Client)
            }
        }
    }
}

impl app_core::RuntimeReconnect for RuntimeReconnector {
    fn connect(&self) -> Result<Box<dyn DesktopClient>, ReconnectUnavailable> {
        self.reconnect()
    }
}

fn plan_unavailable(reason: RuntimeUnavailable) -> ReconnectUnavailable {
    match reason {
        RuntimeUnavailable::CredentialUnavailable => ReconnectUnavailable::Authorization,
        RuntimeUnavailable::NotConfigured
        | RuntimeUnavailable::DiscoveryUnavailable
        | RuntimeUnavailable::UnsupportedEndpoint => ReconnectUnavailable::Discovery,
    }
}

#[cfg(test)]
mod tests {
    //! Reconnect and reset evidence at the real client seam.
    //!
    //! The Runtime is a scripted protocol provider: it can be down, come back under
    //! a new epoch, refuse a resumed grant with `403` and refuse the dead session's
    //! credential with `401`. Nothing here is a fixture publication: the snapshot
    //! the Desktop publishes is parsed from the scripted Runtime's own documents.

    use std::sync::{Arc, Mutex};

    use app_core::ObservationHandle;
    use cos_client::{CosClient, CredentialSlot, ProtocolTransport, ProtocolTransportConfig};
    use desktop_protocol::{
        Counter, Cursor, EpochId, HelloAccepted, Limits, ObservationScope, ObservationScopeRequest,
        Projection, ProjectionMutation, ProtocolVersions, ReadContextId, ReadGrant, ReadRequestId,
        ReadRevision, ReadStamp, ScopeFilter, ScopeId, ServerId, SnapshotBegin, SnapshotChunk,
        SnapshotCounts, SnapshotEnd, SnapshotId, TaskId, TaskSnapshot, TaskStatus, Version,
        projection_bundle_digest,
    };
    use serde_json::json;

    use super::*;
    use crate::credential_issuance::{
        CredentialExchange, ExchangeFailure, HttpResponseBytes, ISSUE_PATH, REFRESH_PATH,
    };
    use crate::credentials::{CredentialStore, CredentialStoreError};
    use cos_client::{DELTAS_PATH, HttpMethod, OBSERVATION_PATH, ProtocolHttp, SERVER_PATH};

    const STALE: &str = "stale-credential-0123456789";
    const FRESH: &str = "fresh-credential-0123456789";
    const OLD_EPOCH: &str = "epoch-before-restart";
    const NEW_EPOCH: &str = "epoch-after-restart";
    const OLD_TITLE: &str = "Before the Runtime restart";
    const NEW_TITLE: &str = "After the Runtime restart";

    struct RuntimeState {
        up: bool,
        epoch: String,
        /// The only credential the current Runtime process accepts.
        token: String,
        title: String,
        /// `route status` in arrival order, so the reauthorization sequence is
        /// asserted from the Runtime's own view.
        served: Vec<String>,
        issues: u32,
    }

    struct ScriptedRuntime {
        state: Arc<Mutex<RuntimeState>>,
        /// The origin is fixed by the test; the body is what decides the answer.
        _origin: String,
    }

    fn hello_document(epoch: &str) -> serde_json::Value {
        json!({
            "schema_version": 1,
            "server": serde_json::to_value(HelloAccepted {
                protocol: Version::new(2, 0),
                schema: Version::new(2, 0),
                server_identity: ServerId::new("cos-runtime-orchestrator").expect("id"),
                server_build: "orchestrator-runtime/test".to_owned(),
                epoch: EpochId::new(epoch).expect("epoch"),
                dedupe_retention_ms: None,
                limits: None,
            })
            .expect("hello"),
        })
    }

    fn open_document(body: &serde_json::Value, epoch: &str, title: &str) -> serde_json::Value {
        let request_id = body["request_id"].as_str().expect("request id");
        let protocol: Version = serde_json::from_value(body["protocol"].clone()).expect("protocol");
        let schema: Version = serde_json::from_value(body["schema"].clone()).expect("schema");
        let scope_request: ObservationScopeRequest =
            serde_json::from_value(body["scope"].clone()).expect("scope");
        let scope = ObservationScope {
            scope_id: ScopeId::new("scope-1").expect("scope id"),
            task_ids: scope_request.task_ids,
            filters: scope_request.filters,
            protocol,
            schema,
        };
        let stamp = ReadStamp {
            request_id: ReadRequestId::new(request_id).expect("request id"),
            server_id: ServerId::new("cos-runtime-orchestrator").expect("server id"),
            epoch: EpochId::new(epoch).expect("epoch"),
            context_id: ReadContextId::new("context-1").expect("context"),
            read_revision: ReadRevision::new(Counter::new("1").expect("counter")),
            scope_id: scope.scope_id.clone(),
        };
        let mutations = vec![ProjectionMutation::Upsert {
            base_revision: None,
            projection: Projection::Task(TaskSnapshot {
                task_ref: TaskId::new("task/Task-184").expect("task"),
                display_id: Some("Task-184".to_owned()),
                title: title.to_owned(),
                task_status: TaskStatus::Active,
                inner_phase: None,
                iteration_ref: None,
                iteration_index: None,
                best_candidate_ref: None,
                formal_outcome_ref: None,
                accepted_brief_ref: None,
                capabilities_ref: None,
                projection_revision: Counter::new("1").expect("revision"),
            }),
        }];
        let snapshot_id = SnapshotId::new("snapshot-1").expect("snapshot");
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
            epoch: EpochId::new(epoch).expect("epoch"),
            // A cursor is bound to the epoch that issued it, exactly like the
            // Runtime's own journal cursor.
            cursor: Cursor::new(format!("cursor-{epoch}")).expect("cursor"),
            expected: SnapshotCounts::of(&mutations),
            integrity_digest: projection_bundle_digest(&mutations).expect("digest"),
        };
        let grant = ReadGrant {
            stamp,
            scope,
            local_intents_readable: false,
            command_reads: Vec::new(),
        };
        json!({
            "schema_version": 1,
            "server": hello_document(epoch)["server"].clone(),
            "grant": serde_json::to_value(grant).expect("grant"),
            "snapshot": {
                "begin": serde_json::to_value(begin).expect("begin"),
                "chunks": chunks
                    .iter()
                    .map(|chunk| serde_json::to_value(chunk).expect("chunk"))
                    .collect::<Vec<_>>(),
                "end": serde_json::to_value(end).expect("end"),
            },
        })
    }

    fn presented_token(request: &cos_client::HttpRequest) -> Option<String> {
        request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .and_then(|(_, value)| value.strip_prefix("Bearer "))
            .map(str::to_owned)
    }

    /// A `403` that states why replay is impossible: the client must not treat it
    /// as a terminal authorization failure, only as a required fresh snapshot.
    fn resync_refusal(reason: &str) -> Vec<u8> {
        json!({
            "code": "RESYNC_REQUIRED",
            "reason": reason,
            "detail": "the epoch changed and this cursor belongs to the previous Runtime",
        })
        .to_string()
        .into_bytes()
    }

    impl ProtocolHttp for ScriptedRuntime {
        fn execute(
            &mut self,
            request: cos_client::HttpRequest,
        ) -> Result<cos_client::HttpResponse, String> {
            let route = request
                .path
                .split('?')
                .next()
                .unwrap_or(request.path.as_str())
                .to_owned();
            let mut state = self.state.lock().expect("state");
            if !state.up {
                state.served.push(format!("{route} down"));
                return Err("the Runtime connection was refused".to_owned());
            }
            let token = presented_token(&request);
            let token_accepted = token.as_deref() == Some(state.token.as_str());
            let answer = match (request.method, route.as_str()) {
                (HttpMethod::Get, SERVER_PATH) => (
                    200,
                    serde_json::to_vec(&hello_document(&state.epoch)).expect("hello"),
                    String::new(),
                ),
                (HttpMethod::Post, OBSERVATION_PATH) => {
                    let body: serde_json::Value =
                        serde_json::from_slice(&request.body).unwrap_or(json!({}));
                    let resumed = body["last_seen_cursor"].is_string()
                        || body["scope_id"].is_string()
                        || body["epoch"].is_string();
                    let marker = format!(" resume={resumed}");
                    if !token_accepted {
                        (401, br#"{"code":"UNAUTHORIZED"}"#.to_vec(), marker)
                    } else if resumed {
                        // A resumed grant from the previous epoch is refused exactly
                        // as the measured Runtime refused it (`403`), and the reason
                        // forbids the client from replaying the dead cursor.
                        (403, resync_refusal("EPOCH_CHANGED"), marker)
                    } else {
                        let document =
                            open_document(&body, &state.epoch.clone(), &state.title.clone());
                        (200, serde_json::to_vec(&document).expect("open"), marker)
                    }
                }
                (HttpMethod::Get, DELTAS_PATH) => {
                    let cursor = request
                        .path
                        .split("cursor=")
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned();
                    if !token_accepted {
                        (401, br#"{"code":"UNAUTHORIZED"}"#.to_vec(), String::new())
                    } else if !cursor.contains(&state.epoch) {
                        (403, resync_refusal("EPOCH_CHANGED"), String::new())
                    } else {
                        (
                            200,
                            json!({"schema_version": 1, "transaction": null})
                                .to_string()
                                .into_bytes(),
                            String::new(),
                        )
                    }
                }
                _ => (404, b"{}".to_vec(), String::new()),
            };
            state
                .served
                .push(format!("{route} {}{}", answer.0, answer.2));
            Ok(cos_client::HttpResponse {
                status: answer.0,
                body: answer.1,
            })
        }
    }

    struct ScriptedExchange {
        state: Arc<Mutex<RuntimeState>>,
    }

    impl CredentialExchange for ScriptedExchange {
        fn post(
            &mut self,
            path: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<HttpResponseBytes, ExchangeFailure> {
            match path {
                ISSUE_PATH => {
                    let mut state = self.state.lock().expect("state");
                    state.issues += 1;
                    Ok(HttpResponseBytes {
                        status: 200,
                        body: credential_document(FRESH),
                    })
                }
                REFRESH_PATH => Ok(HttpResponseBytes {
                    status: 401,
                    body: Vec::new(),
                }),
                _ => Ok(HttpResponseBytes {
                    status: 404,
                    body: Vec::new(),
                }),
            }
        }
    }

    struct MemoryStore {
        state: Mutex<Option<String>>,
    }

    impl CredentialStore for MemoryStore {
        fn load(&self, _origin: &str) -> Result<Option<String>, CredentialStoreError> {
            Ok(self.state.lock().expect("store").clone())
        }
        fn store(&self, _origin: &str, token: &str) -> Result<(), CredentialStoreError> {
            *self.state.lock().expect("store") = Some(token.to_owned());
            Ok(())
        }
        fn delete(&self, _origin: &str) -> Result<(), CredentialStoreError> {
            *self.state.lock().expect("store") = None;
            Ok(())
        }
    }

    fn credential_document(token: &str) -> Vec<u8> {
        format!(
            r#"{{"token":"{token}","principal":"desktop:0123456789abcdef0123456789abcdef","scopes":["read:observe","desktop:command"],"expires_in":43200}}"#
        )
        .into_bytes()
    }

    /// The composition root's client build, with the protocol provider scripted.
    fn build_client(
        state: &Arc<Mutex<RuntimeState>>,
        store: &Arc<dyn CredentialStore>,
        token: &str,
        max_resync_attempts: u32,
    ) -> Box<dyn cos_client::DesktopClient> {
        let slot: CredentialSlot = Arc::new(Mutex::new(Some(token.to_owned())));
        let revoke: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let executor = crate::runtime::ReauthHttp::new(
            ScriptedRuntime {
                state: Arc::clone(state),
                _origin: String::new(),
            },
            Arc::clone(store),
            "http://127.0.0.1:8134".to_owned(),
            Arc::clone(&slot),
            revoke,
            Arc::new(|| Some("operator-secret".to_owned())),
            ScriptedExchange {
                state: Arc::clone(state),
            },
        );
        let transport = ProtocolTransport::new(
            executor,
            ProtocolTransportConfig {
                credential_slot: Some(Arc::clone(&slot)),
                // The transport polls a real worker thread; a short interval keeps
                // the test's wall-clock bounded without changing any behavior.
                delta_poll_interval: std::time::Duration::from_millis(10),
                ..ProtocolTransportConfig::default()
            },
            ProtocolVersions {
                protocol: Version::new(2, 0),
                schema: Version::new(2, 0),
            },
        );
        let config = cos_client::ClientConfig {
            max_resync_attempts,
            ..cos_client::ClientConfig::default()
        };
        Box::new(CosClient::new(
            transport,
            config,
            Limits::DEFAULT,
            cos_client::ClientSource::Unknown,
        ))
    }

    fn scope() -> ObservationScopeRequest {
        ObservationScopeRequest {
            task_ids: Vec::new(),
            filters: vec![ScopeFilter {
                name: "view".to_owned(),
                value: "all".to_owned(),
            }],
        }
    }

    /// Drive the owner until `done` or the bounded number of steps elapses.
    ///
    /// The transport polls from its own worker thread, so the loop yields real time
    /// as well as advancing the owner's clock.
    fn drive_until(
        owner: &mut app_core::ObservationSessionOwner,
        mut now: u64,
        done: impl Fn(&ObservationHandle) -> bool,
        handle: &ObservationHandle,
    ) -> u64 {
        for _ in 0..20_000 {
            if done(handle) {
                return now;
            }
            now += 20;
            owner.step(now);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!(
            "the bounded driver never reached the expected state: {:?}",
            published(handle).map(|publication| {
                (
                    publication.connection,
                    publication.snapshot.is_some(),
                    format!("{:?}", publication.last_error),
                    handle
                        .current()
                        .ok()
                        .and_then(|lease| lease.read_context())
                        .map(|context| context.epoch().as_str().to_owned()),
                    publication.snapshot.as_ref().map(|snapshot| {
                        snapshot
                            .tasks
                            .values()
                            .map(|task| task.title.clone())
                            .collect::<Vec<_>>()
                    }),
                )
            })
        );
    }

    fn published(handle: &ObservationHandle) -> Option<app_core::ObservationPublication> {
        let lease = handle.current().ok()?;
        let mut publication = None;
        lease
            .read(|current| publication = Some(current.clone()))
            .ok()?;
        publication
    }

    fn is_observing(title: &str, epoch: &str) -> impl Fn(&ObservationHandle) -> bool + 'static {
        let title = title.to_owned();
        let epoch = epoch.to_owned();
        move |handle| {
            let Some(publication) = published(handle) else {
                return false;
            };
            let has_title = publication
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.tasks.values().any(|task| task.title == title));
            let right_epoch = handle
                .current()
                .ok()
                .and_then(|lease| lease.read_context())
                .is_some_and(|context| context.epoch().as_str() == epoch);
            publication.connection == app_core::PublicationConnection::Observing
                && has_title
                && right_epoch
        }
    }

    fn start_live(
        state: Arc<Mutex<RuntimeState>>,
    ) -> (app_core::ObservationSessionOwner, ObservationHandle) {
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore {
            state: Mutex::new(Some(STALE.to_owned())),
        });
        let client = build_client(&state, &store, STALE, 2);
        let (mut owner, handle) = app_core::ObservationSessionOwner::new(client);
        owner.arm_control(&handle);
        let reply = handle.change_scope(scope());
        let mut now = 0u64;
        let mut reply = Box::pin(reply);
        let waker = std::task::Waker::noop().clone();
        let mut context = std::task::Context::from_waker(&waker);
        loop {
            if let std::task::Poll::Ready(value) = reply.as_mut().poll(&mut context) {
                value.expect("observation starts");
                break;
            }
            now += 20;
            owner.step(now);
        }
        (owner, handle)
    }

    #[test]
    fn an_outage_past_the_bounded_budget_is_recovered_by_a_reconnect_install() {
        let state = Arc::new(Mutex::new(RuntimeState {
            up: true,
            epoch: OLD_EPOCH.to_owned(),
            token: STALE.to_owned(),
            title: OLD_TITLE.to_owned(),
            served: Vec::new(),
            issues: 0,
        }));
        let (mut owner, handle) = start_live(Arc::clone(&state));
        let now = drive_until(&mut owner, 0, is_observing(OLD_TITLE, OLD_EPOCH), &handle);
        assert!(
            handle.observation_available(),
            "the scripted Runtime session observes"
        );

        // The Runtime goes away for longer than the client's bounded recovery.
        {
            let mut state = state.lock().expect("state");
            state.up = false;
        }
        let now = drive_until(
            &mut owner,
            now,
            |handle| {
                published(handle).is_some_and(|publication| {
                    publication.connection != app_core::PublicationConnection::Observing
                        && publication.snapshot.is_none()
                })
            },
            &handle,
        );
        // Keep stepping until the bounded budget is spent and the terminal state is
        // published; the terminal state is what the explicit action had to reset.
        let now = drive_until(
            &mut owner,
            now,
            |handle| {
                published(handle).is_some_and(|publication| {
                    publication.connection == app_core::PublicationConnection::Incompatible
                })
            },
            &handle,
        );
        assert!(
            !handle.observation_available(),
            "an exhausted recovery is not an observing publication"
        );
        assert!(
            published(&handle).is_some_and(|publication| publication.last_error.is_some()),
            "the terminal state carries the recovery failure"
        );

        // The Runtime comes back under a new epoch with a credential registry that
        // no longer knows this Desktop; the stored credential is still the old one.
        {
            let mut state = state.lock().expect("state");
            state.up = true;
            state.epoch = NEW_EPOCH.to_owned();
            state.token = FRESH.to_owned();
            state.title = NEW_TITLE.to_owned();
            state.served.clear();
        }
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore {
            state: Mutex::new(Some(STALE.to_owned())),
        });
        let fresh = build_client(&state, &store, STALE, 2);
        let _reply = handle.replace_client(fresh);
        let _now = drive_until(&mut owner, now, is_observing(NEW_TITLE, NEW_EPOCH), &handle);

        {
            let state = state.lock().expect("state");
            assert_eq!(
                state.issues, 1,
                "one bounded reissue from the operator secret"
            );
            assert!(
                state.served.starts_with(&[
                    "/api/cos/desktop/server 200".to_owned(),
                    "/api/cos/desktop/observation 401 resume=false".to_owned(),
                    "/api/cos/desktop/observation 200 resume=false".to_owned(),
                ]),
                "the reconnected client is refused with 401, reissues and opens a fresh observation: {:?}",
                state.served
            );
            assert!(
                state
                    .served
                    .iter()
                    .all(|entry| !entry.starts_with("/api/cos/desktop/snapshot")),
                "no stale cursor or snapshot replay is attempted: {:?}",
                state.served
            );
        }
        assert!(
            handle.observation_available(),
            "the install reopens the observation without a process restart"
        );
    }

    #[test]
    fn an_epoch_change_refused_with_403_recovers_through_a_fresh_observation() {
        let state = Arc::new(Mutex::new(RuntimeState {
            up: true,
            epoch: OLD_EPOCH.to_owned(),
            token: STALE.to_owned(),
            title: OLD_TITLE.to_owned(),
            served: Vec::new(),
            issues: 0,
        }));
        let (mut owner, handle) = start_live(Arc::clone(&state));
        let now = drive_until(&mut owner, 0, is_observing(OLD_TITLE, OLD_EPOCH), &handle);

        // The Runtime restarts under a new epoch: the live session's credential and
        // its cursor both belong to the previous Runtime process.
        {
            let mut state = state.lock().expect("state");
            state.epoch = NEW_EPOCH.to_owned();
            state.token = FRESH.to_owned();
            state.title = NEW_TITLE.to_owned();
            state.served.clear();
        }
        let _now = drive_until(&mut owner, now, is_observing(NEW_TITLE, NEW_EPOCH), &handle);

        {
            let state = state.lock().expect("state");
            assert_eq!(
                state.issues, 1,
                "one bounded reissue from the operator secret, not an unbounded loop"
            );
            assert!(
                state
                    .served
                    .iter()
                    .any(|entry| entry == "/api/cos/desktop/deltas 401"),
                "the dead session's first poll is refused with 401: {:?}",
                state.served
            );
            assert!(
                state
                    .served
                    .iter()
                    .any(|entry| entry == "/api/cos/desktop/deltas 403"),
                "the reissued credential still names the previous epoch's cursor, so replay is refused with 403: {:?}",
                state.served
            );
            assert!(
                state
                    .served
                    .iter()
                    .any(|entry| entry == "/api/cos/desktop/observation 200 resume=false"),
                "the recovery opens a fresh observation instead of replaying: {:?}",
                state.served
            );
            assert!(
                !state
                    .served
                    .iter()
                    .any(|entry| entry == "/api/cos/desktop/observation 200 resume=true"),
                "a refused resume is never repeated as a successful resume: {:?}",
                state.served
            );
        }
        assert!(
            handle.observation_available(),
            "the epoch change is recovered without a process restart"
        );
    }

    #[test]
    fn a_restarted_runtime_at_the_same_discovery_path_is_rediscovered() {
        // The descriptor is the only thing that moves when the Runtime restarts;
        // a reconnect attempt must re-read it instead of reusing the startup origin.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/reconnect-discovery-tests")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&root).expect("create test directory");
        let path = root.join("runtime-discovery.json");
        let descriptor = |origin: &str, epoch: &str, pid: u32| {
            format!(
                r#"{{"schema_version":1,"origin":"{origin}","server_identity":"cos-runtime-orchestrator","epoch":"{epoch}","protocol":{{"major":2,"min_minor":0,"max_minor":0}},"schema":{{"major":2,"min_minor":0,"max_minor":0}},"pid":{pid},"started_at":"2026-11-10T05:00:00+09:00","ready":true}}"#
            )
        };
        std::fs::write(&path, descriptor("http://127.0.0.1:8134", OLD_EPOCH, 111)).expect("write");
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore {
            state: Mutex::new(Some(STALE.to_owned())),
        });
        let exchange: Box<super::ExchangeFactory> = Box::new(|origin: &str| {
            Box::new(FixedExchange {
                _origin: origin.to_owned(),
            }) as Box<dyn crate::credential_issuance::CredentialExchange + Send>
        });
        let reconnector = RuntimeReconnector::new(
            crate::app::Mode::Runtime {
                endpoint_configured: true,
            },
            None,
            Some(path.clone()),
            store,
            Arc::new(Mutex::new(None)),
            exchange,
        );
        assert_eq!(
            reconnector.rediscover(),
            Ok("http://127.0.0.1:8134".to_owned()),
            "the startup descriptor is used"
        );

        // The Runtime restarts: same path, new epoch and a different port.
        std::fs::write(&path, descriptor("http://127.0.0.1:8145", NEW_EPOCH, 222)).expect("write");
        assert_eq!(
            reconnector.rediscover(),
            Ok("http://127.0.0.1:8145".to_owned()),
            "a reconnect re-reads the descriptor instead of reusing the dead origin"
        );

        std::fs::remove_file(&path).expect("remove descriptor");
        assert_eq!(
            reconnector.rediscover(),
            Err(ReconnectUnavailable::Discovery),
            "an absent descriptor is an explicit discovery failure, never a stale origin"
        );
    }

    /// Unused credential exchange placeholder for the discovery-only test.
    struct FixedExchange {
        _origin: String,
    }

    impl CredentialExchange for FixedExchange {
        fn post(
            &mut self,
            _path: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<HttpResponseBytes, ExchangeFailure> {
            Ok(HttpResponseBytes {
                status: 404,
                body: Vec::new(),
            })
        }
    }
}
