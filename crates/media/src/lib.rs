//! MP-IMG1: encoded local acquisition and representation identity.
//!
//! MP-IMG2/IMG3 add one bounded raster adapter ([`decode_raster`]) for the four
//! admitted containers under [`DecodeLimits`]; it owns no GPU object, performs no
//! network or descriptor authorization and no control-frame or import action.
//!
//! All synchronous acquisition runs on the caller's background executor. An owned
//! file/reader is an explicit local capability, never obtained from media metadata.

mod cache;
mod decode;
mod key;
mod raster;

pub use cache::{
    Acquisition, Cancellation, EncodedMedia, ExpectedDigest, LocalLimits, MediaCache, Sha256Digest,
    Usage, Verification,
};
pub use decode::{
    DECODER_BOOKKEEPING_BYTES, DecodeAdmission, DecodeBudget, DecodeLimits, DecodeLimitsOverride,
    DecodeRequest, MAX_SUPPORTED_BIT_DEPTH, MAX_SUPPORTED_CHANNELS, admit_decode,
};
pub use key::{
    DecodePolicy, LocalId, LocalProvenance, Orientation, PixelSize, RepresentationKey,
    RepresentationSource,
};
pub use raster::{
    AcceptedFormat, DecodedRaster, decode_png_rows, decode_png_rows_with_hook, decode_raster,
    decode_raster_joined,
};

/// Safe failure categories; no private paths, descriptor tokens or payload bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaError {
    InvalidLimits,
    InvalidMetadata,
    TooLarge,
    /// The process-wide in-flight decode reservation pool would be exceeded.
    Busy,
    Capacity,
    Allocation,
    Cancelled,
    UnsupportedContainer,
    NotRegularFile,
    LengthMismatch,
    ExpectedProvenanceMismatch,
    DigestMismatch,
    ExternalAcquisitionUnavailable,
    KeyAlreadyCached,
    Io(std::io::ErrorKind),
    Unavailable,
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MediaError {}
impl From<std::io::Error> for MediaError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}
