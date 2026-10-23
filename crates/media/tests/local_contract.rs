//! Independent consumer contracts M01–M07. M08 is the repository dependency gate;
//! M09 requires actual Windows Native evidence, not a mocked picker.
use desktop_protocol::{
    ArtifactDigest, ArtifactId, Counter, EpochId, RepresentationKind, ServerId,
};
use media::*;
use std::io::{self, Read};

// Signature + IHDR (32x32) + trailing bytes. Still NOT a decodable PNG body; CRC is
// present on IHDR but no IDAT is claimed. Independently precomputed with Node crypto.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x20, 0x08, 0x06, 0x00, 0x00, 0x00, 0x73, 0x7a, 0x7a,
    0xf4, 0x61, 0x62, 0x63,
];
// Independently precomputed with Node crypto SHA256, not production digest code.
const EXPECTED: Sha256Digest = Sha256Digest([
    188, 67, 117, 23, 213, 9, 123, 212, 128, 242, 226, 113, 5, 219, 207, 117, 188, 174, 108, 179,
    183, 158, 198, 233, 80, 248, 96, 126, 96, 138, 107, 206,
]);
fn id(s: &str) -> LocalId {
    LocalId::new(s).unwrap()
}
fn provenance(fixture: bool) -> LocalProvenance {
    if fixture {
        LocalProvenance::Fixture {
            fixture: id("case"),
            manifest: id("manifest-v1"),
            source: id("image"),
        }
    } else {
        LocalProvenance::Generated {
            generator: id("probe-v1"),
            manifest: id("manifest-v1"),
            source: id("image"),
        }
    }
}
fn key(fixture: bool) -> RepresentationKey {
    RepresentationKey {
        source: RepresentationSource::Local {
            provenance: provenance(fixture),
            revision: 1,
        },
        kind: RepresentationKind::Preview,
        size: PixelSize::new(32, 32).unwrap(),
        orientation: Orientation::Normal,
        policy: DecodePolicy {
            decoder: id("not-decoded"),
            colour: id("not-transformed"),
            generation: 1,
        },
    }
}
fn cache(bytes: usize, entries: usize) -> MediaCache {
    MediaCache::new(LocalLimits {
        max_input_bytes: bytes,
        max_live_bytes: bytes,
        max_live_entries: entries,
        max_pixels: 1_000_000,
    })
    .unwrap()
}
fn request(key: RepresentationKey) -> Acquisition {
    Acquisition {
        key,
        expected: None,
    }
}
fn load(
    c: &mut MediaCache,
    request: Acquisition,
) -> Result<std::sync::Arc<EncodedMedia>, MediaError> {
    c.acquire_reader(request, PNG, PNG.len() as u64, &Cancellation::default())
}

#[test]
fn generated_and_fixture_do_not_alias_or_gain_runtime_verification() {
    let mut c = cache(128, 4);
    let generated = load(&mut c, request(key(false))).unwrap();
    let fixture = load(&mut c, request(key(true))).unwrap();
    assert_ne!(generated.key(), fixture.key());
    assert_eq!(generated.verification(), Verification::Unverified);
    assert_eq!(fixture.verification(), Verification::Unverified);
    assert_eq!(generated.computed_digest(), EXPECTED);
    assert_eq!(generated.expected_digest(), None);
    assert_eq!(fixture.bytes(), PNG);
}

#[test]
fn only_matching_independent_manifest_expectations_verify_local_integrity() {
    for fixture in [false, true] {
        let mut c = cache(128, 2);
        let mut r = request(key(fixture));
        r.expected = Some(ExpectedDigest {
            provenance: provenance(fixture),
            digest: EXPECTED,
        });
        let media = load(&mut c, r).unwrap();
        assert_eq!(
            media.verification(),
            if fixture {
                Verification::FixtureManifestVerified
            } else {
                Verification::GeneratedManifestVerified
            }
        );
        assert_eq!(media.expected_digest().unwrap().digest, EXPECTED);
        c.clear();
        drop(media);
        let mut r = request(key(fixture));
        r.expected = Some(ExpectedDigest {
            provenance: provenance(fixture),
            digest: Sha256Digest([0; 32]),
        });
        assert_eq!(load(&mut c, r).err(), Some(MediaError::DigestMismatch));
        assert_eq!(c.usage().unwrap(), Usage::default());
        let mut r = request(key(fixture));
        r.expected = Some(ExpectedDigest {
            provenance: provenance(!fixture),
            digest: EXPECTED,
        });
        assert_eq!(
            load(&mut c, r).err(),
            Some(MediaError::ExpectedProvenanceMismatch)
        );
        assert!(c.get(&key(fixture)).is_none());
    }
}

#[test]
fn representation_transforms_and_revisions_never_reuse_another_cached_value() {
    let original = key(false);
    let mut variants = Vec::new();
    let mut k = original.clone();
    k.size = PixelSize::new(64, 32).unwrap();
    variants.push(k);
    let mut k = original.clone();
    k.size = PixelSize::new(32, 64).unwrap();
    variants.push(k);
    for orientation in [
        Orientation::MirrorHorizontal,
        Orientation::Rotate180,
        Orientation::MirrorVertical,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ] {
        let mut k = original.clone();
        k.orientation = orientation;
        variants.push(k);
    }
    let mut k = original.clone();
    k.kind = RepresentationKind::Thumbnail;
    variants.push(k);
    let mut k = original.clone();
    k.policy.decoder = id("other-decoder");
    variants.push(k);
    let mut k = original.clone();
    k.policy.colour = id("other-colour");
    variants.push(k);
    let mut k = original.clone();
    k.policy.generation = 2;
    variants.push(k);
    let mut k = original.clone();
    if let RepresentationSource::Local { revision, .. } = &mut k.source {
        *revision = 2;
    }
    variants.push(k);
    let mut c = cache(1024, 32);
    let source = load(&mut c, request(original.clone())).unwrap();
    for variant in variants {
        assert!(c.get(&variant).is_none());
        let other = load(&mut c, request(variant)).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&source, &other));
        assert_ne!(source.key(), other.key());
    }
    assert!(std::sync::Arc::ptr_eq(&source, &c.get(&original).unwrap()));
}

#[test]
fn canonical_key_remains_metadata_only_and_keeps_server_epoch_revision_scope() {
    let mut k = key(false);
    k.source = RepresentationSource::Artifact {
        server: ServerId::new("server").unwrap(),
        epoch: EpochId::new("epoch").unwrap(),
        artifact: ArtifactId::new("artifact").unwrap(),
        digest: ArtifactDigest::new("canonical-digest-not-content-hash").unwrap(),
        revision: Counter::new("7").unwrap(),
    };
    let mut newer = k.clone();
    if let RepresentationSource::Artifact { revision, .. } = &mut newer.source {
        *revision = Counter::new("8").unwrap();
    }
    let mut epoch = k.clone();
    if let RepresentationSource::Artifact { epoch, .. } = &mut epoch.source {
        *epoch = EpochId::new("other").unwrap();
    }
    let mut server = k.clone();
    if let RepresentationSource::Artifact { server, .. } = &mut server.source {
        *server = ServerId::new("other").unwrap();
    }
    assert_ne!(k, newer);
    assert_ne!(k, epoch);
    assert_ne!(k, server);
    let mut c = cache(128, 2);
    assert_eq!(
        c.acquire_reader(request(k), NeverRead, 11, &Cancellation::default())
            .err(),
        Some(MediaError::ExternalAcquisitionUnavailable)
    );
}

struct NeverRead;
impl Read for NeverRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("input was read before admission")
    }
}
#[test]
fn size_and_metadata_rejection_precede_any_read_and_inaccurate_lengths_fail_closed() {
    let mut c = cache(PNG.len(), 4);
    for size in [PNG.len() + 1, usize::MAX] {
        assert_eq!(
            c.acquire_reader(
                request(key(false)),
                NeverRead,
                size as u64,
                &Cancellation::default()
            )
            .err(),
            Some(MediaError::TooLarge)
        );
    }
    // A declared length below the complete header is refused without reading the payload.
    assert_eq!(
        c.acquire_reader(request(key(false)), NeverRead, 32, &Cancellation::default())
            .err(),
        Some(MediaError::UnsupportedContainer)
    );
    // A source shorter than its declared length fails closed after the bounded header read.
    assert_eq!(
        c.acquire_reader(
            request(key(false)),
            &PNG[..20],
            PNG.len() as u64,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::LengthMismatch)
    );
    assert_eq!(
        c.acquire_reader(
            request(key(false)),
            &[0u8; 11][..],
            11,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::UnsupportedContainer)
    );
    assert_eq!(c.usage().unwrap(), Usage::default());
    assert_eq!(PixelSize::new(0, 1), Err(MediaError::InvalidMetadata));
    assert_eq!(LocalId::new("../../data"), Err(MediaError::InvalidMetadata));
    assert_eq!(
        LocalId::new("x".repeat(129)),
        Err(MediaError::InvalidMetadata)
    );
    assert!(matches!(
        MediaCache::new(LocalLimits {
            max_input_bytes: usize::MAX,
            max_live_bytes: usize::MAX,
            max_live_entries: 1,
            max_pixels: 1_000_000
        }),
        Err(MediaError::InvalidLimits)
    ));
}

struct CancelAfterRead {
    cancel: Cancellation,
    bytes: &'static [u8],
}
impl Read for CancelAfterRead {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let n = self.bytes.read(buffer)?;
        self.cancel.cancel();
        Ok(n)
    }
}
struct Broken;
impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}
#[test]
fn cancellation_and_io_failure_never_publish_partial_bytes_or_retain_quota() {
    let mut c = cache(64, 1);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert_eq!(
        c.acquire_reader(request(key(false)), NeverRead, 11, &cancel)
            .err(),
        Some(MediaError::Cancelled)
    );
    let cancel = Cancellation::default();
    assert_eq!(
        c.acquire_reader(
            request(key(false)),
            CancelAfterRead {
                cancel: cancel.clone(),
                bytes: PNG
            },
            PNG.len() as u64,
            &cancel
        )
        .err(),
        Some(MediaError::Cancelled)
    );
    assert_eq!(
        c.acquire_reader(
            request(key(false)),
            Broken,
            PNG.len() as u64,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::Io(io::ErrorKind::PermissionDenied))
    );
    assert!(c.get(&key(false)).is_none());
    assert_eq!(c.usage().unwrap(), Usage::default());
    assert_eq!(load(&mut c, request(key(false))).unwrap().bytes(), PNG);
}

#[test]
fn cache_capacity_tracks_live_consumers_after_eviction_and_releases_on_last_drop() {
    let mut c = cache(PNG.len() * 2, 2);
    for _ in 0..20 {
        let a = load(&mut c, request(key(false))).unwrap();
        let b = load(&mut c, request(key(true))).unwrap();
        assert_eq!(
            c.usage().unwrap(),
            Usage {
                bytes: PNG.len() * 2,
                entries: 2
            }
        );
        assert_eq!(
            load(&mut c, request(key(false))).err(),
            Some(MediaError::KeyAlreadyCached)
        );
        c.clear();
        assert_eq!(a.bytes(), PNG);
        assert_eq!(b.bytes(), PNG);
        assert_eq!(
            load(&mut c, request(key(false))).err(),
            Some(MediaError::Capacity)
        );
        drop(a);
        assert_eq!(
            c.usage().unwrap(),
            Usage {
                bytes: PNG.len(),
                entries: 1
            }
        );
        let replacement = load(&mut c, request(key(false))).unwrap();
        c.remove(&key(false));
        drop(replacement);
        drop(b);
        assert_eq!(c.usage().unwrap(), Usage::default());
    }
    let handle = load(&mut c, request(key(false))).unwrap();
    let weak = std::sync::Arc::downgrade(&handle);
    drop(c);
    assert_eq!(handle.bytes(), PNG);
    drop(handle);
    assert!(weak.upgrade().is_none());
}

#[test]
fn entry_limit_is_independent_of_payload_byte_limit() {
    let mut c = cache(1024, 1);
    let a = load(&mut c, request(key(false))).unwrap();
    assert_eq!(
        load(&mut c, request(key(true))).err(),
        Some(MediaError::Capacity)
    );
    c.clear();
    drop(a);
    assert_eq!(
        load(&mut c, request(key(true))).unwrap().verification(),
        Verification::Unverified
    );
}
