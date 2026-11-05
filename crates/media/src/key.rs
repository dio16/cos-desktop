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

#[cfg(test)]
mod tests {
    use super::{LocalId, MediaError, PixelSize};

    #[test]
    fn an_opaque_local_id_refuses_the_shapes_that_could_be_a_path_or_url() {
        // The value is opaque metadata from an untrusted publisher, so the shapes that would make it
        // usable as a locator are the ones to refuse.
        for refused in [
            "",
            ".",
            "..",
            "../escape",
            "..\\escape",
            "/absolute",
            "\\absolute",
            "C:\\drive",
            "c:drive",
            "scheme:payload",
            "https://example.invalid/x",
            "file:///x",
            "a/b",
            "a\\b",
            "%2e%2e%2fescape",
            "with space",
            "with\ttab",
            "with\nnewline",
            "nul\0byte",
            "é-accented",
        ] {
            assert_eq!(
                LocalId::new(refused).err(),
                Some(MediaError::InvalidMetadata),
                "{refused:?} must be refused"
            );
        }
    }

    #[test]
    fn an_opaque_local_id_accepts_the_bounded_opaque_shapes() {
        for accepted in [
            "a",
            "0",
            "abc123",
            "a_b",
            "a-b",
            "a.b",
            "release-1_2.3",
            &"a".repeat(128),
        ] {
            let id = LocalId::new(accepted)
                .unwrap_or_else(|error| panic!("{accepted:?} must be accepted, got {error:?}"));
            assert_eq!(id.as_str(), accepted, "the value must survive unchanged");
        }
        // The bound is inclusive on 128 and exclusive beyond it.
        assert_eq!(
            LocalId::new("a".repeat(129)).err(),
            Some(MediaError::InvalidMetadata)
        );
    }

    #[test]
    fn a_local_id_that_ends_with_a_dot_is_accepted_today() {
        // Recorded rather than asserted as desirable: the character set allows a trailing dot, and on
        // Windows a path component ending in a dot aliases the name without it. No current caller
        // turns this value into a path - it is only a cache identity - so this is an observation
        // about the validator, not a defect that is being blessed.
        assert!(LocalId::new("release.").is_ok());
        assert_eq!(
            LocalId::new("release.").expect("accepted").as_str(),
            "release."
        );
        // A leading dot is **accepted** too, and so is a doubled interior dot: what is refused is
        // exactly the two separator spellings, not every name containing dots.
        assert!(LocalId::new(".release").is_ok());
        assert!(LocalId::new("a..b").is_ok());
        assert_eq!(LocalId::new(".").err(), Some(MediaError::InvalidMetadata));
        assert_eq!(LocalId::new("..").err(), Some(MediaError::InvalidMetadata));
    }

    #[test]
    fn a_pixel_size_refuses_a_zero_edge() {
        assert_eq!(
            PixelSize::new(0, 1).err(),
            Some(MediaError::InvalidMetadata)
        );
        assert_eq!(
            PixelSize::new(1, 0).err(),
            Some(MediaError::InvalidMetadata)
        );
        assert_eq!(
            PixelSize::new(0, 0).err(),
            Some(MediaError::InvalidMetadata)
        );
        let size = PixelSize::new(1, 1).expect("a one-pixel size");
        assert_eq!((size.width(), size.height()), (1, 1));
    }

    #[test]
    fn the_byte_arithmetic_refuses_to_wrap() {
        // The largest square: the area still fits in `u64`, four bytes per pixel does not, so the
        // RGBA multiplication refuses instead of wrapping.
        let edge_max = PixelSize::new(u32::MAX, u32::MAX).expect("a non-zero size");
        assert_eq!(
            edge_max.checked_area(),
            Ok(u64::from(u32::MAX) * u64::from(u32::MAX))
        );
        assert_eq!(edge_max.checked_rgba_bytes(), Err(MediaError::TooLarge));
        // A large but representable size keeps exact arithmetic rather than saturating.
        let wide = PixelSize::new(u32::MAX, 4).expect("a non-zero size");
        assert_eq!(wide.checked_area(), Ok(u64::from(u32::MAX) * 4));
        assert_eq!(wide.checked_rgba_bytes(), Ok(u64::from(u32::MAX) * 4 * 4));
        // Exactness at a round size, so a future change to saturate would be visible.
        let square = PixelSize::new(1 << 20, 1 << 20).expect("a non-zero size");
        assert_eq!(square.checked_area(), Ok(1u64 << 40));
        assert_eq!(square.checked_rgba_bytes(), Ok(1u64 << 42));
    }
}
