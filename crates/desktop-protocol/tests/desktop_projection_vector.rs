//! Runtime-generated parity vector for the Desktop projection bundle digest.
//!
//! The COS Runtime CLI (`contract-core`) owns every expected value in
//! `vectors/desktop-projection-v1.json`; the file here is a byte-for-byte copy
//! of the Runtime's `tests/vectors/desktop-projection-v1.json` and must never
//! be edited to match this implementation. If the encoding disagrees, the
//! Runtime owner changes first.

use desktop_protocol::{
    ProjectionMutation, SnapshotCounts, canonical_json, projection_bundle_digest,
};

const VECTOR: &str = include_str!("vectors/desktop-projection-v1.json");

/// Offset of the first differing byte, or the shared length when one is a
/// prefix of the other.
fn first_difference(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| left.len().min(right.len()))
}

fn assert_same_bytes(context: &str, desktop: &[u8], runtime: &str) {
    let runtime = runtime.as_bytes();
    if desktop == runtime {
        return;
    }
    let offset = first_difference(desktop, runtime);
    panic!(
        "{context} disagrees with the Runtime vector at byte offset {offset}\n\
         desktop: {:?}\n\
         runtime: {:?}",
        String::from_utf8_lossy(desktop),
        String::from_utf8_lossy(runtime),
    );
}

#[test]
fn runtime_desktop_projection_v1_vector_matches() {
    let vector: serde_json::Value =
        serde_json::from_str(VECTOR).expect("parity vector is valid JSON");
    assert_eq!(
        vector["schema_version"].as_u64(),
        Some(1),
        "vector declares the bundle schema version"
    );

    let expected = &vector["expected"];
    let expected_canonical = expected["canonical"]
        .as_str()
        .expect("expected.canonical is text");
    let expected_digest = expected["integrity_digest"]
        .as_str()
        .expect("expected.integrity_digest is text");
    // Strict decode: the Runtime emits every count field, so a missing field is
    // a vector defect rather than an implicit zero.
    let expected_counts: SnapshotCounts = serde_json::from_value(expected["counts"].clone())
        .expect("expected.counts decodes into SnapshotCounts");

    let mutations: Vec<ProjectionMutation> = serde_json::from_value(vector["mutations"].clone())
        .expect("vector mutations decode into ProjectionMutation values");

    // (a) The Runtime canonicalises the whole bundle, so `expected.canonical` is
    // `{"mutations":<canonical mutations>,"schema_version":1}`. Compare the
    // mutations fragment byte for byte, then the complete bundle text.
    const PREFIX: &str = "{\"mutations\":";
    const SUFFIX: &str = ",\"schema_version\":1}";
    assert!(
        expected_canonical.starts_with(PREFIX),
        "expected.canonical must begin with the mutations object member"
    );
    assert!(
        expected_canonical.ends_with(SUFFIX),
        "expected.canonical must end with the sorted schema_version member"
    );
    let fragment = &expected_canonical[PREFIX.len()..expected_canonical.len() - SUFFIX.len()];
    let mutations_canonical = canonical_json(&mutations).expect("canonical mutations");
    assert_same_bytes("canonical mutations", &mutations_canonical, fragment);

    let bundle = serde_json::json!({
        "schema_version": 1,
        "mutations": mutations,
    });
    let bundle_canonical = canonical_json(&bundle).expect("canonical bundle");
    assert_same_bytes("canonical bundle", &bundle_canonical, expected_canonical);

    // (b) The wire digest is the domain-separated digest of that exact bundle.
    let digest = projection_bundle_digest(&mutations).expect("bundle digest");
    assert_eq!(
        digest.as_str(),
        expected_digest,
        "projection_bundle_digest must equal the Runtime-issued digest"
    );

    // (c) The declared counts are what the mutation list actually contains.
    assert_eq!(
        SnapshotCounts::of(&mutations),
        expected_counts,
        "SnapshotCounts::of must equal the Runtime-issued counts"
    );
}
