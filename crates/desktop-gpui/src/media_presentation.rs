//! RT6/IMG6 presentation adapter for admitted media bytes.
//!
//! `app-core` resolves availability and answers with an explicit state or with
//! bytes that passed remote admission under an authorized descriptor. This
//! module only translates that result into the existing presentation
//! vocabulary: a distinct sentence per non-available state, and a canonical
//! [`UploadRequest`] that the existing preparation service decodes on GPUI's
//! background executor. GPUI texture/resource ownership stays in
//! `image_upload`/`image_viewer`; nothing here fabricates an available state,
//! reads a fixture or performs I/O.

use std::sync::Arc;

use app_core::{AdmittedMedia, MediaAvailability, MediaUnavailable};
use desktop_protocol::RepresentationKind;
use gpui::SharedString;
use media::{
    DecodeBudget, DecodePolicy, LocalId, Orientation, PixelSize, RepresentationKey,
    RepresentationSource,
};

use crate::image_upload::{FRAME_EDGE, ImageUploadError, UploadRequest};

/// The only representation kind this build can decode and present.
///
/// The admitted decode class is the full 2048×2048 RGB8 PNG
/// (`media::decode_for_upload`). A thumbnail/preview representation is a
/// different, smaller payload with its own budget, so it is refused explicitly
/// rather than decoded as if it were the original.
const PRESENTABLE_REPRESENTATION: RepresentationKind = RepresentationKind::Original;

/// Declared processing identity of the one admitted decode class.
const DECODER_ID: &str = "png-rgb8";
const COLOUR_ID: &str = "identity-rgb";
const POLICY_GENERATION: u64 = 1;

/// Why an admitted representation cannot be turned into a presentation request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaRequestRefusal {
    /// The representation kind is not the admitted full-resolution class.
    UnsupportedRepresentation,
    /// The descriptor states a pixel size outside the one admitted decode class.
    UnsupportedGeometry,
    /// The canonical revision cannot be represented by the upload identity.
    UnverifiableRevision,
    /// The media identity cannot be turned into a representation key.
    InvalidIdentity,
}

impl MediaRequestRefusal {
    /// Availability state to render for this refusal.
    pub fn availability(self) -> MediaAvailability {
        match self {
            Self::UnsupportedRepresentation | Self::UnsupportedGeometry => {
                MediaAvailability::Unsupported
            }
            // A revision or identity this build cannot verify is a failure, not "unsupported".
            Self::UnverifiableRevision | Self::InvalidIdentity => MediaAvailability::Failed,
        }
    }

    /// Explicit user-facing sentence. The variant name stays in diagnostics.
    pub fn text(self) -> &'static str {
        match self {
            Self::UnsupportedRepresentation => {
                "画像を利用できません: この表現（サムネイル/プレビュー）のメディアには対応していません。"
            }
            Self::UnsupportedGeometry => {
                "画像を利用できません: 記述子の画像寸法は検証済みの範囲外です。"
            }
            Self::UnverifiableRevision => {
                "画像を利用できません: 成果物の版を確認できないため表示できません。"
            }
            Self::InvalidIdentity => "画像を利用できません: メディアの識別子を構成できません。",
        }
    }
}

/// One explicit sentence per availability state.
///
/// The non-available sentences are distinct from each other and never word a
/// picture as presentable; an unverified success is labelled separately by
/// [`media_success_text`].
pub fn media_state_text(state: MediaAvailability) -> &'static str {
    match state {
        MediaAvailability::Available => "メディア画像を表示できます。",
        MediaAvailability::Expired => {
            "画像を利用できません: メディア記述子の有効期限が切れました。再取得が必要です。"
        }
        MediaAvailability::Missing => {
            "画像を利用できません: この成果物のメディアが見つかりません。"
        }
        MediaAvailability::Forbidden => {
            "画像を利用できません: このセッションにはメディアの閲覧権限がありません。"
        }
        MediaAvailability::Pending => "画像を利用できません: メディアはまだ準備中です。",
        MediaAvailability::Unsupported => {
            "画像を利用できません: この形式または表現のメディアには対応していません。"
        }
        MediaAvailability::Failed => {
            "画像を利用できません: メディアの取得または検証に失敗しました。"
        }
    }
}

/// Render one explicit unavailable state. The value-free cause stays in diagnostics.
pub fn media_unavailable_text(unavailable: &MediaUnavailable) -> SharedString {
    media_state_text(unavailable.availability).into()
}

/// Status while admitted bytes are being decoded on the background executor.
pub fn media_preparing_text(verified: bool) -> SharedString {
    if verified {
        "メディア画像を準備中（記述子のダイジェスト検証済み）".into()
    } else {
        "メディア画像を準備中（記述子は未検証。検証済みの主張はしません）".into()
    }
}

/// Status after the admitted bytes were attached to this pane.
///
/// Verification wording reflects the descriptor alone: an absent expected digest
/// is never reported as verified.
pub fn media_success_text(verified: bool, preview_only: bool) -> SharedString {
    match (verified, preview_only) {
        (true, true) => {
            "メディア画像（記述子のダイジェスト検証済み）: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。"
        }
        (true, false) => {
            "メディア画像（記述子のダイジェスト検証済み）・実験用タイル表示。GPU完了・常駐量・本番の画質受入は未計測。"
        }
        (false, true) => {
            "メディア画像（記述子は未検証）: 低解像度プレビュー。原寸の詳細表示・1:1ではありません。"
        }
        (false, false) => {
            "メディア画像（記述子は未検証）・実験用タイル表示。検証済み・GPU完了・常駐量・本番の画質受入は未計測。"
        }
    }
    .into()
}

/// Map one refused media preparation to an explicit availability state.
///
/// `None` means cancellation, which is not an availability state.
pub fn media_decode_state(error: &ImageUploadError) -> Option<MediaAvailability> {
    Some(match error {
        ImageUploadError::Cancelled => return None,
        // The pinned decode class is the only presentable one; every class/layout
        // refusal is an explicit unsupported state, never a silent fallback.
        ImageUploadError::Decode(media::MediaError::UnsupportedContainer)
        | ImageUploadError::InvalidRasterLayout
        | ImageUploadError::EncodedInputTooLarge { .. } => MediaAvailability::Unsupported,
        _ => MediaAvailability::Failed,
    })
}

/// Render one refused media preparation as an explicit state sentence.
pub fn media_decode_failure_text(error: &ImageUploadError) -> SharedString {
    if matches!(error, ImageUploadError::Cancelled) {
        return "この画像の準備は取り消されました。".into();
    }
    let state = media_decode_state(error).unwrap_or(MediaAvailability::Failed);
    media_state_text(state).into()
}

/// Build the canonical upload request for one admitted media representation.
///
/// The key carries `RepresentationSource::Artifact` with the media server
/// identity, epoch, canonical artifact identity/digest, the selected revision
/// and the descriptor's representation kind/version. The descriptor states no
/// orientation, so the pinned admitted class is written as `Orientation::Normal`
/// (the preparation path refuses any other orientation); EXIF rotation is not
/// interpreted on this route.
pub fn media_upload_request(
    admitted: &AdmittedMedia,
    device_generation: u64,
) -> Result<UploadRequest, MediaRequestRefusal> {
    let identity = &admitted.identity;
    if identity.representation != PRESENTABLE_REPRESENTATION {
        return Err(MediaRequestRefusal::UnsupportedRepresentation);
    }
    // A stated dimension outside the admitted class is refused before the bytes are decoded.
    for stated in [identity.width, identity.height].into_iter().flatten() {
        if stated != FRAME_EDGE {
            return Err(MediaRequestRefusal::UnsupportedGeometry);
        }
    }
    let revision = identity
        .revision
        .to_u128()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(MediaRequestRefusal::UnverifiableRevision)?;
    let size =
        PixelSize::new(FRAME_EDGE, FRAME_EDGE).map_err(|_| MediaRequestRefusal::InvalidIdentity)?;
    let key = RepresentationKey {
        source: RepresentationSource::Artifact {
            server: identity.server_identity.clone(),
            epoch: identity.epoch.clone(),
            artifact: identity.artifact_ref.clone(),
            digest: identity.artifact_digest.clone(),
            revision: identity.revision.clone(),
        },
        kind: identity.representation.clone(),
        size,
        orientation: Orientation::Normal,
        policy: DecodePolicy {
            decoder: LocalId::new(DECODER_ID).map_err(|_| MediaRequestRefusal::InvalidIdentity)?,
            colour: LocalId::new(COLOUR_ID).map_err(|_| MediaRequestRefusal::InvalidIdentity)?,
            generation: POLICY_GENERATION,
        },
    };
    let decode_limits = DecodeBudget::ratified_default()
        .limits()
        .map_err(|_| MediaRequestRefusal::InvalidIdentity)?;
    Ok(UploadRequest {
        key,
        source_revision: revision,
        device_generation,
        current_device_generation: device_generation,
        decode_limits,
        // The admission bound the bytes actually passed under is never relaxed here.
        max_encoded_bytes: admitted.max_bytes,
    })
}

/// Owned encoded bytes for one background preparation.
pub fn admitted_bytes(admitted: &AdmittedMedia) -> Arc<[u8]> {
    Arc::clone(&admitted.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::MediaPresentation;
    use desktop_protocol::{ArtifactDigest, ArtifactId, Counter, EpochId, ServerId};
    use media::Sha256Digest;

    fn identity() -> app_core::MediaIdentity {
        app_core::MediaIdentity {
            server_identity: ServerId::new("server-1").expect("server id"),
            audience: "desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            epoch: EpochId::new("epoch-1").expect("epoch id"),
            artifact_ref: ArtifactId::new("artifact-a").expect("artifact id"),
            artifact_digest: ArtifactDigest::new("sha256:artifact").expect("artifact digest"),
            revision: Counter::from_u128(3),
            representation: RepresentationKind::Original,
            representation_version: 1,
            expected_sha256: Some(Sha256Digest([1; 32])),
            byte_length: Some(1024),
            width: Some(2048),
            height: Some(2048),
        }
    }

    fn admitted() -> AdmittedMedia {
        AdmittedMedia {
            identity: identity(),
            bytes: Arc::from([1_u8, 2, 3]),
            computed_digest: Sha256Digest([1; 32]),
            max_bytes: 4096,
        }
    }

    #[test]
    fn every_unavailable_state_has_a_distinct_explicit_sentence() {
        let states = [
            MediaAvailability::Expired,
            MediaAvailability::Missing,
            MediaAvailability::Forbidden,
            MediaAvailability::Pending,
            MediaAvailability::Unsupported,
            MediaAvailability::Failed,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for state in states {
            let text = media_state_text(state);
            assert!(
                text.contains("画像を利用できません"),
                "{state:?} must be explicit"
            );
            assert!(
                !text.contains("表示できます"),
                "{state:?} must not claim a presentable picture"
            );
            assert!(
                seen.insert(text),
                "{state:?} must not share another state's text"
            );
        }
        assert_eq!(seen.len(), states.len());
        // The available sentence is not an unavailable one.
        assert!(!states.iter().any(
            |state| media_state_text(*state) == media_state_text(MediaAvailability::Available)
        ));
    }

    #[test]
    fn an_unverified_admitted_representation_is_never_labelled_verified() {
        let verified = media_success_text(true, true);
        let unverified = media_success_text(false, true);
        assert_ne!(verified, unverified);
        assert!(verified.contains("検証済み"));
        assert!(unverified.contains("未検証"));
        assert!(!unverified.contains("ダイジェスト検証済み"));
        let preparing = media_preparing_text(false);
        assert!(preparing.contains("未検証"));
        assert!(!preparing.contains("検証済み）"));
    }

    #[test]
    fn the_media_request_carries_the_canonical_artifact_identity() {
        let request = media_upload_request(&admitted(), 7).expect("canonical request");
        assert_eq!(request.source_revision, 3);
        assert_eq!(request.device_generation, 7);
        assert_eq!(request.current_device_generation, 7);
        assert_eq!(request.max_encoded_bytes, 4096);
        let RepresentationSource::Artifact {
            server,
            epoch,
            artifact,
            digest,
            revision,
        } = &request.key.source
        else {
            panic!("the media route must carry a canonical artifact source");
        };
        assert_eq!(server.as_str(), "server-1");
        assert_eq!(epoch.as_str(), "epoch-1");
        assert_eq!(artifact.as_str(), "artifact-a");
        assert_eq!(digest.as_str(), "sha256:artifact");
        assert_eq!(revision, &Counter::from_u128(3));
        assert_eq!(request.key.kind, RepresentationKind::Original);
        assert_eq!(request.key.size.width(), FRAME_EDGE);
        assert_eq!(request.key.orientation, Orientation::Normal);
    }

    #[test]
    fn a_preview_or_thumbnail_representation_is_explicitly_unsupported() {
        for kind in [RepresentationKind::Preview, RepresentationKind::Thumbnail] {
            let mut media = admitted();
            media.identity.representation = kind.clone();
            let refusal = media_upload_request(&media, 1).expect_err("not presentable");
            assert_eq!(refusal, MediaRequestRefusal::UnsupportedRepresentation);
            assert_eq!(refusal.availability(), MediaAvailability::Unsupported);
            assert!(refusal.text().contains("画像を利用できません"));
        }
    }

    #[test]
    fn a_geometry_outside_the_pinned_class_is_explicitly_unsupported() {
        let mut media = admitted();
        media.identity.width = Some(4096);
        let refusal = media_upload_request(&media, 1).expect_err("not the pinned class");
        assert_eq!(refusal, MediaRequestRefusal::UnsupportedGeometry);
        assert_eq!(refusal.availability(), MediaAvailability::Unsupported);
        // One stated mismatching dimension is enough; the descriptor need not state both.
        let mut media = admitted();
        media.identity.height = Some(1024);
        assert_eq!(
            media_upload_request(&media, 1).expect_err("height"),
            MediaRequestRefusal::UnsupportedGeometry
        );
    }

    #[test]
    fn a_revision_beyond_the_upload_identity_is_unverifiable() {
        let mut media = admitted();
        media.identity.revision = Counter::from_u128(u128::from(u64::MAX) + 1);
        let refusal = media_upload_request(&media, 1).expect_err("unverifiable revision");
        assert_eq!(refusal, MediaRequestRefusal::UnverifiableRevision);
        assert_eq!(refusal.availability(), MediaAvailability::Failed);
    }

    #[test]
    fn refused_decode_classes_are_unsupported_and_other_failures_are_failed() {
        assert_eq!(
            media_decode_state(&ImageUploadError::Decode(
                media::MediaError::UnsupportedContainer
            )),
            Some(MediaAvailability::Unsupported)
        );
        assert_eq!(
            media_decode_state(&ImageUploadError::InvalidRasterLayout),
            Some(MediaAvailability::Unsupported)
        );
        assert_eq!(
            media_decode_state(&ImageUploadError::EncodedInputTooLarge {
                actual: 10,
                maximum: 1
            }),
            Some(MediaAvailability::Unsupported)
        );
        assert_eq!(
            media_decode_state(&ImageUploadError::Decode(media::MediaError::TooLarge)),
            Some(MediaAvailability::Failed)
        );
        assert_eq!(
            media_decode_state(&ImageUploadError::Cancelled),
            None,
            "cancellation is not an availability state"
        );
        assert_eq!(
            media_decode_failure_text(&ImageUploadError::Cancelled),
            "この画像の準備は取り消されました。"
        );
    }

    #[test]
    fn an_admitted_presentation_is_the_only_path_to_a_request() {
        // The adapter accepts admitted bytes only; an unavailable presentation never
        // reaches `media_upload_request`, so no identity can be constructed from a state.
        let unavailable = MediaPresentation::Unavailable(MediaUnavailable {
            availability: MediaAvailability::Expired,
            cause: app_core::MediaUnavailableCause::Identity,
        });
        assert!(unavailable.admitted().is_none());
        let admitted_presentation = MediaPresentation::Admitted(Box::new(admitted()));
        assert!(admitted_presentation.admitted().is_some());
        assert_eq!(admitted_bytes(&admitted()), Arc::from([1_u8, 2, 3]));
    }
}
