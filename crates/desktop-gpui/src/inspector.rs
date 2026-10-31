//! Inspector panel (DC-MP-INS1 / MP-INS1).
//!
//! Progressive disclosure over `ui_model::inspector::View`. Every value is shown as
//! the Runtime projects it: a missing value keeps its explicit absent state, and no
//! score, best candidate, outcome or evidence body is inferred, computed or fetched.
//! Disclosure state is derived-only local state keyed by section revision.

use std::sync::Arc;

use desktop_protocol::TaskId;
use gpui::{AnyElement, Context, IntoElement, Role, SharedString, Window, div, prelude::*, rgb};
use ui_model::inspector::{Section, View};

use crate::shell_style::TextSpec;
use crate::tokens::TextStyle;
use crate::{InspectorToggle, Shell};

/// The Task and withholding decision for one inspector render.
pub(super) struct PanelScope<'a> {
    pub task: Option<&'a TaskId>,
    pub withheld: Option<&'static str>,
}

/// Why the inspector must withhold the current projection, if it must.
///
/// The cached display is authorized for the last installed publication. While a
/// Task switch is being derived, or after the selected identity changed, that
/// cache can still describe the previous Task, so neither its values nor its
/// disclosure state may be presented as the new Task's.
pub(super) fn scope(
    authorized: bool,
    selection_pending: bool,
    cached_task: Option<&TaskId>,
    selected_task: Option<&TaskId>,
) -> Option<&'static str> {
    if !authorized {
        return Some("利用不可 — 現在閲覧できる情報がありません");
    }
    if selection_pending {
        return Some("保留 — 選択中のタスクの情報を準備しています");
    }
    // With no explicit local selection the installed publication's task is the
    // authoritative target; with one, the cache must already describe it.
    if let Some(selected) = selected_task
        && cached_task != Some(selected)
    {
        return Some("保留 — 別のタスクの情報は表示しません");
    }
    None
}

impl Shell {
    /// Build the inspector for the selected task and viewed candidate.
    ///
    /// Sections are collapsed by default and expose their available/absent state in
    /// the header, so the collapsed inspector never implies a missing value.
    pub(super) fn inspector_panel(
        &mut self,
        view: View,
        viewed_label: SharedString,
        viewed_available: bool,
        scope: PanelScope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body_text = TextSpec::ui(&self.tokens, TextStyle::Body);
        let xs_text = TextSpec::ui(&self.tokens, TextStyle::Xs);
        let mut panel = div()
            .id("inspector-panel")
            .role(Role::Group)
            .aria_label("詳細と説明")
            .flex()
            .flex_col()
            .gap_2();
        if let Some(reason) = scope.withheld {
            // A blocked or out-of-scope projection invalidates disclosure state
            // instead of merely skipping the sync, so the next in-scope Task or
            // re-observation starts collapsed (A1).
            self.inspector_disclosure.invalidate();
            let message = SharedString::from(reason);
            return panel
                .child(
                    div()
                        .id("inspector-unavailable")
                        .role(Role::Label)
                        .aria_label(message.clone())
                        .aria_value(message.clone())
                        .child(message),
                )
                .into_any_element();
        }
        // The Task binds first: a switch that completed without an intermediate
        // withheld frame must not carry the previous Task's open set into a
        // same-revision section. Identity + availability then clear on a change or
        // disappearance, so a returning identity starts collapsed.
        self.inspector_disclosure.sync_task(scope.task);
        self.inspector_disclosure
            .sync(view.candidate.as_ref(), viewed_available);
        let current: Vec<(Section, Arc<str>)> = view
            .sections
            .iter()
            .map(|section| (section.id, Arc::clone(&section.revision)))
            .collect();
        self.inspector_disclosure.prune(&current);

        let candidate_state: SharedString = if view.candidate.is_none() {
            "候補は表示されていません".into()
        } else if viewed_available {
            SharedString::from(format!(
                "閲覧中の候補（この画面での選択）: {viewed_label} — 正式採用ではありません。正式採用は実行基盤から通知されます。"
            ))
        } else {
            SharedString::from(format!(
                "閲覧中の候補（この画面での選択）: {viewed_label} — 現在の認可済み表示情報では利用できません"
            ))
        };
        panel = panel.child(
            div()
                .id("inspector-viewed")
                .role(Role::Label)
                .aria_label(candidate_state.clone())
                .aria_value(candidate_state.clone())
                .child(candidate_state),
        );

        for (index, section) in view.sections.iter().enumerate() {
            let Some(handle) = self.inspector_focus.get(index) else {
                continue;
            };
            let open = self
                .inspector_disclosure
                .is_open(section.id, &section.revision);
            let focused = handle.is_focused(window);
            let state_label = if section.available {
                format!("利用可能: {}", section.summary)
            } else {
                format!(
                    "利用不可: {}",
                    section
                        .absent_reason
                        .as_deref()
                        .unwrap_or("表示情報がありません")
                )
            };
            let marker = if open { "▾" } else { "▸" };
            let section_label = section.id.label();
            let header_text = format!("{marker} {section_label} — {state_label}");
            let revision = Arc::clone(&section.revision);
            let click_revision = Arc::clone(&revision);
            let action_revision = revision;
            let header = div().id(("inspector-section", index)).role(Role::Button);
            let header = header
                .aria_label(SharedString::from(format!(
                    "{section_label} — {state_label}",
                )))
                .aria_value(SharedString::from(section.summary.clone()))
                .aria_expanded(open)
                .track_focus(&self.inspector_focus[index])
                .tooltip({
                    let style = xs_text.clone();
                    let width = crate::shell_style::inspector_width(&self.tokens);
                    move |_, cx| {
                        crate::help::tooltip(
                            SharedString::from(format!(
                                "{section_label}の情報と説明を表示します。"
                            )),
                            style.clone(),
                            width,
                            cx,
                        )
                    }
                })
                .key_context("InspectorSection")
                .px_3()
                .py_2()
                .border_1()
                .border_color(rgb(if focused { 0xe6c278 } else { 0x343940 }))
                .cursor_pointer()
                .child(
                    xs_text
                        .clone()
                        .apply(div())
                        .child(SharedString::from(header_text)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.inspector_focus[index].focus(window, cx);
                    this.toggle_inspector(index, Arc::clone(&click_revision), cx);
                }))
                .on_action(cx.listener(move |this, _: &InspectorToggle, _, cx| {
                    this.toggle_inspector(index, Arc::clone(&action_revision), cx);
                }));
            panel = panel.child(header);
            if !open {
                continue;
            }
            let mut details = div()
                .id(("inspector-details", index))
                .role(Role::Group)
                .aria_label(SharedString::from(format!("{}の詳細", section_label)))
                .flex()
                .flex_col()
                .gap_1()
                .pl_3();
            if !section.available {
                details = details.child(
                    xs_text
                        .clone()
                        .apply(div())
                        .text_color(rgb(0xd08a6a))
                        .child(SharedString::from(
                            section
                                .absent_reason
                                .clone()
                                .unwrap_or_else(|| Arc::from("表示情報がありません。")),
                        )),
                );
            }
            for entry in section.entries.iter() {
                if let Some(heading) = &entry.heading {
                    details = details.child(
                        body_text
                            .clone()
                            .apply(div())
                            .child(SharedString::from(Arc::clone(heading))),
                    );
                }
                for field in entry.fields.iter() {
                    details = details.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                xs_text
                                    .clone()
                                    .apply(div())
                                    .text_color(rgb(0x8b949e))
                                    .child(SharedString::from(Arc::clone(&field.label))),
                            )
                            .child(
                                xs_text
                                    .clone()
                                    .apply(div())
                                    .flex_1()
                                    .min_w_0()
                                    .child(SharedString::from(Arc::clone(&field.value))),
                            ),
                    );
                }
            }
            panel = panel.child(details);
        }
        panel.into_any_element()
    }

    /// Toggle one inspector section at its current revision.
    pub(super) fn toggle_inspector(
        &mut self,
        index: usize,
        revision: Arc<str>,
        cx: &mut Context<Self>,
    ) {
        let Some(section) = Section::ALL.get(index).copied() else {
            return;
        };
        self.inspector_disclosure.toggle(section, &revision);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::scope;
    use desktop_protocol::TaskId;

    fn task(name: &str) -> TaskId {
        TaskId::fixture(format!("task/{name}"))
    }

    #[test]
    fn the_scope_gate_withholds_a_pending_or_foreign_task_projection() {
        let first = task("first");
        let second = task("second");
        assert_eq!(scope(true, false, Some(&first), Some(&first)), None);
        assert_eq!(scope(true, false, None, None), None);
        assert!(scope(true, true, Some(&first), Some(&second)).is_some());
        assert!(scope(true, false, Some(&first), Some(&second)).is_some());
        assert_eq!(scope(true, false, Some(&first), None), None);
        assert!(scope(true, false, None, Some(&first)).is_some());
        assert!(scope(false, true, Some(&first), Some(&second)).is_some());
    }
}
