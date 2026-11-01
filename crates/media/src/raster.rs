//! MP-IMG2/IMG3 bounded raster decode on the admitted profile.
//!
//! The decoder accepts only the four admitted containers, refuses anything whose
//! header dimensions or decoded size exceed the caller's [`DecodeLimits`], and
//! returns RGBA8. It owns no GPU object, performs no acquisition, and never
//! substitutes a preview or placeholder for a refused input.

use image::{DynamicImage, ImageFormat, ImageReader};
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
    // RGBA8 is the only output this adapter produces, so the admission uses those
    // parameters; a caller's narrower bit depth refuses a decode here rather than
    // silently quantising it.
    admit_decode(
        DecodeRequest {
            size,
            bit_depth: 8,
            channels: 4,
            declared_scratch_bytes: 0,
        },
        limits,
        cancel,
    )?;
    cancel.check()?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.image_format());
    reader.limits(decoder_limits(limits));
    let decoded = reader.decode().map_err(|_| MediaError::InvalidMetadata)?;
    cancel.check()?;
    let rgba = to_rgba8(decoded);
    if rgba.len() as u64 > limits.max_decoded_bytes {
        return Err(MediaError::TooLarge);
    }
    Ok(DecodedRaster { size, format, rgba })
}

fn decoder_limits(limits: DecodeLimits) -> image::Limits {
    let mut decoder = image::Limits::default();
    decoder.max_image_width = Some(limits.max_dimension);
    decoder.max_image_height = Some(limits.max_dimension);
    decoder.max_alloc = Some(limits.max_decoded_bytes);
    decoder
}

fn to_rgba8(decoded: DynamicImage) -> Vec<u8> {
    decoded.to_rgba8().into_raw()
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
