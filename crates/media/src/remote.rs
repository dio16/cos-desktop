//! RC-13 remote (Runtime-authorized) media admission.
//!
//! Bytes fetched through `cos-client` are untrusted even when the transport was
//! authenticated. Admission verifies the descriptor's stated byte length, the
//! container class and (when the descriptor is `VERIFIED`) the exact SHA-256
//! before any caller can decode or cache them. A mismatch is a quarantine: the
//! bytes are dropped and no partial value is returned, so a bad response can
//! never replace canonical identity. Without an expected digest the payload is
//! admitted as *unverified*; callers may bounded-decode it but must not report
//! it as verified.
//!
//! This module owns no GPUI object and performs no network I/O.

use sha2::{Digest, Sha256};

use crate::{AcceptedFormat, Cancellation, MediaError, Sha256Digest};

/// Expectations a validated descriptor supplied for one representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteExpectation {
    /// Exact byte length, when the descriptor stated one.
    pub byte_length: Option<u64>,
    /// Declared media type, when the descriptor stated one.
    pub media_type: Option<String>,
    /// Expected SHA-256, only when the descriptor is verified.
    pub sha256: Option<Sha256Digest>,
    /// Whether the descriptor carried a verified digest.
    pub verified: bool,
}

/// Admitted remote bytes: content identity checked, no GPU/UI types.
#[derive(Clone)]
pub struct AdmittedRemote {
    bytes: Vec<u8>,
    digest: Sha256Digest,
    format: AcceptedFormat,
    verified: bool,
}

impl std::fmt::Debug for AdmittedRemote {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmittedRemote")
            .field("bytes", &format!("<{} bytes>", self.bytes.len()))
            .field("digest", &self.digest)
            .field("format", &self.format)
            .field("verified", &self.verified)
            .finish()
    }
}

impl AdmittedRemote {
    /// Encoded representation bytes (bounded, verified as stated).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Computed SHA-256 of the admitted bytes.
    pub fn digest(&self) -> Sha256Digest {
        self.digest
    }

    /// Admitted container class.
    pub fn format(&self) -> AcceptedFormat {
        self.format
    }

    /// Whether a descriptor-supplied digest was verified (never invented).
    pub fn verified(&self) -> bool {
        self.verified
    }
}

/// Admit fetched bytes against a descriptor expectation.
///
/// Order: cancellation, bounded size, exact declared length, container class,
/// declared media type, then digest. A digest or type mismatch drops the bytes
/// (quarantine) by returning the error and nothing else.
pub fn admit_remote(
    bytes: &[u8],
    expectation: &RemoteExpectation,
    max_input_bytes: usize,
    cancellation: &Cancellation,
) -> Result<AdmittedRemote, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if bytes.len() > max_input_bytes {
        return Err(MediaError::TooLarge);
    }
    if let Some(expected_length) = expectation.byte_length
        && expected_length != bytes.len() as u64
    {
        return Err(MediaError::LengthMismatch);
    }
    let format = AcceptedFormat::from_bytes(bytes)?;
    if let Some(declared) = expectation.media_type.as_deref()
        && declared != media_type_name(format)
    {
        return Err(MediaError::InvalidMetadata);
    }
    let digest = Sha256Digest(Sha256::digest(bytes).into());
    if expectation.verified {
        let expected = expectation.sha256.ok_or(MediaError::InvalidMetadata)?;
        if digest != expected {
            return Err(MediaError::DigestMismatch);
        }
    }
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    Ok(AdmittedRemote {
        bytes: bytes.to_vec(),
        digest,
        format,
        verified: expectation.verified,
    })
}

fn media_type_name(format: AcceptedFormat) -> &'static str {
    match format {
        AcceptedFormat::Png => "image/png",
        AcceptedFormat::Jpeg => "image/jpeg",
        AcceptedFormat::WebP => "image/webp",
        AcceptedFormat::Gif => "image/gif",
    }
}

/// One bounded cache-scope key: canonical identity, representation and the
/// authorized principal/audience, so bytes admitted for one session are never
/// served to another.
#[derive(Clone, Debug)]
pub struct CacheScope<'a> {
    /// Server identity that issued the descriptor/handle.
    pub server_identity: &'a str,
    /// Desktop principal/audience the handle is valid for.
    pub audience: &'a str,
    /// Canonical artifact digest.
    pub artifact_digest: &'a str,
    /// Representation kind.
    pub representation: desktop_protocol::RepresentationKind,
    /// Representation version.
    pub representation_version: u32,
    /// Expected digest, or `None` for an unverified payload.
    pub expected_digest: Option<Sha256Digest>,
}

/// Build the bounded cache key for one authorized representation.
pub fn cache_key(scope: &CacheScope<'_>) -> Result<String, MediaError> {
    for value in [scope.server_identity, scope.audience, scope.artifact_digest] {
        if value.is_empty()
            || value.len() > 512
            || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
            || value.contains('|')
        {
            return Err(MediaError::InvalidMetadata);
        }
    }
    let representation = match &scope.representation {
        desktop_protocol::RepresentationKind::Metadata => "metadata",
        desktop_protocol::RepresentationKind::Thumbnail => "thumbnail",
        desktop_protocol::RepresentationKind::Preview => "preview",
        desktop_protocol::RepresentationKind::Original => "original",
        desktop_protocol::RepresentationKind::Unknown(_) => {
            return Err(MediaError::InvalidMetadata);
        }
    };
    let digest = scope
        .expected_digest
        .map(|digest| digest.to_string())
        .unwrap_or_else(|| "unverified".to_owned());
    Ok(format!(
        "media-v1|{}|{}|{}|{}|{}|{}",
        scope.server_identity,
        scope.audience,
        scope.artifact_digest,
        representation,
        scope.representation_version,
        digest
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_protocol::RepresentationKind;

    /// One valid 1x1 PNG header (the container check only reads the signature).
    fn png() -> Vec<u8> {
        vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        ]
    }

    fn digest_of(bytes: &[u8]) -> Sha256Digest {
        Sha256Digest(Sha256::digest(bytes).into())
    }

    fn expectation(bytes: &[u8]) -> RemoteExpectation {
        RemoteExpectation {
            byte_length: Some(bytes.len() as u64),
            media_type: Some("image/png".to_owned()),
            sha256: Some(digest_of(bytes)),
            verified: true,
        }
    }

    #[test]
    fn verified_bytes_are_admitted_with_their_computed_digest() {
        let bytes = png();
        let admitted = admit_remote(&bytes, &expectation(&bytes), 1024, &Cancellation::default())
            .expect("verified bytes admitted");
        assert_eq!(admitted.bytes(), bytes.as_slice());
        assert_eq!(admitted.digest(), digest_of(&bytes));
        assert!(admitted.verified());
        assert_eq!(admitted.format(), AcceptedFormat::Png);
    }

    #[test]
    fn unverified_bytes_are_admitted_but_never_reported_verified() {
        let bytes = png();
        let admitted = admit_remote(
            &bytes,
            &RemoteExpectation {
                verified: false,
                sha256: None,
                ..expectation(&bytes)
            },
            1024,
            &Cancellation::default(),
        )
        .expect("unverified bytes admitted");
        assert!(!admitted.verified());
    }

    #[test]
    fn a_length_mismatch_is_refused() {
        let bytes = png();
        let mut expected = expectation(&bytes);
        expected.byte_length = Some(bytes.len() as u64 + 1);
        assert_eq!(
            admit_remote(&bytes, &expected, 1024, &Cancellation::default()).expect_err("length"),
            MediaError::LengthMismatch
        );
    }

    #[test]
    fn a_digest_mismatch_quarantines_and_returns_no_bytes() {
        let bytes = png();
        let mut expected = expectation(&bytes);
        expected.sha256 = Some(Sha256Digest([0x5a; 32]));
        assert_eq!(
            admit_remote(&bytes, &expected, 1024, &Cancellation::default()).expect_err("digest"),
            MediaError::DigestMismatch
        );
    }

    #[test]
    fn a_declared_verified_payload_without_a_digest_is_refused() {
        let bytes = png();
        let mut expected = expectation(&bytes);
        expected.sha256 = None;
        assert_eq!(
            admit_remote(&bytes, &expected, 1024, &Cancellation::default())
                .expect_err("verified without digest"),
            MediaError::InvalidMetadata
        );
    }

    #[test]
    fn a_declared_media_type_must_match_the_container() {
        let bytes = png();
        let mut expected = expectation(&bytes);
        expected.media_type = Some("image/jpeg".to_owned());
        assert_eq!(
            admit_remote(&bytes, &expected, 1024, &Cancellation::default()).expect_err("type"),
            MediaError::InvalidMetadata
        );
    }

    #[test]
    fn an_unsupported_container_and_an_oversized_payload_are_refused() {
        let bytes = b"not an image".to_vec();
        assert_eq!(
            admit_remote(&bytes, &expectation(&bytes), 1024, &Cancellation::default())
                .expect_err("container"),
            MediaError::UnsupportedContainer
        );
        let png = png();
        assert_eq!(
            admit_remote(&png, &expectation(&png), 8, &Cancellation::default())
                .expect_err("oversized"),
            MediaError::TooLarge
        );
    }

    #[test]
    fn cancellation_before_or_after_the_digest_is_refused() {
        let bytes = png();
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert_eq!(
            admit_remote(&bytes, &expectation(&bytes), 1024, &cancellation).expect_err("cancel"),
            MediaError::Cancelled
        );
    }

    #[test]
    fn the_cache_key_is_scoped_to_the_authorized_audience() {
        let digest = digest_of(&png());
        let base = CacheScope {
            server_identity: "server-1",
            audience: "desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            artifact_digest: "sha256:artifact",
            representation: RepresentationKind::Original,
            representation_version: 1,
            expected_digest: Some(digest),
        };
        let other_audience = CacheScope {
            audience: "desktop:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ..base.clone()
        };
        let unverified = CacheScope {
            expected_digest: None,
            ..base.clone()
        };
        assert_ne!(
            cache_key(&base).expect("key"),
            cache_key(&other_audience).expect("key")
        );
        assert_ne!(
            cache_key(&base).expect("key"),
            cache_key(&unverified).expect("key")
        );
        assert_eq!(
            cache_key(&base).expect("key"),
            cache_key(&base).expect("key")
        );
    }

    #[test]
    fn a_cache_key_component_cannot_smuggle_the_separator() {
        let scope = CacheScope {
            server_identity: "server|1",
            audience: "desktop:a",
            artifact_digest: "artifact",
            representation: RepresentationKind::Original,
            representation_version: 1,
            expected_digest: None,
        };
        assert_eq!(
            cache_key(&scope).expect_err("separator"),
            MediaError::InvalidMetadata
        );
    }
}
