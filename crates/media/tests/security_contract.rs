//! MP-SEC3 adversarial-input admission (S01–S09) at the encoded media boundary.
//! These assert observable admission outcomes; decoder/network/authorization owners
//! are not exercised or simulated.
use desktop_protocol::RepresentationKind;
use media::{
    Acquisition, Cancellation, DecodePolicy, LocalId, LocalLimits, LocalProvenance, MediaCache,
    MediaError, Orientation, PixelSize, RepresentationKey, RepresentationSource, Sha256Digest,
    Usage, Verification,
};
use std::io::{self, Read};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// Build a signature + IHDR payload with the declared dimensions, plus an optional tail.
fn encoded(width: u32, height: u32, tail: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(41 + tail.len());
    bytes.extend_from_slice(&SIGNATURE);
    bytes.extend_from_slice(&[0, 0, 0, 13]);
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes.extend_from_slice(&[0, 0, 0, 0]); // CRC is not admission input here
    bytes.extend_from_slice(tail);
    bytes
}

fn id(value: &str) -> LocalId {
    LocalId::new(value).unwrap()
}

fn key(decoder: &str) -> RepresentationKey {
    RepresentationKey {
        source: RepresentationSource::Local {
            provenance: LocalProvenance::Generated {
                generator: id("probe-v1"),
                manifest: id("manifest-v1"),
                source: id("image"),
            },
            revision: 1,
        },
        kind: RepresentationKind::Preview,
        size: PixelSize::new(32, 32).unwrap(),
        orientation: Orientation::Normal,
        policy: DecodePolicy {
            decoder: id(decoder),
            colour: id("not-transformed"),
            generation: 1,
        },
    }
}

fn cache(max_input: usize, max_pixels: u64) -> MediaCache {
    MediaCache::new(LocalLimits {
        max_input_bytes: max_input,
        max_live_bytes: max_input,
        max_live_entries: 2,
        max_pixels,
    })
    .unwrap()
}

fn request(decoder: &str) -> Acquisition {
    Acquisition {
        key: key(decoder),
        expected: None,
    }
}

/// Records how many bytes were requested from the source so rejection cost is observable.
struct Counting {
    bytes: Vec<u8>,
    position: usize,
    reads: Arc<AtomicUsize>,
    cancel_after_first: Option<Cancellation>,
}
impl Read for Counting {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = Read::read(&mut &self.bytes[self.position..], buffer)?;
        self.position += read;
        self.reads.fetch_add(read, Ordering::AcqRel);
        if read > 0
            && let Some(cancel) = &self.cancel_after_first
        {
            cancel.cancel();
        }
        Ok(read)
    }
}

fn counting(bytes: Vec<u8>) -> (Counting, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    (
        Counting {
            bytes,
            position: 0,
            reads: Arc::clone(&reads),
            cancel_after_first: None,
        },
        reads,
    )
}

#[test]
fn hostile_dimensions_are_rejected_before_payload_admission() {
    let mut c = cache(4096, 1_000_000);
    for (width, height, expected) in [
        (0, 32, MediaError::InvalidMetadata),
        (32, 0, MediaError::InvalidMetadata),
        (u32::MAX, u32::MAX, MediaError::TooLarge),
        (5000, 5000, MediaError::TooLarge),
    ] {
        let (reader, reads) = counting(encoded(width, height, b"payload"));
        let length = reader.bytes.len() as u64;
        assert_eq!(
            c.acquire_reader(
                request("image-png"),
                reader,
                length,
                &Cancellation::default()
            )
            .err(),
            Some(expected),
            "{width}x{height}"
        );
        assert_eq!(
            reads.load(Ordering::Acquire),
            33,
            "hostile admission reads only the bounded header"
        );
        assert_eq!(c.usage().unwrap(), Usage::default());
        assert!(c.get(&key("image-png")).is_none());
    }
    // The ceiling itself is admitted; one pixel above it is not.
    let ceiling = encoded(1000, 1000, b"payload");
    assert!(
        c.acquire_reader(
            request("image-png"),
            &ceiling[..],
            ceiling.len() as u64,
            &Cancellation::default()
        )
        .is_ok()
    );
    c.clear();
    let above = encoded(1001, 1000, b"payload");
    assert_eq!(
        c.acquire_reader(
            request("image-png"),
            &above[..],
            above.len() as u64,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::TooLarge)
    );
    assert_eq!(c.usage().unwrap(), Usage::default());
}

#[test]
fn pixel_arithmetic_is_checked_and_bounded() {
    assert_eq!(PixelSize::new(32, 32).unwrap().checked_area(), Ok(1024));
    assert_eq!(
        PixelSize::new(32, 32).unwrap().checked_rgba_bytes(),
        Ok(4096)
    );
    let maximal = PixelSize::new(u32::MAX, u32::MAX).unwrap();
    assert_eq!(
        maximal.checked_area(),
        Ok(u64::from(u32::MAX) * u64::from(u32::MAX))
    );
    assert_eq!(maximal.checked_rgba_bytes(), Err(MediaError::TooLarge));
    assert_eq!(PixelSize::new(0, 1), Err(MediaError::InvalidMetadata));
    assert!(matches!(
        MediaCache::new(LocalLimits {
            max_input_bytes: 64,
            max_live_bytes: 64,
            max_live_entries: 1,
            max_pixels: 0,
        }),
        Err(MediaError::InvalidLimits)
    ));
}

#[test]
fn declared_type_identity_cannot_bypass_header_admission() {
    let mut c = cache(4096, 1_000_000);
    for decoder in ["png", "image-png", "verified", "trusted"] {
        let hostile = encoded(0, 64, b"payload");
        assert_eq!(
            c.acquire_reader(
                request(decoder),
                &hostile[..],
                hostile.len() as u64,
                &Cancellation::default()
            )
            .err(),
            Some(MediaError::InvalidMetadata)
        );
        assert_eq!(c.usage().unwrap(), Usage::default());
    }
    let admitted = encoded(32, 32, b"payload");
    let media = c
        .acquire_reader(
            request("verified"),
            &admitted[..],
            admitted.len() as u64,
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(
        media.verification(),
        Verification::Unverified,
        "policy text never creates verification"
    );
}

#[test]
fn metadata_text_is_opaque_and_never_executed_or_resolved() {
    let mut c = cache(4096, 1_000_000);
    let hostile_text = b"cmd.exe /c calc\0C:\\secrets\\key.pem\0https://example.invalid/fetch";
    let bytes = encoded(32, 32, hostile_text);
    let media = c
        .acquire_reader(
            request("image-png"),
            &bytes[..],
            bytes.len() as u64,
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(media.bytes(), bytes.as_slice());
    assert_eq!(media.verification(), Verification::Unverified);
    assert_eq!(media.expected_digest(), None);
    assert!(
        c.get(&key("image-png")).is_some(),
        "opaque bytes are cached by identity, not by metadata text"
    );
}

#[test]
fn identifiers_reject_traversal_drive_and_url_shapes() {
    for value in [
        "../image",
        "..\\image",
        "C:\\image.png",
        "C:image.png",
        "https://example.invalid/image.png",
        "/absolute",
        "a/b",
        "a\\b",
    ] {
        assert_eq!(
            LocalId::new(value),
            Err(MediaError::InvalidMetadata),
            "{value}"
        );
    }
    assert!(LocalId::new("manifest-v1").is_ok());
}

#[test]
fn oversize_and_malformed_lengths_never_reserve_or_publish() {
    let mut c = cache(4096, 1_000_000);
    let valid = encoded(32, 32, b"payload");
    let mut oversized_panic = PanicRead;
    assert_eq!(
        c.acquire_reader(
            request("image-png"),
            &mut oversized_panic,
            4097,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::TooLarge)
    );
    let mut short = &valid[..20];
    assert_eq!(
        c.acquire_reader(
            request("image-png"),
            &mut short,
            valid.len() as u64,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::LengthMismatch)
    );
    let mut wrong_chunk = valid.clone();
    wrong_chunk[12..16].copy_from_slice(b"tEXt");
    assert_eq!(
        c.acquire_reader(
            request("image-png"),
            &wrong_chunk[..],
            wrong_chunk.len() as u64,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::UnsupportedContainer)
    );
    assert_eq!(c.usage().unwrap(), Usage::default());
}

struct PanicRead;
impl Read for PanicRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("payload was read before length admission")
    }
}

#[test]
fn cancellation_during_header_read_publishes_nothing() {
    let mut c = cache(4096, 1_000_000);
    let cancel = Cancellation::default();
    let reads = Arc::new(AtomicUsize::new(0));
    let reader = Counting {
        bytes: encoded(32, 32, b"payload"),
        position: 0,
        reads: Arc::clone(&reads),
        cancel_after_first: Some(cancel.clone()),
    };
    let length = 41 + b"payload".len();
    assert_eq!(
        c.acquire_reader(request("image-png"), reader, length as u64, &cancel)
            .err(),
        Some(MediaError::Cancelled)
    );
    assert!(reads.load(Ordering::Acquire) <= 41);
    assert_eq!(c.usage().unwrap(), Usage::default());
    assert!(c.get(&key("image-png")).is_none());
}

#[test]
fn absent_expected_digest_is_unverified_and_mismatch_is_bounded() {
    let mut c = cache(4096, 1_000_000);
    let bytes = encoded(32, 32, b"payload");
    let absent = c
        .acquire_reader(
            request("image-png"),
            &bytes[..],
            bytes.len() as u64,
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(absent.verification(), Verification::Unverified);
    c.clear();
    drop(absent);
    let mut mismatched = request("image-png");
    mismatched.expected = Some(media::ExpectedDigest {
        provenance: match &mismatched.key.source {
            RepresentationSource::Local { provenance, .. } => provenance.clone(),
            RepresentationSource::Artifact { .. } => unreachable!("local key"),
        },
        digest: Sha256Digest([0; 32]),
    });
    assert_eq!(
        c.acquire_reader(
            mismatched,
            &bytes[..],
            bytes.len() as u64,
            &Cancellation::default()
        )
        .err(),
        Some(MediaError::DigestMismatch)
    );
    assert_eq!(c.usage().unwrap(), Usage::default());
}

#[test]
fn failures_expose_only_bounded_categories() {
    let messages = [
        MediaError::InvalidMetadata,
        MediaError::TooLarge,
        MediaError::UnsupportedContainer,
        MediaError::Io(io::ErrorKind::PermissionDenied),
    ]
    .map(|error| error.to_string());
    for message in messages {
        assert!(!message.contains('\\'), "{message}");
        assert!(!message.contains('/'), "{message}");
        assert!(!message.contains("http"), "{message}");
    }
    assert_eq!(
        MediaError::Io(io::ErrorKind::PermissionDenied).to_string(),
        "Io(PermissionDenied)"
    );
}
