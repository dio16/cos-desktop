//! NAV2 Creative Brief surface (S1): catalog load, frozen form, submit states.
//!
//! The surface is presentation only: the form model lives in
//! [`ui_model::brief`], the catalog and command route live in app-core, and no
//! task row is created locally. Acceptance is shown as a receipt state; the
//! task appears only when the observation projection delivers it.

use super::*;
use desktop_protocol::{CommandEnvelope, CommandPayload, CommandTarget};
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
        app_core::CommandReadError::Client(_) => "ランタイムからカタログを取得できません",
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

    /// Validate and submit the Brief through the owner command path.
    pub(super) fn brief_submit(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            self.brief_notice = Some("観測が構成されていません".to_owned());
            cx.notify();
            return;
        };
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
        let envelope = CommandEnvelope {
            command_id: submission.command_id,
            target: CommandTarget::Creation {
                recipe_ref: submission.recipe_ref,
            },
            expected_revision: None,
            epoch: None,
            payload: CommandPayload::CreateTask(submission.payload),
        };
        let request = handle.request_create_task(envelope);
        self.brief_submit_task = cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(command_id) => {
                        // Admission only; the receipt decides accepted/rejected.
                        let _ = this.brief_form.on_unknown(&command_id);
                        this.brief_notice = Some(
                            "受付票を確認しています。結果が不明な場合は同じ内容で再試行できます。"
                                .to_owned(),
                        );
                        this.brief_check_receipt(cx);
                    }
                    Err(error) => {
                        let _ = this.brief_form.on_unknown(&submission_id);
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
            return;
        };
        let read = handle.read_command(command_id.clone());
        self.brief_submit_task = cx.spawn(async move |this, cx| {
            let answer = read.await;
            let mut derived = None;
            if let Ok(authorized) = answer {
                let tagged = authorized.read(|read| match read {
                    app_core::CommandRead::Found(view) => {
                        Some((view.presentation, view.receipt.clone(), view.issue.clone()))
                    }
                    app_core::CommandRead::NotRetained => None,
                });
                if let Ok(tagged) = tagged {
                    let _ = tagged.consume(|value| derived = value.clone());
                }
            }
            let _ = this.update(cx, |this, cx| {
                match derived {
                    Some((presentation, receipt, _issue)) => match presentation {
                        app_core::CommandPresentation::ReceiptAvailable => {
                            if let Some(receipt) = receipt {
                                match receipt.state {
                                    desktop_protocol::ReceiptState::Accepted
                                    | desktop_protocol::ReceiptState::CanonicalReflected => {
                                        let task_id = receipt.task_id.clone();
                                        if this.brief_form.on_accepted(&command_id, task_id) {
                                            this.brief_notice = None;
                                        }
                                    }
                                    desktop_protocol::ReceiptState::Rejected => {
                                        let reason = receipt
                                            .message
                                            .clone()
                                            .unwrap_or_else(|| "拒否されました".to_owned());
                                        let _ = this.brief_form.on_rejected(&command_id, reason);
                                    }
                                    desktop_protocol::ReceiptState::Unknown(_) => {
                                        this.brief_notice = Some(
                                            "受付状態が不明です。同じ内容で再試行できます。"
                                                .to_owned(),
                                        );
                                    }
                                }
                            }
                        }
                        app_core::CommandPresentation::OutcomeUnconfirmed => {
                            this.brief_notice =
                                Some("結果を確認できません。同じ内容で再試行できます。".to_owned());
                        }
                        app_core::CommandPresentation::Failed => {
                            let _ = this
                                .brief_form
                                .on_rejected(&command_id, "コマンドは失敗しました");
                        }
                        _ => {
                            this.brief_notice = Some(
                                "受付票はまだ観測されていません。少し後に再確認してください。"
                                    .to_owned(),
                            );
                        }
                    },
                    None => {
                        this.brief_notice =
                            Some("受付票を保持していません。同じ内容で再試行できます。".to_owned());
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
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
                children.push(
                    div()
                        .text_color(rgb(colors.danger_text))
                        .child("この環境ではタスクを作成できません（理由コードを確認してください）")
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
                        .map(|value| format!("{value:?}"))
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
        if let Some(notice) = self.brief_notice.clone() {
            children.push(
                div()
                    .text_color(rgb(colors.muted_text))
                    .child(SharedString::from(notice))
                    .into_any_element(),
            );
        }
        let submitting = matches!(self.brief_form.state(), BriefState::Submitting { .. });
        let action = if self.brief_command.is_some() && !submitting {
            div()
                .id("brief-recheck")
                .role(Role::Button)
                .aria_label("受付票を再確認")
                .px_3()
                .py_1()
                .border_1()
                .border_color(rgb(colors.border))
                .on_click(cx.listener(|this, _, _, cx| this.brief_check_receipt(cx)))
                .child("受付票を確認")
                .into_any_element()
        } else {
            let enabled = !submitting
                && matches!(self.brief_catalog_state, BriefCatalogState::Loaded)
                && self.brief_form.validation_error().is_none();
            div()
                .id("brief-submit")
                .role(Role::Button)
                .aria_label("ブリーフを提出")
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
                .child(if submitting { "送信中" } else { "提出" })
                .into_any_element()
        };
        children.push(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(action)
                .into_any_element(),
        );

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
    }
}

fn brief_command_label(error: &app_core::CommandReadError) -> &'static str {
    match error {
        app_core::CommandReadError::AuthorizationUnavailable
        | app_core::CommandReadError::AuthorizationDenied => "観測の許可がありません",
        app_core::CommandReadError::Superseded => "接続が更新されました。開き直してください",
        app_core::CommandReadError::CapacityExceeded => "コマンド容量に達しました",
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
