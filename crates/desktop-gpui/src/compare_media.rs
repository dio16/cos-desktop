//! Pair-driven media acquisition for the Compare surface.
//!
//! Compare owns no canonical state: the pair is a local presentation value and the
//! publication is the only source of the artifact, its availability and the capability
//! decision. This module derives one pane's presentation decision from those values and
//! keeps the per-side load identity, so a changed candidate (or a changed capability
//! revision) is a new resolution instead of a reused picture.

use std::sync::Arc;

use app_core::capability::CapabilityProjection;
use app_core::{MediaFetchCancellation, MediaSelection, MediaSessionContext};
use desktop_protocol::{ArtifactSnapshot, CapabilitiesId, Counter, RepresentationKind};

use crate::media_capability_key;

/// One compare side's presentation decision, derived only from the projection.
///
/// No variant invents an artifact or an availability: a candidate the projection does
/// not carry, a candidate without an artifact, and a withheld representation each stay
/// an explicit state with its own sentence.
#[derive(Debug)]
pub enum CompareSideMedia {
    /// The candidate itself is not in the complete projection.
    NotProjected,
    /// The candidate is projected, but the complete projection carries no artifact.
    MissingArtifact,
    /// The authoritative capability projection withholds the original representation.
    Refused(Arc<str>),
    /// The artifact is addressable; the selection is the exact identity to resolve.
    Ready(Box<CompareSideReady>),
}

/// The identity one addressable compare side resolves.
#[derive(Debug)]
pub struct CompareSideReady {
    /// Exact artifact representation to resolve.
    pub selection: MediaSelection,
    /// Capability identity the request was gated on, when the projection carries one.
    pub capability: Option<(CapabilitiesId, Counter)>,
}

/// Decide one compare side from projection values only.
///
/// `projected` says whether the candidate is in the complete projection, `artifact`
/// whether that projection also carries its artifact. The capability projection is the
/// same one the artifact surface gates on, so a representation withheld there is never
/// offered here.
pub fn side_media(
    projected: bool,
    artifact: Option<&ArtifactSnapshot>,
    capability: &CapabilityProjection,
    session: MediaSessionContext,
) -> CompareSideMedia {
    if !projected {
        return CompareSideMedia::NotProjected;
    }
    let Some(artifact) = artifact else {
        return CompareSideMedia::MissingArtifact;
    };
    let gate = capability.gate_media(&RepresentationKind::Original, &artifact.artifact_ref);
    if !gate.enabled {
        return CompareSideMedia::Refused(Arc::clone(&gate.reason));
    }
    CompareSideMedia::Ready(Box::new(CompareSideReady {
        selection: MediaSelection::for_artifact(artifact, session, RepresentationKind::Original),
        capability: media_capability_key(capability),
    }))
}

/// Per-side identity the compare surface currently addresses.
///
/// `keys` is what the projection says each pane should show; `dispatched` is the
/// resolution already started for that identity. The two differ in exactly one case:
/// a resolution that failed leaves `keys` populated (so a later resolution for the same
/// identity does not need a new gate decision) while `dispatched` still blocks an
/// unbounded retry loop on the same event. Both are cleared whenever the candidate, the
/// pair or the capability identity changes, so stale work can never win.
#[derive(Default)]
pub struct CompareMediaState {
    keys: [Option<crate::MediaLoadKey>; 2],
    dispatched: [Option<crate::MediaLoadKey>; 2],
    cancels: [MediaFetchCancellation; 2],
}

impl CompareMediaState {
    /// Identity this side currently addresses.
    pub fn addressed(&self, index: usize) -> Option<&crate::MediaLoadKey> {
        self.keys[index].as_ref()
    }

    /// Whether a resolution was already started for this identity.
    pub fn already_dispatched(&self, index: usize, key: &crate::MediaLoadKey) -> bool {
        self.dispatched[index].as_ref() == Some(key)
    }

    /// Start a new resolution for this side and return its cancellation token.
    ///
    /// A previous resolution for the side is cancelled first: its result would belong to
    /// an identity the pane no longer shows.
    pub fn renew(&mut self, index: usize, key: crate::MediaLoadKey) -> MediaFetchCancellation {
        self.cancels[index].cancel();
        self.cancels[index] = MediaFetchCancellation::default();
        self.keys[index] = Some(key.clone());
        self.dispatched[index] = Some(key);
        self.cancels[index].clone()
    }

    /// Drop this side's identity and cancel its resolution without touching its frame.
    pub fn forget_side(&mut self, index: usize) {
        self.cancels[index].cancel();
        self.cancels[index] = MediaFetchCancellation::default();
        self.keys[index] = None;
        self.dispatched[index] = None;
    }

    /// Drop both sides; used when Compare is closed, the task changes or no pair is held.
    pub fn forget_all(&mut self) {
        self.forget_side(0);
        self.forget_side(1);
    }

    /// Swap only the per-side identity. The panes' frames and availability are swapped by
    /// the view; a side that ends up without a frame loses its identity so the next sync
    /// resolves the candidate now under its label.
    pub fn swap(&mut self) {
        self.cancels[0].cancel();
        self.cancels[0] = MediaFetchCancellation::default();
        self.cancels[1].cancel();
        self.cancels[1] = MediaFetchCancellation::default();
        self.keys.swap(0, 1);
        self.dispatched.swap(0, 1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use app_core::MediaSessionContext;
    use app_core::capability::{
        CapabilityProjection, CapabilityScopeView, CapabilityView, MediaSupportView,
    };
    use desktop_protocol::{
        ArtifactDigest, ArtifactId, ArtifactSnapshot, Availability, CapabilitiesId, Counter,
        EpochId, RepresentationKind, ServerId, SupportState, TaskId,
    };

    use super::{CompareMediaState, CompareSideMedia, side_media};
    use crate::MediaLoadKey;

    fn session() -> MediaSessionContext {
        MediaSessionContext::new(
            ServerId::fixture("server/local"),
            "fixture:audience/desktop",
            EpochId::fixture("epoch/1"),
            "fixture:read-context/task-184",
        )
    }

    fn artifact(label: &str) -> ArtifactSnapshot {
        ArtifactSnapshot {
            artifact_ref: ArtifactId::fixture(format!("artifact/{label}")),
            artifact_digest: ArtifactDigest::fixture(format!("digest/{label}")),
            media_type: Some("image/png".to_owned()),
            availability: Availability::Available,
            descriptor_refs: Vec::new(),
            verification: None,
            projection_revision: Counter::from_u128(7),
        }
    }

    fn capability(support: SupportState, reason: Option<&str>) -> CapabilityProjection {
        CapabilityProjection::Projected(CapabilityView {
            capabilities_ref: CapabilitiesId::fixture("capabilities/task"),
            scope: CapabilityScopeView::Task(TaskId::fixture("task/Task-184")),
            availability: SupportState::Supported,
            commands: Vec::new().into(),
            media: vec![MediaSupportView {
                representation: RepresentationKind::Original,
                support,
                reason: reason.map(Arc::from),
            }]
            .into(),
            features: Vec::new().into(),
            constraints: Vec::new().into(),
            reason: None,
            revision: Counter::from_u128(3),
        })
    }

    fn key(label: &str) -> MediaLoadKey {
        MediaLoadKey {
            server_identity: ServerId::fixture("server/local"),
            epoch: EpochId::fixture("epoch/1"),
            context_id: desktop_protocol::ReadContextId::fixture("read-context/task-184"),
            audience: "fixture:audience/desktop".to_owned(),
            artifact_ref: ArtifactId::fixture(format!("artifact/{label}")),
            artifact_digest: ArtifactDigest::fixture(format!("digest/{label}")),
            representation: RepresentationKind::Original,
            capability: None,
        }
    }

    #[test]
    fn a_projected_candidate_with_an_admitted_artifact_becomes_a_selection() {
        let artifact = artifact("a");
        let plan = side_media(
            true,
            Some(&artifact),
            &capability(SupportState::Supported, None),
            session(),
        );
        let CompareSideMedia::Ready(ready) = plan else {
            panic!("a supported original must be addressable");
        };
        let selection = &ready.selection;
        assert_eq!(selection.artifact_ref(), &artifact.artifact_ref);
        assert_eq!(selection.representation(), &RepresentationKind::Original);
        assert!(
            ready.capability.is_some(),
            "the gate identity is part of the key"
        );
    }

    #[test]
    fn a_candidate_outside_the_projection_is_not_projected() {
        assert!(matches!(
            side_media(
                false,
                Some(&artifact("a")),
                &capability(SupportState::Supported, None),
                session(),
            ),
            CompareSideMedia::NotProjected
        ));
    }

    #[test]
    fn a_candidate_without_a_projected_artifact_is_explicitly_missing() {
        assert!(matches!(
            side_media(
                true,
                None,
                &capability(SupportState::Supported, None),
                session(),
            ),
            CompareSideMedia::MissingArtifact
        ));
    }

    #[test]
    fn a_withheld_representation_is_refused_with_the_gate_reason() {
        let plan = side_media(
            true,
            Some(&artifact("a")),
            &capability(SupportState::Unavailable, Some("revoked")),
            session(),
        );
        let CompareSideMedia::Refused(reason) = plan else {
            panic!("an unavailable original must be refused");
        };
        assert_eq!(reason.as_ref(), "revoked");
        assert!(matches!(
            side_media(
                true,
                Some(&artifact("a")),
                &CapabilityProjection::NotProjected,
                session(),
            ),
            CompareSideMedia::Refused(_)
        ));
    }

    #[test]
    fn a_new_side_identity_cancels_the_previous_resolution() {
        let mut state = CompareMediaState::default();
        let first = state.renew(0, key("a"));
        assert!(state.addressed(0).is_some());
        assert!(state.already_dispatched(0, &key("a")));
        let second = state.renew(0, key("b"));
        assert!(
            first.is_cancelled(),
            "the superseded resolution is cancelled"
        );
        assert!(!second.is_cancelled());
        assert!(state.already_dispatched(0, &key("b")));
        assert!(!state.already_dispatched(0, &key("a")));
    }

    #[test]
    fn side_resources_swap_without_touching_the_other_side() {
        let mut state = CompareMediaState::default();
        let _a = state.renew(0, key("a"));
        let _b = state.renew(1, key("b"));
        state.swap();
        assert!(state.already_dispatched(0, &key("b")));
        assert!(state.already_dispatched(1, &key("a")));
        state.forget_side(0);
        assert!(state.addressed(0).is_none());
        assert!(state.addressed(1).is_some());
        state.forget_all();
        assert!(state.addressed(0).is_none());
        assert!(state.addressed(1).is_none());
    }

    #[test]
    fn distinct_candidates_never_share_one_load_identity() {
        let first = key("a");
        let second = key("b");
        assert!(first != second);
        let artifact_a = artifact("a");
        let artifact_b = artifact("b");
        let plan = side_media(
            true,
            Some(&artifact_a),
            &capability(SupportState::Supported, None),
            session(),
        );
        let CompareSideMedia::Ready(ready) = plan else {
            panic!("addressable");
        };
        let selection = &ready.selection;
        // The two selections carry their own artifact identity, so panes never collide.
        assert_eq!(selection.artifact_ref(), &artifact_a.artifact_ref);
        assert_ne!(selection.artifact_ref(), &artifact_b.artifact_ref);
    }
}
