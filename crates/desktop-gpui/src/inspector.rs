//! Inspector panel (DC-MP-INS1 / MP-INS1).
//!
//! Progressive disclosure over `ui_model::inspector::View`. Every value is shown as
//! the Runtime projects it: a missing value keeps its explicit absent state, and no
//! score, best candidate, outcome or evidence body is inferred, computed or fetched.
//! Disclosure state is derived-only local state keyed by section revision.

use std::sync::Arc;

use desktop_protocol::{EvaluationId, TaskId};
use gpui::{
    AnyElement, Context, IntoElement, Role, SharedString, Window, div, prelude::*, px, rgb,
};
use ui_model::evidence::{
    EvidenceDetail, EvidenceDetailCache, EvidenceDisplayState, EvidenceGroupSource, EvidenceSource,
};
use ui_model::inspector::{Section, View};

use crate::shell_style::TextSpec;
use crate::tokens::TextStyle;
use crate::{InspectorToggle, Shell};

/// Materialize evidence rows only for the group a user actually opened.
///
/// A collapsed group never builds a row view, and a stale revision is removed by
/// [`EvidenceDetailCache::request`] itself, so the returned rows always match
/// the authoritative evaluation revision the source carries.
pub(super) fn evidence_detail_for_group<'a>(
    cache: &'a mut EvidenceDetailCache,
    source: Option<&EvidenceSource>,
    evaluation: &EvaluationId,
    open: bool,
) -> Option<&'a EvidenceDetail> {
    if !open {
        return None;
    }
    cache.request(source?, evaluation)
}

/// Per-status counts shown in one group header before its detail is opened.
fn group_status_summary(group: &EvidenceGroupSource) -> String {
    use EvidenceDisplayState as State;
    let mut available = 0usize;
    let mut pending = 0usize;
    let mut missing = 0usize;
    let mut forbidden = 0usize;
    let mut unsupported = 0usize;
    let mut unknown = 0usize;
    for record in group.records.iter() {
        match State::from_wire(&record.status) {
            State::Available => available += 1,
            State::Pending => pending += 1,
            State::Missing => missing += 1,
            State::Forbidden => forbidden += 1,
            State::Unsupported => unsupported += 1,
            State::Unknown(_) => unknown += 1,
        }
    }
    let mut parts = Vec::new();
    for (label, count) in [
        ("取得可能", available),
        ("準備中", pending),
        ("見つかりません", missing),
        ("許可されていません", forbidden),
        ("未対応", unsupported),
        ("不明", unknown),
    ] {
        if count > 0 {
            parts.push(format!("{label} {count}"));
        }
    }
    parts.join(" / ")
}

/// The Task and withholding decision for one inspector render.
pub(super) struct PanelScope<'a> {
    pub task: Option<&'a TaskId>,
    pub withheld: Option<&'static str>,
}

/// The compared/viewed candidate the inspector captions: its local label and whether the
/// authorized projection actually contains it. One value, so the label can never be rendered
/// with an availability that belongs to another candidate.
pub(super) struct ViewedCaption {
    pub label: SharedString,
    pub available: bool,
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
        viewed: ViewedCaption,
        scope: PanelScope<'_>,
        palette: crate::contrast::Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scheme = palette.scheme;
        let colors = palette.colors;
        // Text is resolved against the surface the panel actually paints, not the window.
        let panel_surface = palette.color(0x202328, colors.raised_face);
        let body_text = TextSpec::ui(&self.tokens, TextStyle::Body);
        let xs_text = TextSpec::ui(&self.tokens, TextStyle::Xs);
        let mut panel = div()
            .id("inspector-panel")
            .role(Role::Group)
            .aria_label("詳細と説明")
            .flex()
            .flex_col()
            .gap_2()
            .bg(rgb(crate::contrast::role_color(
                scheme,
                0x202328,
                colors.raised_face,
            )))
            .text_color(rgb(palette.readable(colors.text, panel_surface)));
        if let Some(reason) = scope.withheld {
            // A blocked or out-of-scope projection invalidates disclosure state
            // instead of merely skipping the sync, so the next in-scope Task or
            // re-observation starts collapsed (A1). Materialized evidence detail
            // is authorized data, so it is dropped with the projection.
            self.inspector_disclosure.invalidate();
            self.inspector_evidence.clear();
            self.inspector_evidence_focus.clear();
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
            .sync(view.candidate.as_ref(), viewed.available);
        // Drop any materialized detail that no longer matches the current
        // candidate/revision before it can be rendered this frame.
        self.inspector_evidence.sync(view.evidence.as_ref());
        if let Some(source) = view.evidence.as_ref() {
            self.inspector_evidence_focus.retain(|key, _| {
                source
                    .groups
                    .iter()
                    .any(|group| &group.disclosure_key() == key)
            });
        } else {
            self.inspector_evidence_focus.clear();
        }
        let mut current: Vec<(Section, Arc<str>)> = view
            .sections
            .iter()
            .map(|section| (section.id, Arc::clone(&section.revision)))
            .collect();
        if let Some(source) = view.evidence.as_ref() {
            for group in source.groups.iter() {
                current.push((Section::Evidence, group.disclosure_key()));
            }
        }
        self.inspector_disclosure.prune(&current);

        let viewed_label = &viewed.label;
        let candidate_state: SharedString = if view.candidate.is_none() {
            "候補は表示されていません".into()
        } else if viewed.available {
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

        // NAV3 entry point from the task context: the history surface opens for
        // the selected task and restores local view only. Ctrl+H is the keyboard
        // equivalent bound by the shell.
        if scope.task.is_some() {
            panel = panel.child(
                div()
                    .id("inspector-history-open")
                    .role(Role::Button)
                    .aria_label("このタスクの実行履歴を開く")
                    .aria_description(
                        "履歴パネルを開きます。過去の時点はローカル表示のみを復元し、正式な選択は変更しません。",
                    )
                    .px_3()
                    .py_2()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.history_open_surface(window, cx);
                    }))
                    .child("履歴を開く（Ctrl+H）"),
            );
        }

        for (index, section) in view.sections.iter().enumerate() {
            let Some(handle) = self.inspector_focus.get(index) else {
                continue;
            };
            let open = self
                .inspector_disclosure
                .is_open(section.id, &section.revision);
            let focused = handle.is_focused(window);
            // Focus changes the border width as well as the colour: a monochrome High
            // Contrast palette can resolve focus and the resting border to one value.
            let (header_border_width, header_border_color) = match crate::shell_style::control_focus(
                &self.tokens,
                focused,
                window.scale_factor(),
                &colors,
                panel_surface,
                colors.border,
                &[],
            ) {
                Ok(border) => border,
                Err(_) => (px(0.0), colors.border),
            };
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
                            palette,
                            cx,
                        )
                    }
                })
                .key_context("InspectorSection")
                .px_3()
                .py_2()
                .border(header_border_width)
                .border_color(rgb(header_border_color))
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
                        .text_color(rgb(crate::contrast::readable_text(
                            &colors,
                            crate::contrast::role_color(scheme, 0xd08a6a, colors.muted_text),
                            crate::contrast::role_color(scheme, 0x202328, colors.raised_face),
                        )))
                        .child(SharedString::from(
                            section
                                .absent_reason
                                .clone()
                                .unwrap_or_else(|| Arc::from("表示情報がありません。")),
                        )),
                );
            }
            if section.id == Section::Evidence {
                if !section.available {
                    panel = panel.child(details);
                    continue;
                }
                let Some(source) = view.evidence.as_ref() else {
                    details = details.child(
                        xs_text
                            .clone()
                            .apply(div())
                            .text_color(rgb(colors.muted_text))
                            .child("根拠の詳細情報は投影されていません。"),
                    );
                    panel = panel.child(details);
                    continue;
                };
                for (group_index, group) in source.groups.iter().enumerate() {
                    let key = group.disclosure_key();
                    let open_group = self.inspector_disclosure.is_open(Section::Evidence, &key);
                    let handle = self
                        .inspector_evidence_focus
                        .entry(Arc::clone(&key))
                        .or_insert_with(|| cx.focus_handle().tab_stop(true))
                        .clone();
                    let focused = handle.is_focused(window);
                    let (group_border_width, group_border_color) =
                        match crate::shell_style::control_focus(
                            &self.tokens,
                            focused,
                            window.scale_factor(),
                            &colors,
                            panel_surface,
                            colors.border,
                            &[],
                        ) {
                            Ok(border) => border,
                            Err(_) => (px(1.0), colors.border),
                        };
                    let status_summary = group_status_summary(group);
                    let marker = if open_group { "▾" } else { "▸" };
                    let header_text = format!(
                        "{marker} {} — 参照 {}件 · {status_summary}",
                        group.heading,
                        group.records.len()
                    );
                    let aria_label = format!(
                        "{}。根拠は{}件です。{}。展開すると投影済みの参照を表示します。",
                        group.heading,
                        group.records.len(),
                        status_summary
                    );
                    let click_key = Arc::clone(&key);
                    let focus_key = Arc::clone(&key);
                    let action_key = key;
                    details = details.child(
                        div()
                            .id(("inspector-evidence-group", group_index))
                            .role(Role::Button)
                            .key_context("EvidenceGroup")
                            .aria_label(SharedString::from(aria_label))
                            .aria_value(SharedString::from(header_text.clone()))
                            .aria_expanded(open_group)
                            .track_focus(&handle)
                            .px_2()
                            .py_1()
                            .border(group_border_width)
                            .border_color(rgb(group_border_color))
                            .cursor_pointer()
                            .child(
                                xs_text
                                    .clone()
                                    .apply(div())
                                    .child(SharedString::from(header_text)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(handle) =
                                    this.inspector_evidence_focus.get(&focus_key).cloned()
                                {
                                    handle.focus(window, cx);
                                }
                                this.toggle_evidence_group(Arc::clone(&click_key), cx);
                            }))
                            .on_action(cx.listener(move |this, _: &InspectorToggle, _, cx| {
                                this.toggle_evidence_group(Arc::clone(&action_key), cx);
                            })),
                    );
                    if !open_group {
                        continue;
                    }
                    match evidence_detail_for_group(
                        &mut self.inspector_evidence,
                        Some(source),
                        &group.evaluation_ref,
                        open_group,
                    ) {
                        Some(detail) => {
                            for (row_index, row) in detail.rows.iter().enumerate() {
                                details = details.child(
                                    div()
                                        .id((
                                            "inspector-evidence-row",
                                            group_index * 4096 + row_index,
                                        ))
                                        .role(Role::Group)
                                        .aria_label(SharedString::from(Arc::clone(&row.ax_label)))
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .child(xs_text.clone().apply(div()).child(
                                            SharedString::from(format!(
                                                "{} {} — {}",
                                                row.record_kind, row.record_id, row.state_label
                                            )),
                                        ))
                                        .child(
                                            xs_text
                                                .clone()
                                                .apply(div())
                                                .text_color(rgb(colors.muted_text))
                                                .child(SharedString::from(format!(
                                                    "概要: {}",
                                                    row.summary_text
                                                ))),
                                        )
                                        .child(
                                            xs_text
                                                .clone()
                                                .apply(div())
                                                .text_color(rgb(colors.muted_text))
                                                .child(SharedString::from(format!(
                                                    "詳細: {}",
                                                    row.detail_text
                                                ))),
                                        ),
                                );
                            }
                        }
                        None => {
                            details = details.child(
                                xs_text
                                    .clone()
                                    .apply(div())
                                    .text_color(rgb(colors.muted_text))
                                    .child("この評価の根拠は、現在の投影に含まれていません。"),
                            );
                        }
                    }
                }
                panel = panel.child(details);
                continue;
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
                                    .text_color(rgb(crate::contrast::readable_text(
                                        &colors,
                                        crate::contrast::role_color(
                                            scheme,
                                            0x8b949e,
                                            colors.muted_text,
                                        ),
                                        crate::contrast::role_color(
                                            scheme,
                                            0x202328,
                                            colors.raised_face,
                                        ),
                                    )))
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

    /// Toggle one evidence group at its exact evaluation revision.
    ///
    /// The key already contains the evaluation identity and revision, so a new
    /// revision starts collapsed and its detail is materialized only when the
    /// user opens it.
    pub(super) fn toggle_evidence_group(&mut self, key: Arc<str>, cx: &mut Context<Self>) {
        self.inspector_disclosure.toggle(Section::Evidence, &key);
        cx.notify();
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
    use super::{evidence_detail_for_group, group_status_summary, scope};
    use desktop_protocol::{
        AuthorityLabel, CandidateId, CandidateRef, Counter, EntityId, EntityKind, EntityRef,
        EvaluationDecision, EvaluationId, EvaluationSnapshot, EvidenceRef, EvidenceStatus, RunId,
        TaskId,
    };
    use ui_model::evidence::{
        DETAIL_NOT_PROJECTED, EvidenceDetailCache, EvidenceGroupSource, EvidenceSource,
    };

    fn task(name: &str) -> TaskId {
        TaskId::fixture(format!("task/{name}"))
    }

    fn candidate() -> CandidateRef {
        CandidateRef {
            task_id: TaskId::fixture("task/Task-184"),
            run_id: RunId::fixture("run/Task-184/1"),
            iteration: 3,
            candidate_id: CandidateId::fixture("candidate/a"),
            artifact_ref: desktop_protocol::ArtifactId::fixture("artifact/a"),
        }
    }

    fn record(local: &str, status: EvidenceStatus) -> EvidenceRef {
        EvidenceRef {
            record_ref: EntityRef {
                kind: EntityKind::Evidence,
                id: EntityId::fixture(local),
            },
            status,
            summary: None,
            detail: None,
        }
    }

    fn source(revision: u128, evidence: Vec<EvidenceRef>) -> EvidenceSource {
        let evaluation = EvaluationSnapshot {
            evaluation_ref: EvaluationId::fixture("evaluation/a"),
            candidate_ref: candidate(),
            artifact_refs: Vec::new(),
            decision: EvaluationDecision::Pass,
            axes: Vec::new(),
            evidence_refs: evidence,
            authority: AuthorityLabel::new("RUNTIME").expect("valid authority"),
            confidence: None,
            projection_revision: Counter::from_u128(revision),
        };
        EvidenceSource::new(
            candidate(),
            vec![EvidenceGroupSource::from_evaluation(&evaluation, 0)],
        )
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

    #[test]
    fn evidence_rows_are_materialized_only_for_an_open_group() {
        let source = source(1, vec![record("evidence/a", EvidenceStatus::Available)]);
        let evaluation = EvaluationId::fixture("evaluation/a");
        let mut cache = EvidenceDetailCache::default();
        assert!(evidence_detail_for_group(&mut cache, Some(&source), &evaluation, false).is_none());
        assert!(cache.is_empty(), "a closed group must not build a detail");
        let detail = evidence_detail_for_group(&mut cache, Some(&source), &evaluation, true)
            .expect("an open group materializes its detail");
        assert_eq!(detail.rows.len(), 1);
        assert_eq!(detail.rows[0].detail_text.as_ref(), DETAIL_NOT_PROJECTED);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_new_revision_replaces_the_detail_the_adapter_would_render() {
        let first = source(1, vec![record("evidence/a", EvidenceStatus::Available)]);
        let second = source(2, vec![record("evidence/b", EvidenceStatus::Forbidden)]);
        let evaluation = EvaluationId::fixture("evaluation/a");
        let mut cache = EvidenceDetailCache::default();
        let old = evidence_detail_for_group(&mut cache, Some(&first), &evaluation, true)
            .expect("first detail")
            .clone();
        assert_eq!(old.evaluation_revision, Counter::from_u128(1));
        assert_eq!(cache.sync(Some(&second)), 1);
        let fresh = evidence_detail_for_group(&mut cache, Some(&second), &evaluation, true)
            .expect("fresh detail");
        assert_eq!(fresh.evaluation_revision, Counter::from_u128(2));
        assert_eq!(fresh.rows[0].record_id.as_ref(), "fixture:evidence/b");
        assert_eq!(
            fresh.rows[0].state,
            ui_model::evidence::EvidenceDisplayState::Forbidden
        );
    }

    #[test]
    fn the_group_header_counts_each_status_and_never_defaults_unknown() {
        let source = source(
            1,
            vec![
                record("evidence/a", EvidenceStatus::Available),
                record("evidence/p", EvidenceStatus::Unknown("PENDING".to_owned())),
                record("evidence/f", EvidenceStatus::Forbidden),
                record("evidence/u", EvidenceStatus::Unknown("FUTURE".to_owned())),
            ],
        );
        let group = &source.groups[0];
        let summary = group_status_summary(group);
        assert!(summary.contains("取得可能 1"));
        assert!(summary.contains("準備中 1"));
        assert!(summary.contains("許可されていません 1"));
        assert!(summary.contains("不明 1"));
        assert!(!summary.contains("見つかりません"));
    }
}
