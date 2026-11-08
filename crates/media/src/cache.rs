use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use sha2::{Digest, Sha256};

use crate::{
    LocalProvenance, MediaError, RepresentationKey, RepresentationSource,
    remote::{AdmittedRemote, RemoteCacheKey},
};

/// SHA-256 content fingerprint, not canonical ArtifactDigest or authorization evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha256Digest(pub [u8; 32]);

impl std::fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sha256:")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Caller-supplied independent manifest expectation; never filled from computed bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedDigest {
    pub provenance: LocalProvenance,
    pub digest: Sha256Digest,
}

/// Integrity of this local input only; none of these states asserts Runtime authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    Unverified,
    GeneratedManifestVerified,
    FixtureManifestVerified,
}

#[derive(Clone, Debug)]
pub struct Acquisition {
    pub key: RepresentationKey,
    pub expected: Option<ExpectedDigest>,
}

/// Cooperative cancellation observed before allocation, between <=8KiB reads and before publication.
/// A blocking OS read itself is not preemptible; callers must use ordinary local regular files.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether cancellation has been requested. Exposed so callers outside this crate - the
    /// upload admission boundary, for one - can refuse work without attempting it.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub(crate) fn check(&self) -> Result<(), MediaError> {
        if self.0.load(Ordering::Acquire) {
            Err(MediaError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Encoded payload limits, separate from future decoded/scratch/GPU budgets.
/// Counts live retained payload capacity, not allocator bookkeeping, stack or key metadata.
/// `max_pixels` is the adversarial hard ceiling from an encoded header; per-codec decode
/// admission remains MP-IMG2.
#[derive(Clone, Copy, Debug)]
pub struct LocalLimits {
    pub max_input_bytes: usize,
    pub max_live_bytes: usize,
    pub max_live_entries: usize,
    pub max_pixels: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: usize,
    pub entries: usize,
}

struct Budget {
    limits: LocalLimits,
    usage: Mutex<Usage>,
}
struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        // No user code executes under the lock. Recovery still releases ownership if poisoned.
        let mut usage = self
            .budget
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        usage.bytes -= self.bytes;
        usage.entries -= 1;
    }
}

impl Budget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Reservation, MediaError> {
        let mut usage = self.usage.lock().map_err(|_| MediaError::Unavailable)?;
        let total = usage.bytes.checked_add(bytes).ok_or(MediaError::Capacity)?;
        if total > self.limits.max_live_bytes || usage.entries >= self.limits.max_live_entries {
            return Err(MediaError::Capacity);
        }
        usage.bytes = total;
        usage.entries += 1;
        Ok(Reservation {
            budget: Arc::clone(self),
            bytes,
        })
    }
}

/// PNG signature plus a complete IHDR chunk. Bounded and allocation-free.
const ENCODED_HEADER_BYTES: usize = 33;

/// Fill exactly or fail closed; cancellation is observed between reads.
fn read_bounded(
    reader: &mut impl Read,
    buffer: &mut [u8],
    cancel: &Cancellation,
) -> Result<(), MediaError> {
    let mut filled = 0;
    while filled < buffer.len() {
        cancel.check()?;
        match reader.read(&mut buffer[filled..]) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
            Ok(0) => return Err(MediaError::LengthMismatch),
            Ok(read) => filled += read,
        }
    }
    Ok(())
}

/// Minimal adversarial admission: signature and IHDR presence, first chunk layout and
/// positive dimensions. CRC, bit depth, colour, codec and decode budgets belong to
/// MP-IMG2; this neither decodes nor promises format support.
fn validate_encoded_header(header: &[u8; ENCODED_HEADER_BYTES]) -> Result<(u32, u32), MediaError> {
    if !header.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(MediaError::UnsupportedContainer);
    }
    if header[8..12] != [0, 0, 0, 13] || header[12..16] != *b"IHDR" {
        return Err(MediaError::UnsupportedContainer);
    }
    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    if width == 0 || height == 0 {
        return Err(MediaError::InvalidMetadata);
    }
    Ok((width, height))
}

/// Immutable encoded bytes. Eviction cannot invalidate a consumer or release its reservation early.
/// PNG signature recognition is NOT decode admission or proof of a valid PNG image.
pub struct EncodedMedia {
    key: Arc<RepresentationKey>,
    bytes: Vec<u8>,
    computed: Sha256Digest,
    expected: Option<ExpectedDigest>,
    verification: Verification,
    _reservation: Reservation,
}
impl EncodedMedia {
    pub fn key(&self) -> &RepresentationKey {
        &self.key
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn computed_digest(&self) -> Sha256Digest {
        self.computed
    }
    pub fn expected_digest(&self) -> Option<&ExpectedDigest> {
        self.expected.as_ref()
    }
    pub fn verification(&self) -> Verification {
        self.verification
    }
}

/// Explicitly bounded encoded cache. No global registry, automatic I/O, decode or GPU lifetime.
/// Remove/clear drops cache ownership; outstanding Arc handles remain charged until last drop.
pub struct MediaCache {
    entries: BTreeMap<Arc<RepresentationKey>, Arc<EncodedMedia>>,
    /// Admitted remote bytes keyed by their authorized per-principal cache identity.
    ///
    /// Nothing in this map is keyed by, or checked against, descriptor validity:
    /// an expired descriptor cannot remove or invalidate an entry whose key it
    /// no longer reproduces.
    remote: BTreeMap<RemoteCacheKey, Arc<CachedRemote>>,
    budget: Arc<Budget>,
}

/// Admitted remote bytes retained under their authorized cache identity.
pub struct CachedRemote {
    key: RemoteCacheKey,
    admitted: AdmittedRemote,
    _reservation: Reservation,
}

impl CachedRemote {
    /// The bounded authorized cache identity this entry was admitted under.
    pub fn key(&self) -> &RemoteCacheKey {
        &self.key
    }

    /// The admitted bytes and their computed digest.
    pub fn admitted(&self) -> &AdmittedRemote {
        &self.admitted
    }

    /// Encoded representation bytes.
    pub fn bytes(&self) -> &[u8] {
        self.admitted.bytes()
    }
}

impl MediaCache {
    pub fn new(limits: LocalLimits) -> Result<Self, MediaError> {
        if limits.max_input_bytes < 8
            || limits.max_input_bytes > isize::MAX as usize
            || limits.max_live_bytes < limits.max_input_bytes
            || limits.max_live_entries == 0
            || limits.max_pixels == 0
        {
            return Err(MediaError::InvalidLimits);
        }
        Ok(Self {
            entries: BTreeMap::new(),
            remote: BTreeMap::new(),
            budget: Arc::new(Budget {
                limits,
                usage: Mutex::new(Usage::default()),
            }),
        })
    }

    pub fn usage(&self) -> Result<Usage, MediaError> {
        self.budget
            .usage
            .lock()
            .map(|u| *u)
            .map_err(|_| MediaError::Unavailable)
    }
    pub fn get(&self, key: &RepresentationKey) -> Option<Arc<EncodedMedia>> {
        self.entries.get(key).map(Arc::clone)
    }
    pub fn remove(&mut self, key: &RepresentationKey) {
        self.entries.remove(key);
    }

    /// Remove one retained remote entry. Eviction never releases a reservation early: an
    /// outstanding [`CachedRemote`] handle keeps its bytes charged until the last drop.
    pub fn remove_remote(&mut self, key: &RemoteCacheKey) {
        self.remote.remove(key);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.remote.clear();
    }

    /// Look up already admitted remote bytes by their authorized cache identity.
    ///
    /// This takes no descriptor, expiry or clock argument on purpose: an expired
    /// descriptor cannot invalidate an entry that still matches the authorized
    /// principal, canonical identity, representation and expected digest.
    pub fn get_remote(&self, key: &RemoteCacheKey) -> Option<Arc<CachedRemote>> {
        self.remote.get(key).map(Arc::clone)
    }

    /// Retain admitted remote bytes under their bounded authorized cache identity.
    ///
    /// A repeated key returns the retained entry when the computed digest agrees and
    /// refuses with [`MediaError::KeyAlreadyCached`] when the same identity would
    /// carry different bytes, so an unverified conflict never overwrites admitted
    /// content.
    pub fn insert_remote(
        &mut self,
        key: RemoteCacheKey,
        admitted: AdmittedRemote,
    ) -> Result<Arc<CachedRemote>, MediaError> {
        if let Some(existing) = self.remote.get(&key) {
            if existing.admitted.digest() == admitted.digest() {
                return Ok(Arc::clone(existing));
            }
            return Err(MediaError::KeyAlreadyCached);
        }
        let bytes = admitted.byte_len();
        if bytes > self.budget.limits.max_input_bytes {
            return Err(MediaError::TooLarge);
        }
        let reservation = self.budget.reserve(bytes)?;
        let entry = Arc::new(CachedRemote {
            key,
            admitted,
            _reservation: reservation,
        });
        self.remote.insert(entry.key.clone(), Arc::clone(&entry));
        Ok(entry)
    }

    /// The caller owns the explicit local file capability. No path is read from media metadata.
    pub fn acquire_file(
        &mut self,
        request: Acquisition,
        file: File,
        cancel: &Cancellation,
    ) -> Result<Arc<EncodedMedia>, MediaError> {
        self.validate(&request, cancel)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(MediaError::NotRegularFile);
        }
        self.acquire_reader(request, file, metadata.len(), cancel)
    }

    fn validate(&self, request: &Acquisition, cancel: &Cancellation) -> Result<(), MediaError> {
        cancel.check()?;
        let RepresentationSource::Local { provenance, .. } = &request.key.source else {
            return Err(MediaError::ExternalAcquisitionUnavailable);
        };
        if request
            .expected
            .as_ref()
            .is_some_and(|e| &e.provenance != provenance)
        {
            return Err(MediaError::ExpectedProvenanceMismatch);
        }
        if self.entries.contains_key(&request.key) {
            return Err(MediaError::KeyAlreadyCached);
        }
        Ok(())
    }

    /// Explicit reader capability with a declared encoded length. Oversize lengths and
    /// hostile headers are rejected before payload reservation; a lying/growing/shrinking
    /// source is rejected; no partial value is published.
    pub fn acquire_reader(
        &mut self,
        request: Acquisition,
        mut reader: impl Read,
        declared_bytes: u64,
        cancel: &Cancellation,
    ) -> Result<Arc<EncodedMedia>, MediaError> {
        self.validate(&request, cancel)?;
        let length = usize::try_from(declared_bytes).map_err(|_| MediaError::TooLarge)?;
        if length > self.budget.limits.max_input_bytes {
            return Err(MediaError::TooLarge);
        }
        if length < ENCODED_HEADER_BYTES {
            return Err(MediaError::UnsupportedContainer);
        }
        // Header first: adversarial dimensions never reserve payload capacity.
        let mut header = [0u8; ENCODED_HEADER_BYTES];
        read_bounded(&mut reader, &mut header, cancel)?;
        let (width, height) = validate_encoded_header(&header)?;
        let area = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or(MediaError::TooLarge)?;
        if area > self.budget.limits.max_pixels {
            return Err(MediaError::TooLarge);
        }
        let mut reservation = self.budget.reserve(length)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| MediaError::Allocation)?;
        // Vec may receive more capacity than requested; account for the actual retained capacity.
        if bytes.capacity() > length {
            let extra = bytes.capacity() - length;
            let mut usage = self
                .budget
                .usage
                .lock()
                .map_err(|_| MediaError::Unavailable)?;
            let total = usage.bytes.checked_add(extra).ok_or(MediaError::Capacity)?;
            if bytes.capacity() > self.budget.limits.max_input_bytes
                || total > self.budget.limits.max_live_bytes
            {
                return Err(MediaError::Capacity);
            }
            usage.bytes = total;
            reservation.bytes = bytes.capacity();
        }
        bytes.resize(length, 0);
        bytes[..ENCODED_HEADER_BYTES].copy_from_slice(&header);
        let mut hasher = Sha256::new();
        hasher.update(header);
        let mut position = ENCODED_HEADER_BYTES;
        while position < length {
            cancel.check()?;
            let end = position + (length - position).min(8192);
            let read = match reader.read(&mut bytes[position..end]) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if read == 0 {
                return Err(MediaError::LengthMismatch);
            }
            hasher.update(&bytes[position..position + read]);
            position += read;
        }
        cancel.check()?;
        let mut excess = [0u8; 1];
        loop {
            cancel.check()?;
            match reader.read(&mut excess) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
                Ok(0) => break,
                Ok(_) => return Err(MediaError::LengthMismatch),
            }
        }
        cancel.check()?;
        let computed = Sha256Digest(hasher.finalize().into());
        let verification = match &request.expected {
            None => Verification::Unverified,
            Some(expected) => {
                if expected.digest != computed {
                    return Err(MediaError::DigestMismatch);
                }
                match expected.provenance {
                    LocalProvenance::Generated { .. } => Verification::GeneratedManifestVerified,
                    LocalProvenance::Fixture { .. } => Verification::FixtureManifestVerified,
                }
            }
        };
        cancel.check()?;
        let key = Arc::new(request.key);
        let media = Arc::new(EncodedMedia {
            key: Arc::clone(&key),
            bytes,
            computed,
            expected: request.expected,
            verification,
            _reservation: reservation,
        });
        self.entries.insert(key, Arc::clone(&media));
        Ok(media)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CacheScope, Cancellation, RemoteCacheKey, RemoteExpectation, admit_remote};
    use desktop_protocol::RepresentationKind;
    use sha2::{Digest, Sha256};

    /// The bounded container check reads only this signature header.
    fn png() -> Vec<u8> {
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        ];
        bytes.resize(64, 0);
        bytes
    }

    fn limits(max_live_entries: usize) -> LocalLimits {
        LocalLimits {
            max_input_bytes: 128,
            max_live_bytes: 512,
            max_live_entries,
            max_pixels: 16,
        }
    }

    fn admitted(bytes: &[u8]) -> AdmittedRemote {
        admit_remote(
            bytes,
            &RemoteExpectation {
                byte_length: Some(bytes.len() as u64),
                media_type: Some("image/png".to_owned()),
                sha256: None,
                verified: false,
            },
            1024,
            &Cancellation::default(),
        )
        .expect("unverified bytes admitted")
    }

    fn digest_of(bytes: &[u8]) -> Sha256Digest {
        Sha256Digest(Sha256::digest(bytes).into())
    }

    fn key(audience: &str, expected: Option<Sha256Digest>) -> RemoteCacheKey {
        let scope = CacheScope {
            server_identity: "server-1",
            audience,
            artifact_digest: "sha256:artifact",
            representation: RepresentationKind::Original,
            representation_version: 1,
            expected_digest: expected,
        };
        RemoteCacheKey::for_scope(&scope).expect("bounded key")
    }

    #[test]
    fn remote_entries_are_isolated_by_principal_and_expected_digest() {
        let mut cache = MediaCache::new(limits(8)).expect("cache");
        let bytes = png();
        let digest = digest_of(&bytes);
        let retained = key("desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Some(digest));
        let other_principal = key("desktop:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", Some(digest));
        let other_digest = key(
            "desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some(Sha256Digest([7; 32])),
        );
        let unverified = key("desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", None);
        cache
            .insert_remote(retained.clone(), admitted(&bytes))
            .expect("insert");
        assert!(cache.get_remote(&retained).is_some());
        // One session's admitted bytes are never served to another principal, to a
        // different expected digest, or to the explicit unverified marker.
        assert!(cache.get_remote(&other_principal).is_none());
        assert!(cache.get_remote(&other_digest).is_none());
        assert!(cache.get_remote(&unverified).is_none());
    }

    #[test]
    fn an_expired_descriptor_cannot_invalidate_a_digest_matching_entry() {
        let mut cache = MediaCache::new(limits(8)).expect("cache");
        let bytes = png();
        let digest = digest_of(&bytes);
        let retained = cache
            .insert_remote(key("desktop:principal", Some(digest)), admitted(&bytes))
            .expect("insert");
        // Expiry is not an input to the cache identity: the same authorized scope and
        // expected digest rebuild the same key after the descriptor expired, and the
        // retained entry is still returned. The cache never consults a clock.
        let after_expiry = key("desktop:principal", Some(digest));
        let hit = cache
            .get_remote(&after_expiry)
            .expect("a digest-matching entry survives descriptor expiry");
        assert!(Arc::ptr_eq(&retained, &hit));
        assert_eq!(hit.admitted().digest(), digest);
        assert_eq!(hit.bytes(), bytes.as_slice());
    }

    #[test]
    fn the_same_key_never_overwrites_differing_admitted_bytes() {
        let mut cache = MediaCache::new(limits(8)).expect("cache");
        let first = png();
        let mut second = first.clone();
        second[40] = 0x5a;
        let shared = key("desktop:principal", None);
        let retained = cache
            .insert_remote(shared.clone(), admitted(&first))
            .expect("first insert");
        // A repeated identical payload resolves to the retained entry.
        let repeated = cache
            .insert_remote(shared.clone(), admitted(&first))
            .expect("repeat insert");
        assert!(Arc::ptr_eq(&retained, &repeated));
        // The same unverified identity carrying different bytes is refused, never merged.
        assert_eq!(
            cache.insert_remote(shared, admitted(&second)).err(),
            Some(MediaError::KeyAlreadyCached)
        );
    }

    #[test]
    fn remote_entries_are_charged_against_the_live_entry_and_byte_budget() {
        let mut cache = MediaCache::new(limits(1)).expect("cache");
        let bytes = png();
        let first = key("desktop:one", None);
        let second = key("desktop:two", None);
        cache
            .insert_remote(first.clone(), admitted(&bytes))
            .expect("first");
        assert_eq!(
            cache.usage().expect("usage"),
            Usage {
                bytes: bytes.len(),
                entries: 1
            }
        );
        assert_eq!(
            cache.insert_remote(second.clone(), admitted(&bytes)).err(),
            Some(MediaError::Capacity)
        );
        // Oversize admitted payloads are refused before any reservation.
        let mut oversized = MediaCache::new(limits(8)).expect("cache");
        let mut long = png();
        long.resize(129, 0);
        assert_eq!(
            oversized
                .insert_remote(key("desktop:long", None), admitted(&long))
                .err(),
            Some(MediaError::TooLarge)
        );
        assert_eq!(oversized.usage().expect("usage"), Usage::default());
        cache.remove_remote(&first);
        assert_eq!(cache.usage().expect("usage"), Usage::default());
    }
}
