// MP-IMG2: bounded cancellation of a *running* PNG decode.
//
// §19.118 recorded bounded cancellation of a running decoder as HOLD because the
// `image` backend decodes in one uninterruptible call. These tests cover the narrower
// path that does exist: a PNG decode that reads **one row at a time** and checks the
// caller's `Cancellation` between rows, so a cancelled decode stops early, returns a
// typed `Cancelled`, and publishes nothing.
//
// Scope, stated plainly: this covers **PNG only**. JPEG, WebP and GIF still go through
// the uninterruptible backend and remain HOLD for bounded cancellation.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, DecodeLimits, MediaError, PixelSize};
use std::io::Cursor;

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
fn a_row_wise_png_decode_returns_a_complete_raster() {
    let bytes = png_rgb(256, 128);
    let raster = media::decode_png_rows(&bytes, limits(), &Cancellation::default())
        .expect("the PNG decodes row by row");
    assert_eq!(raster.size, PixelSize::new(256, 128).expect("size"));
    assert_eq!(
        raster.rgba.len() as u64,
        raster.size.checked_rgba_bytes().expect("rgba length"),
        "the row-wise path must produce a complete raster"
    );
}

#[test]
fn cancellation_between_rows_stops_the_decode_early() {
    let bytes = png_rgb(256, 512);
    let cancel = Cancellation::default();
    // Cancel from inside the decoder's per-row hook, after a known number of rows.
    let rows_seen = std::cell::Cell::new(0u32);
    let result = media::decode_png_rows_with_hook(&bytes, limits(), &cancel, &mut |rows| {
        rows_seen.set(rows);
        if rows == 16 {
            cancel.cancel();
        }
    });
    assert_eq!(
        result.err(),
        Some(MediaError::Cancelled),
        "a decode cancelled between rows must be typed Cancelled"
    );
    assert_eq!(
        rows_seen.get(),
        16,
        "the decode must stop at the row where cancellation was observed, not run to completion"
    );
}

#[test]
fn a_cancelled_row_wise_decode_publishes_nothing() {
    let bytes = png_rgb(64, 64);
    let cancel = Cancellation::default();
    cancel.cancel();
    match media::decode_png_rows(&bytes, limits(), &cancel) {
        Err(error) => assert_eq!(error, MediaError::Cancelled),
        Ok(_) => panic!("a cancelled decode must not return a raster"),
    }
}

#[test]
fn the_row_wise_path_refuses_an_over_limit_input_before_decoding() {
    let bytes = png_rgb(20_000, 1);
    match media::decode_png_rows(&bytes, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::TooLarge),
        Ok(_) => panic!("a refused input must not publish a raster"),
    }
}

#[test]
fn a_non_png_input_is_refused_by_the_row_wise_path() {
    let image = RgbImage::from_fn(32, 32, |x, y| image::Rgb([(x % 7) as u8, (y % 5) as u8, 3]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Jpeg)
        .expect("encode a JPEG");
    let jpeg = bytes.into_inner();
    match media::decode_png_rows(&jpeg, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::UnsupportedContainer),
        Ok(_) => panic!("the row-wise path is PNG-only"),
    }
}

/// A greyscale 8-bit PNG: the decoder returns grey (or grey+alpha), not RGBA8, so this checks
/// the conversion rather than assuming four channels arrive.
fn png_gray(width: u32, height: u32) -> Vec<u8> {
    let image =
        image::GrayImage::from_fn(width, height, |x, y| image::Luma([((x + y) % 256) as u8]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode an 8-bit grey PNG");
    bytes.into_inner()
}

#[test]
fn a_greyscale_png_is_widened_to_rgba8() {
    let bytes = png_gray(64, 32);
    let raster = media::decode_png_rows(&bytes, limits(), &Cancellation::default())
        .expect("a grey PNG decodes through the row path");
    assert_eq!(raster.size, PixelSize::new(64, 32).expect("size"));
    assert_eq!(
        raster.rgba.len() as u64,
        raster.size.checked_rgba_bytes().expect("rgba length"),
        "the buffer must be RGBA8, not grey"
    );
    // Every pixel is opaque, and the channels come from the grey sample.
    for pixel in raster.rgba.as_chunks::<4>().0 {
        assert_eq!(pixel[3], 0xff, "converted pixels are opaque");
        assert_eq!(pixel[0], pixel[1], "grey is replicated across R, G and B");
        assert_eq!(pixel[1], pixel[2], "grey is replicated across R, G and B");
    }
    // And the value really came from the image, not from a zero fill.
    assert!(
        raster
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[0] != 0),
        "a converted raster must carry the image's samples"
    );
}

/// CRC-32 (ISO-HDLC) as PNG chunks use it, so the synthesized chunks are well-formed.
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

/// Append a well-formed `acTL` chunk immediately after `IHDR`, which is where an APNG puts it.
fn with_actl(png: &[u8]) -> Vec<u8> {
    // Signature (8) + IHDR chunk (4 len + 4 type + 13 data + 4 crc) = 33 bytes.
    const AFTER_IHDR: usize = 33;
    let mut out = Vec::with_capacity(png.len() + 20);
    out.extend_from_slice(&png[..AFTER_IHDR]);
    let mut payload = Vec::new();
    payload.extend_from_slice(b"acTL");
    payload.extend_from_slice(&1u32.to_be_bytes()); // num_frames
    payload.extend_from_slice(&0u32.to_be_bytes()); // num_plays: infinite
    // Chunk length counts the data only: acTL carries num_frames and num_plays, 8 bytes.
    out.extend_from_slice(&8u32.to_be_bytes());
    out.extend_from_slice(&payload);
    let crc = crc32(&payload);
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&png[AFTER_IHDR..]);
    out
}

#[test]
fn a_png_carrying_actl_is_unavailable_through_the_row_path_too() {
    // Scope of this test: a PNG that carries a well-formed `acTL` chunk is refused as
    // `Unavailable` by the row path, as it already is by `decode_raster`. It does **not**
    // claim a complete APNG animation (that would also need `fcTL` frames); it pins the
    // still-image rule this path inherits.
    const AFTER_IHDR: usize = 33;
    let bytes = with_actl(&png_rgb(32, 32));

    // Verify the synthesized chunk independently before relying on it: length counts the
    // data only, and the CRC covers the chunk type together with that data.
    let length = u32::from_be_bytes(
        bytes[AFTER_IHDR..AFTER_IHDR + 4]
            .try_into()
            .expect("length"),
    );
    assert_eq!(length, 8, "acTL data is num_frames plus num_plays");
    assert_eq!(&bytes[AFTER_IHDR + 4..AFTER_IHDR + 8], b"acTL");
    let data_end = AFTER_IHDR + 4 + 4 + length as usize;
    let stored = u32::from_be_bytes(bytes[data_end..data_end + 4].try_into().expect("crc"));
    assert_eq!(
        stored,
        crc32(&bytes[AFTER_IHDR + 4..data_end]),
        "the chunk CRC must be the one a reader computes"
    );

    match media::decode_png_rows(&bytes, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::Unavailable),
        Ok(_) => panic!("a PNG carrying acTL must not produce a still raster"),
    }
}

#[test]
fn an_interlaced_png_is_refused_as_unsupported() {
    // The row loop needs full-width rows, which Adam7 does not provide; the contract is
    // non-interlaced only, so an interlaced input must be a typed refusal rather than a
    // mis-decoded raster or a generic metadata error.
    let bytes = {
        let image = RgbImage::from_fn(17, 9, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 7])
        });
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 17, 9);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let raw: Vec<u8> = image.as_raw().clone();
            writer.write_image_data(&raw).expect("png data");
        }
        out
    };
    // The encoder writes non-interlaced data; flip the IHDR interlace byte to declare Adam7,
    // fix the IHDR CRC, and require the typed refusal.
    let mut bytes = bytes;
    let idat = bytes.windows(4).position(|w| w == b"IHDR").expect("IHDR");
    let interlace_at = idat + 4 + 12; // type(4) + data offset 12 (width..filter method)
    bytes[interlace_at] = 1;
    let crc_start = idat; // PNG hashes the chunk type together with its data
    let ihdr_end = crc_start + 4 + 13; // chunk type + IHDR data
    let crc = crc32(&bytes[crc_start..ihdr_end]);
    bytes[ihdr_end..ihdr_end + 4].copy_from_slice(&crc.to_be_bytes());
    match media::decode_png_rows(&bytes, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(error, MediaError::UnsupportedContainer),
        Ok(_) => panic!("an interlaced PNG is outside this path's contract"),
    }
}

#[test]
fn the_product_path_uses_the_row_decoder_and_keeps_interlaced_pngs_working() {
    // Non-interlaced PNG now goes through the row decoder inside `decode_raster`; interlaced
    // PNGs and every other container keep the generic path, so no class loses support.
    let plain = png_rgb(96, 64);
    let raster = media::decode_raster(&plain, limits(), &Cancellation::default())
        .expect("the product path decodes a plain PNG");
    assert_eq!(raster.size, PixelSize::new(96, 64).expect("size"));
    assert_eq!(
        raster.rgba.len() as u64,
        raster.size.checked_rgba_bytes().expect("rgba length")
    );

    // An interlaced declaration must still be decoded by the generic decoder rather than
    // refused, which is the compatibility this wiring has to preserve.
    let mut interlaced = plain.clone();
    let ihdr = interlaced
        .windows(4)
        .position(|w| w == b"IHDR")
        .expect("IHDR");
    interlaced[ihdr + 4 + 12] = 1;
    let crc_start = ihdr;
    let ihdr_end = crc_start + 4 + 13;
    let crc = crc32(&interlaced[crc_start..ihdr_end]);
    interlaced[ihdr_end..ihdr_end + 4].copy_from_slice(&crc.to_be_bytes());
    // The payload is genuinely non-interlaced, so the header now lies about the layout and the
    // generic decoder must reject it as metadata rather than returning a wrong raster. What is
    // asserted here is only that the product path does not route it through the row decoder,
    // which would answer `UnsupportedContainer`.
    match media::decode_raster(&interlaced, limits(), &Cancellation::default()) {
        Err(error) => assert_eq!(
            error,
            MediaError::InvalidMetadata,
            "an interlaced declaration must reach the generic decoder"
        ),
        Ok(_) => panic!("a header that lies about interlace must not produce a raster"),
    }
}
