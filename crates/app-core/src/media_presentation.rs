//! RT6/IMG6 media availability for a selected candidate/artifact.
//!
//! This module owns the Desktop-side availability vocabulary and the
//! descriptor/cache/admission hand-off between an authorized Runtime
//! descriptor and the presentation layer. It never constructs an available
//! state without bytes that `media::admit_remote` accepted under the
//! descriptor's own stated expectations, never falls back to a fixture, and
//! never renders a credential, media handle or read context.
//!
//! Media authorization itself stays with the Runtime-validated descriptor
//! (`cos-client`); this module duplicates none of that decision. It carries no
//! GPUI type, performs no I/O of its own and adds no network or TLS code: the
//! fetch side is the injected [`MediaSource`], implemented by the binary
//! boundary. Live Runtime evidence therefore remains pending Runtime task-32.
//!
//! Availability stays an independent axis. `Expired` (server 410) is a
//! reauthorization request and is never flattened into `Missing`; an expired
//! descriptor never invalidates bytes already admitted under the same
//! authorized principal/identity/expected-digest cache key.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use cos_client::media::{
    Cancellation, FetchedMedia, MediaExecutor, MediaFetchError, MediaFetcher, expected_sha256,
    representation_budget,
};
use desktop_protocol::{
    ArtifactDigest, ArtifactId, ArtifactSnapshot, Availability, Counter, EpochId, MediaDescriptor,
    RepresentationKind, ServerId,
};
use media::{
    CacheScope, LocalLimits, MediaCache, MediaError, RemoteCacheKey, RemoteExpectation,
    Sha256Digest, Usage, admit_remote,
};

/// Maximum number of representations whose admitted identity is remembered.
///
/// The map only serves the expired-descriptor cache fallback; it is bounded so
/// a long session cannot retain an unbounded identity table.
const MAX_REMEMBERED_IDENTITIES: usize = 64;

/// Descriptor reads allowed when the server reports expiry (the initial read plus
/// one explicit reauthorization).
const MAX_DESCRIPTOR_ATTEMPTS: usize = 2;

/// Byte reads allowed after a descriptor reauthorization.
const MAX_BYTE_ATTEMPTS: usize = 2;

/// Authoritative context one media selection is read under.
///
/// The read context is the RC-03 observation context: it is sent only as the
/// client's header value and is never rendered, logged, formatted or placed in
/// an error. Debug output is deliberately redacted.
#[derive(Clone)]
pub struct MediaSessionContext {
    server_identity: ServerId,
    audience: String,
    epoch: EpochId,
    read_context: String,
}

impl MediaSessionContext {
    /// Bind one selection to the current server identity, principal/audience,
    /// Runtime epoch and authorized read context.
    pub fn new(
        server_identity: ServerId,
        audience: impl Into<String>,
        epoch: EpochId,
        read_context: impl Into<String>,
    ) -> Self {
        Self {
            server_identity,
            audience: audience.into(),
            epoch,
            read_context: read_context.into(),
        }
    }

    /// Server identity the descriptor and handle must belong to.
    pub fn server_identity(&self) -> &ServerId {
        &self.server_identity
    }

    /// Desktop principal/audience the media is authorized for.
    pub fn audience(&self) -> &str {
        &self.audience
    }

    /// Runtime epoch the canonical artifact identity belongs to.
    pub fn epoch(&self) -> &EpochId {
        &self.epoch
    }

    /// Authorized read context. Callers forward it to the fetcher and never render it.
    pub fn read_context(&self) -> &str {
        &self.read_context
    }
}

impl fmt::Debug for MediaSessionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaSessionContext")
            .field("server_identity", &self.server_identity)
            .field("audience", &self.audience)
            .field("epoch", &self.epoch)
            .field("read_context", &"<redacted>")
            .finish()
    }
}

/// One selected artifact/representation to resolve for presentation.
#[derive(Clone, Debug)]
pub struct MediaSelection {
    session: MediaSessionContext,
    artifact_ref: ArtifactId,
    artifact_digest: ArtifactDigest,
    revision: Counter,
    representation: RepresentationKind,
    declared: Availability,
}

impl MediaSelection {
    /// Select one representation of an authoritative artifact projection.
    ///
    /// The canonical artifact digest and projection revision come from the
    /// projection; neither is inferred from a descriptor, a byte payload or a
    /// fixture. The projection's own availability is retained so an
    /// unavailable artifact is rendered as such before any fetch.
    pub fn for_artifact(
        snapshot: &ArtifactSnapshot,
        session: MediaSessionContext,
        representation: RepresentationKind,
    ) -> Self {
        Self {
            session,
            artifact_ref: snapshot.artifact_ref.clone(),
            artifact_digest: snapshot.artifact_digest.clone(),
            revision: snapshot.projection_revision.clone(),
            representation,
            declared: snapshot.availability.clone(),
        }
    }

    /// The context this selection is read under.
    pub fn session(&self) -> &MediaSessionContext {
        &self.session
    }

    /// Canonical artifact identity.
    pub fn artifact_ref(&self) -> &ArtifactId {
        &self.artifact_ref
    }

    /// Canonical artifact digest addressed by the descriptor.
    pub fn artifact_digest(&self) -> &ArtifactDigest {
        &self.artifact_digest
    }

    /// Projection revision of the selected artifact.
    pub fn revision(&self) -> &Counter {
        &self.revision
    }

    /// Selected representation kind.
    pub fn representation(&self) -> &RepresentationKind {
        &self.representation
    }

    /// Availability the authoritative projection declared for the artifact.
    pub fn declared_availability(&self) -> &Availability {
        &self.declared
    }
}

/// Value-free media availability states.
///
/// These are independent of Task status, connection state and verification:
/// none of them is derived from another axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaAvailability {
    /// Bytes were admitted under the descriptor's stated expectations.
    Available,
    /// The descriptor/handle expired (server 410); it needs reauthorization.
    Expired,
    /// The artifact or representation does not exist.
    Missing,
    /// The current principal/audience is not authorized for it.
    Forbidden,
    /// The representation is not ready yet.
    Pending,
    /// This build cannot represent or decode it.
    Unsupported,
    /// Fetch, verification or admission failed; nothing was displayed.
    Failed,
}

impl MediaAvailability {
    /// Map a value-free client fetch failure to the availability vocabulary.
    ///
    /// `None` means cancellation: cancellation is not an availability state and
    /// the caller keeps its current presentation.
    pub fn from_fetch_error(error: MediaFetchError) -> Option<Self> {
        Some(match error {
            // A rejected credential and a mismatched principal both mean this
            // session may not see the representation; neither is `Missing`.
            MediaFetchError::Unauthorized
            | MediaFetchError::Forbidden
            | MediaFetchError::AudienceMismatch => Self::Forbidden,
            MediaFetchError::NotFound => Self::Missing,
            MediaFetchError::Pending => Self::Pending,
            // Expiry asks for reauthorization and is never flattened into Missing.
            MediaFetchError::DescriptorExpired => Self::Expired,
            MediaFetchError::Unsupported => Self::Unsupported,
            MediaFetchError::TooLarge
            | MediaFetchError::RangeUnsatisfiable
            | MediaFetchError::MalformedDescriptor
            | MediaFetchError::HandleRejected
            | MediaFetchError::ServerIdentityMismatch
            | MediaFetchError::Quarantined
            | MediaFetchError::Failed
            | MediaFetchError::Transport
            | MediaFetchError::Oversized
            | MediaFetchError::LengthMismatch
            | MediaFetchError::UnexpectedStatus => Self::Failed,
            MediaFetchError::Cancelled => return None,
        })
    }

    /// Map an admission refusal. `None` means cancellation.
    pub fn from_admission_error(error: MediaError) -> Option<Self> {
        Some(match error {
            MediaError::UnsupportedContainer => Self::Unsupported,
            MediaError::Cancelled => return None,
            _ => Self::Failed,
        })
    }

    /// Map the authoritative projection's declared artifact availability.
    ///
    /// An unrecognized wire value stays an explicit failure; it never defaults
    /// to available.
    pub fn from_declared(availability: &Availability) -> Self {
        match availability {
            Availability::Available => Self::Available,
            Availability::Staged | Availability::Pending => Self::Pending,
            Availability::Missing => Self::Missing,
            Availability::Forbidden => Self::Forbidden,
            Availability::Unsupported => Self::Unsupported,
            Availability::Failed | Availability::Quarantined | Availability::Unknown(_) => {
                Self::Failed
            }
        }
    }

    /// Stable diagnostic label. Never a user-facing sentence.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Expired => "expired",
            Self::Missing => "missing",
            Self::Forbidden => "forbidden",
            Self::Pending => "pending",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
        }
    }

    /// Whether admitted bytes may be presented.
    pub fn is_available(self) -> bool {
        self == Self::Available
    }
}

/// Value-free detail retained with a non-available state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaUnavailableCause {
    /// The authoritative artifact projection already declared it unavailable.
    Declared(Availability),
    /// The descriptor or byte fetch returned a value-free client class.
    Fetch(MediaFetchError),
    /// Admission refused the fetched bytes.
    Admission(MediaError),
    /// The selected identity could not be addressed or the descriptor contradicted it.
    Identity,
}

/// Why one selection is not available, with its value-free cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaUnavailable {
    /// Availability state to render.
    pub availability: MediaAvailability,
    /// Value-free cause retained for diagnostics.
    pub cause: MediaUnavailableCause,
}

/// Canonical presentation identity of one admitted representation.
///
/// It is built only from the selected authoritative projection plus the
/// validated descriptor; a missing digest stays `None` and is never inferred
/// from the artifact digest or from the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaIdentity {
    /// Server identity that issued the descriptor/handle.
    pub server_identity: ServerId,
    /// Desktop principal/audience the representation is authorized for.
    pub audience: String,
    /// Runtime epoch of the canonical artifact identity.
    pub epoch: EpochId,
    /// Canonical artifact identity.
    pub artifact_ref: ArtifactId,
    /// Canonical artifact digest.
    pub artifact_digest: ArtifactDigest,
    /// Projection revision of the selected artifact.
    pub revision: Counter,
    /// Representation kind.
    pub representation: RepresentationKind,
    /// Representation version.
    pub representation_version: u32,
    /// Authorized expected SHA-256, present only for a `VERIFIED` descriptor.
    pub expected_sha256: Option<Sha256Digest>,
    /// Descriptor-stated byte length, when known.
    pub byte_length: Option<u64>,
    /// Descriptor-stated pixel width, when known.
    pub width: Option<u32>,
    /// Descriptor-stated pixel height, when known.
    pub height: Option<u32>,
}

impl MediaIdentity {
    /// Whether a descriptor-supplied digest was verified. Never invented.
    pub fn verified(&self) -> bool {
        self.expected_sha256.is_some()
    }
}

/// Bytes that passed remote admission under one authorized identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedMedia {
    /// Canonical identity the bytes belong to.
    pub identity: MediaIdentity,
    /// Admitted encoded bytes; never control-plane or fixture bytes.
    pub bytes: Arc<[u8]>,
    /// SHA-256 computed over the admitted bytes, never copied from a descriptor.
    pub computed_digest: Sha256Digest,
    /// Bounded admission limit the bytes passed under. Callers must not relax it.
    pub max_bytes: usize,
}

/// Result of resolving one selection for presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaPresentation {
    /// Bytes were admitted; the identity is authoritative and value-free.
    Admitted(Box<AdmittedMedia>),
    /// Not available; the state is explicit and no bytes are carried.
    Unavailable(MediaUnavailable),
    /// Cancellation is not an availability state; the caller keeps its current state.
    Cancelled,
}

impl MediaPresentation {
    /// The admitted bytes, only when this presentation is available.
    pub fn admitted(&self) -> Option<&AdmittedMedia> {
        match self {
            Self::Admitted(admitted) => Some(admitted.as_ref()),
            Self::Unavailable(_) | Self::Cancelled => None,
        }
    }

    /// The explicit unavailable state, only when it is not available.
    pub fn unavailable(&self) -> Option<&MediaUnavailable> {
        match self {
            Self::Unavailable(unavailable) => Some(unavailable),
            Self::Admitted(_) | Self::Cancelled => None,
        }
    }
}

/// Descriptor and byte fetch side, implemented by the binary boundary.
///
/// `cos-client`'s transport-independent [`MediaFetcher`] implements this
/// directly, so no client integration helper is added.
pub trait MediaSource {
    /// Fetch and validate the closed descriptor for one canonical artifact digest.
    fn fetch_descriptor(
        &mut self,
        artifact_digest: &str,
        context: &str,
        cancellation: &Cancellation,
    ) -> Result<MediaDescriptor, MediaFetchError>;

    /// Fetch the bounded bytes for one validated descriptor.
    fn fetch_bytes(
        &mut self,
        descriptor: &MediaDescriptor,
        cancellation: &Cancellation,
    ) -> Result<FetchedMedia, MediaFetchError>;
}

impl<E: MediaExecutor> MediaSource for MediaFetcher<E> {
    fn fetch_descriptor(
        &mut self,
        artifact_digest: &str,
        context: &str,
        cancellation: &Cancellation,
    ) -> Result<MediaDescriptor, MediaFetchError> {
        MediaFetcher::fetch_descriptor(self, artifact_digest, context, cancellation)
    }

    fn fetch_bytes(
        &mut self,
        descriptor: &MediaDescriptor,
        cancellation: &Cancellation,
    ) -> Result<FetchedMedia, MediaFetchError> {
        MediaFetcher::fetch_bytes(self, descriptor, cancellation)
    }
}

/// Bounded identity used only by the expired-descriptor cache fallback.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RememberKey {
    server_identity: ServerId,
    audience: String,
    artifact_ref: ArtifactId,
    representation: RepresentationKind,
}

impl RememberKey {
    fn for_selection(selection: &MediaSelection) -> Self {
        Self {
            server_identity: selection.session.server_identity.clone(),
            audience: selection.session.audience.clone(),
            artifact_ref: selection.artifact_ref.clone(),
            representation: selection.representation.clone(),
        }
    }

    fn for_identity(identity: &MediaIdentity) -> Self {
        Self {
            server_identity: identity.server_identity.clone(),
            audience: identity.audience.clone(),
            artifact_ref: identity.artifact_ref.clone(),
            representation: identity.representation.clone(),
        }
    }
}

/// Bounded media cache and admission boundary for one Desktop process.
///
/// The cache is keyed by the per-principal bounded identity produced by
/// `media::cache_key`: server identity, audience, canonical artifact digest,
/// representation/version and the expected digest (or the explicit unverified
/// marker). Bytes admitted for one principal are never served to another.
pub struct MediaRepository {
    cache: MediaCache,
    remembered: BTreeMap<RememberKey, MediaIdentity>,
    limits: LocalLimits,
}

impl MediaRepository {
    /// Create the bounded repository. Limits are the same encoded-payload ceiling
    /// `media::MediaCache` enforces for local inputs.
    pub fn new(limits: LocalLimits) -> Result<Self, MediaError> {
        Ok(Self {
            cache: MediaCache::new(limits)?,
            remembered: BTreeMap::new(),
            limits,
        })
    }

    /// Current retained encoded-byte and entry usage.
    pub fn cache_usage(&self) -> Result<Usage, MediaError> {
        self.cache.usage()
    }

    /// Resolve one selected representation to an explicit presentation outcome.
    ///
    /// Order: authoritative declared availability, descriptor, digest budget,
    /// cache, bounded byte transfer, admission. No step fabricates an available
    /// state: only `admit_remote` may produce one, and an expired descriptor is
    /// reported as `Expired` (never `Missing`) unless a digest-matching entry is
    /// still retained for the same authorized identity.
    pub fn load(
        &mut self,
        source: &mut dyn MediaSource,
        selection: &MediaSelection,
        cancellation: &Cancellation,
    ) -> MediaPresentation {
        if cancellation.is_cancelled() {
            return MediaPresentation::Cancelled;
        }
        // The authoritative projection's own state is shown before any fetch.
        if !selection.declared_availability().is_available() {
            return MediaPresentation::Unavailable(MediaUnavailable {
                availability: MediaAvailability::from_declared(selection.declared_availability()),
                cause: MediaUnavailableCause::Declared(selection.declared_availability().clone()),
            });
        }
        // An unrecognized representation cannot be fetched or presented safely.
        if !selection.representation().is_known() {
            return unavailable(
                MediaAvailability::Unsupported,
                MediaUnavailableCause::Identity,
            );
        }
        let descriptor = match self.read_descriptor(source, selection, cancellation) {
            Ok(descriptor) => descriptor,
            Err(outcome) => return self.expired_cache_fallback(selection, outcome),
        };
        if descriptor.artifact_ref != *selection.artifact_ref()
            || descriptor.representation != *selection.representation()
        {
            // A descriptor for another artifact or representation is never presented as this one.
            return unavailable(MediaAvailability::Failed, MediaUnavailableCause::Identity);
        }
        if !descriptor.availability.is_available() {
            return MediaPresentation::Unavailable(MediaUnavailable {
                availability: MediaAvailability::from_declared(&descriptor.availability),
                cause: MediaUnavailableCause::Declared(descriptor.availability.clone()),
            });
        }
        if let Err(error) = expected_sha256(&descriptor) {
            return unavailable_from_fetch(error);
        }
        let identity = MediaIdentity::for_selection(selection, &descriptor);
        if let Some(presentation) = self.cached_presentation(&identity) {
            self.remember(&identity);
            return presentation;
        }
        let budget = self.effective_budget(&identity.representation);
        if identity
            .byte_length
            .is_some_and(|length| length > budget as u64)
        {
            // A declared representation above the admitted budget is refused before transfer.
            return unavailable(
                MediaAvailability::Failed,
                MediaUnavailableCause::Fetch(MediaFetchError::TooLarge),
            );
        }
        let fetched = match self.read_bytes(source, selection, &descriptor, cancellation) {
            Ok(fetched) => fetched,
            Err(outcome) => return outcome,
        };
        let expectation = RemoteExpectation {
            byte_length: identity.byte_length,
            media_type: fetched.descriptor.media_type.clone(),
            sha256: identity.expected_sha256,
            verified: identity.expected_sha256.is_some(),
        };
        if cancellation.is_cancelled() {
            return MediaPresentation::Cancelled;
        }
        // Admission samples cancellation only around its bounded hash step, so the
        // fetch cancellation is re-checked immediately before and after it.
        let admitted = match admit_remote(
            &fetched.bytes,
            &expectation,
            budget,
            &media::Cancellation::default(),
        ) {
            Ok(admitted) => admitted,
            Err(MediaError::Cancelled) => return MediaPresentation::Cancelled,
            Err(error) => return unavailable_from_admission(error),
        };
        if cancellation.is_cancelled() {
            return MediaPresentation::Cancelled;
        }
        let key = match key_for_identity(&identity) {
            Ok(key) => key,
            Err(outcome) => return outcome,
        };
        let entry = match self.cache.insert_remote(key, admitted) {
            Ok(entry) => entry,
            Err(MediaError::Cancelled) => return MediaPresentation::Cancelled,
            Err(error) => return unavailable_from_admission(error),
        };
        self.remember(&identity);
        MediaPresentation::Admitted(Box::new(AdmittedMedia {
            identity,
            bytes: Arc::from(entry.bytes()),
            computed_digest: entry.admitted().digest(),
            max_bytes: budget,
        }))
    }

    /// Read the descriptor, allowing one reauthorization when the server reports expiry.
    fn read_descriptor(
        &self,
        source: &mut dyn MediaSource,
        selection: &MediaSelection,
        cancellation: &Cancellation,
    ) -> Result<MediaDescriptor, MediaPresentation> {
        for _ in 0..MAX_DESCRIPTOR_ATTEMPTS {
            if cancellation.is_cancelled() {
                return Err(MediaPresentation::Cancelled);
            }
            match source.fetch_descriptor(
                selection.artifact_digest.as_str(),
                selection.session.read_context(),
                cancellation,
            ) {
                Ok(descriptor) => return Ok(descriptor),
                // Every remaining attempt is an expiry reauthorization.
                Err(MediaFetchError::DescriptorExpired) => {}
                Err(error) => return Err(unavailable_from_fetch(error)),
            }
        }
        Err(unavailable(
            MediaAvailability::Expired,
            MediaUnavailableCause::Fetch(MediaFetchError::DescriptorExpired),
        ))
    }

    /// Read the bytes, reauthorizing once when the server reports the handle expired.
    fn read_bytes(
        &self,
        source: &mut dyn MediaSource,
        selection: &MediaSelection,
        descriptor: &MediaDescriptor,
        cancellation: &Cancellation,
    ) -> Result<FetchedMedia, MediaPresentation> {
        let mut current = descriptor.clone();
        for attempt in 1..=MAX_BYTE_ATTEMPTS {
            if cancellation.is_cancelled() {
                return Err(MediaPresentation::Cancelled);
            }
            match source.fetch_bytes(&current, cancellation) {
                Ok(fetched) => return Ok(fetched),
                Err(MediaFetchError::DescriptorExpired) if attempt < MAX_BYTE_ATTEMPTS => {
                    // The handle expired: reauthorize the descriptor once, and continue
                    // only when it still names the same representation and digest.
                    match source.fetch_descriptor(
                        selection.artifact_digest.as_str(),
                        selection.session.read_context(),
                        cancellation,
                    ) {
                        Ok(next) if same_representation(descriptor, &next) => current = next,
                        Ok(_) => {
                            return Err(unavailable(
                                MediaAvailability::Failed,
                                MediaUnavailableCause::Identity,
                            ));
                        }
                        Err(MediaFetchError::DescriptorExpired) => {
                            return Err(unavailable(
                                MediaAvailability::Expired,
                                MediaUnavailableCause::Fetch(MediaFetchError::DescriptorExpired),
                            ));
                        }
                        Err(error) => return Err(unavailable_from_fetch(error)),
                    }
                }
                Err(error) => return Err(unavailable_from_fetch(error)),
            }
        }
        Err(unavailable(
            MediaAvailability::Expired,
            MediaUnavailableCause::Fetch(MediaFetchError::DescriptorExpired),
        ))
    }

    /// The retained presentation for an already admitted identity, if any.
    fn cached_presentation(&self, identity: &MediaIdentity) -> Option<MediaPresentation> {
        let key = key_for_identity(identity).ok()?;
        let cached = self.cache.get_remote(&key)?;
        Some(MediaPresentation::Admitted(Box::new(AdmittedMedia {
            identity: identity.clone(),
            bytes: Arc::from(cached.bytes()),
            computed_digest: cached.admitted().digest(),
            max_bytes: self.effective_budget(&identity.representation),
        })))
    }

    /// An expired descriptor does not invalidate a digest-matching cache entry.
    fn expired_cache_fallback(
        &self,
        selection: &MediaSelection,
        outcome: MediaPresentation,
    ) -> MediaPresentation {
        let MediaPresentation::Unavailable(unavailable) = &outcome else {
            return outcome;
        };
        if unavailable.availability != MediaAvailability::Expired {
            return outcome;
        }
        let Some(identity) = self.remembered.get(&RememberKey::for_selection(selection)) else {
            return outcome;
        };
        self.cached_presentation(identity).unwrap_or(outcome)
    }

    fn effective_budget(&self, representation: &RepresentationKind) -> usize {
        let protocol = representation_budget(representation);
        usize::try_from(protocol)
            .map(|budget| budget.min(self.limits.max_input_bytes))
            .unwrap_or(self.limits.max_input_bytes)
    }

    /// Remember a bounded number of admitted identities for the expiry fallback.
    fn remember(&mut self, identity: &MediaIdentity) {
        let key = RememberKey::for_identity(identity);
        if self.remembered.len() >= MAX_REMEMBERED_IDENTITIES
            && !self.remembered.contains_key(&key)
            && let Some(oldest) = self.remembered.keys().next().cloned()
        {
            self.remembered.remove(&oldest);
        }
        self.remembered.insert(key, identity.clone());
    }
}

impl MediaIdentity {
    fn for_selection(selection: &MediaSelection, descriptor: &MediaDescriptor) -> Self {
        Self {
            server_identity: selection.session.server_identity.clone(),
            audience: selection.session.audience.clone(),
            epoch: selection.session.epoch.clone(),
            artifact_ref: selection.artifact_ref.clone(),
            artifact_digest: selection.artifact_digest.clone(),
            revision: selection.revision.clone(),
            representation: descriptor.representation.clone(),
            representation_version: descriptor.representation_version,
            expected_sha256: expected_sha256(descriptor).ok().flatten().map(Sha256Digest),
            byte_length: descriptor.byte_length,
            width: descriptor.width,
            height: descriptor.height,
        }
    }
}

/// Whether a reauthorized descriptor still names the same representation and digest.
fn same_representation(reference: &MediaDescriptor, next: &MediaDescriptor) -> bool {
    next.artifact_ref == reference.artifact_ref
        && next.representation == reference.representation
        && next.representation_version == reference.representation_version
        && next.availability.is_available()
        && expected_sha256(reference).ok() == expected_sha256(next).ok()
}

fn key_for_identity(identity: &MediaIdentity) -> Result<RemoteCacheKey, MediaPresentation> {
    let scope = CacheScope {
        server_identity: identity.server_identity.as_str(),
        audience: identity.audience.as_str(),
        artifact_digest: identity.artifact_digest.as_str(),
        representation: identity.representation.clone(),
        representation_version: identity.representation_version,
        expected_digest: identity.expected_sha256,
    };
    RemoteCacheKey::for_scope(&scope)
        .map_err(|_| unavailable(MediaAvailability::Failed, MediaUnavailableCause::Identity))
}

fn unavailable(availability: MediaAvailability, cause: MediaUnavailableCause) -> MediaPresentation {
    MediaPresentation::Unavailable(MediaUnavailable {
        availability,
        cause,
    })
}

fn unavailable_from_fetch(error: MediaFetchError) -> MediaPresentation {
    match MediaAvailability::from_fetch_error(error) {
        Some(availability) => unavailable(availability, MediaUnavailableCause::Fetch(error)),
        // Cancellation is never rendered as an availability state.
        None => MediaPresentation::Cancelled,
    }
}

fn unavailable_from_admission(error: MediaError) -> MediaPresentation {
    match MediaAvailability::from_admission_error(error) {
        Some(availability) => unavailable(availability, MediaUnavailableCause::Admission(error)),
        None => MediaPresentation::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_protocol::{DescriptorId, VerificationState, VerificationSummary};
    use std::collections::VecDeque;

    const SERVER: &str = "server-1";
    const AUDIENCE: &str = "desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_AUDIENCE: &str = "desktop:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const ARTIFACT_DIGEST: &str = "sha256:artifact-digest";

    /// A bounded PNG payload; admission reads only the container signature.
    fn png(seed: u8) -> Vec<u8> {
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        ];
        bytes.push(seed);
        bytes
    }

    /// Compute the digest of one payload through the unverified admission path so the
    /// test never reimplements SHA-256 or invents an expected value.
    fn digest_of(bytes: &[u8]) -> Sha256Digest {
        admit_remote(
            bytes,
            &RemoteExpectation {
                byte_length: Some(bytes.len() as u64),
                media_type: Some("image/png".to_owned()),
                sha256: None,
                verified: false,
            },
            1024,
            &media::Cancellation::default(),
        )
        .expect("unverified admission")
        .digest()
    }

    /// Render the bare lowercase hex form the descriptor's verification summary carries.
    fn expected_hex(digest: Sha256Digest) -> String {
        use std::fmt::Write as _;
        let mut text = String::with_capacity(64);
        for byte in digest.0 {
            // Writing into a String cannot fail.
            let _ = write!(text, "{byte:02x}");
        }
        text
    }

    fn descriptor(expected: Option<Sha256Digest>, availability: Availability) -> MediaDescriptor {
        MediaDescriptor {
            descriptor_id: DescriptorId::new("descriptor-1").expect("descriptor id"),
            artifact_ref: ArtifactId::new("artifact-a").expect("artifact id"),
            artifact_digest: Some(ArtifactDigest::new(ARTIFACT_DIGEST).expect("artifact digest")),
            representation: RepresentationKind::Original,
            representation_version: 1,
            media_type: Some("image/png".to_owned()),
            byte_length: Some(png(0).len() as u64),
            width: Some(2048),
            height: Some(2048),
            duration_ms: None,
            availability,
            verification: expected.map(|digest| VerificationSummary {
                state: VerificationState::Verified,
                algorithm: Some("sha256".to_owned()),
                expected_digest: Some(expected_hex(digest)),
                observed_digest: None,
            }),
            access_handle: Some("/api/cos/media/handle/opaque".to_owned()),
            server_identity: Some(SERVER.to_owned()),
            audience: Some(AUDIENCE.to_owned()),
            expires_at_unix_ms: None,
        }
    }

    fn session(audience: &str) -> MediaSessionContext {
        MediaSessionContext::new(
            ServerId::new(SERVER).expect("server identity"),
            audience,
            EpochId::new("epoch-1").expect("epoch"),
            "read-context-value",
        )
    }

    fn snapshot(availability: Availability) -> ArtifactSnapshot {
        ArtifactSnapshot {
            artifact_ref: ArtifactId::new("artifact-a").expect("artifact id"),
            artifact_digest: ArtifactDigest::new(ARTIFACT_DIGEST).expect("artifact digest"),
            media_type: Some("image/png".to_owned()),
            availability,
            descriptor_refs: Vec::new(),
            verification: None,
            projection_revision: Counter::from_u128(3),
        }
    }

    fn selection(audience: &str, availability: Availability) -> MediaSelection {
        MediaSelection::for_artifact(
            &snapshot(availability),
            session(audience),
            RepresentationKind::Original,
        )
    }

    fn new_repository() -> MediaRepository {
        MediaRepository::new(LocalLimits {
            max_input_bytes: 4096,
            max_live_bytes: 1 << 20,
            max_live_entries: 8,
            max_pixels: 16_777_216,
        })
        .expect("repository")
    }

    #[derive(Default)]
    struct Stub {
        descriptors: VecDeque<Result<MediaDescriptor, MediaFetchError>>,
        bytes: VecDeque<Result<Vec<u8>, MediaFetchError>>,
        descriptor_calls: usize,
        byte_calls: usize,
    }

    impl Stub {
        fn with(
            descriptors: impl IntoIterator<Item = Result<MediaDescriptor, MediaFetchError>>,
            bytes: impl IntoIterator<Item = Result<Vec<u8>, MediaFetchError>>,
        ) -> Self {
            Self {
                descriptors: descriptors.into_iter().collect(),
                bytes: bytes.into_iter().collect(),
                ..Self::default()
            }
        }
    }

    impl MediaSource for Stub {
        fn fetch_descriptor(
            &mut self,
            _artifact_digest: &str,
            _context: &str,
            _cancellation: &Cancellation,
        ) -> Result<MediaDescriptor, MediaFetchError> {
            self.descriptor_calls += 1;
            self.descriptors
                .pop_front()
                .unwrap_or(Err(MediaFetchError::Transport))
        }

        fn fetch_bytes(
            &mut self,
            descriptor: &MediaDescriptor,
            _cancellation: &Cancellation,
        ) -> Result<FetchedMedia, MediaFetchError> {
            self.byte_calls += 1;
            self.bytes
                .pop_front()
                .map(|result| {
                    result.map(|bytes| FetchedMedia {
                        descriptor: descriptor.clone(),
                        bytes,
                    })
                })
                .unwrap_or(Err(MediaFetchError::Transport))
        }
    }

    fn load(
        repository: &mut MediaRepository,
        stub: &mut Stub,
        selection: &MediaSelection,
    ) -> MediaPresentation {
        repository.load(stub, selection, &Cancellation::default())
    }

    #[test]
    fn every_fetch_failure_class_maps_to_one_availability_state_and_expiry_is_not_missing() {
        for (error, expected) in [
            (MediaFetchError::Unauthorized, MediaAvailability::Forbidden),
            (MediaFetchError::Forbidden, MediaAvailability::Forbidden),
            (
                MediaFetchError::AudienceMismatch,
                MediaAvailability::Forbidden,
            ),
            (MediaFetchError::NotFound, MediaAvailability::Missing),
            (MediaFetchError::Pending, MediaAvailability::Pending),
            (
                MediaFetchError::DescriptorExpired,
                MediaAvailability::Expired,
            ),
            (MediaFetchError::Unsupported, MediaAvailability::Unsupported),
            (MediaFetchError::TooLarge, MediaAvailability::Failed),
            (
                MediaFetchError::RangeUnsatisfiable,
                MediaAvailability::Failed,
            ),
            (
                MediaFetchError::MalformedDescriptor,
                MediaAvailability::Failed,
            ),
            (MediaFetchError::HandleRejected, MediaAvailability::Failed),
            (
                MediaFetchError::ServerIdentityMismatch,
                MediaAvailability::Failed,
            ),
            (MediaFetchError::Quarantined, MediaAvailability::Failed),
            (MediaFetchError::Failed, MediaAvailability::Failed),
            (MediaFetchError::Transport, MediaAvailability::Failed),
            (MediaFetchError::Oversized, MediaAvailability::Failed),
            (MediaFetchError::LengthMismatch, MediaAvailability::Failed),
            (MediaFetchError::UnexpectedStatus, MediaAvailability::Failed),
        ] {
            assert_eq!(
                MediaAvailability::from_fetch_error(error),
                Some(expected),
                "{error:?} must map to {expected:?}"
            );
        }
        // Cancellation is not an availability state.
        assert_eq!(
            MediaAvailability::from_fetch_error(MediaFetchError::Cancelled),
            None
        );
        // 410 is Expired and never Missing.
        assert_ne!(
            MediaAvailability::from_fetch_error(MediaFetchError::DescriptorExpired),
            Some(MediaAvailability::Missing)
        );
        assert_ne!(
            MediaAvailability::from_fetch_error(MediaFetchError::NotFound),
            Some(MediaAvailability::Expired)
        );
    }

    #[test]
    fn admission_refusals_map_to_explicit_states_and_never_to_available() {
        for (error, expected) in [
            (
                MediaError::UnsupportedContainer,
                MediaAvailability::Unsupported,
            ),
            (MediaError::DigestMismatch, MediaAvailability::Failed),
            (MediaError::LengthMismatch, MediaAvailability::Failed),
            (MediaError::InvalidMetadata, MediaAvailability::Failed),
            (MediaError::TooLarge, MediaAvailability::Failed),
            (MediaError::KeyAlreadyCached, MediaAvailability::Failed),
        ] {
            let mapped = MediaAvailability::from_admission_error(error);
            assert_eq!(mapped, Some(expected), "{error:?} must map to {expected:?}");
            assert_ne!(mapped, Some(MediaAvailability::Available));
        }
        assert_eq!(
            MediaAvailability::from_admission_error(MediaError::Cancelled),
            None
        );
    }

    #[test]
    fn declared_availability_is_shown_without_a_fetch_and_never_as_available() {
        for (declared, expected) in [
            (Availability::Missing, MediaAvailability::Missing),
            (Availability::Forbidden, MediaAvailability::Forbidden),
            (Availability::Pending, MediaAvailability::Pending),
            (Availability::Staged, MediaAvailability::Pending),
            (Availability::Unsupported, MediaAvailability::Unsupported),
            (Availability::Failed, MediaAvailability::Failed),
            (Availability::Quarantined, MediaAvailability::Failed),
            (
                Availability::Unknown("FUTURE".to_owned()),
                MediaAvailability::Failed,
            ),
        ] {
            let mut repository = new_repository();
            let mut stub = Stub::default();
            let presentation = load(
                &mut repository,
                &mut stub,
                &selection(AUDIENCE, declared.clone()),
            );
            let unavailable = presentation
                .unavailable()
                .unwrap_or_else(|| panic!("{declared:?} must not be available"));
            assert_eq!(unavailable.availability, expected);
            assert_eq!(
                unavailable.cause,
                MediaUnavailableCause::Declared(declared.clone())
            );
            // Declared unavailability is answered before any network-shaped call.
            assert_eq!(stub.descriptor_calls, 0);
            assert_eq!(stub.byte_calls, 0);
        }
        // Unknown availability is explicit failure, never a defaulted success.
        assert_eq!(
            MediaAvailability::from_declared(&Availability::Unknown("FUTURE".to_owned())),
            MediaAvailability::Failed
        );
    }

    #[test]
    fn a_missing_descriptor_is_missing_and_an_expired_descriptor_is_expired() {
        let mut repository = new_repository();
        let mut missing = Stub::with([Err(MediaFetchError::NotFound)], []);
        let outcome = load(
            &mut repository,
            &mut missing,
            &selection(AUDIENCE, Availability::Available),
        );
        let unavailable = outcome.unavailable().expect("missing is explicit");
        assert_eq!(unavailable.availability, MediaAvailability::Missing);
        assert_eq!(
            unavailable.cause,
            MediaUnavailableCause::Fetch(MediaFetchError::NotFound)
        );

        let mut expired = Stub::with(
            [
                Err(MediaFetchError::DescriptorExpired),
                Err(MediaFetchError::DescriptorExpired),
            ],
            [],
        );
        let outcome = load(
            &mut repository,
            &mut expired,
            &selection(AUDIENCE, Availability::Available),
        );
        let unavailable = outcome.unavailable().expect("expired is explicit");
        assert_eq!(unavailable.availability, MediaAvailability::Expired);
        assert_ne!(unavailable.availability, MediaAvailability::Missing);
        // One explicit reauthorization was attempted, not an unbounded retry loop.
        assert_eq!(expired.descriptor_calls, MAX_DESCRIPTOR_ATTEMPTS);
    }

    #[test]
    fn only_admitted_bytes_become_available_and_the_identity_is_the_media_identity() {
        let bytes = png(1);
        let digest = digest_of(&bytes);
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [Ok(descriptor(Some(digest), Availability::Available))],
            [Ok(bytes.clone())],
        );
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let admitted = outcome.admitted().expect("verified bytes admitted");
        assert_eq!(admitted.identity.artifact_ref.as_str(), "artifact-a");
        assert_eq!(admitted.identity.artifact_digest.as_str(), ARTIFACT_DIGEST);
        assert_eq!(admitted.identity.server_identity.as_str(), SERVER);
        assert_eq!(admitted.identity.audience, AUDIENCE);
        assert_eq!(admitted.identity.epoch.as_str(), "epoch-1");
        assert_eq!(admitted.identity.revision, Counter::from_u128(3));
        assert_eq!(
            admitted.identity.representation,
            RepresentationKind::Original
        );
        assert!(admitted.identity.verified());
        assert_eq!(admitted.bytes.as_ref(), bytes.as_slice());
        assert_eq!(admitted.computed_digest, digest);
        assert_eq!(admitted.max_bytes, 4096);
        // The retained cache is charged for exactly these bytes.
        assert_eq!(
            repository.cache_usage().expect("usage"),
            Usage {
                bytes: bytes.len(),
                entries: 1
            }
        );
    }

    #[test]
    fn a_digest_mismatch_quarantines_and_never_reports_available() {
        let bytes = png(2);
        let wrong = Sha256Digest([0x5a; 32]);
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [Ok(descriptor(Some(wrong), Availability::Available))],
            [Ok(bytes)],
        );
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let unavailable = outcome.unavailable().expect("mismatch is not available");
        assert_eq!(unavailable.availability, MediaAvailability::Failed);
        assert_eq!(
            unavailable.cause,
            MediaUnavailableCause::Admission(MediaError::DigestMismatch)
        );
        assert_eq!(repository.cache_usage().expect("usage"), Usage::default());
    }

    #[test]
    fn an_unverified_descriptor_is_never_reported_verified() {
        let bytes = png(3);
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [Ok(descriptor(None, Availability::Available))],
            [Ok(bytes.clone())],
        );
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let admitted = outcome.admitted().expect("unverified bytes still admitted");
        assert!(!admitted.identity.verified());
        assert_eq!(admitted.identity.expected_sha256, None);
        assert_eq!(admitted.computed_digest, digest_of(&bytes));
    }

    #[test]
    fn a_verified_descriptor_without_an_expected_digest_is_refused_by_the_client_shape() {
        let bytes = png(4);
        let mut wrong = descriptor(None, Availability::Available);
        wrong.verification = Some(VerificationSummary {
            state: VerificationState::Verified,
            algorithm: Some("sha256".to_owned()),
            expected_digest: None,
            observed_digest: None,
        });
        let mut repository = new_repository();
        let mut stub = Stub::with([Ok(wrong)], [Ok(bytes)]);
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let unavailable = outcome.unavailable().expect("no fabricated verification");
        assert_eq!(unavailable.availability, MediaAvailability::Failed);
        assert_eq!(
            unavailable.cause,
            MediaUnavailableCause::Fetch(MediaFetchError::MalformedDescriptor)
        );
        assert_eq!(stub.byte_calls, 0);
    }

    #[test]
    fn a_second_selection_reuses_the_admitted_bytes_without_a_second_byte_fetch() {
        let bytes = png(5);
        let digest = digest_of(&bytes);
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [
                Ok(descriptor(Some(digest), Availability::Available)),
                Ok(descriptor(Some(digest), Availability::Available)),
            ],
            [Ok(bytes.clone())],
        );
        let first = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        assert!(first.admitted().is_some());
        let second = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        assert_eq!(second, first);
        assert_eq!(stub.descriptor_calls, 2);
        assert_eq!(stub.byte_calls, 1, "cached bytes must not be fetched again");
    }

    #[test]
    fn another_principal_and_another_expected_digest_never_share_cached_bytes() {
        let bytes = png(6);
        let digest = digest_of(&bytes);
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [
                Ok(descriptor(Some(digest), Availability::Available)),
                Ok(descriptor(Some(digest), Availability::Available)),
                Ok(descriptor(
                    Some(Sha256Digest([9; 32])),
                    Availability::Available,
                )),
            ],
            [Ok(bytes.clone()), Ok(bytes.clone())],
        );
        assert!(
            load(
                &mut repository,
                &mut stub,
                &selection(AUDIENCE, Availability::Available)
            )
            .admitted()
            .is_some()
        );
        // A different principal has a different bounded cache key even for the same artifact.
        assert!(
            load(
                &mut repository,
                &mut stub,
                &selection(OTHER_AUDIENCE, Availability::Available)
            )
            .admitted()
            .is_some()
        );
        assert_eq!(stub.byte_calls, 2);
        // The expected digest is part of the key: a different digest is a different entry.
        let mut other_digest = Stub::with(
            [Ok(descriptor(
                Some(Sha256Digest([9; 32])),
                Availability::Available,
            ))],
            [Ok(bytes.clone())],
        );
        let outcome = load(
            &mut repository,
            &mut other_digest,
            &selection(AUDIENCE, Availability::Available),
        );
        assert_eq!(
            outcome
                .unavailable()
                .map(|unavailable| unavailable.availability),
            Some(MediaAvailability::Failed),
            "a different expected digest must miss the retained entry and quarantine the bytes"
        );
    }

    #[test]
    fn an_expired_descriptor_serves_a_retained_digest_matching_entry() {
        let bytes = png(7);
        let digest = digest_of(&bytes);
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [
                Ok(descriptor(Some(digest), Availability::Available)),
                Err(MediaFetchError::DescriptorExpired),
                Err(MediaFetchError::DescriptorExpired),
            ],
            [Ok(bytes.clone())],
        );
        let first = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let admitted = first.admitted().expect("first load admitted");
        // The descriptor now expires. The cache key does not contain descriptor
        // identity or expiry, so the digest-matching entry still resolves.
        let second = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let replay = second
            .admitted()
            .expect("an expired descriptor cannot invalidate a matching entry");
        assert_eq!(replay.identity, admitted.identity);
        assert_eq!(replay.computed_digest, digest);
        assert_eq!(stub.byte_calls, 1, "no byte refetch was needed");
    }

    #[test]
    fn an_expired_descriptor_without_a_matching_entry_is_expired_not_missing() {
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [
                Err(MediaFetchError::DescriptorExpired),
                Err(MediaFetchError::DescriptorExpired),
            ],
            [],
        );
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let unavailable = outcome.unavailable().expect("expiry explicit");
        assert_eq!(unavailable.availability, MediaAvailability::Expired);
        assert_ne!(unavailable.availability, MediaAvailability::Missing);
        assert_eq!(repository.cache_usage().expect("usage"), Usage::default());
    }

    #[test]
    fn a_handle_expiry_reauthorizes_once_and_then_admits_or_reports_expiry() {
        let bytes = png(8);
        let digest = digest_of(&bytes);
        let mut repository = new_repository();
        let mut stub = Stub::with(
            [
                Ok(descriptor(Some(digest), Availability::Available)),
                Ok(descriptor(Some(digest), Availability::Available)),
            ],
            [Err(MediaFetchError::DescriptorExpired), Ok(bytes.clone())],
        );
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let admitted = outcome.admitted().expect("reauthorized bytes admitted");
        assert_eq!(admitted.computed_digest, digest);
        assert_eq!(stub.descriptor_calls, 2);
        assert_eq!(stub.byte_calls, 2);
    }

    #[test]
    fn a_contradictory_descriptor_or_representation_is_never_presented() {
        let mut repository = new_repository();
        let mut other_artifact = descriptor(None, Availability::Available);
        other_artifact.artifact_ref = ArtifactId::new("artifact-b").expect("artifact id");
        let mut stub = Stub::with([Ok(other_artifact)], []);
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        assert_eq!(
            outcome
                .unavailable()
                .map(|unavailable| unavailable.availability),
            Some(MediaAvailability::Failed)
        );
        assert_eq!(stub.byte_calls, 0);

        let mut repository = new_repository();
        let mut other_representation = descriptor(None, Availability::Available);
        other_representation.representation = RepresentationKind::Preview;
        let mut stub = Stub::with([Ok(other_representation)], []);
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        assert_eq!(
            outcome
                .unavailable()
                .map(|unavailable| unavailable.availability),
            Some(MediaAvailability::Failed)
        );
        assert_eq!(stub.byte_calls, 0);
    }

    #[test]
    fn an_unknown_representation_is_explicitly_unsupported_before_any_fetch() {
        let mut repository = new_repository();
        let mut stub = Stub::default();
        let selection = MediaSelection::for_artifact(
            &snapshot(Availability::Available),
            session(AUDIENCE),
            RepresentationKind::Unknown("HOLOGRAM".to_owned()),
        );
        let outcome = load(&mut repository, &mut stub, &selection);
        let unavailable = outcome.unavailable().expect("unknown representation");
        assert_eq!(unavailable.availability, MediaAvailability::Unsupported);
        assert_eq!(unavailable.cause, MediaUnavailableCause::Identity);
        assert_eq!(stub.descriptor_calls, 0);
    }

    #[test]
    fn cancellation_returns_no_availability_state_and_fetches_nothing() {
        let mut repository = new_repository();
        let mut stub = Stub::default();
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let outcome = repository.load(
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
            &cancellation,
        );
        assert_eq!(outcome, MediaPresentation::Cancelled);
        assert_eq!(stub.descriptor_calls, 0);
        assert_eq!(stub.byte_calls, 0);
    }

    #[test]
    fn a_declared_length_above_the_admission_budget_is_refused_before_transfer() {
        let mut oversized = descriptor(None, Availability::Available);
        oversized.byte_length = Some(8192);
        let mut repository = new_repository();
        let mut stub = Stub::with([Ok(oversized)], []);
        let outcome = load(
            &mut repository,
            &mut stub,
            &selection(AUDIENCE, Availability::Available),
        );
        let unavailable = outcome.unavailable().expect("oversized refusal");
        assert_eq!(unavailable.availability, MediaAvailability::Failed);
        assert_eq!(
            unavailable.cause,
            MediaUnavailableCause::Fetch(MediaFetchError::TooLarge)
        );
        assert_eq!(stub.byte_calls, 0);
    }

    #[test]
    fn the_read_context_is_never_rendered() {
        let context = MediaSessionContext::new(
            ServerId::new(SERVER).expect("server"),
            AUDIENCE,
            EpochId::new("epoch-1").expect("epoch"),
            "super-secret-read-context",
        );
        let rendered = format!("{context:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("super-secret-read-context"));
        let selection = MediaSelection::for_artifact(
            &snapshot(Availability::Available),
            context,
            RepresentationKind::Original,
        );
        let rendered = format!("{selection:?}");
        assert!(!rendered.contains("super-secret-read-context"));
    }
}
