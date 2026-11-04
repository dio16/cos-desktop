// MP-IMG3-S1: the upload admission boundary.
//
// The approved class is the 2048×2048 static 8-bit RGB PNG. These tests pin the boundary's
// behaviour: what is admitted, what is refused, and that every refusal leaves the counters
// untouched. Nothing here uploads anything, and no GPU behaviour is asserted.

use media::{
    AdmittedUpload, Cancellation, DecodedRaster, MediaError, PixelSize, UploadBudget,
    UploadIdentity, UploadPool, UploadRefusal, VerifiedSourceClass, admit_upload, refusal_reason,
};

/// Bytes for one 2048×2048 RGBA8 frame: 16 MiB, the figure the design canon compares against its
/// provisional 4 MiB upload budget.
const FRAME_BYTES: u64 = 2048 * 2048 * 4;

fn approved_size() -> PixelSize {
    PixelSize::new(2048, 2048).expect("approved size")
}

fn approved_raster() -> DecodedRaster {
    DecodedRaster {
        size: approved_size(),
        format: media::AcceptedFormat::Png,
        rgba: vec![0u8; FRAME_BYTES as usize],
    }
}

fn approved_class() -> VerifiedSourceClass {
    VerifiedSourceClass {
        source_bit_depth: 8,
        is_still_png: true,
        size: approved_size(),
    }
}

fn identity(revision: u64, generation: u64) -> UploadIdentity {
    UploadIdentity {
        content_digest: [7u8; 32],
        source_revision: revision,
        device_generation: generation,
    }
}

/// Budget that fits exactly one frame in each counter.
fn one_frame_budget() -> UploadBudget {
    UploadBudget {
        staging_bytes: FRAME_BYTES,
        in_flight_bytes: FRAME_BYTES,
        resident_bytes: FRAME_BYTES,
    }
}

#[test]
fn the_approved_class_is_admitted_and_counted_three_ways() {
    let pool = UploadPool::new();
    let raster = approved_raster();
    let admitted: AdmittedUpload = admit_upload(
        &raster,
        approved_class(),
        identity(1, 5),
        5,
        one_frame_budget(),
        &pool,
        &Cancellation::default(),
    )
    .expect("the approved class is admitted");

    assert_eq!(admitted.class(), approved_class());
    assert_eq!(admitted.stride_bytes(), 2048 * 4, "stride is width times 4");
    assert_eq!(admitted.staging_bytes(), FRAME_BYTES);
    assert_eq!(admitted.in_flight_bytes(), FRAME_BYTES);
    assert_eq!(admitted.resident_bytes(), FRAME_BYTES);
    assert_eq!(
        pool.usage(),
        (FRAME_BYTES, FRAME_BYTES, FRAME_BYTES),
        "staging, in-flight and resident are counted separately"
    );

    drop(admitted);
    assert_eq!(pool.usage(), (0, 0, 0), "dropping releases all three");
}

#[test]
fn one_byte_over_any_budget_is_refused_without_touching_the_counters() {
    let raster = approved_raster();
    for tighten in [
        |budget: &mut UploadBudget| budget.staging_bytes -= 1,
        |budget: &mut UploadBudget| budget.in_flight_bytes -= 1,
        |budget: &mut UploadBudget| budget.resident_bytes -= 1,
    ] {
        let pool = UploadPool::new();
        let mut budget = one_frame_budget();
        tighten(&mut budget);
        match admit_upload(
            &raster,
            approved_class(),
            identity(1, 5),
            5,
            budget,
            &pool,
            &Cancellation::default(),
        ) {
            Err(UploadRefusal::BudgetExceeded { .. }) => {}
            other => panic!("expected a budget refusal, got {other:?}"),
        }
        assert_eq!(
            pool.usage(),
            (0, 0, 0),
            "a refusal must not reserve anything"
        );
    }
}

#[test]
fn the_exact_budget_is_inclusive() {
    let pool = UploadPool::new();
    let raster = approved_raster();
    // Exactly the frame size is allowed; the inclusive edge is what a boundary test is for.
    let admitted = admit_upload(
        &raster,
        approved_class(),
        identity(2, 1),
        1,
        one_frame_budget(),
        &pool,
        &Cancellation::default(),
    )
    .expect("an exact fit is admitted");
    drop(admitted);
}

#[test]
fn a_different_class_is_refused_rather_than_uploaded() {
    let pool = UploadPool::new();
    let raster = approved_raster();
    let cases = [
        VerifiedSourceClass {
            source_bit_depth: 16,
            ..approved_class()
        },
        VerifiedSourceClass {
            is_still_png: false,
            ..approved_class()
        },
        VerifiedSourceClass {
            size: PixelSize::new(1024, 1024).expect("size"),
            ..approved_class()
        },
    ];
    for class in cases {
        match admit_upload(
            &raster,
            class,
            identity(3, 1),
            1,
            one_frame_budget(),
            &pool,
            &Cancellation::default(),
        ) {
            Err(UploadRefusal::UnverifiedClass) => {}
            other => panic!("expected an unverified-class refusal, got {other:?}"),
        }
    }
    assert_eq!(pool.usage(), (0, 0, 0));
}

#[test]
fn a_stale_device_generation_is_refused() {
    let pool = UploadPool::new();
    match admit_upload(
        &approved_raster(),
        approved_class(),
        identity(4, 2),
        3,
        one_frame_budget(),
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::StaleGeneration {
            requested: 2,
            current: 3,
        }) => {}
        other => panic!("expected a stale-generation refusal, got {other:?}"),
    }
    assert_eq!(pool.usage(), (0, 0, 0));
}

#[test]
fn a_cancelled_caller_is_refused_before_anything_is_reserved() {
    let pool = UploadPool::new();
    let cancel = Cancellation::default();
    cancel.cancel();
    match admit_upload(
        &approved_raster(),
        approved_class(),
        identity(5, 1),
        1,
        one_frame_budget(),
        &pool,
        &cancel,
    ) {
        Err(UploadRefusal::Cancelled) => {}
        other => panic!("expected Cancelled, got {other:?}"),
    }
    assert_eq!(pool.usage(), (0, 0, 0));
}

#[test]
fn double_admission_of_the_same_representation_is_refused_and_frees_on_release() {
    let pool = UploadPool::new();
    let first = admit_upload(
        &approved_raster(),
        approved_class(),
        identity(6, 1),
        1,
        one_frame_budget(),
        &pool,
        &Cancellation::default(),
    )
    .expect("first admission");

    // Same content, revision and device: already resident, so the pool must not count it twice.
    match admit_upload(
        &approved_raster(),
        approved_class(),
        identity(6, 1),
        1,
        one_frame_budget(),
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::BudgetExceeded { resident, .. }) => assert!(resident),
        other => panic!("expected a resident refusal, got {other:?}"),
    }
    drop(first);

    // After release the same key is admissible again: reuse is allowed, duplication is not.
    let second = admit_upload(
        &approved_raster(),
        approved_class(),
        identity(6, 1),
        1,
        one_frame_budget(),
        &pool,
        &Cancellation::default(),
    )
    .expect("the same key is reusable after release");
    second.release();
    assert_eq!(pool.usage(), (0, 0, 0));
}

#[test]
fn a_raster_whose_length_disagrees_with_its_geometry_is_refused() {
    let pool = UploadPool::new();
    let mut raster = approved_raster();
    raster.rgba.truncate(1024);
    match admit_upload(
        &raster,
        approved_class(),
        identity(7, 1),
        1,
        one_frame_budget(),
        &pool,
        &Cancellation::default(),
    ) {
        Err(UploadRefusal::WrongLayout) => {}
        other => panic!("expected a layout refusal, got {other:?}"),
    }
    assert_eq!(pool.usage(), (0, 0, 0));
}

#[test]
fn the_refusal_reasons_map_to_the_existing_error_vocabulary() {
    assert_eq!(
        refusal_reason(UploadRefusal::UnverifiedClass),
        MediaError::UnsupportedContainer
    );
    assert_eq!(
        refusal_reason(UploadRefusal::WrongLayout),
        MediaError::InvalidMetadata
    );
    assert_eq!(
        refusal_reason(UploadRefusal::StaleGeneration {
            requested: 1,
            current: 2
        }),
        MediaError::Unavailable
    );
    assert_eq!(
        refusal_reason(UploadRefusal::BudgetExceeded {
            staging: true,
            in_flight: false,
            resident: false
        }),
        MediaError::Busy
    );
    assert_eq!(
        refusal_reason(UploadRefusal::Cancelled),
        MediaError::Cancelled
    );
}
