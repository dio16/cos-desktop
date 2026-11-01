//! MP-IMG2 measured class attempt: static PNG, 8-bit, 64×64.
//!
//! Assigned class (plan §19.119): a **static 8-bit RGB PNG** at 64×64 pixels, decoded to
//! RGBA8 under the ratified declared budget (`DecodeBudget::ratified_default`). The
//! declared admission ceiling for that output is
//! `2 × 64 × 64 × 4 B + DECODER_BOOKKEEPING_BYTES` = **1,081,344 B**.
//!
//! **Internal peak memory is NOT measured here and is HOLD.** The slice asked for a
//! test-only counting global allocator, but implementing `GlobalAlloc` requires an
//! `unsafe impl` and this workspace forbids own-code `unsafe` (`unsafe_code = "forbid"`,
//! not overridable by an `allow`). No established allocator-measurement seam exists in
//! the repository and no dependency may be added without an owner decision, so this
//! binary deliberately does **not** claim an allocation measurement; it proves the
//! conditions that need no allocator: worker lifetime, typed rejection and
//! non-publication. See plan §19.119 for the HOLD and the minimal unblock options.
//!
//! No product code is instrumented for allocation counting; the only product addition is
//! the `live_decode_workers()` diagnostics counter.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{
    Cancellation, DecodeBudget, DecodeLimits, MediaError, PixelSize, decode_raster_joined,
    live_decode_workers,
};
use std::io::Cursor;

/// Serialises the tests: `live_decode_workers()` is process-global, so a concurrent
/// decode would be observed by another test.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().expect("test lock")
}

/// The declared budget this class is measured against (owner-ratified assumed values).
fn limits() -> DecodeLimits {
    DecodeBudget::ratified_default()
        .limits()
        .expect("ratified budget resolves")
}

/// A static 8-bit RGB PNG of the requested dimensions.
fn png_rgb(width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode a 8-bit RGB PNG");
    bytes.into_inner()
}

#[test]
fn the_class_decodes_completely_and_leaves_no_worker() {
    let _guard = test_guard();
    assert_eq!(
        limits()
            .peak_rgba8_bytes(PixelSize::new(64, 64).expect("size"))
            .expect("peak"),
        1_081_344,
        "the class ceiling is fixed by the formula, not by an estimate"
    );
    let bytes = png_rgb(64, 64);
    let raster = decode_raster_joined(&bytes, limits(), &Cancellation::default())
        .expect("the class decodes");
    assert_eq!(raster.size, PixelSize::new(64, 64).expect("size"));
    assert_eq!(
        raster.rgba.len() as u64,
        raster.size.checked_rgba_bytes().expect("rgba length"),
        "the returned raster is complete, never partial"
    );
    assert_eq!(
        live_decode_workers(),
        0,
        "the decode worker is joined before the call returns"
    );
}

#[test]
fn cancellation_leaves_no_worker_and_no_published_raster() {
    let _guard = test_guard();
    let cancel = Cancellation::default();
    cancel.cancel();
    let mut published: Option<media::DecodedRaster> = None;
    let result = decode_raster_joined(&png_rgb(64, 64), limits(), &cancel);
    if let Ok(raster) = result {
        published = Some(raster);
    }
    assert!(
        published.is_none(),
        "a cancelled decode must not publish a raster"
    );
    assert_eq!(
        live_decode_workers(),
        0,
        "the cancelled decode leaves no worker"
    );
}

#[test]
fn an_over_limit_png_is_refused_typed_and_publishes_nothing() {
    let _guard = test_guard();
    // 20,000 px is above the declared 16,384 px edge; admission refuses it before decode.
    let bytes = png_rgb(20_000, 1);
    let mut published: Option<media::DecodedRaster> = None;
    let result = decode_raster_joined(&bytes, limits(), &Cancellation::default());
    match result {
        Err(error) => assert_eq!(error, MediaError::TooLarge),
        Ok(raster) => published = Some(raster),
    }
    assert!(
        published.is_none(),
        "a refused input must not publish a raster"
    );
    assert_eq!(
        live_decode_workers(),
        0,
        "a refused input starts no worker that outlives the call"
    );
}
