// MP-IMG2 internal allocator peak, measured on the assigned class.
//
// §19.119 left internal allocator peak and rejection allocation flatness at HOLD because
// measuring them needs a counting `GlobalAlloc` and this workspace forbids *own-code*
// `unsafe`. This binary takes the unblock option that needs none: a third-party allocator
// wrapper (`peak_alloc`, dev-dependency of `media` only, Apache-2.0/MIT) declared as
// `#[global_allocator]`.
//
// **What the counter measures:** `peak_alloc::peak_usage()` is a **process-wide** figure -
// the maximum live allocation at any moment - not a decoder-only figure. `reset_peak_usage()`
// resets the peak to the *current* live amount, not to zero. Every figure here is therefore
// an upper bound that includes this harness, and the peak assertions are "does not exceed".
//
// Measured: the assigned 2048×2048 static 8-bit PNG class against its declared ceiling, and
// that refusal allocates a **flat increment** as the refused area grows. Not measured:
// concurrency, allocator behaviour under cancellation, or the process Working Set
// (`examples/peak_probe.rs`).

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

/// Peak allocated **during** the call, above what was already live when it started.
///
/// The encoded input is built before this is called and stays alive, so its bytes sit in the
/// baseline instead of being counted as decode work.
fn peak_increment_during<F: FnOnce()>(body: F) -> u64 {
    let baseline = PEAK_ALLOC.current_usage() as u64;
    PEAK_ALLOC.reset_peak_usage();
    body();
    (PEAK_ALLOC.peak_usage() as u64).saturating_sub(baseline)
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

    let mut raster = None;
    let increment = peak_increment_during(|| {
        raster = Some(
            decode_raster_joined(&bytes, limits(), &Cancellation::default())
                .expect("the class decodes"),
        );
    });
    drop(raster);

    eprintln!("2048x2048 decode increment above baseline: {increment} B (ceiling {ceiling} B)");
    assert!(
        increment > 0,
        "the counter observed nothing, so this measurement proves nothing"
    );
    assert!(
        increment <= ceiling,
        "decode increment {increment} B exceeded the declared ceiling {ceiling} B"
    );
}

#[test]
fn refusal_allocation_does_not_grow_with_the_refused_area() {
    let _guard = test_guard();
    // Three refused inputs whose areas differ by 16×: 20,000 px, 80,000 px and 320,000 px.
    // A decode-shaped allocation would grow with the area; a refusal must not. The encoded
    // bytes are built before each measurement, so only the decode work is counted.
    let cases = [(20_000u32, 1u32), (20_000, 4), (20_000, 16)];
    let mut increments = Vec::new();
    for (width, height) in cases {
        let bytes = png_rgb(width, height);
        let area = u64::from(width) * u64::from(height);
        let mut observed = None;
        let increment = peak_increment_during(|| {
            observed = Some(decode_raster_joined(
                &bytes,
                limits(),
                &Cancellation::default(),
            ));
        });
        eprintln!(
            "refused {width}x{height} ({area} px): increment {increment} B; \
             a decode of that area would need roughly {} B of RGBA8",
            area * 4
        );
        assert_eq!(
            observed.expect("a result").err(),
            Some(MediaError::TooLarge),
            "{width}x{height} must be refused for size"
        );
        increments.push(increment);
    }

    // The largest refused area would need 1,280,000 B of RGBA8 alone.
    let largest = *increments.iter().max().expect("three cases");
    let biggest_area_bytes = 20_000u64 * 16 * 4;
    assert!(
        largest * 4 < biggest_area_bytes,
        "the worst refusal increment {largest} B is not far below the {biggest_area_bytes} B an \
         admitted decode of the largest refused area would need"
    );
    // And the increments must not scale with the area: the 16× case may not cost 16× the first.
    let smallest = *increments.iter().min().expect("three cases");
    assert!(
        largest <= smallest.saturating_mul(4) + 64 * 1024,
        "refusal cost grew with the refused area: {increments:?}"
    );
}
