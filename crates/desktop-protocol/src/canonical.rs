//! Canonical encoding and integrity digests for staged bundles.
//!
//! The digest is computed over the canonical UTF-8 JSON encoding of the staged
//! payload (struct field order as declared, list order preserved). Both the
//! server side of the fixture and the client recompute it over parsed values,
//! so an authoring mistake cannot silently pass: a mismatch invalidates the
//! bundle and requests resync.

use std::fmt;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ids::IntegrityDigest;

/// Canonical-encoding failure.
#[derive(Debug)]
pub struct CanonicalError {
    message: String,
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

/// Canonical UTF-8 JSON bytes of a value.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalError> {
    serde_json::to_vec(value).map_err(CanonicalError::from)
}

/// SHA-256 integrity digest of arbitrary bytes, rendered as `sha256:<hex>`.
pub fn sha256_digest(bytes: &[u8]) -> IntegrityDigest {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut rendered = String::with_capacity(7 + 64);
    rendered.push_str("sha256:");
    for byte in digest.iter() {
        rendered.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        rendered.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    IntegrityDigest::new_unchecked(rendered)
}

/// Integrity digest of the canonical encoding of a value.
pub fn integrity_digest_of<T: Serialize + ?Sized>(
    value: &T,
) -> Result<IntegrityDigest, CanonicalError> {
    let bytes = canonical_json(value)?;
    Ok(sha256_digest(&bytes))
}
