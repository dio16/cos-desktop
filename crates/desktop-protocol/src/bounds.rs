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

#[cfg(test)]
mod tests {
    use super::{
        Limits, MAX_CONTROL_FRAME_BYTES, MAX_QUEUE_BYTES, MAX_QUEUE_FRAMES, MAX_STAGED_BUNDLE_BYTES,
    };

    fn local() -> Limits {
        Limits {
            max_frame_bytes: 1_000,
            max_bundle_bytes: 2_000,
            max_queue_frames: 10,
            max_queue_bytes: 4_000,
        }
    }

    fn above() -> Limits {
        Limits {
            max_frame_bytes: 10_000,
            max_bundle_bytes: 20_000,
            max_queue_frames: 100,
            max_queue_bytes: 40_000,
        }
    }

    fn below() -> Limits {
        Limits {
            max_frame_bytes: 500,
            max_bundle_bytes: 1_000,
            max_queue_frames: 5,
            max_queue_bytes: 2_000,
        }
    }

    #[test]
    fn a_server_may_never_raise_a_limit() {
        // The rule the module exists for: a peer proposing larger bounds must not widen them, on any
        // axis. A raise here would let a peer push the desktop past its declared bounds.
        let negotiated = local().negotiate(Some(above()));
        assert_eq!(negotiated, local(), "a higher proposal must be ignored");
        for axis in [
            (negotiated.max_frame_bytes, local().max_frame_bytes),
            (negotiated.max_bundle_bytes, local().max_bundle_bytes),
            (
                u64::from(negotiated.max_queue_frames),
                u64::from(local().max_queue_frames),
            ),
            (negotiated.max_queue_bytes, local().max_queue_bytes),
        ] {
            assert!(axis.0 <= axis.1, "an axis grew: {axis:?}");
        }
    }

    #[test]
    fn a_server_may_lower_any_subset_of_the_limits() {
        // Per-axis, not all-or-nothing: a server that lowers only the frame bound keeps the others.
        let mixed = Limits {
            max_frame_bytes: 500,
            max_bundle_bytes: 20_000,
            max_queue_frames: 5,
            max_queue_bytes: 40_000,
        };
        let negotiated = local().negotiate(Some(mixed));
        assert_eq!(
            negotiated.max_frame_bytes, 500,
            "the lower frame bound wins"
        );
        assert_eq!(negotiated.max_queue_frames, 5, "the lower frame count wins");
        assert_eq!(
            negotiated.max_bundle_bytes,
            local().max_bundle_bytes,
            "an axis the server raised keeps the local value"
        );
        assert_eq!(negotiated.max_queue_bytes, local().max_queue_bytes);
        // Lowering everything lowers everything.
        assert_eq!(local().negotiate(Some(below())), below());
    }

    #[test]
    fn no_proposal_keeps_the_local_limits_and_default_is_the_contract_default() {
        assert_eq!(local().negotiate(None), local());
        assert_eq!(Limits::default(), Limits::DEFAULT);
        assert_eq!(Limits::DEFAULT.max_frame_bytes, MAX_CONTROL_FRAME_BYTES);
        assert_eq!(Limits::DEFAULT.max_bundle_bytes, MAX_STAGED_BUNDLE_BYTES);
        assert_eq!(Limits::DEFAULT.max_queue_frames, MAX_QUEUE_FRAMES);
        assert_eq!(Limits::DEFAULT.max_queue_bytes, MAX_QUEUE_BYTES);
    }

    #[test]
    fn negotiation_is_idempotent_and_order_independent() {
        // Negotiating twice changes nothing, and two servers proposing different bounds resolve to
        // the same result whichever order they are applied - so a retry cannot widen a limit.
        let once = local().negotiate(Some(below()));
        assert_eq!(
            once.negotiate(Some(below())),
            once,
            "negotiation is idempotent"
        );
        let first = local().negotiate(Some(below()));
        let second = local().negotiate(Some(above()));
        assert_eq!(
            first.negotiate(Some(above())),
            first,
            "a later raise changes nothing"
        );
        assert_eq!(
            second.negotiate(Some(below())),
            below(),
            "a later lowering applies"
        );
    }

    #[test]
    fn the_size_checks_accept_exactly_the_limit() {
        // The boundary is inclusive on both checks: a frame or bundle exactly at its bound fits, one
        // byte more does not. An off-by-one here is a message that is dropped or one that is admitted
        // past the bound.
        let limits = local();
        assert!(limits.frame_fits(0));
        assert!(limits.frame_fits(limits.max_frame_bytes));
        assert!(!limits.frame_fits(limits.max_frame_bytes + 1));
        assert!(limits.bundle_fits(limits.max_bundle_bytes));
        assert!(!limits.bundle_fits(limits.max_bundle_bytes + 1));
        // A declared length of u64::MAX must not pass a smaller bound through any arithmetic.
        assert!(!limits.frame_fits(u64::MAX));
        assert!(!limits.bundle_fits(u64::MAX));
    }
}
