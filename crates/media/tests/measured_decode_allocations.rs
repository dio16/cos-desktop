// MP-IMG2 internal allocator peak and rejection allocation flatness.
//
// §19.119 left both conditions at HOLD because measuring them needs a counting
// `GlobalAlloc`, and this workspace forbids *own-code* `unsafe`. This binary takes the
// unblock option that needs no own-code `unsafe`: a third-party allocator wrapper
// (`peak_alloc`, Apache-2.0/MIT) declared as `#[global_allocator]`. Installing a global
// allocator is a declaration, not an `unsafe impl` in this crate.
//
// What is measured here:
// - the internal peak of one decode of the class against the declared admission ceiling;
// - that a refused decode allocates far less than an admitted one (flatness of refusal).
//
// What is NOT measured here: peak across concurrent decodes, allocator behaviour on
// cancellation, or the process Working Set (that is `examples/peak_probe.rs`).

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, MediaError, PixelSize, decode_raster_joined};
use peak_alloc::PeakAlloc;
use std::io::Cursor;

#[global_allocator]
static PEAK_ALLOC: PeakAlloc = PeakAlloc;

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().expect("test lock")
}

fn limits() -> media::DecodeLimits {
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
fn one_decode_peak_stays_within_the_declared_ceiling() {
    let _guard = test_guard();
    let size = PixelSize::new(256, 256).expect("size");
    let ceiling = limits().peak_rgba8_bytes(size).expect("ceiling");
    let bytes = png_rgb(256, 256);

    PEAK_ALLOC.reset_peak_usage();
    let raster = decode_raster_joined(&bytes, limits(), &Cancellation::default())
        .expect("the class decodes");
    let peak = PEAK_ALLOC.peak_usage() as u64;
    drop(raster);

    eprintln!("measured peak {peak} B against ceiling {ceiling} B for 256x256");
    assert!(
        peak > 0,
        "the counter must observe allocations, otherwise this test proves nothing"
    );
    assert!(
        peak <= ceiling,
        "decoded peak {peak} B exceeded the declared ceiling {ceiling} B"
    );
}

#[test]
fn a_refused_decode_allocates_far_less_than_an_admitted_one() {
    let _guard = test_guard();
    let admitted = png_rgb(256, 256);
    let oversized = png_rgb(20_000, 1);

    PEAK_ALLOC.reset_peak_usage();
    let _ = decode_raster_joined(&admitted, limits(), &Cancellation::default());
    let admitted_peak = PEAK_ALLOC.peak_usage() as u64;

    PEAK_ALLOC.reset_peak_usage();
    let refused = decode_raster_joined(&oversized, limits(), &Cancellation::default());
    let refused_peak = PEAK_ALLOC.peak_usage() as u64;

    eprintln!("admitted peak {admitted_peak} B, refused peak {refused_peak} B");
    assert_eq!(refused.err(), Some(MediaError::TooLarge));
    assert!(
        refused_peak * 4 < admitted_peak,
        "refusal should not allocate like a decode: refused {refused_peak} B against admitted {admitted_peak} B"
    );
}
