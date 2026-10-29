//! Command palette surface (DC-MP-NAV5).
//!
//! The palette lists the closed [`ui_model::palette`] inventory, matches it with
//! the frozen rank order, and dispatches only through the same window/app-core
//! paths the owning surface already uses. Availability is derived from the current
//! authorized publication and the same Stop/receipt predicates as the controls;
//! it is never invented here and never bypasses a surface rule.

use crate::{Shell, accessibility, dock};
use gpui::{
    AnyElement, Context, Element, ElementId, ElementInputHandler, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Role, SharedString, Style, Window, div, prelude::*,
    px, relative, rgb,
};
use std::sync::Arc;
use ui_model::WorkbenchText;
use ui_model::palette::{self, Command, Entry, Outcome};

/// One action's availability with the honest reason it cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActionFacts {
    pub enabled: bool,
    pub reason: Arc<str>,
}

/// Availability inputs the palette derives from the current authorized state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Facts {
    pub authorized: bool,
    pub stop: ActionFacts,
    pub reconcile: ActionFacts,
    pub compare_reason: Option<Arc<str>>,
    /// `None` when the rail could activate its active row; the honest reason otherwise.
    pub active_task_reason: Option<&'static str>,
    pub active_candidate: bool,
    pub evaluation_available: bool,
    pub evidence_available: bool,
}

const STALE: &str = "No current authorized observation is available for this command.";

impl Facts {
    fn unavailable(&self, command: Command) -> Option<&str> {
        match command {
            Command::ShowArtifact
            | Command::ShowFlow
            | Command::NextSurface
            | Command::ToggleDiagnostics => None,
            Command::ShowCompare => {
                if self.authorized {
                    self.compare_reason.as_deref()
                } else {
                    Some(STALE)
                }
            }
            Command::OpenActiveTask => require(
                self.authorized,
                self.active_task_reason.is_none(),
                self.active_task_reason.unwrap_or(""),
            ),
            Command::OpenActiveCandidate => require(
                self.authorized,
                self.active_candidate,
                "No candidate is active in the dock.",
            ),
            Command::ShowEvaluation => require(
                self.authorized,
                self.evaluation_available,
                "Evaluation is not available for the viewed candidate.",
            ),
            Command::ShowEvidence => require(
                self.authorized,
                self.evidence_available,
                "Evidence is not available for the viewed candidate.",
            ),
            Command::StopTask => require(self.authorized, self.stop.enabled, &self.stop.reason),
            Command::CheckStopReceipt => require(
                self.authorized,
                self.reconcile.enabled,
                &self.reconcile.reason,
            ),
        }
    }
}

fn require(authorized: bool, available: bool, reason: &str) -> Option<&str> {
    if !authorized {
        Some(STALE)
    } else if available {
        None
    } else {
        Some(reason)
    }
}

/// One row per inventory entry, in inventory order.
pub(super) fn entries(facts: &Facts) -> Vec<Entry<'_>> {
    palette::INVENTORY
        .iter()
        .map(|descriptor| Entry {
            descriptor,
            unavailable: facts.unavailable(descriptor.command),
        })
        .collect()
}

/// Binds GPUI's focused text input to the palette's query field.
struct PaletteInputRegistration {
    entity: gpui::Entity<Shell>,
}

impl IntoElement for PaletteInputRegistration {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PaletteInputRegistration {
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
        cx: &mut gpui::App,
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
        _bounds: gpui::Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut gpui::App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        let focus = self.entity.read(cx).palette_focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
    }
}

impl Shell {
    /// Derive the palette's availability from the current authorized state.
    pub(super) fn palette_facts(&self, authorized_text: bool, text: &WorkbenchText) -> Facts {
        let (stop, reconcile) = self.action_availability(authorized_text, text);
        let viewed = self
            .workbench
            .as_ref()
            .and_then(|workbench| workbench.presentation.viewed());
        let view = text.inspector.view(viewed.map(|viewed| &viewed.0));
        let section_available = |section: ui_model::inspector::Section| {
            view.sections
                .iter()
                .any(|entry| entry.id == section && entry.available)
        };
        Facts {
            authorized: authorized_text,
            stop,
            reconcile,
            compare_reason: dock::compare_unavailable_reason(text.artifacts.len()).map(Arc::from),
            active_task_reason: self.rail_selection().err(),
            active_candidate: self
                .dock_active
                .as_ref()
                .is_some_and(|identity| text.artifacts.iter().any(|row| row.identity == *identity)),
            evaluation_available: section_available(ui_model::inspector::Section::Evaluation),
            evidence_available: section_available(ui_model::inspector::Section::Evidence),
        }
    }

    /// Open the palette and remember the focus to restore on dismiss.
    pub(super) fn palette_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            return;
        }
        self.palette_restore_focus = window.focused(cx);
        self.palette_open = true;
        self.palette_query.clear();
        self.palette_input_selection = 0..0;
        self.palette_marked_range = None;
        self.palette_active = Some(0);
        self.palette_hit_len = 0;
        self.palette_focus.focus(window, cx);
        cx.notify();
    }

    /// Dismiss the palette and restore focus. No partial action survives.
    pub(super) fn palette_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_input_selection = 0..0;
        self.palette_marked_range = None;
        self.palette_active = None;
        self.palette_hit_len = 0;
        let restore = self
            .palette_restore_focus
            .take()
            .unwrap_or_else(|| self.focus.clone());
        restore.focus(window, cx);
        cx.notify();
    }

    pub(super) fn palette_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_close(window, cx);
        } else {
            self.palette_open(window, cx);
        }
    }

    /// Move the active row within the last rendered result length.
    pub(super) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        self.palette_active = palette::step(self.palette_active, self.palette_hit_len, delta);
        cx.notify();
    }

    pub(super) fn palette_set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.palette_query == query {
            return;
        }
        self.palette_query = query.to_owned();
        self.palette_input_selection = query.len()..query.len();
        self.palette_marked_range = None;
        self.palette_active = Some(0);
        cx.notify();
    }

    fn palette_replace_range(
        &mut self,
        range: std::ops::Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let current = self.palette_query.clone();
        let mut replacement =
            String::with_capacity(current.len() - (range.end - range.start) + text.len());
        replacement.push_str(&current[..range.start]);
        replacement.push_str(text);
        replacement.push_str(&current[range.end..]);
        let cursor = range.start + text.len();
        self.palette_set_query(&replacement, cx);
        self.palette_input_selection = cursor..cursor;
    }

    pub(super) fn palette_backspace(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let range = if !self.palette_input_selection.is_empty() {
            self.palette_input_selection.clone()
        } else {
            let cursor = self.palette_input_selection.start;
            self.palette_query[..cursor]
                .char_indices()
                .next_back()
                .map_or(cursor..cursor, |(index, ch)| index..index + ch.len_utf8())
        };
        if !range.is_empty() {
            self.palette_replace_range(range, "", cx);
        }
    }

    pub(super) fn palette_delete(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let range = if !self.palette_input_selection.is_empty() {
            self.palette_input_selection.clone()
        } else {
            let cursor = self.palette_input_selection.end;
            self.palette_query[cursor..]
                .char_indices()
                .next()
                .map_or(cursor..cursor, |(_, ch)| cursor..cursor + ch.len_utf8())
        };
        if !range.is_empty() {
            self.palette_replace_range(range, "", cx);
        }
    }

    pub(super) fn palette_move_cursor(&mut self, direction: isize, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let cursor = if direction < 0 {
            self.palette_query[..self.palette_input_selection.start]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index)
        } else {
            let current = self.palette_input_selection.end;
            self.palette_query[current..]
                .char_indices()
                .next()
                .map_or(self.palette_query.len(), |(offset, ch)| {
                    current + offset + ch.len_utf8()
                })
        };
        self.palette_input_selection = cursor..cursor;
        cx.notify();
    }

    pub(super) fn palette_select_all(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        self.palette_input_selection = 0..self.palette_query.len();
        cx.notify();
    }

    /// Invoke the active row, re-deriving availability at the moment of dispatch.
    pub(super) fn palette_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let authorized = self.authorized_text().is_some();
        let selection = {
            let facts = self.palette_facts(authorized, self.cache.text());
            let entries = entries(&facts);
            let Outcome::Hits(hits) = palette::search(&self.palette_query, &entries) else {
                return;
            };
            let index = self
                .palette_active
                .unwrap_or(0)
                .min(hits.len().saturating_sub(1));
            let entry = &entries[hits[index].slot];
            (
                entry.descriptor.command,
                entry.unavailable.map(str::to_owned),
            )
        };
        self.palette_close(window, cx);
        self.dispatch_palette(selection, window, cx);
    }

    /// Invoke one row by its inventory slot (pointer activation).
    pub(super) fn palette_invoke_slot(
        &mut self,
        slot: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.palette_open {
            return;
        }
        let authorized = self.authorized_text().is_some();
        let selection = {
            let facts = self.palette_facts(authorized, self.cache.text());
            let entries = entries(&facts);
            let Some(entry) = entries.get(slot) else {
                return;
            };
            (
                entry.descriptor.command,
                entry.unavailable.map(str::to_owned),
            )
        };
        self.palette_close(window, cx);
        self.dispatch_palette(selection, window, cx);
    }

    /// A command that lost availability between listing and invocation refuses
    /// with its honest reason and performs no partial action.
    fn dispatch_palette(
        &mut self,
        selection: (Command, Option<String>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (command, unavailable) = selection;
        if let Some(reason) = unavailable {
            self.surface_notice = Some(reason);
            cx.notify();
            return;
        }
        match command {
            Command::ShowArtifact => self.navigate(app_core::navigation::Surface::Artifact, cx),
            Command::ShowCompare => self.navigate(app_core::navigation::Surface::Compare, cx),
            Command::ShowFlow => self.navigate(app_core::navigation::Surface::Flow, cx),
            Command::NextSurface => self.advance_surface(cx),
            Command::OpenActiveTask => {
                if let Ok(task) = self.rail_selection() {
                    let task = task.clone();
                    self.select_rail_task(&task, window, cx);
                }
            }
            Command::OpenActiveCandidate => self.activate_dock_active(window, cx),
            Command::ShowEvaluation => {
                self.show_inspector_section(ui_model::inspector::Section::Evaluation, window, cx)
            }
            Command::ShowEvidence => {
                self.show_inspector_section(ui_model::inspector::Section::Evidence, window, cx)
            }
            Command::ToggleDiagnostics => {
                if let Some(workbench) = &mut self.workbench {
                    workbench.presentation.diagnostics_open =
                        !workbench.presentation.diagnostics_open;
                    cx.notify();
                }
            }
            Command::StopTask => self.start_stop(window, cx),
            Command::CheckStopReceipt => self.start_receipt_check(window, cx),
        }
    }

    fn current_inspector_view(&self) -> Option<ui_model::inspector::View> {
        let viewed = self
            .workbench
            .as_ref()
            .and_then(|workbench| workbench.presentation.viewed());
        Some(
            self.cache
                .text()
                .inspector
                .view(viewed.map(|viewed| &viewed.0)),
        )
    }

    fn show_inspector_section(
        &mut self,
        section: ui_model::inspector::Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.current_inspector_view() else {
            return;
        };
        let Some(index) = view.sections.iter().position(|entry| entry.id == section) else {
            return;
        };
        let revision = Arc::clone(&view.sections[index].revision);
        if !self.inspector_disclosure.is_open(section, &revision) {
            self.inspector_disclosure.toggle(section, &revision);
        }
        if let Some(handle) = self.inspector_focus.get(index) {
            handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Render the palette overlay; an empty element while the palette is closed.
    pub(super) fn palette_overlay(&mut self, facts: &Facts, cx: &mut Context<Self>) -> AnyElement {
        if !self.palette_open {
            return div().into_any_element();
        }
        let entries = entries(facts);
        let outcome = palette::search(&self.palette_query, &entries);
        let hits = match &outcome {
            Outcome::Hits(hits) => hits.clone(),
            _ => Vec::new(),
        };
        self.palette_hit_len = hits.len();
        if hits.is_empty() {
            self.palette_active = None;
        } else {
            let active = self.palette_active.unwrap_or(0).min(hits.len() - 1);
            self.palette_active = Some(active);
        }
        let active = self.palette_active;
        let query_empty = self.palette_query.trim().is_empty();
        let empty_message: Option<SharedString> = match outcome {
            Outcome::Hits(_) => None,
            Outcome::NoMatch => Some(SharedString::from(format!(
                "No commands match “{}”.",
                self.palette_query
            ))),
            Outcome::NoAvailable => Some("No commands are available in the current state.".into()),
        };
        let display = if self.palette_query.is_empty() {
            SharedString::from("Type a command…")
        } else {
            SharedString::from(self.palette_query.clone())
        };
        let entity = cx.entity();
        let mut rows = Vec::with_capacity(hits.len());
        for (index, hit) in hits.iter().enumerate() {
            let slot = hit.slot;
            let entry = entries[slot];
            let descriptor = entry.descriptor;
            let unavailable = entry.unavailable.map(str::to_owned);
            let is_active = active == Some(index);
            let label = SharedString::from(descriptor.title);
            let aria = match &unavailable {
                Some(reason) => format!(
                    "{} — {} — unavailable: {}",
                    descriptor.title, descriptor.category, reason
                ),
                None => format!("{} — {}", descriptor.title, descriptor.category),
            };
            let row = div()
                .id(("palette-row", slot))
                .role(Role::ListItem)
                .aria_label(SharedString::from(aria))
                .when(unavailable.is_some(), |element| {
                    element.aria_description(SharedString::from(
                        unavailable.clone().unwrap_or_default(),
                    ))
                })
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_2()
                .bg(rgb(if is_active && unavailable.is_none() {
                    0x3c5268
                } else if is_active {
                    0x2f3338
                } else {
                    0x24272c
                }))
                .text_color(rgb(if unavailable.is_some() {
                    0x8b949e
                } else {
                    0xe6e8eb
                }))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap_3()
                        .child(label)
                        .child(SharedString::from(descriptor.category)),
                )
                .when_some(unavailable.clone(), |element, reason| {
                    element.child(
                        div()
                            .text_color(rgb(0x8b949e))
                            .child(SharedString::from(reason)),
                    )
                });
            let row = if unavailable.is_some() {
                accessibility::DisabledState {
                    inner: row,
                    disabled: true,
                }
                .into_any_element()
            } else {
                let shell = entity.clone();
                row.on_click(move |_, window, app| {
                    shell.update(app, |this, cx| {
                        this.palette_invoke_slot(slot, window, cx);
                    });
                })
                .into_any_element()
            };
            rows.push(row);
        }
        let results = div()
            .id("palette-results")
            .role(Role::List)
            .aria_label("Commands")
            .flex()
            .flex_col()
            .children(rows)
            .children(empty_message.map(|message| {
                div()
                    .id("palette-empty")
                    .role(Role::Label)
                    .aria_label(message.clone())
                    .aria_value(message.clone())
                    .px_3()
                    .py_2()
                    .child(message)
            }))
            .children((query_empty && self.palette_query.is_empty()).then(|| {
                div()
                    .id("palette-hint")
                    .aria_label("Press Escape to dismiss")
                    .px_3()
                    .py_1()
                    .text_color(rgb(0x8b949e))
                    .child("Enter runs the highlighted command. Escape dismisses.")
            }));
        div()
            .id("command-palette")
            .role(Role::Dialog)
            .aria_label("Command Palette")
            .key_context("CommandPalette")
            .absolute()
            .top(px(72.0))
            .left(px(160.0))
            .w(px(560.0))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(rgb(0x626870))
            .bg(rgb(0x1c1f23))
            .on_action(cx.listener(|this, _: &crate::PaletteClose, window, cx| {
                this.palette_close(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteEnter, window, cx| {
                this.palette_activate(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteUp, _, cx| {
                this.palette_move(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteDown, _, cx| {
                this.palette_move(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteNext, _, cx| {
                this.palette_move(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PalettePrevious, _, cx| {
                this.palette_move(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteBackspace, _, cx| {
                this.palette_backspace(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteDelete, _, cx| {
                this.palette_delete(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteLeft, _, cx| {
                this.palette_move_cursor(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteRight, _, cx| {
                this.palette_move_cursor(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::PaletteSelectAll, _, cx| {
                this.palette_select_all(cx);
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.palette_close(window, cx);
            }))
            .child(
                div()
                    .id("palette-input")
                    .role(Role::TextInput)
                    .aria_label("Command search")
                    .aria_value(display.clone())
                    .track_focus(&self.palette_focus)
                    .relative()
                    .px_3()
                    .py_2()
                    .border_1()
                    .border_color(rgb(0xe6c278))
                    .bg(rgb(0x17191c))
                    .child(display)
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .child(PaletteInputRegistration {
                                entity: entity.clone(),
                            }),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.palette_focus.focus(window, cx);
                    })),
            )
            .child(results)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionFacts, Facts, entries, require};
    use std::sync::Arc;
    use ui_model::palette::{Command, Entry};

    fn action(enabled: bool, reason: &str) -> ActionFacts {
        ActionFacts {
            enabled,
            reason: Arc::from(reason),
        }
    }

    fn facts() -> Facts {
        Facts {
            authorized: true,
            stop: action(true, "stop available"),
            reconcile: action(true, "receipt available"),
            compare_reason: None,
            active_task_reason: None,
            active_candidate: true,
            evaluation_available: true,
            evidence_available: true,
        }
    }

    fn reason_of(entries: &[Entry<'_>], command: Command) -> Option<String> {
        entries
            .iter()
            .find(|entry| entry.descriptor.command == command)
            .expect("inventory covers every command")
            .unavailable
            .map(str::to_owned)
    }

    #[test]
    fn a_fully_available_state_lists_every_command_as_runnable() {
        let facts = facts();
        let entries = entries(&facts);
        assert_eq!(entries.len(), ui_model::palette::INVENTORY.len());
        assert!(entries.iter().all(Entry::is_available));
    }

    #[test]
    fn an_unauthorized_observation_blocks_every_projection_command_but_not_navigation() {
        let mut facts = facts();
        facts.authorized = false;
        let entries = entries(&facts);
        for command in [
            Command::ShowCompare,
            Command::OpenActiveTask,
            Command::OpenActiveCandidate,
            Command::ShowEvaluation,
            Command::ShowEvidence,
            Command::StopTask,
            Command::CheckStopReceipt,
        ] {
            assert_eq!(
                reason_of(&entries, command).as_deref(),
                Some(super::STALE),
                "{command:?} must refuse without an authorized observation"
            );
        }
        for command in [
            Command::ShowArtifact,
            Command::ShowFlow,
            Command::NextSurface,
            Command::ToggleDiagnostics,
        ] {
            assert_eq!(
                reason_of(&entries, command),
                None,
                "{command:?} must stay available"
            );
        }
    }

    #[test]
    fn missing_context_carries_its_own_reason_and_never_invents_availability() {
        let mut facts = facts();
        facts.active_task_reason =
            Some("The active Task is not in the current Task search results.");
        facts.active_candidate = false;
        facts.evaluation_available = false;
        facts.evidence_available = false;
        facts.compare_reason = Some(Arc::from("two candidates are required"));
        facts.stop = action(
            false,
            "Stop is disabled until this window has a current authorized observation view.",
        );
        facts.reconcile = action(false, "No Stop command identity is available.");
        let entries = entries(&facts);
        assert!(reason_of(&entries, Command::OpenActiveTask).is_some());
        assert!(reason_of(&entries, Command::OpenActiveCandidate).is_some());
        assert!(reason_of(&entries, Command::ShowEvaluation).is_some());
        assert!(reason_of(&entries, Command::ShowEvidence).is_some());
        assert_eq!(
            reason_of(&entries, Command::ShowCompare).as_deref(),
            Some("two candidates are required")
        );
        assert_eq!(
            reason_of(&entries, Command::StopTask).as_deref(),
            Some("Stop is disabled until this window has a current authorized observation view.")
        );
        assert_eq!(
            reason_of(&entries, Command::CheckStopReceipt).as_deref(),
            Some("No Stop command identity is available.")
        );
    }

    #[test]
    fn the_stale_reason_wins_over_any_other_unavailability() {
        let mut facts = facts();
        facts.authorized = false;
        facts.stop = action(false, "unused");
        let entries = entries(&facts);
        assert_eq!(
            reason_of(&entries, Command::StopTask).as_deref(),
            Some(super::STALE)
        );
    }

    #[test]
    fn require_reports_authorization_first_then_availability() {
        assert_eq!(require(false, true, "reason"), Some(super::STALE));
        assert_eq!(require(false, false, "reason"), Some(super::STALE));
        assert_eq!(require(true, true, "reason"), None);
        assert_eq!(require(true, false, "reason"), Some("reason"));
    }
}
