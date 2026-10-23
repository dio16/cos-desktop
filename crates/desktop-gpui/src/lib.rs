//! Native presentation only. No fixture files, transport calls, or Domain transitions.
use app_core::{AppCore, navigation::Surface};
use gpui::{
    App, Context, FocusHandle, IntoElement, KeyBinding, Render, Role, SharedString, Task, Window,
    actions, div, prelude::*, px, rgb,
};
use ui_model::{Workbench, presentation::ViewedCandidate};

actions!(
    cos_desktop,
    [
        Artifact,
        Compare,
        Flow,
        NextSurface,
        FocusNext,
        FocusPrevious,
        Activate,
        Diagnostics
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-1", Artifact, None),
        KeyBinding::new("ctrl-2", Compare, None),
        KeyBinding::new("ctrl-3", Flow, None),
        KeyBinding::new("ctrl-tab", NextSurface, None),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
        KeyBinding::new("enter", Activate, None),
        KeyBinding::new("space", Activate, None),
        KeyBinding::new("ctrl-d", Diagnostics, None),
    ]);
}

pub struct Shell {
    fixture: bool,
    workbench: Option<Workbench>,
    error: Option<String>,
    focus: FocusHandle,
    surface_focus: [FocusHandle; 3],
    _load: Task<()>,
}

impl Shell {
    pub fn new(
        fixture: bool,
        load: Task<Result<AppCore, String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let surface_focus =
            std::array::from_fn(|i| cx.focus_handle().tab_index(i as isize).tab_stop(true));
        let prepared = cx
            .background_executor()
            .spawn(async move { load.await.map(Workbench::new) });
        let task = cx.spawn(async move |this, cx| {
            let result = prepared.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(workbench) => this.workbench = Some(workbench),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        });
        Self {
            fixture,
            workbench: None,
            error: None,
            focus,
            surface_focus,
            _load: task,
        }
    }

    fn navigate(&mut self, surface: Surface, cx: &mut Context<Self>) {
        if let Some(workbench) = &mut self.workbench {
            workbench.core.navigation.navigate(surface);
            cx.notify();
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = self
            .workbench
            .as_ref()
            .map(|w| w.core.navigation.surface())
            .unwrap_or_default();
        let text = self.workbench.as_ref().map(|w| &w.text);
        let status = self.error.clone().unwrap_or_else(|| {
            text.map(|t| t.connection.clone())
                .unwrap_or_else(|| "Loading observation…".into())
        });
        let title = text
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "No Task observation".into());
        let summary = text.map(|t| t.summary.clone()).unwrap_or_default();
        let evaluation = text.map(|t| t.evaluation.clone()).unwrap_or_default();
        let graph = text.map(|t| t.graph.clone()).unwrap_or_default();
        let artifacts = text.map(|t| t.artifacts.clone()).unwrap_or_default();
        let viewed = self
            .workbench
            .as_ref()
            .and_then(|w| w.presentation.viewed())
            .map(|v| v.0.clone());
        let viewed_label = artifacts
            .iter()
            .find(|(id, _)| viewed.as_ref() == Some(id))
            .map(|(_, label)| label.as_str())
            .unwrap_or("Choose a candidate below");
        let diagnostics_open = self
            .workbench
            .as_ref()
            .is_some_and(|w| w.presentation.diagnostics_open);
        let diagnostic_text = text.map(|t| t.diagnostics.clone()).unwrap_or_default();
        let mode = if self.fixture {
            "FIXTURE / simulated data"
        } else {
            "RUNTIME / not connected"
        };
        let surface_content = match surface {
            Surface::Artifact => format!("Artifact\n{viewed_label}\n\nMedia viewer not implemented.\nSynthetic metadata only; no artifact verification claimed."),
            Surface::Compare => "Compare\nComparison renderer not implemented.\nLocal comparison is not Formal Selected.".into(),
            Surface::Flow => format!("Flow — fixture inspection\n{}\n\nGraph renderer not implemented.", graph.join(" → ")),
        };

        div()
            .id("cos-shell")
            .role(Role::Application)
            .aria_label("COS Desktop Workbench")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &Artifact, _, cx| this.navigate(Surface::Artifact, cx)),
            )
            .on_action(cx.listener(|this, _: &Compare, _, cx| this.navigate(Surface::Compare, cx)))
            .on_action(cx.listener(|this, _: &Flow, _, cx| this.navigate(Surface::Flow, cx)))
            .on_action(cx.listener(|this, _: &NextSurface, _, cx| {
                if let Some(w) = &mut this.workbench {
                    w.core.navigation.next();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|_, _: &FocusNext, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPrevious, window, cx| window.focus_prev(cx)))
            .on_action(cx.listener(|this, _: &Activate, window, cx| {
                if let Some(index) = this.surface_focus.iter().position(|f| f.is_focused(window)) {
                    this.navigate(Surface::ALL[index], cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Diagnostics, _, cx| {
                if let Some(w) = &mut this.workbench {
                    w.presentation.diagnostics_open = !w.presentation.diagnostics_open;
                    cx.notify();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x17191c))
            .text_color(rgb(0xe6e8eb))
            .text_size(px(15.0))
            .font_family("Yu Gothic UI")
            .child(
                div()
                    .id("context-bar")
                    .aria_label("Context Bar")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_5()
                    .py_3()
                    .bg(rgb(0x24272c))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(format!("COS   {title}"))
                            .child(mode),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_color(rgb(0xbcc6d0))
                            .child(summary)
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("task-rail")
                            .aria_label("Task Rail")
                            .w(px(190.0))
                            .flex_none()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .bg(rgb(0x202328))
                            .child("TASK RAIL")
                            .child("Active")
                            .child(title)
                            .child("New / History: not implemented"),
                    )
                    .child(
                        div()
                            .id("work-surface")
                            .aria_label(SharedString::from(format!(
                                "Work Surface {}",
                                surface.label()
                            )))
                            .flex_1()
                            .min_w_0()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .child("WORK SURFACE")
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(surface_content),
                            )
                            .child("Technical shell — not product UX approval"),
                    )
                    .child(
                        div()
                            .id("inspector")
                            .aria_label("Inspector")
                            .w(px(310.0))
                            .flex_none()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .bg(rgb(0x202328))
                            .child("INSPECTOR")
                            .child("Evaluation / Evidence / Refinement")
                            .children(evaluation.into_iter().map(|line| div().child(line)))
                            .child("Stop: unavailable in this shell; no completion inferred")
                            .child(if diagnostics_open {
                                diagnostic_text
                            } else {
                                "Diagnostics: Ctrl+D".into()
                            }),
                    ),
            )
            .child(
                div()
                    .id("loop-dock")
                    .aria_label("Creative Loop Dock")
                    .p_4()
                    .bg(rgb(0x272b30))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child("CREATIVE LOOP DOCK — Viewed is local; Formal Selected is Runtime-owned")
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .children(artifacts.into_iter().enumerate().map(
                                |(index, (id, label))| {
                                    let selected = viewed.as_ref() == Some(&id);
                                    div()
                                        .id(("candidate", index))
                                        .role(Role::Button)
                                        .aria_label(SharedString::from(format!(
                                            "View candidate {label}"
                                        )))
                                        .px_5()
                                        .py_2()
                                        .border_1()
                                        .border_color(rgb(if selected {
                                            0xaeccec
                                        } else {
                                            0x626870
                                        }))
                                        .cursor_pointer()
                                        .child(format!(
                                            "{label}{}",
                                            if selected { " · Viewed" } else { "" }
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(w) = &mut this.workbench {
                                                w.presentation.view(ViewedCandidate(id.clone()));
                                                cx.notify();
                                            }
                                        }))
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .id("surface-switcher")
                    .aria_label("Surface Switcher")
                    .p_3()
                    .flex()
                    .gap_3()
                    .children(Surface::ALL.into_iter().enumerate().map(|(index, item)| {
                        let focused = self.surface_focus[index].is_focused(window);
                        div()
                            .id(("surface", index))
                            .role(Role::Button)
                            .aria_label(item.label())
                            .track_focus(&self.surface_focus[index])
                            .px_5()
                            .py_2()
                            .border_2()
                            .border_color(rgb(if focused { 0xe6c278 } else { 0x343940 }))
                            .bg(rgb(if surface == item { 0x3c5268 } else { 0x24272c }))
                            .cursor_pointer()
                            .child(format!("{}  Ctrl+{}", item.label(), index + 1))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.surface_focus[index].focus(window, cx);
                                this.navigate(item, cx);
                            }))
                    })),
            )
    }
}
