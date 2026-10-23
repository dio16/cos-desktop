//! Wire bounds.
//!
//! Draft defaults from the protocol: 1 MiB maximum control frame, 8 MiB maximum
//! staged snapshot/transaction bundle, receive queue at most 256 frames or
//! 8 MiB, whichever is reached first. Limits include decoded allocation
//! accounting: oversized declared lengths are rejected before allocating.
//! Later revisions may negotiate *lower* bounds; raising ceilings requires
//! measurement and review.

use serde::{Deserialize, Serialize};

/// 1 MiB maximum control frame.
pub const MAX_CONTROL_FRAME_BYTES: u64 = 1024 * 1024;
/// 8 MiB maximum staged snapshot/transaction bundle.
pub const MAX_STAGED_BUNDLE_BYTES: u64 = 8 * 1024 * 1024;
/// Receive queue bound in frames.
pub const MAX_QUEUE_FRAMES: u32 = 256;
/// Receive queue bound in bytes.
pub const MAX_QUEUE_BYTES: u64 = 8 * 1024 * 1024;

/// Negotiable session bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum control frame size in bytes.
    pub max_frame_bytes: u64,
    /// Maximum staged snapshot/transaction bundle size in bytes.
    pub max_bundle_bytes: u64,
    /// Maximum receive queue length in frames.
    pub max_queue_frames: u32,
    /// Maximum receive queue size in bytes.
    pub max_queue_bytes: u64,
}

impl Limits {
    /// Contract defaults.
    pub const DEFAULT: Limits = Limits {
        max_frame_bytes: MAX_CONTROL_FRAME_BYTES,
        max_bundle_bytes: MAX_STAGED_BUNDLE_BYTES,
        max_queue_frames: MAX_QUEUE_FRAMES,
        max_queue_bytes: MAX_QUEUE_BYTES,
    };

    /// Negotiate against a server-proposed set of limits.
    ///
    /// The result never exceeds the local limits; a server may only lower them.
    pub fn negotiate(self, server: Option<Limits>) -> Limits {
        match server {
            None => self,
            Some(server) => Limits {
                max_frame_bytes: self.max_frame_bytes.min(server.max_frame_bytes),
                max_bundle_bytes: self.max_bundle_bytes.min(server.max_bundle_bytes),
                max_queue_frames: self.max_queue_frames.min(server.max_queue_frames),
                max_queue_bytes: self.max_queue_bytes.min(server.max_queue_bytes),
            },
        }
    }

    /// Whether the declared frame length is within bounds.
    pub fn frame_fits(&self, declared_bytes: u64) -> bool {
        declared_bytes <= self.max_frame_bytes
    }

    /// Whether the accumulated bundle size is within bounds.
    pub fn bundle_fits(&self, accumulated_bytes: u64) -> bool {
        accumulated_bytes <= self.max_bundle_bytes
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
