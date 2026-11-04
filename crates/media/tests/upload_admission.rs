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

fn identity(tag: u8, generation: u64) -> UploadIdentity {
    UploadIdentity {
        key: key(tag),
        source_revision: u64::from(tag),
        device_generation: generation,
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

#[test]
fn only_the_approved_class_produces_a_verified_raster() {
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
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity(1, 4),
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
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity(2, 1),
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
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity(3, 1),
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
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    for (requested, current) in [(2u64, 3u64), (4, 3)] {
        match admit_upload(
            &verified,
            identity(4, requested),
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
            identity(5, 1),
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
    // The pool admits under one lock, so two threads cannot both pass the check and then both
    // reserve. A barrier releases every attempt at once and each successful reservation is held
    // until all attempts have been made, so scheduling order cannot let a late thread succeed
    // after an earlier one was already released.
    const THREADS: u8 = 8;
    let pool = UploadPool::new(one_frame_budget());
    let verified = std::sync::Arc::new(verified_approved());
    let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // One party per thread plus this thread, which waits with them so every attempt starts together.
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(usize::from(THREADS) + 1));
    let released = std::sync::Arc::new(std::sync::Barrier::new(usize::from(THREADS) + 1));
    let mut handles = Vec::new();
    for tag in 0..THREADS {
        let pool = std::sync::Arc::clone(&pool);
        let verified = std::sync::Arc::clone(&verified);
        let results = std::sync::Arc::clone(&results);
        let barrier = std::sync::Arc::clone(&barrier);
        let released = std::sync::Arc::clone(&released);
        handles.push(std::thread::spawn(move || {
            let outcome = admit_upload(
                &verified,
                identity(tag + 10, 1),
                1,
                &pool,
                &Cancellation::default(),
            );
            let admitted = outcome.is_ok();
            results.lock().expect("results").push(admitted);
            // Wait for every attempt before the main thread releases the reservations.
            barrier.wait();
            released.wait();
            if let Ok(mut admission) = outcome {
                admission.release().expect("release");
            }
        }));
    }
    // Let all attempts complete, then let the holders go.
    barrier.wait();
    released.wait();
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
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES * 2,
        in_flight_bytes: FRAME_BYTES * 2,
        resident_bytes: FRAME_BYTES * 2,
    });
    let verified = verified_approved();
    let mut first = admit_upload(
        &verified,
        identity(6, 1),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("first admission");
    match admit_upload(
        &verified,
        identity(6, 1),
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
        identity(6, 1),
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
    let pool = UploadPool::new(one_frame_budget());
    let cancel = Cancellation::default();
    cancel.cancel();
    match admit_upload(&verified_approved(), identity(7, 1), 1, &pool, &cancel) {
        Err(UploadRefusal::Cancelled) => {}
        other => panic!("expected Cancelled, got {other:?}"),
    }
    let usage = pool.usage();
    assert_eq!((usage.staging, usage.resident), (0, 0));
}

#[test]
fn the_refusal_reasons_map_to_the_existing_error_vocabulary() {
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
    // A dropped resident admission leaves bytes nobody confirmed released. They must keep counting
    // against the device budget and must stop the same representation being admitted again.
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    let mut admitted = admit_upload(
        &verified,
        identity(8, 1),
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
        identity(8, 1),
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
        identity(9, 1),
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
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    // A key claiming a different size, and one claiming a different revision, must both be refused.
    let mut wrong_size = identity(11, 1);
    wrong_size.key.size = PixelSize::new(1024, 1024).expect("size");
    match admit_upload(&verified, wrong_size, 1, &pool, &Cancellation::default()) {
        Err(UploadRefusal::WrongLayout) => {}
        other => panic!("expected a layout refusal for the size, got {other:?}"),
    }
    let mut wrong_revision = identity(12, 1);
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
    // A staged admission never transferred anything, so dropping it must give the capacity back
    // rather than quarantining it forever.
    let pool = UploadPool::new(one_frame_budget());
    let verified = verified_approved();
    {
        let _admitted = admit_upload(
            &verified,
            identity(20, 1),
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
            identity(21, 1),
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
    // 4096×4096 is 4× the approved area; it must be refused before the decoder expands it.
    let bytes = approved_png(4096, 4096);
    match decode_for_upload(&bytes, limits(), &Cancellation::default()) {
        Err(MediaError::UnsupportedContainer) => {}
        other => panic!("expected an early header refusal, got {other:?}"),
    }
}

#[test]
fn release_is_terminal_and_cannot_disturb_a_re_admitted_key() {
    // A second release must not touch a key that by then belongs to a new admission.
    let pool = UploadPool::new(UploadBudget {
        staging_bytes: FRAME_BYTES * 2,
        in_flight_bytes: FRAME_BYTES * 2,
        resident_bytes: FRAME_BYTES * 2,
    });
    let verified = verified_approved();
    let mut first = admit_upload(
        &verified,
        identity(30, 1),
        1,
        &pool,
        &Cancellation::default(),
    )
    .expect("first admission");
    first.release().expect("first release");
    assert_eq!(first.stage(), UploadStage::Released);

    let mut second = admit_upload(
        &verified,
        identity(30, 1),
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
        identity(30, 1),
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
            identity(31, 1),
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
