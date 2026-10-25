//! Intervention strip surface (DC-MP-INS3 / ADR-0011).
//!
//! In-app only. Every row is a typed Desktop-local condition; nothing here sends a
//! canonical command, calls an OS notification API, or starts an action on render.
//! Actions are read-only navigation, the existing read-only receipt presentation, or
//! a local re-read of the current authorized observation.

use gpui::{AnyElement, Context, IntoElement, Role, SharedString, Window, div, prelude::*, rgb};
use ui_model::intervention::{Action, Reference, Row, RowIdentity, Strip};

use crate::shell_style::TextSpec;
use crate::tokens::TextStyle;
use app_core::navigation::Surface;

use crate::{InterventionActivate, InterventionDismiss, Shell};

impl Shell {
    /// Build the in-app intervention strip for the current authorized observation.
    ///
    /// An empty strip renders nothing; rendering never triggers an action.
    pub(super) fn intervention_strip(
        &mut self,
        strip: Strip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(top) = strip.top.clone() else {
            return div().into_any_element();
        };
        let xs_text = TextSpec::ui(&self.tokens, TextStyle::Xs);
        let mut panel = div()
            .id("intervention-strip")
            .role(Role::Region)
            .aria_label("Intervention strip — Desktop-local conditions only")
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(rgb(0x626870))
            .bg(rgb(0x2a2f36));
        panel = panel.child(self.strip_row(&top, 0, &xs_text, window, cx));
        if strip.remaining > 0 {
            let label = if self.strip_expanded {
                format!("Show fewer ({} conditions)", strip.rows.len())
            } else {
                format!("{} more", strip.remaining)
            };
            let focused = self.strip_list_focus.is_focused(window);
            let expanded = self.strip_expanded;
            panel = panel.child(
                div()
                    .id("intervention-more")
                    .role(Role::Button)
                    .aria_label(SharedString::from(label.clone()))
                    .aria_expanded(expanded)
                    .aria_keyshortcuts("Enter Space")
                    .track_focus(&self.strip_list_focus)
                    .key_context("InterventionStrip")
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(rgb(if focused { 0xe6c278 } else { 0x343940 }))
                    .cursor_pointer()
                    .child(SharedString::from(label))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.strip_list_focus.focus(window, cx);
                        this.toggle_strip_list(cx);
                    }))
                    .on_action(cx.listener(|this, _: &InterventionActivate, window, cx| {
                        this.strip_list_focus.focus(window, cx);
                        this.toggle_strip_list(cx);
                    })),
            );
            if self.strip_expanded {
                for (offset, row) in strip.rows.iter().skip(1).enumerate() {
                    panel = panel.child(self.strip_row(row, offset + 1, &xs_text, window, cx));
                }
            }
        }
        if let Some(identity) = self.strip_detail.clone()
            && let Some(row) = strip.rows.iter().find(|row| row.identity == identity)
        {
            panel = panel.child(self.strip_details(row, &xs_text, window, cx));
        }
        panel.into_any_element()
    }

    fn strip_row(
        &mut self,
        row: &Row,
        index: usize,
        xs_text: &TextSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let handle = self
            .strip_focus
            .entry(row.identity.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let focused = handle.is_focused(window);
        let label = row.display_label();
        let action = row.action.map_or("No action", Action::label);
        let identity = row.identity.clone();
        let click_identity = identity.clone();
        let action_identity = identity;
        div()
            .id(("intervention-row", index))
            .role(Role::Button)
            .aria_label(SharedString::from(format!("{label} — {action}")))
            .aria_description(SharedString::from(row.detail.to_string()))
            .aria_keyshortcuts("Enter Space")
            .track_focus(&handle)
            .key_context("InterventionStrip")
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(if focused { 0xe6c278 } else { 0x343940 }))
            .cursor_pointer()
            .child(SharedString::from(format!("{label} — {action}")))
            .child(
                xs_text
                    .clone()
                    .apply(div())
                    .text_color(rgb(0x9da3aa))
                    .child(SharedString::from(row.detail.to_string())),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(handle) = this.strip_focus.get(&click_identity).cloned() {
                    handle.focus(window, cx);
                }
                this.activate_strip_row(click_identity.clone(), window, cx);
            }))
            .on_action(
                cx.listener(move |this, _: &InterventionActivate, window, cx| {
                    this.activate_strip_row(action_identity.clone(), window, cx);
                }),
            )
            .into_any_element()
    }

    fn strip_details(
        &mut self,
        row: &Row,
        xs_text: &TextSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.strip_detail_focus.is_focused(window);
        let mut panel = div()
            .id("intervention-details")
            .role(Role::Group)
            .aria_label("Intervention details — read-only")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(rgb(0x626870))
            .bg(rgb(0x202328))
            .child(SharedString::from(row.display_label()))
            .child(
                xs_text
                    .clone()
                    .apply(div())
                    .child(SharedString::from(row.detail.to_string())),
            )
            .child(
                xs_text
                    .clone()
                    .apply(div())
                    .text_color(rgb(0x9da3aa))
                    .child("Read-only — no command is sent."),
            );
        panel = panel.child(
            div()
                .id("intervention-details-close")
                .role(Role::Button)
                .aria_label("Close intervention details")
                .aria_keyshortcuts("Enter Space Escape")
                .track_focus(&self.strip_detail_focus)
                .key_context("InterventionStrip")
                .px_2()
                .py_1()
                .border_1()
                .border_color(rgb(if focused { 0xe6c278 } else { 0x343940 }))
                .cursor_pointer()
                .child("Close")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_strip_details(window, cx);
                }))
                .on_action(cx.listener(|this, _: &InterventionActivate, window, cx| {
                    this.close_strip_details(window, cx);
                }))
                .on_action(cx.listener(|this, _: &InterventionDismiss, window, cx| {
                    this.close_strip_details(window, cx);
                })),
        );
        panel.into_any_element()
    }

    /// Activate one row's read-only action. Only explicit activation reaches this.
    pub(super) fn activate_strip_row(
        &mut self,
        identity: RowIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = self
            .cache
            .text()
            .interventions
            .rows()
            .iter()
            .find(|row| row.identity == identity)
            .cloned();
        let Some(row) = row else {
            return;
        };
        match row.action {
            Some(Action::Inspect) => self.inspect_intervention(&row, window, cx),
            Some(Action::Details) => {
                self.strip_detail = Some(row.identity.clone());
                self.strip_detail_origin = Some(row.identity);
                self.strip_detail_focus.focus(window, cx);
                cx.notify();
            }
            Some(Action::Reconnect) => self.reconnect_observation(cx),
            None => {}
        }
    }

    fn inspect_intervention(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let section = match &row.identity {
            RowIdentity::MissingReference {
                reference: Reference::Evaluation(_),
                ..
            } => 1,
            RowIdentity::MissingReference {
                reference: Reference::Outcome(_),
                ..
            } => 4,
            _ => 0,
        };
        self.navigate(Surface::Artifact, cx);
        if let Some(handle) = self.inspector_focus.get(section) {
            handle.focus(window, cx);
        }
    }

    /// Local re-read of the current authorized observation; no canonical mutation.
    pub(super) fn reconnect_observation(&mut self, cx: &mut Context<Self>) {
        let live = self
            .handle
            .as_ref()
            .and_then(|handle| handle.current().ok())
            .and_then(|lease| lease.notice());
        let available = live.is_some();
        self.cache.revalidate(live);
        if !available {
            self.surface_notice = Some(
                "Reconnect requested — no authorized observation is available yet.".to_owned(),
            );
        }
        cx.notify();
    }

    /// Toggle the expanded list of the remaining conditions.
    pub(super) fn toggle_strip_list(&mut self, cx: &mut Context<Self>) {
        self.strip_expanded = !self.strip_expanded;
        cx.notify();
    }

    /// Close the read-only detail panel and return focus to the originating row.
    pub(super) fn close_strip_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.strip_detail = None;
        let origin = self.strip_detail_origin.take();
        if let Some(identity) = origin
            && self
                .cache
                .text()
                .interventions
                .rows()
                .iter()
                .any(|row| row.identity == identity)
            && let Some(handle) = self.strip_focus.get(&identity).cloned()
        {
            handle.focus(window, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    /// The strip must use the existing focus/action boundary and must never send a
    /// command or call an OS notification API (ADR-0011 §4 exclusions).
    #[test]
    fn the_strip_uses_the_existing_focus_boundary_and_sends_no_command() {
        let source = include_str!("intervention.rs");
        // Scan production code only: this test's own forbidden-string list would
        // otherwise match itself.
        let production = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
        for needle in [
            "track_focus",
            "key_context(\"InterventionStrip\")",
            "aria_label",
            "aria_description",
            "Role::Button",
            "on_click",
            "on_action",
        ] {
            assert!(production.contains(needle), "strip must use {needle}");
        }
        for forbidden in [
            "request_stop",
            "CommandPayload",
            "os_notification",
            "notify_user",
            "send_command",
        ] {
            assert!(
                !production.contains(forbidden),
                "strip must not reach {forbidden}"
            );
        }
        let shell = include_str!("lib.rs");
        assert!(shell.contains(
            "KeyBinding::new(\"enter\", InterventionActivate, Some(\"InterventionStrip\"))"
        ));
        assert!(shell.contains(
            "KeyBinding::new(\"space\", InterventionActivate, Some(\"InterventionStrip\"))"
        ));
    }

    /// Dismissal returns focus to the originating row (ADR-0011 §2).
    #[test]
    fn closing_the_details_returns_focus_to_the_originating_row() {
        let source = include_str!("intervention.rs");
        let close = source
            .find("fn close_strip_details")
            .expect("close_strip_details exists");
        let close = &source[close..(close + 700).min(source.len())];
        assert!(close.contains("strip_detail_origin"));
        assert!(close.contains(".focus(window, cx)"));
    }
}
