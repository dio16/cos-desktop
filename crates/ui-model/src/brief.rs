//! NAV2 Creative Brief form model (frozen field set and submit state machine).
//!
//! The model is GPUI-free and never performs I/O: it validates the frozen
//! Brief fields, builds the `CreateTaskCommand` payload and owns the command-id
//! lifecycle. The shell asks app-core for the authenticated catalog, maps it
//! through [`BriefCatalog::from_authenticated`], and routes the returned
//! [`BriefSubmission`] through the owner command path.
//!
//! Rules frozen by the NAV2 design (task-21, DD-06/R1-R8):
//! * Goal maps to `objective_text` and is trimmed to 1..=8000 characters.
//! * Creative Direction maps to optional `text_prompt` (<=8000).
//! * References are already-observed artifact identities only, bounded by the
//!   recipe's `max_images` (and the hard cap), and only when the recipe accepts
//!   image references.
//! * Recipe is a catalog `recipe_ref` copied verbatim; callers never build one.
//! * Constraints are read-only; Advanced exposes only editable catalog
//!   parameters with typed values. Decimal is canonical text, never a float.
//! * A new command id is minted on every submit. An identical-payload retry is
//!   never performed with the same id; reconcile an uncertain outcome through
//!   the receipt/command lookup instead, and choose a new submission
//!   deliberately.
//! * Acceptance is a receipt (no optimistic task row); the task appears only
//!   when the observation projection delivers it.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use app_core::{
    Catalog, CatalogDocument, CatalogGraphRef, CatalogParameter, CatalogParameterValue,
    CatalogRecipe, CatalogRecipeRef,
};
use desktop_protocol::{
    ArtifactDigest, ArtifactId, CommandEnvelope, CommandId, CommandPayload, CommandTarget,
    CreateTaskCommand, EpochId, GraphId, GraphRef, HarnessVersionId, NamedParameter,
    ParameterValue, ProfileId, RecipeId, RecipeRef, TaskId,
};

/// Maximum Goal characters (after trim).
pub const GOAL_MAX_CHARS: usize = 8000;
/// Maximum Creative Direction characters.
pub const DIRECTION_MAX_CHARS: usize = 8000;
/// Hard cap on reference artifacts, even when a recipe allows more.
pub const REFERENCES_MAX: usize = 8;

/// One Advanced control kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BriefParameterKind {
    /// Free text.
    Text,
    /// Signed integer.
    Integer,
    /// Canonical decimal text.
    Decimal,
    /// Boolean.
    Boolean,
    /// Closed choice.
    Choice,
}

/// One typed parameter value (decimal stays canonical text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BriefParameterValue {
    /// Text value.
    Text(String),
    /// Integer value.
    Integer(i64),
    /// Canonical decimal text.
    Decimal(String),
    /// Boolean value.
    Boolean(bool),
    /// Choice wire value.
    Choice(String),
}

impl BriefParameterValue {
    fn kind(&self) -> BriefParameterKind {
        match self {
            Self::Text(_) => BriefParameterKind::Text,
            Self::Integer(_) => BriefParameterKind::Integer,
            Self::Decimal(_) => BriefParameterKind::Decimal,
            Self::Boolean(_) => BriefParameterKind::Boolean,
            Self::Choice(_) => BriefParameterKind::Choice,
        }
    }
}

impl std::fmt::Display for BriefParameterValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(value) | Self::Decimal(value) | Self::Choice(value) => f.write_str(value),
            Self::Integer(value) => write!(f, "{value}"),
            Self::Boolean(value) => write!(f, "{value}"),
        }
    }
}

/// One Advanced control projected from the authenticated catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefParameter {
    /// Control identity (stable parameter id).
    pub id: String,
    /// Human label.
    pub label: String,
    /// Control kind.
    pub kind: BriefParameterKind,
    /// Default value.
    pub default_value: BriefParameterValue,
    /// Numeric minimum (decimal text) for integer/decimal controls.
    pub min: Option<String>,
    /// Numeric maximum (decimal text) for integer/decimal controls.
    pub max: Option<String>,
    /// Maximum text length for text controls.
    pub max_length: Option<usize>,
    /// Closed choice values.
    pub choices: Vec<String>,
    /// Whether the user may change the value.
    pub editable: bool,
}

/// One recipe option in the Brief catalog view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefRecipe {
    /// Exact catalog recipe reference; copied verbatim into the creation target.
    pub recipe_ref: RecipeRef,
    /// Human label.
    pub label: String,
    /// Human description.
    pub description: String,
    /// Read-only constraint summary.
    pub constraint_summary: String,
    /// Whether reference artifacts may be attached.
    pub accepts_image_references: bool,
    /// Recipe image limit (already clamped by [`REFERENCES_MAX`]).
    pub max_images: usize,
    /// Advanced controls.
    pub parameters: Vec<BriefParameter>,
}

/// Authenticated catalog view used by the Brief surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefCatalog {
    /// Whether at least one recipe is submittable.
    pub can_submit: bool,
    /// Stable capability reasons when submission is unavailable.
    pub reason_codes: Vec<String>,
    /// Observation epoch the authenticated catalog was served under.
    ///
    /// The create command must carry this epoch; a catalog without a validated
    /// epoch cannot produce a submittable brief.
    pub server_epoch: Option<EpochId>,
    /// Submittable recipes.
    pub recipes: Vec<BriefRecipe>,
}

/// Catalog projection failure (closed, value-free).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BriefCatalogError {
    /// A graph/recipe identity on the wire was not a valid Desktop id.
    InvalidIdentity,
    /// A parameter type or value was not one of the frozen controls.
    InvalidParameter,
    /// The catalog carried no usable observation epoch.
    MissingEpoch,
    /// A recipe carried no explicit image limit; the Brief never invents one.
    MissingInputLimit,
    /// A parameter bound was malformed for its control kind.
    InvalidBounds,
}

/// Brief validation failure (field-addressed, bounded text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BriefValidationError {
    /// No catalog is loaded.
    CatalogUnavailable,
    /// The catalog cannot submit (see reason codes).
    CatalogNotSubmittable,
    /// No recipe is selected.
    NoRecipe,
    /// Goal is empty after trim.
    GoalRequired,
    /// Goal exceeds [`GOAL_MAX_CHARS`].
    GoalTooLong,
    /// Creative Direction exceeds [`DIRECTION_MAX_CHARS`].
    DirectionTooLong,
    /// Too many references for this recipe.
    TooManyReferences,
    /// The recipe does not accept image references.
    ReferencesUnsupported,
    /// The same reference appears twice.
    DuplicateReference,
    /// A submitted value does not match the control kind.
    ParameterType { id: String },
    /// A read-only control was overridden.
    ReadOnlyParameter { id: String },
    /// A numeric/text bound was violated.
    ParameterBounds { id: String },
    /// A choice value is not in the closed set.
    ParameterChoice { id: String },
    /// A decimal value is not canonical text.
    DecimalNotCanonical { id: String },
    /// Submission is already in flight.
    Submitting,
    /// The authenticated catalog carried no usable observation epoch.
    EpochUnavailable,
    /// A sent command's receipt is still pending; no second intent is allowed.
    AwaitingReceipt,
}

/// Submit state machine (accepted is a receipt, never an observed task).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BriefState {
    /// Editing a draft.
    Editing,
    /// One create command is being handed to the command channel.
    Submitting { command_id: CommandId },
    /// The command channel admitted the intent; the receipt is still pending.
    Sent { command_id: CommandId },
    /// The Runtime accepted the receipt; the task is not yet observed.
    Accepted {
        command_id: CommandId,
        task_id: Option<TaskId>,
    },
    /// The command was definitively rejected.
    Rejected { command_id: CommandId, code: String },
    /// The outcome is unknown; the receipt stays recheckable and any later
    /// submit is a new intent with a new command id (never an id resend).
    Unknown { command_id: CommandId },
    /// The recipe reference is stale; refresh the catalog and resubmit.
    StaleRecipe,
}

/// One validated submission produced by [`BriefForm::request_submit`].
#[derive(Clone, Debug, PartialEq)]
pub struct BriefSubmission {
    /// Desktop command identity (freshly minted for this submit; never reused
    /// for an identical-payload retry — reconcile via receipt lookup instead).
    pub command_id: CommandId,
    /// Creation target recipe reference (verbatim catalog copy).
    pub recipe_ref: RecipeRef,
    /// Create-task payload.
    pub payload: CreateTaskCommand,
    /// Observation epoch the authenticated catalog was served under.
    pub epoch: EpochId,
}

impl BriefSubmission {
    /// Build the wire create command for this submission.
    ///
    /// The epoch is the catalog's server epoch (never a locally invented one);
    /// a create carries no `expected_revision`, by contract.
    pub fn into_envelope(self) -> CommandEnvelope {
        CommandEnvelope {
            command_id: self.command_id,
            target: CommandTarget::Creation {
                recipe_ref: self.recipe_ref,
            },
            expected_revision: None,
            epoch: Some(self.epoch),
            payload: CommandPayload::CreateTask(self.payload),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BriefFingerprint {
    objective_text: String,
    text_prompt: Option<String>,
    references: Vec<ArtifactId>,
    recipe: RecipeRef,
    parameters: Vec<(String, BriefParameterValue)>,
}

/// The editable Brief form plus its submit state.
#[derive(Clone, Debug)]
pub struct BriefForm {
    catalog: Option<BriefCatalog>,
    selected: Option<usize>,
    goal: String,
    direction: String,
    references: Vec<ArtifactId>,
    overrides: BTreeMap<String, BriefParameterValue>,
    state: BriefState,
    pending: Option<(CommandId, BriefFingerprint)>,
}

impl Default for BriefForm {
    fn default() -> Self {
        Self {
            catalog: None,
            selected: None,
            goal: String::new(),
            direction: String::new(),
            references: Vec::new(),
            overrides: BTreeMap::new(),
            state: BriefState::Editing,
            pending: None,
        }
    }
}

impl BriefForm {
    /// Create an empty form.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current state.
    pub fn state(&self) -> &BriefState {
        &self.state
    }

    /// Current catalog, when loaded.
    pub fn catalog(&self) -> Option<&BriefCatalog> {
        self.catalog.as_ref()
    }

    /// Selected recipe index.
    pub fn selected_recipe(&self) -> Option<usize> {
        self.selected
    }

    /// Goal text.
    pub fn goal(&self) -> &str {
        &self.goal
    }

    /// Creative Direction text.
    pub fn direction(&self) -> &str {
        &self.direction
    }

    /// Selected reference artifacts.
    pub fn references(&self) -> &[ArtifactId] {
        &self.references
    }

    /// User parameter overrides.
    pub fn overrides(&self) -> &BTreeMap<String, BriefParameterValue> {
        &self.overrides
    }

    /// Whether the fields accept edits (false while submitting or accepted).
    pub fn is_editable(&self) -> bool {
        matches!(
            self.state,
            BriefState::Editing
                | BriefState::Rejected { .. }
                | BriefState::Unknown { .. }
                | BriefState::StaleRecipe
        )
    }

    /// Install a freshly authenticated catalog and keep the draft.
    pub fn set_catalog(&mut self, catalog: BriefCatalog) {
        self.catalog = Some(catalog);
        self.selected = None;
        self.overrides.clear();
        self.pending = None;
        self.state = BriefState::Editing;
    }

    /// Select a recipe by index.
    pub fn select_recipe(&mut self, index: usize) -> bool {
        if !self.is_editable()
            || self
                .catalog
                .as_ref()
                .is_none_or(|c| index >= c.recipes.len())
        {
            return false;
        }
        self.selected = Some(index);
        self.overrides.clear();
        self.touch();
        true
    }

    /// Set the Goal.
    pub fn set_goal(&mut self, goal: impl Into<String>) -> bool {
        if !self.is_editable() {
            return false;
        }
        self.goal = goal.into();
        self.touch();
        true
    }

    /// Set the Creative Direction (empty means absent).
    pub fn set_direction(&mut self, direction: impl Into<String>) -> bool {
        if !self.is_editable() {
            return false;
        }
        self.direction = direction.into();
        self.touch();
        true
    }

    /// Add one already-observed reference artifact.
    pub fn add_reference(&mut self, reference: ArtifactId) -> bool {
        if !self.is_editable() {
            return false;
        }
        if !self.references.contains(&reference) {
            self.references.push(reference);
            self.touch();
        }
        true
    }

    /// Remove a reference.
    pub fn remove_reference(&mut self, reference: &ArtifactId) -> bool {
        if !self.is_editable() {
            return false;
        }
        self.references.retain(|entry| entry != reference);
        self.touch();
        true
    }

    /// Override one Advanced control value.
    pub fn set_parameter(&mut self, id: &str, value: BriefParameterValue) -> bool {
        if !self.is_editable() {
            return false;
        }
        self.overrides.insert(id.to_owned(), value);
        self.touch();
        true
    }

    /// Current state after a catalog refresh following a stale recipe.
    pub fn refresh_catalog(&mut self, catalog: BriefCatalog) {
        self.set_catalog(catalog);
    }

    /// Effective value for one control (override or catalog default).
    pub fn effective_parameter(&self, id: &str) -> Option<&BriefParameterValue> {
        let recipe = self.selected_recipe_view()?;
        let control = recipe.parameters.iter().find(|entry| entry.id == id)?;
        Some(self.overrides.get(id).unwrap_or(&control.default_value))
    }

    fn touch(&mut self) {
        if matches!(
            self.state,
            BriefState::Accepted { .. }
                | BriefState::Rejected { .. }
                | BriefState::Unknown { .. }
                | BriefState::StaleRecipe
        ) {
            self.state = BriefState::Editing;
        }
    }

    fn selected_recipe_view(&self) -> Option<&BriefRecipe> {
        let catalog = self.catalog.as_ref()?;
        catalog.recipes.get(self.selected?)
    }

    /// The first validation failure, for inline field display.
    pub fn validation_error(&self) -> Option<BriefValidationError> {
        self.validate().err()
    }

    /// Validate the draft and build the create payload.
    fn validate(
        &self,
    ) -> Result<(RecipeRef, CreateTaskCommand, BriefFingerprint), BriefValidationError> {
        let catalog = self
            .catalog
            .as_ref()
            .ok_or(BriefValidationError::CatalogUnavailable)?;
        if !catalog.can_submit {
            return Err(BriefValidationError::CatalogNotSubmittable);
        }
        let recipe = self
            .selected_recipe_view()
            .ok_or(BriefValidationError::NoRecipe)?;
        let objective_text = self.goal.trim().to_owned();
        if objective_text.is_empty() {
            return Err(BriefValidationError::GoalRequired);
        }
        if objective_text.chars().count() > GOAL_MAX_CHARS {
            return Err(BriefValidationError::GoalTooLong);
        }
        if self.direction.chars().count() > DIRECTION_MAX_CHARS {
            return Err(BriefValidationError::DirectionTooLong);
        }
        if self.references.len() > recipe.max_images.min(REFERENCES_MAX) {
            return Err(BriefValidationError::TooManyReferences);
        }
        if !recipe.accepts_image_references && !self.references.is_empty() {
            return Err(BriefValidationError::ReferencesUnsupported);
        }
        let mut unique = std::collections::BTreeSet::new();
        for reference in &self.references {
            if !unique.insert(reference.clone()) {
                return Err(BriefValidationError::DuplicateReference);
            }
        }
        let mut parameter_values = Vec::new();
        let mut fingerprint_parameters = Vec::new();
        for control in &recipe.parameters {
            if let Some(overridden) = self.overrides.get(&control.id) {
                if !control.editable {
                    return Err(BriefValidationError::ReadOnlyParameter {
                        id: control.id.clone(),
                    });
                }
                if overridden.kind() != control.kind {
                    return Err(BriefValidationError::ParameterType {
                        id: control.id.clone(),
                    });
                }
            }
            let value = self
                .overrides
                .get(&control.id)
                .cloned()
                .unwrap_or_else(|| control.default_value.clone());
            validate_parameter(control, &value)?;
            if control.editable {
                parameter_values.push(NamedParameter {
                    name: control.id.clone(),
                    value: wire_value(value.clone()),
                });
                fingerprint_parameters.push((control.id.clone(), value));
            }
        }
        let direction = {
            let trimmed = self.direction.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        };
        let payload = CreateTaskCommand {
            objective_text: objective_text.clone(),
            text_prompt: direction.clone(),
            reference_artifact_refs: self.references.clone(),
            parameter_values,
        };
        let fingerprint = BriefFingerprint {
            objective_text,
            text_prompt: direction,
            references: self.references.clone(),
            recipe: recipe.recipe_ref.clone(),
            parameters: fingerprint_parameters,
        };
        Ok((recipe.recipe_ref.clone(), payload, fingerprint))
    }

    /// Validate and start one submission.
    ///
    /// The command id is always minted here. An unknown outcome is never
    /// retried with the same id: the client rejects an already-prepared
    /// identity, and resending the same id is only allowed when the server
    /// advertises a valid dedupe window (not part of this slice). The unknown
    /// receipt stays recheckable; a later submit is a new intent.
    pub fn request_submit(&mut self) -> Result<BriefSubmission, BriefValidationError> {
        if matches!(self.state, BriefState::Submitting { .. }) {
            return Err(BriefValidationError::Submitting);
        }
        if matches!(self.state, BriefState::Sent { .. }) {
            return Err(BriefValidationError::AwaitingReceipt);
        }
        let epoch = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.server_epoch.clone())
            .ok_or(BriefValidationError::EpochUnavailable)?;
        let (recipe_ref, payload, fingerprint) = self.validate()?;
        let command_id = CommandId::generate();
        self.pending = Some((command_id.clone(), fingerprint));
        self.state = BriefState::Submitting {
            command_id: command_id.clone(),
        };
        Ok(BriefSubmission {
            command_id,
            recipe_ref,
            payload,
            epoch,
        })
    }

    /// Mark the admitted intent as sent; the receipt is still pending.
    pub fn on_sent(&mut self, command_id: &CommandId) -> bool {
        if !self.matches_pending(command_id) {
            return false;
        }
        self.state = BriefState::Sent {
            command_id: command_id.clone(),
        };
        true
    }

    /// A local refusal means nothing was sent; return to editing honestly.
    pub fn on_not_sent(&mut self) -> bool {
        if !matches!(
            self.state,
            BriefState::Submitting { .. } | BriefState::Sent { .. }
        ) {
            return false;
        }
        self.pending = None;
        self.state = BriefState::Editing;
        true
    }

    /// Apply an accepted receipt.
    ///
    /// Acceptance is definitive for its command: a later answer may fill in a
    /// task reference the first receipt lacked, but it never downgrades the
    /// painted acceptance. The task reference stays receipt data; the model
    /// still owns no observed task row.
    pub fn on_accepted(&mut self, command_id: &CommandId, task_id: Option<TaskId>) -> bool {
        if !self.matches_pending(command_id) {
            return false;
        }
        if let BriefState::Accepted {
            task_id: accepted_task,
            ..
        } = &mut self.state
        {
            if accepted_task.is_none() && task_id.is_some() {
                *accepted_task = task_id;
                return true;
            }
            return false;
        }
        self.state = BriefState::Accepted {
            command_id: command_id.clone(),
            task_id,
        };
        true
    }

    /// Apply a definitive rejection (definitive codes never auto-retry).
    pub fn on_rejected(&mut self, command_id: &CommandId, code: impl Into<String>) -> bool {
        if !self.matches_pending(command_id) {
            return false;
        }
        self.state = BriefState::Rejected {
            command_id: command_id.clone(),
            code: code.into(),
        };
        true
    }

    /// Apply a lost/unknown outcome; the identical payload may be retried.
    ///
    /// An accepted receipt is definitive: a late unknown answer for the same
    /// command never replaces the painted acceptance.
    pub fn on_unknown(&mut self, command_id: &CommandId) -> bool {
        if !self.matches_pending(command_id) {
            return false;
        }
        if matches!(
            &self.state,
            BriefState::Accepted { command_id: accepted, .. } if accepted == command_id
        ) {
            return false;
        }
        self.state = BriefState::Unknown {
            command_id: command_id.clone(),
        };
        true
    }

    /// Apply a stale-recipe refusal; the draft is kept and the catalog refreshed.
    pub fn on_stale_recipe(&mut self) -> bool {
        if !matches!(
            self.state,
            BriefState::Submitting { .. } | BriefState::Rejected { .. } | BriefState::Editing
        ) {
            return false;
        }
        self.state = BriefState::StaleRecipe;
        self.pending = None;
        true
    }

    fn matches_pending(&self, command_id: &CommandId) -> bool {
        matches!(
            &self.pending,
            Some((pending, _)) if pending == command_id
        )
    }

    /// User-visible label for the current state (no fabricated success).
    pub fn state_label(&self) -> &'static str {
        match self.state {
            BriefState::Editing => "編集中",
            BriefState::Submitting { .. } => "送信中",
            BriefState::Sent { .. } => "送信済み — 受付票を確認してください",
            BriefState::Accepted { .. } => "受付済み — 観測待ち",
            BriefState::Rejected { .. } => "拒否されました",
            BriefState::Unknown { .. } => {
                "結果不明 — 受付票を確認するか、新しい提出として送り直してください"
            }
            BriefState::StaleRecipe => "レシピが更新されました — 再読み込みしてください",
        }
    }
}

fn wire_value(value: BriefParameterValue) -> ParameterValue {
    match value {
        BriefParameterValue::Text(value) => ParameterValue::Text(value),
        BriefParameterValue::Integer(value) => ParameterValue::Integer(value),
        BriefParameterValue::Decimal(value) => ParameterValue::Decimal(value),
        BriefParameterValue::Boolean(value) => ParameterValue::Boolean(value),
        BriefParameterValue::Choice(value) => ParameterValue::Choice(value),
    }
}

fn validate_parameter(
    control: &BriefParameter,
    value: &BriefParameterValue,
) -> Result<(), BriefValidationError> {
    if value.kind() != control.kind {
        return Err(BriefValidationError::ParameterType {
            id: control.id.clone(),
        });
    }
    match value {
        BriefParameterValue::Text(text) => {
            if let Some(max_length) = control.max_length
                && text.chars().count() > max_length
            {
                return Err(BriefValidationError::ParameterBounds {
                    id: control.id.clone(),
                });
            }
        }
        BriefParameterValue::Integer(integer) => {
            if let Some(min) = control.min.as_deref()
                && let Ok(min) = min.parse::<i64>()
                && *integer < min
            {
                return Err(BriefValidationError::ParameterBounds {
                    id: control.id.clone(),
                });
            }
            if let Some(max) = control.max.as_deref()
                && let Ok(max) = max.parse::<i64>()
                && *integer > max
            {
                return Err(BriefValidationError::ParameterBounds {
                    id: control.id.clone(),
                });
            }
        }
        BriefParameterValue::Decimal(text) => {
            if !canonical_decimal(text) {
                return Err(BriefValidationError::DecimalNotCanonical {
                    id: control.id.clone(),
                });
            }
            if let Some(min) = control.min.as_deref()
                && canonical_decimal(min)
                && decimal_cmp(text, min) == Ordering::Less
            {
                return Err(BriefValidationError::ParameterBounds {
                    id: control.id.clone(),
                });
            }
            if let Some(max) = control.max.as_deref()
                && canonical_decimal(max)
                && decimal_cmp(text, max) == Ordering::Greater
            {
                return Err(BriefValidationError::ParameterBounds {
                    id: control.id.clone(),
                });
            }
        }
        BriefParameterValue::Boolean(_) => {}
        BriefParameterValue::Choice(choice) => {
            if !control.choices.iter().any(|entry| entry == choice) {
                return Err(BriefValidationError::ParameterChoice {
                    id: control.id.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Canonical decimal text: optional '-', no leading zeros, optional fraction.
pub fn canonical_decimal(text: &str) -> bool {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    if unsigned.is_empty() {
        return false;
    }
    let mut parts = unsigned.split('.');
    let integer = parts.next().unwrap_or_default();
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || (integer.len() > 1 && integer.starts_with('0'))
    {
        return false;
    }
    match parts.next() {
        Some(fraction) => {
            !fraction.is_empty()
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
                && parts.next().is_none()
        }
        None => true,
    }
}

/// Compare two canonical decimal texts without floating point.
pub fn decimal_cmp(left: &str, right: &str) -> Ordering {
    let (left_negative, left_unsigned) = split_sign(left);
    let (right_negative, right_unsigned) = split_sign(right);
    if left_negative != right_negative {
        return if left_negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let ordering = unsigned_decimal_cmp(left_unsigned, right_unsigned);
    if left_negative {
        ordering.reverse()
    } else {
        ordering
    }
}

fn split_sign(text: &str) -> (bool, &str) {
    match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    }
}

fn unsigned_decimal_cmp(left: &str, right: &str) -> Ordering {
    let (left_integer, left_fraction) = split_fraction(left);
    let (right_integer, right_fraction) = split_fraction(right);
    let left_trimmed = left_integer.trim_start_matches('0');
    let right_trimmed = right_integer.trim_start_matches('0');
    let integer_order = left_trimmed
        .len()
        .cmp(&right_trimmed.len())
        .then_with(|| left_trimmed.cmp(right_trimmed));
    if integer_order != Ordering::Equal {
        return integer_order;
    }
    let width = left_fraction.len().max(right_fraction.len());
    let left_padded = format!("{left_fraction:0<width$}");
    let right_padded = format!("{right_fraction:0<width$}");
    left_padded.cmp(&right_padded)
}

fn split_fraction(text: &str) -> (&str, &str) {
    match text.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (text, ""),
    }
}

impl BriefCatalog {
    /// Map the authenticated app-core catalog document into the Brief view.
    ///
    /// The document's server epoch is validated here and carried with the view;
    /// an empty or invalid epoch refuses the whole catalog (no submit without
    /// the epoch the Runtime served the recipe under).
    pub fn from_authenticated(document: &CatalogDocument) -> Result<Self, BriefCatalogError> {
        let raw_epoch = document.server.epoch.trim();
        if raw_epoch.is_empty() {
            return Err(BriefCatalogError::MissingEpoch);
        }
        let epoch =
            EpochId::new(raw_epoch.to_owned()).map_err(|_| BriefCatalogError::MissingEpoch)?;
        let mut catalog = Self::from_catalog(&document.catalog)?;
        catalog.server_epoch = Some(epoch);
        Ok(catalog)
    }

    /// Map one authenticated catalog body.
    pub fn from_catalog(catalog: &Catalog) -> Result<Self, BriefCatalogError> {
        let mut recipes = Vec::with_capacity(catalog.recipes.len());
        for recipe in &catalog.recipes {
            recipes.push(BriefRecipe::from_catalog(recipe)?);
        }
        Ok(Self {
            can_submit: catalog.can_submit,
            reason_codes: catalog.reason_codes.clone(),
            server_epoch: None,
            recipes,
        })
    }
}

impl BriefRecipe {
    fn from_catalog(recipe: &CatalogRecipe) -> Result<Self, BriefCatalogError> {
        let recipe_ref = recipe_ref(&recipe.recipe_ref)?;
        let max_images = recipe
            .input_limits
            .max_images
            .ok_or(BriefCatalogError::MissingInputLimit)? as usize;
        let max_images = max_images.min(REFERENCES_MAX);
        let mut parameters = Vec::with_capacity(recipe.parameters.len());
        for parameter in &recipe.parameters {
            parameters.push(BriefParameter::from_catalog(parameter)?);
        }
        Ok(Self {
            recipe_ref,
            label: recipe.label.clone(),
            description: recipe.description.clone(),
            constraint_summary: recipe.constraint_summary.clone(),
            accepts_image_references: recipe.accepts_image_references,
            max_images,
            parameters,
        })
    }
}

impl BriefParameter {
    fn from_catalog(parameter: &CatalogParameter) -> Result<Self, BriefCatalogError> {
        let kind = match parameter.kind.as_str() {
            "text" => BriefParameterKind::Text,
            "integer" => BriefParameterKind::Integer,
            "decimal" => BriefParameterKind::Decimal,
            "boolean" => BriefParameterKind::Boolean,
            "choice" => BriefParameterKind::Choice,
            _ => return Err(BriefCatalogError::InvalidParameter),
        };
        let default_value = parse_value(kind, &parameter.value)?;
        let min = parameter
            .bounds
            .as_ref()
            .and_then(|bounds| bounds.min.clone());
        let max = parameter
            .bounds
            .as_ref()
            .and_then(|bounds| bounds.max.clone());
        let max_length = match parameter
            .bounds
            .as_ref()
            .and_then(|bounds| bounds.max_length)
        {
            Some(value) if value >= 0 => Some(value as usize),
            Some(_) => return Err(BriefCatalogError::InvalidBounds),
            None => None,
        };
        validate_catalog_bounds(kind, min.as_deref(), max.as_deref(), max_length)?;
        let choices: Vec<String> = parameter
            .choices
            .iter()
            .map(|choice| choice.value.clone())
            .collect();
        if kind == BriefParameterKind::Choice && choices.is_empty() {
            return Err(BriefCatalogError::InvalidParameter);
        }
        let control = Self {
            id: parameter.id.clone(),
            label: parameter.label.clone(),
            kind,
            default_value,
            min,
            max,
            max_length,
            choices,
            editable: parameter.editable,
        };
        // The catalog default itself must satisfy the control's own bounds.
        validate_parameter(&control, &control.default_value)
            .map_err(|_| BriefCatalogError::InvalidBounds)?;
        Ok(control)
    }
}

/// Refuse malformed bounds at catalog-map time instead of silently skipping them.
fn validate_catalog_bounds(
    kind: BriefParameterKind,
    min: Option<&str>,
    max: Option<&str>,
    max_length: Option<usize>,
) -> Result<(), BriefCatalogError> {
    match kind {
        BriefParameterKind::Text => {
            if min.is_some() || max.is_some() {
                return Err(BriefCatalogError::InvalidBounds);
            }
        }
        BriefParameterKind::Integer => {
            if max_length.is_some() {
                return Err(BriefCatalogError::InvalidBounds);
            }
            let min = min
                .map(str::parse::<i64>)
                .transpose()
                .map_err(|_| BriefCatalogError::InvalidBounds)?;
            let max = max
                .map(str::parse::<i64>)
                .transpose()
                .map_err(|_| BriefCatalogError::InvalidBounds)?;
            if let (Some(min), Some(max)) = (min, max)
                && min > max
            {
                return Err(BriefCatalogError::InvalidBounds);
            }
        }
        BriefParameterKind::Decimal => {
            if max_length.is_some() {
                return Err(BriefCatalogError::InvalidBounds);
            }
            if min.is_some_and(|value| !canonical_decimal(value))
                || max.is_some_and(|value| !canonical_decimal(value))
            {
                return Err(BriefCatalogError::InvalidBounds);
            }
            if let (Some(min), Some(max)) = (min, max)
                && decimal_cmp(min, max) == Ordering::Greater
            {
                return Err(BriefCatalogError::InvalidBounds);
            }
        }
        BriefParameterKind::Boolean | BriefParameterKind::Choice => {
            if min.is_some() || max.is_some() || max_length.is_some() {
                return Err(BriefCatalogError::InvalidBounds);
            }
        }
    }
    Ok(())
}

fn parse_value(
    kind: BriefParameterKind,
    value: &CatalogParameterValue,
) -> Result<BriefParameterValue, BriefCatalogError> {
    match (kind, value) {
        (BriefParameterKind::Text, CatalogParameterValue::Text(text)) => {
            Ok(BriefParameterValue::Text(text.clone()))
        }
        (BriefParameterKind::Integer, CatalogParameterValue::Integer(value)) => {
            Ok(BriefParameterValue::Integer(*value))
        }
        (BriefParameterKind::Decimal, CatalogParameterValue::Text(text)) => {
            Ok(BriefParameterValue::Decimal(text.clone()))
        }
        (BriefParameterKind::Boolean, CatalogParameterValue::Boolean(value)) => {
            Ok(BriefParameterValue::Boolean(*value))
        }
        (BriefParameterKind::Choice, CatalogParameterValue::Text(text)) => {
            Ok(BriefParameterValue::Choice(text.clone()))
        }
        _ => Err(BriefCatalogError::InvalidParameter),
    }
}

fn recipe_ref(reference: &CatalogRecipeRef) -> Result<RecipeRef, BriefCatalogError> {
    Ok(RecipeRef {
        recipe_id: RecipeId::new(reference.recipe_id.clone())
            .map_err(|_| BriefCatalogError::InvalidIdentity)?,
        graph_ref: graph_ref(&reference.graph_ref)?,
        source_harness_version_id: reference
            .source_harness_version_id
            .as_ref()
            .map(|raw| HarnessVersionId::new(raw.clone()))
            .transpose()
            .map_err(|_| BriefCatalogError::InvalidIdentity)?,
        source_harness_digest: reference
            .source_harness_digest
            .as_ref()
            .map(|raw| ArtifactDigest::new(raw.clone()))
            .transpose()
            .map_err(|_| BriefCatalogError::InvalidIdentity)?,
        profile_ref: reference
            .profile_ref
            .as_ref()
            .map(|raw| ProfileId::new(raw.clone()))
            .transpose()
            .map_err(|_| BriefCatalogError::InvalidIdentity)?,
        projection_digest: reference
            .projection_digest
            .as_ref()
            .map(|raw| ArtifactDigest::new(raw.clone()))
            .transpose()
            .map_err(|_| BriefCatalogError::InvalidIdentity)?,
    })
}

fn graph_ref(reference: &CatalogGraphRef) -> Result<GraphRef, BriefCatalogError> {
    let version =
        u64::try_from(reference.graph_version).map_err(|_| BriefCatalogError::InvalidIdentity)?;
    Ok(GraphRef {
        graph_id: GraphId::new(reference.graph_id.clone())
            .map_err(|_| BriefCatalogError::InvalidIdentity)?,
        version,
        digest: ArtifactDigest::new(reference.graph_digest.clone())
            .map_err(|_| BriefCatalogError::InvalidIdentity)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::{
        CatalogExecutionLimits, CatalogInputLimits, CatalogParameterValue as WireValue,
    };
    use desktop_protocol::{ArtifactDigest, GraphId, HarnessVersionId, ProfileId};

    fn digest(byte: char) -> ArtifactDigest {
        ArtifactDigest::new(format!("sha256:{}", byte.to_string().repeat(64)))
            .expect("valid digest")
    }

    fn sample_recipe_ref() -> RecipeRef {
        RecipeRef {
            recipe_id: RecipeId::new("recipe-1").expect("recipe"),
            graph_ref: GraphRef {
                graph_id: GraphId::new("graph-1").expect("graph"),
                version: 3,
                digest: digest('a'),
            },
            source_harness_version_id: Some(
                HarnessVersionId::new(format!("hvid_{}", "a".repeat(64))).expect("harness"),
            ),
            source_harness_digest: Some(digest('b')),
            profile_ref: Some(ProfileId::new("profile-1").expect("profile")),
            projection_digest: Some(digest('c')),
        }
    }

    fn parameter(
        id: &str,
        kind: BriefParameterKind,
        default: BriefParameterValue,
    ) -> BriefParameter {
        BriefParameter {
            id: id.to_owned(),
            label: id.to_owned(),
            kind,
            default_value: default,
            min: None,
            max: None,
            max_length: None,
            choices: Vec::new(),
            editable: true,
        }
    }

    pub(super) fn sample_recipe() -> BriefRecipe {
        let mut seed = parameter(
            "seed",
            BriefParameterKind::Integer,
            BriefParameterValue::Integer(7),
        );
        seed.min = Some("1".to_owned());
        seed.max = Some("99".to_owned());
        let mut prompt = parameter(
            "prompt",
            BriefParameterKind::Text,
            BriefParameterValue::Text("base".to_owned()),
        );
        prompt.max_length = Some(5);
        let mut locked = parameter(
            "locked",
            BriefParameterKind::Text,
            BriefParameterValue::Text("x".to_owned()),
        );
        locked.editable = false;
        let mut ratio = parameter(
            "ratio",
            BriefParameterKind::Choice,
            BriefParameterValue::Choice("square".to_owned()),
        );
        ratio.choices = vec!["square".to_owned(), "wide".to_owned()];
        let mut strength = parameter(
            "strength",
            BriefParameterKind::Decimal,
            BriefParameterValue::Decimal("0.5".to_owned()),
        );
        strength.min = Some("0".to_owned());
        strength.max = Some("1".to_owned());
        BriefRecipe {
            recipe_ref: sample_recipe_ref(),
            label: "Still".to_owned(),
            description: "One still image".to_owned(),
            constraint_summary: "tasteful".to_owned(),
            accepts_image_references: true,
            max_images: 2,
            parameters: vec![seed, prompt, locked, ratio, strength],
        }
    }

    fn sample_catalog() -> BriefCatalog {
        BriefCatalog {
            can_submit: true,
            reason_codes: Vec::new(),
            server_epoch: Some(EpochId::new("epoch-1").expect("epoch")),
            recipes: vec![sample_recipe()],
        }
    }

    fn form_with_recipe() -> BriefForm {
        let mut form = BriefForm::new();
        form.set_catalog(sample_catalog());
        assert!(form.select_recipe(0));
        form
    }

    #[test]
    fn goal_and_direction_bounds_follow_the_frozen_contract() {
        let mut form = form_with_recipe();
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::GoalRequired)
        );
        assert!(form.set_goal("   render a still   "));
        assert!(form.set_direction("soft light"));
        assert_eq!(form.validation_error(), None);

        let (_, payload, _) = form.validate().expect("valid draft");
        assert_eq!(payload.objective_text, "render a still");

        assert!(form.set_goal("あ".repeat(GOAL_MAX_CHARS + 1)));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::GoalTooLong)
        );
        assert!(form.set_goal("ok"));
        assert!(form.set_direction("a".repeat(DIRECTION_MAX_CHARS + 1)));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::DirectionTooLong)
        );
    }

    #[test]
    fn references_are_bounded_and_recipe_gated() {
        let mut form = form_with_recipe();
        assert!(form.set_goal("goal"));
        let first = ArtifactId::new("artifact-1").expect("artifact");
        let second = ArtifactId::new("artifact-2").expect("artifact");
        let third = ArtifactId::new("artifact-3").expect("artifact");
        assert!(form.add_reference(first.clone()));
        assert!(form.add_reference(second));
        assert!(form.add_reference(third));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::TooManyReferences)
        );
        assert!(form.remove_reference(&ArtifactId::new("artifact-3").expect("artifact")));

        form.references = vec![first.clone(), first.clone()];
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::DuplicateReference)
        );

        let mut catalog = sample_catalog();
        catalog.recipes[0].accepts_image_references = false;
        let mut gated = BriefForm::new();
        gated.set_catalog(catalog);
        assert!(gated.select_recipe(0));
        assert!(gated.set_goal("goal"));
        assert!(gated.add_reference(first));
        assert_eq!(
            gated.validation_error(),
            Some(BriefValidationError::ReferencesUnsupported)
        );
    }

    #[test]
    fn advanced_controls_validate_type_bounds_readonly_and_decimal_text() {
        let mut form = form_with_recipe();
        assert!(form.set_goal("goal"));

        assert!(form.set_parameter("seed", BriefParameterValue::Integer(100)));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::ParameterBounds {
                id: "seed".to_owned()
            })
        );
        assert!(form.set_parameter("seed", BriefParameterValue::Integer(8)));
        assert!(form.set_parameter("prompt", BriefParameterValue::Text("too long".to_owned())));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::ParameterBounds {
                id: "prompt".to_owned()
            })
        );
        assert!(form.set_parameter("prompt", BriefParameterValue::Text("short".to_owned())));
        assert!(form.set_parameter("locked", BriefParameterValue::Text("changed".to_owned())));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::ReadOnlyParameter {
                id: "locked".to_owned()
            })
        );
        form.overrides.remove("locked");
        assert!(form.set_parameter("ratio", BriefParameterValue::Choice("tall".to_owned())));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::ParameterChoice {
                id: "ratio".to_owned()
            })
        );
        assert!(form.set_parameter("ratio", BriefParameterValue::Choice("wide".to_owned())));
        assert!(form.set_parameter("strength", BriefParameterValue::Decimal("1e3".to_owned())));
        assert_eq!(
            form.validation_error(),
            Some(BriefValidationError::DecimalNotCanonical {
                id: "strength".to_owned()
            })
        );
        assert!(form.set_parameter("strength", BriefParameterValue::Decimal("0.75".to_owned())));
        assert_eq!(form.validation_error(), None);

        let (_, payload, _) = form.validate().expect("valid draft");
        assert_eq!(
            payload.parameter_values.len(),
            4,
            "only editable values are sent"
        );
        assert!(
            payload
                .parameter_values
                .iter()
                .any(|entry| entry.name == "strength"
                    && entry.value == ParameterValue::Decimal("0.75".to_owned()))
        );
    }

    #[test]
    fn decimal_helpers_are_canonical_and_float_free() {
        assert!(canonical_decimal("0"));
        assert!(canonical_decimal("-12.50"));
        assert!(!canonical_decimal("01"));
        assert!(!canonical_decimal("1."));
        assert!(!canonical_decimal(".5"));
        assert!(!canonical_decimal("1e3"));
        assert_eq!(decimal_cmp("0.5", "0.50"), Ordering::Equal);
        assert_eq!(decimal_cmp("0.05", "0.5"), Ordering::Less);
        assert_eq!(decimal_cmp("-1", "0"), Ordering::Less);
        assert_eq!(decimal_cmp("2", "1.99"), Ordering::Greater);
    }

    #[test]
    fn an_unknown_outcome_is_never_retried_with_the_same_command_id() {
        let mut form = form_with_recipe();
        assert!(form.set_goal("goal"));
        let first = form.request_submit().expect("submit");
        assert!(matches!(form.state(), BriefState::Submitting { .. }));
        assert_eq!(first.recipe_ref, sample_recipe_ref());
        assert_eq!(first.epoch, EpochId::new("epoch-1").expect("catalog epoch"));
        let envelope = first.clone().into_envelope();
        assert_eq!(envelope.epoch, Some(first.epoch.clone()));
        assert_eq!(envelope.expected_revision, None);
        assert!(!form.set_goal("blocked while submitting"));
        assert_eq!(
            form.request_submit().err(),
            Some(BriefValidationError::Submitting)
        );

        assert!(form.on_unknown(&first.command_id));
        assert_eq!(
            form.state_label(),
            "結果不明 — 受付票を確認するか、新しい提出として送り直してください"
        );
        let resubmitted = form.request_submit().expect("new intent");
        assert_ne!(
            resubmitted.command_id, first.command_id,
            "the client rejects an already-prepared id; never resend it blindly"
        );

        assert!(form.on_unknown(&resubmitted.command_id));
        assert!(form.set_goal("another goal"));
        assert!(matches!(form.state(), BriefState::Editing));
        let changed = form.request_submit().expect("changed submit");
        assert_ne!(changed.command_id, resubmitted.command_id);
    }

    #[test]
    fn receipt_states_are_honest_and_stale_recipe_keeps_the_draft() {
        let mut form = form_with_recipe();
        assert!(form.set_goal("goal"));
        let submission = form.request_submit().expect("submit");
        let task = TaskId::new("task-1").expect("task");
        assert!(form.on_accepted(&submission.command_id, Some(task)));
        assert_eq!(form.state_label(), "受付済み — 観測待ち");
        assert!(!form.is_editable(), "an accepted Brief is read-only");
        assert!(!form.set_goal("accepted edits are refused"));
        assert!(!form.set_direction("accepted direction"));

        let mut rejected = form_with_recipe();
        assert!(rejected.set_goal("goal"));
        let submission = rejected.request_submit().expect("submit");
        assert!(rejected.on_rejected(&submission.command_id, "VALIDATION_FAILED"));
        assert_eq!(rejected.state_label(), "拒否されました");

        let mut stale = form_with_recipe();
        assert!(stale.set_goal("goal"));
        let _ = stale.request_submit().expect("submit");
        assert!(stale.on_stale_recipe());
        assert_eq!(
            stale.state_label(),
            "レシピが更新されました — 再読み込みしてください"
        );
        assert_eq!(stale.goal(), "goal");
        stale.refresh_catalog(sample_catalog());
        assert!(matches!(stale.state(), BriefState::Editing));
    }

    #[test]
    fn an_accepted_receipt_paints_its_task_ref_and_is_never_downgraded() {
        let mut form = form_with_recipe();
        assert!(form.set_goal("goal"));
        let submission = form.request_submit().expect("submit");
        assert!(form.on_sent(&submission.command_id));
        assert_eq!(form.state_label(), "送信済み — 受付票を確認してください");

        let task = TaskId::new("task-receipt-1").expect("task");
        assert!(form.on_accepted(&submission.command_id, Some(task.clone())));
        assert_eq!(
            form.state(),
            &BriefState::Accepted {
                command_id: submission.command_id.clone(),
                task_id: Some(task),
            }
        );
        assert_eq!(form.state_label(), "受付済み — 観測待ち");
        assert!(
            !form.state_label().contains("完了") && !form.state_label().contains("観測済み"),
            "an accepted receipt is not an observed or completed task"
        );
        assert!(!form.is_editable());

        // A late unknown answer for the same command must not replace the
        // acceptance the Runtime already returned.
        assert!(
            !form.on_unknown(&submission.command_id),
            "unknown cannot downgrade an accepted receipt"
        );
        assert_eq!(form.state_label(), "受付済み — 観測待ち");
    }

    #[test]
    fn an_accepted_receipt_fills_a_missing_task_ref_but_never_clears_it() {
        let mut form = form_with_recipe();
        assert!(form.set_goal("goal"));
        let submission = form.request_submit().expect("submit");
        assert!(form.on_accepted(&submission.command_id, None));
        assert!(matches!(
            form.state(),
            BriefState::Accepted { task_id: None, .. }
        ));

        let task = TaskId::new("task-receipt-2").expect("task");
        assert!(form.on_accepted(&submission.command_id, Some(task.clone())));
        assert!(matches!(
            form.state(),
            BriefState::Accepted {
                task_id: Some(accepted),
                ..
            } if accepted == &task
        ));

        assert!(
            !form.on_accepted(&submission.command_id, None),
            "a later receipt without a task ref never clears the accepted ref"
        );
        assert!(matches!(
            form.state(),
            BriefState::Accepted {
                task_id: Some(accepted),
                ..
            } if accepted == &task
        ));
    }

    #[test]
    fn authenticated_catalog_maps_to_the_brief_view_verbatim() {
        let catalog = Catalog {
            schema_version: 1,
            can_submit: true,
            reason_codes: vec!["RECIPE_UNAVAILABLE:other".to_owned()],
            recipes: vec![CatalogRecipe {
                recipe_ref: CatalogRecipeRef {
                    recipe_id: "recipe-1".to_owned(),
                    source_harness_version_id: Some(format!("hvid_{}", "a".repeat(64))),
                    source_harness_digest: Some(format!("sha256:{}", "b".repeat(64))),
                    graph_ref: CatalogGraphRef {
                        graph_id: "graph-1".to_owned(),
                        graph_version: 3,
                        graph_digest: format!("sha256:{}", "a".repeat(64)),
                    },
                    profile_ref: Some("profile-1".to_owned()),
                    projection_digest: Some(format!("sha256:{}", "c".repeat(64))),
                },
                label: "Still".to_owned(),
                description: "One still image".to_owned(),
                input_limits: CatalogInputLimits {
                    max_images: Some(2),
                },
                accepts_image_references: true,
                execution_limits: CatalogExecutionLimits::default(),
                constraint_summary: "tasteful".to_owned(),
                parameters: vec![CatalogParameter {
                    id: "seed".to_owned(),
                    label: "Seed".to_owned(),
                    kind: "integer".to_owned(),
                    value: WireValue::Integer(7),
                    bounds: None,
                    choices: Vec::new(),
                    editable: true,
                }],
            }],
        };
        let view = BriefCatalog::from_catalog(&catalog).expect("brief view");
        assert!(view.can_submit);
        assert_eq!(view.reason_codes, vec!["RECIPE_UNAVAILABLE:other"]);
        let recipe = &view.recipes[0];
        assert_eq!(recipe.recipe_ref, sample_recipe_ref());
        assert_eq!(recipe.max_images, 2);
        assert_eq!(
            recipe.parameters[0].default_value,
            BriefParameterValue::Integer(7)
        );
    }
}

#[cfg(test)]
mod epoch_tests {
    use super::*;

    fn catalog_with_epoch(epoch: &str) -> CatalogDocument {
        CatalogDocument {
            schema_version: 1,
            server: app_core::CatalogServer {
                server_identity: "cos-runtime-orchestrator".to_owned(),
                epoch: epoch.to_owned(),
            },
            catalog: Catalog {
                schema_version: 1,
                can_submit: true,
                reason_codes: Vec::new(),
                recipes: Vec::new(),
            },
        }
    }

    #[test]
    fn authenticated_catalog_carries_its_server_epoch() {
        let document = catalog_with_epoch("epoch-9");
        let view = BriefCatalog::from_authenticated(&document).expect("catalog view");
        assert_eq!(
            view.server_epoch,
            Some(EpochId::new("epoch-9").expect("epoch"))
        );
    }

    #[test]
    fn an_empty_or_invalid_server_epoch_is_refused() {
        for raw in ["", "   "] {
            let document = catalog_with_epoch(raw);
            assert_eq!(
                BriefCatalog::from_authenticated(&document),
                Err(BriefCatalogError::MissingEpoch)
            );
        }
        // An opaque epoch beyond the id bound is not usable either.
        let document = catalog_with_epoch(&"e".repeat(10_000));
        assert_eq!(
            BriefCatalog::from_authenticated(&document),
            Err(BriefCatalogError::MissingEpoch)
        );
    }

    #[test]
    fn a_catalog_without_an_epoch_cannot_submit() {
        let mut form = BriefForm::new();
        form.set_catalog(BriefCatalog {
            can_submit: true,
            reason_codes: Vec::new(),
            server_epoch: None,
            recipes: vec![super::tests::sample_recipe()],
        });
        assert!(form.select_recipe(0));
        assert!(form.set_goal("goal"));
        assert_eq!(
            form.request_submit().err(),
            Some(BriefValidationError::EpochUnavailable)
        );
    }
}

#[cfg(test)]
mod fail_closed_tests {
    use super::*;
    use app_core::{
        CatalogExecutionLimits, CatalogGraphRef, CatalogInputLimits, CatalogParameterBounds,
        CatalogParameterChoice, CatalogParameterValue as WireValue,
    };

    fn recipe_with(max_images: Option<u32>, parameter: CatalogParameter) -> CatalogRecipe {
        CatalogRecipe {
            recipe_ref: CatalogRecipeRef {
                recipe_id: "recipe-1".to_owned(),
                source_harness_version_id: None,
                source_harness_digest: None,
                graph_ref: CatalogGraphRef {
                    graph_id: "graph-1".to_owned(),
                    graph_version: 1,
                    graph_digest: format!("sha256:{}", "a".repeat(64)),
                },
                profile_ref: None,
                projection_digest: None,
            },
            label: "Still".to_owned(),
            description: "One still".to_owned(),
            input_limits: CatalogInputLimits { max_images },
            accepts_image_references: false,
            execution_limits: CatalogExecutionLimits::default(),
            constraint_summary: "none".to_owned(),
            parameters: vec![parameter],
        }
    }

    fn parameter(
        kind: &str,
        value: WireValue,
        bounds: Option<CatalogParameterBounds>,
        choices: Vec<CatalogParameterChoice>,
    ) -> CatalogParameter {
        CatalogParameter {
            id: "p".to_owned(),
            label: "P".to_owned(),
            kind: kind.to_owned(),
            value,
            bounds,
            choices,
            editable: true,
        }
    }

    fn map(recipe: CatalogRecipe) -> Result<BriefCatalog, BriefCatalogError> {
        BriefCatalog::from_catalog(&Catalog {
            schema_version: 1,
            can_submit: true,
            reason_codes: Vec::new(),
            recipes: vec![recipe],
        })
    }

    #[test]
    fn a_missing_image_limit_refuses_the_catalog() {
        let recipe = recipe_with(
            None,
            parameter("text", WireValue::Text("x".to_owned()), None, Vec::new()),
        );
        assert_eq!(map(recipe), Err(BriefCatalogError::MissingInputLimit));
    }

    #[test]
    fn malformed_numeric_bounds_refuse_the_catalog() {
        let integer = recipe_with(
            Some(2),
            parameter(
                "integer",
                WireValue::Integer(1),
                Some(CatalogParameterBounds {
                    min: Some("not-an-integer".to_owned()),
                    max: None,
                    max_length: None,
                }),
                Vec::new(),
            ),
        );
        assert_eq!(map(integer), Err(BriefCatalogError::InvalidBounds));

        let decimal = recipe_with(
            Some(2),
            parameter(
                "decimal",
                WireValue::Text("0.5".to_owned()),
                Some(CatalogParameterBounds {
                    min: Some("1e3".to_owned()),
                    max: None,
                    max_length: None,
                }),
                Vec::new(),
            ),
        );
        assert_eq!(map(decimal), Err(BriefCatalogError::InvalidBounds));

        let inverted = recipe_with(
            Some(2),
            parameter(
                "decimal",
                WireValue::Text("0.5".to_owned()),
                Some(CatalogParameterBounds {
                    min: Some("2".to_owned()),
                    max: Some("1".to_owned()),
                    max_length: None,
                }),
                Vec::new(),
            ),
        );
        assert_eq!(map(inverted), Err(BriefCatalogError::InvalidBounds));
    }

    #[test]
    fn malformed_text_and_choice_bounds_refuse_the_catalog() {
        let negative_length = recipe_with(
            Some(2),
            parameter(
                "text",
                WireValue::Text("x".to_owned()),
                Some(CatalogParameterBounds {
                    min: None,
                    max: None,
                    max_length: Some(-1),
                }),
                Vec::new(),
            ),
        );
        assert_eq!(map(negative_length), Err(BriefCatalogError::InvalidBounds));

        let empty_choices = recipe_with(
            Some(2),
            parameter("choice", WireValue::Text("a".to_owned()), None, Vec::new()),
        );
        assert_eq!(map(empty_choices), Err(BriefCatalogError::InvalidParameter));

        let boolean_with_min = recipe_with(
            Some(2),
            parameter(
                "boolean",
                WireValue::Boolean(true),
                Some(CatalogParameterBounds {
                    min: Some("0".to_owned()),
                    max: None,
                    max_length: None,
                }),
                Vec::new(),
            ),
        );
        assert_eq!(map(boolean_with_min), Err(BriefCatalogError::InvalidBounds));
    }

    #[test]
    fn a_default_outside_its_bounds_refuses_the_catalog() {
        let recipe = recipe_with(
            Some(2),
            parameter(
                "integer",
                WireValue::Integer(9),
                Some(CatalogParameterBounds {
                    min: Some("1".to_owned()),
                    max: Some("5".to_owned()),
                    max_length: None,
                }),
                Vec::new(),
            ),
        );
        assert_eq!(map(recipe), Err(BriefCatalogError::InvalidBounds));
    }
}
