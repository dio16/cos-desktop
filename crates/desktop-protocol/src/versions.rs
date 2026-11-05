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

#[cfg(test)]
mod tests {
    use super::*;

    fn range(major: u16, min_minor: u16, max_minor: u16) -> VersionRange {
        VersionRange::new(major, min_minor, max_minor)
    }

    #[test]
    fn the_compatibility_matrix_selects_the_highest_shared_version() {
        // One row per shape the negotiation table has to handle: exact overlap, an intersection
        // narrower than either side, a single shared version, no shared minor, a different major,
        // and a malformed entry that must never match.
        struct Row {
            label: &'static str,
            offered: Vec<VersionRange>,
            supported: Vec<VersionRange>,
            expected: Option<Version>,
        }
        let rows = vec![
            Row {
                label: "identical ranges",
                offered: vec![range(1, 0, 3)],
                supported: vec![range(1, 0, 3)],
                expected: Some(Version::new(1, 3)),
            },
            Row {
                // Each side contributes one end of the intersection, so the shared range is
                // strictly inside both - not one range nested in the other.
                label: "the intersection is strictly inside both ranges",
                offered: vec![range(1, 0, 3)],
                supported: vec![range(1, 2, 5)],
                expected: Some(Version::new(1, 3)),
            },
            Row {
                label: "one range nested in the other",
                offered: vec![range(1, 2, 3)],
                supported: vec![range(1, 0, 5)],
                expected: Some(Version::new(1, 3)),
            },
            Row {
                label: "a single shared version",
                offered: vec![range(1, 4, 4)],
                supported: vec![range(1, 4, 4)],
                expected: Some(Version::new(1, 4)),
            },
            Row {
                label: "ranges that only touch at one minor",
                offered: vec![range(1, 0, 2)],
                supported: vec![range(1, 2, 7)],
                expected: Some(Version::new(1, 2)),
            },
            Row {
                label: "no shared minor",
                offered: vec![range(1, 0, 1)],
                supported: vec![range(1, 4, 6)],
                expected: None,
            },
            Row {
                label: "different major",
                offered: vec![range(1, 0, 9)],
                supported: vec![range(2, 0, 9)],
                expected: None,
            },
            Row {
                label: "a malformed entry never matches",
                offered: vec![range(1, 5, 1)],
                supported: vec![range(1, 0, 9)],
                expected: None,
            },
            Row {
                label: "the highest of several candidates wins",
                offered: vec![range(1, 0, 2), range(1, 5, 7)],
                supported: vec![range(1, 0, 9)],
                expected: Some(Version::new(1, 7)),
            },
        ];
        for row in rows {
            assert_eq!(
                negotiate(&row.offered, &row.supported),
                row.expected,
                "{}: offered {:?} against supported {:?}",
                row.label,
                row.offered,
                row.supported
            );
        }
    }

    #[test]
    fn negotiation_does_not_depend_on_which_side_offers() {
        // The same two advertisements must resolve the same way whichever one is the client: a
        // negotiation that only looked at one side would pass the matrix above and fail here.
        let pairs = [
            (vec![range(1, 0, 3)], vec![range(1, 1, 5)]),
            (vec![range(1, 0, 9)], vec![range(2, 0, 9)]),
            (vec![range(3, 2, 2), range(3, 8, 12)], vec![range(3, 1, 9)]),
            (vec![range(1, 5, 1)], vec![range(1, 0, 9)]),
        ];
        for (left, right) in pairs {
            assert_eq!(
                negotiate(&left, &right),
                negotiate(&right, &left),
                "{left:?} against {right:?} negotiated differently depending on which side offered"
            );
        }
    }

    #[test]
    fn a_negotiated_version_is_inside_both_ranges() {
        // Whatever is selected has to be acceptable to both advertisements; returning a version the
        // client never offered would be a silent incompatibility.
        let offered = vec![range(1, 0, 4), range(2, 1, 3)];
        let supported = vec![range(1, 2, 9), range(2, 0, 2), range(3, 0, 9)];
        let selected = negotiate(&offered, &supported).expect("an intersection exists");
        assert!(offered.iter().any(|r| r.contains(selected)));
        assert!(supported.iter().any(|r| r.contains(selected)));
        // The rule is the lexicographically greatest `(major, minor)` both sides accept, so a
        // higher major wins over a higher minor - `(2, 2)` here, not `(1, 4)`. The assertion pins
        // that rule; the two assertions above pin the property that matters for compatibility.
        assert_eq!(selected, Version::new(2, 2));
    }

    #[test]
    fn a_session_needs_both_a_protocol_and_a_schema_intersection() {
        let server = VersionAdvertisement {
            protocol: vec![range(1, 0, 4)],
            schema: vec![range(2, 0, 1)],
        };
        let agreeing_client = VersionAdvertisement {
            protocol: vec![range(1, 0, 9)],
            schema: vec![range(2, 0, 9)],
        };
        let session = negotiate_session(&agreeing_client, &server).expect("both axes intersect");
        assert_eq!(session.protocol, Version::new(1, 4));
        assert_eq!(session.schema, Version::new(2, 1));

        let protocol_only = VersionAdvertisement {
            protocol: vec![range(1, 0, 9)],
            schema: vec![range(5, 0, 9)],
        };
        assert_eq!(
            negotiate_session(&protocol_only, &server),
            Err(NegotiationFailure::NoSchemaIntersection)
        );

        let schema_only = VersionAdvertisement {
            protocol: vec![range(7, 0, 9)],
            schema: vec![range(2, 0, 9)],
        };
        assert_eq!(
            negotiate_session(&schema_only, &server),
            Err(NegotiationFailure::NoProtocolIntersection)
        );
    }
}
