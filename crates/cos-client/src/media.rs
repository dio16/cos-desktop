//! RT6/RC-13 media descriptor and byte fetch, transport-independent.
//!
//! The executor is injected by the binary boundary (the same plain-loopback
//! `ureq` boundary that carries protocol requests), so this crate owns no
//! network dependency and never sees the Desktop token or the media handle
//! value beyond the header it is asked to send. Only the **validated loopback
//! origin plus the opaque relative handle** is ever requested: an absolute URL,
//! a scheme, a redirect target or a filesystem-shaped handle is refused.
//!
//! Descriptors are the closed v2 document of `DESKTOP_PROTOCOL.md` §6: unknown
//! members are refused (unlike the forward-compatible projection envelope),
//! absent fields stay absent, and no digest, length or representation is
//! synthesized locally. Byte identity is verified by `crates/media`; this
//! module only enforces the descriptor's stated byte length, the
//! representation budget and the error vocabulary.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use desktop_protocol::{Availability, MediaDescriptor, RepresentationKind, VerificationState};

/// Descriptor route prefix; the artifact digest is appended as one path segment.
pub const MEDIA_DESCRIPTOR_PATH: &str = "/api/cos/media";
/// Header carrying the RC-03 observation context on descriptor reads.
pub const MEDIA_CONTEXT_HEADER: &str = "X-Cos-Context";
/// Header carrying the opaque media handle on byte reads.
pub const MEDIA_HANDLE_HEADER: &str = "X-Media-Handle";

/// Fixed representation budgets (accepted RC-13 decision).
pub fn representation_budget(representation: &RepresentationKind) -> u64 {
    match representation {
        RepresentationKind::Metadata => 64 * 1024,
        RepresentationKind::Thumbnail => 1024 * 1024,
        RepresentationKind::Preview => 16 * 1024 * 1024,
        RepresentationKind::Original => 256 * 1024 * 1024,
        // Unknown representation: fail closed by allowing no bytes.
        RepresentationKind::Unknown(_) => 0,
    }
}

/// Cooperative cancellation shared with the executor.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    /// Request cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// One bounded response from the media executor.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaResponse {
    /// HTTP status.
    pub status: u16,
    /// Declared `Content-Length`, when the server sent one.
    pub content_length: Option<u64>,
    /// Response body, already bounded by the executor.
    pub body: Vec<u8>,
}

impl std::fmt::Debug for MediaResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaResponse")
            .field("status", &self.status)
            .field("content_length", &self.content_length)
            .field("body", &format!("<{} bytes>", self.body.len()))
            .finish()
    }
}

/// Executor-level failure classes (value-free).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaTransportError {
    /// The loopback exchange failed.
    Transport,
    /// The executor refused a body above its bound.
    Oversized,
}

/// Blocking same-authority media executor supplied by the binary boundary.
///
/// Implementations must request only the validated loopback origin, follow no
/// redirect, consult no environment proxy, attach the bearer credential only on
/// these same-authority paths, and bound the body to `max_bytes`.
pub trait MediaExecutor: Send {
    /// Perform one bounded `GET` for a relative same-authority path.
    fn get(
        &mut self,
        path: &str,
        headers: &[(&str, &str)],
        max_bytes: u64,
        cancellation: &Cancellation,
    ) -> Result<MediaResponse, MediaTransportError>;
}

/// Value-free fetch failure classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaFetchError {
    /// The Runtime rejected the credential (401).
    Unauthorized,
    /// The principal is not authorized for this artifact (403).
    Forbidden,
    /// The artifact or descriptor does not exist (404).
    NotFound,
    /// The representation is not available yet (409).
    Pending,
    /// The descriptor/handle expired (410); refetch the descriptor.
    DescriptorExpired,
    /// The representation exceeds its budget (413).
    TooLarge,
    /// The media type is not supported (415).
    Unsupported,
    /// The requested range is not satisfiable (416).
    RangeUnsatisfiable,
    /// The descriptor document is not the closed v2 shape, or violates it.
    MalformedDescriptor,
    /// The access handle is not an opaque same-authority relative path.
    HandleRejected,
    /// The descriptor was issued by a different server identity.
    ServerIdentityMismatch,
    /// The descriptor is addressed to a different principal.
    AudienceMismatch,
    /// The server reports the representation as quarantined.
    Quarantined,
    /// The server reports the representation as failed.
    Failed,
    /// The transport failed.
    Transport,
    /// A body exceeded the bounded size.
    Oversized,
    /// Cancellation was requested.
    Cancelled,
    /// The response length contradicts the descriptor or `Content-Length`.
    LengthMismatch,
    /// Any other status is not part of the media contract.
    UnexpectedStatus,
}

impl std::fmt::Display for MediaFetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unauthorized => "media request was not authorized",
            Self::Forbidden => "media request was refused",
            Self::NotFound => "media descriptor or artifact was not found",
            Self::Pending => "media representation is not available yet",
            Self::DescriptorExpired => "media descriptor expired; refetch it",
            Self::TooLarge => "media representation exceeds its budget",
            Self::Unsupported => "media representation is not supported",
            Self::RangeUnsatisfiable => "media range is not satisfiable",
            Self::MalformedDescriptor => "media descriptor is not the documented shape",
            Self::HandleRejected => "media access handle is not a same-authority path",
            Self::ServerIdentityMismatch => "media descriptor server identity differs",
            Self::AudienceMismatch => "media descriptor audience differs",
            Self::Quarantined => "media representation is quarantined",
            Self::Failed => "media representation failed verification",
            Self::Transport => "media transport failed",
            Self::Oversized => "media body exceeded the bounded size",
            Self::Cancelled => "media fetch was cancelled",
            Self::LengthMismatch => "media body length contradicts the descriptor",
            Self::UnexpectedStatus => "media endpoint returned an unexpected status",
        })
    }
}

/// A fetched descriptor plus the exact bytes the server returned.
#[derive(Clone, PartialEq, Eq)]
pub struct FetchedMedia {
    /// The validated descriptor the bytes belong to.
    pub descriptor: MediaDescriptor,
    /// Bounded representation bytes; identity is verified by `crates/media`.
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for FetchedMedia {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FetchedMedia")
            .field("descriptor", &self.descriptor)
            .field("bytes", &format!("<{} bytes>", self.bytes.len()))
            .finish()
    }
}

/// Descriptor and byte fetch for one authenticated Desktop principal.
pub struct MediaFetcher<E: MediaExecutor> {
    executor: E,
    server_identity: String,
    audience: String,
}

impl<E: MediaExecutor> MediaFetcher<E> {
    /// Bind a fetcher to the expected server identity and Desktop principal.
    pub fn new(
        executor: E,
        server_identity: impl Into<String>,
        audience: impl Into<String>,
    ) -> Self {
        Self {
            executor,
            server_identity: server_identity.into(),
            audience: audience.into(),
        }
    }

    /// Fetch and validate the closed v2 descriptor for one artifact digest.
    ///
    /// `context` is the RC-03 observation context. It is bounded and sent only
    /// as the `X-Cos-Context` header value; it is never rendered.
    pub fn fetch_descriptor(
        &mut self,
        artifact_digest: &str,
        context: &str,
        cancellation: &Cancellation,
    ) -> Result<MediaDescriptor, MediaFetchError> {
        if cancellation.is_cancelled() {
            return Err(MediaFetchError::Cancelled);
        }
        if !valid_digest_segment(artifact_digest) || !valid_header_value(context) {
            return Err(MediaFetchError::MalformedDescriptor);
        }
        let path = format!("{MEDIA_DESCRIPTOR_PATH}/{artifact_digest}");
        let response = self
            .executor
            .get(
                &path,
                &[
                    ("Accept", "application/json"),
                    (MEDIA_CONTEXT_HEADER, context),
                ],
                64 * 1024,
                cancellation,
            )
            .map_err(map_transport)?;
        if cancellation.is_cancelled() {
            return Err(MediaFetchError::Cancelled);
        }
        if response.status != 200 {
            return Err(status_error(response.status));
        }
        let descriptor = parse_descriptor(&response.body)?;
        self.validate_descriptor(&descriptor, artifact_digest)?;
        Ok(descriptor)
    }

    /// Fetch the bytes for one validated descriptor.
    pub fn fetch_bytes(
        &mut self,
        descriptor: &MediaDescriptor,
        cancellation: &Cancellation,
    ) -> Result<FetchedMedia, MediaFetchError> {
        if cancellation.is_cancelled() {
            return Err(MediaFetchError::Cancelled);
        }
        if !descriptor.availability.is_available() {
            return Err(availability_error(&descriptor.availability));
        }
        let handle = descriptor
            .access_handle
            .as_deref()
            .ok_or(MediaFetchError::HandleRejected)?;
        let path = relative_handle(handle)?;
        let budget = representation_budget(&descriptor.representation);
        let media_type = descriptor.media_type.as_deref().unwrap_or("*/*");
        let response = self
            .executor
            .get(
                path,
                &[("Accept", media_type), (MEDIA_HANDLE_HEADER, handle)],
                budget,
                cancellation,
            )
            .map_err(map_transport)?;
        if cancellation.is_cancelled() {
            return Err(MediaFetchError::Cancelled);
        }
        if response.status != 200 {
            return Err(status_error(response.status));
        }
        if response.body.len() as u64 > budget {
            return Err(MediaFetchError::Oversized);
        }
        if let Some(expected) = descriptor.byte_length
            && response.body.len() as u64 != expected
        {
            return Err(MediaFetchError::LengthMismatch);
        }
        if let Some(declared) = response.content_length
            && declared != response.body.len() as u64
        {
            return Err(MediaFetchError::LengthMismatch);
        }
        Ok(FetchedMedia {
            descriptor: descriptor.clone(),
            bytes: response.body,
        })
    }

    fn validate_descriptor(
        &self,
        descriptor: &MediaDescriptor,
        requested_digest: &str,
    ) -> Result<(), MediaFetchError> {
        if descriptor.descriptor_id.as_str().is_empty()
            || descriptor.artifact_ref.as_str().is_empty()
        {
            return Err(MediaFetchError::MalformedDescriptor);
        }
        if let Some(digest) = descriptor.artifact_digest.as_ref()
            && digest.as_str() != requested_digest
        {
            return Err(MediaFetchError::MalformedDescriptor);
        }
        if descriptor.server_identity.as_deref() != Some(self.server_identity.as_str()) {
            return Err(MediaFetchError::ServerIdentityMismatch);
        }
        if descriptor.audience.as_deref() != Some(self.audience.as_str()) {
            return Err(MediaFetchError::AudienceMismatch);
        }
        if let Some(expires_at) = descriptor.expires_at_unix_ms
            && now_unix_ms() >= expires_at
        {
            return Err(MediaFetchError::DescriptorExpired);
        }
        if let Some(verification) = descriptor.verification.as_ref() {
            match &verification.state {
                VerificationState::Verified => {
                    if verification.algorithm.as_deref() != Some("sha256")
                        || !verification
                            .expected_digest
                            .as_deref()
                            .is_some_and(is_sha256_hex)
                    {
                        return Err(MediaFetchError::MalformedDescriptor);
                    }
                }
                VerificationState::Mismatch | VerificationState::Quarantined => {
                    return Err(MediaFetchError::Quarantined);
                }
                VerificationState::Unverified => {}
                VerificationState::Unknown(_) => return Err(MediaFetchError::MalformedDescriptor),
            }
        }
        Ok(())
    }
}

/// The expected SHA-256 for a verified descriptor, if the server stated one.
///
/// A descriptor without a `VERIFIED` verification summary has no expected
/// digest; callers must treat its bytes as unverified rather than invent one.
pub fn expected_sha256(descriptor: &MediaDescriptor) -> Result<Option<[u8; 32]>, MediaFetchError> {
    let Some(verification) = descriptor.verification.as_ref() else {
        return Ok(None);
    };
    if verification.state != VerificationState::Verified {
        return Ok(None);
    }
    if verification.algorithm.as_deref() != Some("sha256") {
        return Err(MediaFetchError::MalformedDescriptor);
    }
    let digest = verification
        .expected_digest
        .as_deref()
        .ok_or(MediaFetchError::MalformedDescriptor)?;
    parse_sha256_hex(digest)
        .map(Some)
        .ok_or(MediaFetchError::MalformedDescriptor)
}

fn parse_descriptor(body: &[u8]) -> Result<MediaDescriptor, MediaFetchError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| MediaFetchError::MalformedDescriptor)?;
    let object = value
        .as_object()
        .ok_or(MediaFetchError::MalformedDescriptor)?;
    let allowed: BTreeSet<&str> = [
        "descriptor_id",
        "artifact_ref",
        "artifact_digest",
        "representation",
        "representation_version",
        "media_type",
        "byte_length",
        "width",
        "height",
        "duration_ms",
        "availability",
        "verification",
        "access_handle",
        "server_identity",
        "audience",
        "expires_at_unix_ms",
    ]
    .into_iter()
    .collect();
    if object.keys().any(|key| !allowed.contains(key.as_str())) {
        return Err(MediaFetchError::MalformedDescriptor);
    }
    let descriptor: MediaDescriptor =
        serde_json::from_value(value).map_err(|_| MediaFetchError::MalformedDescriptor)?;
    Ok(descriptor)
}

fn map_transport(failure: MediaTransportError) -> MediaFetchError {
    match failure {
        MediaTransportError::Transport => MediaFetchError::Transport,
        MediaTransportError::Oversized => MediaFetchError::Oversized,
    }
}

fn status_error(status: u16) -> MediaFetchError {
    match status {
        401 => MediaFetchError::Unauthorized,
        403 => MediaFetchError::Forbidden,
        404 => MediaFetchError::NotFound,
        409 => MediaFetchError::Pending,
        410 => MediaFetchError::DescriptorExpired,
        413 => MediaFetchError::TooLarge,
        415 => MediaFetchError::Unsupported,
        416 => MediaFetchError::RangeUnsatisfiable,
        _ => MediaFetchError::UnexpectedStatus,
    }
}

fn availability_error(availability: &Availability) -> MediaFetchError {
    match availability {
        Availability::Available => MediaFetchError::MalformedDescriptor,
        Availability::Staged | Availability::Pending => MediaFetchError::Pending,
        Availability::Missing => MediaFetchError::NotFound,
        Availability::Forbidden => MediaFetchError::Forbidden,
        Availability::Failed => MediaFetchError::Failed,
        Availability::Quarantined => MediaFetchError::Quarantined,
        Availability::Unsupported => MediaFetchError::Unsupported,
        Availability::Unknown(_) => MediaFetchError::UnexpectedStatus,
    }
}

/// A handle must be an opaque relative same-authority path.
///
/// Absolute URLs, scheme-relative paths, backslashes, query/fragment, dot
/// segments, percent-encoding, whitespace and control characters are refused:
/// the client joins the value onto the already-validated loopback origin
/// itself. Percent-encoding is refused outright rather than decoded, so a
/// `%2e`/`%2E` dot segment cannot bypass the literal dot-segment rule when a
/// server decodes the path before routing it.
pub fn relative_handle(handle: &str) -> Result<&str, MediaFetchError> {
    if handle.is_empty()
        || handle.len() > 2048
        || !handle.starts_with('/')
        || handle.starts_with("//")
    {
        return Err(MediaFetchError::HandleRejected);
    }
    if handle.contains("://")
        || handle.contains('\\')
        || handle.contains('?')
        || handle.contains('#')
        || handle.contains('%')
        || handle.bytes().any(|byte| !(0x21..=0x7e).contains(&byte))
    {
        return Err(MediaFetchError::HandleRejected);
    }
    if handle
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(MediaFetchError::HandleRejected);
    }
    Ok(handle)
}

fn valid_digest_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
}

fn valid_header_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_sha256_hex(value: &str) -> Option<[u8; 32]> {
    if !is_sha256_hex(value) {
        return None;
    }
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in value.bytes().enumerate() {
        let value = nibble(byte)?;
        digest[index / 2] |= if index % 2 == 0 { value << 4 } else { value };
    }
    Some(digest)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const SERVER: &str = "server-1";
    const AUDIENCE: &str = "desktop:0123456789abcdef0123456789abcdef";

    #[derive(Default)]
    struct Stub {
        responses: VecDeque<Result<MediaResponse, MediaTransportError>>,
        paths: Vec<String>,
        header_names: Vec<Vec<String>>,
        budgets: Vec<u64>,
    }

    impl Stub {
        fn with(
            responses: impl IntoIterator<Item = Result<MediaResponse, MediaTransportError>>,
        ) -> Self {
            Self {
                responses: responses.into_iter().collect(),
                ..Self::default()
            }
        }

        fn ok(body: &[u8]) -> Self {
            Self::with([Ok(MediaResponse {
                status: 200,
                content_length: Some(body.len() as u64),
                body: body.to_vec(),
            })])
        }

        fn status(status: u16) -> Self {
            Self::with([Ok(MediaResponse {
                status,
                content_length: Some(0),
                body: Vec::new(),
            })])
        }
    }

    impl MediaExecutor for Stub {
        fn get(
            &mut self,
            path: &str,
            headers: &[(&str, &str)],
            max_bytes: u64,
            _cancellation: &Cancellation,
        ) -> Result<MediaResponse, MediaTransportError> {
            self.paths.push(path.to_owned());
            self.header_names.push(
                headers
                    .iter()
                    .map(|(name, _value)| (*name).to_owned())
                    .collect(),
            );
            self.budgets.push(max_bytes);
            self.responses
                .pop_front()
                .unwrap_or(Err(MediaTransportError::Transport))
        }
    }

    fn make_fetcher(stub: Stub) -> MediaFetcher<Stub> {
        MediaFetcher::new(stub, SERVER, AUDIENCE)
    }

    fn descriptor(extra: &str) -> String {
        format!(
            r#"{{"descriptor_id":"descriptor-1","artifact_ref":"artifact-1",
                "artifact_digest":"{DIGEST}","representation":"ORIGINAL",
                "representation_version":1,"media_type":"image/png","byte_length":3,
                "availability":"AVAILABLE","server_identity":"{SERVER}","audience":"{AUDIENCE}",
                "access_handle":"/api/cos/media/{DIGEST}/blob","expires_at_unix_ms":4102444800000
                {extra}}}"#
        )
    }

    fn available_descriptor() -> MediaDescriptor {
        parse_descriptor(descriptor("").as_bytes()).expect("descriptor parses")
    }

    #[test]
    fn a_closed_v2_descriptor_is_accepted_with_its_optional_fields() {
        let body = descriptor(
            r#","verification":{"state":"VERIFIED","algorithm":"sha256","expected_digest":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#,
        );
        let mut fetcher = make_fetcher(Stub::ok(body.as_bytes()));
        let descriptor = fetcher
            .fetch_descriptor(DIGEST, "context-1", &Cancellation::default())
            .expect("descriptor accepted");
        assert_eq!(
            descriptor.artifact_digest.as_ref().map(|d| d.as_str()),
            Some(DIGEST)
        );
        assert_eq!(descriptor.byte_length, Some(3));
        assert_eq!(
            descriptor.access_handle.as_deref(),
            Some(&format!("/api/cos/media/{DIGEST}/blob")[..])
        );
        assert_eq!(
            fetcher.executor.paths[0],
            format!("{MEDIA_DESCRIPTOR_PATH}/{DIGEST}")
        );
        assert!(
            fetcher.executor.header_names[0]
                .iter()
                .any(|name| name == MEDIA_CONTEXT_HEADER)
        );
        assert_eq!(
            expected_sha256(&descriptor).expect("verified digest"),
            Some([
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
                0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,
                0x89, 0xab, 0xcd, 0xef
            ])
        );
    }

    #[test]
    fn an_unknown_descriptor_member_is_refused() {
        let body = descriptor(r#","future_field":"value""#);
        let mut fetcher = make_fetcher(Stub::ok(body.as_bytes()));
        assert_eq!(
            fetcher.fetch_descriptor(DIGEST, "context-1", &Cancellation::default()),
            Err(MediaFetchError::MalformedDescriptor)
        );
    }

    #[test]
    fn absent_fields_stay_absent_and_are_never_synthesized() {
        let minimal = r#"{"descriptor_id":"descriptor-1","artifact_ref":"artifact-1",
            "representation":"THUMBNAIL","availability":"AVAILABLE"}"#;
        let descriptor = parse_descriptor(minimal.as_bytes()).expect("minimal descriptor parses");
        assert!(descriptor.artifact_digest.is_none());
        assert!(descriptor.media_type.is_none());
        assert!(descriptor.byte_length.is_none());
        assert!(descriptor.verification.is_none());
        assert!(descriptor.access_handle.is_none());
        assert!(descriptor.expires_at_unix_ms.is_none());
        assert_eq!(descriptor.representation_version, 1);
        assert_eq!(
            expected_sha256(&descriptor).expect("no digest stated"),
            None
        );
    }

    #[test]
    fn http_statuses_map_to_the_documented_error_classes() {
        for (status, expected) in [
            (401, MediaFetchError::Unauthorized),
            (403, MediaFetchError::Forbidden),
            (404, MediaFetchError::NotFound),
            (409, MediaFetchError::Pending),
            (410, MediaFetchError::DescriptorExpired),
            (413, MediaFetchError::TooLarge),
            (415, MediaFetchError::Unsupported),
            (416, MediaFetchError::RangeUnsatisfiable),
            (418, MediaFetchError::UnexpectedStatus),
        ] {
            let mut fetcher = make_fetcher(Stub::status(status));
            assert_eq!(
                fetcher.fetch_descriptor(DIGEST, "context-1", &Cancellation::default()),
                Err(expected),
                "status {status}"
            );
        }
    }

    #[test]
    fn stale_wrong_identity_and_wrong_audience_descriptors_are_refused() {
        let stale = descriptor(r#","expires_at_unix_ms":1"#);
        let mut fetcher = make_fetcher(Stub::ok(stale.as_bytes()));
        assert_eq!(
            fetcher.fetch_descriptor(DIGEST, "context-1", &Cancellation::default()),
            Err(MediaFetchError::DescriptorExpired)
        );
        let wrong_server = descriptor("").replace(SERVER, "server-2");
        let mut fetcher = make_fetcher(Stub::ok(wrong_server.as_bytes()));
        assert_eq!(
            fetcher.fetch_descriptor(DIGEST, "context-1", &Cancellation::default()),
            Err(MediaFetchError::ServerIdentityMismatch)
        );
        let wrong_audience =
            descriptor("").replace(AUDIENCE, "desktop:ffffffffffffffffffffffffffffffff");
        let mut fetcher = make_fetcher(Stub::ok(wrong_audience.as_bytes()));
        assert_eq!(
            fetcher.fetch_descriptor(DIGEST, "context-1", &Cancellation::default()),
            Err(MediaFetchError::AudienceMismatch)
        );
        let quarantined = descriptor(
            r#","verification":{"state":"MISMATCH","algorithm":"sha256","expected_digest":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#,
        );
        let mut fetcher = make_fetcher(Stub::ok(quarantined.as_bytes()));
        assert_eq!(
            fetcher.fetch_descriptor(DIGEST, "context-1", &Cancellation::default()),
            Err(MediaFetchError::Quarantined)
        );
    }

    #[test]
    fn an_absolute_or_scheme_relative_handle_is_never_followed() {
        for handle in [
            "https://example.invalid/blob",
            "//example.invalid/blob",
            "/api/../escape",
            "/api/./blob",
            "/api\\blob",
            "C:\\blob",
            "/api/blob?token=1",
            "/api/blob#fragment",
            "/api/blo b",
            "/api/%2e%2e/escape",
            "/api/%2E/blob",
            "/api/.%2e/escape",
            "/api/%2e./escape",
            "/api/blob%00",
            "",
        ] {
            assert_eq!(
                relative_handle(handle),
                Err(MediaFetchError::HandleRejected),
                "{handle}"
            );
        }
        assert_eq!(
            relative_handle("/api/cos/media/x/blob").expect("relative handle"),
            "/api/cos/media/x/blob"
        );
    }

    #[test]
    fn representation_budgets_are_the_accepted_values() {
        assert_eq!(
            representation_budget(&RepresentationKind::Metadata),
            64 * 1024
        );
        assert_eq!(
            representation_budget(&RepresentationKind::Thumbnail),
            1024 * 1024
        );
        assert_eq!(
            representation_budget(&RepresentationKind::Preview),
            16 * 1024 * 1024
        );
        assert_eq!(
            representation_budget(&RepresentationKind::Original),
            256 * 1024 * 1024
        );
    }

    #[test]
    fn the_byte_fetch_uses_the_handle_header_and_enforces_the_declared_length() {
        let descriptor = available_descriptor();
        let mut fetcher = make_fetcher(Stub::ok(b"abc"));
        let fetched = fetcher
            .fetch_bytes(&descriptor, &Cancellation::default())
            .expect("bytes accepted");
        assert_eq!(fetched.bytes, b"abc");
        assert_eq!(
            fetcher.executor.header_names[0]
                .iter()
                .filter(|name| *name == MEDIA_HANDLE_HEADER)
                .count(),
            1
        );
        assert_eq!(
            fetcher.executor.paths[0],
            format!("/api/cos/media/{DIGEST}/blob")
        );
        assert_eq!(fetcher.executor.budgets[0], 256 * 1024 * 1024);

        let mut short = make_fetcher(Stub::ok(b"ab"));
        assert_eq!(
            short.fetch_bytes(&descriptor, &Cancellation::default()),
            Err(MediaFetchError::LengthMismatch)
        );
    }

    #[test]
    fn an_oversized_body_and_a_cancelled_fetch_are_refused() {
        let descriptor = available_descriptor();
        let oversized = vec![b'x'; (64 * 1024 + 1) as usize];
        let metadata = MediaDescriptor {
            representation: RepresentationKind::Metadata,
            byte_length: None,
            ..descriptor
        };
        let mut fetcher = make_fetcher(Stub::ok(&oversized));
        assert_eq!(
            fetcher.fetch_bytes(&metadata, &Cancellation::default()),
            Err(MediaFetchError::Oversized)
        );

        let cancellation = Cancellation::default();
        cancellation.cancel();
        let mut fetcher = make_fetcher(Stub::ok(b"abc"));
        assert_eq!(
            fetcher.fetch_descriptor(DIGEST, "context-1", &cancellation),
            Err(MediaFetchError::Cancelled)
        );
        assert!(fetcher.executor.paths.is_empty());
    }

    #[test]
    fn error_renderings_never_carry_a_handle_or_token() {
        let sentinel = "/api/secret-handle?token=sentinel";
        for error in [
            MediaFetchError::Unauthorized,
            MediaFetchError::HandleRejected,
            MediaFetchError::MalformedDescriptor,
            MediaFetchError::Transport,
        ] {
            let rendered = format!("{error} {error:?}");
            assert!(!rendered.contains(sentinel));
            assert!(!rendered.contains("token"));
        }
    }
}
