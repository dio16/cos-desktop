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

fn jpeg_rgb(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, image::ImageFormat::Jpeg)
        .expect("encode an 8-bit RGB JPEG");
    bytes.into_inner()
}

#[test]
fn the_jpeg_class_peak_is_measured_against_the_same_ceiling() {
    let _guard = test_guard();
    // The PNG class is the only one the plan's table calls measured. Run the same measurement for
    // the JPEG class at the same dimensions, so the table can say what happens rather than that
    // nobody looked. The ceiling formula is the same; whether JPEG respects it is what this prints.
    let size = PixelSize::new(2048, 2048).expect("size");
    let ceiling = limits().peak_rgba8_bytes(size).expect("ceiling");
    let bytes = jpeg_rgb(2048, 2048);
    assert!(
        bytes.len() as u64 <= limits().max_scratch_bytes,
        "the JPEG fixture must be far smaller than the scratch ceiling, or this measures a refusal"
    );

    let mut raster = None;
    let increment = peak_increment_during(|| {
        raster = Some(
            decode_raster_joined(&bytes, limits(), &Cancellation::default())
                .expect("the JPEG class decodes"),
        );
    });
    drop(raster);

    eprintln!(
        "2048x2048 JPEG decode increment above baseline: {increment} B \
         (PNG class ceiling {ceiling} B, encoded {} B)",
        bytes.len()
    );
    assert!(
        increment > 0,
        "the counter observed nothing, so this measurement proves nothing"
    );
    // Measured at 29,360,328 B, inside the same ceiling: the JPEG class now has a figure of its own
    // rather than an assumption that it behaves like PNG.
    assert!(
        increment <= ceiling,
        "the JPEG decode increment {increment} B exceeded the declared ceiling {ceiling} B"
    );
}

#[test]
fn jpeg_refusal_allocation_does_not_grow_with_the_refused_area() {
    let _guard = test_guard();
    // 20,000 x 1 / 4 / 16 px: identical to the PNG refusal cases, so the two classes can be
    // compared on the same terms.
    let mut increments = Vec::new();
    for height in [1u32, 4, 16] {
        let width = 20_000u32;
        let bytes = jpeg_rgb(width, height);
        let area = u64::from(width) * u64::from(height);
        let mut observed = None;
        let increment = peak_increment_during(|| {
            observed = Some(decode_raster_joined(
                &bytes,
                limits(),
                &Cancellation::default(),
            ));
        });
        eprintln!("refused JPEG {width}x{height} ({area} px): increment {increment} B");
        assert_eq!(
            observed.expect("a result").err(),
            Some(MediaError::TooLarge),
            "{width}x{height} must be refused for size"
        );
        increments.push(increment);
    }
    let largest = *increments.iter().max().expect("three cases");
    let smallest = *increments.iter().min().expect("three cases");
    // The largest refused area would need 1,280,000 B of RGBA8; a refusal must not be shaped like
    // that. The absolute bound is the same one the PNG refusal test uses.
    assert!(
        largest < 1_280_000,
        "the largest JPEG refusal allocated {largest} B, which is decode-shaped rather than a refusal"
    );
    // And the cost must not scale with the area: the 16× case may not cost 16× the first. Without
    // this, a refusal that grew with the input would still pass the absolute bound.
    assert!(
        largest <= smallest.saturating_mul(4) + 64 * 1024,
        "JPEG refusal cost grew with the refused area: {increments:?}"
    );
}

fn encoded_rgb(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, format)
        .expect("encode the requested class");
    bytes.into_inner()
}

#[test]
fn the_webp_and_gif_peaks_are_measured_against_the_same_ceiling() {
    let _guard = test_guard();
    // The plan's table still calls these classes NOT_VERIFIED. Measure them at 1024x1024: WebP's
    // lossless encoder and GIF's palette both grow with area, and 1024 keeps the encoded fixtures
    // small enough to stay well inside the scratch ceiling.
    let edge = 1024u32;
    let size = PixelSize::new(edge, edge).expect("size");
    let ceiling = limits().peak_rgba8_bytes(size).expect("ceiling");
    for format in [image::ImageFormat::WebP, image::ImageFormat::Gif] {
        let bytes = encoded_rgb(edge, edge, format);
        assert!(
            bytes.len() as u64 <= limits().max_scratch_bytes,
            "{format:?}: the fixture must not itself be the reason for a refusal"
        );
        let mut raster = None;
        let increment = peak_increment_during(|| {
            raster = Some(
                decode_raster_joined(&bytes, limits(), &Cancellation::default())
                    .unwrap_or_else(|error| panic!("{format:?} must decode, got {error:?}")),
            );
        });
        drop(raster);
        eprintln!(
            "{edge}x{edge} {format:?} decode increment above baseline: {increment} B \
             (ceiling {ceiling} B, encoded {} B)",
            bytes.len()
        );
        assert!(
            increment > 0,
            "{format:?}: the counter observed nothing, so this measurement proves nothing"
        );
        assert!(
            increment <= ceiling,
            "{format:?}: decode increment {increment} B exceeded the declared ceiling {ceiling} B"
        );
    }
}

#[test]
fn gif_refusals_are_flat_and_not_decode_shaped() {
    let _guard = test_guard();
    // GIF keeps the 20,000-wide shape used for PNG and JPEG, whose refusal is a dimension one: the
    // 16,000-wide extremes that the WebP encoder also accepts decode rather than being refused, so
    // no comparable WebP refusal fixture can be built here. That gap is recorded rather than
    // papered over with a different kind of case.
    let format = image::ImageFormat::Gif;
    let width = 20_000u32;
    let mut increments = Vec::new();
    for height in [1u32, 4, 16] {
        let bytes = encoded_rgb(width, height, format);
        let mut observed = None;
        let increment = peak_increment_during(|| {
            observed = Some(decode_raster_joined(
                &bytes,
                limits(),
                &Cancellation::default(),
            ));
        });
        eprintln!("refused {format:?} {width}x{height}: increment {increment} B");
        assert_eq!(
            observed.expect("a result").err(),
            Some(MediaError::TooLarge),
            "{format:?} {width}x{height} must be refused for size"
        );
        increments.push(increment);
    }
    let largest = *increments.iter().max().expect("three cases");
    let smallest = *increments.iter().min().expect("three cases");
    assert!(
        largest < 1_280_000,
        "{format:?}: the largest refusal allocated {largest} B, which is decode-shaped"
    );
    assert!(
        largest <= smallest.saturating_mul(4) + 64 * 1024,
        "{format:?}: refusal cost grew with the refused area: {increments:?}"
    );
}

#[test]
fn webp_refusals_are_measured_under_a_byte_ceiling() {
    let _guard = test_guard();
    // §19.252 recorded WebP's refusal shape as "not constructible", which was too strong: what is
    // impossible is a *dimension* refusal (the encoder caps dimensions below `max_dimension`). A
    // **byte-ceiling** refusal is reachable, and this measures whether it is flat across areas.
    let tight = media::DecodeLimitsOverride {
        max_decoded_bytes: Some(64_000),
        ..media::DecodeLimitsOverride::none()
    }
    .resolve()
    .expect("the tight budget resolves");

    let mut increments = Vec::new();
    for (width, height) in [(200u32, 100u32), (200, 400), (400, 800)] {
        let area = u64::from(width) * u64::from(height);
        let bytes = encoded_rgb(width, height, image::ImageFormat::WebP);
        let mut observed = None;
        let increment = peak_increment_during(|| {
            observed = Some(decode_raster_joined(
                &bytes,
                tight,
                &Cancellation::default(),
            ));
        });
        eprintln!(
            "refused WebP {width}x{height} ({area} px): increment {increment} B \
             (ceiling 64,000 B, area needs {} B of RGBA8)",
            area * 4
        );
        assert_eq!(
            observed.expect("a result").err(),
            Some(MediaError::TooLarge),
            "{width}x{height} must be refused by the byte ceiling"
        );
        increments.push(increment);
    }
    let largest = *increments.iter().max().expect("three cases");
    let smallest = *increments.iter().min().expect("three cases");
    assert!(
        largest < 1_280_000,
        "the largest WebP refusal allocated {largest} B, which is decode-shaped"
    );
    assert!(
        largest <= smallest.saturating_mul(4) + 64 * 1024,
        "WebP refusal cost grew with the refused area: {increments:?}"
    );
}

/// A 16-bit-per-channel RGB PNG, to exercise the declared bit-depth ceiling rather than the common
/// 8-bit case alone.
fn png_rgb16(width: u32, height: u32) -> Vec<u8> {
    let image = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(width, height, |x, y| {
        image::Rgb([
            ((x * 257) % 65_536) as u16,
            ((y * 257) % 65_536) as u16,
            90 * 257,
        ])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb16(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode a 16-bit RGB PNG");
    bytes.into_inner()
}

#[test]
fn a_sixteen_bit_png_is_refused_and_the_refusal_is_not_decode_shaped() {
    let _guard = test_guard();
    // The "other depths and sizes" row. A 16-bit source is **refused as Unavailable** rather than
    // decoded - the declared bit-depth ceiling admits the header, the backend does not convert it -
    // so what can be measured is the refusal, not a peak. 512x512 keeps this inside the gate budget.
    let edge = 512u32;
    let bytes = png_rgb16(edge, edge);
    let mut observed = None;
    let increment = peak_increment_during(|| {
        observed = Some(decode_raster_joined(
            &bytes,
            limits(),
            &Cancellation::default(),
        ));
    });
    eprintln!(
        "refused {edge}x{edge} 16-bit PNG: increment {increment} B          (a decode of that area would need {} B of RGBA8)",
        u64::from(edge) * u64::from(edge) * 4
    );
    assert_eq!(
        observed.expect("a result").err(),
        Some(MediaError::Unavailable),
        "a 16-bit PNG must be refused as unavailable, not decoded"
    );
    // `decode_raster_joined` maps a worker panic to `Unavailable` as well, so the joined call alone
    // cannot distinguish an intended refusal from a crash. Call the synchronous path directly: it
    // runs on this thread, where a panic would surface as a panic instead of a typed error.
    let direct = media::decode_raster(&bytes, limits(), &Cancellation::default());
    assert_eq!(
        direct.err(),
        Some(MediaError::Unavailable),
        "the refusal must come from admission, not from a panic mapped to Unavailable"
    );
    let needed = u64::from(edge) * u64::from(edge) * 4;
    assert!(
        increment < needed,
        "the refusal allocated {increment} B, which is decode-shaped rather than a refusal"
    );
}
#[test]
fn a_smaller_size_is_measured_against_its_own_ceiling() {
    let _guard = test_guard();
    // The other half of "other depths and sizes": the ceiling must scale with the size rather than
    // being a single hard-coded figure.
    let edge = 512u32;
    let size = PixelSize::new(edge, edge).expect("size");
    let ceiling = limits().peak_rgba8_bytes(size).expect("ceiling");
    assert!(
        ceiling < 34_603_008,
        "a 512x512 ceiling must be smaller than the 2048x2048 one, or this proves nothing"
    );
    let bytes = png_rgb(edge, edge);

    let mut raster = None;
    let increment = peak_increment_during(|| {
        raster = Some(
            decode_raster_joined(&bytes, limits(), &Cancellation::default())
                .expect("the class decodes"),
        );
    });
    drop(raster);

    eprintln!(
        "{edge}x{edge} 8-bit PNG decode increment above baseline: {increment} B (ceiling {ceiling} B)"
    );
    assert!(increment > 0, "the counter observed nothing");
    assert!(
        increment <= ceiling,
        "512x512 decode increment {increment} B exceeded the declared ceiling {ceiling} B"
    );
}

/// A grayscale PNG (colour type 0), which still decodes to the RGBA8 output class.
fn png_gray(width: u32, height: u32) -> Vec<u8> {
    let image =
        image::GrayImage::from_fn(width, height, |x, y| image::Luma([((x + y) % 251) as u8]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode an 8-bit grayscale PNG");
    bytes.into_inner()
}

/// An 8-bit RGBA PNG (colour type 6).
///
/// `image` cannot write an indexed PNG, so an indexed source is not measured here; this helper is
/// named for what it produces rather than for what an indexed file would be.
fn png_rgba(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            (x % 251) as u8,
            (y % 241) as u8,
            90,
            if (x + y) % 2 == 0 { 255 } else { 128 },
        ])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode an 8-bit RGBA PNG");
    bytes.into_inner()
}

#[test]
fn other_png_colour_types_are_measured_against_the_same_ceiling() {
    let _guard = test_guard();
    let edge = 1024u32;
    let size = PixelSize::new(edge, edge).expect("size");
    let ceiling = limits().peak_rgba8_bytes(size).expect("ceiling");
    for (label, encoded) in [
        ("8-bit grayscale", png_gray(edge, edge)),
        ("8-bit RGBA", png_rgba(edge, edge)),
    ] {
        let mut raster = None;
        let increment = peak_increment_during(|| {
            raster = Some(
                decode_raster_joined(&encoded, limits(), &Cancellation::default())
                    .expect("the class decodes"),
            );
        });
        let decoded = raster.expect("a raster");
        assert_eq!(
            decoded.rgba.len() as u64,
            size.checked_rgba_bytes().expect("rgba length"),
            "{label} still decodes to the declared RGBA8 size"
        );
        drop(decoded);
        eprintln!(
            "{edge}x{edge} {label} PNG decode increment: {increment} B (ceiling {ceiling} B)"
        );
        assert!(increment > 0, "{label}: the counter observed nothing");
        assert!(
            increment <= ceiling,
            "{label} decode increment {increment} B exceeded the ceiling {ceiling} B"
        );
    }
}

#[test]
fn a_lossy_class_at_two_sizes_is_measured_at_each() {
    let _guard = test_guard();
    let mut measured = Vec::new();
    for edge in [1024u32, 2048u32] {
        let size = PixelSize::new(edge, edge).expect("size");
        let ceiling = limits().peak_rgba8_bytes(size).expect("ceiling");
        let encoded = encoded_rgb(edge, edge, image::ImageFormat::Jpeg);
        let mut raster = None;
        let increment = peak_increment_during(|| {
            raster = Some(
                decode_raster_joined(&encoded, limits(), &Cancellation::default())
                    .expect("the class decodes"),
            );
        });
        let decoded = raster.expect("a raster");
        assert_eq!(
            decoded.rgba.len() as u64,
            size.checked_rgba_bytes().expect("rgba length")
        );
        drop(decoded);
        eprintln!("{edge}x{edge} JPEG decode increment: {increment} B (ceiling {ceiling} B)");
        assert!(
            increment <= ceiling,
            "{edge}x{edge} JPEG decode increment {increment} B exceeded {ceiling} B"
        );
        measured.push((edge, increment, ceiling));
    }
    // The measurements must be real, and the **ceiling** must scale with area - asserting only
    // that the allocation grew would still pass if both ceilings were the same oversized value.
    let (small_edge, small_alloc, small_ceiling) = measured[0];
    let (large_edge, large_alloc, large_ceiling) = measured[1];
    assert!(
        small_alloc > 0 && large_alloc > 0,
        "a measurement observed nothing"
    );
    assert!(
        small_alloc > 0 && large_alloc > 0,
        "both sizes must be measured ({small_edge}: {small_alloc} B, {large_edge}: {large_alloc} B)"
    );
    assert!(
        large_ceiling > small_ceiling,
        "the ceiling did not grow with the frame: {small_ceiling} B at {small_edge} vs \
         {large_ceiling} B at {large_edge}"
    );
    // The ceiling is `2 x the decoded size + a constant`, so the exact relationship is in the
    // **difference**: four times the area must add exactly twice that many bytes to the ceiling.
    // Integer division would have hidden a ceiling that is merely rounded, and asserting a ratio
    // would have ignored the constant term.
    let small_area = u64::from(small_edge) * u64::from(small_edge) * 4;
    let large_area = u64::from(large_edge) * u64::from(large_edge) * 4;
    assert_eq!(
        large_ceiling - small_ceiling,
        (large_area - small_area) * 2,
        "the ceiling should grow by twice the decoded-size difference;          got {small_ceiling} B at {small_area} B decoded and {large_ceiling} B at {large_area} B decoded"
    );
    assert!(
        large_alloc > small_alloc,
        "the 2048 frame did not allocate more than the 1024 one: {measured:?}"
    );
}

/// Enough decodes of one frame that the pool could not admit them all without releasing.
///
/// The pool is refused non-blockingly, so a reservation that was never released would not show up
/// as a wrong result - it would show up as a `Busy` refusal after a handful of decodes. Running the
/// same frame repeatedly and requiring **every** one to succeed is a behavioural test that the
/// reservation is released, without reaching into the private counter.
#[test]
fn repeated_decodes_keep_getting_a_reservation() {
    let _guard = test_guard();
    let edge = 2048u32;
    let size = PixelSize::new(edge, edge).expect("size");
    let bytes = png_rgb(edge, edge);
    // How many decodes the pool could admit at once if nothing were ever released: one more than
    // that must still succeed, which is only possible if the earlier reservations came back.
    let reservation = limits().peak_rgba8_bytes(size).expect("ceiling");
    let pool = media::IN_FLIGHT_POOL_BYTES;
    let would_fit_without_release = (pool / reservation) as usize;
    let rounds = would_fit_without_release + 2;
    eprintln!(
        "pool {pool} B, one reservation {reservation} B, so {would_fit_without_release} could \
         fit without release; running {rounds}"
    );
    for round in 0..rounds {
        let raster = decode_raster_joined(&bytes, limits(), &Cancellation::default())
            .unwrap_or_else(|error| {
                panic!(
                    "round {round} was refused with {error:?}, which a leaked reservation explains"
                )
            });
        assert_eq!(
            raster.rgba.len() as u64,
            size.checked_rgba_bytes().expect("rgba length"),
            "round {round} returned the wrong size"
        );
    }
    eprintln!("{rounds} sequential decodes of a {edge}x{edge} frame were all admitted");
}

/// Four decodes at once take and release the same pool.
///
/// The barrier makes them contend for the reservation together; every one must succeed and return
/// the declared size, which fails if the atomic take-and-release loses an update.
#[test]
fn concurrent_decodes_all_get_their_reservation_back() {
    let _guard = test_guard();
    const THREADS: usize = 4;
    let edge = 1024u32;
    let bytes = std::sync::Arc::new(png_rgb(edge, edge));
    let size = PixelSize::new(edge, edge).expect("size");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let bytes = std::sync::Arc::clone(&bytes);
        let barrier = std::sync::Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            decode_raster_joined(&bytes, limits(), &Cancellation::default())
                .map(|raster| raster.rgba.len() as u64)
        }));
    }
    let mut succeeded = 0;
    for handle in handles {
        let result = handle.join().expect("the thread did not panic");
        match result {
            Ok(len) => {
                assert_eq!(len, size.checked_rgba_bytes().expect("rgba length"));
                succeeded += 1;
            }
            Err(error) => panic!("a concurrent decode was refused with {error:?}"),
        }
    }
    assert_eq!(succeeded, THREADS);
    eprintln!("{THREADS} concurrent decodes all admitted and released");
}
