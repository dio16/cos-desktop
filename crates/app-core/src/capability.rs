//! Capability-driven action gating (DC-MP-RT5 / MP-RT5).
//!
//! Every gate is derived from the authoritative CAPABILITIES projection only.
//! There is no provider-name inference, no catalog substitution and no local
//! default: when the Runtime projects no capability reference or no matching
//! projection, the action stays disabled with an explicit reason. An unknown
//! support value stays [`SupportState::Unknown`] and keeps the action disabled.
//!
//! The server remains authoritative. An enabled gate is an offer, not an
//! authorization: the Runtime revalidates every command and media fetch even
//! when the projection advertised support.

use std::sync::Arc;

use desktop_protocol::{
    ArtifactId, CapabilitiesId, CapabilityScope, CommandOperation, Counter, CreativeCapabilities,
    RepresentationKind, SupportState, TaskId,
};

use crate::ObservationPublication;

/// Honest reason for a capability-dependent action without any projection.
pub const NOT_PROJECTED_REASON: &str =
    "機能の利用可否情報が投影されていません（capability not projected）";

/// Stable reason code of one gate decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityGateCode {
    /// The projection states support and the scope applies.
    Supported,
    /// The task projected no capability reference at all.
    NotProjected,
    /// The task referenced a capability projection that is absent.
    Missing,
    /// The referenced projection's scope does not cover this task/artifact.
    ScopeMismatch,
    /// More than one server-scope projection exists, so no scope is decided.
    Ambiguous,
    /// The projection (or the entry) states the operation is unavailable.
    Unavailable,
    /// The projection (or the entry) states the operation is unsupported.
    Unsupported,
    /// The projection does not determine support, including unrecognized values.
    Unknown,
}

impl CapabilityGateCode {
    /// Stable machine-readable token; never shown as a user sentence.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::NotProjected => "capability_not_projected",
            Self::Missing => "capability_projection_missing",
            Self::ScopeMismatch => "capability_scope_mismatch",
            Self::Ambiguous => "capability_ambiguous",
            Self::Unavailable => "capability_unavailable",
            Self::Unsupported => "capability_unsupported",
            Self::Unknown => "capability_unknown",
        }
    }
}

/// One action decision. Disabled is the only outcome for anything but a
/// supported entry in a supported, in-scope projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityGate {
    /// Whether the action may be offered.
    pub enabled: bool,
    /// Stable decision code.
    pub code: CapabilityGateCode,
    /// User-facing reason (always present, so a disabled control is explained).
    pub reason: Arc<str>,
}

impl CapabilityGate {
    fn allow(reason: impl Into<Arc<str>>) -> Self {
        Self {
            enabled: true,
            code: CapabilityGateCode::Supported,
            reason: reason.into(),
        }
    }

    fn deny(code: CapabilityGateCode, reason: impl Into<Arc<str>>) -> Self {
        Self {
            enabled: false,
            code,
            reason: reason.into(),
        }
    }
}

/// Scope carried by one capability projection, as a Desktop view value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityScopeView {
    /// Applies server-wide.
    Server,
    /// Applies to one task.
    Task(TaskId),
    /// Applies to one artifact.
    Artifact(ArtifactId),
}

/// One supported command entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSupportView {
    /// Command family.
    pub operation: CommandOperation,
    /// Projected support state.
    pub support: SupportState,
    /// Projected reason when not supported.
    pub reason: Option<Arc<str>>,
}

/// One supported media-representation entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaSupportView {
    /// Representation kind.
    pub representation: RepresentationKind,
    /// Projected support state.
    pub support: SupportState,
    /// Projected reason when not supported.
    pub reason: Option<Arc<str>>,
}

/// One supported feature entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureSupportView {
    /// Feature tag.
    pub feature: Arc<str>,
    /// Projected support state.
    pub support: SupportState,
    /// Projected reason when not supported.
    pub reason: Option<Arc<str>>,
}

/// One advertised constraint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintView {
    /// Constraint name.
    pub name: Arc<str>,
    /// Constraint value.
    pub value: Arc<str>,
}

/// Read-only view of one authorized CAPABILITIES projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityView {
    /// Projection identity.
    pub capabilities_ref: CapabilitiesId,
    /// Scope the projection applies to.
    pub scope: CapabilityScopeView,
    /// Overall availability.
    pub availability: SupportState,
    /// Command families and their support.
    pub commands: Arc<[CommandSupportView]>,
    /// Media representations and their support.
    pub media: Arc<[MediaSupportView]>,
    /// Named features and their support.
    pub features: Arc<[FeatureSupportView]>,
    /// Advertised constraints (informational; never inferred).
    pub constraints: Arc<[ConstraintView]>,
    /// Projection-level reason when unavailable/unsupported.
    pub reason: Option<Arc<str>>,
    /// Projection revision within the Runtime epoch.
    pub revision: Counter,
}

impl CapabilityView {
    fn from_projection(capabilities: &CreativeCapabilities) -> Self {
        Self {
            capabilities_ref: capabilities.capabilities_ref.clone(),
            scope: match &capabilities.scope {
                CapabilityScope::Server => CapabilityScopeView::Server,
                CapabilityScope::Task { task_id } => CapabilityScopeView::Task(task_id.clone()),
                CapabilityScope::Artifact { artifact_id } => {
                    CapabilityScopeView::Artifact(artifact_id.clone())
                }
            },
            availability: capabilities.availability.clone(),
            commands: capabilities
                .commands
                .iter()
                .map(|command| CommandSupportView {
                    operation: command.operation.clone(),
                    support: command.support.clone(),
                    reason: command.reason.as_deref().map(Arc::from),
                })
                .collect::<Vec<_>>()
                .into(),
            media: capabilities
                .media
                .iter()
                .map(|media| MediaSupportView {
                    representation: media.representation.clone(),
                    support: media.support.clone(),
                    reason: media.reason.as_deref().map(Arc::from),
                })
                .collect::<Vec<_>>()
                .into(),
            features: capabilities
                .features
                .iter()
                .map(|feature| FeatureSupportView {
                    feature: Arc::from(feature.feature.as_str()),
                    support: feature.support.clone(),
                    reason: feature.reason.as_deref().map(Arc::from),
                })
                .collect::<Vec<_>>()
                .into(),
            constraints: capabilities
                .constraints
                .iter()
                .map(|constraint| ConstraintView {
                    name: Arc::from(constraint.name.as_str()),
                    value: Arc::from(constraint.value.as_str()),
                })
                .collect::<Vec<_>>()
                .into(),
            reason: capabilities.reason.as_deref().map(Arc::from),
            revision: capabilities.projection_revision.clone(),
        }
    }

    /// Advertised constraint value, when the projection states that exact name.
    pub fn constraint(&self, name: &str) -> Option<&Arc<str>> {
        self.constraints
            .iter()
            .find(|constraint| constraint.name.as_ref() == name)
            .map(|constraint| &constraint.value)
    }

    /// Gate one command family.
    pub fn gate_command(&self, operation: &CommandOperation) -> CapabilityGate {
        if let Some(gate) = self.unavailable_gate("この操作") {
            return gate;
        }
        if matches!(self.scope, CapabilityScopeView::Artifact(_)) {
            return CapabilityGate::deny(
                CapabilityGateCode::ScopeMismatch,
                "機能の利用可否情報が成果物の範囲だけを対象としており、操作には適用できません",
            );
        }
        match self
            .commands
            .iter()
            .find(|support| support.operation == *operation)
        {
            Some(support) => support_gate(&support.support, support.reason.as_ref(), "この操作"),
            None => CapabilityGate::deny(
                CapabilityGateCode::Unknown,
                format!(
                    "この操作（{}）の利用可否は投影されていません",
                    operation.as_str()
                ),
            ),
        }
    }

    /// Gate one media representation of an artifact.
    pub fn gate_media(
        &self,
        representation: &RepresentationKind,
        artifact: &ArtifactId,
    ) -> CapabilityGate {
        if let Some(gate) = self.unavailable_gate("この表現") {
            return gate;
        }
        let scope_applies = match &self.scope {
            CapabilityScopeView::Server | CapabilityScopeView::Task(_) => true,
            CapabilityScopeView::Artifact(projected) => projected == artifact,
        };
        if !scope_applies {
            return CapabilityGate::deny(
                CapabilityGateCode::ScopeMismatch,
                "機能の利用可否情報が、この成果物の範囲を対象としていません",
            );
        }
        if !representation.is_known() {
            return CapabilityGate::deny(
                CapabilityGateCode::Unsupported,
                "この表現形式は未対応です（UNSUPPORTED）",
            );
        }
        match self
            .media
            .iter()
            .find(|support| support.representation == *representation)
        {
            Some(support) => support_gate(&support.support, support.reason.as_ref(), "この表現"),
            None => CapabilityGate::deny(
                CapabilityGateCode::Unknown,
                format!(
                    "この表現（{}）の利用可否は投影されていません",
                    representation.as_str()
                ),
            ),
        }
    }

    /// Gate one named feature.
    pub fn gate_feature(&self, feature: &str) -> CapabilityGate {
        if let Some(gate) = self.unavailable_gate("この機能") {
            return gate;
        }
        match self
            .features
            .iter()
            .find(|support| support.feature.as_ref() == feature)
        {
            Some(support) => support_gate(&support.support, support.reason.as_ref(), "この機能"),
            None => CapabilityGate::deny(
                CapabilityGateCode::Unknown,
                format!("この機能（{feature}）の利用可否は投影されていません"),
            ),
        }
    }

    fn unavailable_gate(&self, subject: &str) -> Option<CapabilityGate> {
        match &self.availability {
            SupportState::Supported => None,
            SupportState::Unavailable => Some(CapabilityGate::deny(
                CapabilityGateCode::Unavailable,
                self.reason.as_ref().map_or_else(
                    || Arc::from(format!("{subject}は現在利用できません（UNAVAILABLE）")),
                    Arc::clone,
                ),
            )),
            SupportState::Unsupported => Some(CapabilityGate::deny(
                CapabilityGateCode::Unsupported,
                self.reason.as_ref().map_or_else(
                    || Arc::from(format!("{subject}は未対応です（UNSUPPORTED）")),
                    Arc::clone,
                ),
            )),
            SupportState::Unknown(raw) => Some(CapabilityGate::deny(
                CapabilityGateCode::Unknown,
                format!("{subject}の利用可否が不明です（{raw}）"),
            )),
        }
    }
}

/// The capability projection state of one task (or of the server scope).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CapabilityProjection {
    /// The Runtime projected a usable capability projection.
    Projected(CapabilityView),
    /// The task states no capability reference (not projected).
    #[default]
    NotProjected,
    /// The task references a projection that is absent from the publication.
    Missing(CapabilitiesId),
    /// The referenced projection's scope does not cover the task/artifact.
    ScopeMismatch(CapabilitiesId),
    /// More than one server-scope projection exists; no scope is decided.
    Ambiguous,
}

impl CapabilityProjection {
    /// The underlying view, when one was admitted.
    pub fn view(&self) -> Option<&CapabilityView> {
        match self {
            Self::Projected(view) => Some(view),
            _ => None,
        }
    }

    /// Why no view was admitted, when that is the case.
    pub fn unavailable_reason(&self) -> Option<Arc<str>> {
        match self {
            Self::Projected(_) => None,
            Self::NotProjected => Some(Arc::from(NOT_PROJECTED_REASON)),
            Self::Missing(_) => Some(Arc::from(
                "機能の利用可否情報を参照できません（投影が見つかりません）",
            )),
            Self::ScopeMismatch(_) => Some(Arc::from(
                "機能の利用可否情報が、このタスクまたは成果物の範囲を対象としていません",
            )),
            Self::Ambiguous => Some(Arc::from(
                "機能の利用可否情報が複数投影されており、対象の範囲を決定できません",
            )),
        }
    }

    /// Gate one command family. Every non-projected state fails closed.
    pub fn gate_command(&self, operation: &CommandOperation) -> CapabilityGate {
        match self {
            Self::Projected(view) => view.gate_command(operation),
            Self::NotProjected => {
                CapabilityGate::deny(CapabilityGateCode::NotProjected, NOT_PROJECTED_REASON)
            }
            Self::Missing(_) => CapabilityGate::deny(
                CapabilityGateCode::Missing,
                "機能の利用可否情報を参照できません（投影が見つかりません）",
            ),
            Self::ScopeMismatch(_) => CapabilityGate::deny(
                CapabilityGateCode::ScopeMismatch,
                "機能の利用可否情報が、このタスクの範囲を対象としていません",
            ),
            Self::Ambiguous => CapabilityGate::deny(
                CapabilityGateCode::Ambiguous,
                "機能の利用可否情報が複数投影されており、対象の範囲を決定できません",
            ),
        }
    }

    /// Gate one media representation of an artifact.
    pub fn gate_media(
        &self,
        representation: &RepresentationKind,
        artifact: &ArtifactId,
    ) -> CapabilityGate {
        match self {
            Self::Projected(view) => view.gate_media(representation, artifact),
            Self::NotProjected => {
                CapabilityGate::deny(CapabilityGateCode::NotProjected, NOT_PROJECTED_REASON)
            }
            Self::Missing(_) => CapabilityGate::deny(
                CapabilityGateCode::Missing,
                "機能の利用可否情報を参照できません（投影が見つかりません）",
            ),
            Self::ScopeMismatch(_) => CapabilityGate::deny(
                CapabilityGateCode::ScopeMismatch,
                "機能の利用可否情報が、この成果物の範囲を対象としていません",
            ),
            Self::Ambiguous => CapabilityGate::deny(
                CapabilityGateCode::Ambiguous,
                "機能の利用可否情報が複数投影されており、対象の範囲を決定できません",
            ),
        }
    }

    /// Gate one named feature. Every non-projected state fails closed.
    pub fn gate_feature(&self, feature: &str) -> CapabilityGate {
        match self {
            Self::Projected(view) => view.gate_feature(feature),
            Self::NotProjected => {
                CapabilityGate::deny(CapabilityGateCode::NotProjected, NOT_PROJECTED_REASON)
            }
            Self::Missing(_) => CapabilityGate::deny(
                CapabilityGateCode::Missing,
                "機能の利用可否情報を参照できません（投影が見つかりません）",
            ),
            Self::ScopeMismatch(_) => CapabilityGate::deny(
                CapabilityGateCode::ScopeMismatch,
                "機能の利用可否情報が、この対象の範囲を対象としていません",
            ),
            Self::Ambiguous => CapabilityGate::deny(
                CapabilityGateCode::Ambiguous,
                "機能の利用可否情報が複数投影されており、対象の範囲を決定できません",
            ),
        }
    }
}

fn support_gate(
    support: &SupportState,
    reason: Option<&Arc<str>>,
    subject: &str,
) -> CapabilityGate {
    match support {
        SupportState::Supported => CapabilityGate::allow("利用可能（SUPPORTED）"),
        SupportState::Unsupported => CapabilityGate::deny(
            CapabilityGateCode::Unsupported,
            reason.map_or_else(
                || Arc::from(format!("{subject}は未対応です（UNSUPPORTED）")),
                Arc::clone,
            ),
        ),
        SupportState::Unavailable => CapabilityGate::deny(
            CapabilityGateCode::Unavailable,
            reason.map_or_else(
                || Arc::from(format!("{subject}は現在利用できません（UNAVAILABLE）")),
                Arc::clone,
            ),
        ),
        SupportState::Unknown(raw) => CapabilityGate::deny(
            CapabilityGateCode::Unknown,
            format!("{subject}の利用可否が不明です（{raw}）"),
        ),
    }
}

/// Derive the capability projection of one task from the publication.
///
/// Absence is never defaulted: a task without a reference is
/// [`CapabilityProjection::NotProjected`], and a dangling reference is
/// [`CapabilityProjection::Missing`]. A projection scoped to a different task
/// or artifact fails closed instead of being applied to this task.
pub fn for_task(publication: &ObservationPublication, task_id: &TaskId) -> CapabilityProjection {
    let Some(snapshot) = publication.snapshot.as_ref() else {
        return CapabilityProjection::NotProjected;
    };
    let Some(task) = snapshot.tasks.get(task_id) else {
        return CapabilityProjection::NotProjected;
    };
    let Some(identity) = task.capabilities_ref.as_ref() else {
        return CapabilityProjection::NotProjected;
    };
    let Some(capabilities) = snapshot.capabilities.get(identity) else {
        return CapabilityProjection::Missing(identity.clone());
    };
    let view = CapabilityView::from_projection(capabilities);
    let scope_applies = match &view.scope {
        CapabilityScopeView::Server => true,
        CapabilityScopeView::Task(projected) => projected == task_id,
        CapabilityScopeView::Artifact(_) => false,
    };
    if scope_applies {
        CapabilityProjection::Projected(view)
    } else {
        CapabilityProjection::ScopeMismatch(identity.clone())
    }
}

/// Derive the sole server-scope capability projection, when one exists.
///
/// Used only by server-scope actions such as task creation. Multiple
/// server-scope projections are ambiguous and fail closed; a missing one is
/// [`CapabilityProjection::NotProjected`] and never enables anything by itself.
pub fn for_server(publication: &ObservationPublication) -> CapabilityProjection {
    let Some(snapshot) = publication.snapshot.as_ref() else {
        return CapabilityProjection::NotProjected;
    };
    let mut server = snapshot
        .capabilities
        .values()
        .filter(|capabilities| matches!(capabilities.scope, CapabilityScope::Server));
    let Some(first) = server.next() else {
        return CapabilityProjection::NotProjected;
    };
    if server.next().is_some() {
        return CapabilityProjection::Ambiguous;
    }
    CapabilityProjection::Projected(CapabilityView::from_projection(first))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use desktop_protocol::{
        ArtifactDigest, ArtifactId, ArtifactSnapshot, Availability, CapabilitiesId,
        CapabilityConstraint, CapabilityScope, CommandCapability, Counter, CreativeCapabilities,
        EpochId, FeatureCapability, MediaCapability, RepresentationKind, SupportState, TaskId,
        TaskSnapshot, TaskStatus,
    };

    use super::{
        CapabilityGateCode, CapabilityProjection, NOT_PROJECTED_REASON, for_server, for_task,
    };
    use crate::{ObservationPublication, PublicationConnection, PublicationSource};

    const TASK: &str = "fixture:task/Task-184";

    fn task_id() -> TaskId {
        TaskId::new(TASK).expect("valid task")
    }

    fn task(capabilities_ref: Option<CapabilitiesId>) -> TaskSnapshot {
        TaskSnapshot {
            task_ref: task_id(),
            display_id: Some("Task-184".to_owned()),
            title: "Gating".to_owned(),
            task_status: TaskStatus::Active,
            inner_phase: None,
            iteration_ref: None,
            iteration_index: None,
            best_candidate_ref: None,
            formal_outcome_ref: None,
            accepted_brief_ref: None,
            capabilities_ref,
            projection_revision: Counter::from_u128(1),
        }
    }

    fn capabilities(
        identity: &str,
        scope: CapabilityScope,
        availability: SupportState,
        commands: &[(
            desktop_protocol::CommandOperation,
            SupportState,
            Option<&str>,
        )],
        media: &[(RepresentationKind, SupportState, Option<&str>)],
        revision: u128,
    ) -> CreativeCapabilities {
        CreativeCapabilities {
            capabilities_ref: CapabilitiesId::fixture(identity),
            scope,
            commands: commands
                .iter()
                .map(|(operation, support, reason)| CommandCapability {
                    operation: operation.clone(),
                    support: support.clone(),
                    reason: reason.map(str::to_owned),
                })
                .collect(),
            media: media
                .iter()
                .map(|(representation, support, reason)| MediaCapability {
                    representation: representation.clone(),
                    support: support.clone(),
                    reason: reason.map(str::to_owned),
                })
                .collect(),
            features: vec![FeatureCapability {
                feature: "compare".to_owned(),
                support: SupportState::Supported,
                reason: None,
            }],
            constraints: vec![CapabilityConstraint {
                name: "max_bytes".to_owned(),
                value: "1048576".to_owned(),
            }],
            availability,
            reason: None,
            projection_revision: Counter::from_u128(revision),
        }
    }

    fn publication(
        task: TaskSnapshot,
        capabilities: Option<CreativeCapabilities>,
        revision: u128,
    ) -> ObservationPublication {
        let mut snapshot: crate::SnapshotHandle = Arc::default();
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        state.epoch = Some(EpochId::fixture("epoch/1"));
        state.tasks.insert(task.task_ref.clone(), task);
        if let Some(capabilities) = capabilities {
            state
                .capabilities
                .insert(capabilities.capabilities_ref.clone(), capabilities);
        }
        state.artifacts.insert(
            ArtifactId::fixture("artifact/a"),
            ArtifactSnapshot {
                artifact_ref: ArtifactId::fixture("artifact/a"),
                artifact_digest: ArtifactDigest::fixture("digest/a"),
                media_type: Some("image/png".to_owned()),
                availability: Availability::Available,
                descriptor_refs: Vec::new(),
                verification: None,
                projection_revision: Counter::from_u128(revision),
            },
        );
        ObservationPublication {
            snapshot: Some(snapshot),
            revision: Some(Counter::from_u128(revision)),
            connection: PublicationConnection::Observing,
            source: PublicationSource::Fixture,
            stale: false,
            ..ObservationPublication::default()
        }
    }

    fn task_scope(identity: &str) -> CapabilityScope {
        CapabilityScope::Task {
            task_id: TaskId::fixture(format!("task/{identity}")),
        }
    }

    fn stop(
        support: SupportState,
        reason: Option<&str>,
    ) -> (
        desktop_protocol::CommandOperation,
        SupportState,
        Option<&str>,
    ) {
        (
            desktop_protocol::CommandOperation::StopTask,
            support,
            reason,
        )
    }

    #[test]
    fn an_absent_projection_disables_with_the_honest_not_projected_reason() {
        let publication = publication(task(None), None, 1);
        let projection = for_task(&publication, &task_id());
        assert_eq!(projection, CapabilityProjection::NotProjected);
        let gate = projection.gate_command(&desktop_protocol::CommandOperation::StopTask);
        assert!(!gate.enabled);
        assert_eq!(gate.code, CapabilityGateCode::NotProjected);
        assert_eq!(gate.reason.as_ref(), NOT_PROJECTED_REASON);
        assert!(gate.reason.contains("capability not projected"));
    }

    #[test]
    fn a_dangling_reference_is_missing_not_not_projected() {
        let reference = CapabilitiesId::fixture("capabilities/absent");
        let publication = publication(task(Some(reference.clone())), None, 1);
        let projection = for_task(&publication, &task_id());
        assert_eq!(projection, CapabilityProjection::Missing(reference));
        assert!(
            !projection
                .gate_command(&desktop_protocol::CommandOperation::StopTask)
                .enabled
        );
    }

    #[test]
    fn an_available_projection_enables_only_the_projected_command() {
        let reference = CapabilitiesId::fixture("capabilities/task");
        let publication = publication(
            task(Some(reference.clone())),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[stop(SupportState::Supported, None)],
                &[],
                1,
            )),
            1,
        );
        let projection = for_task(&publication, &task_id());
        assert_eq!(
            projection.view().expect("view").revision,
            Counter::from_u128(1)
        );
        assert!(
            projection
                .gate_command(&desktop_protocol::CommandOperation::StopTask)
                .enabled
        );
        let create = projection.gate_command(&desktop_protocol::CommandOperation::CreateTask);
        assert!(!create.enabled, "an unlisted command is never enabled");
        assert_eq!(create.code, CapabilityGateCode::Unknown);
    }

    #[test]
    fn an_unavailable_projection_disables_every_action_with_its_reason() {
        let mut capabilities = capabilities(
            "capabilities/task",
            task_scope("Task-184"),
            SupportState::Unavailable,
            &[stop(SupportState::Supported, None)],
            &[],
            1,
        );
        capabilities.reason = Some("provider quota exhausted".to_owned());
        let publication = publication(
            task(Some(capabilities.capabilities_ref.clone())),
            Some(capabilities),
            1,
        );
        let projection = for_task(&publication, &task_id());
        let gate = projection.gate_command(&desktop_protocol::CommandOperation::StopTask);
        assert!(!gate.enabled);
        assert_eq!(gate.code, CapabilityGateCode::Unavailable);
        assert_eq!(gate.reason.as_ref(), "provider quota exhausted");
    }

    #[test]
    fn an_unsupported_command_keeps_its_projected_reason() {
        let publication = publication(
            task(Some(CapabilitiesId::fixture("capabilities/task"))),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[stop(
                    SupportState::Unsupported,
                    Some("stop is disabled by policy"),
                )],
                &[],
                1,
            )),
            1,
        );
        let projection = for_task(&publication, &task_id());
        let gate = projection.gate_command(&desktop_protocol::CommandOperation::StopTask);
        assert!(!gate.enabled);
        assert_eq!(gate.code, CapabilityGateCode::Unsupported);
        assert_eq!(gate.reason.as_ref(), "stop is disabled by policy");
    }

    #[test]
    fn an_unknown_support_value_stays_unknown_and_disabled() {
        let publication = publication(
            task(Some(CapabilitiesId::fixture("capabilities/task"))),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[stop(SupportState::Unknown("FUTURE".to_owned()), None)],
                &[],
                1,
            )),
            1,
        );
        let projection = for_task(&publication, &task_id());
        let gate = projection.gate_command(&desktop_protocol::CommandOperation::StopTask);
        assert!(!gate.enabled);
        assert_eq!(gate.code, CapabilityGateCode::Unknown);
        assert!(gate.reason.contains("FUTURE"));
    }

    #[test]
    fn a_scope_mismatch_fails_closed_instead_of_applying_another_tasks_grant() {
        let publication = publication(
            task(Some(CapabilitiesId::fixture("capabilities/other"))),
            Some(capabilities(
                "capabilities/other",
                task_scope("Task-999"),
                SupportState::Supported,
                &[stop(SupportState::Supported, None)],
                &[],
                1,
            )),
            1,
        );
        let projection = for_task(&publication, &task_id());
        assert!(matches!(projection, CapabilityProjection::ScopeMismatch(_)));
        assert!(
            !projection
                .gate_command(&desktop_protocol::CommandOperation::StopTask)
                .enabled
        );
    }

    #[test]
    fn a_revision_change_re_evaluates_the_same_identity() {
        let reference = CapabilitiesId::fixture("capabilities/task");
        let first = publication(
            task(Some(reference.clone())),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[stop(SupportState::Supported, None)],
                &[],
                1,
            )),
            1,
        );
        assert!(
            for_task(&first, &task_id())
                .gate_command(&desktop_protocol::CommandOperation::StopTask)
                .enabled
        );
        let revoked = publication(
            task(Some(reference.clone())),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[stop(SupportState::Unavailable, Some("grant revoked"))],
                &[],
                2,
            )),
            2,
        );
        let gate = for_task(&revoked, &task_id())
            .gate_command(&desktop_protocol::CommandOperation::StopTask);
        assert!(!gate.enabled, "a newer revision must revoke the offer");
        assert_eq!(gate.reason.as_ref(), "grant revoked");
    }

    #[test]
    fn media_gating_distinguishes_supported_unavailable_and_unprojected() {
        let artifact = ArtifactId::fixture("artifact/a");
        let enabled = publication(
            task(Some(CapabilitiesId::fixture("capabilities/task"))),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[],
                &[(RepresentationKind::Original, SupportState::Supported, None)],
                1,
            )),
            1,
        );
        assert!(
            for_task(&enabled, &task_id())
                .gate_media(&RepresentationKind::Original, &artifact)
                .enabled
        );

        let unavailable = publication(
            task(Some(CapabilitiesId::fixture("capabilities/task"))),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[],
                &[(
                    RepresentationKind::Preview,
                    SupportState::Unsupported,
                    Some("preview disabled"),
                )],
                1,
            )),
            1,
        );
        let gate =
            for_task(&unavailable, &task_id()).gate_media(&RepresentationKind::Preview, &artifact);
        assert!(!gate.enabled);
        assert_eq!(gate.code, CapabilityGateCode::Unsupported);
        assert_eq!(gate.reason.as_ref(), "preview disabled");

        let absent = publication(task(None), None, 1);
        let gate =
            for_task(&absent, &task_id()).gate_media(&RepresentationKind::Original, &artifact);
        assert!(!gate.enabled);
        assert_eq!(gate.code, CapabilityGateCode::NotProjected);
    }

    #[test]
    fn an_artifact_scope_must_match_the_requested_artifact() {
        let capability = capabilities(
            "capabilities/artifact",
            CapabilityScope::Artifact {
                artifact_id: ArtifactId::fixture("artifact/other"),
            },
            SupportState::Supported,
            &[],
            &[(RepresentationKind::Original, SupportState::Supported, None)],
            1,
        );
        // A task must not borrow an artifact-scoped projection.
        let publication = publication(
            task(Some(CapabilitiesId::fixture("capabilities/artifact"))),
            Some(capability.clone()),
            1,
        );
        assert!(matches!(
            for_task(&publication, &task_id()),
            CapabilityProjection::ScopeMismatch(_)
        ));

        // The view itself never applies that scope to another artifact.
        let view = super::CapabilityView::from_projection(&capability);
        let foreign = view.gate_media(
            &RepresentationKind::Original,
            &ArtifactId::fixture("artifact/a"),
        );
        assert!(!foreign.enabled);
        assert_eq!(foreign.code, CapabilityGateCode::ScopeMismatch);
        assert!(
            view.gate_media(
                &RepresentationKind::Original,
                &ArtifactId::fixture("artifact/other")
            )
            .enabled
        );
    }

    #[test]
    fn features_and_constraints_are_never_inferred() {
        let publication = publication(
            task(Some(CapabilitiesId::fixture("capabilities/task"))),
            Some(capabilities(
                "capabilities/task",
                task_scope("Task-184"),
                SupportState::Supported,
                &[],
                &[],
                1,
            )),
            1,
        );
        let projection = for_task(&publication, &task_id());
        assert!(projection.gate_feature("compare").enabled);
        assert!(!projection.gate_feature("video").enabled);
        let view = projection.view().expect("view");
        assert_eq!(
            view.constraint("max_bytes").map(|value| value.as_ref()),
            Some("1048576")
        );
        assert_eq!(view.constraint("unknown"), None);
    }

    #[test]
    fn no_non_supported_value_ever_enables_an_action() {
        for support in [
            SupportState::Unsupported,
            SupportState::Unavailable,
            SupportState::Unknown("FUTURE".to_owned()),
        ] {
            let publication = publication(
                task(Some(CapabilitiesId::fixture("capabilities/task"))),
                Some(capabilities(
                    "capabilities/task",
                    task_scope("Task-184"),
                    SupportState::Supported,
                    &[stop(support.clone(), Some("entry"))],
                    &[(RepresentationKind::Original, support.clone(), Some("entry"))],
                    1,
                )),
                1,
            );
            let projection = for_task(&publication, &task_id());
            let command = projection.gate_command(&desktop_protocol::CommandOperation::StopTask);
            assert!(!command.enabled, "{support:?} must not enable a command");
            let media = projection.gate_media(
                &RepresentationKind::Original,
                &ArtifactId::fixture("artifact/a"),
            );
            assert!(!media.enabled, "{support:?} must not enable media");
        }
        let absent = for_task(&publication(task(None), None, 1), &task_id());
        assert!(!absent.gate_feature("compare").enabled);
    }

    #[test]
    fn a_server_scope_projection_is_used_only_when_unambiguous() {
        let mut server = capabilities(
            "capabilities/server",
            CapabilityScope::Server,
            SupportState::Supported,
            &[(
                desktop_protocol::CommandOperation::CreateTask,
                SupportState::Supported,
                None,
            )],
            &[],
            1,
        );
        let publication = publication(task(None), Some(server.clone()), 1);
        let projection = for_server(&publication);
        assert!(
            projection
                .gate_command(&desktop_protocol::CommandOperation::CreateTask)
                .enabled
        );

        server.capabilities_ref = CapabilitiesId::fixture("capabilities/server-2");
        let mut snapshot = publication.snapshot.clone().expect("snapshot");
        let state = Arc::make_mut(&mut snapshot);
        state
            .capabilities
            .insert(server.capabilities_ref.clone(), server);
        let ambiguous = ObservationPublication {
            snapshot: Some(snapshot),
            ..publication
        };
        assert_eq!(for_server(&ambiguous), CapabilityProjection::Ambiguous);
        assert!(
            !for_server(&ambiguous)
                .gate_command(&desktop_protocol::CommandOperation::CreateTask)
                .enabled
        );
    }
}
