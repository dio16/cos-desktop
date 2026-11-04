// A-06: is a second decode of the same bytes identical to the first?
//
// §19.244 recorded a Gate failure in which a test compared the digest of one decode against the
// digest of another decode of the same encoded bytes and found them different, and recorded that
// the cause was **not** identified. This binary tries to answer the question directly rather than
// leaving it as a story: it decodes the same bytes repeatedly, sequentially and in parallel, and
// compares the encoded input, the decoded RGBA8 bytes and the digest each time.

use image::{DynamicImage, ImageFormat, RgbImage};
use media::{Cancellation, DecodeBudget, DecodeLimits, decode_raster};
use sha2::{Digest, Sha256};
use std::io::Cursor;

/// SHA-256 of a byte slice, computed the same way the upload boundary computes it.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn limits() -> DecodeLimits {
    DecodeBudget::ratified_default()
        .limits()
        .expect("ratified budget resolves")
}

fn png(bytes_width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(bytes_width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])
    });
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut out, ImageFormat::Png)
        .expect("encode an 8-bit RGB PNG");
    out.into_inner()
}

/// Decode once and return the RGBA8 bytes, so two runs can be compared byte for byte.
fn decode_pixels(encoded: &[u8]) -> Vec<u8> {
    decode_raster(encoded, limits(), &Cancellation::default())
        .expect("decode")
        .rgba
}

#[test]
fn repeated_decodes_of_the_same_bytes_are_identical() {
    // 512x512 keeps the test quick; the property under test does not depend on size.
    let encoded = png(512, 512);
    let first = decode_pixels(&encoded);
    assert_eq!(first.len(), 512 * 512 * 4, "the raster is complete");

    for run in 1..5 {
        let again = decode_pixels(&encoded);
        assert_eq!(
            again.len(),
            first.len(),
            "run {run}: a repeated decode produced a different length"
        );
        assert_eq!(
            again, first,
            "run {run}: a repeated decode produced different bytes"
        );
    }
}

#[test]
fn concurrent_decodes_of_the_same_bytes_are_identical() {
    // The Gate failure happened under a parallel runner, so this is the shape to test: several
    // decodes of the same bytes running at once must still agree with each other and with the
    // sequential result.
    let encoded = std::sync::Arc::new(png(512, 512));
    let expected = decode_pixels(&encoded);
    let mut handles = Vec::new();
    for _ in 0..4 {
        let encoded = std::sync::Arc::clone(&encoded);
        handles.push(std::thread::spawn(move || decode_pixels(&encoded)));
    }
    for (index, handle) in handles.into_iter().enumerate() {
        let produced = handle.join().expect("decode thread");
        assert_eq!(
            produced, expected,
            "concurrent decode {index} disagreed with the sequential result"
        );
    }
}

#[test]
fn the_row_decoder_and_the_generic_decoder_agree_on_the_pixels() {
    // `decode_raster` sends a non-interlaced PNG through the row decoder. Comparing that against
    // the standalone row decoder only proves the wiring; comparing two decodes that take the same
    // path proves the property the failed test assumed.
    let encoded = png(256, 256);
    let through_raster = decode_pixels(&encoded);
    let through_rows = media::decode_png_rows(&encoded, limits(), &Cancellation::default())
        .expect("row decode")
        .rgba;
    assert_eq!(
        through_raster, through_rows,
        "the entry point and the row decoder must produce the same pixels"
    );
}

#[test]
fn the_approved_class_size_is_also_deterministic() {
    // The Gate failure involved a 2048x2048 image, so repeat the comparison at that size rather
    // than generalising from 512x512.
    let encoded = png(2048, 2048);
    let first = decode_pixels(&encoded);
    assert_eq!(first.len(), 2048 * 2048 * 4);
    let second = decode_pixels(&encoded);
    assert_eq!(
        second, first,
        "two decodes of the approved class size produced different bytes"
    );
}

#[test]
fn the_encoded_and_decoded_digests_repeat_exactly() {
    // The Gate failure compared digests rather than bytes, so compare digests directly: one of the
    // encoded input and one of the decoded output, sequentially and concurrently. If these repeat,
    // the digest computation is not the part that differed; if they do not, this is where it is.
    let encoded = png(512, 512);
    let encoded_digest = sha256(&encoded);
    let decoded_digest = sha256(&decode_pixels(&encoded));

    let encoded_again = png(512, 512);
    assert_eq!(
        sha256(&encoded_again),
        encoded_digest,
        "encoding the same image twice must produce the same bytes"
    );

    for run in 1..4 {
        assert_eq!(
            sha256(&decode_pixels(&encoded)),
            decoded_digest,
            "run {run}: the decoded digest changed"
        );
        assert_eq!(
            sha256(&encoded),
            encoded_digest,
            "run {run}: the encoded digest changed"
        );
    }

    // Now the same comparison with the decodes running at once.
    let encoded = std::sync::Arc::new(encoded);
    let mut handles = Vec::new();
    for _ in 0..4 {
        let encoded = std::sync::Arc::clone(&encoded);
        handles.push(std::thread::spawn(move || {
            (sha256(&encoded), sha256(&decode_pixels(&encoded)))
        }));
    }
    for (index, handle) in handles.into_iter().enumerate() {
        let (encoded_seen, decoded_seen) = handle.join().expect("decode thread");
        assert_eq!(encoded_seen, encoded_digest, "thread {index}: input digest");
        assert_eq!(
            decoded_seen, decoded_digest,
            "thread {index}: output digest"
        );
    }
}
