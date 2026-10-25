//! Inspector panel (DC-MP-INS1 / MP-INS1).
//!
//! Progressive disclosure over `ui_model::inspector::View`. Every value is shown as
//! the Runtime projects it: a missing value keeps its explicit absent state, and no
//! score, best candidate, outcome or evidence body is inferred, computed or fetched.
//! Disclosure state is derived-only local state keyed by section revision.

use std::sync::Arc;

use gpui::{AnyElement, Context, IntoElement, Role, SharedString, Window, div, prelude::*, rgb};
use ui_model::inspector::{Section, View};

use crate::shell_style::TextSpec;
use crate::tokens::TextStyle;
use crate::{InspectorToggle, Shell};

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
        authorized: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body_text = TextSpec::ui(&self.tokens, TextStyle::Body);
        let xs_text = TextSpec::ui(&self.tokens, TextStyle::Xs);
        let mut panel = div()
            .id("inspector-panel")
            .role(Role::Group)
            .aria_label("Inspector sections")
            .flex()
            .flex_col()
            .gap_2();
        if !authorized {
            // A blocked observation invalidates disclosure state instead of merely
            // skipping the sync, so re-observation starts collapsed (A1).
            self.inspector_disclosure.invalidate();
            return panel
                .child(
                    div()
                        .id("inspector-unavailable")
                        .role(Role::Label)
                        .aria_label("Inspector unavailable — no current authorized observation")
                        .child("Unavailable — no current authorized observation"),
                )
                .into_any_element();
        }
        // Identity + availability drive the disclosure scope: an identity change or a
        // disappearance clears it, so a returning identity starts collapsed.
        self.inspector_disclosure
            .sync(view.candidate.as_ref(), viewed_available);
        let current: Vec<(Section, Arc<str>)> = view
            .sections
            .iter()
            .map(|section| (section.id, Arc::clone(&section.revision)))
            .collect();
        self.inspector_disclosure.prune(&current);

        let candidate_state: SharedString = if view.candidate.is_none() {
            "No candidate viewed".into()
        } else if viewed_available {
            SharedString::from(format!("Viewed candidate: {viewed_label}"))
        } else {
            SharedString::from(format!(
                "Viewed candidate: {viewed_label} — unavailable in the current authorized projection"
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
                format!("available: {}", section.summary)
            } else {
                format!(
                    "unavailable: {}",
                    section
                        .absent_reason
                        .as_deref()
                        .unwrap_or("no projection is present")
                )
            };
            let marker = if open { "▾" } else { "▸" };
            let header_text = format!("{marker} {} — {state_label}", section.id.label());
            let revision = Arc::clone(&section.revision);
            let click_revision = Arc::clone(&revision);
            let action_revision = revision;
            let header = div()
                .id(("inspector-section", index))
                .role(Role::Button)
                .aria_label(SharedString::from(format!(
                    "{} — {state_label}",
                    section.id.label()
                )))
                .aria_value(SharedString::from(section.summary.clone()))
                .aria_expanded(open)
                .track_focus(&self.inspector_focus[index])
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
                .aria_label(SharedString::from(format!(
                    "{} details",
                    section.id.label()
                )))
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
                                .unwrap_or_else(|| Arc::from("No projection is present.")),
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
    /// A blocked observation must invalidate disclosure before the early return, so a
    /// later re-observation starts collapsed (A1).
    #[test]
    fn a_blocked_observation_invalidates_disclosure_before_the_early_return() {
        let panel = include_str!("inspector.rs");
        let unauthorized = panel
            .find("if !authorized")
            .expect("unauthorized branch exists");
        let rest = &panel[unauthorized..];
        let invalidate = rest
            .find("invalidate()")
            .expect("disclosure invalidation is called");
        let early_return = rest.find("return panel").expect("early return exists");
        assert!(
            invalidate < early_return,
            "invalidation must run before the unauthorized early return"
        );
    }

    /// The inspector must use the existing GPUI focus/action/accessibility boundary:
    /// no new mechanism is introduced for keyboard reachability (DC-MP-INS1).
    #[test]
    fn the_inspector_uses_the_existing_focus_and_action_boundary() {
        let panel = include_str!("inspector.rs");
        for needle in [
            "track_focus",
            "key_context(\"InspectorSection\")",
            "aria_expanded",
            "aria_label",
            "Role::Button",
            "on_click",
            "on_action",
        ] {
            assert!(panel.contains(needle), "inspector panel must use {needle}");
        }
        let shell = include_str!("lib.rs");
        for needle in [
            "KeyBinding::new(\"enter\", InspectorToggle, Some(\"InspectorSection\"))",
            "KeyBinding::new(\"space\", InspectorToggle, Some(\"InspectorSection\"))",
            "cx.listener(|_, _: &FocusNext, window, cx| window.focus_next(cx))",
            "cx.listener(|_, _: &FocusPrevious, window, cx| window.focus_prev(cx))",
        ] {
            assert!(shell.contains(needle), "shell must keep {needle}");
        }
    }
}
