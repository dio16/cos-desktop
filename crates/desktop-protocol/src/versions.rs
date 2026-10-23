//! Version advertisement, range negotiation and the handshake.
//!
//! `desktop_version` (client release), `server_build` (server build; a runtime
//! adapter may identify itself without claiming to be cosd), `protocol_version`
//! (envelope/interaction) and `schema_version` (payload grammar/meaning) are
//! separate. Complete application version equality is never required.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::bounds::Limits;
use crate::errors::ErrorCode;
use crate::ids::{EpochId, ServerId};
use crate::macros::wire_enum;

/// A `major.minor` version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Version {
    /// Major number; incompatible changes require a new major.
    pub major: u16,
    /// Minor number; additive, semantics-preserving changes only.
    pub minor: u16,
}

impl Version {
    /// Construct a version.
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Inclusive minor range within one major, e.g. `{major: 1, min_minor: 0, max_minor: 2}`.
///
/// Multiple majors are separate entries, never one ambiguous semver expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    /// Major this entry applies to.
    pub major: u16,
    /// Lowest supported minor (inclusive).
    pub min_minor: u16,
    /// Highest supported minor (inclusive).
    pub max_minor: u16,
}

impl VersionRange {
    /// Construct a range.
    pub const fn new(major: u16, min_minor: u16, max_minor: u16) -> Self {
        Self {
            major,
            min_minor,
            max_minor,
        }
    }

    /// Whether the range is well-formed (`min_minor <= max_minor`).
    pub const fn is_well_formed(&self) -> bool {
        self.min_minor <= self.max_minor
    }

    /// Whether the range contains the version.
    pub fn contains(&self, version: Version) -> bool {
        self.is_well_formed()
            && self.major == version.major
            && version.minor >= self.min_minor
            && version.minor <= self.max_minor
    }
}

/// Advertised protocol and schema ranges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionAdvertisement {
    /// Supported protocol ranges.
    pub protocol: Vec<VersionRange>,
    /// Supported schema ranges.
    pub schema: Vec<VersionRange>,
}

impl VersionAdvertisement {
    /// Whether the advertisement is well-formed and non-empty.
    pub fn is_well_formed(&self) -> bool {
        !self.protocol.is_empty()
            && !self.schema.is_empty()
            && self.protocol.iter().all(VersionRange::is_well_formed)
            && self.schema.iter().all(VersionRange::is_well_formed)
    }

    /// Whether the advertisement covers the given pair.
    pub fn contains(&self, versions: ProtocolVersions) -> bool {
        self.protocol
            .iter()
            .any(|range| range.contains(versions.protocol))
            && self
                .schema
                .iter()
                .any(|range| range.contains(versions.schema))
    }
}

/// Selected protocol/schema pair, fixed for the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtocolVersions {
    /// Selected protocol version.
    pub protocol: Version,
    /// Selected schema version.
    pub schema: Version,
}

wire_enum! {
    /// Why no session could be negotiated.
    pub enum NegotiationFailure {
        NoProtocolIntersection => "NO_PROTOCOL_INTERSECTION",
        NoSchemaIntersection => "NO_SCHEMA_INTERSECTION",
        MalformedAdvertisement => "MALFORMED_ADVERTISEMENT",
    }
}

/// Highest mutually supported version across two range sets.
///
/// Selection is the lexicographically greatest `(major, minor)` present in both
/// advertisements; malformed entries never match.
pub fn negotiate(offered: &[VersionRange], supported: &[VersionRange]) -> Option<Version> {
    let mut best: Option<Version> = None;
    for offered_range in offered {
        if !offered_range.is_well_formed() {
            continue;
        }
        for supported_range in supported {
            if !supported_range.is_well_formed() || offered_range.major != supported_range.major {
                continue;
            }
            let low = offered_range.min_minor.max(supported_range.min_minor);
            let high = offered_range.max_minor.min(supported_range.max_minor);
            if low > high {
                continue;
            }
            let candidate = Version::new(offered_range.major, high);
            if best.is_none_or(|current| candidate > current) {
                best = Some(candidate);
            }
        }
    }
    best
}

/// Negotiate the session pair from a client and a server advertisement.
pub fn negotiate_session(
    client: &VersionAdvertisement,
    server: &VersionAdvertisement,
) -> Result<ProtocolVersions, NegotiationFailure> {
    if !client.is_well_formed() || !server.is_well_formed() {
        return Err(NegotiationFailure::MalformedAdvertisement);
    }
    let protocol = negotiate(&client.protocol, &server.protocol)
        .ok_or(NegotiationFailure::NoProtocolIntersection)?;
    let schema = negotiate(&client.schema, &server.schema)
        .ok_or(NegotiationFailure::NoSchemaIntersection)?;
    Ok(ProtocolVersions { protocol, schema })
}

/// Client handshake request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloRequest {
    /// Client release identity (never required to equal the server build).
    pub desktop_version: String,
    /// Supported protocol ranges.
    pub protocol: Vec<VersionRange>,
    /// Supported schema ranges.
    pub schema: Vec<VersionRange>,
    /// Advertised feature tags; feature support is separate from permission.
    #[serde(default)]
    pub features: Vec<String>,
}

impl HelloRequest {
    /// Build an advertisement view of this request.
    pub fn advertisement(&self) -> VersionAdvertisement {
        VersionAdvertisement {
            protocol: self.protocol.clone(),
            schema: self.schema.clone(),
        }
    }
}

/// Server handshake acceptance with the selected pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAccepted {
    /// Selected protocol version.
    pub protocol: Version,
    /// Selected schema version.
    pub schema: Version,
    /// Server identity.
    pub server_identity: ServerId,
    /// Server build identity. A runtime adapter identifies itself here without
    /// claiming to be cosd.
    pub server_build: String,
    /// Runtime epoch.
    pub epoch: EpochId,
    /// Advertised dedupe retention window, when the server guarantees one.
    #[serde(default)]
    pub dedupe_retention_ms: Option<u64>,
    /// Server-proposed session limits; may only lower the contract defaults.
    #[serde(default)]
    pub limits: Option<Limits>,
}

/// Server handshake rejection; produces no side effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloRejected {
    /// Error code (`VERSION_INCOMPATIBLE`).
    pub code: ErrorCode,
    /// Safe message.
    pub message: String,
    /// Why negotiation failed.
    pub reason: NegotiationFailure,
    /// Server-supported protocol ranges, for diagnostics.
    #[serde(default)]
    pub protocol: Vec<VersionRange>,
    /// Server-supported schema ranges, for diagnostics.
    #[serde(default)]
    pub schema: Vec<VersionRange>,
}
