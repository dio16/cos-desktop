//! Canonical encoding and integrity digests for staged bundles.
//!
//! # CanonicalV1
//!
//! [`canonical_json`] emits the COS Runtime's `CanonicalV1` encoding. It is the
//! compression of the whole rule set, because both sides of the wire must agree
//! byte for byte:
//!
//! - the value is first converted to [`serde_json::Value`], then written; the
//!   declared struct field order never reaches the output,
//! - object keys are NFC-normalised, sorted by code point order (Rust `String`
//!   ordering is UTF-8 byte ordering, which is code point ordering) and must not
//!   collide under NFC normalisation,
//! - object members whose value is `null` are omitted entirely; array elements
//!   are kept in order, including `null` elements,
//! - strings are NFC-normalised and escaped exactly like Python
//!   `json.dumps(ensure_ascii=False)`: only `"`, `\` and C0 controls are
//!   escaped (`\b`, `\f`, `\n`, `\r`, `\t`, otherwise `\u00xx` with lowercase
//!   hex); every other character is written verbatim as UTF-8,
//! - numbers must be integers in plain decimal text. A number whose text
//!   contains `.`, `e` or `E` is a [`CanonicalError`]: floats are outside the
//!   contract, so a projection DTO that carries one fails closed instead of
//!   producing a digest no conforming server could issue,
//! - separators are `,` and `:` with no whitespace anywhere,
//! - the top-level value may be any allowed type.
//!
//! The COS Runtime owns this rule: `contract-core`'s `canonical_json_v1` (and
//! `digest_v1`) is the authority, and this module is a parity implementation
//! pinned against Runtime-generated vectors
//! (`tests/vectors/desktop-projection-v1.json`). Do not "fix" a disagreement
//! here without changing the Runtime owner first.
//!
//! # Domain separation
//!
//! Two digest families exist on purpose:
//!
//! - [`integrity_digest_of`] is an internal, **domainless** digest over one
//!   value: `sha256(canonical_json(value))`. It covers command envelopes and
//!   receipts, where both the issuing and the verifying side are this crate and
//!   a value can never be confused with another payload type. Adding a domain
//!   here would move every existing command/receipt digest, so it stays as is.
//! - [`domain_digest`] is the **wire** digest:
//!   `sha256(domain bytes || 0x00 || canonical_json(value))`. A server issues a
//!   digest for a staged mutation bundle that is re-verified by a client across
//!   a language boundary, and the same canonical bytes may legitimately appear
//!   in two protocol roles. The domain plus the NUL separator binds the digest
//!   to its role, exactly like the Runtime's `digest_v1`. The NUL byte is a
//!   delimiter the canonical encoding can never emit, so no domain/payload pair
//!   can be re-split into another one.
//!
//! [`projection_bundle_digest`] is the only bundle digest on the wire: the
//! `SnapshotEnd.integrity_digest` of every staged snapshot chunk and the
//! `DeltaTransaction.integrity_digest` of every atomic delta. Both sides of the
//! fixture and the client recompute it over parsed values, so an authoring
//! mistake cannot silently pass: a mismatch invalidates the bundle and requests
//! resync.
//!
//! # Migration boundary
//!
//! This module used to emit `serde_json::to_vec`, whose declared field order and
//! escaping differ from CanonicalV1. [`sha256_digest`] and
//! [`integrity_digest_of`] keep their signatures and stay domainless, but their
//! **values for a given payload moved** when the encoding changed:
//!
//! - command/receipt dedupe digests are issued and verified inside this client
//!   (the fixture path included), so nothing compares them against a value
//!   produced by the old encoding; no old digest is persisted or migrated,
//! - there is no interoperable command digest in the wire contract yet. If the
//!   Runtime starts verifying one, that needs its own domain-separated spec and
//!   digest domain; reusing this domainless digest would be unambiguous only by
//!   accident,
//! - every digest verified across the language boundary is
//!   [`projection_bundle_digest`] (domain `desktop-projection`); no command or
//!   receipt digest is wire-verified today.
//!
//! # Integer width
//!
//! CanonicalV1 numbers are integers in plain decimal text, and `serde_json` is
//! built with `arbitrary_precision` (the Runtime enables it too) so a literal
//! wider than `u64`/`i64` keeps its exact digits instead of failing to parse or
//! drifting through an `f64`.

use std::fmt;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::ids::IntegrityDigest;
use crate::messages::ProjectionMutation;

/// Digest domain of a Desktop projection mutation bundle.
pub const DESKTOP_PROJECTION_DOMAIN: &str = "desktop-projection";

/// Schema version folded into a Desktop projection mutation bundle.
pub const PROJECTION_BUNDLE_SCHEMA_VERSION: u32 = 1;

/// Canonical-encoding failure.
#[derive(Debug)]
pub struct CanonicalError {
    message: String,
}

impl CanonicalError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CanonicalError {}

impl From<serde_json::Error> for CanonicalError {
    fn from(error: serde_json::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

/// CanonicalV1 UTF-8 JSON bytes of a value.
///
/// See the module documentation for the exact rules; the COS Runtime's
/// `canonical_json_v1` owns them.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalError> {
    let value = serde_json::to_value(value)?;
    let mut out = String::new();
    write_value(&value, &mut out)?;
    Ok(out.into_bytes())
}

fn write_string(out: &mut String, text: &str) {
    // Python `json.dumps(..., ensure_ascii=False)`: non-ASCII is written
    // verbatim; only quote, backslash and C0 controls are escaped.
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn write_value(value: &Value, out: &mut String) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            // Integer text is preserved as-is. A fraction or exponent marker
            // means the value is a float, which CanonicalV1 does not admit.
            let raw = number.to_string();
            if raw.contains('.') || raw.contains('e') || raw.contains('E') {
                return Err(CanonicalError::new(
                    "float is not an allowed canonical type (integers only)",
                ));
            }
            out.push_str(&raw);
        }
        Value::String(text) => {
            let normalised: String = text.nfc().collect();
            write_string(out, &normalised);
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(members) => {
            // Keys are NFC-normalised, sorted once, then checked for collisions:
            // two distinct wire keys that normalise to one key would make the
            // encoding depend on map iteration order.
            let mut normalised: Vec<(String, &Value)> = Vec::with_capacity(members.len());
            for (key, member) in members {
                let nfc: String = key.nfc().collect();
                normalised.push((nfc, member));
            }
            normalised.sort_by(|left, right| left.0.cmp(&right.0));
            for window in normalised.windows(2) {
                if window[0].0 == window[1].0 {
                    return Err(CanonicalError::new(
                        "object keys collide under NFC normalisation (non-deterministic)",
                    ));
                }
            }
            out.push('{');
            let mut first = true;
            for (key, member) in normalised {
                if member.is_null() {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                write_string(out, &key);
                out.push(':');
                write_value(member, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// SHA-256 integrity digest of arbitrary bytes, rendered as `sha256:<hex>`.
pub fn sha256_digest(bytes: &[u8]) -> IntegrityDigest {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    finish_digest(hasher)
}

/// Integrity digest of the canonical encoding of a value (domainless).
///
/// Keep this digest domainless: it covers command envelopes and receipts, and
/// the wire bundle digest uses [`domain_digest`] instead (see the module
/// documentation).
pub fn integrity_digest_of<T: Serialize + ?Sized>(
    value: &T,
) -> Result<IntegrityDigest, CanonicalError> {
    let bytes = canonical_json(value)?;
    Ok(sha256_digest(&bytes))
}

/// Domain-separated digest: `sha256(domain bytes || 0x00 || canonical_json)`.
///
/// Rendered as `sha256:<hex>`, like the COS Runtime's `digest_v1`.
pub fn domain_digest<T: Serialize + ?Sized>(
    domain: &str,
    value: &T,
) -> Result<IntegrityDigest, CanonicalError> {
    let canonical = canonical_json(value)?;
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    hasher.update(&canonical);
    Ok(finish_digest(hasher))
}

/// Digest of a Desktop projection mutation bundle.
///
/// Covers exactly `{"schema_version": 1, "mutations": <mutations>}` under
/// [`DESKTOP_PROJECTION_DOMAIN`]. This is the value a server declares in
/// `SnapshotEnd.integrity_digest` and `DeltaTransaction.integrity_digest`.
pub fn projection_bundle_digest(
    mutations: &[ProjectionMutation],
) -> Result<IntegrityDigest, CanonicalError> {
    // Field order is irrelevant: CanonicalV1 sorts object keys. The field set
    // is exactly schema_version + mutations.
    #[derive(Serialize)]
    struct ProjectionBundle<'a> {
        schema_version: u32,
        mutations: &'a [ProjectionMutation],
    }

    let bundle = ProjectionBundle {
        schema_version: PROJECTION_BUNDLE_SCHEMA_VERSION,
        mutations,
    };
    domain_digest(DESKTOP_PROJECTION_DOMAIN, &bundle)
}

fn finish_digest(hasher: Sha256) -> IntegrityDigest {
    let digest = hasher.finalize();
    let mut rendered = String::with_capacity(7 + 64);
    rendered.push_str("sha256:");
    for byte in digest.iter() {
        rendered.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        rendered.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    IntegrityDigest::new_unchecked(rendered)
}

#[cfg(test)]
mod tests {
    use super::{
        DESKTOP_PROJECTION_DOMAIN, PROJECTION_BUNDLE_SCHEMA_VERSION, canonical_json, domain_digest,
        integrity_digest_of, projection_bundle_digest, sha256_digest,
    };
    use crate::ids::TaskId;
    use crate::messages::{ProjectionMutation, RemoveTarget};
    use serde_json::json;

    fn canonical_text(value: &serde_json::Value) -> String {
        String::from_utf8(canonical_json(value).expect("canonical")).expect("utf-8")
    }

    fn task_removal(id: &str) -> ProjectionMutation {
        ProjectionMutation::Remove {
            base_revision: crate::ids::Counter::new("1").expect("counter"),
            target: RemoveTarget::Task {
                task_id: TaskId::fixture(id),
            },
        }
    }

    #[test]
    fn sorts_object_keys_by_code_point() {
        assert_eq!(
            canonical_text(&json!({"b": 1, "a": 2, "C": 3})),
            r#"{"C":3,"a":2,"b":1}"#
        );
        // Code point order, not locale order.
        assert_eq!(
            canonical_text(&json!({"z": 1, "É": 2, "a": 3})),
            r#"{"a":3,"z":1,"É":2}"#
        );
    }

    #[test]
    fn omits_null_object_members_only() {
        assert_eq!(
            canonical_text(&json!({"a": 1, "b": null, "c": {"d": null, "e": 2}})),
            r#"{"a":1,"c":{"e":2}}"#
        );
        // Array order and null elements are preserved.
        assert_eq!(
            canonical_text(&json!([null, 2, {"a": null}])),
            r#"[null,2,{}]"#
        );
    }

    #[test]
    fn uses_compact_separators_and_empty_containers() {
        assert_eq!(
            canonical_text(&json!({"a": 1, "b": [true, null, "x"]})),
            r#"{"a":1,"b":[true,null,"x"]}"#
        );
        assert_eq!(canonical_text(&json!({})), "{}");
        assert_eq!(canonical_text(&json!([])), "[]");
    }

    #[test]
    fn nfc_normalises_strings() {
        // "é" as e + combining acute (U+0065 U+0301) becomes NFC U+00E9.
        assert_eq!(
            canonical_text(&json!({"label": "e\u{301}"})),
            "{\"label\":\"é\"}"
        );
        assert_eq!(
            canonical_json(&json!({"label": "e\u{301}"})).expect("canonical"),
            canonical_json(&json!({"label": "é"})).expect("canonical")
        );
        // Keys normalise too, and a collision under NFC is rejected.
        assert_eq!(canonical_text(&json!({"e\u{301}": 1})), "{\"é\":1}");
        let colliding: serde_json::Value =
            serde_json::from_str(r#"{"e\u0301": 1, "é": 2}"#).expect("json");
        assert!(canonical_json(&colliding).is_err());
    }

    #[test]
    fn escapes_like_python_json_dumps_without_ascii_escaping() {
        assert_eq!(
            canonical_text(
                &json!({"s": "quote:\" backslash:\\ newline:\n tab:\t cr:\r bs:\u{08} ff:\u{0c}"})
            ),
            "{\"s\":\"quote:\\\" backslash:\\\\ newline:\\n tab:\\t cr:\\r bs:\\b ff:\\f\"}"
        );
        // Remaining C0 controls use lowercase \u00xx; DEL and non-ASCII stay raw.
        assert_eq!(
            canonical_text(&json!({"s": "\u{01}\u{1f}\u{7f}日本"})),
            "{\"s\":\"\\u0001\\u001f\u{7f}日本\"}"
        );
    }

    #[test]
    fn rejects_floats_and_keeps_integers() {
        assert!(canonical_json(&json!({"score": 1.5})).is_err());
        assert!(canonical_json(&json!({"score": 58.0})).is_err());
        assert!(canonical_json(&json!({"score": 1e3})).is_err());
        assert_eq!(
            canonical_text(&json!({"n": -42, "m": 0})),
            r#"{"m":0,"n":-42}"#
        );
        assert_eq!(
            canonical_text(&json!({"n": 9007199254740993i64})),
            r#"{"n":9007199254740993}"#
        );
    }

    #[test]
    fn keeps_integers_wider_than_u64_and_i64() {
        // serde_json `arbitrary_precision` is a parity requirement: the Runtime
        // enables it, so a literal beyond u64/i64 must keep its exact decimal
        // text instead of failing to parse or rounding through an f64.
        let value: serde_json::Value =
            serde_json::from_str(r#"{"n":18446744073709551616,"m":-9223372036854775809}"#)
                .expect("arbitrary-precision integers parse");
        assert_eq!(
            canonical_text(&value),
            r#"{"m":-9223372036854775809,"n":18446744073709551616}"#
        );
        // Width does not turn a fraction into an integer.
        let wide_float: serde_json::Value =
            serde_json::from_str(r#"{"n":18446744073709551616.5}"#).expect("parses");
        assert!(canonical_json(&wide_float).is_err());
    }

    #[test]
    fn nfc_normalises_unicode_boundary_cases() {
        // U+212B ANGSTROM SIGN composes to U+00C5 under NFC.
        assert_eq!(canonical_text(&json!({"k": "\u{212b}"})), "{\"k\":\"Å\"}");
        // U+0958 is a composition exclusion: NFC leaves KA + NUKTA (U+0915 U+093C).
        assert_eq!(
            canonical_text(&json!({"k": "\u{958}"})),
            "{\"k\":\"\u{915}\u{93c}\"}"
        );
    }

    #[test]
    fn non_bmp_keys_sort_by_code_point() {
        // U+E000 < U+10000 < U+1F600: `String` ordering is UTF-8 byte order,
        // which is code point order for every scalar value, including non-BMP.
        let value: serde_json::Value =
            serde_json::from_str(r#"{"\ud83d\ude00":1,"\ue000":2,"\ud800\udc00":3}"#)
                .expect("surrogate-pair keys parse");
        assert_eq!(
            canonical_text(&value),
            "{\"\u{e000}\":2,\"\u{10000}\":3,\"\u{1f600}\":1}"
        );
    }

    #[test]
    fn keeps_array_order_and_nesting() {
        assert_eq!(
            canonical_text(&json!({"a": [[3], [2, [1]]]})),
            r#"{"a":[[3],[2,[1]]]}"#
        );
    }

    #[test]
    fn protocol_domain_is_pinned() {
        assert_eq!(DESKTOP_PROJECTION_DOMAIN, "desktop-projection");
        assert_eq!(PROJECTION_BUNDLE_SCHEMA_VERSION, 1);
        // The literal domain bytes are part of the wire contract.
        assert_eq!(DESKTOP_PROJECTION_DOMAIN.as_bytes(), b"desktop-projection");
    }

    #[test]
    fn domain_digest_is_pinned_to_the_runtime_recipe() {
        // sha256("desktop-projection\0" + `{"mutations":[],"schema_version":1}`),
        // computed independently (Python hashlib) from the CanonicalV1 text.
        let expected = "sha256:1417b7b52c8802348ef2149f8b2600cacaaba06841bd23d6c0ab4c8ba8e8a34c";
        let empty = projection_bundle_digest(&[]).expect("digest");
        assert_eq!(empty.as_str(), expected);
        // The empty elevation is domain-separated, not the plain sha256.
        assert_ne!(
            empty,
            sha256_digest(br#"{"mutations":[],"schema_version":1}"#)
        );
        // Different domains never collide on the same payload.
        let first = domain_digest("desktop-projection", &json!({"a": 1})).expect("digest");
        let second = domain_digest("other-domain", &json!({"a": 1})).expect("digest");
        assert_ne!(first, second);
    }

    #[test]
    fn projection_bundle_digest_is_deterministic_and_sensitive() {
        let first = projection_bundle_digest(&[task_removal("task/1")]).expect("digest");
        let second = projection_bundle_digest(&[task_removal("task/1")]).expect("digest");
        assert_eq!(first, second);
        assert!(first.as_str().starts_with("sha256:"));
        assert_eq!(first.as_str().len(), 7 + 64);
        let changed = projection_bundle_digest(&[task_removal("task/2")]).expect("digest");
        assert_ne!(first, changed);
        let more = projection_bundle_digest(&[task_removal("task/1"), task_removal("task/1")])
            .expect("digest");
        assert_ne!(first, more);
    }

    #[test]
    fn sealing_digest_and_command_digest_stay_identical() {
        // `sha256_digest` is unchanged: the empty-string digest is a fixed pin.
        assert_eq!(
            sha256_digest(b"").as_str(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // The internal digest has no domain prefix: it is the canonical bytes
        // digest only.
        let value = json!({"a": 1, "b": null});
        assert_eq!(
            integrity_digest_of(&value).expect("digest"),
            sha256_digest(br#"{"a":1}"#)
        );
    }
}

#[cfg(test)]
mod determinism_tests {
    use super::{CanonicalError, canonical_json};

    fn encoded(value: &serde_json::Value) -> String {
        String::from_utf8(canonical_json(value).expect("canonical")).expect("utf-8")
    }

    /// The encoding's key order is what a peer sees, so it is pinned here.
    ///
    /// **What this cannot prove:** `serde_json::Value`'s object is already ordered, so deleting the
    /// encoder's own sort does not change any result and this test still passes - a mutation showed
    /// exactly that. The sort in `write_value` is therefore the guarantee that survives a change to
    /// that representation (for example enabling `preserve_order`), and the test pins the output
    /// order without being able to attribute it to one of the two.
    #[test]
    fn key_order_in_the_input_does_not_change_the_encoding() {
        // The digest is over these bytes, so two peers that spell the same object in different
        // orders must produce the same encoding or the digest is worthless.
        let first: serde_json::Value =
            serde_json::from_str(r#"{"b": 1, "a": 2, "c": {"z": 1, "y": 2}}"#).expect("json");
        let second: serde_json::Value =
            serde_json::from_str(r#"{"c": {"y": 2, "z": 1}, "a": 2, "b": 1}"#).expect("json");
        assert_eq!(encoded(&first), encoded(&second));
        assert_eq!(encoded(&first), r#"{"a":2,"b":1,"c":{"y":2,"z":1}}"#);
    }

    #[test]
    fn a_non_ascii_key_and_value_are_normalised_before_encoding() {
        // Composed and decomposed spellings are the same text; NFC makes them the same bytes.
        let composed: serde_json::Value =
            serde_json::from_str("{\"caf\u{e9}\": \"\u{e9}\"}").expect("json");
        let decomposed: serde_json::Value =
            serde_json::from_str("{\"cafe\u{301}\": \"e\u{301}\"}").expect("json");
        assert_eq!(
            encoded(&composed),
            encoded(&decomposed),
            "NFC must make the two spellings identical"
        );
    }

    #[test]
    fn two_keys_that_collide_under_normalisation_are_refused() {
        // Sorting alone would not help here: the two keys normalise to one, so the encoding would
        // depend on which member the map happened to hold last. The encoder must refuse instead.
        let colliding: serde_json::Value =
            serde_json::from_str("{\"caf\u{e9}\": 1, \"cafe\u{301}\": 2}").expect("json");
        let error = canonical_json(&colliding).expect_err("a collision must be refused");
        let message = format!("{error}");
        assert!(
            message.contains("collide"),
            "the refusal names the cause: {message}"
        );
    }

    #[test]
    fn a_float_is_refused_and_an_integer_is_kept_verbatim() {
        let float: serde_json::Value = serde_json::from_str(r#"{"a": 1.5}"#).expect("json");
        assert!(matches!(canonical_json(&float), Err(CanonicalError { .. })));
        let exponent: serde_json::Value = serde_json::from_str(r#"{"a": 1e3}"#).expect("json");
        assert!(canonical_json(&exponent).is_err(), "an exponent is a float");

        // Integers are kept exactly, including values past `i64` and `u64` bounds that serde_json can
        // still represent as text.
        let big: serde_json::Value =
            serde_json::from_str(r#"{"a": 123456789012345678901234567890}"#).expect("json");
        assert_eq!(encoded(&big), r#"{"a":123456789012345678901234567890}"#);
    }

    #[test]
    fn a_null_member_is_omitted_but_a_null_array_element_is_kept() {
        // Absence and nullness are the same statement for a member, so omitting it removes a way for
        // two peers to differ; inside an array the position matters, so the null stays.
        let member: serde_json::Value =
            serde_json::from_str(r#"{"a": null, "b": 1}"#).expect("json");
        assert_eq!(encoded(&member), r#"{"b":1}"#);
        let element: serde_json::Value = serde_json::from_str(r#"[null, 1]"#).expect("json");
        assert_eq!(encoded(&element), "[null,1]");
        // An object that is only nulls encodes as an empty object rather than disappearing.
        let only_null: serde_json::Value = serde_json::from_str(r#"{"a": null}"#).expect("json");
        assert_eq!(encoded(&only_null), "{}");
    }

    #[test]
    fn the_encoding_is_stable_across_repeated_calls() {
        // A digest that changed between two calls on the same value would break every integrity
        // check, so the encoder must be a pure function of its input.
        let value: serde_json::Value =
            serde_json::from_str(r#"{"list": [3, 1, 2], "obj": {"b": true, "a": "text"}}"#)
                .expect("json");
        let first = encoded(&value);
        for _ in 0..8 {
            assert_eq!(encoded(&value), first);
        }
        // Array order is meaningful and therefore preserved, unlike object order.
        assert_eq!(first, r#"{"list":[3,1,2],"obj":{"a":"text","b":true}}"#);
    }
}
