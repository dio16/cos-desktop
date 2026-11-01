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
    cancel.check()?;
    let format = AcceptedFormat::from_bytes(bytes)?;
    let (width, height) =
        image::ImageReader::with_format(Cursor::new(bytes), format.image_format())
            .into_dimensions()
            .map_err(|_| MediaError::InvalidMetadata)?;
    let size = PixelSize::new(width, height)?;
    let bit_depth = source_bit_depth(bytes, format)?;
    // This adapter only produces RGBA8. A source deeper than 8 bits would be
    // silently quantised, so it is refused rather than advertised as preserved.
    if bit_depth > 8 || bit_depth > limits.max_bit_depth {
        return Err(MediaError::InvalidMetadata);
    }
    // Conservative all-concurrent residency: the decoder's own buffer plus one
    // full RGBA8 conversion buffer of the same size. Both are declared before any
    // allocation, and `max_scratch_bytes` bounds the conversion buffer.
    let decoded_bytes = size.checked_rgba_bytes()?;
    admit_decode(
        DecodeRequest {
            size,
            bit_depth,
            channels: 4,
            declared_scratch_bytes: decoded_bytes,
        },
        limits,
        cancel,
    )?;
    cancel.check()?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.image_format());
    reader.limits(decoder_limits(limits));
    let decoded = reader.decode().map_err(|_| MediaError::InvalidMetadata)?;
    cancel.check()?;
    // `into_rgba8` reuses an already-RGBA8 buffer and converts otherwise; the
    // result is checked again so a decoder reporting different geometry than its
    // header cannot exceed the admitted size.
    let rgba = decoded.into_rgba8().into_raw();
    if rgba.len() as u64 != decoded_bytes || rgba.len() as u64 > limits.max_decoded_bytes {
        return Err(MediaError::TooLarge);
    }
    Ok(DecodedRaster { size, format, rgba })
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
        let mut bytes = encode(ImageFormat::Png);
        let tail = bytes.len() - 8;
        bytes.truncate(tail);
        let error = decode_raster(&bytes, limits(), &Cancellation::default());
        assert!(error.is_err(), "a truncated payload must be refused");
    }

    #[test]
    fn refuses_a_sixteen_bit_png_under_an_eight_bit_limit() {
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
        assert_eq!(error.unwrap_err(), crate::MediaError::InvalidMetadata);
    }

    #[test]
    fn refuses_an_unadmitted_container() {
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
        let cancel = Cancellation::default();
        cancel.cancel();
        let error = decode_raster(&encode(ImageFormat::Png), limits(), &cancel);
        assert_eq!(error.unwrap_err(), crate::MediaError::Cancelled);
    }
}
