//! NAV2 Creative Brief surface (S1): catalog load, frozen form, submit states.
//!
//! The surface is presentation only: the form model lives in
//! [`ui_model::brief`], the catalog and command route live in app-core, and no
//! task row is created locally. Acceptance is shown as a receipt state; the
//! task appears only when the observation projection delivers it.

use super::*;
use ui_model::brief::BriefState;

/// Catalog load state for the Brief surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BriefCatalogState {
    /// Not requested yet.
    Idle,
    /// Request in flight.
    Loading,
    /// Authenticated catalog installed.
    Loaded,
    /// Read failed; the fixed label is shown.
    Unavailable(&'static str),
}

/// One Brief text field bound to GPUI's input handler.
pub(super) struct BriefInputRegistration {
    pub(super) entity: Entity<Shell>,
    pub(super) focus: FocusHandle,
}

impl IntoElement for BriefInputRegistration {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for BriefInputRegistration {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
    }
}

fn catalog_read_label(error: &app_core::CommandReadError) -> &'static str {
    match error {
        app_core::CommandReadError::AuthorizationUnavailable
        | app_core::CommandReadError::AuthorizationDenied => "観測の許可がありません",
        app_core::CommandReadError::Superseded => "接続が更新されました。開き直してください",
        app_core::CommandReadError::CapacityExceeded => "読み取り上限に達しました",
        app_core::CommandReadError::Client(app_core::PublicationError::NotConnected) => {
            "ランタイムに接続できません"
        }
        app_core::CommandReadError::Client(app_core::PublicationError::Unauthorized) => {
            "カタログの読み取りが許可されていません"
        }
        app_core::CommandReadError::Client(_) => "ランタイムからカタログを取得できません",
    }
}

/// One classified command-receipt observation.
///
/// The variants never conflate local pending with a server rejection: a missing
/// receipt, an unconfirmed outcome, a denied read, an observed canonical effect
/// and a definitive rejection are separate states.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ReceiptObservation {
    /// The read was refused by the visibility fence.
    Denied,
    /// The client holds no retained receipt (positive "not retained" answer).
    NotRetained,
    /// The command is prepared or awaiting a response; no receipt yet.
    Pending,
    /// A receipt is expected but absent from the view.
    ReceiptMissing,
    /// Server receipt accepted the intent.
    Accepted {
        task_id: Option<desktop_protocol::TaskId>,
    },
    /// A canonical effect referenced the command; no receipt is needed.
    CanonicalObserved,
    /// Definitive server rejection with its explicit reason.
    Rejected {
        reason: Option<desktop_protocol::RejectReason>,
        message: Option<String>,
    },
    /// The command family is unsupported (stale recipe/capability).
    Unsupported,
    /// Definitive local/server failure.
    Failed,
    /// The outcome is unknown; never treated as rejection.
    Unknown,
}

fn classify_receipt(
    read_error: Option<&app_core::CommandReadError>,
    view: Option<&app_core::CommandReadView>,
) -> ReceiptObservation {
    if let Some(error) = read_error {
        return match error {
            app_core::CommandReadError::AuthorizationUnavailable
            | app_core::CommandReadError::AuthorizationDenied => ReceiptObservation::Denied,
            _ => ReceiptObservation::Unknown,
        };
    }
    let Some(view) = view else {
        return ReceiptObservation::NotRetained;
    };
    match view.presentation {
        app_core::CommandPresentation::ReceiptAvailable => match &view.receipt {
            Some(receipt) => match receipt.state {
                desktop_protocol::ReceiptState::Accepted => ReceiptObservation::Accepted {
                    task_id: receipt.task_id.clone(),
                },
                desktop_protocol::ReceiptState::CanonicalReflected => {
                    ReceiptObservation::CanonicalObserved
                }
                desktop_protocol::ReceiptState::Rejected => ReceiptObservation::Rejected {
                    reason: receipt.reject_reason.clone(),
                    message: receipt.message.clone(),
                },
                desktop_protocol::ReceiptState::Unknown(_) => ReceiptObservation::Unknown,
            },
            None => ReceiptObservation::ReceiptMissing,
        },
        app_core::CommandPresentation::CanonicalEffectObserved => {
            ReceiptObservation::CanonicalObserved
        }
        app_core::CommandPresentation::Unsupported => ReceiptObservation::Unsupported,
        app_core::CommandPresentation::Prepared
        | app_core::CommandPresentation::AwaitingResponse => ReceiptObservation::Pending,
        app_core::CommandPresentation::OutcomeUnconfirmed => ReceiptObservation::Unknown,
        app_core::CommandPresentation::Failed => ReceiptObservation::Failed,
    }
}

fn reject_code(reason: &desktop_protocol::RejectReason) -> &'static str {
    match reason {
        desktop_protocol::RejectReason::ValidationFailed => "VALIDATION_FAILED",
        desktop_protocol::RejectReason::Unauthorized => "UNAUTHORIZED",
        desktop_protocol::RejectReason::Forbidden => "FORBIDDEN",
        desktop_protocol::RejectReason::RevisionConflict => "REVISION_CONFLICT",
        desktop_protocol::RejectReason::UnsupportedCommand => "UNSUPPORTED_COMMAND",
        desktop_protocol::RejectReason::CommandIdConflict => "COMMAND_ID_CONFLICT",
        desktop_protocol::RejectReason::InternalError => "INTERNAL_ERROR",
        desktop_protocol::RejectReason::Unknown(_) => "UNKNOWN_REJECTION",
    }
}

fn bounded_message(message: Option<&str>, fallback: &'static str) -> String {
    let Some(message) = message else {
        return fallback.to_owned();
    };
    let bounded: String = message
        .chars()
        .filter(|character| !character.is_control())
        .take(200)
        .collect();
    if bounded.trim().is_empty() {
        fallback.to_owned()
    } else {
        bounded
    }
}

impl Shell {
    /// Open the central Brief surface and load the authenticated catalog.
    pub(super) fn brief_open_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.brief_open {
            self.brief_goal_focus.focus(window, cx);
            return;
        }
        self.brief_open = true;
        self.brief_form = ui_model::brief::BriefForm::new();
        self.brief_catalog_state = BriefCatalogState::Loading;
        self.brief_notice = None;
        self.brief_command = None;
        self.brief_catalog_epoch = None;
        self.brief_input_selection = 0..0;
        self.brief_goal_focus.focus(window, cx);
        self.load_brief_catalog(cx);
        cx.notify();
    }

    /// Close the Brief surface (local only; no command is issued).
    pub(super) fn brief_close_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.brief_open = false;
        self.brief_notice = None;
        self.brief_command = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn load_brief_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            self.brief_catalog_state = BriefCatalogState::Unavailable("観測が構成されていません");
            return;
        };
        self.brief_catalog_state = BriefCatalogState::Loading;
        self.brief_catalog_task = cx.spawn(async move |this, cx| {
            let result = handle.fetch_catalog().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(document) => {
                        match ui_model::brief::BriefCatalog::from_authenticated(&document) {
                            Ok(catalog) => {
                                this.brief_catalog_epoch = catalog.server_epoch.clone();
                                this.brief_form.set_catalog(catalog);
                                this.brief_catalog_state = BriefCatalogState::Loaded;
                                this.brief_notice = None;
                            }
                            Err(_) => {
                                this.brief_catalog_state =
                                    BriefCatalogState::Unavailable("カタログを読み取れません");
                            }
                        }
                    }
                    Err(error) => {
                        this.brief_catalog_state =
                            BriefCatalogState::Unavailable(catalog_read_label(&error));
                    }
                }
                cx.notify();
            });
        });
    }

    /// Reload the catalog after a stale-recipe refusal (keeps the draft).
    pub(super) fn brief_reload_catalog(&mut self, cx: &mut Context<Self>) {
        self.load_brief_catalog(cx);
        cx.notify();
    }

    /// Server-scope capability denial for the create action, when one applies.
    ///
    /// The Runtime normally projects capabilities per run. A projected
    /// server-scope projection is authoritative for `CREATE_TASK`; a dangling
    /// reference, a scope mismatch or an ambiguous set fails closed. Absence is
    /// not an offer either, but the authenticated catalog remains the create
    /// authority for that pre-existing path, so it is not overridden here.
    fn brief_create_capability_denial(&self) -> Option<app_core::capability::CapabilityGate> {
        match self.authorized_text().map(|text| &text.create_capability) {
            None | Some(app_core::capability::CapabilityProjection::NotProjected) => None,
            Some(projection) => {
                let gate = projection.gate_command(&desktop_protocol::CommandOperation::CreateTask);
                (!gate.enabled).then_some(gate)
            }
        }
    }

    /// Validate and submit the Brief through the owner command path.
    pub(super) fn brief_submit(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            self.brief_notice = Some("観測が構成されていません".to_owned());
            cx.notify();
            return;
        };
        if self.brief_catalog_epoch.is_none() {
            self.brief_notice =
                Some("カタログのエポックが不明です。再読み込みしてください".to_owned());
            cx.notify();
            return;
        }
        let submission = match self.brief_form.request_submit() {
            Ok(submission) => submission,
            Err(error) => {
                self.brief_notice = Some(brief_validation_label(&error).to_owned());
                cx.notify();
                return;
            }
        };
        let submission_id = submission.command_id.clone();
        self.brief_command = Some(submission_id.clone());
        self.brief_notice = Some("送信中…".to_owned());
        let envelope = submission.into_envelope();
        let request = handle.request_create_task(envelope);
        self.brief_submit_task = cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(command_id) => {
                        // The channel admitted the intent; the receipt is still
                        // pending, so the state is Sent, not Unknown.
                        let _ = this.brief_form.on_sent(&command_id);
                        this.brief_notice = Some("送信済み — 受付票を確認してください".to_owned());
                        this.brief_check_receipt(cx);
                    }
                    Err(error) => {
                        // Local refusal: nothing was sent, so return to editing
                        // instead of claiming an unknown outcome.
                        let _ = this.brief_form.on_not_sent();
                        this.brief_command = None;
                        this.brief_notice = Some(brief_command_label(&error).to_owned());
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Read the retained receipt for the current create command.
    pub(super) fn brief_check_receipt(&mut self, cx: &mut Context<Self>) {
        let (Some(handle), Some(command_id)) = (self.handle.clone(), self.brief_command.clone())
        else {
            self.brief_notice = Some("確認できる受付票がありません".to_owned());
            cx.notify();
            return;
        };
        let read = handle.read_command(command_id.clone());
        self.brief_submit_task = cx.spawn(async move |this, cx| {
            let (view, read_error) = match read.await {
                Ok(authorized) => {
                    let mut found = None;
                    match authorized.read(|read| match read {
                        app_core::CommandRead::Found(view) => Some(view.clone()),
                        app_core::CommandRead::NotRetained => None,
                    }) {
                        Ok(tagged) => {
                            let _ = tagged.consume(|value| found = value.clone());
                            (found, None)
                        }
                        Err(_) => (None, Some(app_core::CommandReadError::Superseded)),
                    }
                }
                Err(error) => (None, Some(error)),
            };
            let observation = classify_receipt(read_error.as_ref(), view.as_ref());
            let _ = this.update(cx, |this, cx| {
                this.apply_receipt_observation(&command_id, observation);
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Apply one classified receipt observation without conflating local
    /// pending, unknown, denial or server rejection.
    fn apply_receipt_observation(
        &mut self,
        command_id: &desktop_protocol::CommandId,
        observation: ReceiptObservation,
    ) {
        match observation {
            ReceiptObservation::Denied => {
                self.brief_notice =
                    Some("受付票を開示できません（観測の許可がありません）".to_owned());
            }
            ReceiptObservation::NotRetained => {
                let _ = self.brief_form.on_unknown(command_id);
                self.brief_notice = Some("受付票は保持されていません — 結果は不明です".to_owned());
            }
            ReceiptObservation::Pending | ReceiptObservation::ReceiptMissing => {
                self.brief_notice = Some("送信済み — 受付票を待っています".to_owned());
            }
            ReceiptObservation::Accepted { task_id } => {
                if self.brief_form.on_accepted(command_id, task_id) {
                    self.brief_notice = None;
                } else {
                    self.brief_notice = Some("この受付票は現在の提出には一致しません".to_owned());
                }
            }
            ReceiptObservation::CanonicalObserved => {
                if self.brief_form.on_accepted(command_id, None) {
                    self.brief_notice =
                        Some("実行結果が観測されました（受付票はありません）".to_owned());
                }
            }
            ReceiptObservation::Rejected { reason, message } => match reason {
                Some(desktop_protocol::RejectReason::UnsupportedCommand) => {
                    let _ = self.brief_form.on_stale_recipe();
                    self.brief_notice =
                        Some("レシピが利用できません。カタログを再読み込みしてください".to_owned());
                }
                Some(reason) => {
                    let code = reject_code(&reason);
                    let _ = self.brief_form.on_rejected(command_id, code);
                    self.brief_notice = Some(bounded_message(
                        message.as_deref(),
                        "コマンドは拒否されました",
                    ));
                }
                None => {
                    let _ = self.brief_form.on_rejected(command_id, "REJECTED");
                    self.brief_notice = Some(bounded_message(
                        message.as_deref(),
                        "コマンドは拒否されました",
                    ));
                }
            },
            ReceiptObservation::Unsupported => {
                let _ = self.brief_form.on_stale_recipe();
                self.brief_notice =
                    Some("コマンドが利用できません。カタログを再読み込みしてください".to_owned());
            }
            ReceiptObservation::Failed => {
                let _ = self.brief_form.on_rejected(command_id, "FAILED");
                self.brief_notice = Some("コマンドは失敗しました".to_owned());
            }
            ReceiptObservation::Unknown => {
                let _ = self.brief_form.on_unknown(command_id);
                self.brief_notice = Some(
                    "結果不明 — 受付票を確認するか、新しい提出として送り直してください".to_owned(),
                );
            }
        }
    }

    /// Cycle keyboard focus only inside the open Brief surface (focus trap).
    pub(super) fn brief_cycle_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        forward: bool,
    ) {
        if !self.brief_open {
            return;
        }
        let handles = [
            &self.brief_goal_focus,
            &self.brief_direction_focus,
            &self.brief_submit_focus,
            &self.brief_recheck_focus,
            &self.brief_reload_focus,
        ];
        let count = handles.len();
        let current = handles.iter().position(|handle| handle.is_focused(window));
        let next = match (current, forward) {
            (Some(index), true) => (index + 1) % count,
            (Some(0), false) => count - 1,
            (Some(index), false) => index - 1,
            (None, _) => 0,
        };
        handles[next].focus(window, cx);
    }

    /// Render the central Brief surface (hidden unless open).
    pub(super) fn brief_overlay(
        &mut self,
        palette: crate::contrast::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.brief_open {
            return div().into_any_element();
        }
        let colors = palette.colors;
        let text_color = rgb(palette.readable(colors.text, colors.background));
        let muted = rgb(palette.readable(colors.muted_text, colors.background));
        let entity = cx.entity();
        let mut children: Vec<AnyElement> = Vec::new();
        children.push(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_color(text_color)
                        .child("新しいタスク — クリエイティブ・ブリーフ"),
                )
                .child(
                    div()
                        .id("brief-close")
                        .role(Role::Button)
                        .aria_label("ブリーフを閉じる")
                        .px_3()
                        .py_1()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.brief_close_surface(window, cx);
                        }))
                        .child("閉じる"),
                )
                .into_any_element(),
        );

        match self.brief_catalog_state {
            BriefCatalogState::Idle | BriefCatalogState::Loading => {
                children.push(
                    div()
                        .text_color(muted)
                        .child("認証済みカタログを読み込んでいます…")
                        .into_any_element(),
                );
            }
            BriefCatalogState::Unavailable(label) => {
                children.push(
                    div()
                        .text_color(rgb(colors.danger_text))
                        .child(SharedString::from(label))
                        .into_any_element(),
                );
                children.push(
                    div()
                        .id("brief-reload")
                        .role(Role::Button)
                        .aria_label("カタログを再読み込み")
                        .track_focus(&self.brief_reload_focus)
                        .px_3()
                        .py_1()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .on_click(cx.listener(|this, _, _, cx| this.brief_reload_catalog(cx)))
                        .child("再読み込み")
                        .into_any_element(),
                );
            }
            BriefCatalogState::Loaded => {}
        }
        if matches!(self.brief_catalog_state, BriefCatalogState::Loaded) {
            let can_submit = self
                .brief_form
                .catalog()
                .map(|catalog| catalog.can_submit)
                .unwrap_or(false);
            if !can_submit {
                let reasons = self
                    .brief_form
                    .catalog()
                    .map(|catalog| catalog.reason_codes.join(" / "))
                    .unwrap_or_default();
                children.push(
                    div()
                        .text_color(rgb(colors.danger_text))
                        .child(SharedString::from(if reasons.is_empty() {
                            "この環境ではタスクを作成できません（理由コードなし）".to_owned()
                        } else {
                            format!("この環境ではタスクを作成できません。理由: {reasons}")
                        }))
                        .into_any_element(),
                );
            }
            children.push(
                div()
                    .text_color(text_color)
                    .child("目的（必須・8000文字まで）")
                    .into_any_element(),
            );
            let goal = self.brief_form.goal().to_owned();
            let goal_display = if goal.is_empty() {
                SharedString::from("何を達成するかを入力…")
            } else {
                SharedString::from(goal)
            };
            children.push(
                div()
                    .id("brief-goal")
                    .role(Role::TextInput)
                    .aria_label("目的")
                    .aria_value(goal_display.clone())
                    .track_focus(&self.brief_goal_focus)
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .text_color(text_color)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.brief_goal_focus.focus(window, cx);
                        let end = this.brief_form.goal().len();
                        this.brief_input_selection = end..end;
                        cx.notify();
                    }))
                    .child(goal_display)
                    .child(div().absolute().size_full().child(BriefInputRegistration {
                        entity: entity.clone(),
                        focus: self.brief_goal_focus.clone(),
                    }))
                    .into_any_element(),
            );
            children.push(
                div()
                    .text_color(text_color)
                    .child("方向性（任意・8000文字まで）")
                    .into_any_element(),
            );
            let direction = self.brief_form.direction().to_owned();
            children.push(
                div()
                    .id("brief-direction")
                    .role(Role::TextInput)
                    .aria_label("方向性")
                    .aria_value(SharedString::from(direction.clone()))
                    .track_focus(&self.brief_direction_focus)
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .text_color(text_color)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.brief_direction_focus.focus(window, cx);
                        let end = this.brief_form.direction().len();
                        this.brief_input_selection = end..end;
                        cx.notify();
                    }))
                    .child(SharedString::from(direction))
                    .child(div().absolute().size_full().child(BriefInputRegistration {
                        entity: entity.clone(),
                        focus: self.brief_direction_focus.clone(),
                    }))
                    .into_any_element(),
            );

            let recipes = self
                .brief_form
                .catalog()
                .map(|catalog| catalog.recipes.clone())
                .unwrap_or_default();
            let selected = self.brief_form.selected_recipe();
            children.push(
                div()
                    .text_color(text_color)
                    .child("レシピ")
                    .into_any_element(),
            );
            for (index, recipe) in recipes.iter().enumerate() {
                let active = selected == Some(index);
                let label = SharedString::from(recipe.label.clone());
                children.push(
                    div()
                        .id(("brief-recipe", index))
                        .role(Role::Button)
                        .aria_label(label.clone())
                        .aria_selected(active)
                        .px_2()
                        .py_1()
                        .border_1()
                        .border_color(rgb(if active { colors.focus } else { colors.border }))
                        .text_color(text_color)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.brief_form.select_recipe(index) {
                                cx.notify();
                            }
                        }))
                        .child(label)
                        .into_any_element(),
                );
            }
            if let Some(index) = selected
                && let Some(catalog) = self.brief_form.catalog()
                && let Some(recipe) = catalog.recipes.get(index)
            {
                children.push(
                    div()
                        .text_color(muted)
                        .child(SharedString::from(recipe.constraint_summary.clone()))
                        .into_any_element(),
                );
                for parameter in &recipe.parameters {
                    let value = self
                        .brief_form
                        .effective_parameter(&parameter.id)
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    children.push(
                        div()
                            .text_color(muted)
                            .child(SharedString::from(format!(
                                "{}: {}（既定値・詳細編集は次のスライス）",
                                parameter.label, value
                            )))
                            .into_any_element(),
                    );
                }
            }
        }

        children.push(
            div()
                .text_color(muted)
                .child(SharedString::from(self.brief_form.state_label()))
                .into_any_element(),
        );
        if let BriefState::Accepted {
            task_id: Some(task_id),
            ..
        } = self.brief_form.state()
        {
            children.push(
                div()
                    .text_color(text_color)
                    .child(SharedString::from(format!("受付タスク: {task_id}")))
                    .into_any_element(),
            );
        }
        let state = self.brief_form.state().clone();
        let busy = matches!(
            state,
            BriefState::Submitting { .. } | BriefState::Sent { .. }
        );
        if matches!(state, BriefState::Accepted { .. }) {
            children.push(
                div()
                    .text_color(muted)
                    .child("受付済みのため編集できません")
                    .into_any_element(),
            );
        }
        if matches!(state, BriefState::StaleRecipe) {
            children.push(
                div()
                    .id("brief-stale-reload")
                    .role(Role::Button)
                    .aria_label("カタログを再読み込み")
                    .track_focus(&self.brief_reload_focus)
                    .px_3()
                    .py_1()
                    .border_1()
                    .border_color(rgb(colors.focus))
                    .on_click(cx.listener(|this, _, _, cx| this.brief_reload_catalog(cx)))
                    .child("カタログを再読み込み")
                    .into_any_element(),
            );
        }
        let validation = self.brief_form.validation_error();
        if let Some(error) = validation.as_ref()
            && !busy
        {
            children.push(
                div()
                    .text_color(rgb(colors.danger_text))
                    .child(SharedString::from(brief_validation_label(error)))
                    .into_any_element(),
            );
        }
        let mut actions: Vec<AnyElement> = Vec::new();
        if self.brief_command.is_some() {
            actions.push(
                div()
                    .id("brief-recheck")
                    .role(Role::Button)
                    .aria_label("受付票を再確認")
                    .track_focus(&self.brief_recheck_focus)
                    .px_3()
                    .py_1()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .on_click(cx.listener(|this, _, _, cx| this.brief_check_receipt(cx)))
                    .child("受付票を確認")
                    .into_any_element(),
            );
        }
        let can_attempt = matches!(
            state,
            BriefState::Editing | BriefState::Unknown { .. } | BriefState::Rejected { .. }
        );
        if !matches!(state, BriefState::Sent { .. }) {
            let create_denial = self.brief_create_capability_denial();
            let enabled = can_attempt
                && !busy
                && self.handle.is_some()
                && self.brief_catalog_epoch.is_some()
                && matches!(self.brief_catalog_state, BriefCatalogState::Loaded)
                && self
                    .brief_form
                    .catalog()
                    .is_some_and(|catalog| catalog.can_submit)
                && create_denial.is_none()
                && validation.is_none();
            let disabled_reason = if self.handle.is_none() {
                "観測が構成されていません"
            } else if self.brief_catalog_epoch.is_none() {
                "カタログのエポックが不明です"
            } else if let Some(gate) = &create_denial {
                gate.reason.as_ref()
            } else if validation.is_some() {
                "入力内容を確認してください"
            } else if matches!(state, BriefState::StaleRecipe) {
                "カタログを再読み込みしてください"
            } else {
                "提出は現在できません"
            };
            actions.push(
                div()
                    .id("brief-submit")
                    .role(Role::Button)
                    .aria_label(if enabled {
                        "ブリーフを提出"
                    } else {
                        "ブリーフを提出（無効）"
                    })
                    .track_focus(&self.brief_submit_focus)
                    .aria_description(SharedString::from(if enabled {
                        "入力を検証して提出します"
                    } else {
                        disabled_reason
                    }))
                    .when(enabled, |element| {
                        element.on_click(cx.listener(|this, _, _, cx| this.brief_submit(cx)))
                    })
                    .px_3()
                    .py_1()
                    .border_1()
                    .border_color(rgb(if enabled {
                        colors.focus
                    } else {
                        colors.disabled_border
                    }))
                    .text_color(rgb(if enabled {
                        colors.text
                    } else {
                        colors.disabled
                    }))
                    .child(if busy { "送信中" } else { "提出" })
                    .into_any_element(),
            );
        }
        children.push(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .children(actions)
                .into_any_element(),
        );
        if let Some(notice) = self.brief_notice.clone() {
            children.push(
                div()
                    .text_color(rgb(colors.muted_text))
                    .child(SharedString::from(notice))
                    .into_any_element(),
            );
        }

        div()
            .id("brief-surface")
            .role(Role::Dialog)
            .aria_label("クリエイティブ・ブリーフ")
            .key_context("Brief")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt_8()
            .bg(rgb(colors.background))
            .track_focus(&self.brief_focus)
            .child(
                div()
                    .w(px(680.))
                    .max_h(px(720.))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .bg(rgb(colors.raised_face))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .children(children),
            )
            .into_any_element()
    }
}

fn brief_validation_label(error: &ui_model::brief::BriefValidationError) -> &'static str {
    use ui_model::brief::BriefValidationError as E;
    match error {
        E::CatalogUnavailable => "カタログがありません",
        E::CatalogNotSubmittable => "この環境では作成できません",
        E::NoRecipe => "レシピを選択してください",
        E::GoalRequired => "目的を入力してください",
        E::GoalTooLong => "目的が長すぎます",
        E::DirectionTooLong => "方向性が長すぎます",
        E::TooManyReferences => "参照画像が多すぎます",
        E::ReferencesUnsupported => "このレシピは参照画像を受け付けません",
        E::DuplicateReference => "同じ参照画像が重複しています",
        E::ParameterType { .. } => "パラメータの型が不正です",
        E::ReadOnlyParameter { .. } => "読み取り専用パラメータは変更できません",
        E::ParameterBounds { .. } => "パラメータが範囲外です",
        E::ParameterChoice { .. } => "選択肢にない値です",
        E::DecimalNotCanonical { .. } => "小数の形式が不正です",
        E::Submitting => "送信中です",
        E::EpochUnavailable => "カタログのエポックが不明です。再読み込みしてください",
        E::AwaitingReceipt => "送信済みの受付票を待っています",
    }
}

fn brief_command_label(error: &app_core::CommandReadError) -> &'static str {
    match error {
        app_core::CommandReadError::AuthorizationUnavailable
        | app_core::CommandReadError::AuthorizationDenied => "観測の許可がありません",
        app_core::CommandReadError::Superseded => "接続が更新されました。開き直してください",
        app_core::CommandReadError::CapacityExceeded => "コマンド容量に達しました",
        app_core::CommandReadError::Client(app_core::PublicationError::NotConnected) => {
            "コマンド経路が接続されていません"
        }
        app_core::CommandReadError::Client(app_core::PublicationError::InvalidCommand(_)) => {
            "コマンドがローカルで拒否されました"
        }
        app_core::CommandReadError::Client(_) => "コマンドを送信できません",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui_model::brief::BriefValidationError;

    #[test]
    fn brief_validation_labels_are_closed_and_nonempty() {
        let all = [
            BriefValidationError::CatalogUnavailable,
            BriefValidationError::CatalogNotSubmittable,
            BriefValidationError::NoRecipe,
            BriefValidationError::GoalRequired,
            BriefValidationError::GoalTooLong,
            BriefValidationError::DirectionTooLong,
            BriefValidationError::TooManyReferences,
            BriefValidationError::ReferencesUnsupported,
            BriefValidationError::DuplicateReference,
            BriefValidationError::ParameterType { id: "p".to_owned() },
            BriefValidationError::ReadOnlyParameter { id: "p".to_owned() },
            BriefValidationError::ParameterBounds { id: "p".to_owned() },
            BriefValidationError::ParameterChoice { id: "p".to_owned() },
            BriefValidationError::DecimalNotCanonical { id: "p".to_owned() },
            BriefValidationError::Submitting,
        ];
        for error in all {
            assert!(!brief_validation_label(&error).is_empty());
        }
    }

    #[test]
    fn command_and_catalog_failures_have_fixed_labels() {
        use app_core::{CommandReadError, PublicationError};
        let failures = [
            CommandReadError::AuthorizationUnavailable,
            CommandReadError::AuthorizationDenied,
            CommandReadError::Superseded,
            CommandReadError::CapacityExceeded,
            CommandReadError::Client(PublicationError::Transport),
        ];
        for failure in failures {
            assert!(!catalog_read_label(&failure).is_empty());
            assert!(!brief_command_label(&failure).is_empty());
        }
        assert!(!catalog_read_label(&CommandReadError::Superseded).contains("token"));
    }
}

#[cfg(test)]
mod receipt_taxonomy_tests {
    use super::*;
    use app_core::{CommandPresentation, CommandReadView};

    fn view(presentation: CommandPresentation) -> CommandReadView {
        CommandReadView {
            presentation,
            receipt: None,
            issue: None,
        }
    }

    fn receipt(
        state: desktop_protocol::ReceiptState,
        reason: Option<desktop_protocol::RejectReason>,
    ) -> desktop_protocol::CommandReceipt {
        desktop_protocol::CommandReceipt {
            command_id: desktop_protocol::CommandId::generate(),
            operation: desktop_protocol::CommandOperation::CreateTask,
            state,
            task_id: None,
            accepted_at: None,
            reject_reason: reason,
            effect_refs: Vec::new(),
            dedupe_expires_at: None,
            message: Some("safe detail".to_owned()),
        }
    }

    #[test]
    fn every_receipt_answer_keeps_its_own_state() {
        assert_eq!(
            classify_receipt(Some(&app_core::CommandReadError::AuthorizationDenied), None),
            ReceiptObservation::Denied
        );
        assert_eq!(
            classify_receipt(None, None),
            ReceiptObservation::NotRetained
        );
        assert_eq!(
            classify_receipt(None, Some(&view(CommandPresentation::Prepared))),
            ReceiptObservation::Pending
        );
        assert_eq!(
            classify_receipt(None, Some(&view(CommandPresentation::AwaitingResponse))),
            ReceiptObservation::Pending
        );
        assert_eq!(
            classify_receipt(None, Some(&view(CommandPresentation::ReceiptAvailable))),
            ReceiptObservation::ReceiptMissing
        );
        assert_eq!(
            classify_receipt(
                None,
                Some(&view(CommandPresentation::CanonicalEffectObserved))
            ),
            ReceiptObservation::CanonicalObserved
        );
        assert_eq!(
            classify_receipt(None, Some(&view(CommandPresentation::Unsupported))),
            ReceiptObservation::Unsupported
        );
        assert_eq!(
            classify_receipt(None, Some(&view(CommandPresentation::Failed))),
            ReceiptObservation::Failed
        );
        assert_eq!(
            classify_receipt(None, Some(&view(CommandPresentation::OutcomeUnconfirmed))),
            ReceiptObservation::Unknown
        );
    }

    #[test]
    fn accepted_and_rejected_receipts_are_distinguished() {
        let mut accepted = view(CommandPresentation::ReceiptAvailable);
        accepted.receipt = Some(receipt(desktop_protocol::ReceiptState::Accepted, None));
        assert_eq!(
            classify_receipt(None, Some(&accepted)),
            ReceiptObservation::Accepted { task_id: None }
        );

        let mut stale = view(CommandPresentation::ReceiptAvailable);
        stale.receipt = Some(receipt(
            desktop_protocol::ReceiptState::Rejected,
            Some(desktop_protocol::RejectReason::UnsupportedCommand),
        ));
        assert_eq!(
            classify_receipt(None, Some(&stale)),
            ReceiptObservation::Rejected {
                reason: Some(desktop_protocol::RejectReason::UnsupportedCommand),
                message: Some("safe detail".to_owned()),
            }
        );

        let mut validation = view(CommandPresentation::ReceiptAvailable);
        validation.receipt = Some(receipt(
            desktop_protocol::ReceiptState::Rejected,
            Some(desktop_protocol::RejectReason::ValidationFailed),
        ));
        assert_eq!(
            classify_receipt(None, Some(&validation)),
            ReceiptObservation::Rejected {
                reason: Some(desktop_protocol::RejectReason::ValidationFailed),
                message: Some("safe detail".to_owned()),
            }
        );
    }

    #[test]
    fn rejection_codes_are_explicit_for_every_reason() {
        use desktop_protocol::RejectReason;
        for reason in [
            RejectReason::ValidationFailed,
            RejectReason::Unauthorized,
            RejectReason::Forbidden,
            RejectReason::RevisionConflict,
            RejectReason::UnsupportedCommand,
            RejectReason::CommandIdConflict,
            RejectReason::InternalError,
        ] {
            assert!(!reject_code(&reason).is_empty());
        }
    }
}
