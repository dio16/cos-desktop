// MP-IMG3-S1: the upload admission boundary, second version.
//
// The first version was refused by review on three counts, and these tests pin the repairs:
// the class proof cannot be fabricated, admission is atomic under one lock, and lifetime is a
// state machine where a plain drop does not pretend the GPU freed anything.
//
// Nothing here uploads. `complete_upload` is the caller asserting a device copy exists; the
// tests use it to move through the states and never claim a GPU was involved.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{
    Cancellation, DecodeBudget, DecodeLimits, DecodePolicy, LocalId, LocalProvenance, MediaError,
    Orientation, PixelSize, RepresentationKey, RepresentationSource, UploadBudget, UploadIdentity,
    UploadPool, UploadRefusal, UploadStage, admit_upload, decode_for_upload, refusal_reason,
};
use std::io::Cursor;

const FRAME_BYTES: u64 = 2048 * 2048 * 4;

fn limits() -> DecodeLimits {
    DecodeBudget::ratified_default()
        .limits()
        .expect("ratified budget resolves")
}

/// A static 8-bit RGB PNG of the approved class.
fn approved_png(width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode an 8-bit RGB PNG");
    bytes.into_inner()
}

/// A static 8-bit RGBA PNG: same size, different colour format, so the class must be refused.
fn rgba_png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([(x % 251) as u8, (y % 241) as u8, 90, 255])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode an 8-bit RGBA PNG");
    bytes.into_inner()
}

fn key(tag: u8) -> RepresentationKey {
    RepresentationKey {
        source: RepresentationSource::Local {
            provenance: LocalProvenance::Fixture {
                fixture: LocalId::new("f").expect("id"),
                manifest: LocalId::new("m").expect("id"),
                source: LocalId::new("s").expect("id"),
            },
            revision: u64::from(tag),
        },
        kind: desktop_protocol::RepresentationKind::Preview,
        size: PixelSize::new(2048, 2048).expect("size"),
        orientation: Orientation::Normal,
        policy: DecodePolicy {
            decoder: LocalId::new("test").expect("id"),
            colour: LocalId::new("test").expect("id"),
            generation: 1,
        },
    }
}

/// An identity whose digest matches the given raster, so only the fields under test vary.
fn identity_for(tag: u8, generation: u64, verified: &media::VerifiedRaster) -> UploadIdentity {
    UploadIdentity {
        key: key(tag),
        source_revision: u64::from(tag),
        device_generation: generation,
        content_digest: verified.content_digest(),
    }
}

fn one_frame_budget() -> UploadBudget {
    UploadBudget {
        staging_bytes: FRAME_BYTES,
        in_flight_bytes: FRAME_BYTES,
        resident_bytes: FRAME_BYTES,
    }
}

fn verified_approved() -> media::VerifiedRaster {
    decode_for_upload(
        &approved_png(2048, 2048),
        limits(),
        &Cancellation::default(),
    )
    .expect("the approved class decodes for upload")
}

/// Serialises the memory-heavy tests in this binary.
///
/// Every test here decodes at least one 2048×2048 frame (16 MiB of RGBA8) and one decodes 4096×4096.
/// Running twenty such tests at once made the binary die with `STATUS_ACCESS_VIOLATION` in the
/// Gate's environment while passing locally, which is a memory-pressure crash rather than a test
/// failure. One heavy test at a time keeps the peak to a single frame.
static HEAVY: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn heavy() -> std::sync::MutexGuard<'static, ()> {
    HEAVY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn only_the_approved_class_produces_a_verified_raster() {
    let _heavy = heavy();
    // The proof type has no public constructor, so the only way to get one is a decode that checks
    // the container, the bit depth, the colour format and the size.
    assert!(
        decode_for_upload(&rgba_png(2048, 2048), limits(), &Cancellation::default()).is_err(),
        "RGBA is not the approved colour format"
    );
    assert!(
        decode_for_upload(
            &approved_png(1024, 1024),
            limits(),
            &Cancellation::default()
        )
        .is_err(),
        "the approved class is 2048x2048"
    );
    let verified = verified_approved();
    assert_eq!(verified.class(), media::SourceClass::StillPng8Rgb2048);
    assert_eq!(verified.size(), PixelSize::new(2048, 2048).expect("size"));
}

#[test]
fn the_approved_raster_is_admitted_and_holds_its_payload() {
    let _heavy = heavy();
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity_for(1, 4, &verified),
        4,
        &pool,
        &Cancellation::default(),
    )
    .expect("the approved class is admitted");

    assert_eq!(admitted.stage(), UploadStage::Staged);
    assert!(
        admitted.raster().is_some(),
        "the CPU payload is held for the whole admission"
    );
    assert_eq!(admitted.bytes(), FRAME_BYTES);
    assert_eq!(admitted.stride_bytes().expect("stride"), 2048 * 4);
    let usage = pool.usage();
    assert_eq!(usage.staging, FRAME_BYTES);
    assert_eq!(usage.in_flight, FRAME_BYTES);
    assert_eq!(usage.resident, FRAME_BYTES);

    admitted.release().expect("release");
    let usage = pool.usage();
    assert_eq!(
        (
            usage.staging,
            usage.in_flight,
            usage.resident,
            usage.unreclaimed
        ),
        (0, 0, 0, 0),
        "release frees all three and quarantines nothing"
    );
}

#[test]
fn lifetime_moves_through_states_and_a_plain_drop_quarantines_resident_bytes() {
    let _heavy = heavy();
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity_for(2, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("admitted");

    assert_eq!(admitted.begin_upload(), Ok(()));
    assert_eq!(admitted.stage(), UploadStage::Uploading);
    assert!(
        admitted.raster().is_some(),
        "the CPU copy stays until completion is asserted"
    );
    assert_eq!(admitted.complete_upload(), Ok(()));
    assert_eq!(admitted.stage(), UploadStage::Resident);
    assert!(
        admitted.raster().is_none(),
        "the CPU copy is dropped once the caller asserts the device copy exists"
    );

    // Dropping a resident admission without release must not report the device memory as freed.
    drop(admitted);
    let usage = pool.usage();
    assert_eq!(usage.resident, 0, "the resident reservation is withdrawn");
    assert_eq!(
        usage.unreclaimed, FRAME_BYTES,
        "but the bytes are recorded as unreclaimed, not silently freed"
    );
}

#[test]
fn the_state_machine_refuses_out_of_order_calls() {
    let _heavy = heavy();
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity_for(3, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("admitted");

    assert_eq!(
        admitted.complete_upload(),
        Err(UploadRefusal::StageViolation),
        "completing before beginning is a violation"
    );
    assert_eq!(admitted.begin_upload(), Ok(()));
    assert_eq!(
        admitted.begin_upload(),
        Err(UploadRefusal::StageViolation),
        "beginning twice is a violation"
    );
    // Releasing mid-transfer would hand in-flight bytes to another transfer.
    assert_eq!(
        admitted.release(),
        Err(UploadRefusal::StageViolation),
        "an uploading admission cannot be released"
    );
}

#[test]
fn a_generation_that_is_not_current_is_refused_in_both_directions() {
    let _heavy = heavy();
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    for (requested, current) in [(2u64, 3u64), (4, 3)] {
        match admit_upload(
            &verified,
            identity_for(4, requested, &verified),
            current,
            &pool,
            &Cancellation::default(),
        ) {
            Err(UploadRefusal::GenerationMismatch { .. }) => {}
            other => {
                panic!("expected a generation mismatch for {requested} vs {current}, got {other:?}")
            }
        }
    }
    let usage = pool.usage();
    assert_eq!((usage.staging, usage.resident), (0, 0));
}

#[test]
fn a_one_byte_tighter_budget_refuses_each_counter_independently() {
    let _heavy = heavy();
    let verified = verified_approved();
    for tighten in [
        |budget: &mut UploadBudget| budget.staging_bytes -= 1,
        |budget: &mut UploadBudget| budget.in_flight_bytes -= 1,
        |budget: &mut UploadBudget| budget.resident_bytes -= 1,
    ] {
        let mut budget = one_frame_budget();
        tighten(&mut budget);
        let pool = UploadPool::new(budget);
        match admit_upload(
            &verified,
            identity_for(5, 1, &verified),
            1,
            &pool,
            &Cancellation::default(),
        ) {
            Err(UploadRefusal::BudgetExceeded { .. }) => {}
            other => panic!("expected a budget refusal, got {other:?}"),
        }
        let usage = pool.usage();
        assert_eq!(
            (usage.staging, usage.in_flight, usage.resident),
            (0, 0, 0),
            "a refusal must not reserve anything"
        );
    }
}

#[test]
fn concurrent_admissions_of_different_identities_cannot_exceed_the_budget() {
    let _heavy = heavy();
    // The pool admits under one lock, so two threads cannot both pass the check and then both
    // reserve. A barrier releases every attempt at once and each successful reservation is held
    // until all attempts have been made, so scheduling order cannot let a late thread succeed
    // after an earlier one was already released.
    const THREADS: u8 = 8;
    let pool = UploadPool::new(one_frame_budget());
    let verified = std::sync::Arc::new(verified_approved());
    let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // One party per thread plus this thread, which waits with them so every attempt starts together.
    let barrier_start = std::sync::Arc::new(std::sync::Barrier::new(usize::from(THREADS) + 1));
    let barrier_hold = std::sync::Arc::new(std::sync::Barrier::new(usize::from(THREADS) + 1));
    let mut handles = Vec::new();
    for tag in 0..THREADS {
        let pool = std::sync::Arc::clone(&pool);
        let verified = std::sync::Arc::clone(&verified);
        let results = std::sync::Arc::clone(&results);
        let barrier_start = std::sync::Arc::clone(&barrier_start);
        let barrier_hold = std::sync::Arc::clone(&barrier_hold);
        handles.push(std::thread::spawn(move || {
            // Wait here so every attempt is issued together; the barrier is before the call,
            // not after it.
            barrier_start.wait();
            let outcome = admit_upload(
                &verified,
                identity_for(tag + 10, 1, &verified),
                1,
                &pool,
                &Cancellation::default(),
            );
            let admitted = outcome.is_ok();
            results.lock().expect("results").push(admitted);
            // Hold the reservation until every attempt has been made.
            barrier_hold.wait();
            if let Ok(mut admission) = outcome {
                admission.release().expect("release");
            }
        }));
    }
    // Release every attempt at once, then let the holders finish.
    barrier_start.wait();
    barrier_hold.wait();
    for handle in handles {
        handle.join().expect("thread");
    }
    let admitted = results
        .lock()
        .expect("results")
        .iter()
        .filter(|ok| **ok)
        .count();
    assert_eq!(
        admitted, 1,
        "exactly one frame fits, so exactly one admission may succeed"
    );
    let usage = pool.usage();
    assert_eq!(
        (
            usage.staging,
            usage.in_flight,
            usage.resident,
            usage.unreclaimed
        ),
        (0, 0, 0, 0),
        "every successful admission was released"
    );
}

#[test]
fn duplicate_and_reuse_of_the_same_key() {
    let _heavy = heavy();
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES * 2,
        in_flight_bytes: FRAME_BYTES * 2,
        resident_bytes: FRAME_BYTES * 2,
    });
    let verified = verified_approved();
    let mut first = admit_upload(
        &verified,
        identity_for(6, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("first admission");
    match admit_upload(
        &verified,
        identity_for(6, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::DuplicateRepresentation) => {}
        other => panic!("expected a duplicate refusal, got {other:?}"),
    }
    first.release().expect("release");
    // After release the same key is admitted again: reuse is allowed, duplication is not.
    let mut second = admit_upload(
        &verified,
        identity_for(6, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("the same key is reusable after release");
    second.release().expect("release");
    let usage = pool.usage();
    assert_eq!((usage.staging, usage.resident), (0, 0));
}

#[test]
fn a_cancelled_caller_is_refused_before_anything_is_reserved() {
    let _heavy = heavy();
    let pool = UploadPool::new(one_frame_budget());
    let cancel = Cancellation::default();
    cancel.cancel();
    let verified = verified_approved();
    match admit_upload(&verified, identity_for(7, 1, &verified), 1, &pool, &cancel) {
        Err(UploadRefusal::Cancelled) => {}
        other => panic!("expected Cancelled, got {other:?}"),
    }
    let usage = pool.usage();
    assert_eq!((usage.staging, usage.resident), (0, 0));
}

#[test]
fn the_refusal_reasons_map_to_the_existing_error_vocabulary() {
    let _heavy = heavy();
    assert_eq!(
        refusal_reason(&UploadRefusal::GenerationMismatch {
            requested: 1,
            current: 2
        }),
        MediaError::Unavailable
    );
    assert_eq!(
        refusal_reason(&UploadRefusal::BudgetExceeded {
            staging: true,
            in_flight: false,
            resident: false
        }),
        MediaError::Busy
    );
    assert_eq!(
        refusal_reason(&UploadRefusal::DuplicateRepresentation),
        MediaError::Busy
    );
    assert_eq!(
        refusal_reason(&UploadRefusal::WrongLayout),
        MediaError::InvalidMetadata
    );
    assert_eq!(
        refusal_reason(&UploadRefusal::Cancelled),
        MediaError::Cancelled
    );
    assert_eq!(
        refusal_reason(&UploadRefusal::StageViolation),
        MediaError::Unavailable
    );
}

#[test]
fn a_quarantined_identity_blocks_reuse_and_counts_against_the_resident_limit() {
    let _heavy = heavy();
    // A dropped resident admission leaves bytes nobody confirmed released. They must keep counting
    // against the device budget and must stop the same representation being admitted again.
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity_for(8, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("admitted");
    admitted.begin_upload().expect("begin");
    admitted.complete_upload().expect("complete");
    drop(admitted);

    let usage = pool.usage();
    assert_eq!(usage.unreclaimed, FRAME_BYTES);

    // Same identity: refused, because the device may still hold it.
    match admit_upload(
        &verified,
        identity_for(8, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::DuplicateRepresentation) => {}
        other => panic!("expected the quarantined key to be refused, got {other:?}"),
    }
    // A different identity is also refused: the quarantined bytes consume the resident budget.
    match admit_upload(
        &verified,
        identity_for(9, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::BudgetExceeded { resident, .. }) => assert!(resident),
        other => panic!("expected the quarantined bytes to consume the budget, got {other:?}"),
    }
}

#[test]
fn an_interlaced_png_cannot_obtain_the_class_proof() {
    let _heavy = heavy();
    // The cancellation evidence behind this approval covers non-interlaced PNG, so an Adam7 image
    // must be refused from the header, before any decoding work happens.
    let mut bytes = approved_png(2048, 2048);
    let ihdr = bytes.windows(4).position(|w| w == b"IHDR").expect("IHDR");
    bytes[ihdr + 4 + 12] = 1;
    let crc_start = ihdr;
    let ihdr_end = crc_start + 4 + 13;
    let crc = crc32(&bytes[crc_start..ihdr_end]);
    bytes[ihdr_end..ihdr_end + 4].copy_from_slice(&crc.to_be_bytes());
    assert!(
        decode_for_upload(&bytes, limits(), &Cancellation::default()).is_err(),
        "an interlaced PNG is outside the approved class"
    );
}

#[test]
fn an_identity_that_disagrees_with_the_raster_is_refused() {
    let _heavy = heavy();
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    // A key claiming a different size, and one claiming a different revision, must both be refused.
    let mut wrong_size = identity_for(11, 1, &verified);
    wrong_size.key.size = PixelSize::new(1024, 1024).expect("size");
    match admit_upload(&verified, wrong_size, 1, &pool, &Cancellation::default()) {
        Err(UploadRefusal::WrongLayout) => {}
        other => panic!("expected a layout refusal for the size, got {other:?}"),
    }
    let mut wrong_revision = identity_for(12, 1, &verified);
    wrong_revision.source_revision = 999;
    match admit_upload(
        &verified,
        wrong_revision,
        1,
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::WrongLayout) => {}
        other => panic!("expected a layout refusal for the revision, got {other:?}"),
    }
    let usage = pool.usage();
    assert_eq!((usage.staging, usage.resident), (0, 0));
}

/// CRC-32 as PNG chunks use it, for the interlaced fixture.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[test]
fn a_staged_admission_releases_normally_but_an_uploading_one_is_quarantined() {
    let _heavy = heavy();
    // A staged admission never transferred anything, so dropping it must give the capacity back
    // rather than quarantining it forever.
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    {
        let _admitted = admit_upload(
            &verified,
            identity_for(20, 1, &verified),
            1,
            &pool,
            &Cancellation::default(),
        )
        .expect("admitted");
    }
    let usage = pool.usage();
    assert_eq!(
        (
            usage.staging,
            usage.in_flight,
            usage.resident,
            usage.unreclaimed
        ),
        (0, 0, 0, 0),
        "a staged admission that is dropped early frees everything"
    );

    // Mid-transfer the bytes are still in use, so nothing may be reported as released.
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES * 4,
        in_flight_bytes: FRAME_BYTES * 4,
        resident_bytes: FRAME_BYTES * 4,
    });
    {
        let mut admitted = admit_upload(
            &verified,
            identity_for(21, 1, &verified),
            1,
            &pool,
            &Cancellation::default(),
        )
        .expect("admitted");
        admitted.begin_upload().expect("begin");
        // Release during upload is refused, and the caller keeps ownership until it drops.
        assert!(
            admitted.release().is_err(),
            "release during upload is refused"
        );
    }
    let usage = pool.usage();
    assert_eq!(
        usage.unreclaimed, FRAME_BYTES,
        "an uploading admission quarantines one frame when dropped, not three"
    );
    assert_eq!(
        (usage.staging, usage.in_flight, usage.resident),
        (0, 0, 0),
        "and the live counters no longer hold it"
    );
}

#[test]
fn an_out_of_class_size_is_refused_from_the_header() {
    let _heavy = heavy();
    // 4096×4096 is 4× the approved area; it must be refused before the decoder expands it.
    let bytes = approved_png(4096, 4096);
    match decode_for_upload(&bytes, limits(), &Cancellation::default()) {
        Err(MediaError::UnsupportedContainer) => {}
        other => panic!("expected an early header refusal, got {other:?}"),
    }
}

#[test]
fn release_is_terminal_and_cannot_disturb_a_re_admitted_key() {
    let _heavy = heavy();
    // A second release must not touch a key that by then belongs to a new admission.
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES * 2,
        in_flight_bytes: FRAME_BYTES * 2,
        resident_bytes: FRAME_BYTES * 2,
    });
    let verified = verified_approved();
    let mut first = admit_upload(
        &verified,
        identity_for(30, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("first admission");
    first.release().expect("first release");
    assert_eq!(first.stage(), UploadStage::Released);

    let mut second = admit_upload(
        &verified,
        identity_for(30, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("the same key is admissible again after a clean release");

    // Releasing the finished one again would remove the live key from the pool.
    assert_eq!(
        first.release(),
        Err(UploadRefusal::StageViolation),
        "release is terminal"
    );
    assert_eq!(
        first.begin_upload(),
        Err(UploadRefusal::StageViolation),
        "a released admission cannot transition"
    );
    // The second admission still holds its identity: a duplicate attempt is still refused.
    match admit_upload(
        &verified,
        identity_for(30, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::DuplicateRepresentation) => {}
        other => panic!("the live admission must still own its key, got {other:?}"),
    }
    second.release().expect("release");
}

#[test]
fn an_uploading_drop_quarantines_one_frame_not_three() {
    let _heavy = heavy();
    // The bytes must leave the live counters and appear once in the quarantine, not twice.
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES * 4,
        in_flight_bytes: FRAME_BYTES * 4,
        resident_bytes: FRAME_BYTES * 4,
    });
    let verified = verified_approved();
    {
        let mut admitted = admit_upload(
            &verified,
            identity_for(31, 1, &verified),
            1,
            &pool,
            &Cancellation::default(),
        )
        .expect("admitted");
        admitted.begin_upload().expect("begin");
    }
    let usage = pool.usage();
    assert_eq!(
        (usage.staging, usage.in_flight, usage.resident),
        (0, 0, 0),
        "the live counters must not keep the quarantined frame"
    );
    assert_eq!(
        usage.unreclaimed, FRAME_BYTES,
        "the quarantine holds exactly one frame"
    );
}

#[test]
fn a_quarantined_transfer_keeps_charging_staging_and_in_flight() {
    let _heavy = heavy();
    // Quarantine is per resource: a stuck transfer must not free the staging or in-flight capacity
    // it still occupies, or a second transfer could exceed those limits.
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES,
        in_flight_bytes: FRAME_BYTES,
        resident_bytes: FRAME_BYTES * 4,
    });
    let verified = verified_approved();
    {
        let mut admitted = admit_upload(
            &verified,
            identity_for(40, 1, &verified),
            1,
            &pool,
            &Cancellation::default(),
        )
        .expect("admitted");
        admitted.begin_upload().expect("begin");
    }
    assert!(pool.has_quarantine());
    // Resident has room for three more frames, staging and in-flight have none: the refusal must be
    // charged to those two, not to resident.
    match admit_upload(
        &verified,
        identity_for(41, 1, &verified),
        1,
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::BudgetExceeded {
            staging,
            in_flight,
            resident,
        }) => {
            assert!(staging, "the stuck transfer still occupies staging");
            assert!(in_flight, "the stuck transfer still occupies in-flight");
            assert!(!resident, "resident capacity is not the limit here");
        }
        other => panic!("expected a staging/in-flight refusal, got {other:?}"),
    }
}

#[test]
fn an_abandoned_transfer_keeps_its_payload_until_termination_is_confirmed() {
    let _heavy = heavy();
    // The payload must not be takeable: a caller holding the bytes could drop them while a
    // transfer still reads them. It is held with its identity and freed only by an explicit
    // termination assertion.
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES * 2,
        in_flight_bytes: FRAME_BYTES * 2,
        resident_bytes: FRAME_BYTES * 2,
    });
    let verified = verified_approved();
    let identity_used = identity_for(42, 1, &verified);
    {
        let mut admitted = admit_upload(
            &verified,
            identity_used.clone(),
            1,
            &pool,
            &Cancellation::default(),
        )
        .expect("admitted");
        admitted.begin_upload().expect("begin");
    }
    let held = pool.orphaned();
    assert_eq!(held.len(), 1, "the payload is held, not handed out");
    assert_eq!(held[0].bytes, FRAME_BYTES);
    assert_eq!(held[0].device_generation, 1);
    assert_eq!(held[0].key, identity_used.key);

    // Termination for a different identity changes nothing.
    let mut wrong = identity_used.clone();
    wrong.source_revision += 1;
    assert!(pool.confirm_transfer_terminated(&wrong).is_err());
    assert_eq!(pool.orphaned().len(), 1);

    // Confirming the right one frees the payload and every quarantined category.
    pool.confirm_transfer_terminated(&identity_used)
        .expect("termination confirmed");
    assert!(pool.orphaned().is_empty());
    let usage = pool.usage();
    assert_eq!(
        (
            usage.staging_stuck,
            usage.in_flight_stuck,
            usage.resident_stuck
        ),
        (0, 0, 0),
        "confirming termination releases all three categories"
    );
}

#[test]
fn an_identity_whose_digest_does_not_match_the_pixels_is_refused() {
    let _heavy = heavy();
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut wrong = identity_for(50, 1, &verified);
    wrong.content_digest[0] ^= 0xff;
    match admit_upload(&verified, wrong, 1, &pool, &Cancellation::default()) {
        Err(UploadRefusal::WrongLayout) => {}
        other => panic!("expected a digest mismatch to be refused, got {other:?}"),
    }
    let usage = pool.usage();
    assert_eq!((usage.staging, usage.resident), (0, 0));
}
