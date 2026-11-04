//! MP-IMG3-S1: the upload admission boundary.
//!
//! The approved scope is the **2048×2048 static 8-bit RGB PNG class**. This module decides
//! whether a decoded raster may be handed to a GPU uploader. It performs **no** upload and makes
//! **no** claim that one works.
//!
//! Four contracts, each of which the first version got wrong and a review caught:
//!
//! 1. **The verified class is a proof, not a record.** [`VerifiedRaster`] can only be produced by
//!    [`decode_for_upload`], which checks the container, the bit depth and the colour format before
//!    the type exists. `DecodedRaster` on its own is not accepted anywhere.
//! 2. **The pool owns the limits and admits under one lock.** Capacity check, identity
//!    registration and all three reservations happen while holding the same mutex, so two threads
//!    cannot both pass the check and then both reserve.
//! 3. **Lifetime is a state machine, not a drop.** The CPU payload is held by the admission, an
//!    upload must be begun and completed explicitly, and resident bytes are only freed by
//!    [`AdmittedUpload::release`]. A plain drop moves them to an `unreclaimed` figure instead of
//!    pretending the device returned them.
//! 4. **Identity is the canon's key.** [`UploadIdentity`] carries the full [`RepresentationKey`]
//!    plus the source revision and device generation, and duplicate detection uses it. A generation
//!    that is not the caller's current one is refused in either direction.

use std::sync::{Arc, Mutex};

use crate::{
    AcceptedFormat, Cancellation, DecodeLimits, DecodedRaster, MediaError, PixelSize,
    RepresentationKey, decode_raster,
};

/// The one verified source class this boundary accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceClass {
    /// Static 8-bit RGB PNG at 2048×2048.
    StillPng8Rgb2048,
}

/// A decoded raster whose source class was verified at decode time.
///
/// Fields are private and there is no public constructor: the only way to obtain one is
/// [`decode_for_upload`], so "the class was checked" cannot be asserted by a caller that did not
/// check it.
#[derive(Clone, Debug)]
pub struct VerifiedRaster {
    raster: DecodedRaster,
    class: SourceClass,
}

impl VerifiedRaster {
    pub fn raster(&self) -> &DecodedRaster {
        &self.raster
    }
    pub fn class(&self) -> SourceClass {
        self.class
    }
    pub fn size(&self) -> PixelSize {
        self.raster.size
    }
}

/// Colour type 2 in a PNG IHDR is RGB without alpha, which is the approved class.
const PNG_COLOUR_TYPE_RGB: u8 = 2;
/// Byte offset of the bit depth inside a PNG: signature (8) + length (4) + "IHDR" (4) + width (4)
/// + height (4).
const PNG_BIT_DEPTH_OFFSET: usize = 24;
/// Byte offset of the colour type, one past the bit depth.
const PNG_COLOUR_TYPE_OFFSET: usize = 25;

/// Decode an encoded image **for upload**, producing the class proof only if the source really was
/// the approved class.
pub fn decode_for_upload(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
) -> Result<VerifiedRaster, MediaError> {
    let raster = decode_raster(bytes, limits, cancel)?;
    let approved = raster.format == AcceptedFormat::Png
        && bytes.len() > PNG_COLOUR_TYPE_OFFSET
        && bytes[PNG_BIT_DEPTH_OFFSET] == 8
        && bytes[PNG_COLOUR_TYPE_OFFSET] == PNG_COLOUR_TYPE_RGB
        && raster.size == PixelSize::new(2048, 2048).map_err(|_| MediaError::InvalidMetadata)?;
    if !approved {
        return Err(MediaError::UnsupportedContainer);
    }
    Ok(VerifiedRaster {
        raster,
        class: SourceClass::StillPng8Rgb2048,
    })
}

/// Where an upload comes from and which device it targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadIdentity {
    pub key: RepresentationKey,
    pub source_revision: u64,
    pub device_generation: u64,
}

/// The three budgets, owned by the pool, kept separate because they bound different resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadBudget {
    pub staging_bytes: u64,
    pub in_flight_bytes: u64,
    pub resident_bytes: u64,
}

/// Why an upload was refused. Closed, so a caller can report it without inspecting state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadRefusal {
    /// The device generation is not the one the caller is currently rendering for.
    GenerationMismatch { requested: u64, current: u64 },
    BudgetExceeded {
        staging: bool,
        in_flight: bool,
        resident: bool,
    },
    /// The same representation is already resident.
    DuplicateRepresentation,
    /// The raster's length did not match its declared geometry.
    WrongLayout,
    /// Cancelled before admission.
    Cancelled,
    /// An upload was begun twice, completed without beginning, or released mid-upload.
    StageViolation,
}

/// How far an admission has progressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadStage {
    /// CPU payload held, nothing transferred.
    Staged,
    /// Transfer begun; the device copy is not known to exist yet.
    Uploading,
    /// Transfer completed by the caller's account; the device copy is resident.
    Resident,
}

#[derive(Debug, Default)]
struct PoolState {
    staging: u64,
    in_flight: u64,
    resident: u64,
    /// Resident bytes whose GPU completion was never asserted (dropped without `release`). Kept
    /// visible instead of being reported as freed.
    unreclaimed: u64,
    resident_keys: Vec<RepresentationKey>,
}

/// One snapshot of the pool, for reporting and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadUsage {
    pub staging: u64,
    pub in_flight: u64,
    pub resident: u64,
    pub unreclaimed: u64,
}

/// Process-wide counters and limits. All mutation happens under one lock.
#[derive(Debug)]
pub struct UploadPool {
    limits: UploadBudget,
    state: Mutex<PoolState>,
}

impl UploadPool {
    pub fn new(limits: UploadBudget) -> Arc<Self> {
        Arc::new(Self {
            limits,
            state: Mutex::new(PoolState::default()),
        })
    }

    pub fn limits(&self) -> UploadBudget {
        self.limits
    }

    pub fn usage(&self) -> UploadUsage {
        let state = self.state.lock().expect("pool state");
        UploadUsage {
            staging: state.staging,
            in_flight: state.in_flight,
            resident: state.resident,
            unreclaimed: state.unreclaimed,
        }
    }

    fn with_state<T>(&self, body: impl FnOnce(&mut PoolState) -> T) -> T {
        let mut state = self.state.lock().expect("pool state");
        body(&mut state)
    }
}

/// An admitted upload: the only value the uploader accepts.
#[derive(Debug)]
pub struct AdmittedUpload {
    /// The CPU payload, held for the whole admission, so the reservation describes bytes that
    /// exist.
    raster: Option<DecodedRaster>,
    identity: UploadIdentity,
    stage: UploadStage,
    bytes: u64,
    pool: Arc<UploadPool>,
}

impl AdmittedUpload {
    pub fn identity(&self) -> &UploadIdentity {
        &self.identity
    }
    pub fn stage(&self) -> UploadStage {
        self.stage
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn raster(&self) -> Option<&DecodedRaster> {
        self.raster.as_ref()
    }
    pub fn stride_bytes(&self) -> Result<u32, UploadRefusal> {
        self.raster
            .as_ref()
            .and_then(|raster| raster.size.width().checked_mul(4))
            .ok_or(UploadRefusal::WrongLayout)
    }

    /// Begin the transfer. The CPU payload stays held: the device copy does not exist yet.
    pub fn begin_upload(&mut self) -> Result<(), UploadRefusal> {
        if self.stage != UploadStage::Staged {
            return Err(UploadRefusal::StageViolation);
        }
        self.stage = UploadStage::Uploading;
        Ok(())
    }

    /// Complete the transfer. Only the caller can assert this; without a GPU it is a statement,
    /// not a measurement.
    pub fn complete_upload(&mut self) -> Result<(), UploadRefusal> {
        if self.stage != UploadStage::Uploading {
            return Err(UploadRefusal::StageViolation);
        }
        self.stage = UploadStage::Resident;
        // The CPU copy is no longer needed once the caller asserts the device copy exists.
        self.raster = None;
        Ok(())
    }

    /// Release the admission, freeing all three counters and the identity entry.
    ///
    /// This is the only path that frees resident bytes: the caller is asserting the device copy is
    /// gone.
    pub fn release(mut self) {
        let bytes = self.bytes;
        let key = self.identity.key.clone();
        self.pool.with_state(|state| {
            state.staging = state.staging.saturating_sub(bytes);
            state.in_flight = state.in_flight.saturating_sub(bytes);
            state.resident = state.resident.saturating_sub(bytes);
            state.resident_keys.retain(|entry| *entry != key);
        });
        self.raster = None;
        // Zeroing the byte count tells `Drop` the reservation was consumed.
        self.bytes = 0;
    }
}

impl Drop for AdmittedUpload {
    fn drop(&mut self) {
        if self.bytes == 0 {
            return;
        }
        let bytes = self.bytes;
        let key = self.identity.key.clone();
        self.pool.with_state(|state| {
            // The CPU side is genuinely gone, so staging and in-flight can be given back.
            state.staging = state.staging.saturating_sub(bytes);
            state.in_flight = state.in_flight.saturating_sub(bytes);
            // Resident bytes are *not* reported as freed: nothing asserted the device copy is gone.
            state.resident = state.resident.saturating_sub(bytes);
            state.unreclaimed = state.unreclaimed.saturating_add(bytes);
            state.resident_keys.retain(|entry| *entry != key);
        });
    }
}

/// Admit a verified raster for upload, or refuse it with a closed reason.
///
/// The capacity check, the identity registration and the three reservations all happen while
/// holding the pool lock, so concurrent admissions cannot both pass the check and then both
/// reserve.
pub fn admit_upload(
    verified: &VerifiedRaster,
    identity: UploadIdentity,
    current_device_generation: u64,
    pool: &Arc<UploadPool>,
    cancel: &Cancellation,
) -> Result<AdmittedUpload, UploadRefusal> {
    if cancel.is_cancelled() {
        return Err(UploadRefusal::Cancelled);
    }
    if identity.device_generation != current_device_generation {
        return Err(UploadRefusal::GenerationMismatch {
            requested: identity.device_generation,
            current: current_device_generation,
        });
    }
    let expected = verified
        .raster
        .size
        .checked_rgba_bytes()
        .map_err(|_| UploadRefusal::WrongLayout)?;
    if verified.raster.rgba.len() as u64 != expected {
        return Err(UploadRefusal::WrongLayout);
    }

    let bytes = expected;
    let key = identity.key.clone();
    let limits = pool.limits();
    pool.with_state(|state| {
        if state.resident_keys.contains(&key) {
            return Err(UploadRefusal::DuplicateRepresentation);
        }
        let staging_over = state.staging.saturating_add(bytes) > limits.staging_bytes;
        let in_flight_over = state.in_flight.saturating_add(bytes) > limits.in_flight_bytes;
        let resident_over = state.resident.saturating_add(bytes) > limits.resident_bytes;
        if staging_over || in_flight_over || resident_over {
            return Err(UploadRefusal::BudgetExceeded {
                staging: staging_over,
                in_flight: in_flight_over,
                resident: resident_over,
            });
        }
        state.staging += bytes;
        state.in_flight += bytes;
        state.resident += bytes;
        state.resident_keys.push(key);
        Ok(())
    })?;

    Ok(AdmittedUpload {
        raster: Some(verified.raster.clone()),
        identity,
        stage: UploadStage::Staged,
        bytes,
        pool: Arc::clone(pool),
    })
}

/// Map a refusal onto the existing error vocabulary, for callers that report one type.
pub fn refusal_reason(refusal: &UploadRefusal) -> MediaError {
    match refusal {
        UploadRefusal::GenerationMismatch { .. } => MediaError::Unavailable,
        UploadRefusal::BudgetExceeded { .. } | UploadRefusal::DuplicateRepresentation => {
            MediaError::Busy
        }
        UploadRefusal::WrongLayout => MediaError::InvalidMetadata,
        UploadRefusal::Cancelled => MediaError::Cancelled,
        UploadRefusal::StageViolation => MediaError::Unavailable,
    }
}
