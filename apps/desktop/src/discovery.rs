//! RT8 / RC-16: per-user Runtime discovery descriptor.
//!
//! When `COS_DESKTOP_RUNTIME_ENDPOINT` is absent, the composition root reads
//! one closed discovery descriptor published by the Runtime after readiness:
//!
//! - Windows: `%LOCALAPPDATA%\COS\runtime.json`
//! - other platforms: `$XDG_STATE_HOME/cos/runtime.json`, otherwise
//!   `$HOME/.local/state/cos/runtime.json`
//! - test/alternate root: `COS_DESKTOP_RUNTIME_DISCOVERY_PATH` names the exact
//!   file and wins over both defaults.
//!
//! The descriptor is untrusted input: the file is bounded, the schema is closed
//! (`deny_unknown_fields`), every field is range-checked, the Runtime must be
//! `ready`, and the origin must be a plain-HTTP **loopback IP literal** accepted
//! by [`crate::runtime::validated_origin`]. Anything else is an explicit
//! unavailable state; it is never a fixture fallback and never an endpoint
//! override. A descriptor left behind by a crashed Runtime is structurally
//! valid but useless: the bounded connection attempt fails and the surface
//! stays unavailable until the Runtime publishes a fresh descriptor.
//!
//! The descriptor carries no credential, secret or principal. `server_identity`
//! is informational here: the authenticated channel identity still comes from
//! the negotiated handshake and is verified by the transport, so a descriptor
//! cannot upgrade an untrusted server.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Exact descriptor path override (tests and alternate service roots).
pub(crate) const DISCOVERY_PATH_ENV: &str = "COS_DESKTOP_RUNTIME_DISCOVERY_PATH";

/// The only descriptor schema this build understands.
pub(crate) const DISCOVERY_SCHEMA_VERSION: u32 = 1;

/// Bounded descriptor document size.
pub(crate) const MAX_DESCRIPTOR_BYTES: u64 = 16 * 1024;

/// Maximum accepted identity/epoch/started-at text.
const MAX_TEXT_BYTES: usize = 128;
/// Maximum accepted origin text.
const MAX_ORIGIN_BYTES: usize = 256;
/// The protocol pair this Desktop build speaks.
const PROTOCOL_MAJOR: u16 = 2;
const PROTOCOL_MINOR: u16 = 0;

/// Why discovery produced no usable Runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscoveryError {
    /// No discovery path is available (no override and no per-user root).
    PathUnavailable,
    /// The descriptor file does not exist.
    NotPublished,
    /// The descriptor could not be opened or is not a regular file.
    Unreadable,
    /// The descriptor exceeds the bounded size.
    Oversized,
    /// The document is not the closed schema.
    Malformed,
    /// A descriptor schema version this build cannot read.
    UnsupportedSchema,
    /// The origin is not a plain-HTTP loopback IP literal.
    UnsupportedOrigin,
    /// The advertised protocol/schema range does not include this build's pair.
    UnsupportedRange,
    /// The Runtime has not reached readiness.
    NotReady,
    /// `server_identity` is empty, oversized or carries non-printable bytes.
    InvalidIdentity,
    /// `epoch` is empty, oversized or carries non-printable bytes.
    InvalidEpoch,
    /// `pid` is zero.
    InvalidPid,
    /// `started_at` is empty, oversized or carries non-printable bytes.
    InvalidStartTime,
}

impl DiscoveryError {
    /// Fixed, value-free operator label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::PathUnavailable => "runtime discovery path is unavailable",
            Self::NotPublished => "runtime discovery descriptor was not published",
            Self::Unreadable => "runtime discovery descriptor could not be read",
            Self::Oversized => "runtime discovery descriptor exceeds the bounded size",
            Self::Malformed => "runtime discovery descriptor is not the documented schema",
            Self::UnsupportedSchema => "runtime discovery descriptor schema is unsupported",
            Self::UnsupportedOrigin => {
                "runtime discovery origin is not a plain-HTTP loopback IP literal"
            }
            Self::UnsupportedRange => {
                "runtime discovery descriptor does not advertise this build's protocol pair"
            }
            Self::NotReady => "runtime discovery descriptor reports that the Runtime is not ready",
            Self::InvalidIdentity => "runtime discovery server identity is not usable",
            Self::InvalidEpoch => "runtime discovery epoch is not usable",
            Self::InvalidPid => "runtime discovery process identity is not usable",
            Self::InvalidStartTime => "runtime discovery start time is not usable",
        }
    }
}

/// One validated Runtime discovery descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Descriptor {
    /// Normalized plain-HTTP loopback origin.
    origin: String,
    /// Runtime build identity (informational; the handshake identity is verified).
    server_identity: String,
    /// Process-scoped observation epoch.
    epoch: String,
    /// Runtime process id.
    pid: u32,
}

/// One advertised `major.minor` range, inclusive.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireRange {
    major: u16,
    min_minor: u16,
    max_minor: u16,
}

impl WireRange {
    fn contains_build_pair(self, major: u16, minor: u16) -> bool {
        self.major == major && (self.min_minor..=self.max_minor).contains(&minor)
    }
}

/// The closed wire schema; unknown fields are refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireDescriptor {
    schema_version: u32,
    origin: String,
    server_identity: String,
    epoch: String,
    protocol: WireRange,
    #[serde(rename = "schema")]
    schema: WireRange,
    pid: u32,
    started_at: String,
    ready: bool,
}

impl Descriptor {
    /// Normalized plain-HTTP loopback origin.
    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    /// Runtime build identity (informational).
    pub(crate) fn server_identity(&self) -> &str {
        &self.server_identity
    }

    /// Process-scoped observation epoch.
    pub(crate) fn epoch(&self) -> &str {
        &self.epoch
    }

    /// Runtime process id.
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }
}

/// Resolve the descriptor path for this process.
///
/// The environment override wins; otherwise the per-user location is derived
/// from the platform environment. `None` means no usable root exists.
pub(crate) fn default_path() -> Option<PathBuf> {
    default_path_from(|key| std::env::var(key).ok())
}

fn default_path_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(explicit) = get(DISCOVERY_PATH_ENV)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(explicit));
    }
    #[cfg(windows)]
    {
        let root = get("LOCALAPPDATA")?;
        let root = root.trim();
        if root.is_empty() {
            return None;
        }
        Some(Path::new(root).join("COS").join("runtime.json"))
    }
    #[cfg(not(windows))]
    {
        if let Some(state) = get("XDG_STATE_HOME") {
            let state = state.trim();
            if !state.is_empty() {
                return Some(Path::new(state).join("cos").join("runtime.json"));
            }
        }
        let home = get("HOME")?;
        let home = home.trim();
        if home.is_empty() {
            return None;
        }
        Some(
            Path::new(home)
                .join(".local")
                .join("state")
                .join("cos")
                .join("runtime.json"),
        )
    }
}

/// Read and validate the discovery descriptor at one exact path.
///
/// This never contacts the origin and never falls back to another source.
pub(crate) fn discover(path: &Path) -> Result<Descriptor, DiscoveryError> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => DiscoveryError::NotPublished,
        _ => DiscoveryError::Unreadable,
    })?;
    if !metadata.is_file() {
        return Err(DiscoveryError::Unreadable);
    }
    if metadata.len() > MAX_DESCRIPTOR_BYTES {
        return Err(DiscoveryError::Oversized);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    {
        use std::io::Read;
        let handle = std::fs::File::open(path).map_err(|_| DiscoveryError::Unreadable)?;
        handle
            .take(MAX_DESCRIPTOR_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| DiscoveryError::Unreadable)?;
    }
    if bytes.len() as u64 > MAX_DESCRIPTOR_BYTES {
        return Err(DiscoveryError::Oversized);
    }
    let wire: WireDescriptor =
        serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::Malformed)?;
    validate(wire)
}

fn validate(wire: WireDescriptor) -> Result<Descriptor, DiscoveryError> {
    if wire.schema_version != DISCOVERY_SCHEMA_VERSION {
        return Err(DiscoveryError::UnsupportedSchema);
    }
    if wire.origin.len() > MAX_ORIGIN_BYTES {
        return Err(DiscoveryError::UnsupportedOrigin);
    }
    let origin =
        crate::runtime::validated_origin(&wire.origin).ok_or(DiscoveryError::UnsupportedOrigin)?;
    if !wire
        .protocol
        .contains_build_pair(PROTOCOL_MAJOR, PROTOCOL_MINOR)
        || !wire
            .schema
            .contains_build_pair(PROTOCOL_MAJOR, PROTOCOL_MINOR)
    {
        return Err(DiscoveryError::UnsupportedRange);
    }
    if !wire.ready {
        return Err(DiscoveryError::NotReady);
    }
    if !usable_text(&wire.server_identity) {
        return Err(DiscoveryError::InvalidIdentity);
    }
    if !usable_text(&wire.epoch) {
        return Err(DiscoveryError::InvalidEpoch);
    }
    if !usable_text(&wire.started_at) {
        return Err(DiscoveryError::InvalidStartTime);
    }
    if wire.pid == 0 {
        return Err(DiscoveryError::InvalidPid);
    }
    Ok(Descriptor {
        origin,
        server_identity: wire.server_identity,
        epoch: wire.epoch,
        pid: wire.pid,
    })
}

/// Whether a descriptor text field is bounded, non-empty printable ASCII.
fn usable_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b'\\')
}

/// A discovery value rendered for a diagnostic line.
///
/// A descriptor is server-issued, not Desktop-controlled: it may be shaped like a
/// private path or a signed URL even though it is printable ASCII. Only characters
/// that cannot form a path separator or a URL survive, the result is bounded, and a
/// value with nothing safe left renders as `unknown`. The descriptor path, origin and
/// credential are never printed by these lines.
pub(crate) fn diagnostic_label(value: &str) -> String {
    let label: String = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        .take(MAX_TEXT_BYTES)
        .collect();
    if label.is_empty() {
        return "unknown".to_owned();
    }
    label
}

#[cfg(test)]
mod tests {
    use super::{
        DISCOVERY_PATH_ENV, Descriptor, DiscoveryError, MAX_DESCRIPTOR_BYTES, MAX_TEXT_BYTES,
        default_path_from, diagnostic_label, discover,
    };
    use std::path::PathBuf;

    fn unique_path(label: &str) -> PathBuf {
        // The workspace target directory is writable in the gate environment;
        // the OS temp directory may be sandboxed read-only for test processes.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/rt8-discovery-tests")
            .join(format!("{}-{label}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create test directory");
        root.join(format!("{label}.json"))
    }

    fn write(label: &str, body: &str) -> PathBuf {
        let path = unique_path(label);
        std::fs::write(&path, body).expect("write descriptor");
        path
    }

    fn valid() -> String {
        r#"{"schema_version":1,"origin":"http://127.0.0.1:8124","server_identity":"cos-runtime-orchestrator","epoch":"09dfcf00916c432b978a62a4a01b34a2","protocol":{"major":2,"min_minor":0,"max_minor":0},"schema":{"major":2,"min_minor":0,"max_minor":0},"pid":4242,"started_at":"2026-11-09T02:00:00+09:00","ready":true}"#.to_owned()
    }

    fn discover_body(label: &str, body: &str) -> Result<Descriptor, DiscoveryError> {
        let path = write(label, body);
        let result = discover(&path);
        let _ = std::fs::remove_file(&path);
        result
    }

    #[test]
    fn a_published_descriptor_is_accepted_and_names_the_runtime() {
        let descriptor = discover_body("valid", &valid()).expect("valid descriptor");
        assert_eq!(descriptor.origin(), "http://127.0.0.1:8124");
        assert_eq!(descriptor.server_identity(), "cos-runtime-orchestrator");
        assert_eq!(descriptor.epoch(), "09dfcf00916c432b978a62a4a01b34a2");
        assert_eq!(descriptor.pid(), 4242);
    }

    #[test]
    fn a_missing_descriptor_is_explicitly_not_published() {
        let path = unique_path("missing");
        let _ = std::fs::remove_file(&path);
        assert_eq!(discover(&path), Err(DiscoveryError::NotPublished));
    }

    #[test]
    fn unknown_fields_and_oversized_documents_are_refused() {
        let unknown = valid().replace(r#""ready":true"#, r#""ready":true,"extra":1"#);
        assert_eq!(
            discover_body("unknown", &unknown),
            Err(DiscoveryError::Malformed)
        );
        let oversized = format!(
            r#"{{"schema_version":1,"padding":"{}"}}"#,
            "x".repeat(MAX_DESCRIPTOR_BYTES as usize)
        );
        assert_eq!(
            discover_body("oversized", &oversized),
            Err(DiscoveryError::Oversized)
        );
    }

    #[test]
    fn only_a_ready_loopback_ip_literal_origin_is_accepted() {
        for origin in [
            "http://localhost:8124",
            "https://127.0.0.1:8124",
            "http://10.0.0.5:8124",
            "http://127.0.0.1:8124/path",
            "http://user@127.0.0.1:8124",
        ] {
            let body = valid().replace("http://127.0.0.1:8124", origin);
            assert_eq!(
                discover_body("origin", &body),
                Err(DiscoveryError::UnsupportedOrigin),
                "{origin} must be refused"
            );
        }
        let not_ready = valid().replace(r#""ready":true"#, r#""ready":false"#);
        assert_eq!(
            discover_body("not-ready", &not_ready),
            Err(DiscoveryError::NotReady)
        );
    }

    #[test]
    fn unsupported_schema_ranges_and_unusable_identities_are_refused() {
        let schema = valid().replace(r#""schema_version":1"#, r#""schema_version":2"#);
        assert_eq!(
            discover_body("schema", &schema),
            Err(DiscoveryError::UnsupportedSchema)
        );
        let range = valid().replace(
            r#""protocol":{"major":2,"min_minor":0,"max_minor":0}"#,
            r#""protocol":{"major":3,"min_minor":0,"max_minor":0}"#,
        );
        assert_eq!(
            discover_body("range", &range),
            Err(DiscoveryError::UnsupportedRange)
        );
        assert_eq!(
            discover_body("identity", &valid().replace("cos-runtime-orchestrator", "")),
            Err(DiscoveryError::InvalidIdentity)
        );
        assert_eq!(
            discover_body(
                "epoch",
                &valid().replace("09dfcf00916c432b978a62a4a01b34a2", "")
            ),
            Err(DiscoveryError::InvalidEpoch)
        );
        assert_eq!(
            discover_body("pid", &valid().replace(r#""pid":4242"#, r#""pid":0"#)),
            Err(DiscoveryError::InvalidPid)
        );
    }

    #[test]
    fn the_override_wins_over_the_platform_root() {
        let explicit = default_path_from(|key| match key {
            DISCOVERY_PATH_ENV => Some("C:/tmp/rt8/runtime.json".to_owned()),
            _ => Some("C:/Users/test/AppData/Local".to_owned()),
        })
        .expect("override path");
        assert_eq!(explicit, PathBuf::from("C:/tmp/rt8/runtime.json"));
    }

    #[test]
    fn an_absent_root_is_explicitly_unavailable() {
        assert_eq!(default_path_from(|_| None), None);
    }

    /// A server-issued descriptor value can be shaped like a private path or a signed
    /// URL; the diagnostic line must not be able to reproduce either.
    #[test]
    fn a_diagnostic_label_cannot_carry_a_path_or_a_url() {
        assert_eq!(diagnostic_label("/home/alice/private"), "homealiceprivate");
        assert_eq!(
            diagnostic_label("https://example.test/x?y=1"),
            "httpsexample.testxy1"
        );
        assert_eq!(diagnostic_label(""), "unknown");
        assert_eq!(diagnostic_label("///\\::"), "unknown");
        assert_eq!(diagnostic_label(&"a".repeat(4096)).len(), MAX_TEXT_BYTES);
    }
}
