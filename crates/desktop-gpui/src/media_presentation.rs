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

use std::sync::{Arc, Mutex};

use app_core::{AdmittedMedia, MediaAvailability, MediaRepository, MediaSource, MediaUnavailable};
use desktop_protocol::{RepresentationKind, ServerId};
use gpui::SharedString;
use media::{
    DecodeBudget, DecodePolicy, LocalId, Orientation, PixelSize, RepresentationKey,
    RepresentationSource,
};

use crate::image_upload::{ImageUploadError, UploadRequest};

/// Build one media source for the authenticated read context.
///
/// The binary boundary supplies it; the closure captures the validated loopback
/// origin and the live session credential slot, so no token, origin or handle
/// ever enters this crate, `app-core`, a status sentence or a diagnostic.
pub type MediaSourceFactory =
    Box<dyn Fn(&ServerId, &str) -> Box<dyn MediaSource + Send> + Send + Sync>;

/// The authenticated Desktop principal the current session credential owns.
///
/// The binary updates this one shared cell when a bounded reauthorization or a
/// reconnect install persists a replacement credential (a Runtime restart mints
/// a new principal). Media reads it per resolution, so a launch-time copy can
/// never be presented against a descriptor minted for the current credential.
pub type MediaAudienceCell = Arc<Mutex<Option<String>>>;

/// One binary-provided media acquisition boundary for the artifact surface.
///
/// The repository is process-wide (one bounded cache per session) and every
/// resolution gets its own source, because a source owns one transport session
/// binding. Nothing here fetches on the main thread: the caller resolves on
/// GPUI's background executor through [`crate::image_viewer::ImageViewer::resolve_media`].
pub struct MediaRuntime {
    repository: Arc<Mutex<MediaRepository>>,
    audience: MediaAudienceCell,
    source: MediaSourceFactory,
}

impl MediaRuntime {
    /// Bind the process-wide repository, the live authenticated principal cell
    /// and the per-load source factory.
    pub fn new(
        repository: MediaRepository,
        audience: MediaAudienceCell,
        source: MediaSourceFactory,
    ) -> Self {
        Self {
            repository: Arc::new(Mutex::new(repository)),
            audience,
            source,
        }
    }

    /// Shared bounded media repository.
    pub fn repository(&self) -> Arc<Mutex<MediaRepository>> {
        Arc::clone(&self.repository)
    }

    /// Authenticated Desktop principal the descriptor audience must match.
    ///
    /// `None` means no authenticated principal is installed for the current
    /// credential (an operator bare token has none); the caller must surface the
    /// explicit unavailable state instead of requesting media.
    pub fn audience(&self) -> Option<String> {
        self.audience
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .filter(|value| !value.is_empty())
    }

    /// The live principal cell, shared with the binary's credential lifecycle.
    pub fn audience_cell(&self) -> MediaAudienceCell {
        Arc::clone(&self.audience)
    }

    /// One source bound to this publication's server identity and audience.
    pub fn source(
        &self,
        server_identity: &ServerId,
        audience: &str,
    ) -> Box<dyn MediaSource + Send> {
        (self.source)(server_identity, audience)
    }
}

/// What the artifact surface can do in this session.
///
/// The distinction is part of the honest availability vocabulary: an explicit
/// fixture session has no Runtime media boundary, while a Runtime session whose
/// authenticated principal could not be established must say so instead of
/// looking like a build that cannot display pictures at all.
pub enum MediaBinding {
    /// No Runtime media boundary exists (explicit fixture/local session).
    Absent,
    /// A Runtime session exists, but its authenticated media principal is unknown.
    Unavailable,
    /// Runtime media acquisition is available for this session.
    Runtime(MediaRuntime),
}

/// Surface statement for one media binding.
///
/// `Absent` keeps the pre-existing local-only sentence; `Unavailable` is an
/// explicit refusal; `Runtime` names the real acquisition path without
/// promising that a particular artifact is available.
pub fn media_binding_statement(binding: &MediaBinding) -> &'static str {
    match binding {
        MediaBinding::Absent => {
            "現在の版では、成果物の画像を表示できません。候補の説明情報のみを表示しています。"
        }
        MediaBinding::Unavailable => media_principal_unavailable_statement(),
        MediaBinding::Runtime(_) => {
            "Runtime が認可した成果物メディアを取得して表示します。取得できない場合は、理由を画像領域に明示します。"
        }
    }
}

/// Explicit sentence for a session whose credential no longer proves an
/// authenticated Desktop principal.
///
/// A Runtime binding whose live principal cell is empty (a revoked or
/// unavailable identity) must say so instead of promising media acquisition or
/// validating against a stale launch-time principal.
pub fn media_principal_unavailable_statement() -> &'static str {
    "画像を利用できません: このセッションの認証済みメディア主体を確認できないため、成果物の画像を取得できません。"
}

/// The only representation kind this build can decode and present.
///
/// The admitted decode class is the full-resolution RGB8 PNG at one of the explicit square edges
/// (`media::APPROVED_EDGES`: 320, 1024, 2048 — the real live artifact sizes plus the original
/// 2048 ceiling). A thumbnail/preview representation is a different, smaller payload with its own
/// budget, so it is refused explicitly rather than decoded as if it were the original.
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
    /// The descriptor states a pixel size outside the admitted square edge class.
    UnsupportedGeometry,
    /// The descriptor states no pixel size, so the admitted class cannot be pinned.
    UnstatedGeometry,
    /// The canonical revision cannot be represented by the upload identity.
    UnverifiableRevision,
    /// The media identity cannot be turned into a representation key.
    InvalidIdentity,
}

impl MediaRequestRefusal {
    /// Availability state to render for this refusal.
    pub fn availability(self) -> MediaAvailability {
        match self {
            Self::UnsupportedRepresentation
            | Self::UnsupportedGeometry
            | Self::UnstatedGeometry => MediaAvailability::Unsupported,
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
            Self::UnstatedGeometry => {
                "画像を利用できません: 記述子に画像寸法がないため、検証済みの表示寸法を決められません。"
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

/// Explicit sentence for a manual refetch that cannot start.
///
/// The pane keeps an actionable control only while a resolution can still be
/// attempted through the live session; when no observation/lease is available
/// the attempt is refused with this sentence instead of pretending to retry.
pub fn media_refetch_unavailable_statement() -> &'static str {
    "画像を利用できません: 現在の観測情報がないため、メディアを再取得できません。"
}

/// The media outcome a pane currently shows, plus the one manual refetch it offers.
///
/// The viewer never invents an availability state: it only records the state one
/// resolution reported. A manual refetch is offered exactly for the outcomes a
/// fresh resolution through the current session identity can change — an expired
/// descriptor/handle and a refused principal — and never while one is already in
/// flight, so one activation starts at most one resolution (no retry storm).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaRefetchState {
    availability: Option<MediaAvailability>,
    pending: bool,
}

impl MediaRefetchState {
    /// Record one explicit unavailable outcome.
    pub fn record_unavailable(&mut self, availability: MediaAvailability) {
        self.availability = Some(availability);
        self.pending = false;
    }

    /// Record a presented picture: the empty/unavailable state is gone.
    pub fn record_available(&mut self) {
        *self = Self::default();
    }

    /// A resolution ended without an outcome (cancelled/superseded); any attempt
    /// that was in flight is no longer running.
    pub fn settle(&mut self) {
        self.pending = false;
    }

    /// Forget any media outcome: a non-media reason replaced the pane's status.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether the pane should show the refetch control now.
    pub fn offers(&self) -> bool {
        !self.pending
            && matches!(
                self.availability,
                Some(MediaAvailability::Expired | MediaAvailability::Forbidden)
            )
    }

    /// Take the one offer, if present. Returns whether a refetch may start.
    pub fn request(&mut self) -> bool {
        if self.offers() {
            self.pending = true;
            true
        } else {
            false
        }
    }

    /// Whether one manual refetch is in flight.
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// The state the pane currently shows; `None` means no explicit media outcome.
    pub fn availability(&self) -> Option<MediaAvailability> {
        self.availability
    }
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
    // The admitted class is an explicit square edge set; the descriptor must state that edge so
    // the request identity and the decode class cannot disagree. A stated dimension outside the
    // set is refused before the bytes are decoded; a descriptor that states nothing cannot be
    // pinned and is refused instead of silently assuming one class member.
    let edge = match (identity.width, identity.height) {
        (Some(width), Some(height)) if width == height => width,
        (Some(width), None) => width,
        (None, Some(height)) => height,
        (Some(_), Some(_)) => return Err(MediaRequestRefusal::UnsupportedGeometry),
        (None, None) => return Err(MediaRequestRefusal::UnstatedGeometry),
    };
    if !media::approved_edge(edge) {
        return Err(MediaRequestRefusal::UnsupportedGeometry);
    }
    let revision = identity
        .revision
        .to_u128()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(MediaRequestRefusal::UnverifiableRevision)?;
    let size = PixelSize::new(edge, edge).map_err(|_| MediaRequestRefusal::InvalidIdentity)?;
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
    fn a_manual_refetch_is_offered_only_for_expired_and_forbidden_outcomes() {
        // A cold pane (no media resolution has reported anything) offers nothing.
        let cold = MediaRefetchState::default();
        assert!(!cold.offers());
        assert_eq!(cold.availability(), None);
        for (availability, offered) in [
            (MediaAvailability::Expired, true),
            (MediaAvailability::Forbidden, true),
            (MediaAvailability::Missing, false),
            (MediaAvailability::Pending, false),
            (MediaAvailability::Unsupported, false),
            (MediaAvailability::Failed, false),
            (MediaAvailability::Available, false),
        ] {
            let mut state = MediaRefetchState::default();
            state.record_unavailable(availability);
            assert_eq!(state.offers(), offered, "{availability:?}");
            assert_eq!(state.availability(), Some(availability));
            assert!(!state.is_pending());
        }
        // A non-media reason (Missing/gate/principal sentence) replaces the pane
        // status and must not leave an actionable media control behind.
        let mut cleared = MediaRefetchState::default();
        cleared.record_unavailable(MediaAvailability::Expired);
        cleared.clear();
        assert!(!cleared.offers());
        assert_eq!(cleared.availability(), None);
    }

    #[test]
    fn one_activation_starts_at_most_one_manual_refetch() {
        let mut state = MediaRefetchState::default();
        state.record_unavailable(MediaAvailability::Expired);
        assert!(state.offers());
        assert!(state.request());
        // While one attempt is in flight the control is gone: a second activation
        // (double click or key repeat) cannot start another resolution.
        assert!(state.is_pending());
        assert!(!state.offers());
        assert!(!state.request());
        // An attempt that could not start (no observation/lease) re-enables the
        // control instead of leaving it silently disabled.
        state.settle();
        assert!(state.offers());
    }

    #[test]
    fn a_presented_picture_replaces_the_expired_state() {
        let mut state = MediaRefetchState::default();
        state.record_unavailable(MediaAvailability::Expired);
        assert!(state.request());
        state.record_available();
        assert_eq!(state.availability(), None);
        assert!(!state.is_pending());
        assert!(!state.offers());
    }

    #[test]
    fn a_repeated_failure_keeps_an_explicit_state_and_reoffers_the_control() {
        let mut state = MediaRefetchState::default();
        state.record_unavailable(MediaAvailability::Expired);
        assert!(state.request());
        // The refetch failed again: the explicit state is unchanged, and the
        // user may try once more (one attempt at a time, never an automatic loop).
        state.record_unavailable(MediaAvailability::Expired);
        assert_eq!(state.availability(), Some(MediaAvailability::Expired));
        assert!(!state.is_pending());
        assert!(state.offers());
        // A refused principal keeps its own explicit state.
        state.record_unavailable(MediaAvailability::Forbidden);
        assert_eq!(state.availability(), Some(MediaAvailability::Forbidden));
        assert!(state.offers());
    }

    #[test]
    fn a_refused_manual_refetch_has_its_own_explicit_sentence() {
        let text = media_refetch_unavailable_statement();
        assert!(text.contains("画像を利用できません"));
        assert!(text.contains("再取得できません"));
        assert!(!text.contains("表示できます"));
        for state in [
            MediaAvailability::Expired,
            MediaAvailability::Missing,
            MediaAvailability::Forbidden,
            MediaAvailability::Pending,
            MediaAvailability::Unsupported,
            MediaAvailability::Failed,
        ] {
            assert_ne!(text, media_state_text(state));
        }
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
        assert_eq!(request.key.size.width(), 2048);
        assert_eq!(request.key.orientation, Orientation::Normal);
    }

    #[test]
    fn every_admitted_square_edge_becomes_a_request_with_its_own_geometry() {
        for edge in media::APPROVED_EDGES {
            let mut media = admitted();
            media.identity.width = Some(edge);
            media.identity.height = Some(edge);
            let request = media_upload_request(&media, 7).expect("an admitted live edge");
            assert_eq!(request.key.size.width(), edge);
            assert_eq!(request.key.size.height(), edge);
            let RepresentationSource::Artifact {
                artifact, digest, ..
            } = &request.key.source
            else {
                panic!("the media route must carry a canonical artifact source");
            };
            assert_eq!(artifact.as_str(), "artifact-a");
            assert_eq!(digest.as_str(), "sha256:artifact");
        }
        // A descriptor that states one side of an admitted square edge is enough: the class is
        // square, and the actual PNG header is checked again at decode/admission.
        for (width, height) in [(Some(1024), None), (None, Some(1024))] {
            let mut media = admitted();
            media.identity.width = width;
            media.identity.height = height;
            let request = media_upload_request(&media, 7).expect("square edge from one side");
            assert_eq!(request.key.size.width(), 1024);
        }
    }

    #[test]
    fn a_geometry_outside_the_admitted_set_is_explicitly_unsupported() {
        // Just outside each admitted edge, rectangles, and clearly over the 2048 ceiling.
        for (width, height) in [
            (Some(319), Some(319)),
            (Some(321), Some(321)),
            (Some(1023), Some(1023)),
            (Some(1025), Some(1025)),
            (Some(2047), Some(2047)),
            (Some(2049), Some(2049)),
            (Some(4096), Some(4096)),
            (Some(1024), Some(320)),
        ] {
            let mut media = admitted();
            media.identity.width = width;
            media.identity.height = height;
            let refusal = media_upload_request(&media, 1).expect_err("outside the admitted set");
            assert_eq!(refusal, MediaRequestRefusal::UnsupportedGeometry);
            assert_eq!(refusal.availability(), MediaAvailability::Unsupported);
            assert!(refusal.text().contains("画像を利用できません"));
        }
    }

    #[test]
    fn a_descriptor_without_stated_geometry_is_explicitly_unsupported() {
        let mut media = admitted();
        media.identity.width = None;
        media.identity.height = None;
        let refusal = media_upload_request(&media, 1).expect_err("no stated geometry");
        assert_eq!(refusal, MediaRequestRefusal::UnstatedGeometry);
        assert_eq!(refusal.availability(), MediaAvailability::Unsupported);
        assert!(refusal.text().contains("画像を利用できません"));
        assert!(refusal.text().contains("画像寸法"));
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
    fn one_stated_mismatching_side_is_enough_to_refuse_a_rectangle() {
        // The identity fixture states 2048×2048; stating a different height is a rectangle and
        // must be refused even though 1024 is itself an admitted square edge.
        let mut media = admitted();
        media.identity.height = Some(1024);
        let refusal = media_upload_request(&media, 1).expect_err("not a square");
        assert_eq!(refusal, MediaRequestRefusal::UnsupportedGeometry);
        assert_eq!(refusal.availability(), MediaAvailability::Unsupported);
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

    /// A source that never fetches: this module passes it through untouched.
    struct StubSource;

    impl app_core::MediaSource for StubSource {
        fn fetch_descriptor(
            &mut self,
            _artifact_digest: &str,
            _context: &str,
            _cancellation: &app_core::MediaFetchCancellation,
        ) -> Result<desktop_protocol::MediaDescriptor, app_core::MediaFetchError> {
            Err(app_core::MediaFetchError::Transport)
        }

        fn fetch_bytes(
            &mut self,
            _descriptor: &desktop_protocol::MediaDescriptor,
            _cancellation: &app_core::MediaFetchCancellation,
        ) -> Result<app_core::FetchedMedia, app_core::MediaFetchError> {
            Err(app_core::MediaFetchError::Transport)
        }
    }

    fn audience_cell(principal: &str) -> MediaAudienceCell {
        Arc::new(Mutex::new(Some(principal.to_owned())))
    }

    fn runtime() -> MediaRuntime {
        let repository = app_core::MediaRepository::new(media::LocalLimits {
            max_input_bytes: 1024,
            max_live_bytes: 4096,
            max_live_entries: 2,
            max_pixels: 1024,
        })
        .expect("repository limits");
        MediaRuntime::new(
            repository,
            audience_cell("desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Box::new(|_, _| Box::new(StubSource)),
        )
    }

    #[test]
    fn every_media_binding_has_its_own_explicit_surface_statement() {
        let absent = media_binding_statement(&MediaBinding::Absent);
        let unavailable = media_binding_statement(&MediaBinding::Unavailable);
        let runtime = media_binding_statement(&MediaBinding::Runtime(runtime()));
        assert_eq!(
            absent,
            "現在の版では、成果物の画像を表示できません。候補の説明情報のみを表示しています。"
        );
        assert!(unavailable.contains("画像を利用できません"));
        assert!(unavailable.contains("メディア主体"));
        assert_ne!(absent, unavailable);
        assert_ne!(absent, runtime);
        assert_ne!(unavailable, runtime);
        assert!(runtime.contains("Runtime"));
        assert!(!runtime.contains("画像を利用できません"));
    }

    #[test]
    fn the_binding_joins_the_authenticated_audience_and_shares_one_repository() {
        let (server, audience) = (
            ServerId::new("server-1").expect("server"),
            "desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let repository = app_core::MediaRepository::new(media::LocalLimits {
            max_input_bytes: 1024,
            max_live_bytes: 4096,
            max_live_entries: 2,
            max_pixels: 1024,
        })
        .expect("repository limits");
        let cell = audience_cell(audience);
        let runtime = MediaRuntime::new(
            repository,
            Arc::clone(&cell),
            Box::new(move |server, audience| {
                record
                    .lock()
                    .expect("capture")
                    .push((server.as_str().to_owned(), audience.to_owned()));
                Box::new(StubSource)
            }),
        );
        assert_eq!(runtime.audience().as_deref(), Some(audience));
        assert!(
            Arc::ptr_eq(&runtime.audience_cell(), &cell),
            "the runtime holds the one live principal cell"
        );
        let first = runtime.repository();
        let second = runtime.repository();
        assert!(
            Arc::ptr_eq(&first, &second),
            "one bounded repository per session"
        );
        let resolved = runtime
            .audience()
            .expect("a principal is installed for this test");
        let _ = runtime.source(&server, &resolved);
        let captured = seen.lock().expect("capture");
        assert_eq!(
            captured.as_slice(),
            [("server-1".to_owned(), audience.to_owned())]
        );
    }

    #[test]
    fn a_reinstalled_principal_is_visible_to_the_next_resolution() {
        // A Runtime restart mints a new principal; the one shared cell must make
        // the replacement visible without rebuilding the binding.
        let repository = app_core::MediaRepository::new(media::LocalLimits {
            max_input_bytes: 1024,
            max_live_bytes: 4096,
            max_live_entries: 2,
            max_pixels: 1024,
        })
        .expect("repository limits");
        let cell = audience_cell("desktop:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let runtime = MediaRuntime::new(
            repository,
            Arc::clone(&cell),
            Box::new(|_, _| Box::new(StubSource)),
        );
        let replacement = "desktop:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        *cell.lock().expect("cell") = Some(replacement.to_owned());
        assert_eq!(runtime.audience().as_deref(), Some(replacement));
        *cell.lock().expect("cell") = None;
        assert_eq!(
            runtime.audience(),
            None,
            "an absent authenticated principal is explicit, never a stale value"
        );
    }
}
