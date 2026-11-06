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
    RepresentationKey, RepresentationSource, decode_raster,
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
    raster: Arc<DecodedRaster>,
    class: SourceClass,
    content_digest: [u8; 32],
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
    /// SHA-256 of the decoded bytes, computed once when this representation is created.
    pub fn content_digest(&self) -> [u8; 32] {
        self.content_digest
    }

    /// Derive a power-of-two mip representation from the original verified RGBA8 pixels.
    ///
    /// `class` remains proof about the decoded 2048×2048 source, not this derivative; `size`,
    /// `raster().rgba`, and `content_digest()` describe the returned representation. Pixel values
    /// are treated as raw RGBA8 samples: this operation makes no ICC or colour-fidelity claim.
    /// Levels 0 through 4 produce 2048, 1024, 512, 256, and 128 square pixels respectively.
    /// Every nonzero level uses exact integer area averaging over each source footprint, rounded
    /// to nearest (ties upward), including alpha. Each level is computed directly from this raster.
    pub fn mip(&self, level: u8, cancel: &Cancellation) -> Result<VerifiedRaster, MediaError> {
        let shift = u32::from(level);
        if level > 4
            || self.raster.size
                != PixelSize::new(APPROVED_EDGE, APPROVED_EDGE)
                    .map_err(|_| MediaError::InvalidMetadata)?
        {
            return Err(MediaError::InvalidMetadata);
        }
        cancel.check()?;
        if level == 0 {
            return Ok(self.clone());
        }

        let edge = APPROVED_EDGE >> shift;
        let size = PixelSize::new(edge, edge).map_err(|_| MediaError::InvalidMetadata)?;
        let pixel_count = (edge as usize)
            .checked_mul(edge as usize)
            .ok_or(MediaError::TooLarge)?;
        let byte_count = pixel_count.checked_mul(4).ok_or(MediaError::TooLarge)?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(byte_count)
            .map_err(|_| MediaError::Allocation)?;
        rgba.resize(byte_count, 0);

        let source_edge = APPROVED_EDGE as usize;
        let scale = 1usize << shift;
        let area = (scale * scale) as u64;
        for y in 0..edge as usize {
            cancel.check()?;
            for x in 0..edge as usize {
                let mut sums = [0u64; 4];
                for dy in 0..scale {
                    let source_row = (y * scale + dy) * source_edge;
                    for dx in 0..scale {
                        let offset = (source_row + x * scale + dx) * 4;
                        for (channel, sum) in sums.iter_mut().enumerate() {
                            *sum += u64::from(self.raster.rgba[offset + channel]);
                        }
                    }
                }
                let target = (y * edge as usize + x) * 4;
                for (channel, sum) in sums.into_iter().enumerate() {
                    rgba[target + channel] = ((sum + area / 2) / area) as u8;
                }
            }
        }
        cancel.check()?;
        let content_digest = digest_of(&rgba);
        Ok(VerifiedRaster {
            raster: Arc::new(DecodedRaster {
                size,
                format: self.raster.format,
                rgba,
            }),
            class: self.class,
            content_digest,
        })
    }
}

/// Colour type 2 in a PNG IHDR is RGB without alpha, which is the approved class.
const PNG_COLOUR_TYPE_RGB: u8 = 2;
/// Byte offset of the bit depth inside a PNG: signature (8) + length (4) + "IHDR" (4) + width (4)
/// + height (4).
const PNG_BIT_DEPTH_OFFSET: usize = 24;
/// Byte offset of the colour type, one past the bit depth.
const PNG_COLOUR_TYPE_OFFSET: usize = 25;
/// Byte offset of the interlace method, one past the colour type.
const PNG_INTERLACE_OFFSET: usize = 28;
/// The single approved edge length.
const APPROVED_EDGE: u32 = 2048;

/// Decode an encoded image **for upload**, producing the class proof only if the source really was
/// the approved class.
pub fn decode_for_upload(
    bytes: &[u8],
    limits: DecodeLimits,
    cancel: &Cancellation,
) -> Result<VerifiedRaster, MediaError> {
    // Decide what can be decided from the header, before paying for a decode. The approved class
    // is a non-interlaced PNG: the running-decoder cancellation evidence behind this approval is
    // non-interlaced only, so an Adam7 image must not acquire the proof.
    if bytes.len() <= PNG_INTERLACE_OFFSET {
        return Err(MediaError::UnsupportedContainer);
    }
    if &bytes[12..16] != b"IHDR" {
        return Err(MediaError::UnsupportedContainer);
    }
    if bytes[PNG_BIT_DEPTH_OFFSET] != 8
        || bytes[PNG_COLOUR_TYPE_OFFSET] != PNG_COLOUR_TYPE_RGB
        || bytes[PNG_INTERLACE_OFFSET] != 0
    {
        return Err(MediaError::UnsupportedContainer);
    }
    // Dimensions too: an out-of-class image must be refused before the decoder expands it, not
    // after paying for a full raster.
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width != APPROVED_EDGE || height != APPROVED_EDGE {
        return Err(MediaError::UnsupportedContainer);
    }
    let raster = decode_raster(bytes, limits, cancel)?;
    let approved = raster.format == AcceptedFormat::Png
        && raster.size == PixelSize::new(2048, 2048).map_err(|_| MediaError::InvalidMetadata)?;
    if !approved {
        return Err(MediaError::UnsupportedContainer);
    }
    let content_digest = digest_of(&raster.rgba);
    Ok(VerifiedRaster {
        raster: Arc::new(raster),
        class: SourceClass::StillPng8Rgb2048,
        content_digest,
    })
}

/// Where an upload comes from and which device it targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadIdentity {
    pub key: RepresentationKey,
    pub source_revision: u64,
    pub device_generation: u64,
    /// SHA-256 of the decoded RGBA8 bytes. Requiring it here is what ties the identity to the
    /// actual pixels: a key alone could describe any raster of the same size.
    pub content_digest: [u8; 32],
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
    /// Released or aborted. Terminal: no further transition is accepted.
    Released,
}

#[derive(Debug, Default)]
struct PoolState {
    staging: u64,
    in_flight: u64,
    resident: u64,
    /// Bytes whose release was never confirmed, tracked **per resource** so each of the three
    /// limits is charged for what is stuck in its own category. A single combined figure would let
    /// a stuck transfer free staging or in-flight capacity it still occupies.
    staging_stuck: u64,
    in_flight_stuck: u64,
    resident_stuck: u64,
    /// CPU payloads whose transfer was abandoned mid-flight, with the identity and generation they
    /// belong to. A payload is only released by an explicit
    /// [`UploadPool::confirm_transfer_terminated`] for that identity, never by a caller taking the
    /// bytes away.
    orphaned_payloads: Vec<OrphanedTransfer>,
    resident_keys: Vec<RepresentationKey>,
    /// Keys whose resident bytes are quarantined. Reuse is refused until a safe release is
    /// asserted, so the same representation cannot be admitted twice on the device.
    unreclaimed_keys: Vec<RepresentationKey>,
}

/// A payload held for an abandoned transfer, kept with the identity it belongs to.
#[derive(Debug)]
struct OrphanedTransfer {
    identity: UploadIdentity,
    raster: Arc<DecodedRaster>,
}

/// Read-only description of what is held for abandoned transfers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanedSummary {
    pub key: RepresentationKey,
    pub device_generation: u64,
    pub bytes: u64,
}

/// One snapshot of the pool, for reporting and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadUsage {
    pub staging: u64,
    pub in_flight: u64,
    pub resident: u64,
    /// Stuck bytes, per resource. Each category is charged to its own limit.
    pub staging_stuck: u64,
    pub in_flight_stuck: u64,
    pub resident_stuck: u64,
    /// Frames stuck on the device: the resident-category figure, which is what a device budget
    /// cares about. The three categories can each hold the same frame, so their sum is not a
    /// device figure.
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
            staging_stuck: state.staging_stuck,
            in_flight_stuck: state.in_flight_stuck,
            resident_stuck: state.resident_stuck,
            unreclaimed: state.resident_stuck,
        }
    }

    /// What is currently held for abandoned transfers. Read-only: there is no call that hands the
    /// bytes out, because a caller could drop them while a transfer still reads them.
    pub fn orphaned(&self) -> Vec<OrphanedSummary> {
        let state = self.state.lock().expect("pool state");
        state
            .orphaned_payloads
            .iter()
            .map(|held| OrphanedSummary {
                key: held.identity.key.clone(),
                device_generation: held.identity.device_generation,
                bytes: held.raster.rgba.len() as u64,
            })
            .collect()
    }

    /// Assert that the transfer for this identity has terminated, releasing the payload and the
    /// quarantined bytes for it.
    ///
    /// This is the only way an abandoned payload is freed. Without a GPU it is the caller's
    /// assertion, not a measurement, and the module says so rather than pretending to confirm it.
    pub fn confirm_transfer_terminated(
        &self,
        identity: &UploadIdentity,
    ) -> Result<(), UploadRefusal> {
        let mut state = self.state.lock().expect("pool state");
        let before = state.orphaned_payloads.len();
        let mut released_bytes = None;
        state.orphaned_payloads.retain(|held| {
            if held.identity == *identity {
                released_bytes = Some(held.raster.rgba.len() as u64);
                false
            } else {
                true
            }
        });
        if state.orphaned_payloads.len() == before {
            return Err(UploadRefusal::StageViolation);
        }
        if let Some(bytes) = released_bytes {
            state.staging_stuck = state.staging_stuck.saturating_sub(bytes);
            state.in_flight_stuck = state.in_flight_stuck.saturating_sub(bytes);
            state.resident_stuck = state.resident_stuck.saturating_sub(bytes);
        }
        state
            .unreclaimed_keys
            .retain(|entry| *entry != identity.key);
        Ok(())
    }

    /// Whether any per-resource quarantine is non-empty.
    pub fn has_quarantine(&self) -> bool {
        let state = self.state.lock().expect("pool state");
        state.staging_stuck + state.in_flight_stuck + state.resident_stuck > 0
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
    raster: Option<Arc<DecodedRaster>>,
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
        self.raster.as_deref()
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

    /// Abort a staged admission, or release a resident one.
    ///
    /// The two cases are deliberately separate in effect: a `Staged` admission never transferred
    /// anything, so all three counters go back; a `Resident` one is the caller asserting the device
    /// copy is gone. Releasing mid-transfer is refused **without consuming the admission** - the
    /// caller keeps ownership, so in-flight bytes cannot be handed to another transfer by a failed
    /// call followed by a drop.
    pub fn release(&mut self) -> Result<(), UploadRefusal> {
        if matches!(self.stage, UploadStage::Uploading | UploadStage::Released) {
            return Err(UploadRefusal::StageViolation);
        }
        let bytes = self.bytes;
        let key = self.identity.key.clone();
        self.pool.with_state(|state| {
            state.staging = state.staging.saturating_sub(bytes);
            state.in_flight = state.in_flight.saturating_sub(bytes);
            state.resident = state.resident.saturating_sub(bytes);
            state.resident_keys.retain(|entry| *entry != key);
        });
        self.raster = None;
        // Terminal: a second release, or any later transition, is refused instead of touching a
        // key that may by then belong to another admission.
        self.stage = UploadStage::Released;
        // Zeroing the byte count tells `Drop` the reservation was consumed.
        self.bytes = 0;
        Ok(())
    }
}

impl Drop for AdmittedUpload {
    fn drop(&mut self) {
        if self.bytes == 0 {
            return;
        }
        let bytes = self.bytes;
        let key = self.identity.key.clone();
        let identity = self.identity.clone();
        let stage = self.stage;
        // An abandoned transfer keeps its payload: the bytes a device transfer may still read must
        // outlive this value.
        let orphan = if stage == UploadStage::Uploading {
            self.raster.take()
        } else {
            None
        };
        self.pool.with_state(|state| {
            match stage {
                // Never transferred: the CPU side is gone and nothing reached the device, so all three
                // counters can be given back without quarantining anything.
                UploadStage::Staged => {
                    state.staging = state.staging.saturating_sub(bytes);
                    state.in_flight = state.in_flight.saturating_sub(bytes);
                    state.resident = state.resident.saturating_sub(bytes);
                    state.resident_keys.retain(|entry| *entry != key);
                }
                // Mid-transfer: the transfer may still be running. The reservation moves out of the
                // live counters into the per-resource quarantine, and the CPU payload moves to the pool
                // rather than being freed, so a future asynchronous uploader cannot lose the bytes it
                // may still be reading.
                UploadStage::Uploading => {
                    state.staging = state.staging.saturating_sub(bytes);
                    state.in_flight = state.in_flight.saturating_sub(bytes);
                    state.resident = state.resident.saturating_sub(bytes);
                    state.staging_stuck = state.staging_stuck.saturating_add(bytes);
                    state.in_flight_stuck = state.in_flight_stuck.saturating_add(bytes);
                    state.resident_stuck = state.resident_stuck.saturating_add(bytes);
                    state.resident_keys.retain(|entry| *entry != key);
                    if !state.unreclaimed_keys.contains(&key) {
                        state.unreclaimed_keys.push(key);
                    }
                }
                // Resident: the CPU copy is gone, but nothing asserted the device copy was released.
                UploadStage::Resident => {
                    state.staging = state.staging.saturating_sub(bytes);
                    state.in_flight = state.in_flight.saturating_sub(bytes);
                    state.resident = state.resident.saturating_sub(bytes);
                    state.resident_stuck = state.resident_stuck.saturating_add(bytes);
                    state.resident_keys.retain(|entry| *entry != key);
                    if !state.unreclaimed_keys.contains(&key) {
                        state.unreclaimed_keys.push(key);
                    }
                }
                // Already released: `release` zeroed the byte count, so this arm is unreachable in
                // practice; it exists so the state machine is exhaustive.
                UploadStage::Released => {}
            }
            // The payload is registered in the same locked section as the counters and the
            // identity, so no observer can see quarantined bytes whose payload is missing.
            if let Some(raster) = orphan {
                state
                    .orphaned_payloads
                    .push(OrphanedTransfer { identity, raster });
            }
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
    // The identity must describe this raster: a key whose size disagrees with the pixels would let
    // a caller label one representation as another.
    if identity.key.size != verified.raster.size {
        return Err(UploadRefusal::WrongLayout);
    }
    if identity.content_digest != verified.content_digest {
        return Err(UploadRefusal::WrongLayout);
    }
    match revision_of(&identity.key) {
        Some(revision) if revision == identity.source_revision => {}
        _ => return Err(UploadRefusal::WrongLayout),
    }
    pool.with_state(|state| {
        if state.resident_keys.contains(&key) || state.unreclaimed_keys.contains(&key) {
            return Err(UploadRefusal::DuplicateRepresentation);
        }
        // Every limit is charged for what is stuck in its own category: a quarantined transfer
        // still occupies staging and in-flight capacity, and a quarantined frame still occupies the
        // device, so none of them is free to hand out.
        let staging_over = state
            .staging
            .saturating_add(state.staging_stuck)
            .saturating_add(bytes)
            > limits.staging_bytes;
        let in_flight_over = state
            .in_flight
            .saturating_add(state.in_flight_stuck)
            .saturating_add(bytes)
            > limits.in_flight_bytes;
        let resident_over = state
            .resident
            .saturating_add(state.resident_stuck)
            .saturating_add(bytes)
            > limits.resident_bytes;
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
        raster: Some(Arc::clone(&verified.raster)),
        identity,
        stage: UploadStage::Staged,
        bytes,
        pool: Arc::clone(pool),
    })
}

/// The revision carried by a representation key, when this boundary can verify it.
///
/// Only a local source carries a revision that a decoded raster can be checked against; an
/// artifact source has no equivalent here, so admitting one would mean trusting a value nobody
/// verified.
fn revision_of(key: &RepresentationKey) -> Option<u64> {
    match &key.source {
        RepresentationSource::Local { revision, .. } => Some(*revision),
        RepresentationSource::Artifact { .. } => None,
    }
}

/// SHA-256 of RGBA8 bytes, so an identity can be tied to the pixels it describes.
fn digest_of(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
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
