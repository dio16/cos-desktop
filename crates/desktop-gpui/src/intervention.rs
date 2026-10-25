//! Intervention strip surface (DC-MP-INS3 / ADR-0011).
//!
//! In-app only. Every row is a typed Desktop-local condition; nothing here sends a
//! canonical command, calls an OS notification API, or starts an action on render.
//! Actions are read-only navigation, the existing read-only receipt presentation, or a
//! local re-subscription of the authorized observation.
//!
//! Same-kind conditions are grouped for display (ADR-0011 §2): one visible row per kind
//! carries the group's count and its highest-priority member, and expanding it lists every
//! individual condition. The model never flattens distinct typed identities.
//!
//! Every bit of shell-local strip state is task-scoped: a switch invalidates it at the switch
//! boundary and again at render, and rows derived for another task are never shown, actioned or
//! focused — including while the new task's derivation is still in flight (ADR-0011 §3).

use std::collections::{BTreeMap, BTreeSet};

use desktop_protocol::TaskId;
use gpui::{AnyElement, Context, IntoElement, Role, SharedString, Window, div, prelude::*, rgb};
use ui_model::intervention::{Action, Reference, Row, RowIdentity, Strip};

use crate::shell_style::TextSpec;
use crate::tokens::TextStyle;
use app_core::navigation::Surface;

use crate::{InterventionActivate, InterventionDismiss, Shell};

/// Shell-local strip state: expansion, the row whose read-only details are open, and the row
/// that opened them, all scoped to the task they belong to.
///
/// Identity-only so it can be reconciled against the active row set without any view data
/// (ADR-0011 §3: a resolved condition must never persist, and a task switch must never carry the
/// previous task's state across).
#[derive(Default)]
pub(super) struct StripSelection {
    scope: Option<TaskId>,
    expanded: bool,
    detail: Option<RowIdentity>,
    detail_origin: Option<RowIdentity>,
}

impl StripSelection {
    /// Drop every selection state when the task scope changes, and remember the new scope.
    ///
    /// Returns whether the scope changed. Expansion, open details and the dismissal origin belong
    /// to the task they were opened in, so switching back never resurrects them.
    pub(super) fn switch_task_scope(&mut self, scope: Option<TaskId>) -> bool {
        if self.scope == scope {
            return false;
        }
        self.scope = scope;
        self.expanded = false;
        self.detail = None;
        self.detail_origin = None;
        true
    }

    /// Whether the grouped conditions are listed in full.
    pub(super) fn expanded(&self) -> bool {
        self.expanded
    }

    /// Expand or collapse the grouped conditions.
    pub(super) fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// The row whose details are open.
    pub(super) fn detail(&self) -> Option<&RowIdentity> {
        self.detail.as_ref()
    }

    /// Open the read-only details for one row and remember where focus returns on dismissal.
    pub(super) fn open_details(&mut self, identity: RowIdentity) {
        self.detail = Some(identity.clone());
        self.detail_origin = Some(identity);
    }

    /// Close the details and return the originating row, but only while that row is displayed:
    /// focus never returns to a row that disappeared or is grouped away.
    pub(super) fn close_details(
        &mut self,
        displayed: &BTreeSet<RowIdentity>,
    ) -> Option<RowIdentity> {
        self.detail = None;
        self.detail_origin
            .take()
            .filter(|identity| displayed.contains(identity))
    }

    /// Drop details state for rows that are no longer active; returns whether anything was dropped.
    pub(super) fn reconcile(&mut self, active: &BTreeSet<RowIdentity>) -> bool {
        let mut changed = false;
        if self
            .detail
            .as_ref()
            .is_some_and(|identity| !active.contains(identity))
        {
            self.detail = None;
            changed = true;
        }
        if self
            .detail_origin
            .as_ref()
            .is_some_and(|identity| !active.contains(identity))
        {
            self.detail_origin = None;
            changed = true;
        }
        changed
    }
}

/// Drop per-row focus handles for identities that are no longer active, so a reappearing identity
/// starts with a fresh, unfocused handle instead of reusing a stale one.
pub(super) fn prune_strip_focus<T>(
    focus: &mut BTreeMap<RowIdentity, T>,
    active: &BTreeSet<RowIdentity>,
) -> bool {
    let before = focus.len();
    focus.retain(|identity, _| active.contains(identity));
    focus.len() != before
}

/// Whether a strip derived for `text_task` describes the task currently selected.
///
/// The window between a task switch and the new derivation still holds the previous task's derived
/// text, so those rows are out of scope. A derivation with no task (blocked observation, or no task
/// at all) is never task-scoped, and with no selection the fallback derivation is what the window
/// actually shows, so it stays in scope.
pub(super) fn strip_scope_matches(text_task: Option<&TaskId>, scope: Option<&TaskId>) -> bool {
    match (text_task, scope) {
        (Some(text_task), Some(scope)) => text_task == scope,
        _ => true,
    }
}

/// The strip the shell may render: the derived strip only while it belongs to the selected task,
/// otherwise nothing (the in-flight window after a task switch).
pub(super) fn scoped_strip(
    strip: Strip,
    text_task: Option<&TaskId>,
    scope: Option<&TaskId>,
) -> Strip {
    if strip_scope_matches(text_task, scope) {
        strip
    } else {
        Strip::default()
    }
}

/// One row the strip renders, and how many conditions that row stands for: the group size for a
/// group's visible row, and one for each individually revealed member.
pub(super) struct RevealedRow {
    pub(super) row: Row,
    pub(super) count: usize,
}

/// The rows the strip renders for the current expansion state: one entry per kind group while
/// collapsed, plus every grouped-away member listed below its head while expanded. Every identity
/// in the model is reachable in exactly one entry.
pub(super) fn revealed_rows(strip: &Strip, expanded: bool) -> Vec<RevealedRow> {
    let mut revealed = Vec::new();
    for group in strip.groups.iter() {
        revealed.push(RevealedRow {
            row: group.head.clone(),
            count: group.count(),
        });
        if expanded {
            for row in group.members.iter().skip(1) {
                revealed.push(RevealedRow {
                    row: row.clone(),
                    count: 1,
                });
            }
        }
    }
    revealed
}

impl Shell {
    /// Build the in-app intervention strip for the current authorized observation.
    ///
    /// An empty strip renders nothing; rendering never triggers an action. Same-kind conditions
    /// are one visible row plus a count while collapsed, with every identity listed on demand.
    pub(super) fn intervention_strip(
        &mut self,
        strip: Strip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if strip.is_empty() {
            return div().into_any_element();
        }
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
        let expanded = self.strip_selection.expanded();
        let revealed = revealed_rows(&strip, expanded);
        for (slot, revealed_row) in revealed.into_iter().enumerate() {
            panel = panel.child(self.strip_row(
                &revealed_row.row,
                slot,
                revealed_row.count,
                &xs_text,
                window,
                cx,
            ));
        }
        if strip.grouped_away() > 0 {
            let label = if expanded {
                format!("Show fewer ({} conditions)", strip.rows.len())
            } else {
                format!("Show all {} conditions", strip.rows.len())
            };
            let focused = self.strip_list_focus.is_focused(window);
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
        }
        if let Some(identity) = self.strip_selection.detail().cloned()
            && let Some(row) = strip.rows.iter().find(|row| row.identity == identity)
        {
            panel = panel.child(self.strip_details(row, &xs_text, window, cx));
        }
        panel.into_any_element()
    }

    fn strip_row(
        &mut self,
        row: &Row,
        slot: usize,
        count: usize,
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
        // The visible row of a group states how many conditions it stands for, so grouping can
        // never look like a single unrelated condition.
        let summary = if count > 1 {
            format!("{label} — {action} · {count} conditions")
        } else {
            format!("{label} — {action}")
        };
        let identity = row.identity.clone();
        let click_identity = identity.clone();
        let action_identity = identity;
        div()
            .id(("intervention-row", slot))
            .role(Role::Button)
            .aria_label(SharedString::from(summary.clone()))
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
            .child(SharedString::from(summary))
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
        let scope = self.rail_state.selected().cloned();
        if !strip_scope_matches(self.cache.text().task.as_ref(), scope.as_ref()) {
            // Rows derived for another task are not operable, even if a stale identity reaches
            // this path during the in-flight window after a switch.
            return;
        }
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
                self.strip_selection.open_details(row.identity);
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

    /// Re-establish this window's observation through the same app-core publication subscription
    /// the shell starts with (ADR-0011 §1.5: local re-subscription, no canonical mutation).
    ///
    /// The previous subscription task is dropped and a fresh one is armed, so a lost or closed
    /// subscription is genuinely re-established. A display-cache re-read is never a substitute;
    /// text is only revalidated to the live publication identity, exactly as render does.
    pub(super) fn reconnect_observation(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            self.surface_notice = Some(
                "Reconnect requested — this window has no authorized observation to subscribe to."
                    .to_owned(),
            );
            cx.notify();
            return;
        };
        self._subscription = Self::subscribe(handle.clone(), cx);
        let live = Self::current_notice(&handle);
        self.cache.revalidate(live);
        self.rail_state.revalidate(live);
        self.request_rail_work(cx);
        if handle.current().is_err() {
            self.surface_notice = Some(
                "Reconnect requested — no authorized observation is available yet.".to_owned(),
            );
        }
        cx.notify();
    }

    /// Expand or collapse the grouped conditions.
    pub(super) fn toggle_strip_list(&mut self, cx: &mut Context<Self>) {
        self.strip_selection.toggle_expanded();
        cx.notify();
    }

    /// Close the read-only detail panel and return focus to the originating row, but only when that
    /// row is still displayed (a resolved or grouped-away row never receives focus).
    pub(super) fn close_strip_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let displayed = self.displayed_strip_identities();
        let origin = self.strip_selection.close_details(&displayed);
        if let Some(identity) = origin
            && let Some(handle) = self.strip_focus.get(&identity).cloned()
        {
            handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Identities rendered right now: every condition while expanded, group heads while collapsed.
    fn displayed_strip_identities(&self) -> BTreeSet<RowIdentity> {
        let strip = self.cache.text().interventions.strip();
        if self.strip_selection.expanded() {
            strip.rows.iter().map(|row| row.identity.clone()).collect()
        } else {
            strip
                .groups
                .iter()
                .map(|group| group.head.identity.clone())
                .collect()
        }
    }

    /// Invalidate identity-keyed strip state when the task scope changes, and record the new scope.
    ///
    /// Called at every switch boundary (`select_rail_task`, `restore_pending_task`) and again from
    /// render, so the switch itself and any render in the in-flight window both drop the previous
    /// task's rows, details, focus handles and dismissal origin.
    pub(super) fn invalidate_strip_for_task_scope(&mut self, scope: Option<TaskId>) {
        if self.strip_selection.switch_task_scope(scope) {
            self.strip_focus.clear();
        }
    }

    /// Reconcile shell-local strip state with the task scope and the rows about to be rendered.
    ///
    /// A task switch invalidates the identity-keyed selection immediately, and while the new task's
    /// derivation is still in flight the cached text belongs to the previous task: its rows are out
    /// of scope, so the active set is empty and nothing from them survives. A resolved condition or
    /// a reappearing identity behave as before (ADR-0011 §3).
    pub(super) fn reconcile_strip_state(&mut self) {
        let scope = self.rail_state.selected().cloned();
        self.invalidate_strip_for_task_scope(scope.clone());
        let text = self.cache.text();
        let active: BTreeSet<RowIdentity> =
            if strip_scope_matches(text.task.as_ref(), scope.as_ref()) {
                text.interventions
                    .rows()
                    .iter()
                    .map(|row| row.identity.clone())
                    .collect()
            } else {
                BTreeSet::new()
            };
        self.strip_selection.reconcile(&active);
        prune_strip_focus(&mut self.strip_focus, &active);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use desktop_protocol::{CapabilitiesId, CommandId, TaskId};
    use ui_model::intervention::{
        Action, ConnectionKind, Kind, Provenance, Reference, Row, RowIdentity, Strip, StripGroup,
    };

    use super::{
        StripSelection, prune_strip_focus, revealed_rows, scoped_strip, strip_scope_matches,
    };

    fn stop_row(command: &str) -> Row {
        Row {
            identity: RowIdentity::Stop(CommandId::fixture(command)),
            kind: Kind::Stop,
            task_status: None,
            provenance: Provenance::DesktopLocal,
            label: format!("Stop {command}").into(),
            detail: "Local presentation only.".into(),
            action: Some(Action::Details),
        }
    }

    fn connection_row(kind: ConnectionKind) -> Row {
        Row {
            identity: RowIdentity::Connection(kind),
            kind: Kind::Connection,
            task_status: None,
            provenance: Provenance::DesktopLocal,
            label: "Connection stale".into(),
            detail: "The visible projection may be stale.".into(),
            action: Some(Action::Reconnect),
        }
    }

    fn group(kind: Kind, rows: Vec<Row>) -> StripGroup {
        let head = rows.first().cloned().expect("a group has rows");
        StripGroup {
            kind,
            head,
            members: rows.into(),
        }
    }

    fn strip(groups: Vec<StripGroup>) -> Strip {
        let rows: Vec<Row> = groups
            .iter()
            .flat_map(|group| group.members.iter().cloned())
            .collect();
        Strip {
            rows: rows.into(),
            groups: groups.into(),
        }
    }

    fn missing_reference(task: &str, capability: &str) -> RowIdentity {
        RowIdentity::MissingReference {
            task: TaskId::fixture(task),
            reference: Reference::Capabilities(CapabilitiesId::fixture(capability)),
        }
    }

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

    #[test]
    fn reconnect_rearms_the_app_core_subscription() {
        let source = include_str!("intervention.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
        let start = production
            .find("fn reconnect_observation")
            .expect("reconnect_observation exists");
        let reconnect = &production[start..];
        let reconnect = &reconnect[..reconnect.find("\n    /// ").unwrap_or(reconnect.len())];
        assert!(
            reconnect.contains("self._subscription = Self::subscribe(handle.clone(), cx)"),
            "reconnect must re-arm the existing app-core subscription, not re-read the cache"
        );
    }

    #[test]
    fn one_visible_row_per_kind_lists_every_identity_on_demand() {
        let strip = strip(vec![
            group(
                Kind::Stop,
                vec![
                    stop_row("command/a"),
                    stop_row("command/b"),
                    stop_row("command/c"),
                ],
            ),
            group(
                Kind::Connection,
                vec![connection_row(ConnectionKind::Stale)],
            ),
        ]);

        let collapsed = revealed_rows(&strip, false);
        assert_eq!(
            collapsed.len(),
            2,
            "one visible row per kind while collapsed"
        );
        assert_eq!(collapsed[0].count, 3, "the group row carries its count");
        assert_eq!(
            collapsed[0].row.identity,
            RowIdentity::Stop(CommandId::fixture("command/a")),
            "the head is the highest-priority member"
        );
        assert_eq!(collapsed[1].count, 1);

        let expanded = revealed_rows(&strip, true);
        assert_eq!(
            expanded.len(),
            strip.rows.len(),
            "expanding reaches every identity exactly once"
        );
        assert_eq!(
            expanded
                .iter()
                .map(|revealed| revealed.row.identity.clone())
                .collect::<Vec<_>>(),
            vec![
                RowIdentity::Stop(CommandId::fixture("command/a")),
                RowIdentity::Stop(CommandId::fixture("command/b")),
                RowIdentity::Stop(CommandId::fixture("command/c")),
                RowIdentity::Connection(ConnectionKind::Stale),
            ],
            "each distinct typed identity stays individually reachable"
        );
        assert_eq!(
            expanded[0].count, 3,
            "the revealed head still states its group count"
        );
        assert_eq!(expanded[1].count, 1);
    }

    #[test]
    fn an_authorization_block_drops_stale_strip_state() {
        let stale = RowIdentity::Connection(ConnectionKind::Stale);
        let mut selection = StripSelection::default();
        selection.open_details(stale.clone());
        let mut focus = BTreeMap::from([(stale.clone(), 1usize)]);

        let blocked = BTreeSet::from([RowIdentity::ObservationUnavailable]);
        assert!(selection.reconcile(&blocked));
        assert!(prune_strip_focus(&mut focus, &blocked));
        assert_eq!(None, selection.detail(), "the details panel must close");
        assert!(focus.is_empty(), "no handle survives the block");
        assert_eq!(
            None,
            selection.close_details(&blocked),
            "nothing may regain focus"
        );
    }

    #[test]
    fn a_task_switch_drops_the_previous_tasks_strip_state() {
        let previous = missing_reference("task/1", "capabilities/1");
        let next = missing_reference("task/2", "capabilities/2");
        let mut selection = StripSelection::default();
        selection.open_details(previous.clone());
        let mut focus = BTreeMap::from([(previous.clone(), 1usize)]);

        let switched = BTreeSet::from([next]);
        assert!(selection.reconcile(&switched));
        assert!(prune_strip_focus(&mut focus, &switched));
        assert_eq!(None, selection.detail());
        assert!(focus.is_empty());
    }

    #[test]
    fn a_disappearing_row_drops_its_details_and_focus() {
        let kept = RowIdentity::Connection(ConnectionKind::Stale);
        let resolved = RowIdentity::Stop(CommandId::fixture("command/stop"));
        let mut selection = StripSelection::default();
        selection.open_details(resolved.clone());
        let mut focus = BTreeMap::from([(kept.clone(), 1usize), (resolved.clone(), 2usize)]);

        let active = BTreeSet::from([kept.clone()]);
        assert!(selection.reconcile(&active));
        assert!(prune_strip_focus(&mut focus, &active));
        assert_eq!(None, selection.detail());
        assert_eq!(None, selection.close_details(&active));
        assert_eq!(Some(&1usize), focus.get(&kept));
        assert!(!focus.contains_key(&resolved));
    }

    #[test]
    fn a_reappearing_identity_starts_clean() {
        let identity = RowIdentity::Connection(ConnectionKind::Disconnected);
        let mut selection = StripSelection::default();
        selection.open_details(identity.clone());
        let mut focus = BTreeMap::from([(identity.clone(), 1usize)]);

        let gone = BTreeSet::new();
        assert!(selection.reconcile(&gone));
        assert!(prune_strip_focus(&mut focus, &gone));

        let back = BTreeSet::from([identity.clone()]);
        assert!(!selection.reconcile(&back), "nothing stale remains to drop");
        assert_eq!(
            None,
            selection.detail(),
            "details must not reopen from a previous publication"
        );
        assert!(
            !focus.contains_key(&identity),
            "a stale handle is never reused"
        );
    }

    #[test]
    fn dismissal_from_a_grouped_away_row_does_not_restore_its_focus() {
        let head = RowIdentity::Connection(ConnectionKind::Stale);
        let grouped_away = RowIdentity::Stop(CommandId::fixture("command/stop"));
        let mut selection = StripSelection::default();
        selection.open_details(grouped_away.clone());
        // Collapsed: only group heads are displayed.
        assert_eq!(
            None,
            selection.close_details(&BTreeSet::from([head.clone()])),
            "a grouped-away row is not displayed and must not regain focus"
        );
        assert_eq!(
            None,
            selection.detail(),
            "dismissal always closes the panel"
        );

        let mut selection = StripSelection::default();
        selection.open_details(grouped_away.clone());
        // Expanded: the same row is displayed and is the focus target.
        let expanded = BTreeSet::from([head, grouped_away.clone()]);
        assert_eq!(Some(grouped_away), selection.close_details(&expanded));
    }

    fn scoped_a_strip() -> (TaskId, TaskId, Strip) {
        let task_a = TaskId::fixture("task/a");
        let task_b = TaskId::fixture("task/b");
        let row = connection_row(ConnectionKind::Stale);
        let strip = strip(vec![group(Kind::Connection, vec![row])]);
        (task_a, task_b, strip)
    }

    #[test]
    fn a_task_switch_immediately_invalidates_the_previous_tasks_strip() {
        let (task_a, task_b, previous_strip) = scoped_a_strip();
        let row = previous_strip.rows[0].identity.clone();
        let mut selection = StripSelection::default();
        assert!(selection.switch_task_scope(Some(task_a.clone())));
        selection.toggle_expanded();
        selection.open_details(row.clone());
        let mut focus = BTreeMap::from([(row.clone(), 1usize)]);

        // The switch boundary drops the previous task's state without waiting for a render.
        assert!(selection.switch_task_scope(Some(task_b.clone())));
        assert!(
            prune_strip_focus(&mut focus, &BTreeSet::new()),
            "no focus handle may survive the switch"
        );
        assert!(
            !selection.expanded(),
            "expansion belongs to the previous task"
        );
        assert_eq!(
            None,
            selection.detail(),
            "details belong to the previous task"
        );
        assert!(focus.is_empty());
        // And the previous task's derived rows are out of scope for the new selection.
        assert!(scoped_strip(previous_strip, Some(&task_a), Some(&task_b)).is_empty());
    }

    #[test]
    fn an_in_flight_derivation_shows_no_previous_task_rows_or_state() {
        let (task_a, task_b, previous_strip) = scoped_a_strip();
        let stale_row = previous_strip.rows[0].identity.clone();
        let mut selection = StripSelection::default();
        assert!(selection.switch_task_scope(Some(task_a.clone())));
        selection.open_details(stale_row.clone());
        let mut focus = BTreeMap::from([(stale_row.clone(), 1usize)]);

        // Switch to B; B's derivation has not completed, so the cached text is still A's.
        selection.switch_task_scope(Some(task_b.clone()));
        assert!(
            !strip_scope_matches(Some(&task_a), Some(&task_b)),
            "the previous task's text is not in scope"
        );
        let rendered = scoped_strip(previous_strip, Some(&task_a), Some(&task_b));
        assert!(
            rendered.is_empty(),
            "no previous-task row may be rendered while B derives"
        );
        assert_eq!(rendered.groups.len(), 0);
        // Reconciliation against the empty active set leaves no handle, details or origin.
        let active = BTreeSet::new();
        selection.reconcile(&active);
        prune_strip_focus(&mut focus, &active);
        assert!(focus.is_empty(), "no stale focus handle");
        assert_eq!(None, selection.detail(), "no stale details");
        assert_eq!(None, selection.close_details(&active), "no stale origin");
    }

    #[test]
    fn a_completed_derivation_shows_only_the_new_tasks_rows() {
        let (task_a, task_b, _) = scoped_a_strip();
        let rows_b = vec![stop_row("command/b1"), stop_row("command/b2")];
        let strip_b = strip(vec![group(Kind::Stop, rows_b.clone())]);
        assert!(strip_scope_matches(Some(&task_b), Some(&task_b)));

        let rendered = scoped_strip(strip_b, Some(&task_b), Some(&task_b));
        assert_eq!(
            rendered
                .rows
                .iter()
                .map(|row| row.identity.clone())
                .collect::<Vec<_>>(),
            rows_b
                .iter()
                .map(|row| row.identity.clone())
                .collect::<Vec<_>>(),
            "only the selected task's rows render"
        );
        assert!(
            !strip_scope_matches(Some(&task_a), Some(&task_b)),
            "the previous task's text stays out of scope"
        );
    }

    #[test]
    fn switching_back_does_not_resurrect_earlier_strip_state() {
        let (task_a, task_b, strip_a) = scoped_a_strip();
        let row = strip_a.rows[0].identity.clone();
        let mut selection = StripSelection::default();
        assert!(selection.switch_task_scope(Some(task_a.clone())));
        selection.toggle_expanded();
        selection.open_details(row.clone());
        let mut focus = BTreeMap::from([(row.clone(), 1usize)]);

        assert!(selection.switch_task_scope(Some(task_b.clone())));
        prune_strip_focus(&mut focus, &BTreeSet::new());
        // Back to A: its rows may render again, but only as freshly derived state.
        assert!(selection.switch_task_scope(Some(task_a.clone())));
        assert!(
            !selection.expanded(),
            "expanded state must not be resurrected"
        );
        assert_eq!(None, selection.detail(), "details must not reopen");
        assert_eq!(
            None,
            selection.close_details(&BTreeSet::from([row.clone()])),
            "no earlier dismissal origin is resurrected"
        );
        assert!(focus.is_empty(), "no earlier focus handle is resurrected");
    }

    #[test]
    fn the_shell_applies_the_scope_gate_at_the_switch_and_at_render() {
        let shell = include_str!("lib.rs");
        assert!(
            shell.contains("self.invalidate_strip_for_task_scope(Some(task.clone()));"),
            "the switch boundary must invalidate the previous task's strip state"
        );
        assert!(
            shell.contains("self.reconcile_strip_state();"),
            "render must reconcile the strip state"
        );
        assert!(
            shell.contains("intervention::scoped_strip("),
            "render must gate the strip on the selected task scope"
        );
    }
}
