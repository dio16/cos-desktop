//! MP-IMG2/IMG3 bounded raster decode on the admitted profile.
//!
//! The decoder accepts only the four admitted containers, refuses anything whose
//! header dimensions or decoded size exceed the caller's [`DecodeLimits`], and
//! returns RGBA8. It owns no GPU object, performs no acquisition, and never
//! substitutes a preview or placeholder for a refused input.

use image::{ImageFormat, ImageReader};
use std::io::Cursor;

use crate::{Cancellation, DecodeLimits, DecodeRequest, MediaError, PixelSize, admit_decode};

/// Containers this backend decodes. Adding another is a new owner decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptedFormat {
    Png,
    Jpeg,
    WebP,
    Gif,
}

impl AcceptedFormat {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MediaError> {
        match image::guess_format(bytes).map_err(|_| MediaError::UnsupportedContainer)? {
            ImageFormat::Png => Ok(Self::Png),
            ImageFormat::Jpeg => Ok(Self::Jpeg),
            ImageFormat::WebP => Ok(Self::WebP),
            ImageFormat::Gif => Ok(Self::Gif),
            _ => Err(MediaError::UnsupportedContainer),
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::WebP => ImageFormat::WebP,
            Self::Gif => ImageFormat::Gif,
        }
    }
}

/// One cropped-out decoded image: RGBA8 pixels plus the authoritative size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedRaster {
    pub size: PixelSize,
    pub format: AcceptedFormat,
    pub rgba: Vec<u8>,
}

/// Decode one encoded image under the caller's limits.
///
/// The header is read first so an oversized image is refused before the pixel
/// buffer is allocated; the decoded length is checked again afterwards in case a
/// decoder reports different geometry than its header.
pub fn decode_raster(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
) -> Result<DecodedRaster, MediaError> {
    decode_raster_inner(bytes, limits, cancel, None, None, None)
}

/// Test seam: run a decode that cancels at a chosen checkpoint, so the cancellation
/// contract can be exercised without sleeping. Not compiled into production builds.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum HookPhase {
    /// After admission and before the decoder call.
    AfterAdmission,
    /// After RGBA8 conversion and before the final cancellation check.
    AfterConversion,
}

#[cfg(test)]
pub(crate) fn decode_raster_with_hook(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
    phase: HookPhase,
    hook: &mut dyn FnMut(),
) -> Result<DecodedRaster, MediaError> {
    match phase {
        HookPhase::AfterAdmission => {
            decode_raster_inner(bytes, limits, cancel, Some(hook), None, None)
        }
        HookPhase::AfterConversion => {
            decode_raster_inner(bytes, limits, cancel, None, Some(hook), None)
        }
    }
}

fn decode_raster_inner(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
    before_decode: Option<&mut dyn FnMut()>,
    before_publish: Option<&mut dyn FnMut()>,
    png_row_hook: Option<&mut dyn FnMut(u32)>,
) -> Result<DecodedRaster, MediaError> {
    cancel.check()?;
    let format = AcceptedFormat::from_bytes(bytes)?;
    let (width, height) =
        image::ImageReader::with_format(Cursor::new(bytes), format.image_format())
            .into_dimensions()
            .map_err(|_| MediaError::InvalidMetadata)?;
    ensure_still(bytes, format)?;
    let size = PixelSize::new(width, height)?;
    let source_depth = source_bit_depth(bytes, format)?;
    let admission = admit_raster(size, source_depth, limits, cancel)?;
    let decoded_bytes = admission.decoded_bytes;
    // Reserve the declared ceiling process-wide for the duration of this decode; the
    // reservation is released when the guard drops. Refusal is non-blocking (`Busy`).
    let _reservation = crate::decode::reserve_in_flight(admission.peak_bytes)?;
    if let Some(hook) = before_decode {
        hook();
    }
    cancel.check()?;
    // A non-interlaced PNG goes through the row decoder, which can observe cancellation
    // between rows instead of only before and after one uninterruptible call. Interlaced
    // PNGs and every other container keep the generic path, so no input class loses support.
    if format == AcceptedFormat::Png && !png_is_interlaced(bytes) {
        // The reservation was taken above for this decode; the row decoder must not take a
        // second one for the same image.
        let mut no_hook = |_rows: u32| {};
        let row_hook: &mut dyn FnMut(u32) = match png_row_hook {
            Some(hook) => hook,
            None => &mut no_hook,
        };
        let raster = decode_png_rows_inner(bytes, limits, cancel, row_hook, false)?;
        // Keep the publish seam meaningful on this path: a cancellation arriving after the
        // pixels are ready must still prevent publication, exactly as on the generic path.
        if let Some(hook) = before_publish {
            hook();
        }
        cancel.check()?;
        return Ok(raster);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.image_format());
    reader.limits(decoder_limits(limits));
    let decoded = reader.decode().map_err(|_| MediaError::InvalidMetadata)?;
    cancel.check()?;
    // `into_rgba8` reuses an already-RGBA8 buffer and converts otherwise; the
    // result is checked again so a decoder reporting different geometry than its
    // header cannot exceed the admitted size.
    let rgba = decoded.into_rgba8().into_raw();
    if let Some(hook) = before_publish {
        hook();
    }
    cancel.check()?;
    if rgba.len() as u64 != decoded_bytes || rgba.len() as u64 > limits.max_decoded_bytes {
        return Err(MediaError::TooLarge);
    }
    Ok(DecodedRaster { size, format, rgba })
}

/// Refuse input classes this backend cannot render as one still raster.
///
/// Animated containers are a capability this backend does not implement, so they are
/// reported as [`MediaError::Unavailable`] instead of silently decoding the first
/// frame. The scan is bounded by the input length and stops at the first image data.
fn ensure_still(bytes: &[u8], format: AcceptedFormat) -> Result<(), MediaError> {
    match format {
        AcceptedFormat::Png => ensure_still_png(bytes),
        AcceptedFormat::WebP => ensure_still_webp(bytes),
        AcceptedFormat::Gif => ensure_still_gif(bytes),
        AcceptedFormat::Jpeg => Ok(()),
    }
}

/// Whether the PNG header declares Adam7 interlacing. Reads only the IHDR chunk, so it costs
/// nothing next to a decode and never depends on the pixel payload.
fn png_is_interlaced(bytes: &[u8]) -> bool {
    // Signature (8) + length (4) + "IHDR" (4) + 13 data bytes; the interlace byte is last.
    const IHDR_TYPE: usize = 12;
    const INTERLACE_BYTE: usize = 28;
    if bytes.len() <= INTERLACE_BYTE {
        return false;
    }
    if &bytes[IHDR_TYPE..IHDR_TYPE + 4] != b"IHDR" {
        return false;
    }
    bytes[INTERLACE_BYTE] == 1
}

fn ensure_still_png(bytes: &[u8]) -> Result<(), MediaError> {
    let mut offset = 8usize;
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        if kind == b"acTL" {
            return Err(MediaError::Unavailable);
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return Ok(());
        }
        offset = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
            .ok_or(MediaError::Unavailable)?;
    }
    Ok(())
}

fn ensure_still_webp(bytes: &[u8]) -> Result<(), MediaError> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Ok(());
    }
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let kind = &bytes[offset..offset + 4];
        let length = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        if kind == b"ANIM" || kind == b"ANMF" {
            return Err(MediaError::Unavailable);
        }
        let padded = length
            .checked_add(length & 1)
            .ok_or(MediaError::Unavailable)?;
        offset = offset
            .checked_add(8)
            .and_then(|value| value.checked_add(padded))
            .ok_or(MediaError::Unavailable)?;
    }
    Ok(())
}

fn ensure_still_gif(bytes: &[u8]) -> Result<(), MediaError> {
    if bytes.len() < 13 || (&bytes[..6] != b"GIF87a" && &bytes[..6] != b"GIF89a") {
        return Ok(());
    }
    let packed = bytes[10];
    let mut offset = 13usize;
    if packed & 0x80 != 0 {
        let table = 3usize.wrapping_shl(u32::from(packed & 0x07) + 1);
        offset = offset.checked_add(table).ok_or(MediaError::Unavailable)?;
    }
    let mut images = 0usize;
    while offset < bytes.len() {
        match bytes[offset] {
            0x3b => break,
            0x21 => {
                // Extension: label byte, then size-prefixed sub-blocks.
                offset = offset.checked_add(2).ok_or(MediaError::Unavailable)?;
                loop {
                    let Some(&size) = bytes.get(offset) else {
                        return Ok(());
                    };
                    offset = offset.checked_add(1).ok_or(MediaError::Unavailable)?;
                    if size == 0 {
                        break;
                    }
                    offset = offset
                        .checked_add(usize::from(size))
                        .ok_or(MediaError::Unavailable)?;
                }
            }
            0x2c => {
                images += 1;
                if images > 1 {
                    return Err(MediaError::Unavailable);
                }
                let Some(packed) = bytes.get(offset + 9) else {
                    return Ok(());
                };
                offset = offset.checked_add(10).ok_or(MediaError::Unavailable)?;
                if packed & 0x80 != 0 {
                    let table = 3usize.wrapping_shl(u32::from(packed & 0x07) + 1);
                    offset = offset.checked_add(table).ok_or(MediaError::Unavailable)?;
                }
                // LZW minimum code size, then size-prefixed sub-blocks.
                offset = offset.checked_add(1).ok_or(MediaError::Unavailable)?;
                loop {
                    let Some(&size) = bytes.get(offset) else {
                        return Ok(());
                    };
                    offset = offset.checked_add(1).ok_or(MediaError::Unavailable)?;
                    if size == 0 {
                        break;
                    }
                    offset = offset
                        .checked_add(usize::from(size))
                        .ok_or(MediaError::Unavailable)?;
                }
            }
            _ => return Ok(()),
        }
    }
    Ok(())
}

/// Decode a PNG **one row at a time**, honouring `cancel` between rows.
///
/// This is the row-wise path for a *running* decode. The generic backend hands the whole image
/// to `image` in one uninterruptible call, so a cancellation there can only be observed before
/// or after it. Here the caller's cancellation is checked between rows, which lets a cancelled
/// decode stop early instead of finishing work nobody will use.
///
/// Which path a given input takes: [`decode_raster`] delegates **non-interlaced PNG** to this
/// decoder, and everything else - interlaced PNG, JPEG, WebP, GIF - to the generic backend.
///
/// Scope: **non-interlaced PNG only**. An Adam7 PNG is refused here as `UnsupportedContainer`
/// because `next_row` yields per-pass rows rather than full-width rows. [`decode_raster`] calls
/// this decoder for non-interlaced PNGs (and this entry point reserves for itself, while that
/// caller passes the reservation it already holds); interlaced PNGs and the other containers
/// go through the generic backend. Admission and the declared ceiling are the same either way;
/// a refused input never starts the row loop, and a cancelled one publishes nothing.
pub fn decode_png_rows(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
) -> Result<DecodedRaster, MediaError> {
    let mut hook = |_rows: u32| {};
    decode_png_rows_with_hook(bytes, limits, cancel, &mut hook)
}

/// Test seam: observes the row count and can cancel from inside the loop.
pub fn decode_png_rows_with_hook(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
    on_row: &mut dyn FnMut(u32),
) -> Result<DecodedRaster, MediaError> {
    decode_png_rows_inner(bytes, limits, cancel, on_row, true)
}

/// Row-wise PNG decode. `reserve` is false only for the product path, which already holds the
/// process-wide reservation for this decode: taking it twice would count one image's ceiling
/// against a finite pool twice and refuse input that fits.
fn decode_png_rows_inner(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
    on_row: &mut dyn FnMut(u32),
    reserve: bool,
) -> Result<DecodedRaster, MediaError> {
    cancel.check()?;
    if AcceptedFormat::from_bytes(bytes)? != AcceptedFormat::Png {
        return Err(MediaError::UnsupportedContainer);
    }
    // Same input contract as `decode_raster`: an animated PNG is `Unavailable`, never a still
    // frame quietly taken from an animation.
    ensure_still(bytes, AcceptedFormat::Png)?;
    let size = {
        let decoder = png::Decoder::new(Cursor::new(bytes));
        let reader = decoder
            .read_info()
            .map_err(|_| MediaError::InvalidMetadata)?;
        let info = reader.info();
        if info.interlaced {
            // Adam7 hands back rows of a pass, not full-width rows, so this loop cannot place
            // them. The contract is therefore "non-interlaced PNG only" and says so.
            return Err(MediaError::UnsupportedContainer);
        }
        PixelSize::new(info.width, info.height)?
    };
    let admission = admit_raster(
        size,
        source_bit_depth(bytes, AcceptedFormat::Png)?,
        limits,
        cancel,
    )?;
    let decoded_bytes = admission.decoded_bytes;
    // The product path already holds the reservation for this decode; taking it twice would
    // count one image's ceiling against the pool twice and refuse input that fits.
    let _reservation = if reserve {
        Some(crate::decode::reserve_in_flight(admission.peak_bytes)?)
    } else {
        None
    };

    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // Expand palette/low-bit-depth input and add an alpha channel so every row arrives as
    // RGBA8, the format this adapter returns.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder
        .read_info()
        .map_err(|_| MediaError::InvalidMetadata)?;
    // `EXPAND | ALPHA` widens to 8-bit and adds an alpha channel, but a grey image becomes
    // grey+alpha, not RGBA8. Read the actual output layout and convert per row, rather than
    // assuming four channels and zero-filling whatever does not arrive.
    let channels = match reader.output_color_type() {
        (png::ColorType::Rgba, png::BitDepth::Eight) => 4usize,
        (png::ColorType::Rgb, png::BitDepth::Eight) => 3,
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => 2,
        (png::ColorType::Grayscale, png::BitDepth::Eight) => 1,
        _ => return Err(MediaError::UnsupportedContainer),
    };
    let mut rgba = vec![0u8; decoded_bytes as usize];
    let mut offset = 0usize;
    let mut rows: u32 = 0;
    loop {
        cancel.check()?;
        let row = reader.next_row().map_err(|_| MediaError::InvalidMetadata)?;
        let Some(row) = row else { break };
        let data = row.data();
        let expect = channels
            .checked_mul(size.width() as usize)
            .ok_or(MediaError::TooLarge)?;
        if data.len() != expect {
            return Err(MediaError::InvalidMetadata);
        }
        let end = offset
            .checked_add(4 * (size.width() as usize))
            .ok_or(MediaError::TooLarge)?;
        if end > rgba.len() {
            return Err(MediaError::TooLarge);
        }
        for (index, pixel) in data.chunks_exact(channels).enumerate() {
            let target = offset + index * 4;
            let (r, g, b, a) = match channels {
                4 => (pixel[0], pixel[1], pixel[2], pixel[3]),
                3 => (pixel[0], pixel[1], pixel[2], 0xff),
                2 => (pixel[0], pixel[0], pixel[0], pixel[1]),
                _ => (pixel[0], pixel[0], pixel[0], 0xff),
            };
            rgba[target] = r;
            rgba[target + 1] = g;
            rgba[target + 2] = b;
            rgba[target + 3] = a;
        }
        offset = end;
        rows += 1;
        on_row(rows);
    }
    cancel.check()?;
    if rows != size.height() {
        return Err(MediaError::InvalidMetadata);
    }
    let expected = size
        .checked_rgba_bytes()
        .map_err(|_| MediaError::TooLarge)?;
    if rgba.len() as u64 != expected || rgba.len() as u64 != decoded_bytes {
        return Err(MediaError::TooLarge);
    }
    Ok(DecodedRaster {
        size,
        format: AcceptedFormat::Png,
        rgba,
    })
}

/// Product-path seam: decode exactly as [`decode_raster`] does, but report each PNG row and let
/// the caller cancel from inside the row loop. Used to prove that the product path - not just the
/// standalone row decoder - observes cancellation between rows.
pub fn decode_raster_with_row_hook(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
    on_row: &mut dyn FnMut(u32),
) -> Result<DecodedRaster, MediaError> {
    decode_raster_inner(bytes, limits, cancel, None, None, Some(on_row))
}

/// Decode on a scoped worker and join it before returning.
///
/// This guarantees **worker ownership**, not a completion-time bound: the worker is owned
/// by this call and always joined, so a cancelled or failed decode cannot leave a decode
/// thread behind. Cancellation is cooperative and is only observed at the bounded
/// checkpoints inside [`decode_raster`] (before metadata, before the decoder call, after
/// decode and after RGBA8 conversion). For a **non-interlaced PNG** the decode itself now runs
/// through [`decode_png_rows`], which checks cancellation between rows, so a cancellation is
/// observed inside the image rather than only at those checkpoints. For **interlaced PNG,
/// JPEG, WebP and GIF** the decoder call is still uninterruptible, so a cancellation arriving
/// inside it is only observed at the next checkpoint and the `join` may wait for that call to
/// return; bounded cancellation for those classes remains `NOT_VERIFIED` (plan §19.234-19.240). On `Cancelled` (or any refusal) no raster is
/// returned, so a caller cannot publish a partial artifact from this path.
pub fn decode_raster_joined(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
) -> Result<DecodedRaster, MediaError> {
    std::thread::scope(
        |scope| match scope.spawn(|| decode_raster(bytes, limits, cancel)).join() {
            Ok(result) => result,
            Err(_) => Err(MediaError::Unavailable),
        },
    )
}

/// Admit the RGBA8 output of one source image before the full-image decode and its
/// pixel-buffer allocation. Metadata parsing can allocate internally; this admission
/// does not claim to cover that.
///
/// The source depth is validated separately and the admission always describes
/// the **output** format (8-bit RGBA), so a 1-, 2- or 4-bit PNG cannot pass a
/// budget that is smaller than its RGBA8 result. One conversion buffer of the
/// same size is declared as scratch, so all concurrent residency is admitted
/// under the caller's ceilings.
fn admit_raster(
    size: PixelSize,
    source_depth: u8,
    limits: DecodeLimits,
    cancel: &Cancellation,
) -> Result<crate::DecodeAdmission, MediaError> {
    if source_depth == 0 || source_depth > crate::MAX_SUPPORTED_BIT_DEPTH {
        return Err(MediaError::InvalidMetadata);
    }
    // A depth this backend cannot render (above 8 bits per channel) or one that exceeds
    // the caller's ceiling is an unavailable class, never silently rounded down.
    if source_depth > 8 || source_depth > limits.max_bit_depth {
        return Err(MediaError::Unavailable);
    }
    let out_bytes = size.checked_rgba_bytes()?;
    admit_decode(
        DecodeRequest {
            size,
            bit_depth: 8,
            channels: 4,
            declared_scratch_bytes: out_bytes,
        },
        limits,
        cancel,
    )
}

/// Bit depth of the admitted source container.
///
/// Only PNG can carry more than 8 bits among the admitted containers; JPEG, WebP
/// and GIF are 8-bit or paletted. The PNG IHDR byte is read directly instead of
/// trusting the decoder's colour type.
fn source_bit_depth(bytes: &[u8], format: AcceptedFormat) -> Result<u8, MediaError> {
    match format {
        AcceptedFormat::Png => {
            const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            if bytes.len() < 25 || bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
                return Err(MediaError::InvalidMetadata);
            }
            Ok(bytes[24])
        }
        AcceptedFormat::Jpeg | AcceptedFormat::WebP | AcceptedFormat::Gif => Ok(8),
    }
}

fn decoder_limits(limits: DecodeLimits) -> image::Limits {
    let mut decoder = image::Limits::default();
    decoder.max_image_width = Some(limits.max_dimension);
    decoder.max_image_height = Some(limits.max_dimension);
    decoder.max_alloc = Some(limits.max_decoded_bytes);
    decoder
}

#[cfg(test)]
mod tests {
    use super::{AcceptedFormat, decode_raster};
    use crate::{Cancellation, DecodeBudget, DecodeLimits, PixelSize};
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use std::io::Cursor;

    fn limits() -> DecodeLimits {
        DecodeBudget::ratified_default()
            .limits()
            .expect("ratified budget resolves")
    }

    fn encode(format: ImageFormat) -> Vec<u8> {
        let image = RgbaImage::from_fn(4, 3, |x, y| {
            image::Rgba([(x * 40) as u8, (y * 30) as u8, 90, 255])
        });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, format)
            .expect("encode");
        bytes.into_inner()
    }

    #[test]
    fn decodes_each_admitted_container_to_rgba8() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let cancel = Cancellation::default();
        for (format, expected) in [
            (ImageFormat::Png, AcceptedFormat::Png),
            (ImageFormat::Jpeg, AcceptedFormat::Jpeg),
            (ImageFormat::WebP, AcceptedFormat::WebP),
            (ImageFormat::Gif, AcceptedFormat::Gif),
        ] {
            let bytes = encode(format);
            let decoded = decode_raster(&bytes, limits(), &cancel).expect("decode");
            assert_eq!(decoded.format, expected);
            assert_eq!(decoded.size, PixelSize::new(4, 3).expect("size"));
            assert_eq!(
                decoded.rgba.len() as u64,
                decoded.size.checked_rgba_bytes().expect("rgba length")
            );
        }
    }

    #[test]
    fn refuses_a_corrupt_payload() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let mut bytes = encode(ImageFormat::Png);
        let tail = bytes.len() - 8;
        bytes.truncate(tail);
        let error = decode_raster(&bytes, limits(), &Cancellation::default());
        assert!(error.is_err(), "a truncated payload must be refused");
    }

    #[test]
    fn admission_always_uses_the_rgba8_output_size() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let size = PixelSize::new(4, 3).expect("size");
        let base = limits();
        // 12 px -> 48 bytes of RGBA8. A 1-bit source would estimate 1.5 bytes, so a
        // 24-byte budget only passes when the output format is what is admitted.
        let tight = DecodeLimits {
            max_decoded_bytes: 24,
            ..base
        };
        let error = super::admit_raster(size, 1, tight, &Cancellation::default());
        assert_eq!(error.unwrap_err(), crate::MediaError::TooLarge);
        let admitted = super::admit_raster(size, 1, base, &Cancellation::default())
            .expect("output-sized admission");
        assert_eq!(admitted.decoded_bytes, 48);
        assert_eq!(admitted.scratch_bytes, 48);
    }

    #[test]
    fn a_sixteen_bit_png_is_unavailable_under_an_eight_bit_limit() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let image = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_fn(2, 2, |x, y| {
            image::Rgba([x as u16 * 1000, y as u16 * 1000, 3000, u16::MAX])
        });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba16(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("encode 16-bit png");
        let narrow = DecodeLimits {
            max_bit_depth: 8,
            ..limits()
        };
        let error = decode_raster(&bytes.into_inner(), narrow, &Cancellation::default());
        assert_eq!(error.unwrap_err(), crate::MediaError::Unavailable);
    }

    #[test]
    fn refuses_an_unadmitted_container() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        // A minimal BMP header: recognised by the sniffer, not admitted by policy.
        let mut bmp = vec![0u8; 54];
        bmp[0] = b'B';
        bmp[1] = b'M';
        bmp[2..6].copy_from_slice(&54u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&1i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&1i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        let error = decode_raster(&bmp, limits(), &Cancellation::default());
        assert_eq!(error.unwrap_err(), crate::MediaError::UnsupportedContainer);
    }

    #[test]
    fn refuses_dimensions_beyond_the_limits_before_decoding() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let bytes = encode(ImageFormat::Png);
        let narrow = DecodeLimits {
            max_dimension: 2,
            ..limits()
        };
        let error = decode_raster(&bytes, narrow, &Cancellation::default());
        assert_eq!(error.unwrap_err(), crate::MediaError::TooLarge);
    }

    #[test]
    fn refuses_a_cancelled_decode() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let cancel = Cancellation::default();
        cancel.cancel();
        let error = decode_raster(&encode(ImageFormat::Png), limits(), &cancel);
        assert_eq!(error.unwrap_err(), crate::MediaError::Cancelled);
    }

    #[test]
    fn an_animated_png_is_unavailable() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        // Insert an acTL chunk right after IHDR; the scan must stop there instead of
        // silently decoding the first frame.
        let mut bytes = encode(ImageFormat::Png);
        let ihdr_end = 8 + 25;
        let mut actl = Vec::new();
        actl.extend_from_slice(&8u32.to_be_bytes());
        actl.extend_from_slice(b"acTL");
        actl.extend_from_slice(&2u32.to_be_bytes());
        actl.extend_from_slice(&0u32.to_be_bytes());
        actl.extend_from_slice(&0u32.to_be_bytes());
        bytes.splice(ihdr_end..ihdr_end, actl);
        let error = decode_raster(&bytes, limits(), &Cancellation::default());
        assert_eq!(error.unwrap_err(), crate::MediaError::Unavailable);
    }

    fn gif_with_images(images: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GIF89a");
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.push(0x80); // global colour table, size 0 (2 entries)
        bytes.push(0);
        bytes.push(0);
        bytes.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        for _ in 0..images {
            bytes.push(0x2c); // image descriptor
            bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0, 0]);
            bytes.push(2); // LZW minimum code size
            bytes.push(2);
            bytes.extend_from_slice(&[0x44, 0x01]);
            bytes.push(0); // sub-block terminator
        }
        bytes.push(0x3b);
        bytes
    }

    #[test]
    fn an_animated_gif_is_unavailable() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let error = decode_raster(&gif_with_images(2), limits(), &Cancellation::default());
        assert_eq!(error.unwrap_err(), crate::MediaError::Unavailable);
        // A single-frame GIF still reaches the decoder and is not misclassified.
        let single =
            super::decode_raster_joined(&gif_with_images(1), limits(), &Cancellation::default());
        assert!(
            single.is_ok(),
            "a one-image GIF must stay decodable: {:?}",
            single.err()
        );
    }

    #[test]
    fn a_cancelled_joined_decode_leaves_no_partial_raster() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let cancel = Cancellation::default();
        cancel.cancel();
        let mut published: Option<super::DecodedRaster> = None;
        if let Ok(raster) =
            super::decode_raster_joined(&encode(ImageFormat::Png), limits(), &cancel)
        {
            published = Some(raster);
        }
        assert!(
            published.is_none(),
            "a cancelled joined decode must not produce a raster"
        );
    }

    #[test]
    fn a_joined_decode_returns_a_complete_worker_result() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let raster = super::decode_raster_joined(
            &encode(ImageFormat::Png),
            limits(),
            &Cancellation::default(),
        )
        .expect("decode");
        assert_eq!(
            raster.rgba.len() as u64,
            raster.size.checked_rgba_bytes().expect("rgba length")
        );
    }

    #[test]
    fn a_cancel_at_the_pre_decode_checkpoint_publishes_no_raster() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let cancel = Cancellation::default();
        let mut hook = || cancel.cancel();
        let result = super::decode_raster_with_hook(
            &encode(ImageFormat::Png),
            limits(),
            &cancel,
            super::HookPhase::AfterAdmission,
            &mut hook,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("a cancelled decode must not return a raster"),
        };
        assert_eq!(error, crate::MediaError::Cancelled);
    }

    #[test]
    fn a_cancel_after_conversion_publishes_no_stale_raster() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let cancel = Cancellation::default();
        let mut hook = || cancel.cancel();
        let result = super::decode_raster_with_hook(
            &encode(ImageFormat::Png),
            limits(),
            &cancel,
            super::HookPhase::AfterConversion,
            &mut hook,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("a cancelled decode must not return a raster"),
        };
        assert_eq!(error, crate::MediaError::Cancelled);
    }

    #[test]
    fn a_full_in_flight_pool_refuses_a_decode_without_blocking() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        let held = crate::decode::reserve_in_flight(crate::decode::IN_FLIGHT_POOL_BYTES - 1)
            .expect("hold all but one byte");
        let refused = decode_raster(
            &encode(ImageFormat::Png),
            limits(),
            &Cancellation::default(),
        );
        assert_eq!(refused.unwrap_err(), crate::MediaError::Busy);
        drop(held);
        decode_raster(
            &encode(ImageFormat::Png),
            limits(),
            &Cancellation::default(),
        )
        .expect("capacity released");
    }

    #[test]
    fn an_animated_webp_is_unavailable_and_a_still_webp_is_not() {
        let _guard = crate::decode::DECODE_TEST_LOCK.lock().expect("test lock");
        // Minimal RIFF/WEBP container with an ANIM chunk; the classification is a pure
        // byte scan, so the payload does not need to be a decodable image.
        let mut animated = Vec::new();
        animated.extend_from_slice(b"RIFF");
        animated.extend_from_slice(&(4u32 + 8 + 6).to_le_bytes());
        animated.extend_from_slice(b"WEBP");
        animated.extend_from_slice(b"ANIM");
        animated.extend_from_slice(&6u32.to_le_bytes());
        animated.extend_from_slice(&[0u8; 6]);
        assert_eq!(
            super::ensure_still(&animated, AcceptedFormat::WebP).err(),
            Some(crate::MediaError::Unavailable)
        );
        // A still WebP from the encoder is not misclassified.
        let still = encode(ImageFormat::WebP);
        assert!(super::ensure_still(&still, AcceptedFormat::WebP).is_ok());
    }
}
