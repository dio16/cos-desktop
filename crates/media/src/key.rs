//! Presentation identity, never a source of canonical or authorization truth.

use desktop_protocol::{
    ArtifactDigest, ArtifactId, Counter, EpochId, RepresentationKind, ServerId,
};

use crate::MediaError;

/// A bounded local manifest/source identifier, not a filesystem path or locator.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalId(String);

impl LocalId {
    pub fn new(value: impl Into<String>) -> Result<Self, MediaError> {
        let value = value.into();
        // Opaque bounded identity, never a path or URL: no separators, drive colon,
        // scheme colon or dot-segment shapes.
        if value.is_empty()
            || value.len() > 128
            || value == "."
            || value == ".."
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(MediaError::InvalidMetadata);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit source and manifest provenance. Neither variant is authenticated Runtime input.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LocalProvenance {
    Generated {
        generator: LocalId,
        manifest: LocalId,
        source: LocalId,
    },
    Fixture {
        fixture: LocalId,
        manifest: LocalId,
        source: LocalId,
    },
}

/// Existing canonical values may identify a representation but grant no acquisition permission.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RepresentationSource {
    Local {
        provenance: LocalProvenance,
        revision: u64,
    },
    Artifact {
        server: ServerId,
        epoch: EpochId,
        artifact: ArtifactId,
        digest: ArtifactDigest,
        revision: Counter,
    },
}

/// Requested output size; does not claim that decoding this size is admissible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PixelSize {
    width: u32,
    height: u32,
}

impl PixelSize {
    pub fn new(width: u32, height: u32) -> Result<Self, MediaError> {
        if width == 0 || height == 0 {
            return Err(MediaError::InvalidMetadata);
        }
        Ok(Self { width, height })
    }

    pub fn width(self) -> u32 {
        self.width
    }
    pub fn height(self) -> u32 {
        self.height
    }

    /// Checked pixel count. Overflow is an explicit bounded failure, never a wrapped value.
    pub fn checked_area(self) -> Result<u64, MediaError> {
        (u64::from(self.width))
            .checked_mul(u64::from(self.height))
            .ok_or(MediaError::TooLarge)
    }

    /// Checked decoded RGBA byte count for a future decoder. MP-IMG2 owns whether that
    /// budget is admissible; this only refuses arithmetic that would wrap.
    pub fn checked_rgba_bytes(self) -> Result<u64, MediaError> {
        self.checked_area()?
            .checked_mul(4)
            .ok_or(MediaError::TooLarge)
    }
}

/// All eight EXIF orientation transforms, included in representation identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Orientation {
    Normal,
    MirrorHorizontal,
    Rotate180,
    MirrorVertical,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}

/// Declared processing policy identity. This task neither selects nor runs a decoder/colour transform.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecodePolicy {
    pub decoder: LocalId,
    pub colour: LocalId,
    pub generation: u64,
}

/// Immutable full representation identity. Source revisions must change when source content changes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepresentationKey {
    pub source: RepresentationSource,
    pub kind: RepresentationKind,
    pub size: PixelSize,
    pub orientation: Orientation,
    pub policy: DecodePolicy,
}
