//! Runtime-mode observation wiring: the plain-HTTP protocol executor and the
//! startup branch that decides whether a real observation client exists.
//!
//! The UI never touches `cos-client`: this module builds the client, hands it to
//! `window::ObservationHost`, and attaches the resulting `ObservationHandle` to
//! `AppCore`. Any missing configuration or construction failure is an explicit
//! unavailable state; there is no fixture fallback in runtime mode.
//!
//! # Credential acquisition and rotation
//!
//! `COS_DESKTOP_RUNTIME_ISSUANCE_SECRET` is the operator-provisioned issuance
//! secret (never logged or persisted). At the next session boundary the startup
//! path issues a credential when the store is empty, or renews a stored one that
//! is inside its rotation margin, through the Runtime credential routes; the
//! fetched token is only written to the protected store and then dropped. The
//! immutable per-session transport is built after that decision, so a session
//! that outlives its credential still ends in the `401` → delete → re-authorize
//! path. An explicit teardown revokes the session credential best-effort inside
//! a bounded budget after the app loop has stopped.
//!
//! # Transport boundary
//!
//! `COS_DESKTOP_RUNTIME_ENDPOINT` names a plain-HTTP **loopback IP-literal**
//! origin (for example `http://127.0.0.1:8123`; hostnames such as `localhost`
//! are refused because a resolver answer can change between validation and the
//! connection). The Desktop credential is resolved as **environment override >
//! protected store > explicit unavailable** (MP-SEC1): `COS_DESKTOP_RUNTIME_TOKEN`
//! is the operator override, the Windows Credential Manager entry for the origin
//! is the stored copy, and an operator-supplied token is written back to the
//! store (replacing a rotated value). The credential is presented by the
//! protocol transport on the three protocol routes (observation open, snapshot
//! read, delta read); the executor never holds or attaches it itself. The token
//! is read once, moved into the transport, and never logged, rendered, put in a
//! diagnostic or echoed in an error message. A `401` from the Runtime deletes
//! the stored entry and requires re-authorization. Redirects and environment
//! proxies are disabled so a credential can never be replayed to another origin.
//! TLS is not implemented: `ureq` is built with
//! `default-features = false` and non-HTTP or credential bearing endpoints are
//! refused at startup.

use std::io::Read;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cos_client::media::MediaFetcher;
use cos_client::{
    ClientConfig, ClientSource, CosClient, CredentialSlot, DesktopClient, HttpMethod, HttpRequest,
    HttpResponse, ProtocolHttp, ProtocolTransport, ProtocolTransportConfig,
};
use desktop_gpui::media_presentation::{MediaBinding, MediaRuntime};
use desktop_protocol::{Limits, ObservationScopeRequest, ProtocolVersions, ScopeFilter, Version};

use crate::app::Mode;
use crate::credential_issuance::CredentialExchange;
use crate::credentials::CredentialStore;
use crate::media_executor::MediaHttp;

/// Environment variable naming the Runtime origin for runtime mode.
pub(crate) const ENDPOINT_ENV: &str = "COS_DESKTOP_RUNTIME_ENDPOINT";
/// Environment variable carrying the optional bearer token. Never logged.
pub(crate) const TOKEN_ENV: &str = "COS_DESKTOP_RUNTIME_TOKEN";

/// Whole-request budget for one protocol exchange.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Whole-request budget for one credential reauthorization exchange.
const REAUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// The bounded credential-exchange budget a reconnect attempt uses.
pub(crate) const REAUTH_BUDGET: Duration = REAUTH_TIMEOUT;
/// Bounded accepted response body (staged bundle ceiling plus one control frame).
const MAX_RESPONSE_BYTES: u64 = Limits::DEFAULT
    .max_bundle_bytes
    .saturating_add(Limits::DEFAULT.max_frame_bytes);
/// Protocol pair this build negotiates with the Runtime adapter.
const PROTOCOL: Version = Version::new(2, 0);
/// Wire schema pair this build negotiates with the Runtime adapter.
const SCHEMA: Version = Version::new(2, 0);

/// Why an explicitly configured runtime observation cannot start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeUnavailable {
    /// No endpoint is configured and no usable per-user discovery descriptor
    /// exists.
    NotConfigured,
    /// The per-user discovery descriptor is absent, invalid, stale or names
    /// something this build refuses. Never a fixture fallback.
    DiscoveryUnavailable,
    /// The configured endpoint is not a plain-HTTP origin this build may use.
    UnsupportedEndpoint,
    /// No bearer token is configured for the authenticated observation request.
    /// Neither the environment override nor the protected store supplies a
    /// credential; no unauthenticated attempt is made.
    CredentialUnavailable,
}

impl RuntimeUnavailable {
    /// Safe operator-facing label; never echoes an endpoint or token value.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NotConfigured => "runtime endpoint is not configured",
            Self::DiscoveryUnavailable => {
                "runtime discovery descriptor is unavailable or not usable"
            }
            Self::UnsupportedEndpoint => {
                "runtime endpoint is not a plain-HTTP origin this build supports"
            }
            Self::CredentialUnavailable => {
                "runtime credential is unavailable (set the override or provision the store)"
            }
        }
    }
}

/// Resolved runtime connection settings.
///
/// Deliberately not `Debug`/`Display`: the token must never be formatted into a
/// log line, diagnostic report or panic message.
pub(crate) struct RuntimeSettings {
    endpoint: String,
    token: String,
    /// Only a credential this app issued or renewed may be revoked on teardown.
    revocable: bool,
}

impl RuntimeSettings {
    /// The validated origin this attempt talks to (never a token value).
    pub(crate) fn origin(&self) -> &str {
        &self.endpoint
    }
}

/// One started observation: the client the host drives and the scope to open.
pub(crate) struct RuntimeObservation {
    pub(crate) client: Box<dyn DesktopClient>,
    pub(crate) scope: ObservationScopeRequest,
    /// Present when this session owns a credential that must be revoked on exit.
    pub(crate) teardown: Option<RuntimeTeardown>,
}

/// Explicit-teardown revocation handle for one started Runtime observation.
///
/// It holds the one in-memory copy of the credential this session must revoke
/// (empty until this app has issued, refreshed or recovered a versioned
/// credential) so the process can revoke it after the app loop has stopped. An
/// operator-provisioned value is never placed here. The budget is bounded
/// ([`credential_issuance::REVOKE_TIMEOUT`]) and every failure is non-fatal and
/// value-free. Deliberately not `Debug`/`Display`: the token is never rendered.
pub(crate) struct RuntimeTeardown {
    origin: String,
    token: Arc<Mutex<Option<String>>>,
    store: Arc<dyn CredentialStore>,
}

impl RuntimeTeardown {
    /// Revoke the session credential and forget the stored copy.
    ///
    /// Called only after the app loop returned, so it delays process exit by at
    /// most one bounded request. With nothing armed (an operator override that
    /// was never replaced) this performs no exchange at all. A failed revoke
    /// keeps the stored credential for a later retry and prints a fixed label.
    pub(crate) fn revoke(self) {
        let token = match self.token.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => None,
        };
        let Some(token) = token else {
            return;
        };
        let mut exchange = crate::credential_issuance::UreqExchange::with_timeout(
            &self.origin,
            crate::credential_issuance::REVOKE_TIMEOUT,
        );
        match crate::credential_issuance::revoke(&token, &mut exchange) {
            Ok(()) => match self.store.delete(&self.origin) {
                Ok(()) => eprintln!("Runtime credential revoked on explicit teardown"),
                Err(_) => eprintln!(
                    "Runtime credential revoked on explicit teardown; the stored entry could not be removed"
                ),
            },
            Err(error) => eprintln!(
                "Runtime credential revoke on teardown failed: {}",
                error.label()
            ),
        }
    }

    #[cfg(test)]
    fn armed(&self) -> bool {
        self.token.lock().is_ok_and(|slot| slot.is_some())
    }
}

/// What the composition root should launch.
pub(crate) enum LaunchPlan {
    /// Load the explicit fixture provider.
    Fixture { path: std::path::PathBuf },
    /// Start a real Runtime observation with these settings.
    Runtime(RuntimeSettings),
    /// No observation exists; show the explicit disconnected surface.
    RuntimeUnavailable {
        endpoint_configured: bool,
        reason: RuntimeUnavailable,
    },
}

/// Decide the launch plan for one startup mode without side effects.
///
/// A runtime mode can only produce a runtime (ready or unavailable) plan; it can
/// never produce a fixture plan, which is what keeps fixture data from silently
/// standing in for a real connection.
pub(crate) fn plan(
    mode: &Mode,
    endpoint: Option<&str>,
    token: Option<&str>,
    store: &dyn CredentialStore,
) -> LaunchPlan {
    match mode {
        Mode::Fixture { path } => LaunchPlan::Fixture { path: path.clone() },
        Mode::Runtime {
            endpoint_configured,
        } => match settings(endpoint, token, store) {
            Ok(settings) => LaunchPlan::Runtime(settings),
            Err(reason) => LaunchPlan::RuntimeUnavailable {
                endpoint_configured: *endpoint_configured || is_configured(endpoint),
                reason,
            },
        },
    }
}

/// Resolve the connection settings for terminal runtime configuration.
///
/// Precedence: the environment override wins, then the protected store; with
/// neither, the plan is explicitly unavailable and no unauthenticated request
/// is ever attempted. An override that is present is also written to the store
/// (replacing a rotated value), so later launches need no environment variable.
pub(crate) fn settings(
    endpoint: Option<&str>,
    env_token: Option<&str>,
    store: &dyn CredentialStore,
) -> Result<RuntimeSettings, RuntimeUnavailable> {
    let endpoint = endpoint.map(str::trim).filter(|value| !value.is_empty());
    let Some(endpoint) = endpoint else {
        return Err(RuntimeUnavailable::NotConfigured);
    };
    let Some(origin) = plain_http_origin(endpoint) else {
        return Err(RuntimeUnavailable::UnsupportedEndpoint);
    };
    let env_token = env_token.map(str::trim).filter(|value| !value.is_empty());
    let (token, revocable) = match env_token {
        Some(token) => {
            // Operator override: the credential is persisted through the
            // protected store so the next launch can run without the variable.
            // A store failure is not fatal for this session and is logged with
            // a value-free label only. The operator's value is not revoked by
            // this app on exit.
            if store.store(&origin, token).is_err() {
                eprintln!(
                    "Runtime credential store unavailable; the environment credential is used for this session only"
                );
            }
            (token.to_owned(), false)
        }
        None => match store.load(&origin) {
            Ok(Some(value)) => match crate::credential_issuance::stored_token(&value) {
                Some(token) if !token.is_empty() => {
                    let revocable = crate::credential_issuance::stored_is_versioned(&value);
                    (token, revocable)
                }
                _ => return Err(RuntimeUnavailable::CredentialUnavailable),
            },
            Ok(_) => return Err(RuntimeUnavailable::CredentialUnavailable),
            Err(error) => {
                eprintln!("Runtime credential lookup failed: {}", error.label());
                return Err(RuntimeUnavailable::CredentialUnavailable);
            }
        },
    };
    Ok(RuntimeSettings {
        endpoint: origin,
        token,
        revocable,
    })
}

fn is_configured(endpoint: Option<&str>) -> bool {
    endpoint
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

/// The validated plain-HTTP loopback origin for a configured endpoint.
///
/// The composition root uses this to decide whether a credential acquisition or
/// rotation attempt may talk to the endpoint at all; an unsupported endpoint is
/// reported by [`plan`] and never contacted.
pub(crate) fn validated_origin(endpoint: &str) -> Option<String> {
    plain_http_origin(endpoint)
}

/// Validate a plain-HTTP **loopback origin**: `http://address[:port]` with no
/// userinfo, no path, no query and no fragment.
///
/// Only a loopback **IP literal** is accepted: an IPv4 address in
/// `127.0.0.0/8`, or the bracketed IPv6 `[::1]`. Hostnames — `localhost`
/// included — are refused rather than resolved: a resolver answer can differ
/// between validation and the later connection (TOCTOU), and the bearer token
/// plus the `X-Cos-Context` capability must never leave the host on a
/// cleartext channel to an address that was not the one validated.
///
/// Returns the normalized origin (no trailing slash).
fn plain_http_origin(value: &str) -> Option<String> {
    let rest = value.strip_prefix("http://")?;
    if rest.is_empty() {
        return None;
    }
    // Embedded credentials are not supported: they would have to be rendered or
    // stored, and this build has no credential model beyond the bearer token.
    // Control characters, whitespace and backslashes could smuggle a second
    // request line or header.
    if value.chars().any(|character| {
        character.is_control() || character.is_whitespace() || character == '\\' || character == '@'
    }) {
        return None;
    }
    // An origin is authority only: an empty path or a single trailing slash.
    let authority = match rest.split_once('/') {
        Some((authority, "")) => authority,
        Some(_) => return None,
        None => rest,
    };
    if authority.is_empty() || authority.contains('?') || authority.contains('#') {
        return None;
    }
    if !loopback_authority(authority) {
        return None;
    }
    Some(format!("http://{authority}"))
}

/// The host form of an authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthorityHost<'a> {
    /// A bracketed IPv6 literal (`[::1]`); brackets are never valid for IPv4.
    Bracketed(&'a str),
    /// A plain IPv4 literal.
    Plain(&'a str),
}

/// Whether an authority names an explicit loopback IP literal.
///
/// No name resolution happens here or at connect time: the validated address is
/// the address `ureq` dials, so validation cannot race a second resolution.
fn loopback_authority(authority: &str) -> bool {
    let Some((host, _port)) = split_authority(authority) else {
        return false;
    };
    match host {
        // Brackets are IPv6-only URL syntax: `[127.0.0.1]` is not a valid URL.
        AuthorityHost::Bracketed(address) => address
            .parse::<Ipv6Addr>()
            .is_ok_and(|address| address.is_loopback()),
        // A plain authority accepts only an IPv4 loopback literal; hostnames are
        // refused because their resolution can change between validation and use.
        AuthorityHost::Plain(name) => name
            .parse::<Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback()),
    }
}

/// Split a host/port authority; `None` for a malformed or zero port, a stray
/// colon, or an unbracketed IPv6 literal.
fn split_authority(authority: &str) -> Option<(AuthorityHost<'_>, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (address, tail) = rest.split_once(']')?;
        let port = match tail {
            "" => None,
            _ => Some(non_zero_port(tail.strip_prefix(':')?)?),
        };
        return Some((AuthorityHost::Bracketed(address), port));
    }
    // An unbracketed IPv6 literal would carry several colons; an authority is
    // `host[:port]` only.
    if authority.matches(':').count() > 1 {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((AuthorityHost::Plain(host), Some(non_zero_port(port)?))),
        None => Some((AuthorityHost::Plain(authority), None)),
    }
}

/// A usable TCP port: ASCII decimal digits only, `1..=65535`.
fn non_zero_port(raw: &str) -> Option<u16> {
    // The Rust integer parser also accepts a leading `+`, which is not valid URL
    // port syntax, so digits are checked before parsing.
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u16>().ok().filter(|port| *port != 0)
}

/// Build the production media binding for one Runtime session.
///
/// `audience` is the authenticated Desktop principal of the session credential,
/// read from the protected record by the composition root. The descriptor's own
/// audience claim is never trusted, so an absent principal is an explicit
/// `Unavailable` rather than a permission to skip audience validation. The
/// binding owns one process-wide bounded repository and a source factory that
/// builds a fresh same-authority executor per resolution; the credential lives
/// only inside that factory and no rendering path can reach it.
pub(crate) fn media_binding(settings: &RuntimeSettings, audience: Option<&str>) -> MediaBinding {
    let Some(audience) = audience.map(str::trim).filter(|value| !value.is_empty()) else {
        eprintln!(
            "Runtime media acquisition unavailable: the authenticated Desktop principal is unavailable"
        );
        return MediaBinding::Unavailable;
    };
    // Validate the origin and non-empty credential before exposing a binding;
    // the factory re-creates the executor for each resolution.
    let Some(probe) = MediaHttp::new(&settings.endpoint, &settings.token) else {
        eprintln!(
            "Runtime media acquisition unavailable: the configured endpoint is not a supported loopback origin"
        );
        return MediaBinding::Unavailable;
    };
    drop(probe);
    let Ok(repository) = app_core::MediaRepository::new(app_core::media_limits()) else {
        eprintln!("Runtime media acquisition unavailable: bounded media cache limits were refused");
        return MediaBinding::Unavailable;
    };
    let origin = settings.endpoint.clone();
    let token = settings.token.clone();
    let audience = audience.to_owned();
    MediaBinding::Runtime(MediaRuntime::new(
        repository,
        audience,
        Box::new(move |server_identity, audience| {
            let executor = MediaHttp::new(&origin, &token)
                .expect("the composition root validated this origin and credential");
            Box::new(MediaFetcher::new(
                executor,
                server_identity.as_str().to_owned(),
                audience.to_owned(),
            ))
        }),
    ))
}

/// Build the observation client over the configured Runtime origin.
pub(crate) fn observation(
    settings: RuntimeSettings,
    store: Arc<dyn CredentialStore>,
) -> Result<RuntimeObservation, RuntimeUnavailable> {
    let RuntimeSettings {
        endpoint,
        token,
        revocable,
    } = settings;
    // One in-memory credential value shared by the transport and the recovery
    // executor: a reauthorized value is presented by every later request
    // without rebuilding the session. Only a credential this app issued or
    // recovered is ever armed for teardown revocation.
    let slot: CredentialSlot = Arc::new(Mutex::new(Some(token)));
    let revoke_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let teardown = Some(RuntimeTeardown {
        origin: endpoint.clone(),
        token: Arc::clone(&revoke_slot),
        store: Arc::clone(&store),
    });
    if revocable {
        let initial = slot.lock().ok().and_then(|value| value.clone());
        if let Ok(mut armed) = revoke_slot.lock() {
            *armed = initial;
        }
    }
    let executor = ReauthHttp::new(
        PlainHttp::new(&endpoint),
        Arc::clone(&store),
        endpoint.clone(),
        Arc::clone(&slot),
        Arc::clone(&revoke_slot),
        // Re-read at each bounded recovery boundary; the secret is never
        // retained by the long-lived executor.
        Arc::new(|| std::env::var(crate::credential_issuance::ISSUANCE_SECRET_ENV).ok()),
        crate::credential_issuance::UreqExchange::with_timeout(&endpoint, REAUTH_TIMEOUT),
    );
    let transport = ProtocolTransport::new(
        executor,
        ProtocolTransportConfig {
            // The transport presents the current slot value on the credential
            // routes only (open, snapshot, deltas, catalog, commands, receipt
            // lookups), never on the handshake or another path.
            bearer_token: None,
            credential_slot: Some(slot),
            ..ProtocolTransportConfig::default()
        },
        ProtocolVersions {
            protocol: PROTOCOL,
            schema: SCHEMA,
        },
    );
    let client = CosClient::new(
        transport,
        ClientConfig::default(),
        Limits::DEFAULT,
        // The client replaces this with `ClientSource::Runtime` once the
        // handshake succeeds; before that nothing may claim a real server.
        ClientSource::Unknown,
    );
    Ok(RuntimeObservation {
        client: Box::new(client),
        scope: ObservationScopeRequest {
            task_ids: Vec::new(),
            filters: vec![ScopeFilter {
                name: "view".to_owned(),
                value: "all".to_owned(),
            }],
        },
        teardown,
    })
}

/// Executor wrapper that recovers one rejected Desktop credential inside the
/// same session, without a relaunch.
///
/// Only a `401` on a **credential-bearing route** is evidence against the
/// credential; the handshake route carries no credential, so its `401` never
/// triggers a reauthorization. Recovery is bounded (a small attempt count and a
/// minimum interval between attempts) and never resends a command: a `401`
/// proves the request was refused before any effect, but a command intent is
/// still only ever sent once. Reading routes are retried exactly once with the
/// replacement credential; a second `401` is returned to the client, which tears
/// the session down and opens a fresh observation (never a stale cursor).
pub(crate) struct ReauthHttp<E: ProtocolHttp, X: CredentialExchange + Send> {
    inner: E,
    store: Arc<dyn CredentialStore>,
    origin: String,
    /// Credential the transport presents per request. Updated in place after a
    /// successful reauthorization so later requests never replay a rejected
    /// value.
    slot: cos_client::CredentialSlot,
    /// Token this session must revoke on explicit teardown, if any. Empty for
    /// an operator-provisioned value, which the Desktop must not revoke.
    revoke_slot: Arc<Mutex<Option<String>>>,
    /// Reads the operator issuance secret at the recovery boundary only, so the
    /// value is never retained by this long-lived executor.
    secret: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    exchange: X,
    policy: ReauthPolicy,
    attempts: u32,
    last_attempt: Option<Instant>,
}

/// Bounds for automatic reauthorization.
#[derive(Clone, Copy)]
struct ReauthPolicy {
    /// Maximum replaceable credentials per session.
    max_attempts: u32,
    /// Minimum spacing between attempts.
    min_interval: Duration,
}

impl Default for ReauthPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            min_interval: Duration::from_secs(2),
        }
    }
}

impl<E: ProtocolHttp, X: CredentialExchange + Send> ReauthHttp<E, X> {
    pub(crate) fn new(
        inner: E,
        store: Arc<dyn CredentialStore>,
        origin: String,
        slot: cos_client::CredentialSlot,
        revoke_slot: Arc<Mutex<Option<String>>>,
        secret: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        exchange: X,
    ) -> Self {
        Self {
            inner,
            store,
            origin,
            slot,
            revoke_slot,
            secret,
            exchange,
            policy: ReauthPolicy::default(),
            attempts: 0,
            last_attempt: None,
        }
    }

    /// Claim one bounded recovery attempt, or `None` when the budget or the
    /// minimum interval forbids it.
    #[cfg(test)]
    fn set_policy(&mut self, policy: ReauthPolicy) {
        self.policy = policy;
    }

    fn claim_attempt(&mut self) -> bool {
        if self.attempts >= self.policy.max_attempts {
            return false;
        }
        let now = Instant::now();
        if self
            .last_attempt
            .is_some_and(|last| now.duration_since(last) < self.policy.min_interval)
        {
            return false;
        }
        self.attempts += 1;
        self.last_attempt = Some(now);
        true
    }

    /// Exchange the rejected credential for a replacement, if any is available.
    fn reauthorize(&mut self) -> Option<String> {
        let secret = (self.secret)();
        match crate::credential_issuance::recover(
            &self.origin,
            secret.as_deref(),
            self.store.as_ref(),
            &mut self.exchange,
            std::time::SystemTime::now(),
        ) {
            crate::credential_issuance::Recovery::Rotated(token) => {
                if let Ok(mut slot) = self.slot.lock() {
                    *slot = Some(token.clone());
                }
                if let Ok(mut revoke) = self.revoke_slot.lock() {
                    *revoke = Some(token.clone());
                }
                eprintln!(
                    "Runtime credential reauthorized after rejection; the session reopens a fresh observation"
                );
                Some(token)
            }
            crate::credential_issuance::Recovery::Unavailable => {
                eprintln!(
                    "Runtime credential reauthorization unavailable; the session requires authorization"
                );
                None
            }
            crate::credential_issuance::Recovery::Transient => {
                eprintln!(
                    "Runtime credential reauthorization deferred; a later bounded attempt may succeed"
                );
                None
            }
        }
    }
}

/// The bearer value presented by one protocol request.
fn presented_token(request: &HttpRequest) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
        .map(str::to_owned)
        .filter(|token| !token.is_empty())
}

/// One protocol command route: never retried after a `401`.
fn is_command_route(path: &str) -> bool {
    let route = path.split('?').next().unwrap_or(path);
    route == cos_client::COMMANDS_PATH || route == cos_client::COMMANDS_LOOKUP_PATH
}

/// Rebuild a request with the replacement credential.
fn with_authorization(mut request: HttpRequest, token: &str) -> HttpRequest {
    request
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
    request
        .headers
        .push(("authorization".to_owned(), format!("Bearer {token}")));
    request
}

impl<E: ProtocolHttp, X: CredentialExchange + Send> ProtocolHttp for ReauthHttp<E, X> {
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        let credential_route = cos_client::is_protocol_route(&request.path);
        let presented = presented_token(&request);
        let response = self.inner.execute(request.clone())?;
        if response.status != 401 || !credential_route || presented.is_none() {
            return Ok(response);
        }
        // A 401 is a refusal before any effect. Only a bounded, due recovery
        // may replace the credential; a command is never resent.
        if !self.claim_attempt() {
            return Ok(response);
        }
        let Some(token) = self.reauthorize() else {
            return Ok(response);
        };
        if is_command_route(&request.path) {
            return Ok(response);
        }
        self.inner.execute(with_authorization(request, &token))
    }
}

/// Blocking plain-HTTP executor handed to the protocol transport.
///
/// Owns the origin, the request timeout and the response body bound. It is
/// deliberately credential-free: the transport adds `Authorization` to the three
/// protocol routes only, and this layer forwards the headers it is given
/// verbatim. `ureq` is built without TLS or compression features: the only
/// accepted endpoints are loopback development origins.
struct PlainHttp {
    origin: String,
    agent: ureq::Agent,
    max_body_bytes: u64,
}

impl PlainHttp {
    fn new(origin: &str) -> Self {
        Self::with_max_body_bytes(origin, MAX_RESPONSE_BYTES)
    }

    fn with_max_body_bytes(origin: &str, max_body_bytes: u64) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(REQUEST_TIMEOUT)
            // Never follow a redirect or an environment proxy: either could
            // replay a credential to another origin.
            .redirects(0)
            .try_proxy_from_env(false)
            .build();
        Self {
            origin: origin.trim_end_matches('/').to_owned(),
            agent,
            max_body_bytes,
        }
    }
}

impl ProtocolHttp for PlainHttp {
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        let url = format!("{}{}", self.origin, request.path);
        let method = match request.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        };
        let mut call = self.agent.request(method, &url);
        // The transport owns the credential rule; this executor forwards every
        // header it was given and adds none of its own (it cannot leak the token
        // to another path or origin because it never sees a token at all).
        for (name, value) in &request.headers {
            call = call.set(name, value);
        }
        let outcome = match request.method {
            HttpMethod::Get => call.call(),
            HttpMethod::Post => call.send_bytes(&request.body),
        };
        match outcome {
            Ok(response) => self.read_response(response),
            // A 4xx/5xx body carries the protocol error document; it is a
            // response, not a transport failure.
            Err(ureq::Error::Status(status, response)) => Ok(HttpResponse {
                status,
                body: self.read_body(response)?,
            }),
            // `ErrorKind` is a closed enum: it cannot contain the URL, the
            // endpoint or the token.
            Err(ureq::Error::Transport(error)) => {
                Err(format!("plain HTTP transport failed ({:?})", error.kind()))
            }
        }
    }
}

impl PlainHttp {
    fn read_response(&self, response: ureq::Response) -> Result<HttpResponse, String> {
        let status = response.status();
        let body = self.read_body(response)?;
        Ok(HttpResponse { status, body })
    }

    fn read_body(&self, response: ureq::Response) -> Result<Vec<u8>, String> {
        let limit = self.max_body_bytes.saturating_add(1);
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit)
            .read_to_end(&mut body)
            .map_err(|_| "response body could not be read".to_owned())?;
        if body.len() as u64 > self.max_body_bytes {
            return Err("response body exceeds the bounded size".to_owned());
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LaunchPlan, Mode, PlainHttp, ReauthHttp, ReauthPolicy, RuntimeUnavailable, observation,
        plain_http_origin, plan, settings,
    };
    use cos_client::{
        COMMANDS_PATH, DELTAS_PATH, HttpMethod, HttpRequest, HttpResponse, OBSERVATION_PATH,
        ProtocolHttp, SERVER_PATH, SNAPSHOT_PATH,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use crate::credential_issuance::{CredentialExchange, ExchangeFailure, HttpResponseBytes};
    use crate::credentials::{CredentialStore, CredentialStoreError};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test double for the protected store: records every operation and can be
    /// told to fail, so the startup resolution and revocation are observable.
    #[derive(Default)]
    struct MemoryStore {
        entry: Mutex<Option<String>>,
        loads: AtomicUsize,
        stores: Mutex<Vec<String>>,
        deletes: AtomicUsize,
        fail_load: bool,
        fail_store: bool,
        fail_delete: bool,
    }

    impl MemoryStore {
        fn with_token(token: &str) -> Self {
            Self {
                entry: Mutex::new(Some(token.to_owned())),
                ..Self::default()
            }
        }

        fn stored(&self) -> Option<String> {
            self.entry.lock().expect("memory store").clone()
        }

        fn store_calls(&self) -> Vec<String> {
            self.stores.lock().expect("memory store").clone()
        }

        fn load_calls(&self) -> usize {
            self.loads.load(Ordering::SeqCst)
        }

        fn delete_calls(&self) -> usize {
            self.deletes.load(Ordering::SeqCst)
        }
    }

    impl CredentialStore for MemoryStore {
        fn load(&self, _origin: &str) -> Result<Option<String>, CredentialStoreError> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            if self.fail_load {
                return Err(CredentialStoreError::Failed);
            }
            Ok(self.stored())
        }

        fn store(&self, _origin: &str, token: &str) -> Result<(), CredentialStoreError> {
            self.stores
                .lock()
                .expect("memory store")
                .push(token.to_owned());
            if self.fail_store {
                return Err(CredentialStoreError::Unavailable);
            }
            *self.entry.lock().expect("memory store") = Some(token.to_owned());
            Ok(())
        }

        fn delete(&self, _origin: &str) -> Result<(), CredentialStoreError> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            if self.fail_delete {
                return Err(CredentialStoreError::Failed);
            }
            *self.entry.lock().expect("memory store") = None;
            Ok(())
        }
    }

    fn runtime_mode(endpoint_configured: bool) -> Mode {
        Mode::Runtime {
            endpoint_configured,
        }
    }

    fn unavailable(reason: LaunchPlan) -> RuntimeUnavailable {
        match reason {
            LaunchPlan::RuntimeUnavailable { reason, .. } => reason,
            LaunchPlan::Runtime(_) => panic!("expected an unavailable runtime plan"),
            LaunchPlan::Fixture { .. } => panic!("runtime mode must never plan a fixture"),
        }
    }

    #[test]
    fn runtime_mode_without_an_endpoint_or_token_is_explicitly_unavailable() {
        let store = MemoryStore::default();
        assert_eq!(
            unavailable(plan(&runtime_mode(false), None, None, &store)),
            RuntimeUnavailable::NotConfigured
        );
        assert_eq!(
            unavailable(plan(&runtime_mode(true), Some("  "), None, &store)),
            RuntimeUnavailable::NotConfigured
        );
        // A configured endpoint without an override or stored credential never
        // plans a fixture client and never attempts an unauthenticated session.
        assert_eq!(
            unavailable(plan(
                &runtime_mode(true),
                Some("http://127.0.0.1:8123"),
                None,
                &store
            )),
            RuntimeUnavailable::CredentialUnavailable
        );
        assert_eq!(
            unavailable(plan(
                &runtime_mode(true),
                Some("http://127.0.0.1:8123"),
                Some("   "),
                &store
            )),
            RuntimeUnavailable::CredentialUnavailable
        );
        // The unavailable label is static text and never echoes the endpoint.
        assert!(
            !RuntimeUnavailable::CredentialUnavailable
                .label()
                .contains("127.0.0.1")
        );
    }

    #[test]
    fn runtime_mode_with_endpoint_and_token_plans_a_runtime_observation() {
        let store = MemoryStore::default();
        let plan = plan(
            &runtime_mode(true),
            Some("http://127.0.0.1:8123/"),
            Some("token"),
            &store,
        );
        let LaunchPlan::Runtime(settings) = plan else {
            panic!("configured runtime mode must plan a runtime observation");
        };
        // The operator override is persisted for later launches.
        assert_eq!(store.store_calls(), vec!["token".to_owned()]);
        // The executor is built from the settings; the scope is the documented
        // all-task view and the client starts unbound from any real server.
        let observation =
            observation(settings, std::sync::Arc::new(store)).expect("observation builds");
        assert!(observation.scope.task_ids.is_empty());
        assert_eq!(observation.scope.filters.len(), 1);
        assert_eq!(observation.scope.filters[0].name, "view");
        assert_eq!(observation.scope.filters[0].value, "all");
    }

    #[test]
    fn only_an_issued_or_rotated_credential_is_revocable_on_teardown() {
        let override_store = MemoryStore::default();
        let override_settings = settings(
            Some("http://127.0.0.1:8123"),
            Some("override-token"),
            &override_store,
        )
        .expect("override resolves");
        let override_observation =
            observation(override_settings, std::sync::Arc::new(override_store))
                .expect("observation builds");
        assert!(
            override_observation
                .teardown
                .as_ref()
                .is_some_and(|teardown| !teardown.armed()),
            "an operator override is never armed for revocation"
        );

        let record = crate::credential_issuance::encode_for_test(
            "stored-token-0123456789",
            "desktop:0123456789abcdef0123456789abcdef",
            1_700_000_000,
        );
        let issued_store = MemoryStore::with_token(&record);
        let issued_settings =
            settings(Some("http://127.0.0.1:8123"), None, &issued_store).expect("record resolves");
        let issued_observation = observation(issued_settings, std::sync::Arc::new(issued_store))
            .expect("observation builds");
        assert!(
            issued_observation
                .teardown
                .as_ref()
                .is_some_and(|teardown| teardown.armed()),
            "an issued record is armed for explicit teardown revocation"
        );
    }

    #[test]
    fn the_environment_override_wins_and_replaces_the_stored_value() {
        let store = MemoryStore::with_token("stored-token");
        let settings = settings(
            Some("http://127.0.0.1:8123"),
            Some("override-token"),
            &store,
        )
        .expect("override resolves");
        assert_eq!(settings.token, "override-token");
        assert_eq!(store.stored().as_deref(), Some("override-token"));
        assert_eq!(store.store_calls(), vec!["override-token".to_owned()]);
        // The override short-circuits the store lookup.
        assert_eq!(store.load_calls(), 0);
    }

    #[test]
    fn the_stored_credential_is_used_when_the_environment_is_absent() {
        let store = MemoryStore::with_token("stored-token");
        let settings = settings(Some("http://127.0.0.1:8123"), None, &store)
            .expect("stored credential resolves");
        assert_eq!(settings.token, "stored-token");
        assert_eq!(store.load_calls(), 1);
        assert!(store.store_calls().is_empty());
    }

    #[test]
    fn a_missing_or_failing_store_is_explicitly_unavailable() {
        let empty = MemoryStore::default();
        match settings(Some("http://127.0.0.1:8123"), None, &empty) {
            Err(reason) => assert_eq!(reason, RuntimeUnavailable::CredentialUnavailable),
            Ok(_) => panic!("an empty store must be explicitly unavailable"),
        }
        let failing = MemoryStore {
            fail_load: true,
            ..MemoryStore::default()
        };
        match settings(Some("http://127.0.0.1:8123"), None, &failing) {
            Err(reason) => assert_eq!(reason, RuntimeUnavailable::CredentialUnavailable),
            Ok(_) => panic!("a failing store must be explicitly unavailable"),
        }
        // A failed store write does not block an explicit override.
        let unwritable = MemoryStore {
            fail_store: true,
            ..MemoryStore::default()
        };
        assert!(
            settings(
                Some("http://127.0.0.1:8123"),
                Some("override-token"),
                &unwritable
            )
            .is_ok()
        );
    }

    /// Scripted protocol executor: consumes canned responses, records requests.
    struct SequencedHttp {
        responses: std::collections::VecDeque<(u16, Vec<u8>)>,
        requests: Vec<HttpRequest>,
    }

    impl SequencedHttp {
        fn new(responses: impl IntoIterator<Item = (u16, Vec<u8>)>) -> Self {
            Self {
                responses: responses.into_iter().collect(),
                requests: Vec::new(),
            }
        }
    }

    impl ProtocolHttp for SequencedHttp {
        fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
            self.requests.push(request);
            let (status, body) = self.responses.pop_front().unwrap_or((500, b"{}".to_vec()));
            Ok(HttpResponse { status, body })
        }
    }

    /// Scripted credential endpoint; records the paths it was asked for.
    struct SequencedExchange {
        responses: std::collections::VecDeque<(u16, Vec<u8>)>,
        paths: Vec<String>,
    }

    impl SequencedExchange {
        fn new(responses: impl IntoIterator<Item = (u16, Vec<u8>)>) -> Self {
            Self {
                responses: responses.into_iter().collect(),
                paths: Vec::new(),
            }
        }
    }

    impl CredentialExchange for SequencedExchange {
        fn post(
            &mut self,
            path: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<HttpResponseBytes, ExchangeFailure> {
            self.paths.push(path.to_owned());
            let (status, body) = self.responses.pop_front().unwrap_or((500, Vec::new()));
            Ok(HttpResponseBytes { status, body })
        }
    }

    const OLD_TOKEN: &str = "old-token-0123456789abcdef";
    const NEW_TOKEN: &str = "new-token-0123456789abcdef";
    const OPERATOR_TOKEN: &str = "operator-token-0123456789ab";
    const TEST_ORIGIN: &str = "http://127.0.0.1:8123";

    fn versioned_record(token: &str) -> String {
        crate::credential_issuance::encode_for_test(
            token,
            "desktop:0123456789abcdef0123456789abcdef",
            1_700_000_000,
        )
    }

    fn credential_document(token: &str) -> Vec<u8> {
        format!(
            r#"{{"token":"{token}","principal":"desktop:0123456789abcdef0123456789abcdef","scopes":["read:observe","desktop:command"],"expires_in":43200}}"#
        )
        .into_bytes()
    }

    fn authorized(path: &str, method: HttpMethod, token: &str) -> HttpRequest {
        let mut request = request_for(path, method);
        request
            .headers
            .push(("authorization".to_owned(), format!("Bearer {token}")));
        request
    }

    fn reauth_http(
        responses: impl IntoIterator<Item = (u16, Vec<u8>)>,
        store: std::sync::Arc<MemoryStore>,
        slot: cos_client::CredentialSlot,
        revoke: std::sync::Arc<Mutex<Option<String>>>,
        secret: Option<&'static str>,
        exchange: SequencedExchange,
    ) -> ReauthHttp<SequencedHttp, SequencedExchange> {
        let mut executor = ReauthHttp::new(
            SequencedHttp::new(responses),
            store,
            TEST_ORIGIN.to_owned(),
            slot,
            revoke,
            std::sync::Arc::new(move || secret.map(str::to_owned)),
            exchange,
        );
        executor.set_policy(ReauthPolicy {
            max_attempts: 3,
            min_interval: Duration::ZERO,
        });
        executor
    }

    #[test]
    fn a_401_on_a_read_route_refreshes_the_issued_credential_and_retries_once() {
        let store = std::sync::Arc::new(MemoryStore::with_token(&versioned_record(OLD_TOKEN)));
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some(OLD_TOKEN.to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let mut executor = reauth_http(
            [(401, b"{}".to_vec()), (200, b"{}".to_vec())],
            std::sync::Arc::clone(&store),
            std::sync::Arc::clone(&slot),
            std::sync::Arc::clone(&revoke),
            None,
            SequencedExchange::new([(200, credential_document(NEW_TOKEN))]),
        );
        let response = executor
            .execute(authorized(DELTAS_PATH, HttpMethod::Get, OLD_TOKEN))
            .expect("two responses");
        assert_eq!(response.status, 200);
        let requests = executor.inner.requests;
        assert_eq!(requests.len(), 2, "the read is retried exactly once");
        assert_eq!(presented_header(&requests[0]).as_deref(), Some(OLD_TOKEN));
        assert_eq!(
            presented_header(&requests[1]).as_deref(),
            Some(NEW_TOKEN),
            "the retry presents the replacement, not the rejected value"
        );
        assert_eq!(
            slot.lock().expect("slot").as_deref(),
            Some(NEW_TOKEN),
            "later requests use the rotated credential"
        );
        assert_eq!(
            revoke.lock().expect("revoke").as_deref(),
            Some(NEW_TOKEN),
            "the rotated credential is armed for teardown revocation"
        );
        let stored = store.stored().expect("record");
        assert_eq!(
            crate::credential_issuance::stored_token(&stored).as_deref(),
            Some(NEW_TOKEN)
        );
        assert_eq!(store.delete_calls(), 0);
    }

    #[test]
    fn a_command_401_reauthorizes_but_never_resends_the_command() {
        let store = std::sync::Arc::new(MemoryStore::with_token(&versioned_record(OLD_TOKEN)));
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some(OLD_TOKEN.to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let mut executor = reauth_http(
            [(401, b"{}".to_vec())],
            std::sync::Arc::clone(&store),
            std::sync::Arc::clone(&slot),
            std::sync::Arc::clone(&revoke),
            None,
            SequencedExchange::new([(200, credential_document(NEW_TOKEN))]),
        );
        let response = executor
            .execute(authorized(COMMANDS_PATH, HttpMethod::Post, OLD_TOKEN))
            .expect("one response");
        assert_eq!(response.status, 401);
        assert_eq!(
            executor.inner.requests.len(),
            1,
            "a command intent is sent once and never automatically resent"
        );
        assert_eq!(
            slot.lock().expect("slot").as_deref(),
            Some(NEW_TOKEN),
            "the credential is still renewed for later reads"
        );
    }

    #[test]
    fn a_401_on_the_handshake_never_reauthorizes() {
        let store = std::sync::Arc::new(MemoryStore::with_token("valid-token"));
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some("valid-token".to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let mut executor = reauth_http(
            vec![(401, b"{}".to_vec()); 3],
            std::sync::Arc::clone(&store),
            std::sync::Arc::clone(&slot),
            std::sync::Arc::clone(&revoke),
            Some("operator-secret"),
            SequencedExchange::new([]),
        );
        for _ in 0..3 {
            executor
                .execute(request_for(SERVER_PATH, HttpMethod::Get))
                .expect("a 401 is a response");
        }
        assert_eq!(executor.inner.requests.len(), 3);
        assert_eq!(executor.exchange.paths.len(), 0, "no credential exchange");
        assert_eq!(store.stored().as_deref(), Some("valid-token"));
    }

    #[test]
    fn an_operator_provisioned_token_is_never_rotated_or_deleted() {
        let store = std::sync::Arc::new(MemoryStore::with_token(OPERATOR_TOKEN));
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some(OPERATOR_TOKEN.to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let mut executor = reauth_http(
            vec![(401, b"{}".to_vec()); 4],
            std::sync::Arc::clone(&store),
            slot,
            revoke,
            None,
            SequencedExchange::new([]),
        );
        for _ in 0..3 {
            executor
                .execute(authorized(
                    OBSERVATION_PATH,
                    HttpMethod::Post,
                    OPERATOR_TOKEN,
                ))
                .expect("a 401 is a response");
        }
        assert_eq!(executor.exchange.paths.len(), 0, "no refresh is attempted");
        assert_eq!(store.delete_calls(), 0, "the operator value is preserved");
        assert_eq!(store.stored().as_deref(), Some(OPERATOR_TOKEN));
    }

    #[test]
    fn a_rejected_issued_credential_is_reissued_once_after_refresh_fails() {
        let store = std::sync::Arc::new(MemoryStore::with_token(&versioned_record(OLD_TOKEN)));
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some(OLD_TOKEN.to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let mut executor = reauth_http(
            [(401, b"{}".to_vec()), (200, b"{}".to_vec())],
            std::sync::Arc::clone(&store),
            std::sync::Arc::clone(&slot),
            std::sync::Arc::clone(&revoke),
            Some("operator-secret"),
            SequencedExchange::new([(401, b"{}".to_vec()), (200, credential_document(NEW_TOKEN))]),
        );
        let response = executor
            .execute(authorized(SNAPSHOT_PATH, HttpMethod::Get, OLD_TOKEN))
            .expect("two responses");
        assert_eq!(response.status, 200);
        assert_eq!(
            executor.exchange.paths,
            vec![
                crate::credential_issuance::REFRESH_PATH.to_owned(),
                crate::credential_issuance::ISSUE_PATH.to_owned()
            ],
            "refresh preserves the principal; issuance is the fallback"
        );
        assert_eq!(store.delete_calls(), 1, "the dead record is cleared");
        let stored = store.stored().expect("replacement record");
        assert_eq!(
            crate::credential_issuance::stored_token(&stored).as_deref(),
            Some(NEW_TOKEN)
        );
    }

    #[test]
    fn reauthorization_is_bounded_and_a_transient_failure_keeps_the_record() {
        let store = std::sync::Arc::new(MemoryStore::with_token(&versioned_record(OLD_TOKEN)));
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some(OLD_TOKEN.to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let mut executor = reauth_http(
            vec![(401, b"{}".to_vec()); 5],
            std::sync::Arc::clone(&store),
            slot,
            revoke,
            None,
            SequencedExchange::new([(503, Vec::new())]),
        );
        for _ in 0..5 {
            executor
                .execute(authorized(DELTAS_PATH, HttpMethod::Get, OLD_TOKEN))
                .expect("a 401 is a response");
        }
        assert_eq!(
            executor.exchange.paths.len(),
            3,
            "transient refusals stay inside the bounded attempt budget"
        );
        assert_eq!(
            store.stored().as_deref(),
            Some(versioned_record(OLD_TOKEN).as_str()),
            "a transient failure keeps the existing record"
        );
        assert_eq!(store.delete_calls(), 0);

        // A hard refusal exhausts the bounded attempt budget across rejections.
        let store = std::sync::Arc::new(MemoryStore::with_token(&versioned_record(OLD_TOKEN)));
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some(OLD_TOKEN.to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let mut executor = reauth_http(
            vec![(401, b"{}".to_vec()); 8],
            store,
            slot,
            revoke,
            Some("operator-secret"),
            SequencedExchange::new([
                (401, Vec::new()),
                (401, Vec::new()),
                (401, Vec::new()),
                (401, Vec::new()),
            ]),
        );
        for _ in 0..8 {
            executor
                .execute(authorized(DELTAS_PATH, HttpMethod::Get, OLD_TOKEN))
                .expect("a 401 is a response");
        }
        assert_eq!(
            executor.exchange.paths.len(),
            4,
            "three bounded attempts: one refresh+issue, then one issue each"
        );
        assert_eq!(
            executor.attempts, 3,
            "the attempt budget stops reauthorization"
        );
    }

    fn presented_header(request: &HttpRequest) -> Option<String> {
        request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .and_then(|(_, value)| value.strip_prefix("Bearer ").map(str::to_owned))
    }

    #[test]
    fn fixture_mode_stays_an_explicit_fixture_plan() {
        let plan = plan(
            &Mode::Fixture {
                path: std::path::PathBuf::from("fixture.json"),
            },
            Some("http://127.0.0.1:8123"),
            Some("token"),
            &MemoryStore::default(),
        );
        assert!(matches!(plan, LaunchPlan::Fixture { .. }));
    }

    #[test]
    fn endpoints_are_loopback_origins_only() {
        // Refused: hostnames (their resolution can change between validation and
        // the connection), remote or lookalike addresses, malformed authorities
        // (unbracketed IPv6, zero port), non-origin shapes, userinfo, control
        // characters and non-http schemes.
        for endpoint in [
            "http://example.com",
            "http://localhost",
            "http://localhost:8123",
            "http://LOCALHOST",
            "http://127.0.0.1.evil.example",
            "http://10.0.0.5",
            "http://127.0.0.1:8123/path?x=1",
            "http://127.0.0.1:8123/snapshot",
            "http://user@127.0.0.1",
            "http://::1",
            "http://::1:8123",
            "http://127.0.0.1:0",
            "http://[::1]:0",
            "http://127.0.0.1:+80",
            "http://[127.0.0.1]",
            "http://[127.0.0.1]:8123",
            "http://127.0.0.1:not-a-port",
            "https://127.0.0.1:8123",
            "ftp://127.0.0.1:8123",
            "127.0.0.1:8123",
            "http://",
            "http://user:secret@127.0.0.1:8123",
            "http://127.0.0.1:8123\r\nX-Injected: 1",
        ] {
            match settings(Some(endpoint), Some("token"), &MemoryStore::default()) {
                Err(reason) => assert_eq!(
                    reason,
                    RuntimeUnavailable::UnsupportedEndpoint,
                    "endpoint {endpoint:?} must be refused"
                ),
                Ok(_) => panic!("endpoint {endpoint:?} must be refused"),
            }
        }
        // The refusal label is static text and never echoes the endpoint.
        assert!(
            !RuntimeUnavailable::UnsupportedEndpoint
                .label()
                .contains("example.com")
        );
        assert!(
            !RuntimeUnavailable::UnsupportedEndpoint
                .label()
                .contains("localhost")
        );

        // Accepted: loopback IP literals (127.0.0.0/8 and [::1]), with an
        // optional non-zero port and a single trailing slash normalized away.
        for (endpoint, origin) in [
            ("http://127.0.0.1:8123", "http://127.0.0.1:8123"),
            ("http://127.0.0.1:8123/", "http://127.0.0.1:8123"),
            ("http://127.0.0.1", "http://127.0.0.1"),
            ("http://127.0.0.5:8123", "http://127.0.0.5:8123"),
            ("http://[::1]:8123", "http://[::1]:8123"),
            ("http://[::1]", "http://[::1]"),
        ] {
            assert_eq!(
                plain_http_origin(endpoint).as_deref(),
                Some(origin),
                "endpoint {endpoint:?} must be accepted"
            );
            assert!(
                settings(Some(endpoint), Some("token"), &MemoryStore::default()).is_ok(),
                "endpoint {endpoint:?} must yield settings"
            );
        }
    }

    /// One-shot HTTP/1.1 server: reads the request (headers and Content-Length
    /// body) and answers with the given status/body.
    fn serve_once(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            let mut headers_end = None;
            while headers_end.is_none() {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                headers_end = request.windows(4).position(|window| window == b"\r\n\r\n");
            }
            if let Some(position) = headers_end {
                let headers = String::from_utf8_lossy(&request[..position]).to_ascii_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let body_start = position + 4;
                while request.len() < body_start + content_length {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
            }
            let response = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            String::from_utf8_lossy(&request).to_string()
        });
        (format!("http://{address}"), handle)
    }

    fn get_request(path: &str) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Get,
            path: path.to_owned(),
            headers: vec![("accept".to_owned(), "application/json".to_owned())],
            body: Vec::new(),
        }
    }

    fn request_for(path: &str, method: HttpMethod) -> HttpRequest {
        HttpRequest {
            method,
            path: path.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn post_request(path: &str) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            path: path.to_owned(),
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: br#"{"schema_version":1}"#.to_vec(),
        }
    }

    #[test]
    fn the_executor_forwards_transport_headers_and_adds_none_of_its_own() {
        // The transport supplies the credential on the protocol routes; the
        // executor must forward it verbatim (it never decides about credentials).
        let (origin, server) = serve_once("HTTP/1.1 200 OK", "{}");
        let mut executor = PlainHttp::new(&origin);
        let mut request = post_request(OBSERVATION_PATH);
        request.headers.push((
            "authorization".to_owned(),
            "Bearer local-test-token".to_owned(),
        ));
        let response = executor.execute(request).expect("the grant post succeeds");
        assert_eq!(response.status, 200);
        let seen = server.join().expect("server finished").to_ascii_lowercase();
        assert!(
            seen.contains("authorization: bearer local-test-token"),
            "{seen}"
        );
        assert!(
            seen.starts_with("post /api/cos/desktop/observation"),
            "{seen}"
        );

        // A request without a credential stays credential-free: the executor
        // never invents one, and a non-protocol path (the handshake) is sent
        // exactly as the transport built it.
        let (origin, server) = serve_once(
            "HTTP/1.1 401 Unauthorized",
            r#"{"code":"UNAUTHORIZED_CONTEXT"}"#,
        );
        let mut executor = PlainHttp::new(&origin);
        let response = executor
            .execute(get_request(SERVER_PATH))
            .expect("a 401 is a response, not a transport failure");
        assert_eq!(response.status, 401);
        assert_eq!(
            String::from_utf8_lossy(&response.body),
            r#"{"code":"UNAUTHORIZED_CONTEXT"}"#
        );
        let seen = server.join().expect("server finished").to_ascii_lowercase();
        assert!(
            !seen.contains("authorization"),
            "the handshake never carries a credential: {seen}"
        );
    }

    #[test]
    fn the_executor_bounds_the_response_body() {
        let (origin, server) = serve_once("HTTP/1.1 200 OK", "0123456789abcdef");
        let mut executor = PlainHttp::with_max_body_bytes(&origin, 4);
        let error = executor
            .execute(get_request("/api/cos/desktop/server"))
            .expect_err("an oversized body is refused");
        assert!(error.contains("bounded size"), "{error}");
        let _ = server.join();
    }

    #[test]
    fn transport_failures_never_echo_the_request_credential() {
        // Reserve and release a port so the connection is refused.
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        drop(listener);
        let mut executor = PlainHttp::new(&format!("http://{address}"));
        // The executor still receives the transport-built credential header, so
        // its failure messages must never echo it.
        let mut request = get_request(SERVER_PATH);
        request.headers.push((
            "authorization".to_owned(),
            "Bearer token-that-must-never-leak".to_owned(),
        ));
        // Give the OS a moment before the refused connect on a released port.
        std::thread::sleep(Duration::from_millis(10));
        let error = executor
            .execute(request)
            .expect_err("a refused connection is a transport failure");
        assert!(!error.contains("token-that-must-never-leak"), "{error}");
        assert!(!error.contains(&format!("{address}")), "{error}");
    }

    /// End-to-end stub-server recovery: a rejected open is reauthorized from the
    /// operator secret and the client applies a fresh observation under the new
    /// credential, with no command retry.
    #[test]
    fn a_401_reissues_the_credential_and_opens_a_fresh_observation() {
        use desktop_protocol::{
            Counter, Cursor, EpochId, HelloAccepted, Limits, ObservationScope,
            ObservationScopeRequest, Projection, ProjectionMutation, ProtocolVersions,
            ReadContextId, ReadGrant, ReadRequestId, ReadRevision, ReadStamp, ScopeFilter, ScopeId,
            ServerId, SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotEnd, SnapshotId,
            TaskId, TaskSnapshot, TaskStatus, Version, projection_bundle_digest,
        };
        use serde_json::json;

        const STALE: &str = "stale-token-0123456789";
        const FRESH: &str = "fresh-token-0123456789";
        const NEW_EPOCH: &str = "epoch-after-restart";
        const TITLE: &str = "Fresh observation after reauthorization";

        struct StubState {
            opens: Vec<Option<String>>,
            issues: u32,
        }

        struct StubProtocol {
            state: std::sync::Arc<Mutex<StubState>>,
            expected: String,
        }

        fn hello_document() -> serde_json::Value {
            json!({
                "schema_version": 1,
                "server": serde_json::to_value(HelloAccepted {
                    protocol: Version::new(2, 0),
                    schema: Version::new(2, 0),
                    server_identity: ServerId::new("cos-runtime-orchestrator").expect("id"),
                    server_build: "orchestrator-runtime/test".to_owned(),
                    epoch: EpochId::new(NEW_EPOCH).expect("epoch"),
                    dedupe_retention_ms: None,
                    limits: None,
                })
                .expect("hello"),
            })
        }

        fn open_document(body: &serde_json::Value, title: &str) -> serde_json::Value {
            let request_id = body["request_id"].as_str().expect("request id");
            let protocol: Version =
                serde_json::from_value(body["protocol"].clone()).expect("protocol");
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
                epoch: EpochId::new(NEW_EPOCH).expect("epoch"),
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
            let cursor = Cursor::new("cursor-1").expect("cursor");
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
                epoch: EpochId::new(NEW_EPOCH).expect("epoch"),
                cursor,
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
                "server": hello_document()["server"].clone(),
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

        impl ProtocolHttp for StubProtocol {
            fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
                let route = request
                    .path
                    .split('?')
                    .next()
                    .unwrap_or(request.path.as_str());
                match (request.method, route) {
                    (HttpMethod::Get, SERVER_PATH) => Ok(HttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&hello_document()).expect("hello"),
                    }),
                    (HttpMethod::Post, OBSERVATION_PATH) => {
                        let token = super::presented_token(&request);
                        let rejected = token.as_deref() != Some(self.expected.as_str());
                        {
                            let mut state = self.state.lock().expect("state");
                            state.opens.push(token);
                        }
                        if rejected {
                            return Ok(HttpResponse {
                                status: 401,
                                body: b"{}".to_vec(),
                            });
                        }
                        let body: serde_json::Value =
                            serde_json::from_slice(&request.body).expect("open body");
                        Ok(HttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&open_document(&body, TITLE))
                                .expect("open document"),
                        })
                    }
                    (HttpMethod::Get, DELTAS_PATH) => Ok(HttpResponse {
                        status: 200,
                        body: json!({"schema_version": 1, "transaction": null})
                            .to_string()
                            .into_bytes(),
                    }),
                    _ => Ok(HttpResponse {
                        status: 404,
                        body: b"{}".to_vec(),
                    }),
                }
            }
        }

        struct StubExchange {
            state: std::sync::Arc<Mutex<StubState>>,
        }

        impl CredentialExchange for StubExchange {
            fn post(
                &mut self,
                path: &str,
                _headers: &[(&str, &str)],
                _body: &[u8],
            ) -> Result<HttpResponseBytes, ExchangeFailure> {
                match path {
                    crate::credential_issuance::ISSUE_PATH => {
                        self.state.lock().expect("state").issues += 1;
                        Ok(HttpResponseBytes {
                            status: 200,
                            body: credential_document(FRESH),
                        })
                    }
                    crate::credential_issuance::REFRESH_PATH => Ok(HttpResponseBytes {
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

        let state = std::sync::Arc::new(Mutex::new(StubState {
            opens: Vec::new(),
            issues: 0,
        }));
        let memory_store = std::sync::Arc::new(MemoryStore::default());
        let store: std::sync::Arc<dyn CredentialStore> = memory_store.clone();
        let slot: cos_client::CredentialSlot =
            std::sync::Arc::new(Mutex::new(Some(STALE.to_owned())));
        let revoke = std::sync::Arc::new(Mutex::new(None));
        let executor = ReauthHttp::new(
            StubProtocol {
                state: std::sync::Arc::clone(&state),
                expected: FRESH.to_owned(),
            },
            std::sync::Arc::clone(&store),
            TEST_ORIGIN.to_owned(),
            std::sync::Arc::clone(&slot),
            revoke,
            std::sync::Arc::new(|| Some("operator-secret".to_owned())),
            StubExchange {
                state: std::sync::Arc::clone(&state),
            },
        );
        let transport = cos_client::ProtocolTransport::new(
            executor,
            cos_client::ProtocolTransportConfig {
                credential_slot: Some(std::sync::Arc::clone(&slot)),
                ..cos_client::ProtocolTransportConfig::default()
            },
            ProtocolVersions {
                protocol: Version::new(2, 0),
                schema: Version::new(2, 0),
            },
        );
        let mut client = cos_client::CosClient::new(
            transport,
            cos_client::ClientConfig::default(),
            Limits::DEFAULT,
            cos_client::ClientSource::Unknown,
        );
        client
            .begin_authorized_observation(client.observation_request(ObservationScopeRequest {
                task_ids: Vec::new(),
                filters: vec![ScopeFilter {
                    name: "view".to_owned(),
                    value: "all".to_owned(),
                }],
            }))
            .expect("observation begins");

        let mut now = 0u64;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            now += 10;
            client.pump(now);
            if client.connection().is_live()
                && client
                    .snapshot()
                    .tasks
                    .values()
                    .any(|task| task.title == TITLE)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            client.connection().is_live(),
            "state {:?}, last error {:?}",
            client.connection(),
            client.last_error()
        );
        assert!(
            client
                .snapshot()
                .tasks
                .values()
                .any(|task| task.title == TITLE),
            "a fresh snapshot from the reissued credential is published"
        );
        {
            let state = state.lock().expect("state");
            assert_eq!(state.issues, 1, "the operator secret issues one credential");
            assert_eq!(
                state.opens,
                vec![Some(STALE.to_owned()), Some(FRESH.to_owned())],
                "the rejected open is retried once with the replacement credential"
            );
        }
        assert_eq!(
            slot.lock().expect("slot").as_deref(),
            Some(FRESH),
            "the transport slot carries the replacement for later requests"
        );
        let stored = memory_store.stored().expect("record");
        assert_eq!(
            crate::credential_issuance::stored_token(&stored).as_deref(),
            Some(FRESH)
        );
    }
}
