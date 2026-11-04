//! MP-IMG3-S1: the upload admission boundary.
//!
//! The approved scope is the **2048×2048 static 8-bit RGB PNG class**. This module turns a
//! decoded raster into something a GPU uploader may use, and it is deliberately strict about
//! three things the plan calls out:
//!
//! 1. **The verified class travels with the pixels.** [`DecodedRaster`] only says "RGBA8 of this
//!    size". Treating that as proof that the source was the approved class would be an
//!    over-claim, so an upload needs a [`VerifiedSourceClass`] produced by the decoder path.
//! 2. **Staging, in-flight and resident bytes are counted separately.** They bound different
//!    things, so one combined figure would hide which limit was hit.
//! 3. **A stale device generation is refused**, not silently uploaded against a new device.
//!
//! Nothing here performs a GPU upload, measures one, or claims one works. It admits or refuses.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::{Cancellation, DecodedRaster, MediaError, PixelSize};

/// The class the decoder actually verified, carried to the upload boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedSourceClass {
    /// Bit depth per channel, as read from the encoded header.
    pub source_bit_depth: u8,
    /// Whether the encoded container was the approved still PNG class.
    pub is_still_png: bool,
    /// Decoded dimensions, which must match the raster.
    pub size: PixelSize,
}

impl VerifiedSourceClass {
    /// The single class approved for MP-IMG3-S1: static 8-bit RGB PNG, 2048×2048.
    pub const APPROVED_EDGE: u32 = 2048;

    /// Whether this is the approved class. Anything else is refused rather than uploaded on a
    /// best-effort basis.
    pub fn is_approved_class(&self) -> bool {
        self.is_still_png
            && self.source_bit_depth == 8
            && self.size.width() == Self::APPROVED_EDGE
            && self.size.height() == Self::APPROVED_EDGE
    }

    /// Read the class from a decoded raster plus what the decoder recorded about the source.
    pub fn from_decoded(raster: &DecodedRaster, source_bit_depth: u8, is_still_png: bool) -> Self {
        Self {
            source_bit_depth,
            is_still_png,
            size: raster.size,
        }
    }
}

/// Where an upload comes from and which device it targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadIdentity {
    /// Content identity of the representation being uploaded.
    pub content_digest: [u8; 32],
    /// Source revision this raster was decoded from.
    pub source_revision: u64,
    /// Device generation the upload is intended for.
    pub device_generation: u64,
}

/// The three budgets, kept separate because they bound different resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadBudget {
    pub staging_bytes: u64,
    pub in_flight_bytes: u64,
    pub resident_bytes: u64,
}

/// Why an upload was refused. Closed, so a caller can report it without inspecting state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadRefusal {
    /// The source was not the approved class (wrong container, depth or dimensions).
    UnverifiedClass,
    /// The raster's length did not match its declared geometry.
    WrongLayout,
    /// A device generation older than the current one.
    StaleGeneration { requested: u64, current: u64 },
    /// Staging, in-flight or resident capacity would be exceeded.
    BudgetExceeded {
        staging: bool,
        in_flight: bool,
        resident: bool,
    },
    /// Cancelled before admission.
    Cancelled,
}

/// An admitted upload: the only value the uploader accepts.
#[derive(Debug)]
pub struct AdmittedUpload {
    class: VerifiedSourceClass,
    identity: UploadIdentity,
    stride_bytes: u32,
    staging_bytes: u64,
    in_flight_bytes: u64,
    resident_bytes: u64,
    pool: Arc<UploadPool>,
}

impl AdmittedUpload {
    pub fn class(&self) -> VerifiedSourceClass {
        self.class
    }
    pub fn identity(&self) -> UploadIdentity {
        self.identity
    }
    pub fn stride_bytes(&self) -> u32 {
        self.stride_bytes
    }
    pub fn staging_bytes(&self) -> u64 {
        self.staging_bytes
    }
    pub fn in_flight_bytes(&self) -> u64 {
        self.in_flight_bytes
    }
    pub fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }
}

impl Drop for AdmittedUpload {
    fn drop(&mut self) {
        forget_identity(&self.pool, &self.identity);
        self.pool.release(
            self.staging_bytes,
            self.in_flight_bytes,
            self.resident_bytes,
        );
    }
}

/// Process-wide counters for the three budgets. Separate counters, separate limits.
#[derive(Debug, Default)]
pub struct UploadPool {
    staging: AtomicU64,
    in_flight: AtomicU64,
    resident: AtomicU64,
    /// Identity of the upload currently holding resident capacity, so a second admission of the
    /// same representation is refused instead of counted twice.
    resident_identities: Mutex<Vec<([u8; 32], u64, u64)>>,
}

impl UploadPool {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn usage(&self) -> (u64, u64, u64) {
        (
            self.staging.load(Ordering::SeqCst),
            self.in_flight.load(Ordering::SeqCst),
            self.resident.load(Ordering::SeqCst),
        )
    }

    fn release(&self, staging: u64, in_flight: u64, resident: u64) {
        self.staging.fetch_sub(staging, Ordering::SeqCst);
        self.in_flight.fetch_sub(in_flight, Ordering::SeqCst);
        self.resident.fetch_sub(resident, Ordering::SeqCst);
    }

    fn identity_key(identity: &UploadIdentity) -> ([u8; 32], u64, u64) {
        (
            identity.content_digest,
            identity.source_revision,
            identity.device_generation,
        )
    }
}

/// Admit a decoded raster for upload, or refuse it with a closed reason.
///
/// `budget` bounds each counter independently; the bytes requested are the raster's own size for
/// staging and in-flight (the CPU copy and the transfer in progress) and the same size again for
/// residency, because the device copy outlives both.
pub fn admit_upload(
    raster: &DecodedRaster,
    class: VerifiedSourceClass,
    identity: UploadIdentity,
    current_device_generation: u64,
    budget: UploadBudget,
    pool: &Arc<UploadPool>,
    cancel: &Cancellation,
) -> Result<AdmittedUpload, UploadRefusal> {
    if cancel.is_cancelled() {
        return Err(UploadRefusal::Cancelled);
    }
    if class.size != raster.size || !class.is_approved_class() {
        return Err(UploadRefusal::UnverifiedClass);
    }
    let expected = raster
        .size
        .checked_rgba_bytes()
        .map_err(|_| UploadRefusal::WrongLayout)?;
    if raster.rgba.len() as u64 != expected {
        return Err(UploadRefusal::WrongLayout);
    }
    if identity.device_generation < current_device_generation {
        return Err(UploadRefusal::StaleGeneration {
            requested: identity.device_generation,
            current: current_device_generation,
        });
    }

    let bytes = expected;
    let stride = raster
        .size
        .width()
        .checked_mul(4)
        .ok_or(UploadRefusal::WrongLayout)?;

    // Compute everything before mutating, so a refusal leaves the pool untouched.
    let (staging_now, in_flight_now, resident_now) = pool.usage();
    let staging_over = staging_now.saturating_add(bytes) > budget.staging_bytes;
    let in_flight_over = in_flight_now.saturating_add(bytes) > budget.in_flight_bytes;
    let resident_over = resident_now.saturating_add(bytes) > budget.resident_bytes;
    if staging_over || in_flight_over || resident_over {
        return Err(UploadRefusal::BudgetExceeded {
            staging: staging_over,
            in_flight: in_flight_over,
            resident: resident_over,
        });
    }

    // Same representation, same source revision and same device: already resident.
    let key = UploadPool::identity_key(&identity);
    {
        let mut identities = pool.resident_identities.lock().expect("identities");
        if identities.contains(&key) {
            return Err(UploadRefusal::BudgetExceeded {
                staging: false,
                in_flight: false,
                resident: true,
            });
        }
        identities.push(key);
    }
    pool.staging.fetch_add(bytes, Ordering::SeqCst);
    pool.in_flight.fetch_add(bytes, Ordering::SeqCst);
    pool.resident.fetch_add(bytes, Ordering::SeqCst);

    Ok(AdmittedUpload {
        class,
        identity,
        stride_bytes: stride,
        staging_bytes: bytes,
        in_flight_bytes: bytes,
        resident_bytes: bytes,
        pool: Arc::clone(pool),
    })
}

/// Release hook for the identity table when an admission is dropped.
pub(crate) fn forget_identity(pool: &UploadPool, identity: &UploadIdentity) {
    let key = UploadPool::identity_key(identity);
    let mut identities = pool.resident_identities.lock().expect("identities");
    identities.retain(|entry| *entry != key);
}

impl AdmittedUpload {
    /// Drop this admission and free all three counters together.
    pub fn release(self) {
        // Dropping runs the `Drop` impl, which clears the identity and releases the counters.
        drop(self);
    }
}

/// Convenience: the surface-used error type for callers that only need a reason.
pub fn refusal_reason(refusal: UploadRefusal) -> MediaError {
    match refusal {
        UploadRefusal::UnverifiedClass => MediaError::UnsupportedContainer,
        UploadRefusal::WrongLayout => MediaError::InvalidMetadata,
        UploadRefusal::StaleGeneration { .. } => MediaError::Unavailable,
        UploadRefusal::BudgetExceeded { .. } => MediaError::Busy,
        UploadRefusal::Cancelled => MediaError::Cancelled,
    }
}
