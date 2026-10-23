//! Version range negotiation policy tests.

use desktop_protocol::{
    ErrorCode, HelloAccepted, HelloRejected, HelloRequest, NegotiationFailure, ProtocolVersions,
    Version, VersionAdvertisement, VersionRange, negotiate, negotiate_session,
};

fn range(major: u16, min_minor: u16, max_minor: u16) -> VersionRange {
    VersionRange::new(major, min_minor, max_minor)
}

#[test]
fn highest_mutually_supported_minor_is_selected() {
    let selected = negotiate(&[range(1, 0, 3)], &[range(1, 1, 2)]).expect("intersection");
    assert_eq!(selected, Version::new(1, 2));

    let exact = negotiate(&[range(1, 0, 0)], &[range(1, 0, 0)]).expect("intersection");
    assert_eq!(exact, Version::new(1, 0));
}

#[test]
fn multiple_majors_are_separate_entries_and_highest_wins() {
    let client = [range(1, 0, 2), range(2, 0, 0)];
    let server = [range(2, 0, 1), range(1, 0, 1)];
    let selected = negotiate(&client, &server).expect("intersection");
    assert_eq!(selected, Version::new(2, 0));

    let only_old = negotiate(&[range(1, 0, 2)], &[range(1, 0, 1)]).expect("intersection");
    assert_eq!(only_old, Version::new(1, 1));
}

#[test]
fn disjoint_ranges_produce_no_intersection() {
    assert_eq!(negotiate(&[range(1, 0, 2)], &[range(2, 0, 0)]), None);
    assert_eq!(negotiate(&[range(1, 5, 9)], &[range(1, 0, 4)]), None);
}

#[test]
fn malformed_ranges_never_match() {
    assert_eq!(negotiate(&[range(1, 3, 1)], &[range(1, 0, 2)]), None);
    assert!(!range(1, 3, 1).is_well_formed());
}

#[test]
fn session_negotiation_reports_precise_failures() {
    let client = VersionAdvertisement {
        protocol: vec![range(1, 0, 1)],
        schema: vec![range(1, 0, 1)],
    };
    let compatible = VersionAdvertisement {
        protocol: vec![range(1, 0, 0)],
        schema: vec![range(1, 0, 0)],
    };
    let selected = negotiate_session(&client, &compatible).expect("compatible");
    assert_eq!(
        selected,
        ProtocolVersions {
            protocol: Version::new(1, 0),
            schema: Version::new(1, 0),
        }
    );

    let disjoint_protocol = VersionAdvertisement {
        protocol: vec![range(2, 0, 0)],
        schema: vec![range(1, 0, 1)],
    };
    assert_eq!(
        negotiate_session(&client, &disjoint_protocol),
        Err(NegotiationFailure::NoProtocolIntersection)
    );

    let disjoint_schema = VersionAdvertisement {
        protocol: vec![range(1, 0, 1)],
        schema: vec![range(3, 0, 0)],
    };
    assert_eq!(
        negotiate_session(&client, &disjoint_schema),
        Err(NegotiationFailure::NoSchemaIntersection)
    );

    let malformed = VersionAdvertisement {
        protocol: vec![range(1, 4, 2)],
        schema: vec![range(1, 0, 1)],
    };
    assert_eq!(
        negotiate_session(&client, &malformed),
        Err(NegotiationFailure::MalformedAdvertisement)
    );
}

#[test]
fn application_version_equality_is_never_required() {
    let request = HelloRequest {
        desktop_version: "cos-desktop/0.1.0".to_owned(),
        protocol: vec![range(1, 0, 0)],
        schema: vec![range(1, 0, 0)],
        features: vec!["artifact.view".to_owned()],
    };
    let accepted = HelloAccepted {
        protocol: Version::new(1, 0),
        schema: Version::new(1, 0),
        server_identity: desktop_protocol::ServerId::fixture("server/local"),
        server_build: "fixture-adapter-0.1.0".to_owned(),
        epoch: desktop_protocol::EpochId::fixture("epoch/1"),
        dedupe_retention_ms: Some(60_000),
        limits: None,
    };
    assert_ne!(request.desktop_version, accepted.server_build);
    let wire = serde_json::to_string(&accepted).expect("accepted serializes");
    assert!(wire.contains("fixture-adapter-0.1.0"));
    assert_eq!(request.advertisement().protocol, vec![range(1, 0, 0)]);

    let rejected = HelloRejected {
        code: ErrorCode::VersionIncompatible,
        message: "no intersection".to_owned(),
        reason: NegotiationFailure::NoProtocolIntersection,
        protocol: vec![range(2, 0, 0)],
        schema: vec![range(2, 0, 0)],
    };
    let encoded = serde_json::to_string(&rejected).expect("rejected serializes");
    assert!(encoded.contains("VERSION_INCOMPATIBLE"));
    assert!(encoded.contains("NO_PROTOCOL_INTERSECTION"));
}
