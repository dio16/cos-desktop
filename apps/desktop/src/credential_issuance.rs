//! Desktop credential issuance, rotation and revocation (MP-SEC1).
//!
//! The protocol transport already presents a bearer credential; this module is
//! the only place that *obtains*, *rotates* or *revokes* it. It never invents a
//! wire shape: the contract below was read from the running Runtime adapter
//! (`POST /api/cos/desktop/credentials` with `X-Auth-Secret`, and the matching
//! `refresh`/`revoke` routes with `Authorization: Bearer`). Responses are
//! `{"token","principal","scopes","expires_in"}`; the Desktop-bound principal is
//! `desktop:<32 lowercase hex>` and the credential carries exactly the two
//! documented scopes, `read:observe` then `desktop:command`.
//!
//! # Operator provisioning
//!
//! The issuance secret is read once from `COS_DESKTOP_RUNTIME_ISSUANCE_SECRET`.
//! It is placed in the `X-Auth-Secret` header of one loopback request and is
//! never written to the credential store, a file, a preference, a log line, a
//! diagnostic, an error message or a panic message. The fetched token is only
//! persisted through [`CredentialStore`] (Windows Credential Manager) and then
//! dropped; it is never rendered by this module.
//!
//! Residual exposure: the variable remains in the process environment for the
//! rest of the process lifetime. `std::env::remove_var` is `unsafe` in Rust
//! 2024 and this crate forbids unsafe code, so no unset is attempted — the value
//! is simply never read again after the single acquisition. An operator that
//! needs it gone must not provision it for the session or start the process with
//! a scoped environment.
//!
//! # Rotation boundary
//!
//! [`ProtocolTransport`](cos_client::ProtocolTransport) owns an immutable
//! bearer value for the lifetime of one session, so this build rotates at the
//! **next session boundary** (process start), before the transport is built:
//! a stored credential whose deadline is within [`REFRESH_MARGIN`] is renewed
//! through `/refresh` and the stored record is replaced. A long-running session
//! that outlives its credential still ends in the existing fail-closed path: the
//! transport reports `401`, `RevokingHttp` deletes the stored entry and the next
//! launch requires re-authorization. No background thread is introduced.
//!
//! # Storage format
//!
//! A record written by this module is a four-line ASCII document (version line,
//! deadline in Unix seconds, principal, token). A raw operator override written
//! by `runtime::settings` stays a bare token and has no known deadline, so it is
//! never refreshed. Both forms are opaque to [`CredentialStore`].
//!
//! # Memory handling
//!
//! No credential type here implements `Display` or a value-bearing `Debug`, and
//! no error carries a token, principal, origin or header value. Rust does not
//! zeroize dropped `String` allocations and this crate deliberately adds no
//! zeroize dependency, so the bounded copies are dropped without being rendered;
//! the protected store is the only durable copy. An issued credential whose
//! store write fails is revoked immediately instead of being left orphaned.

use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::credentials::CredentialStore;

/// Operator-provisioned issuance secret. Never logged or persisted.
pub(crate) const ISSUANCE_SECRET_ENV: &str = "COS_DESKTOP_RUNTIME_ISSUANCE_SECRET";

pub(crate) const ISSUE_PATH: &str = "/api/cos/desktop/credentials";
pub(crate) const REFRESH_PATH: &str = "/api/cos/desktop/credentials/refresh";
const REVOKE_PATH: &str = "/api/cos/desktop/credentials/revoke";
const SECRET_HEADER: &str = "X-Auth-Secret";
const AUTHORIZATION_HEADER: &str = "Authorization";
const BEARER_PREFIX: &str = "Bearer ";

/// The scopes a Desktop credential must carry, in the documented order.
///
/// The Runtime emits this exact sequence; a missing, extra, duplicate or
/// reordered scope is a closed-document failure rather than text to tolerate.
const EXPECTED_SCOPES: [&str; 2] = ["read:observe", "desktop:command"];
/// Server-managed Desktop principal prefix. The remainder is pinned to 32
/// lowercase hexadecimal characters (the observed `desktop:<random>` shape); it
/// is validated on the wire and in the stored record, and never rendered.
const PRINCIPAL_PREFIX: &str = "desktop:";
/// Length of the random suffix of a Desktop principal.
const PRINCIPAL_SUFFIX_LEN: usize = 32;

/// Bounded accepted credential document (the observed document is 155 bytes).
pub(crate) const MAX_CREDENTIAL_RESPONSE_BYTES: u64 = 8 * 1024;
/// Bounded rotation margin: renew a stored credential this close to its deadline.
pub(crate) const REFRESH_MARGIN: Duration = Duration::from_secs(300);
/// Bounded teardown budget, so a revoke cannot hold the process open.
pub(crate) const REVOKE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Upper bound for a server-provided lifetime (the observed value is 12 h).
const MAX_EXPIRES_IN_SECONDS: u64 = 24 * 60 * 60;
const MIN_TOKEN_BYTES: usize = 16;
const MAX_TOKEN_BYTES: usize = 4096;

const STORED_VERSION: &str = "cos-desktop-credential-v1";

/// Fixed failure classes. No variant carries a token, a principal, an origin,
/// a header value or an OS message, so the labels are safe to print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialError {
    /// No stored credential and no issuance secret were provisioned.
    MissingSecret,
    /// The Runtime rejected the secret or the presented credential (`401`).
    Unauthorized,
    /// The Runtime refused the operation (`403`).
    Forbidden,
    /// The Runtime reported temporary unavailability (`503`).
    ServiceUnavailable,
    /// The request document was refused (`422`).
    Rejected,
    /// Any other non-success status.
    UnexpectedStatus,
    /// The response exceeded the bounded document size.
    OversizedResponse,
    /// The response was not the closed credential document.
    MalformedResponse,
    /// The loopback exchange failed before a response existed.
    TransportFailure,
    /// The protected store refused a read or write.
    StoreUnavailable,
    /// A stored credential is past its deadline and could not be renewed.
    Expired,
}

impl CredentialError {
    /// Value-free operator label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::MissingSecret => {
                "runtime credential is missing (provision the store or the issuance secret)"
            }
            Self::Unauthorized => "runtime credential request was not authorized",
            Self::Forbidden => "runtime credential request was refused",
            Self::ServiceUnavailable => "runtime credential service is unavailable",
            Self::Rejected => "runtime credential request document was rejected",
            Self::UnexpectedStatus => "runtime credential endpoint returned an unexpected status",
            Self::OversizedResponse => "runtime credential response exceeded the bounded size",
            Self::MalformedResponse => "runtime credential response was not the expected document",
            Self::TransportFailure => "runtime credential exchange failed",
            Self::StoreUnavailable => "runtime credential store is unavailable",
            Self::Expired => "stored runtime credential expired and could not be renewed",
        }
    }

    fn from_status(status: u16) -> Self {
        match status {
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            422 => Self::Rejected,
            503 => Self::ServiceUnavailable,
            _ => Self::UnexpectedStatus,
        }
    }

    /// Failures that prove the credential can no longer be used.
    fn requires_reauthorization(self) -> bool {
        matches!(self, Self::Unauthorized | Self::Forbidden | Self::Rejected)
    }
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// One bounded loopback exchange. Implementations never log a header value.
pub(crate) trait CredentialExchange {
    /// Send one `POST` to `path` (relative to the validated origin).
    fn post(
        &mut self,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<HttpResponseBytes, ExchangeFailure>;
}

/// A raw response from the credential endpoint.
pub(crate) struct HttpResponseBytes {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

impl std::fmt::Debug for HttpResponseBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpResponseBytes")
            .field("status", &self.status)
            .field("body", &"<redacted>")
            .finish()
    }
}

/// Transport-level failure classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExchangeFailure {
    /// The connection failed, timed out or the body could not be read.
    Transport,
    /// The body exceeded the caller's bound.
    Oversized,
}

/// Production loopback exchange over the existing plain-HTTP `ureq` boundary.
///
/// Redirects and environment proxies are disabled for the same reason as the
/// protocol executor: a credential-bearing request must never be replayed to a
/// different origin.
pub(crate) struct UreqExchange {
    origin: String,
    timeout: Duration,
    max_body_bytes: u64,
}

impl UreqExchange {
    /// Exchange for one credential operation with the standard request budget.
    pub(crate) fn new(origin: &str) -> Self {
        Self::with_timeout(origin, Duration::from_secs(15))
    }

    /// Exchange with an explicit bounded budget (teardown uses a short one).
    pub(crate) fn with_timeout(origin: &str, timeout: Duration) -> Self {
        Self {
            origin: origin.trim_end_matches('/').to_owned(),
            timeout,
            max_body_bytes: MAX_CREDENTIAL_RESPONSE_BYTES,
        }
    }
}

impl CredentialExchange for UreqExchange {
    fn post(
        &mut self,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<HttpResponseBytes, ExchangeFailure> {
        let agent = ureq::AgentBuilder::new()
            .timeout(self.timeout)
            .redirects(0)
            .try_proxy_from_env(false)
            .build();
        let url = format!("{}{}", self.origin, path);
        let mut call = agent.post(&url);
        for (name, value) in headers {
            call = call.set(name, value);
        }
        match call.send_bytes(body) {
            Ok(response) => read_bounded(response, self.max_body_bytes),
            Err(ureq::Error::Status(status, response)) => {
                let mut response = read_bounded(response, self.max_body_bytes)?;
                response.status = status;
                Ok(response)
            }
            // `ErrorKind` is a closed enum and cannot contain the URL or a header.
            Err(ureq::Error::Transport(_)) => Err(ExchangeFailure::Transport),
        }
    }
}

fn read_bounded(
    response: ureq::Response,
    max_body_bytes: u64,
) -> Result<HttpResponseBytes, ExchangeFailure> {
    let status = response.status();
    let mut body = Vec::new();
    response
        .into_reader()
        .take(max_body_bytes.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|_| ExchangeFailure::Transport)?;
    if body.len() as u64 > max_body_bytes {
        return Err(ExchangeFailure::Oversized);
    }
    Ok(HttpResponseBytes { status, body })
}

/// What the startup acquisition did. Carries no secret value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialState {
    /// The explicit environment override is in use (no network, no store write).
    EnvironmentOverride,
    /// A stored credential was still outside the rotation margin.
    Stored,
    /// A legacy raw stored token (no deadline known) is in use.
    LegacyStored,
    /// The stored credential was renewed and replaced.
    Refreshed,
    /// The stored credential was renewed as needed; a transient failure kept it.
    RefreshDeferred,
    /// A new credential was issued and persisted.
    Issued,
}

/// A validated credential document. Never formatted with its values.
struct IssuedCredential {
    token: String,
    principal: String,
    expires_in: u64,
}

impl std::fmt::Debug for IssuedCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IssuedCredential")
            .field("token", &"<redacted>")
            .field("principal", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// A stored credential record with a known deadline.
struct StoredCredential {
    token: String,
    principal: String,
    expires_at: u64,
}

impl std::fmt::Debug for StoredCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredCredential")
            .field("token", &"<redacted>")
            .field("principal", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A stored value as read from the protected store.
enum StoredValue {
    Record(StoredCredential),
    Legacy(String),
}

impl StoredCredential {
    fn encode(&self) -> String {
        format!(
            "{STORED_VERSION}\n{}\n{}\n{}",
            self.expires_at, self.principal, self.token
        )
    }
}

/// Parse a stored value. `None` means a malformed versioned record; a value that
/// does not start with the version line is a legacy raw token.
fn decode_stored(raw: &str) -> Option<StoredValue> {
    if !raw.starts_with(STORED_VERSION) {
        if raw.is_empty() {
            return None;
        }
        return Some(StoredValue::Legacy(raw.to_owned()));
    }
    let mut lines = raw.split('\n');
    if lines.next() != Some(STORED_VERSION) {
        return None;
    }
    let expires_at = lines.next()?.parse::<u64>().ok()?;
    let principal = lines.next()?;
    let token = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    if !valid_principal(principal) || !valid_field(token, MIN_TOKEN_BYTES, MAX_TOKEN_BYTES) {
        return None;
    }
    Some(StoredValue::Record(StoredCredential {
        token: token.to_owned(),
        principal: principal.to_owned(),
        expires_at,
    }))
}

/// The bearer token a stored value currently supplies, if any.
pub(crate) fn stored_token(raw: &str) -> Option<String> {
    match decode_stored(raw) {
        Some(StoredValue::Record(record)) => Some(record.token),
        Some(StoredValue::Legacy(token)) => Some(token),
        None => None,
    }
}

/// Whether a stored value is a versioned record this module may revoke on
/// explicit teardown.
///
/// A bare operator override (environment token, stored verbatim) is never
/// revoked by the Desktop: it was provisioned by the operator and may be shared
/// with another tool, so only a credential this module issued or renewed (which
/// always carries a deadline record) is revoked on exit.
pub(crate) fn stored_is_versioned(raw: &str) -> bool {
    matches!(decode_stored(raw), Some(StoredValue::Record(_)))
}

/// Authenticated Desktop principal of a stored credential record.
///
/// Returns `None` for a legacy bare operator token or a malformed record: the
/// principal is never synthesized from local state. The media boundary validates
/// a descriptor's audience against this value, so an absent principal means
/// media acquisition is explicitly unavailable instead of trusting the
/// descriptor's own claim. Value-free failure surface: the caller may log the
/// absence, never the value.
pub(crate) fn stored_principal(raw: &str) -> Option<String> {
    match decode_stored(raw) {
        Some(StoredValue::Record(record)) => Some(record.principal),
        Some(StoredValue::Legacy(_)) | None => None,
    }
}

#[cfg(test)]
pub(crate) fn encode_for_test(token: &str, principal: &str, expires_at: u64) -> String {
    StoredCredential {
        token: token.to_owned(),
        principal: principal.to_owned(),
        expires_at,
    }
    .encode()
}

/// Normalize the operator secret: trimmed and empty values are absent.
fn normalize_secret(secret: Option<&str>) -> Option<&str> {
    secret.map(str::trim).filter(|value| !value.is_empty())
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn needs_refresh(expires_at: u64, now_seconds: u64) -> bool {
    now_seconds.saturating_add(REFRESH_MARGIN.as_secs()) >= expires_at
}

/// Obtain, rotate or leave alone the Runtime credential for one startup.
///
/// Precedence is unchanged: environment override > stored credential. When the
/// store is empty, a provisioned issuance secret is exchanged exactly once and
/// the result is persisted; when a stored record is inside the rotation margin it
/// is renewed before the immutable transport is built. The returned state never
/// contains the secret or the token.
pub(crate) fn prepare(
    origin: &str,
    env_token: Option<&str>,
    issuance_secret: Option<&str>,
    store: &dyn CredentialStore,
    exchange: &mut dyn CredentialExchange,
    now: SystemTime,
) -> Result<CredentialState, CredentialError> {
    if env_token
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(CredentialState::EnvironmentOverride);
    }
    let stored = store
        .load(origin)
        .map_err(|_| CredentialError::StoreUnavailable)?;
    match stored {
        Some(raw) => match decode_stored(&raw) {
            Some(StoredValue::Legacy(token)) => {
                if token.is_empty() {
                    Err(CredentialError::StoreUnavailable)
                } else {
                    Ok(CredentialState::LegacyStored)
                }
            }
            Some(StoredValue::Record(record)) => {
                let now_seconds = unix_seconds(now);
                if !needs_refresh(record.expires_at, now_seconds) {
                    return Ok(CredentialState::Stored);
                }
                match refresh(&record.token, exchange) {
                    Ok(document) => {
                        if let Err(error) = persist(store, origin, &document, now) {
                            // The rotation already invalidated the presented
                            // credential; revoke the replacement rather than leave
                            // a usable credential that nothing can read.
                            let _ = revoke(&document.token, exchange);
                            return Err(error);
                        }
                        Ok(CredentialState::Refreshed)
                    }
                    Err(error) => {
                        if error.requires_reauthorization() || now_seconds >= record.expires_at {
                            let _ = store.delete(origin);
                            return Err(if now_seconds >= record.expires_at {
                                CredentialError::Expired
                            } else {
                                error
                            });
                        }
                        // The credential is still inside its lifetime; a transient
                        // failure keeps it and the next session retries.
                        eprintln!("Runtime credential refresh deferred: {}", error.label());
                        Ok(CredentialState::RefreshDeferred)
                    }
                }
            }
            None => Err(CredentialError::StoreUnavailable),
        },
        None => {
            let Some(secret) = normalize_secret(issuance_secret) else {
                return Err(CredentialError::MissingSecret);
            };
            let document = issue(secret, exchange)?;
            if let Err(error) = persist(store, origin, &document, now) {
                // Never leave an issued credential usable when the protected
                // store could not take it: nothing would ever present or revoke
                // it again.
                let _ = revoke(&document.token, exchange);
                return Err(error);
            }
            Ok(CredentialState::Issued)
        }
    }
}

/// Outcome of one bounded reauthorization after a rejected credential.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// A new credential was issued or rotated and persisted. The token is
    /// returned only for the in-memory transport slot and is never rendered.
    Rotated(String),
    /// No usable credential exists; the caller must surface the authorization
    /// failure instead of retrying an unusable value.
    Unavailable,
    /// A transient exchange or store failure kept the existing record; a later
    /// bounded attempt may succeed.
    Transient,
}

/// Reauthorize one rejected Desktop credential inside a bounded exchange.
///
/// Precedence preserves the Desktop principal whenever possible: a versioned
/// record this module issued is refreshed in place (rotation keeps the same
/// server-bound principal), and only when that fails — for example after a
/// Runtime restart invalidated the process-local registry — is a new credential
/// issued from the operator issuance secret. An operator-provisioned bare token
/// is never refreshed or deleted here: it is not this module's credential and
/// the Runtime may still accept it under a fresh observation context.
pub(crate) fn recover(
    origin: &str,
    issuance_secret: Option<&str>,
    store: &dyn CredentialStore,
    exchange: &mut dyn CredentialExchange,
    now: SystemTime,
) -> Recovery {
    let stored = match store.load(origin) {
        Ok(value) => value,
        Err(_) => return Recovery::Transient,
    };
    let versioned = stored
        .as_deref()
        .and_then(|raw| stored_token(raw).map(|token| (token, stored_is_versioned(raw))));
    if let Some((token, true)) = versioned {
        match refresh(&token, exchange) {
            Ok(document) => {
                return match persist(store, origin, &document, now) {
                    Ok(()) => Recovery::Rotated(document.token),
                    Err(_) => {
                        // The rotation already invalidated the presented value;
                        // revoke the replacement instead of leaving a usable
                        // credential nothing can read.
                        let _ = revoke(&document.token, exchange);
                        Recovery::Transient
                    }
                };
            }
            Err(error) => {
                if error.requires_reauthorization() || matches!(error, CredentialError::Expired) {
                    // The Runtime no longer recognizes this credential (for
                    // example after a restart). Remove the dead record before
                    // attempting the issuance path.
                    let _ = store.delete(origin);
                } else {
                    return Recovery::Transient;
                }
            }
        }
    }
    let Some(secret) = normalize_secret(issuance_secret) else {
        return Recovery::Unavailable;
    };
    match issue(secret, exchange) {
        Ok(document) => match persist(store, origin, &document, now) {
            Ok(()) => Recovery::Rotated(document.token),
            Err(_) => {
                let _ = revoke(&document.token, exchange);
                Recovery::Transient
            }
        },
        Err(error) => {
            if error.requires_reauthorization() {
                Recovery::Unavailable
            } else {
                Recovery::Transient
            }
        }
    }
}

fn issue(
    secret: &str,
    exchange: &mut dyn CredentialExchange,
) -> Result<IssuedCredential, CredentialError> {
    let response = exchange
        .post(ISSUE_PATH, &[(SECRET_HEADER, secret)], b"")
        .map_err(map_exchange)?;
    if response.status != 200 {
        return Err(CredentialError::from_status(response.status));
    }
    parse_document(&response.body)
}

fn refresh(
    token: &str,
    exchange: &mut dyn CredentialExchange,
) -> Result<IssuedCredential, CredentialError> {
    let authorization = format!("{BEARER_PREFIX}{token}");
    let response = exchange
        .post(
            REFRESH_PATH,
            &[(AUTHORIZATION_HEADER, authorization.as_str())],
            b"",
        )
        .map_err(map_exchange)?;
    if response.status != 200 {
        return Err(CredentialError::from_status(response.status));
    }
    parse_document(&response.body)
}

/// Revoke one credential. Best-effort by design: the caller bounds the budget
/// and treats failure as non-fatal.
pub(crate) fn revoke(
    token: &str,
    exchange: &mut dyn CredentialExchange,
) -> Result<(), CredentialError> {
    let authorization = format!("{BEARER_PREFIX}{token}");
    let response = exchange
        .post(
            REVOKE_PATH,
            &[(AUTHORIZATION_HEADER, authorization.as_str())],
            b"",
        )
        .map_err(map_exchange)?;
    if response.status != 200 {
        return Err(CredentialError::from_status(response.status));
    }
    Ok(())
}

fn map_exchange(failure: ExchangeFailure) -> CredentialError {
    match failure {
        ExchangeFailure::Transport => CredentialError::TransportFailure,
        ExchangeFailure::Oversized => CredentialError::OversizedResponse,
    }
}

fn persist(
    store: &dyn CredentialStore,
    origin: &str,
    document: &IssuedCredential,
    now: SystemTime,
) -> Result<(), CredentialError> {
    let expires_at = unix_seconds(now).saturating_add(document.expires_in);
    let record = StoredCredential {
        token: document.token.clone(),
        principal: document.principal.clone(),
        expires_at,
    };
    store
        .store(origin, &record.encode())
        .map_err(|_| CredentialError::StoreUnavailable)
}

/// A minimal closed-shape JSON reader for the credential document.
///
/// It accepts exactly the four documented keys with string/array/integer values,
/// rejects duplicate keys, unknown keys, escapes, non-ASCII text and trailing
/// data, and never formats the input. That keeps the accepted shape auditable
/// without adding a JSON dependency.
struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            position: 0,
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn expect(&mut self, expected: u8) -> Result<(), CredentialError> {
        if self.bump() == Some(expected) {
            Ok(())
        } else {
            Err(CredentialError::MalformedResponse)
        }
    }

    fn string(&mut self) -> Result<String, CredentialError> {
        self.expect(b'"')?;
        let mut value = String::new();
        loop {
            match self.bump() {
                // The closing quote is checked before the visible-ASCII range:
                // `"` itself lies inside 0x21..=0x7e.
                Some(b'"') => return Ok(value),
                // Visible ASCII only: no escapes, no control/space characters.
                Some(byte) if (0x21..=0x7e).contains(&byte) => value.push(char::from(byte)),
                _ => return Err(CredentialError::MalformedResponse),
            }
        }
    }

    fn digits(&mut self) -> Result<u64, CredentialError> {
        let start = self.position;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
            self.position += 1;
        }
        if self.position == start {
            return Err(CredentialError::MalformedResponse);
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| CredentialError::MalformedResponse)?;
        text.parse::<u64>()
            .map_err(|_| CredentialError::MalformedResponse)
    }

    fn string_array(&mut self) -> Result<Vec<String>, CredentialError> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(values);
        }
        loop {
            self.skip_whitespace();
            values.push(self.string()?);
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => return Ok(values),
                _ => return Err(CredentialError::MalformedResponse),
            }
        }
    }
}

fn valid_field(value: &str, minimum: usize, maximum: usize) -> bool {
    let length = value.len();
    (minimum..=maximum).contains(&length) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

/// Pin the server-managed principal shape: `desktop:` plus 32 lowercase hex.
///
/// The Client only validates and compares this value; it never renders it, so a
/// principal that does not have the documented shape is a closed-document
/// failure rather than text to display.
fn valid_principal(value: &str) -> bool {
    value.strip_prefix(PRINCIPAL_PREFIX).is_some_and(|suffix| {
        suffix.len() == PRINCIPAL_SUFFIX_LEN
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn parse_document(raw: &[u8]) -> Result<IssuedCredential, CredentialError> {
    if raw.is_empty() {
        return Err(CredentialError::MalformedResponse);
    }
    if raw.len() as u64 > MAX_CREDENTIAL_RESPONSE_BYTES {
        return Err(CredentialError::OversizedResponse);
    }
    let text = std::str::from_utf8(raw).map_err(|_| CredentialError::MalformedResponse)?;
    let mut cursor = Cursor::new(text);
    let mut token = None;
    let mut principal = None;
    let mut scopes = None;
    let mut expires_in = None;

    cursor.skip_whitespace();
    cursor.expect(b'{')?;
    cursor.skip_whitespace();
    if cursor.peek() == Some(b'}') {
        cursor.position += 1;
    } else {
        loop {
            cursor.skip_whitespace();
            let key = cursor.string()?;
            cursor.skip_whitespace();
            cursor.expect(b':')?;
            cursor.skip_whitespace();
            match key.as_str() {
                "token" => {
                    if token.is_some() {
                        return Err(CredentialError::MalformedResponse);
                    }
                    token = Some(cursor.string()?);
                }
                "principal" => {
                    if principal.is_some() {
                        return Err(CredentialError::MalformedResponse);
                    }
                    principal = Some(cursor.string()?);
                }
                "scopes" => {
                    if scopes.is_some() {
                        return Err(CredentialError::MalformedResponse);
                    }
                    scopes = Some(cursor.string_array()?);
                }
                "expires_in" => {
                    if expires_in.is_some() {
                        return Err(CredentialError::MalformedResponse);
                    }
                    expires_in = Some(cursor.digits()?);
                }
                // The document shape is closed: an unknown key means the peer
                // changed the contract and this build must not guess.
                _ => return Err(CredentialError::MalformedResponse),
            }
            cursor.skip_whitespace();
            match cursor.bump() {
                Some(b',') => continue,
                Some(b'}') => break,
                _ => return Err(CredentialError::MalformedResponse),
            }
        }
    }
    cursor.skip_whitespace();
    if cursor.position != cursor.bytes.len() {
        return Err(CredentialError::MalformedResponse);
    }

    let token = token.ok_or(CredentialError::MalformedResponse)?;
    let principal = principal.ok_or(CredentialError::MalformedResponse)?;
    let scopes = scopes.ok_or(CredentialError::MalformedResponse)?;
    let expires_in = expires_in.ok_or(CredentialError::MalformedResponse)?;
    if !valid_field(&token, MIN_TOKEN_BYTES, MAX_TOKEN_BYTES) {
        return Err(CredentialError::MalformedResponse);
    }
    if !valid_principal(&principal) {
        return Err(CredentialError::MalformedResponse);
    }
    if scopes.len() != EXPECTED_SCOPES.len()
        || scopes
            .iter()
            .zip(EXPECTED_SCOPES)
            .any(|(actual, expected)| actual != expected)
    {
        return Err(CredentialError::MalformedResponse);
    }
    if !(1..=MAX_EXPIRES_IN_SECONDS).contains(&expires_in) {
        return Err(CredentialError::MalformedResponse);
    }
    Ok(IssuedCredential {
        token,
        principal,
        expires_in,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialError, CredentialExchange, CredentialState, ExchangeFailure, HttpResponseBytes,
        ISSUANCE_SECRET_ENV, ISSUE_PATH, MAX_CREDENTIAL_RESPONSE_BYTES, REFRESH_MARGIN,
        REFRESH_PATH, REVOKE_PATH, Recovery, UreqExchange, encode_for_test, parse_document,
        prepare, revoke, stored_principal, stored_token,
    };
    use crate::credentials::{CredentialStore, CredentialStoreError};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    const ORIGIN: &str = "http://127.0.0.1:8123";
    const PRINCIPAL: &str = "desktop:0123456789abcdef0123456789abcdef";
    const TOKEN: &str = "sentinel-token-0123456789abcdef";
    const NOW: u64 = 1_700_000_000;

    const TWO_SCOPES: &str = "[\"read:observe\",\"desktop:command\"]";

    fn document_with(token: &str, scopes: &str, expires_in: u64) -> Vec<u8> {
        format!(
            "{{\"token\":\"{token}\",\"principal\":\"{PRINCIPAL}\",\"scopes\":{scopes},\"expires_in\":{expires_in}}}"
        )
        .into_bytes()
    }

    fn document_with_principal(document: &str) -> Vec<u8> {
        document.as_bytes().to_vec()
    }

    fn document(token: &str) -> Vec<u8> {
        document_with(token, TWO_SCOPES, 43_200)
    }

    #[derive(Default)]
    struct MemoryStore {
        entry: Mutex<Option<String>>,
        stores: Mutex<Vec<String>>,
        loads: AtomicUsize,
        deletes: AtomicUsize,
        fail_load: bool,
        fail_store: bool,
    }

    impl MemoryStore {
        fn with_value(value: &str) -> Self {
            Self {
                entry: Mutex::new(Some(value.to_owned())),
                ..Self::default()
            }
        }

        fn value(&self) -> Option<String> {
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
            Ok(self.value())
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
            *self.entry.lock().expect("memory store") = None;
            Ok(())
        }
    }

    struct Request {
        path: String,
        header_names: Vec<String>,
    }

    struct ScriptedExchange {
        responses: VecDeque<Result<HttpResponseBytes, ExchangeFailure>>,
        requests: Vec<Request>,
    }

    impl ScriptedExchange {
        fn new(responses: impl IntoIterator<Item = (u16, Vec<u8>)>) -> Self {
            Self {
                responses: responses
                    .into_iter()
                    .map(|(status, body)| Ok(HttpResponseBytes { status, body }))
                    .collect(),
                requests: Vec::new(),
            }
        }

        fn failing(failure: ExchangeFailure) -> Self {
            let mut exchange = Self::new([]);
            exchange.responses.push_back(Err(failure));
            exchange
        }

        fn requests(&self) -> &[Request] {
            &self.requests
        }
    }

    impl CredentialExchange for ScriptedExchange {
        fn post(
            &mut self,
            path: &str,
            headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<HttpResponseBytes, ExchangeFailure> {
            self.requests.push(Request {
                path: path.to_owned(),
                header_names: headers
                    .iter()
                    .map(|(name, _value)| (*name).to_owned())
                    .collect(),
            });
            self.responses
                .pop_front()
                .unwrap_or(Err(ExchangeFailure::Transport))
        }
    }

    fn now(seconds: u64) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn issuance_persists_once_behind_the_secret_header() {
        let store = MemoryStore::default();
        let mut exchange = ScriptedExchange::new([(200, document(TOKEN))]);
        let state = prepare(
            ORIGIN,
            None,
            Some("operator-secret"),
            &store,
            &mut exchange,
            now(NOW),
        )
        .expect("issuance succeeds");
        assert_eq!(state, CredentialState::Issued);
        assert_eq!(store.store_calls().len(), 1);
        let stored = store.value().expect("credential is stored");
        assert_eq!(stored_token(&stored).as_deref(), Some(TOKEN));
        let requests = exchange.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, ISSUE_PATH);
        assert!(
            requests[0]
                .header_names
                .iter()
                .any(|name| name == "X-Auth-Secret")
        );
    }

    #[test]
    fn failed_statuses_map_to_fixed_labels_and_never_write_the_store() {
        for (status, expected) in [
            (401, CredentialError::Unauthorized),
            (403, CredentialError::Forbidden),
            (422, CredentialError::Rejected),
            (503, CredentialError::ServiceUnavailable),
            (418, CredentialError::UnexpectedStatus),
        ] {
            let store = MemoryStore::default();
            let mut exchange = ScriptedExchange::new([(status, b"{\"detail\":\"no\"}".to_vec())]);
            let error = prepare(
                ORIGIN,
                None,
                Some("operator-secret"),
                &store,
                &mut exchange,
                now(NOW),
            )
            .expect_err("status is refused");
            assert_eq!(error, expected, "status {status}");
            assert!(
                store.store_calls().is_empty(),
                "status {status} writes nothing"
            );
            let rendered = format!("{error} {error:?}");
            assert!(!rendered.contains(TOKEN));
            assert!(!rendered.contains("127.0.0.1"));
            assert!(!rendered.contains(PRINCIPAL));
        }
    }

    #[test]
    fn oversized_and_transport_failures_are_refused_without_a_write() {
        let store = MemoryStore::default();
        let oversized = vec![b'x'; (MAX_CREDENTIAL_RESPONSE_BYTES + 1) as usize];
        let mut exchange = ScriptedExchange::new([(200, oversized)]);
        assert_eq!(
            prepare(ORIGIN, None, Some("s"), &store, &mut exchange, now(NOW))
                .expect_err("oversized response"),
            CredentialError::OversizedResponse
        );
        assert!(store.store_calls().is_empty());

        let mut failing = ScriptedExchange::failing(ExchangeFailure::Transport);
        assert_eq!(
            prepare(ORIGIN, None, Some("s"), &store, &mut failing, now(NOW))
                .expect_err("transport failure"),
            CredentialError::TransportFailure
        );
        assert!(store.store_calls().is_empty());
    }

    #[test]
    fn malformed_documents_are_refused_without_a_write() {
        let bodies: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"{".to_vec(),
            b"[]".to_vec(),
            b"{\"token\":\"sentinel-token-0123456789abcdef\"}".to_vec(),
            document_with(TOKEN, "[\"read:write\"]", 43_200),
            document_with(TOKEN, "[]", 43_200),
            document_with(TOKEN, TWO_SCOPES, 0),
            document_with(TOKEN, TWO_SCOPES, 86_401),
            document_with("short", TWO_SCOPES, 43_200),
            document_with(TOKEN, TWO_SCOPES, 43_200)
                .into_iter()
                .chain(b"x".iter().copied())
                .collect(),
            format!(
                "{{\"token\":\"{TOKEN}\",\"principal\":\"desktop:0123456789abcdef0123456789abcdef\",\"token\":\"{TOKEN}\",\"expires_in\":43200}}"
            )
            .into_bytes(),
            format!(
                "{{\"token\":\"{TOKEN}\",\"principal\":\"desktop:0123456789abcdef0123456789abcdef\",\"scopes\":[\"read:observe\",\"desktop:command\"],\"expires_in\":43200,\"extra\":1}}"
            )
            .into_bytes(),
            format!(
                "{{\"token\":\"{TOKEN}\",\"principal\":\"{PRINCIPAL}\",\"scopes\":[\"read:observe\",\"desktop:command\"],\"expires_in\":43200}} trailing"
            )
            .into_bytes(),
            document_with("with space in token", TWO_SCOPES, 43_200),
            // Scope shape: one scope only (missing desktop:command).
            document_with(TOKEN, "[\"read:observe\"]", 43_200),
            // Scope shape: the command scope alone.
            document_with(TOKEN, "[\"desktop:command\"]", 43_200),
            // Scope shape: an extra scope.
            document_with(
                TOKEN,
                "[\"read:observe\",\"desktop:command\",\"desktop:admin\"]",
                43_200,
            ),
            // Scope shape: a duplicate.
            document_with(
                TOKEN,
                "[\"read:observe\",\"desktop:command\",\"desktop:command\"]",
                43_200,
            ),
            // Scope shape: reversed order is not the documented sequence.
            document_with(
                TOKEN,
                "[\"desktop:command\",\"read:observe\"]",
                43_200,
            ),
            document_with_principal(&format!(
                "{{\"token\":\"{TOKEN}\",\"principal\":\"desktop:0123456789ABCDEF0123456789ABCDEF\",\"scopes\":[\"read:observe\",\"desktop:command\"],\"expires_in\":43200}}"
            )),
            document_with_principal(&format!(
                "{{\"token\":\"{TOKEN}\",\"principal\":\"desktop:0123456789abcdef0123456789abcde\",\"scopes\":[\"read:observe\",\"desktop:command\"],\"expires_in\":43200}}"
            )),
            document_with_principal(&format!(
                "{{\"token\":\"{TOKEN}\",\"principal\":\"desktop:0123456789abcdef0123456789abcdeg\",\"scopes\":[\"read:observe\",\"desktop:command\"],\"expires_in\":43200}}"
            )),
            document_with_principal(&format!(
                "{{\"token\":\"{TOKEN}\",\"principal\":\"principal-without-the-prefix\",\"scopes\":[\"read:observe\",\"desktop:command\"],\"expires_in\":43200}}"
            )),
        ];
        for body in bodies {
            let store = MemoryStore::default();
            let mut exchange = ScriptedExchange::new([(200, body)]);
            let error = prepare(ORIGIN, None, Some("s"), &store, &mut exchange, now(NOW))
                .expect_err("malformed response is refused");
            assert_eq!(error, CredentialError::MalformedResponse);
            assert!(store.store_calls().is_empty());
        }
    }

    #[test]
    fn a_record_inside_the_margin_is_rotated_and_replaced() {
        let stale = TOKEN;
        let fresh = "sentinel-token-fedcba9876543210";
        let store = MemoryStore::with_value(&encode_for_test(
            stale,
            PRINCIPAL,
            NOW + REFRESH_MARGIN.as_secs(),
        ));
        let mut exchange = ScriptedExchange::new([(200, document(fresh))]);
        let state = prepare(ORIGIN, None, None, &store, &mut exchange, now(NOW))
            .expect("rotation succeeds");
        assert_eq!(state, CredentialState::Refreshed);
        assert_eq!(exchange.requests().len(), 1);
        assert_eq!(exchange.requests()[0].path, REFRESH_PATH);
        assert!(
            exchange.requests()[0]
                .header_names
                .iter()
                .any(|name| name == "Authorization")
        );
        assert_eq!(store.store_calls().len(), 1);
        let stored = store.value().expect("rotated record is stored");
        assert_eq!(stored_token(&stored).as_deref(), Some(fresh));
        assert!(!stored.contains(stale), "the old token is replaced");
    }

    #[test]
    fn a_transient_refresh_failure_before_the_deadline_keeps_the_record() {
        let record = encode_for_test(TOKEN, PRINCIPAL, NOW + 60);
        let store = MemoryStore::with_value(&record);
        let mut exchange = ScriptedExchange::new([(503, b"{\"detail\":\"busy\"}".to_vec())]);
        let state = prepare(ORIGIN, None, None, &store, &mut exchange, now(NOW))
            .expect("deferred refresh keeps the credential");
        assert_eq!(state, CredentialState::RefreshDeferred);
        assert!(store.store_calls().is_empty());
        assert_eq!(store.delete_calls(), 0);
        assert_eq!(store.value().as_deref(), Some(record.as_str()));
    }

    #[test]
    fn a_rejected_or_expired_rotation_deletes_the_record() {
        let rejected = MemoryStore::with_value(&encode_for_test(TOKEN, PRINCIPAL, NOW + 60));
        let mut exchange = ScriptedExchange::new([(401, b"{}".to_vec())]);
        assert_eq!(
            prepare(ORIGIN, None, None, &rejected, &mut exchange, now(NOW))
                .expect_err("rejected refresh"),
            CredentialError::Unauthorized
        );
        assert_eq!(rejected.delete_calls(), 1);

        let expired = MemoryStore::with_value(&encode_for_test(TOKEN, PRINCIPAL, NOW - 1));
        let mut exchange = ScriptedExchange::new([(503, b"{}".to_vec())]);
        assert_eq!(
            prepare(ORIGIN, None, None, &expired, &mut exchange, now(NOW))
                .expect_err("expired credential"),
            CredentialError::Expired
        );
        assert_eq!(expired.delete_calls(), 1);
    }

    #[test]
    fn a_stored_record_outside_the_margin_uses_the_store_without_network() {
        let record = encode_for_test(TOKEN, PRINCIPAL, NOW + 3_600);
        let store = MemoryStore::with_value(&record);
        let mut exchange = ScriptedExchange::new([]);
        let state = prepare(
            ORIGIN,
            None,
            Some("unused"),
            &store,
            &mut exchange,
            now(NOW),
        )
        .expect("stored credential resolves");
        assert_eq!(state, CredentialState::Stored);
        assert!(exchange.requests().is_empty());
        assert!(store.store_calls().is_empty());
    }

    #[test]
    fn a_legacy_raw_value_is_used_but_never_rotated() {
        let store = MemoryStore::with_value("legacy-operator-token");
        let mut exchange = ScriptedExchange::new([]);
        let state = prepare(ORIGIN, None, None, &store, &mut exchange, now(NOW))
            .expect("legacy value resolves");
        assert_eq!(state, CredentialState::LegacyStored);
        assert!(exchange.requests().is_empty());
    }

    #[test]
    fn the_environment_override_short_circuits_acquisition() {
        let store = MemoryStore::default();
        let mut exchange = ScriptedExchange::new([]);
        let state = prepare(
            ORIGIN,
            Some("override-token"),
            Some("operator-secret"),
            &store,
            &mut exchange,
            now(NOW),
        )
        .expect("override wins");
        assert_eq!(state, CredentialState::EnvironmentOverride);
        assert_eq!(store.load_calls(), 0);
        assert!(exchange.requests().is_empty());
    }

    #[test]
    fn a_missing_secret_and_empty_store_is_explicitly_unavailable() {
        let store = MemoryStore::default();
        let mut exchange = ScriptedExchange::new([]);
        assert_eq!(
            prepare(ORIGIN, None, None, &store, &mut exchange, now(NOW)).expect_err("no secret"),
            CredentialError::MissingSecret
        );
        assert_eq!(
            prepare(ORIGIN, None, Some("   "), &store, &mut exchange, now(NOW))
                .expect_err("blank secret"),
            CredentialError::MissingSecret
        );
        assert!(exchange.requests().is_empty());
        let unavailable = MemoryStore {
            fail_load: true,
            ..MemoryStore::default()
        };
        assert_eq!(
            prepare(
                ORIGIN,
                None,
                Some("s"),
                &unavailable,
                &mut exchange,
                now(NOW)
            )
            .expect_err("store failure"),
            CredentialError::StoreUnavailable
        );
    }

    #[test]
    fn a_failed_store_write_revokes_the_orphan_credential() {
        let store = MemoryStore {
            fail_store: true,
            ..MemoryStore::default()
        };
        let mut exchange = ScriptedExchange::new([
            (200, document(TOKEN)),
            (200, b"{\"revoked\":true}".to_vec()),
        ]);
        let error = prepare(ORIGIN, None, Some("s"), &store, &mut exchange, now(NOW))
            .expect_err("store failure is explicit");
        assert_eq!(error, CredentialError::StoreUnavailable);
        let paths: Vec<&str> = exchange
            .requests()
            .iter()
            .map(|request| request.path.as_str())
            .collect();
        assert_eq!(paths, vec![ISSUE_PATH, REVOKE_PATH]);
    }

    #[test]
    fn a_failed_store_write_after_rotation_revokes_the_replacement() {
        let record = encode_for_test(TOKEN, PRINCIPAL, NOW + 60);
        let mut store = MemoryStore::with_value(&record);
        store.fail_store = true;
        let fresh = "sentinel-token-fedcba9876543210";
        let mut exchange = ScriptedExchange::new([
            (200, document(fresh)),
            (200, b"{\"revoked\":true}".to_vec()),
        ]);
        let error = prepare(ORIGIN, None, None, &store, &mut exchange, now(NOW))
            .expect_err("store failure is explicit");
        assert_eq!(error, CredentialError::StoreUnavailable);
        let paths: Vec<&str> = exchange
            .requests()
            .iter()
            .map(|request| request.path.as_str())
            .collect();
        assert_eq!(paths, vec![REFRESH_PATH, REVOKE_PATH]);
    }

    #[test]
    fn revoke_is_best_effort_and_expresses_only_a_fixed_label() {
        let mut success = ScriptedExchange::new([(200, b"{\"revoked\":true}".to_vec())]);
        revoke(TOKEN, &mut success).expect("revoke succeeds");
        assert_eq!(success.requests()[0].path, REVOKE_PATH);
        assert!(
            success.requests()[0]
                .header_names
                .iter()
                .any(|name| name == "Authorization")
        );

        let mut rejected = ScriptedExchange::new([(401, b"{}".to_vec())]);
        assert_eq!(
            revoke(TOKEN, &mut rejected).expect_err("revoke refused"),
            CredentialError::Unauthorized
        );
        let mut broken = ScriptedExchange::failing(ExchangeFailure::Transport);
        assert_eq!(
            revoke(TOKEN, &mut broken).expect_err("transport failure"),
            CredentialError::TransportFailure
        );
    }

    #[test]
    fn stored_values_round_trip_and_malformed_records_are_not_tokens() {
        let encoded = encode_for_test(TOKEN, PRINCIPAL, NOW);
        assert_eq!(stored_token(&encoded).as_deref(), Some(TOKEN));
        assert_eq!(
            stored_token("legacy-token"),
            Some("legacy-token".to_owned())
        );
        assert_eq!(stored_token(""), None);
        assert_eq!(
            stored_token("cos-desktop-credential-v1\nnot-a-number\nx\ny"),
            None
        );
        assert_eq!(
            stored_token(&format!("cos-desktop-credential-v1\n{NOW}\nbad\n{TOKEN}")),
            None
        );
        // The parsed document's Debug rendering never shows the token.
        let parsed = parse_document(&document(TOKEN)).expect("document parses");
        let rendered = format!("{parsed:?}");
        assert!(!rendered.contains(TOKEN));
        assert!(!rendered.contains(PRINCIPAL));
        assert!(rendered.contains("<redacted>"));
        assert_eq!(parsed.expires_in, 43_200);
    }

    #[test]
    fn the_two_scope_document_shape_is_pinned() {
        let accepted = "[\"read:observe\",\"desktop:command\"]";
        for scopes in [
            accepted,
            "[\"read:observe\"]",
            "[\"desktop:command\"]",
            "[\"read:observe\",\"desktop:command\",\"desktop:admin\"]",
            "[\"read:observe\",\"read:observe\"]",
            "[\"desktop:command\",\"read:observe\"]",
            "[]",
        ] {
            let document = document_with(TOKEN, scopes, 43_200);
            let parsed = parse_document(&document);
            if scopes == accepted {
                assert_eq!(
                    parsed
                        .expect("the two-scope document is accepted")
                        .expires_in,
                    43_200
                );
            } else {
                assert_eq!(
                    parsed.err(),
                    Some(CredentialError::MalformedResponse),
                    "{scopes} must be refused"
                );
            }
        }
    }

    #[test]
    fn a_stored_record_must_carry_the_documented_principal_shape() {
        for principal in [
            "desktop:0123456789ABCDEF0123456789ABCDEF",
            "desktop:0123456789abcdef0123456789abcde",
            "desktop:0123456789abcdef0123456789abcdef0",
            "desktop:0123456789abcdef0123456789abcdeg",
            "desktop:",
            "plain-principal",
        ] {
            let record = encode_for_test(TOKEN, principal, NOW);
            assert_eq!(stored_token(&record), None, "{principal} must be refused");
        }
        assert_eq!(
            stored_token(&encode_for_test(TOKEN, PRINCIPAL, NOW)).as_deref(),
            Some(TOKEN)
        );
    }

    #[test]
    fn the_stored_principal_is_exposed_only_for_a_versioned_record() {
        let record = encode_for_test(TOKEN, PRINCIPAL, NOW);
        assert_eq!(stored_principal(&record).as_deref(), Some(PRINCIPAL));
        // A bare operator override has no authenticated principal to expose.
        assert_eq!(stored_principal("bare-operator-token-value"), None);
        assert_eq!(stored_principal(""), None);
        // A malformed record never yields an invented principal.
        assert_eq!(
            stored_principal(&format!("cos-desktop-credential-v1\n{NOW}\nbad\n{TOKEN}")),
            None
        );
    }

    #[test]
    fn every_error_rendering_is_value_free() {
        let errors = [
            CredentialError::MissingSecret,
            CredentialError::Unauthorized,
            CredentialError::Forbidden,
            CredentialError::ServiceUnavailable,
            CredentialError::Rejected,
            CredentialError::UnexpectedStatus,
            CredentialError::OversizedResponse,
            CredentialError::MalformedResponse,
            CredentialError::TransportFailure,
            CredentialError::StoreUnavailable,
            CredentialError::Expired,
        ];
        for error in errors {
            let rendered = format!("{error} {error:?} {}", error.label());
            for needle in [TOKEN, PRINCIPAL, "127.0.0.1", "desktop:", "OS error"] {
                assert!(
                    !rendered.contains(needle),
                    "{needle} must not appear in an error rendering"
                );
            }
        }
    }

    #[test]
    fn recovery_refreshes_an_issued_record_before_issuing_a_new_principal() {
        let store = MemoryStore::with_value(&encode_for_test(TOKEN, PRINCIPAL, NOW + 3600));
        let mut exchange = ScriptedExchange::new([(200, document("rotated-token-0123456789"))]);
        let outcome = super::recover(
            ORIGIN,
            Some("operator-secret"),
            &store,
            &mut exchange,
            now(NOW),
        );
        assert_eq!(
            outcome,
            Recovery::Rotated("rotated-token-0123456789".to_owned())
        );
        let requests = exchange.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].path, REFRESH_PATH,
            "rotation keeps the principal"
        );
        assert_eq!(
            stored_token(&store.value().expect("rotated record")).as_deref(),
            Some("rotated-token-0123456789")
        );
    }

    #[test]
    fn recovery_issues_a_new_credential_when_the_runtime_no_longer_knows_the_record() {
        let store = MemoryStore::with_value(&encode_for_test(TOKEN, PRINCIPAL, NOW + 3600));
        let mut exchange = ScriptedExchange::new([
            (401, Vec::new()),
            (200, document("reissued-token-0123456789")),
        ]);
        let outcome = super::recover(
            ORIGIN,
            Some("operator-secret"),
            &store,
            &mut exchange,
            now(NOW),
        );
        assert_eq!(
            outcome,
            Recovery::Rotated("reissued-token-0123456789".to_owned())
        );
        let requests = exchange.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, REFRESH_PATH);
        assert_eq!(requests[1].path, ISSUE_PATH);
        assert_eq!(store.delete_calls(), 1, "the dead record is cleared");
        assert_eq!(
            stored_token(&store.value().expect("reissued record")).as_deref(),
            Some("reissued-token-0123456789")
        );
    }

    #[test]
    fn recovery_never_reissues_without_a_provisioned_secret() {
        let store = MemoryStore::with_value(&encode_for_test(TOKEN, PRINCIPAL, NOW + 3600));
        let mut exchange = ScriptedExchange::new([(401, Vec::new())]);
        let outcome = super::recover(ORIGIN, None, &store, &mut exchange, now(NOW));
        assert_eq!(outcome, Recovery::Unavailable);
        assert_eq!(exchange.requests().len(), 1);
        assert_eq!(store.delete_calls(), 1);
        assert_eq!(store.value(), None);
    }

    #[test]
    fn recovery_never_rotates_or_deletes_an_operator_provisioned_token() {
        let store = MemoryStore::with_value("operator-token-0123456789");
        let mut exchange = ScriptedExchange::new([]);
        let outcome = super::recover(ORIGIN, None, &store, &mut exchange, now(NOW));
        assert_eq!(outcome, Recovery::Unavailable);
        assert!(exchange.requests().is_empty());
        assert_eq!(store.delete_calls(), 0);
        assert_eq!(store.value().as_deref(), Some("operator-token-0123456789"));
    }

    #[test]
    fn a_transient_recovery_failure_keeps_the_record_for_a_later_attempt() {
        let store = MemoryStore::with_value(&encode_for_test(TOKEN, PRINCIPAL, NOW + 3600));
        let mut exchange = ScriptedExchange::new([(503, Vec::new())]);
        let outcome = super::recover(ORIGIN, None, &store, &mut exchange, now(NOW));
        assert_eq!(outcome, Recovery::Transient);
        assert_eq!(store.delete_calls(), 0);
        assert_eq!(
            store_value_token(&store),
            Some(TOKEN.to_owned()),
            "the record survives a transient refusal"
        );
    }

    fn store_value_token(store: &MemoryStore) -> Option<String> {
        store.value().as_deref().and_then(stored_token)
    }

    #[test]
    #[ignore = "requires a live loopback Runtime and COS_DESKTOP_RUNTIME_ISSUANCE_SECRET"]
    fn live_exchange_contract_round_trip() {
        let origin = std::env::var("COS_DESKTOP_RUNTIME_ENDPOINT").expect("endpoint env");
        let secret = std::env::var(ISSUANCE_SECRET_ENV).expect("issuance secret env");
        let clock = || std::time::SystemTime::now();
        let store = MemoryStore::default();
        let mut exchange = UreqExchange::new(&origin);
        let state = prepare(&origin, None, Some(&secret), &store, &mut exchange, clock())
            .expect("live issuance");
        assert_eq!(state, CredentialState::Issued);
        let stored = store.value().expect("record is stored");
        assert_eq!(stored.split('\n').count(), 4);
        let token = stored_token(&stored).expect("record carries a token");

        // Rotation through the same runtime boundary, using a record that is
        // already inside the margin.
        let expires_at = clock()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs();
        let rotating = MemoryStore::with_value(&encode_for_test(&token, PRINCIPAL, expires_at));
        let state =
            prepare(&origin, None, None, &rotating, &mut exchange, clock()).expect("live rotation");
        assert_eq!(state, CredentialState::Refreshed);
        let rotated = stored_token(&rotating.value().expect("rotated record"))
            .expect("rotated record carries a token");
        super::revoke(&rotated, &mut exchange).expect("live revoke");
    }
}
