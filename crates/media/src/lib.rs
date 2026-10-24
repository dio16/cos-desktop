//! MP-IMG1: encoded local feasibility only. No network, descriptor authorization,
//! image decode, GPU object, control-frame media or Product import action.
//!
//! All synchronous acquisition runs on the caller's background executor. An owned
//! file/reader is an explicit local capability, never obtained from media metadata.

mod cache;
mod decode;
mod key;

pub use cache::{
    Acquisition, Cancellation, EncodedMedia, ExpectedDigest, LocalLimits, MediaCache, Sha256Digest,
    Usage, Verification,
};
pub use decode::{
    DecodeAdmission, DecodeBudget, DecodeLimits, DecodeLimitsOverride, DecodeRequest,
    MAX_SUPPORTED_BIT_DEPTH, MAX_SUPPORTED_CHANNELS, admit_decode,
};
pub use key::{
    DecodePolicy, LocalId, LocalProvenance, Orientation, PixelSize, RepresentationKey,
    RepresentationSource,
};

/// Safe failure categories; no private paths, descriptor tokens or payload bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaError {
    InvalidLimits,
    InvalidMetadata,
    TooLarge,
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
