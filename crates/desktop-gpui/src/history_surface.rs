//! NAV3 History surface (DC-MP-NAV3): timeline, bounded load-older and local reopen.
//!
//! Presentation only. The rows come from app-core's bounded history publication
//! (the Runtime-served run-level records), the load-older continuation is the
//! server-issued cursor, and reopen applies a window-local view through
//! `ui_model::history` without issuing any command, selection or outcome change.
//!
//! Entry points are the inspector action (`履歴`) and `Ctrl+H`; while the surface
//! is open the list is keyboard operable (Up/Down/Home/End, Enter/Space to
//! reopen) and every state — loading, unloaded, authoritative empty, end,
//! bound, error and unauthorized — has its own accessible label.

use super::*;
use app_core::{HistoryPublication, HistoryRequest};
use ui_model::history::{HistoryLoadState, HistoryView};

/// The publication a surface renders while no owner exists.
fn unavailable_view() -> HistoryView {
    HistoryView {
        status_label: Arc::from("観測が構成されていません"),
        load_older_label: Arc::from("観測が構成されていません"),
        ..HistoryView::default()
    }
}

/// The load-older control decision for one render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum OlderAction {
    /// The bounded action is offered with this label.
    Action(String),
    /// The action is not offered; the label states the exact reason.
    Reason(String),
}

/// Pure render decision derived from the view model and shell availability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HistoryPresentation {
    pub title: String,
    pub status_label: String,
    pub row_labels: Vec<String>,
    pub older: OlderAction,
    pub reload: bool,
    pub empty_placeholder: Option<String>,
}

/// Map one history view onto the surface's render decisions.
///
/// This is the adapter worth testing: an unloaded page must never render as an
/// absence, the load-older action appears only with a real continuation, and a
/// bounded or ended view states its reason instead of hiding the control.
pub(super) fn presentation_for(
    task: Option<&str>,
    view: &HistoryView,
    has_handle: bool,
) -> HistoryPresentation {
    let task_label = task.unwrap_or("タスク未選択");
    let older = if view.can_load_older {
        OlderAction::Action(view.load_older_label.to_string())
    } else {
        OlderAction::Reason(view.load_older_label.to_string())
    };
    HistoryPresentation {
        title: format!("履歴 — {task_label}"),
        status_label: view.status_label.to_string(),
        row_labels: view
            .rows
            .iter()
            .map(|row| row.accessible_label.to_string())
            .collect(),
        older,
        reload: has_handle
            && matches!(
                view.state,
                HistoryLoadState::Error(_)
                    | HistoryLoadState::Unauthorized
                    | HistoryLoadState::Idle
            ),
        empty_placeholder: view.rows.is_empty().then(|| view.status_label.to_string()),
    }
}

impl Shell {
    /// Open the history surface for the selected task (local view only).
    pub(super) fn history_open_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_open {
            self.history_list_focus.focus(window, cx);
            return;
        }
        let task = self
            .rail_selection()
            .ok()
            .cloned()
            .or_else(|| self.authorized_text().and_then(|text| text.task.clone()));
        let Some(task) = task else {
            self.surface_notice = Some("履歴を表示するタスクがありません".to_owned());
            cx.notify();
            return;
        };
        self.history_restore_focus = window.focused(cx);
        self.history_open = true;
        self.history_task = Some(task.clone());
        self.history_active = Some(0);
        self.history_notice = None;
        self.history_list_focus.focus(window, cx);
        // A previously served view for this task is still authoritative; only an
        // unloaded, failed or unauthorized view needs a fresh page.
        let settled = self.history_publication().is_some_and(|publication| {
            publication.is_for(&task)
                && !matches!(
                    publication.status,
                    app_core::HistoryStatus::Idle
                        | app_core::HistoryStatus::Error(_)
                        | app_core::HistoryStatus::Unauthorized
                )
        });
        if !settled && self.handle.is_some() {
            self.history_request(HistoryRequest::first(task), cx);
        }
        cx.notify();
    }

    /// Close the surface (local only) and restore focus.
    pub(super) fn history_close_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.history_open {
            return;
        }
        self.history_open = false;
        self.history_notice = None;
        self.history_active = None;
        let restore = self
            .history_restore_focus
            .take()
            .unwrap_or_else(|| self.focus.clone());
        restore.focus(window, cx);
        cx.notify();
    }

    /// The current bounded history publication, when an owner exists.
    fn history_publication(&self) -> Option<Arc<HistoryPublication>> {
        self.handle.as_ref().map(|handle| handle.history())
    }

    /// Render the publication into the view model.
    fn history_view(&self) -> HistoryView {
        match self.history_publication() {
            Some(publication) => HistoryView::from_publication(&publication),
            None => unavailable_view(),
        }
    }

    /// Issue one bounded history request through the owner thread.
    fn history_request(&mut self, request: HistoryRequest, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            self.history_notice = Some("観測が構成されていません".to_owned());
            cx.notify();
            return;
        };
        self.history_task = Some(request.query.task_ref().clone());
        self.history_load_task = cx.spawn(async move |this, cx| {
            let answer = handle.fetch_history(request.query).await;
            let _ = this.update(cx, |this, cx| {
                match answer {
                    app_core::HistoryFetchReply::Published => {}
                    app_core::HistoryFetchReply::Superseded => {
                        this.history_notice =
                            Some("履歴の読み込みは新しい要求に置き換えられました".to_owned());
                    }
                    app_core::HistoryFetchReply::Closed => {
                        this.history_notice = Some("観測は終了しています".to_owned());
                    }
                }
                let rows = this.history_view().rows.len();
                this.history_active = match (this.history_active, rows) {
                    (_, 0) => None,
                    (Some(active), len) => Some(active.min(len - 1)),
                    (None, len) => Some(len - 1),
                };
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Load the next (older) page with the server-issued continuation cursor.
    pub(super) fn history_load_older(&mut self, cx: &mut Context<Self>) {
        if !self.history_open {
            return;
        }
        let Some(publication) = self.history_publication() else {
            return;
        };
        let view = HistoryView::from_publication(&publication);
        let Some(request) = view.older_request(&publication) else {
            return;
        };
        self.history_request(request, cx);
    }

    /// Reload the first page after an explicit failure.
    pub(super) fn history_reload(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.history_task.clone() else {
            return;
        };
        self.history_request(HistoryRequest::first(task), cx);
    }

    /// Move the active row within the currently rendered rows.
    pub(super) fn history_move_active(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.history_open {
            return;
        }
        let rows = self.history_view().rows.len();
        self.history_active = list_cursor::stepped_index(self.history_active, rows, delta);
        self.reveal_history_active();
        self.history_list_focus.focus(window, cx);
        cx.notify();
    }

    /// Home/End within the rendered rows.
    pub(super) fn history_move_edge(
        &mut self,
        edge: RailEdge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.history_open {
            return;
        }
        let rows = self.history_view().rows.len();
        self.history_active = rail_edge_index(rows, edge);
        self.reveal_history_active();
        self.history_list_focus.focus(window, cx);
        cx.notify();
    }

    /// Scroll the list so the keyboard cursor is inside the visible range.
    ///
    /// The list is a bounded scroll region, so moving the cursor without
    /// revealing it lets Enter reopen a row the user cannot see.
    fn reveal_history_active(&self) {
        if let Some(index) = self.history_active {
            self.history_scroll.scroll_to_item(index);
        }
    }

    /// Reopen the active row as a local view only.
    pub(super) fn history_reopen_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.history_active else {
            return;
        };
        self.history_reopen_row(index, window, cx);
    }

    /// Reopen one row by its rendered index.
    fn history_reopen_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.history_task.clone() else {
            return;
        };
        let view = self.history_view();
        let Some(row) = view.rows.get(index).cloned() else {
            return;
        };
        let Some(iteration) = row.reopen_iteration else {
            self.history_notice = Some("この記録の反復番号を特定できません".to_owned());
            cx.notify();
            return;
        };
        self.history_active = Some(index);
        // Candidates come only from the current authorized projection; a reopen
        // never fetches, never selects canonically and never invents a candidate.
        let candidates: Vec<desktop_protocol::CandidateRef> = self
            .authorized_text()
            .map(|text| {
                text.artifacts
                    .iter()
                    .map(|row| row.identity.clone())
                    .collect()
            })
            .unwrap_or_default();
        if self.rail_selection().ok() != Some(&task) {
            // Local rail selection and local dock task scope only.
            self.select_rail_task(&task, window, cx);
        }
        let Some(workbench) = self.workbench.as_mut() else {
            return;
        };
        let outcome = ui_model::history::reopen_local_view(
            &mut workbench.presentation,
            &ui_model::history::ReopenTarget {
                task: task.clone(),
                iteration,
            },
            &candidates,
        );
        self.history_notice = Some(outcome.label().to_owned());
        self.history_list_focus.focus(window, cx);
        // A reopen changes the local viewed candidate in place; it is the same
        // presentation transition an explicit dock selection performs, so the
        // artifact and compare panes must re-resolve instead of keeping the
        // previous candidate's picture under the new label.
        self.sync_artifact_media(cx);
        self.sync_compare_media(cx);
        cx.notify();
    }

    /// Build the History overlay for the shell.
    pub(super) fn history_overlay(
        &mut self,
        palette: crate::contrast::Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.history_open {
            return div().into_any_element();
        }
        let colors = palette.colors;
        let surface = colors.raised_face;
        let text_color = rgb(palette.readable(colors.text, surface));
        let muted = rgb(palette.readable(colors.muted_text, surface));
        let danger = rgb(colors.danger_text);
        let view = self.history_view();
        let list_focused = self.history_list_focus.is_focused(window);
        let notice = self.history_notice.clone();
        let task_label = self
            .history_task
            .clone()
            .or_else(|| self.history_publication().and_then(|p| p.task_ref.clone()))
            .map(|task| task.as_str().to_owned())
            .unwrap_or_else(|| "タスク未選択".to_owned());
        let presentation =
            presentation_for(Some(task_label.as_str()), &view, self.handle.is_some());

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
                        .child(SharedString::from(presentation.title.clone())),
                )
                .child(
                    div()
                        .id("history-close")
                        .role(Role::Button)
                        .aria_label("履歴を閉じる")
                        .px_3()
                        .py_1()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.history_close_surface(window, cx);
                        }))
                        .child("閉じる"),
                )
                .into_any_element(),
        );
        children.push(
            div()
                .id("history-status")
                .role(Role::Label)
                .aria_label(SharedString::from(presentation.status_label.clone()))
                .aria_value(SharedString::from(presentation.status_label.clone()))
                .text_color(
                    if matches!(
                        view.state,
                        HistoryLoadState::Error(_) | HistoryLoadState::Unauthorized
                    ) {
                        danger
                    } else {
                        muted
                    },
                )
                .child(SharedString::from(presentation.status_label.clone()))
                .into_any_element(),
        );

        let rows = view.rows.clone();
        let active = self.history_active;
        let mut row_elements: Vec<AnyElement> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            row_elements.push(history_row_element(
                index,
                row,
                active == Some(index),
                list_focused,
                colors,
                text_color,
                muted,
                cx,
            ));
        }
        let placeholder = presentation.empty_placeholder.clone();
        let list = div()
            .id("history-list")
            .role(Role::List)
            .aria_label(SharedString::from(format!(
                "実行履歴。{}件。上下キーで移動、Enterで開きます。",
                rows.len()
            )))
            .key_context("HistoryList")
            .track_focus(&self.history_list_focus)
            .track_scroll(&self.history_scroll)
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .max_h(px(360.0))
            .overflow_y_scroll()
            .on_action(cx.listener(|this, _: &HistoryRowUp, window, cx| {
                this.history_move_active(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryRowDown, window, cx| {
                this.history_move_active(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryRowHome, window, cx| {
                this.history_move_edge(RailEdge::First, window, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryRowEnd, window, cx| {
                this.history_move_edge(RailEdge::Last, window, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryReopen, window, cx| {
                this.history_reopen_active(window, cx);
            }))
            .children(if row_elements.is_empty() {
                vec![
                    div()
                        .text_color(muted)
                        .child(SharedString::from(
                            placeholder.unwrap_or_else(|| "履歴はありません".to_owned()),
                        ))
                        .into_any_element(),
                ]
            } else {
                row_elements
            });
        children.push(list.into_any_element());

        // Bounded load-older: available only with a real server cursor and only
        // while the entry ceiling has not been reached. Otherwise the exact
        // reason is stated so unloaded older pages never look like an end.
        match &presentation.older {
            OlderAction::Action(label) => {
                children.push(
                    div()
                        .id("history-older")
                        .role(Role::Button)
                        .aria_label(SharedString::from(label.clone()))
                        .key_context("HistoryOlder")
                        .track_focus(&self.history_older_focus)
                        .px_3()
                        .py_1()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .on_action(cx.listener(|this, _: &HistoryOlder, _, cx| {
                            this.history_load_older(cx);
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history_load_older(cx);
                        }))
                        .child(SharedString::from(label.clone()))
                        .into_any_element(),
                );
            }
            OlderAction::Reason(label) => {
                children.push(
                    div()
                        .id("history-older-state")
                        .role(Role::Label)
                        .aria_label(SharedString::from(label.clone()))
                        .aria_value(SharedString::from(label.clone()))
                        .text_color(muted)
                        .child(SharedString::from(label.clone()))
                        .into_any_element(),
                );
            }
        }
        if presentation.reload {
            children.push(
                div()
                    .id("history-reload")
                    .role(Role::Button)
                    .aria_label("履歴を再読み込み")
                    .key_context("HistoryReload")
                    .track_focus(&self.history_reload_focus)
                    .px_3()
                    .py_1()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .on_action(cx.listener(|this, _: &HistoryReload, _, cx| {
                        this.history_reload(cx);
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.history_reload(cx)))
                    .child("再読み込み")
                    .into_any_element(),
            );
        }
        if let Some(notice) = notice {
            children.push(
                div()
                    .text_color(muted)
                    .child(SharedString::from(notice))
                    .into_any_element(),
            );
        }

        div()
            .id("history-surface")
            .role(Role::Dialog)
            .aria_label("実行履歴")
            .key_context("History")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt_8()
            .bg(rgb(colors.background))
            .track_focus(&self.history_focus)
            .on_action(cx.listener(|this, _: &HistoryClose, window, cx| {
                this.history_close_surface(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryOlder, _, cx| {
                this.history_load_older(cx);
            }))
            .child(
                div()
                    .w(px(720.))
                    .max_h(px(720.))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .bg(rgb(surface))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .children(children),
            )
            .into_any_element()
    }
}

/// One timeline row: authoritative run-level fields only.
#[allow(clippy::too_many_arguments)]
fn history_row_element(
    index: usize,
    row: &ui_model::history::HistoryRow,
    active: bool,
    list_focused: bool,
    colors: crate::contrast::ShellColors,
    text_color: gpui::Rgba,
    muted: gpui::Rgba,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let element_id = SharedString::from(format!("history-row-{}", row.record_ref));
    div()
        .id(element_id.clone())
        .accessibility_id(element_id)
        .role(Role::ListItem)
        .aria_label(SharedString::from(row.accessible_label.clone()))
        .aria_description(SharedString::from(row.reopen_label.clone()))
        .when(active, |element| element.aria_selected(true))
        .when(active && list_focused, |element| {
            element.aria_active_descendant()
        })
        .flex()
        .flex_col()
        .gap_0()
        .p_1()
        .border_1()
        .border_color(rgb(if active { colors.focus } else { colors.border }))
        .bg(rgb(if active {
            colors.selection
        } else {
            colors.button_face
        }))
        .text_color(if active {
            rgb(colors.selection_text)
        } else {
            text_color
        })
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| {
            this.history_reopen_row(index, window, cx);
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .child(SharedString::from(format!(
                    "反復 {} · {}",
                    row.iteration_label, row.phase_label
                )))
                .child(SharedString::from(row.created_label.clone())),
        )
        .child(div().text_color(muted).child(SharedString::from(format!(
            "実行 {} · 計画 {} · 記録 {}",
            row.run_label, row.plan_label, row.record_ref
        ))))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{OlderAction, presentation_for, unavailable_view};
    use std::sync::Arc;
    use ui_model::history::{HistoryLoadState, HistoryRow, HistoryView};

    fn row(label: &str) -> HistoryRow {
        HistoryRow {
            index: 0,
            iteration_label: Arc::from("3"),
            phase_label: Arc::from("REFINE"),
            run_label: Arc::from("run/3"),
            plan_label: Arc::from("不明"),
            created_label: Arc::from("2026-11-08T00:00:00Z"),
            record_ref: Arc::from("record/3"),
            accessible_label: Arc::from(label),
            reopen_label: Arc::from("この時点を開く（反復 3）"),
            reopen_iteration: Some(3),
        }
    }

    fn view(
        state: HistoryLoadState,
        status: &str,
        rows: Vec<HistoryRow>,
        can_load_older: bool,
        older_label: &str,
    ) -> HistoryView {
        HistoryView {
            task: Some(Arc::from("task/Task-184")),
            state,
            status_label: Arc::from(status),
            rows,
            can_load_older,
            load_older_label: Arc::from(older_label),
            at_bound: false,
        }
    }

    #[test]
    fn an_unloaded_history_never_renders_as_an_empty_absence() {
        let presentation = presentation_for(
            Some("task/Task-184"),
            &view(
                HistoryLoadState::Idle,
                "履歴はまだ読み込まれていません",
                Vec::new(),
                false,
                "履歴はまだ読み込まれていません",
            ),
            true,
        );
        assert_eq!(
            presentation.status_label, "履歴はまだ読み込まれていません",
            "the unloaded state has its own label"
        );
        assert_ne!(
            presentation.status_label,
            "このタスクには実行履歴がありません"
        );
        assert_eq!(
            presentation.empty_placeholder.as_deref(),
            Some("履歴はまだ読み込まれていません"),
            "the placeholder repeats the unloaded state, not an empty claim"
        );
        assert!(presentation.reload, "an unloaded view can be loaded");
        assert_eq!(
            presentation.older,
            OlderAction::Reason("履歴はまだ読み込まれていません".to_owned())
        );
        assert_eq!(presentation.title, "履歴 — task/Task-184");
    }

    #[test]
    fn an_authoritative_empty_view_has_its_own_label_and_no_reload() {
        let presentation = presentation_for(
            Some("task/Task-184"),
            &view(
                HistoryLoadState::Empty,
                "このタスクには実行履歴がありません",
                Vec::new(),
                false,
                "これより古い履歴はありません",
            ),
            true,
        );
        assert_eq!(
            presentation.empty_placeholder.as_deref(),
            Some("このタスクには実行履歴がありません")
        );
        assert!(!presentation.reload);
        assert_eq!(
            presentation.older,
            OlderAction::Reason("これより古い履歴はありません".to_owned())
        );
    }

    #[test]
    fn a_loaded_view_with_a_cursor_offers_the_bounded_action() {
        let presentation = presentation_for(
            Some("task/Task-184"),
            &view(
                HistoryLoadState::Loaded,
                "履歴を表示しています（さらに古い履歴があります）",
                vec![row("反復 3。フェーズ REFINE。")],
                true,
                "さらに古い履歴を読み込む",
            ),
            true,
        );
        assert_eq!(
            presentation.older,
            OlderAction::Action("さらに古い履歴を読み込む".to_owned())
        );
        assert_eq!(presentation.row_labels, vec!["反復 3。フェーズ REFINE。"]);
        assert_eq!(presentation.empty_placeholder, None);
        assert!(!presentation.reload);
    }

    #[test]
    fn the_entry_bound_and_the_end_state_are_distinct_reasons() {
        let bound = presentation_for(
            Some("task/Task-184"),
            &view(
                HistoryLoadState::Loaded,
                "履歴を表示しています",
                vec![row("反復 3。")],
                false,
                "表示上限（200件）に達しました。これより古い履歴は未読み込みです",
            ),
            true,
        );
        match bound.older {
            OlderAction::Reason(reason) => {
                assert!(reason.contains("表示上限"));
                assert!(reason.contains("未読み込み"));
            }
            OlderAction::Action(_) => panic!("the bound must not offer the action"),
        }
        let end = presentation_for(
            Some("task/Task-184"),
            &view(
                HistoryLoadState::End,
                "最新の履歴をすべて表示しています",
                vec![row("反復 3。")],
                false,
                "これより古い履歴はありません",
            ),
            true,
        );
        assert_eq!(
            end.older,
            OlderAction::Reason("これより古い履歴はありません".to_owned())
        );
    }

    #[test]
    fn a_failed_view_offers_a_reload_and_an_unauthorized_one_stays_explicit() {
        let failed = presentation_for(
            Some("task/Task-184"),
            &view(
                HistoryLoadState::Error(app_core::HistoryError::Conflict),
                "観測を開き直す必要があります",
                Vec::new(),
                false,
                "これより古い履歴はありません",
            ),
            true,
        );
        assert!(failed.reload);
        assert_eq!(failed.status_label, "観測を開き直す必要があります");

        let unauthorized = presentation_for(
            Some("task/Task-184"),
            &view(
                HistoryLoadState::Unauthorized,
                "履歴を表示する許可がありません",
                Vec::new(),
                false,
                "これより古い履歴はありません",
            ),
            true,
        );
        assert!(unauthorized.reload);
        assert_eq!(unauthorized.status_label, "履歴を表示する許可がありません");
    }

    #[test]
    fn no_owner_is_explicit_and_not_reloadable() {
        let unavailable = unavailable_view();
        let presentation = presentation_for(None, &unavailable, false);
        assert_eq!(presentation.status_label, "観測が構成されていません");
        assert_eq!(presentation.title, "履歴 — タスク未選択");
        assert!(!presentation.reload, "no owner cannot reload");
        assert_eq!(
            presentation.older,
            OlderAction::Reason("観測が構成されていません".to_owned())
        );
    }

    #[test]
    fn the_surface_is_reachable_from_the_task_context_and_the_keyboard() {
        // The inspector action opens the same surface the global shortcut opens.
        let inspector = include_str!("inspector.rs");
        assert!(
            inspector.contains("inspector-history-open"),
            "the task context exposes the History entry"
        );
        assert!(
            inspector.contains("history_open_surface"),
            "the inspector action calls the same open path"
        );
        let shell = include_str!("lib.rs");
        assert!(
            shell.contains("KeyBinding::new(\"ctrl-h\", HistoryOpen, None)"),
            "the surface has a global keyboard entry"
        );
        for binding in [
            "KeyBinding::new(\"up\", HistoryRowUp, Some(\"HistoryList\"))",
            "KeyBinding::new(\"enter\", HistoryReopen, Some(\"HistoryList\"))",
            "KeyBinding::new(\"escape\", HistoryClose, Some(\"History\"))",
        ] {
            assert!(shell.contains(binding), "missing binding: {binding}");
        }
        assert!(
            shell.contains("!HistoryList && !HistoryOlder"),
            "Enter on the history list must not also activate the underlying surface"
        );
    }
}
