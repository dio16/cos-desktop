// MP-IMG2 internal allocator peak, measured on the assigned class.
//
// §19.119 left internal allocator peak and rejection allocation flatness at HOLD because
// measuring them needs a counting `GlobalAlloc` and this workspace forbids *own-code*
// `unsafe`. This binary takes the unblock option that needs none: a third-party allocator
// wrapper (`peak_alloc`, dev-dependency of `media` only, Apache-2.0/MIT) declared as
// `#[global_allocator]`.
//
// **What the counter measures:** `peak_alloc::peak_usage()` is a **process-wide** figure -
// the maximum live allocation at any moment - not a decoder-only figure. `reset_peak_usage`
// resets the peak to the *current* live amount, not to zero. Both facts make every number
// here an **upper bound** that includes this test harness's own allocations, which is why
// the assertions are phrased as "does not exceed", never as "the decoder used exactly X".
//
// Measured: the assigned 2048×2048 static 8-bit PNG class against its declared admission
// ceiling, and that a refused input does not allocate anywhere near what admitting it would
// have needed. Not measured: concurrency, allocator behaviour under cancellation, or the
// process Working Set (`examples/peak_probe.rs`).

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{
    Cancellation, DecodeBudget, DecodeLimits, MediaError, PixelSize, decode_raster_joined,
};
use peak_alloc::PeakAlloc;
use std::io::Cursor;

#[global_allocator]
static PEAK_ALLOC: PeakAlloc = PeakAlloc;

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().expect("test lock")
}

fn limits() -> DecodeLimits {
    DecodeBudget::ratified_default()
        .limits()
        .expect("ratified budget resolves")
}

fn png_rgb(width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode an 8-bit RGB PNG");
    bytes.into_inner()
}

#[test]
fn the_assigned_class_peak_does_not_exceed_its_ceiling() {
    let _guard = test_guard();
    // The assigned measurement class from §19.119: static 8-bit RGB PNG at 2048×2048.
    let size = PixelSize::new(2048, 2048).expect("size");
    let ceiling = limits().peak_rgba8_bytes(size).expect("ceiling");
    assert_eq!(
        ceiling, 34_603_008,
        "the assigned class ceiling is fixed by the formula"
    );
    let bytes = png_rgb(2048, 2048);

    PEAK_ALLOC.reset_peak_usage();
    let raster = decode_raster_joined(&bytes, limits(), &Cancellation::default())
        .expect("the class decodes");
    let peak = PEAK_ALLOC.peak_usage() as u64;
    drop(raster);

    eprintln!(
        "2048x2048 process-wide peak {peak} B against the declared ceiling {ceiling} B \
         (upper bound: includes this harness)"
    );
    assert!(
        peak > 0,
        "the counter observed nothing, so this measurement proves nothing"
    );
    assert!(
        peak <= ceiling,
        "process-wide peak {peak} B exceeded the declared ceiling {ceiling} B"
    );
}

#[test]
fn a_refused_input_does_not_allocate_like_the_decode_it_would_have_needed() {
    let _guard = test_guard();
    // Comparing a refused 20,000×1 input against an admitted 256×256 decode would compare
    // different pixel counts. Compare it instead against the ceiling that admitting it would
    // have required: refusal must not pay for the work it refuses.
    let refused_size = PixelSize::new(20_000, 1).expect("size");
    let would_be_ceiling = limits().peak_rgba8_bytes(refused_size).expect("ceiling");
    let oversized = png_rgb(20_000, 1);

    PEAK_ALLOC.reset_peak_usage();
    let refused = decode_raster_joined(&oversized, limits(), &Cancellation::default());
    let refused_peak = PEAK_ALLOC.peak_usage() as u64;

    eprintln!(
        "refused 20000x1 process-wide peak {refused_peak} B against the {would_be_ceiling} B \
         it would have needed if admitted"
    );
    assert_eq!(refused.err(), Some(MediaError::TooLarge));
    assert!(
        refused_peak * 2 < would_be_ceiling,
        "refusal allocated {refused_peak} B, which is not far below the {would_be_ceiling} B \
         an admitted decode would have needed"
    );
}
