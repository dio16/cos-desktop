//! Native presentation only. No fixture files, transport calls, or Domain transitions.
use app_core::{
    CommandRead, ObservationHandle, PublicationNotice, ReadUnavailable,
    navigation::Surface,
    preferences::{Preferences, WindowGeometry},
};
use desktop_protocol::{EpochId, TaskId};
use gpui::{
    App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, GlobalElementId, InspectorElementId, IntoElement, KeyBinding, LayoutId,
    ListAlignment, ListOffset, ListState, Pixels, Render, Role, ScrollHandle, SharedString, Style,
    Task, UTF16Selection, Window, actions, div, list, prelude::*, px, relative, rgb,
};
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use ui_model::task_rail::{TaskRailCache, TaskRailState};
use ui_model::{DerivationCache, DisplayCache, Workbench, presentation::CompareSide};

mod accessibility;
pub mod budget;
mod dock;
mod flow;
mod help;
mod inspector;
mod intervention;
mod palette;
#[cfg(feature = "r0-s2b-instrumentation")]
pub mod r0_probe;
mod shell_style;
mod stop;
pub mod tokens;
mod trace;
use shell_style::TextSpec;
use stop::{ReadCompletion, ReadFailure, ReceiptData, ReceiptFeedback, StopProgress, StopTracker};
use tokens::{DesignTokens, TextStyle};

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
        Diagnostics,
        InspectorToggle,
        InterventionActivate,
        InterventionDismiss,
        RailUp,
        RailDown,
        RailEnter,
        RailSpace,
        RailBackspace,
        RailDelete,
        RailLeft,
        RailRight,
        RailSelectAll,
        DockNext,
        DockPrevious,
        DockActivate,
        FlowNodeUp,
        FlowNodeDown,
        FlowEdgeUp,
        FlowEdgeDown,
        FlowEnter,
        FlowBackspace,
        FlowDelete,
        FlowLeft,
        FlowRight,
        FlowSelectAll,
        PaletteToggle,
        PaletteClose,
        PaletteEnter,
        PaletteUp,
        PaletteDown,
        PaletteNext,
        PalettePrevious,
        PaletteBackspace,
        PaletteDelete,
        PaletteLeft,
        PaletteRight,
        PaletteSelectAll
    ]
);

/// Width of the iteration gutter inside each row; the row stride is unchanged.
const DOCK_GROUP_GUTTER_PX: f32 = 56.0;

fn task_status_label(status: &desktop_protocol::TaskStatus) -> &str {
    use desktop_protocol::TaskStatus;
    match status {
        TaskStatus::Assigned => "割り当て済み（ASSIGNED）",
        TaskStatus::Active => "実行中（ACTIVE）",
        TaskStatus::Completed => "完了（COMPLETED）",
        TaskStatus::Cancelled => "中止（CANCELLED）",
        TaskStatus::Unknown(raw) => raw,
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-1", Artifact, None),
        KeyBinding::new("ctrl-2", Compare, None),
        KeyBinding::new("ctrl-3", Flow, None),
        KeyBinding::new("ctrl-tab", NextSurface, None),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
        KeyBinding::new("enter", Activate, Some("!TaskRailInput && !TaskRailList && !FlowSearchInput && !FlowNodeList && !FlowEdgeList")),
        KeyBinding::new("space", Activate, Some("!TaskRailInput && !TaskRailList && !FlowSearchInput && !FlowNodeList && !FlowEdgeList")),
        KeyBinding::new("ctrl-d", Diagnostics, None),
        KeyBinding::new("enter", InspectorToggle, Some("InspectorSection")),
        KeyBinding::new("space", InspectorToggle, Some("InspectorSection")),
        KeyBinding::new("enter", InterventionActivate, Some("InterventionStrip")),
        KeyBinding::new("space", InterventionActivate, Some("InterventionStrip")),
        KeyBinding::new("escape", InterventionDismiss, Some("InterventionStrip")),
        KeyBinding::new("up", RailUp, Some("TaskRailList")),
        KeyBinding::new("down", RailDown, Some("TaskRailList")),
        KeyBinding::new("enter", RailEnter, Some("TaskRailList")),
        KeyBinding::new("space", RailSpace, Some("TaskRailList")),
        KeyBinding::new("backspace", RailBackspace, Some("TaskRailInput")),
        KeyBinding::new("delete", RailDelete, Some("TaskRailInput")),
        KeyBinding::new("left", RailLeft, Some("TaskRailInput")),
        KeyBinding::new("right", RailRight, Some("TaskRailInput")),
        KeyBinding::new("ctrl-a", RailSelectAll, Some("TaskRailInput")),
        KeyBinding::new("right", DockNext, Some("CandidateDock")),
        KeyBinding::new("left", DockPrevious, Some("CandidateDock")),
        KeyBinding::new("enter", DockActivate, Some("CandidateDock")),
        KeyBinding::new("space", DockActivate, Some("CandidateDock")),
        KeyBinding::new("up", FlowNodeUp, Some("FlowNodeList")),
        KeyBinding::new("down", FlowNodeDown, Some("FlowNodeList")),
        KeyBinding::new("up", FlowEdgeUp, Some("FlowEdgeList")),
        KeyBinding::new("down", FlowEdgeDown, Some("FlowEdgeList")),
        KeyBinding::new("enter", FlowEnter, Some("FlowSearchInput")),
        KeyBinding::new("backspace", FlowBackspace, Some("FlowSearchInput")),
        KeyBinding::new("delete", FlowDelete, Some("FlowSearchInput")),
        KeyBinding::new("left", FlowLeft, Some("FlowSearchInput")),
        KeyBinding::new("right", FlowRight, Some("FlowSearchInput")),
        KeyBinding::new("ctrl-a", FlowSelectAll, Some("FlowSearchInput")),
        KeyBinding::new("ctrl-k", PaletteToggle, None),
        KeyBinding::new("escape", PaletteClose, Some("CommandPalette")),
        KeyBinding::new("enter", PaletteEnter, Some("CommandPalette")),
        KeyBinding::new("up", PaletteUp, Some("CommandPalette")),
        KeyBinding::new("down", PaletteDown, Some("CommandPalette")),
        KeyBinding::new("tab", PaletteNext, Some("CommandPalette")),
        KeyBinding::new("shift-tab", PalettePrevious, Some("CommandPalette")),
        KeyBinding::new("backspace", PaletteBackspace, Some("CommandPalette")),
        KeyBinding::new("delete", PaletteDelete, Some("CommandPalette")),
        KeyBinding::new("left", PaletteLeft, Some("CommandPalette")),
        KeyBinding::new("right", PaletteRight, Some("CommandPalette")),
        KeyBinding::new("ctrl-a", PaletteSelectAll, Some("CommandPalette")),
    ]);
}

/// One waiter and one coalesced request generation; no idle timer or job queue.
struct RailWorkSignal {
    generation: AtomicUsize,
    waiter: Mutex<Option<Waker>>,
}

impl RailWorkSignal {
    fn new() -> Self {
        Self {
            generation: AtomicUsize::new(1),
            waiter: Mutex::new(None),
        }
    }

    fn next_generation(&self) -> usize {
        self.generation.load(Ordering::Acquire).saturating_add(1)
    }

    fn request(&self) {
        let _ = self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            });
        let waiter = self.waiter.lock().ok().and_then(|mut waiter| waiter.take());
        if let Some(waiter) = waiter {
            waiter.wake();
        }
    }

    async fn changed(&self, completed: usize, handle: &ObservationHandle) -> bool {
        std::future::poll_fn(|cx| {
            let Ok(mut waiter) = self.waiter.lock() else {
                return Poll::Ready(false);
            };
            if self.generation.load(Ordering::Acquire) != completed || handle.is_finished() {
                *waiter = None;
                return Poll::Ready(!handle.is_finished());
            }
            if !waiter
                .as_ref()
                .is_some_and(|waiter| waiter.will_wake(cx.waker()))
            {
                *waiter = Some(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }
}

pub struct Shell {
    #[cfg(feature = "r0-s2b-instrumentation")]
    probe_window: u64,
    handle: Option<ObservationHandle>,
    workbench: Option<Workbench>,
    /// Derived display text with its publication identity, revalidated on render.
    cache: DisplayCache,
    error: Option<String>,
    focus: FocusHandle,
    surface_focus: [FocusHandle; 3],
    stop_focus: FocusHandle,
    reconcile_focus: FocusHandle,
    /// Command palette state (DC-MP-NAV5): the overlay, its bounded query, the
    /// active result row and the focus handle restored on dismiss.
    palette_open: bool,
    palette_focus: FocusHandle,
    palette_restore_focus: Option<FocusHandle>,
    palette_query: String,
    palette_input_selection: Range<usize>,
    palette_marked_range: Option<Range<usize>>,
    palette_active: Option<usize>,
    palette_hit_len: usize,
    stop_intent: StopTracker,
    /// One dock-level focus handle with an active-descendant row: focus identity
    /// stays stable while the mounted window moves, and no per-candidate handle
    /// is allocated for a collection that is only partly visible.
    dock_focus: FocusHandle,
    /// The dock's keyboard cursor, kept by scoped identity rather than position.
    dock_active: Option<desktop_protocol::CandidateRef>,
    /// Local visible anchor and fractional pixel offset; survives withheld display
    /// placeholders, but never authorizes rendering or acting on cached rows.
    dock_anchor: Option<(desktop_protocol::CandidateRef, Pixels)>,
    dock_mounts: Arc<AtomicUsize>,
    dock_frame: usize,
    dock_trace_lines: Arc<AtomicUsize>,
    dock_trace_enabled: bool,
    candidate_scroll: ScrollHandle,
    /// Flow's search and non-canvas inspection state. The cursor is kept by
    /// exact graph revision plus NodeId/EdgeId, never by list position.
    flow_input_focus: FocusHandle,
    flow_node_focus: FocusHandle,
    flow_edge_focus: FocusHandle,
    flow_node_list: ListState,
    flow_edge_list: ListState,
    flow_query: String,
    flow_input_selection: Range<usize>,
    flow_marked_range: Option<Range<usize>>,
    flow_node_active: Option<flow::NodeKey>,
    flow_edge_active: Option<flow::EdgeKey>,
    flow_revision_key: Option<flow::RevisionKey>,
    /// Only fully derived rows whose complete admission identity still matches
    /// may be rendered or inspected.
    flow_admitted: Option<(flow::AdmissionKey, Arc<flow::Rows>)>,
    flow_pending: Option<flow::AdmissionKey>,
    flow_request_generation: Arc<RailWorkSignal>,
    flow_jobs: Arc<Mutex<Option<flow::Job>>>,
    _flow_worker: Option<Task<()>>,
    preferences: Arc<Mutex<Preferences>>,
    /// The single resolved typography/geometry token set injected by the
    /// composition root. No surface constructs or re-derives a set of its own.
    tokens: DesignTokens,
    pending_restore_task: Option<TaskId>,
    pending_restore_viewed: Option<desktop_protocol::CandidateRef>,
    pending_restore_compare: Option<[desktop_protocol::CandidateRef; 2]>,
    restore_presentation_done: bool,
    restore_notice: Option<String>,
    /// A closed, user-facing reason for a refused surface action (ADR-0010 D-2).
    surface_notice: Option<String>,
    /// Inspector disclosure scoped to the viewed identity and section revision.
    inspector_disclosure: ui_model::inspector::Disclosure,
    /// One focus handle per inspector section, in `Section::ALL` order.
    inspector_focus: Vec<FocusHandle>,
    /// Expansion, open details and the dismissal origin of the intervention strip; reconciled
    /// against the active rows so a resolved condition never leaves local state behind.
    strip_selection: intervention::StripSelection,
    /// Per-row focus handles keyed by typed row identity, pruned with the active row set.
    strip_focus: BTreeMap<ui_model::intervention::RowIdentity, FocusHandle>,
    /// Focus handle for the remaining-count/list toggle.
    strip_list_focus: FocusHandle,
    /// Focus handle for the details panel's close control.
    strip_detail_focus: FocusHandle,
    /// The last rendered immutable candidate family; reused without cloning identities.
    rendered_candidates: Arc<[Arc<ui_model::CandidateDisplay>]>,
    rail_state: TaskRailState,
    rail_list: ListState,
    rail_focus: FocusHandle,
    rail_input_focus: FocusHandle,
    rail_input_selection: Range<usize>,
    rail_marked_range: Option<Range<usize>>,
    rail_active: Option<TaskId>,
    rail_active_index: Option<usize>,
    rail_identity_scope: Option<(u64, Option<EpochId>)>,
    rail_selection_pending: bool,
    rail_trace_lines: Arc<AtomicUsize>,
    rail_anchor: Option<(TaskId, Pixels)>,
    rail_mounts: Arc<AtomicUsize>,
    rail_frame: usize,
    rail_trace_enabled: bool,
    rail_request_generation: Arc<RailWorkSignal>,
    _rail_worker: Option<Task<()>>,
    _subscription: Task<()>,
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.rail_request_generation.request();
        self.flow_request_generation.request();
        if let (Some(handle), Some(command)) = (&self.handle, self.stop_intent.release()) {
            handle.release_command(command);
        }
    }
}

impl Shell {
    pub fn new(
        mut core: app_core::AppCore,
        handle: Option<ObservationHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
        preferences: Arc<Mutex<Preferences>>,
        tokens: DesignTokens,
    ) -> Self {
        let restored = preferences
            .lock()
            .map(|preferences| preferences.clone())
            .unwrap_or_default();
        core.navigation.navigate(restored.surface);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let surface_focus =
            std::array::from_fn(|i| cx.focus_handle().tab_index(i as isize).tab_stop(true));
        let inspector_focus = (0..ui_model::inspector::Section::ALL.len())
            .map(|index| {
                cx.focus_handle()
                    .tab_index(3 + index as isize)
                    .tab_stop(true)
            })
            .collect::<Vec<_>>();
        let stop_focus = cx.focus_handle().tab_stop(true);

        let reconcile_focus = cx.focus_handle().tab_stop(true);
        let rail_input_focus = cx.focus_handle().tab_stop(true);
        let rail_focus = cx.focus_handle().tab_stop(true);
        let rail_list = ListState::new(0, ListAlignment::Top, px(120.0));
        let flow_input_focus = cx.focus_handle().tab_stop(true);
        let flow_node_focus = cx.focus_handle().tab_stop(true);
        let flow_edge_focus = cx.focus_handle().tab_stop(true);
        let flow_node_list = ListState::new(0, ListAlignment::Top, px(72.0));
        let flow_edge_list = ListState::new(0, ListAlignment::Top, px(72.0));
        let flow_request_generation = Arc::new(RailWorkSignal::new());
        let flow_jobs = Arc::new(Mutex::new(None));
        let rail_request_generation = Arc::new(RailWorkSignal::new());
        let workbench = Some(Workbench {
            core,
            presentation: ui_model::presentation::Presentation::default(),
        });
        let subscription = match handle.clone() {
            Some(handle) => Self::subscribe(handle, cx),
            None => cx.spawn(async move |_this, _cx| {}),
        };
        let rail_worker = handle.clone().map(|handle| {
            Self::start_rail_worker(handle, Arc::clone(&rail_request_generation), cx)
        });
        let flow_worker = handle.clone().map(|handle| {
            Self::start_flow_worker(
                handle,
                Arc::clone(&flow_request_generation),
                Arc::clone(&flow_jobs),
                cx,
            )
        });
        Self {
            #[cfg(feature = "r0-s2b-instrumentation")]
            probe_window: window.window_handle().window_id().as_u64(),
            handle,
            workbench,
            cache: DisplayCache::default(),
            error: None,
            focus,
            surface_focus,
            stop_focus,
            reconcile_focus,
            palette_open: false,
            palette_focus: cx.focus_handle().tab_stop(true),
            palette_restore_focus: None,
            palette_query: String::new(),
            palette_input_selection: 0..0,
            palette_marked_range: None,
            palette_active: None,
            palette_hit_len: 0,
            stop_intent: StopTracker::default(),
            dock_focus: cx.focus_handle().tab_stop(true),
            dock_active: None,
            dock_anchor: None,
            dock_mounts: Arc::new(AtomicUsize::new(0)),
            dock_frame: 0,
            dock_trace_lines: Arc::new(AtomicUsize::new(0)),
            dock_trace_enabled: cfg!(debug_assertions)
                && std::env::var_os("COS_FIXTURE_TRACE").is_some(),
            candidate_scroll: ScrollHandle::new(),
            flow_input_focus,
            flow_node_focus,
            flow_edge_focus,
            flow_node_list,
            flow_edge_list,
            flow_query: String::new(),
            flow_input_selection: 0..0,
            flow_marked_range: None,
            flow_node_active: None,
            flow_edge_active: None,
            flow_revision_key: None,
            flow_admitted: None,
            flow_pending: None,
            flow_request_generation,
            flow_jobs,
            _flow_worker: flow_worker,
            preferences,
            tokens,
            pending_restore_task: restored.last_task_id,
            pending_restore_viewed: restored.viewed,
            pending_restore_compare: restored.compare,
            restore_presentation_done: false,
            restore_notice: None,
            surface_notice: None,
            inspector_disclosure: ui_model::inspector::Disclosure::default(),
            inspector_focus,
            strip_selection: intervention::StripSelection::default(),
            strip_focus: BTreeMap::new(),
            strip_list_focus: cx.focus_handle().tab_stop(true),
            strip_detail_focus: cx.focus_handle().tab_stop(true),
            rail_state: TaskRailState::default(),
            rail_list,
            rail_focus,
            rail_input_focus,
            rail_input_selection: 0..0,
            rail_marked_range: None,
            rail_selection_pending: false,
            rail_active: None,
            rail_active_index: None,
            rail_identity_scope: None,
            rail_trace_lines: Arc::new(AtomicUsize::new(0)),
            rail_anchor: None,
            rail_mounts: Arc::new(AtomicUsize::new(0)),
            rail_frame: 0,
            rail_trace_enabled: cfg!(debug_assertions)
                && std::env::var_os("COS_FIXTURE_TRACE").is_some(),
            rail_request_generation: Arc::clone(&rail_request_generation),
            rendered_candidates: Arc::default(),
            _rail_worker: rail_worker,
            _subscription: subscription,
        }
    }

    /// Subscribe to complete publications and install revalidated display text.
    ///
    /// Each derivation job temporarily owns the mutable cache. Its cancellation
    /// predicate observes the exact lease notice, so a closed fence or superseding
    /// publication cannot publish background work into this consumer.
    fn subscribe(handle: ObservationHandle, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut subscription = handle.subscribe();
            loop {
                let Some(notice) = subscription.changed().await else {
                    let finished = handle.is_finished();
                    let _ = this.update(cx, |this, cx| {
                        this.cache.invalidate();
                        this.rail_state.revalidate(None);
                        this.request_rail_work(cx);
                        cx.notify();
                    });
                    if finished {
                        break;
                    }
                    continue;
                };
                if Self::current_notice(&handle) != Some(notice) {
                    continue;
                }
                let _ = this.update(cx, |this, cx| {
                    this.cache.revalidate(Some(notice));
                    this.rail_state.revalidate(Some(notice));
                    this.request_rail_work(cx);
                    cx.notify();
                });
            }
        })
    }

    /// Serial, event-driven derivation/search; requests coalesce while work runs.
    fn start_rail_worker(
        handle: ObservationHandle,
        requests: Arc<RailWorkSignal>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut rail_cache = TaskRailCache::default();
            let mut display_cache = DerivationCache::default();
            let mut completed = 0usize;
            loop {
                if !requests.changed(completed, &handle).await {
                    break;
                }
                let generation = requests.generation.load(Ordering::Acquire);
                let request = this
                    .update(cx, |this, _| {
                        let notice = Self::current_notice(&handle)?;
                        let lease = handle.current().ok()?;
                        let mut publication = None;
                        lease
                            .read(|current| publication = Some(current.clone()))
                            .ok()?;
                        Some((
                            notice,
                            publication?,
                            this.rail_state.query().to_owned(),
                            this.rail_state.query_generation(),
                            this.rail_state.selected().cloned(),
                        ))
                    })
                    .ok()
                    .flatten();
                let Some((notice, publication, query, query_generation, selected)) = request else {
                    rail_cache.reset();
                    display_cache.reset();
                    completed = generation;
                    continue;
                };
                let worker_handle = handle.clone();
                let worker_requests = requests.clone();
                let (returned_rail_cache, returned_display_cache, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let cancelled = || {
                            worker_requests.generation.load(Ordering::Acquire) != generation
                                || worker_handle
                                    .current()
                                    .ok()
                                    .and_then(|lease| lease.notice())
                                    != Some(notice)
                                || worker_handle.is_finished()
                        };
                        let display =
                            display_cache.derive(&publication, selected.as_ref(), cancelled);
                        let rail =
                            rail_cache
                                .derive(&publication, cancelled)
                                .and_then(|collection| {
                                    collection
                                        .search(&query, cancelled)
                                        .map(|search| (collection, search))
                                });
                        let result = display.zip(rail).map(|result| (selected.clone(), result));
                        let still_current = worker_handle
                            .current()
                            .ok()
                            .and_then(|lease| lease.notice())
                            == Some(notice)
                            && !worker_handle.is_finished();
                        if !still_current {
                            rail_cache.reset();
                            display_cache.reset();
                        }
                        (rail_cache, display_cache, result)
                    })
                    .await;
                rail_cache = returned_rail_cache;
                display_cache = returned_display_cache;
                completed = generation;
                if requests.generation.load(Ordering::Acquire) != generation {
                    continue;
                }
                let Some((requested_selection, (derived, (collection, search)))) = result else {
                    continue;
                };
                let live = Self::current_notice(&handle);
                let identity = (notice.session_generation, notice.publication_version);
                if this
                    .update(cx, |this, cx| {
                        let scope = (notice.session_generation, collection.epoch.clone());
                        let scope_changed = this.rail_identity_scope.as_ref() != Some(&scope);
                        if scope_changed {
                            this.dock_active = None;
                            this.dock_anchor = None;
                            this.rendered_candidates = Arc::default();
                            this.candidate_scroll.set_offset(Default::default());
                            this.flow_node_active = None;
                            this.flow_edge_active = None;
                            this.flow_revision_key = None;
                            this.flow_admitted = None;
                            this.flow_pending = None;
                            if let Ok(mut job) = this.flow_jobs.lock() {
                                *job = None;
                            }
                            this.flow_node_list.reset(0);
                            this.flow_edge_list.reset(0);
                            this.rail_active = None;
                            this.rail_active_index = None;
                            this.rail_anchor = None;
                        }
                        this.rail_identity_scope = Some(scope);
                        let previous_scroll = if scope_changed {
                            ListOffset {
                                item_ix: 0,
                                offset_in_item: px(0.0),
                            }
                        } else {
                            this.rail_list.logical_scroll_top()
                        };
                        let previous_anchor = this.rail_anchor.clone();
                        let rail_installed = this.rail_state.install(
                            notice,
                            query_generation,
                            collection,
                            search,
                            live,
                        );
                        if rail_installed {
                            this.restore_pending_task(live);
                        }
                        if rail_installed
                            && this.rail_state.selected() != requested_selection.as_ref()
                        {
                            this.rail_selection_pending = true;
                            this.request_rail_work(cx);
                            cx.notify();
                            return;
                        }
                        if !rail_installed || !this.cache.install(identity, derived, live) {
                            return;
                        }
                        this.error = None;
                        this.rail_selection_pending = false;
                        let candidates = Arc::clone(&this.cache.text().artifacts);
                        let task = this.cache.text().task.clone();
                        this.apply_restored_presentation(&candidates, task.as_ref(), cx);
                        this.synchronize_stop_context();
                        let count = this
                            .rail_state
                            .results()
                            .map_or(0, |result| result.indices.len());
                        this.rail_list.reset(count);
                        if count > 0 {
                            let target = previous_anchor
                                .as_ref()
                                .and_then(|(identity, _)| {
                                    let rows = &this.rail_state.collection()?.rows;
                                    let indices = &this.rail_state.results()?.indices;
                                    indices.iter().position(|row_index| {
                                        rows[*row_index].identity == *identity
                                    })
                                })
                                .unwrap_or_else(|| previous_scroll.item_ix.min(count - 1));
                            let offset = previous_anchor
                                .as_ref()
                                .and_then(|(identity, offset)| {
                                    this.rail_state
                                        .collection()?
                                        .rows
                                        .iter()
                                        .any(|row| &row.identity == identity)
                                        .then_some(*offset)
                                })
                                .unwrap_or(px(0.0));
                            this.rail_list.scroll_to(ListOffset {
                                item_ix: target,
                                offset_in_item: offset,
                            });
                        }
                        let candidate = this
                            .rail_active
                            .clone()
                            .or_else(|| this.rail_state.selected().cloned())
                            .or_else(|| this.cache.text().task.clone());
                        let active = candidate.and_then(|task| {
                            let collection = this.rail_state.collection()?;
                            let search = this.rail_state.results()?;
                            let index = search
                                .indices
                                .iter()
                                .position(|index| collection.rows[*index].identity == task)?;
                            Some((task, index))
                        });
                        this.rail_active = None;
                        this.rail_active_index = None;
                        if let Some((task, index)) = active {
                            this.set_rail_active(task, index);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    fn request_rail_work(&mut self, cx: &mut Context<Self>) {
        self.rail_request_generation.request();
        cx.notify();
    }

    /// Serial, cancellable Flow filtering. A newer query/revision increments the
    /// generation and the active scan stops at its next check; partial rows are
    /// never returned.
    fn start_flow_worker(
        handle: ObservationHandle,
        requests: Arc<RailWorkSignal>,
        jobs: Arc<Mutex<Option<flow::Job>>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut completed = 0usize;
            loop {
                if !requests.changed(completed, &handle).await {
                    break;
                }
                let generation = requests.generation.load(Ordering::Acquire);
                let job = jobs.lock().ok().and_then(|mut jobs| jobs.take());
                let Some(job) = job else {
                    completed = generation;
                    continue;
                };
                let cancellation = Arc::clone(&requests);
                let job_generation = job.generation;
                let graphs = Arc::clone(&job.graphs);
                let query = job.key.query.clone();
                let rows = cx
                    .background_executor()
                    .spawn(async move {
                        flow::rows_cancellable(&graphs, &query, || {
                            cancellation.generation.load(Ordering::Acquire) != job_generation
                        })
                    })
                    .await;
                completed = generation;
                let Some(rows) = rows else {
                    continue;
                };
                if requests.generation.load(Ordering::Acquire) != job.generation {
                    continue;
                }
                if this
                    .update(cx, |this, cx| this.install_flow_rows(job, rows, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    fn flow_admission_key(&self) -> Option<flow::AdmissionKey> {
        if !self.flow_is_current() {
            return None;
        }
        let text = self.cache.text();
        Some(flow::AdmissionKey {
            session_generation: text.session_generation,
            publication_version: text.publication_version,
            epoch: text.epoch.clone(),
            revision: self.flow_revision_key.clone()?,
            query: self.flow_query.clone(),
        })
    }

    fn flow_admitted_rows(&self) -> Option<Arc<flow::Rows>> {
        let key = self.flow_admission_key()?;
        self.flow_admitted
            .as_ref()
            .filter(|(admitted, _)| *admitted == key)
            .map(|(_, rows)| Arc::clone(rows))
    }

    fn install_flow_rows(&mut self, job: flow::Job, rows: flow::Rows, cx: &mut Context<Self>) {
        let current = self.flow_admission_key();
        if !flow::admit(&job.key, self.flow_pending.as_ref(), current.as_ref()) {
            if self.flow_pending.as_ref() == Some(&job.key) {
                self.flow_pending = None;
            }
            cx.notify();
            return;
        }
        self.flow_pending = None;
        let rows = Arc::new(rows);
        self.flow_node_list.reset(rows.nodes.len());
        self.flow_edge_list.reset(rows.edges.len());
        self.flow_admitted = Some((job.key, rows));
        if let Some((_, rows)) = self.flow_admitted.as_ref() {
            if self
                .flow_node_active
                .as_ref()
                .is_some_and(|key| !flow::contains_node(rows, key))
            {
                self.flow_node_active = None;
            }
            if self
                .flow_edge_active
                .as_ref()
                .is_some_and(|key| !flow::contains_edge(rows, key))
            {
                self.flow_edge_active = None;
            }
        }
        cx.notify();
    }

    fn current_notice(handle: &ObservationHandle) -> Option<PublicationNotice> {
        handle.current().ok().and_then(|lease| lease.notice())
    }

    fn read_failure_of(error: ReadUnavailable) -> ReadFailure {
        if error == ReadUnavailable::Superseded {
            ReadFailure::Superseded
        } else {
            ReadFailure::Unavailable
        }
    }

    fn synchronize_stop_context(&mut self) {
        let Some(handle) = self.handle.as_ref() else {
            return;
        };
        if !self.cache.is_authorized(Self::current_notice(handle)) {
            return;
        }
        let text = self.cache.text();
        let released = match text.task.as_ref() {
            Some(task) => self.stop_intent.synchronize(task, text.session_generation),
            None => self.stop_intent.release(),
        };
        if let Some(command) = released {
            handle.release_command(command);
        }
    }

    /// The current authorized display text, when this window holds one.
    fn authorized_text(&self) -> Option<&ui_model::WorkbenchText> {
        let handle = self.handle.as_ref()?;
        self.cache
            .is_authorized(Self::current_notice(handle))
            .then(|| self.cache.text())
    }

    fn stop_retryable_admission(&self) -> bool {
        self.stop_intent
            .active
            .as_ref()
            .is_some_and(|active| active.retryable_admission && active.command_id.is_none())
    }

    /// Stop and receipt availability. The palette and the visible controls read
    /// this one predicate set, so neither can drift from the other.
    fn action_availability(
        &self,
        authorized_text: bool,
        text: &ui_model::WorkbenchText,
    ) -> (palette::ActionFacts, palette::ActionFacts) {
        let retryable_admission = self.stop_retryable_admission();
        let stop_enabled = authorized_text
            && !self.rail_selection_pending
            && text.stop_enabled
            && self.stop_intent.can_begin();
        let stop_reason: Arc<str> = if !authorized_text {
            "閲覧が認可された最新の情報を受信するまで、停止は操作できません。".into()
        } else if self.rail_selection_pending {
            "選択したタスクの表示を更新中のため、停止は操作できません。".into()
        } else if !text.stop_enabled {
            Arc::clone(&text.stop_reason)
        } else if retryable_admission {
            "前の停止要求は受付前に容量制限で拒否されました。もう一度押すと新しい要求を送ります。前の要求は再送しません。"
                .into()
        } else if self.stop_intent.active.is_some() {
            "この画面からの停止要求は記録済みです。自動で再送したり別の要求に置き換えたりしません。"
                .into()
        } else if !self.stop_intent.can_begin() {
            "この画面で停止要求を識別できる上限に達したため、新しい要求は送れません。".into()
        } else {
            Arc::clone(&text.stop_reason)
        };
        let local_stop = self.stop_intent.active.as_ref().filter(|active| {
            stop_scope_matches(active, authorized_text, self.rail_selection_pending, text)
        });
        let feedback = local_stop
            .map(|active| active.receipt_feedback())
            .unwrap_or(ReceiptFeedback::Idle);
        let reconcile_enabled =
            authorized_text && local_stop.is_some() && self.stop_intent.can_read();
        let reconcile_reason: Arc<str> = if !authorized_text {
            "閲覧が認可された最新の情報を受信するまで、受付状況は確認できません。".into()
        } else if feedback == ReceiptFeedback::Checking {
            "停止要求の受付状況を確認中です。".into()
        } else if local_stop.is_none() && self.stop_intent.active.is_some() {
            "表示中のタスクと停止要求の対象が一致しないため、受付状況は確認できません。".into()
        } else if local_stop.is_none_or(|active| active.command_id.is_none()) {
            "受付状況を確認できる停止要求はありません。".into()
        } else {
            "停止要求の受付状況を確認中です。".into()
        };
        (
            palette::ActionFacts {
                enabled: stop_enabled,
                reason: stop_reason,
            },
            palette::ActionFacts {
                enabled: reconcile_enabled,
                reason: reconcile_reason,
            },
        )
    }

    fn start_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        if self.rail_selection_pending {
            return;
        }
        let notice = Self::current_notice(&handle);
        self.cache.revalidate(notice);
        if !self.cache.is_authorized(notice) {
            return;
        }
        self.synchronize_stop_context();
        let text = self.cache.text();
        if !text.stop_enabled {
            return;
        }
        let Some(task) = text.task.clone() else {
            return;
        };
        let Some(intent) = self
            .stop_intent
            .begin(task.clone(), text.session_generation)
        else {
            return;
        };
        self.stop_focus.focus(window, cx);
        cx.notify();

        let request = handle.request_stop(task);
        cx.spawn(async move |this, cx| match request.await {
            Ok(command_id) => {
                let installed = this.update(cx, |this, cx| {
                    this.synchronize_stop_context();
                    let installed = this.stop_intent.admitted(intent, command_id.clone());
                    if installed {
                        cx.notify();
                    }
                    installed
                });
                if !installed.is_ok_and(|installed| installed) {
                    return;
                }
                let retain_result = handle.retain_command(command_id.clone()).await;
                let retained = retain_result.is_ok();
                let installed = this.update(cx, |this, cx| {
                    this.synchronize_stop_context();
                    let installed = this
                        .stop_intent
                        .retained(intent, &command_id, &retain_result);
                    if installed {
                        cx.notify();
                    }
                    installed
                });
                if retained && !installed.is_ok_and(|installed| installed) {
                    handle.release_command(command_id);
                }
            }
            Err(error) => {
                let _ = this.update(cx, |this, cx| {
                    this.synchronize_stop_context();
                    this.stop_intent.refused(intent, &error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn start_receipt_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        if self.rail_selection_pending {
            return;
        }
        let notice = Self::current_notice(&handle);
        self.cache.revalidate(notice);
        if !self.cache.is_authorized(notice) {
            return;
        }
        self.synchronize_stop_context();
        let text = self.cache.text();
        if !self.stop_intent.active.as_ref().is_some_and(|active| {
            active.session_generation == text.session_generation
                && text.task.as_ref() == Some(&active.task)
        }) {
            return;
        }
        let Some((intent, attempt, command_id)) = self.stop_intent.begin_read() else {
            return;
        };
        self.reconcile_focus.focus(window, cx);
        cx.notify();

        let read = handle.read_command(command_id);
        cx.spawn(async move |this, cx| {
            let completion = match read.await {
                Ok(authorized) => match authorized.read(|answer| match answer {
                    CommandRead::Found(_) => ReceiptData::Found,
                    CommandRead::NotRetained => ReceiptData::NotRetained,
                }) {
                    Ok(tagged) => ReadCompletion::Data(tagged),
                    Err(error) => ReadCompletion::Failed(Self::read_failure_of(error)),
                },
                Err(error) => ReadCompletion::Failed(ReadFailure::from_error(&error)),
            };
            let _ = this.update(cx, |this, cx| {
                this.synchronize_stop_context();
                if this.stop_intent.read_finished(intent, attempt, completion) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn navigate(&mut self, surface: Surface, cx: &mut Context<Self>) {
        let candidate_count = self.rendered_candidates.len();
        let Some(workbench) = &mut self.workbench else {
            return;
        };
        #[cfg(feature = "r0-s2b-instrumentation")]
        let changed = workbench.core.navigation.surface() != surface;
        match dock::admit(surface, candidate_count, &mut workbench.core.navigation) {
            Ok(()) => {
                self.surface_notice = None;
                #[cfg(feature = "r0-s2b-instrumentation")]
                if changed {
                    r0_probe::record(
                        self.probe_window,
                        r0_probe::EventKind::SurfaceChanged(surface),
                    );
                }
                cx.notify();
                #[cfg(feature = "r0-s2b-instrumentation")]
                r0_probe::record(self.probe_window, r0_probe::EventKind::Notify);
            }
            Err(notice) => {
                self.surface_notice = Some(notice);
                cx.notify();
            }
        }
    }

    /// The Ctrl+Tab (`NextSurface`) action's dispatch path.
    ///
    /// The target comes from the canonical cycle and goes through the same Compare
    /// admission as `navigate()`: Compare is never entered while it is unavailable, and
    /// the reason is identical to the dock/switcher (ADR-0010 D-2).
    fn advance_surface(&mut self, cx: &mut Context<Self>) {
        let candidate_count = self.rendered_candidates.len();
        let Some(workbench) = &mut self.workbench else {
            return;
        };
        let event = dock::advance(&mut workbench.core.navigation, candidate_count);
        self.surface_notice = event.notice;
        #[cfg(feature = "r0-s2b-instrumentation")]
        if let Some(surface) = event.surface {
            r0_probe::record(
                self.probe_window,
                r0_probe::EventKind::SurfaceChanged(surface),
            );
        }
        cx.notify();
    }

    fn set_rail_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.rail_state.set_query(query) {
            self.rail_input_selection = query.len()..query.len();
            self.rail_marked_range = None;
            self.request_rail_work(cx);
        }
    }

    fn replace_rail_range(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let current = self.rail_state.query().to_owned();
        let mut replacement =
            String::with_capacity(current.len() - (range.end - range.start) + text.len());
        replacement.push_str(&current[..range.start]);
        replacement.push_str(text);
        replacement.push_str(&current[range.end..]);
        let cursor = range.start + text.len();
        self.set_rail_query(&replacement, cx);
        self.rail_input_selection = cursor..cursor;
    }

    fn rail_backspace(&mut self, cx: &mut Context<Self>) {
        let query = self.rail_state.query();
        let range = if !self.rail_input_selection.is_empty() {
            self.rail_input_selection.clone()
        } else {
            let cursor = self.rail_input_selection.start;
            query[..cursor]
                .char_indices()
                .next_back()
                .map_or(cursor..cursor, |(index, ch)| index..index + ch.len_utf8())
        };
        if !range.is_empty() {
            self.replace_rail_range(range, "", cx);
        }
    }

    fn rail_delete(&mut self, cx: &mut Context<Self>) {
        let query = self.rail_state.query();
        let range = if !self.rail_input_selection.is_empty() {
            self.rail_input_selection.clone()
        } else {
            let cursor = self.rail_input_selection.end;
            query[cursor..]
                .char_indices()
                .next()
                .map_or(cursor..cursor, |(_, ch)| cursor..cursor + ch.len_utf8())
        };
        if !range.is_empty() {
            self.replace_rail_range(range, "", cx);
        }
    }

    fn rail_move_cursor(&mut self, direction: isize, cx: &mut Context<Self>) {
        let query = self.rail_state.query();
        let cursor = if direction < 0 {
            query[..self.rail_input_selection.start]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index)
        } else {
            let current = self.rail_input_selection.end;
            query[current..]
                .char_indices()
                .next()
                .map_or(query.len(), |(offset, ch)| current + offset + ch.len_utf8())
        };
        self.rail_input_selection = cursor..cursor;
        cx.notify();
    }

    /// Keep keyboard focus on the list, with one active-descendant row. Associate
    /// that container handle with only the active item for GPUI's offscreen retention.
    fn set_rail_active(&mut self, task: TaskId, index: usize) {
        let scroll = self.rail_list.logical_scroll_top();
        if let Some(previous) = self.rail_active_index
            && previous != index
            && previous < self.rail_list.item_count()
        {
            self.rail_list
                .splice_focusable(previous..previous + 1, [None]);
        }
        if self.rail_active_index != Some(index) {
            self.rail_list
                .splice_focusable(index..index + 1, [Some(self.rail_focus.clone())]);
            self.rail_list.scroll_to(scroll);
        }
        self.rail_active = Some(task);
        self.rail_active_index = Some(index);
    }

    fn restore_pending_task(&mut self, live: Option<PublicationNotice>) {
        let Some(task) = self.pending_restore_task.clone() else {
            return;
        };
        if self.rail_state.collection().is_none() {
            return;
        }
        self.pending_restore_task = None;
        if self.rail_state.select(&task, live) {
            // The strip belongs to the task it was derived in, so a restored selection is a scope
            // change like any other switch.
            self.invalidate_strip_for_task_scope(Some(task.clone()));
        } else {
            self.restore_notice =
                Some("前回のタスクは、現在閲覧できる情報に含まれていません。".to_owned());
        }
    }

    fn apply_restored_presentation(
        &mut self,
        candidates: &[Arc<ui_model::CandidateDisplay>],
        task: Option<&TaskId>,
        cx: &mut Context<Self>,
    ) {
        if self.restore_presentation_done {
            return;
        }
        let viewed = self.pending_restore_viewed.take();
        let pair = self.pending_restore_compare.take();
        if let Some(workbench) = &mut self.workbench {
            if let Some(task) = task {
                workbench.presentation.adopt_task(task.clone());
            }
            let present = |reference: &desktop_protocol::CandidateRef| {
                candidates
                    .iter()
                    .any(|candidate| candidate.identity == *reference)
            };
            let outcome = workbench
                .presentation
                .restore(viewed.as_ref(), pair.as_ref(), present);
            self.restore_notice = dock::restore_notice(&outcome);
        }
        self.restore_presentation_done = true;
        cx.notify();
    }

    /// The rail row its Enter path could activate right now, or the honest
    /// reason there is none. The palette uses this exact admission, so it never
    /// enables a command whose dispatch would silently no-op.
    fn rail_selection(&self) -> Result<&TaskId, &'static str> {
        let Some(active) = self.rail_active.as_ref() else {
            return Err("タスク一覧で操作対象が選ばれていません。");
        };
        if self.rail_state.selectable(active) {
            Ok(active)
        } else {
            Err("操作対象のタスクは、現在の検索結果に含まれていません。")
        }
    }

    fn select_rail_task(&mut self, task: &TaskId, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_restore_task = None;
        let live = self.handle.as_ref().and_then(Self::current_notice);
        self.rail_state.revalidate(live);
        let Some(search) = self.rail_state.results() else {
            return;
        };
        let Some(collection) = self.rail_state.collection() else {
            return;
        };
        let Some(index) = search
            .indices
            .iter()
            .position(|index| collection.rows[*index].identity == *task)
        else {
            return;
        };
        if self.rail_state.select(task, live) {
            // Invalidate the previous task's strip state at the switch boundary itself, before the
            // new derivation completes: its rows, details, focus handles and dismissal origin must
            // not remain shown, actionable or focusable (ADR-0011 §3).
            self.invalidate_strip_for_task_scope(Some(task.clone()));
            let switched = self
                .workbench
                .as_mut()
                .map(|workbench| dock::switch_task(&mut workbench.presentation, task.clone()));
            if let Some(event) = switched {
                self.surface_notice = event.notice;
                if let Some(target) = event.surface {
                    self.navigate(target, cx);
                }
            }
            self.dock_active = None;
            self.set_rail_active(task.clone(), index);
            self.rail_selection_pending = true;
            self.rail_focus.focus(window, cx);
            self.request_rail_work(cx);
        }
    }

    fn move_rail_active(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.rail_state
            .revalidate(self.handle.as_ref().and_then(Self::current_notice));
        let Some(search) = self.rail_state.results() else {
            return;
        };
        let Some(collection) = self.rail_state.collection() else {
            return;
        };
        if search.indices.is_empty() {
            return;
        }
        let current = self
            .rail_active_index
            .filter(|index| *index < search.indices.len());
        let next = current.map_or(0, |current| {
            (current as isize + delta).clamp(0, search.indices.len().saturating_sub(1) as isize)
                as usize
        });
        let task = collection.rows[search.indices[next]].identity.clone();
        self.set_rail_active(task, next);
        self.rail_list.scroll_to_reveal_item(next);
        self.rail_focus.focus(window, cx);
        cx.notify();
    }

    /// View one candidate by scoped identity and move the dock cursor there.
    ///
    /// Viewed stays a local presentation value: this never edits canonical
    /// selection and never dispatches a Runtime command.
    fn view_candidate(
        &mut self,
        identity: desktop_protocol::CandidateRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dock_is_current()
            || !self
                .rendered_candidates
                .iter()
                .any(|row| row.identity == identity)
        {
            return;
        }
        let surface = self
            .workbench
            .as_ref()
            .map(|workbench| workbench.core.navigation.surface())
            .unwrap_or_default();
        let event = match &mut self.workbench {
            Some(workbench) => dock::select(&mut workbench.presentation, identity.clone(), surface),
            None => return,
        };
        self.dock_active = Some(identity);
        self.dock_focus.focus(window, cx);
        if let Some(target) = event.surface
            && target != surface
        {
            self.navigate(target, cx);
        }
        self.surface_notice = event.notice;
        cx.notify();
    }

    /// Arm an explicit change-side action; the next dock selection updates that side
    /// and never moves Viewed (ADR-0010 D-3/D-5). Nothing is replaced silently.
    fn change_compare_side(&mut self, side: CompareSide, cx: &mut Context<Self>) {
        let event = match &mut self.workbench {
            Some(workbench) => dock::change_side(&mut workbench.presentation, side),
            None => return,
        };
        self.surface_notice = event.notice;
        cx.notify();
    }

    /// Close Compare: return to the artifact surface with the viewed candidate (D-7).
    fn close_compare(&mut self, cx: &mut Context<Self>) {
        let event = match &mut self.workbench {
            Some(workbench) => dock::close(&mut workbench.presentation),
            None => return,
        };
        self.surface_notice = event.notice;
        if let Some(target) = event.surface {
            self.navigate(target, cx);
        }
        cx.notify();
    }

    /// Move the dock cursor by one logical candidate and reveal it.
    fn move_dock_active(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dock_is_current() {
            return;
        }
        let count = self.rendered_candidates.len();
        if count == 0 {
            return;
        }
        // Row order is the authoritative iteration order, not identity order, so
        // the cursor is resolved by identity rather than by a sorted search.
        let current = self.dock_active.as_ref().and_then(|active| {
            self.rendered_candidates
                .iter()
                .position(|row| row.identity == *active)
        });
        let next = current.map_or(0, |current| {
            (current as isize + delta).clamp(0, count.saturating_sub(1) as isize) as usize
        });
        let identity = self.rendered_candidates[next].identity.clone();
        self.dock_active = Some(identity);
        let viewport = f32::from(self.candidate_scroll.bounds().size.width);
        let offset = dock::reveal(
            count,
            next,
            viewport,
            f32::from(self.candidate_scroll.offset().x),
        );
        self.candidate_scroll
            .set_offset(gpui::point(px(offset), px(0.0)));
        self.dock_focus.focus(window, cx);
        cx.notify();
    }

    /// A cached row may never authorize input after its publication fence closes.
    fn dock_is_current(&self) -> bool {
        let live = self
            .handle
            .as_ref()
            .and_then(|handle| handle.current().ok())
            .and_then(|lease| lease.notice());
        !self.rail_selection_pending && self.cache.is_authorized(live)
    }

    fn flow_is_current(&self) -> bool {
        let live = self
            .handle
            .as_ref()
            .and_then(|handle| handle.current().ok())
            .and_then(|lease| lease.notice());
        !self.rail_selection_pending && self.cache.is_authorized(live)
    }

    fn flow_rows(&self) -> Option<Arc<flow::Rows>> {
        self.flow_admitted_rows()
    }

    fn set_flow_query(&mut self, query: &str, cx: &mut Context<Self>) {
        self.flow_query.clear();
        self.flow_query.push_str(query);
        self.flow_input_selection = query.len()..query.len();
        self.flow_marked_range = None;
        self.flow_node_active = None;
        self.flow_edge_active = None;
        self.flow_admitted = None;
        self.flow_pending = None;
        if let Ok(mut job) = self.flow_jobs.lock() {
            *job = None;
        }
        cx.notify();
    }

    fn flow_replace_range(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let (replacement, _) = ime_splice(&self.flow_query, &range, text);
        self.set_flow_query(&replacement, cx);
    }

    fn flow_backspace(&mut self, cx: &mut Context<Self>) {
        let query = self.flow_query.clone();
        let range = if !self.flow_input_selection.is_empty() {
            self.flow_input_selection.clone()
        } else {
            let cursor = self.flow_input_selection.start;
            query[..cursor]
                .char_indices()
                .next_back()
                .map_or(cursor..cursor, |(index, ch)| index..index + ch.len_utf8())
        };
        if !range.is_empty() {
            self.flow_replace_range(range, "", cx);
        }
    }

    fn flow_delete(&mut self, cx: &mut Context<Self>) {
        let query = self.flow_query.clone();
        let range = if !self.flow_input_selection.is_empty() {
            self.flow_input_selection.clone()
        } else {
            let cursor = self.flow_input_selection.end;
            query[cursor..]
                .char_indices()
                .next()
                .map_or(cursor..cursor, |(_, ch)| cursor..cursor + ch.len_utf8())
        };
        if !range.is_empty() {
            self.flow_replace_range(range, "", cx);
        }
    }

    fn flow_move_cursor(&mut self, direction: isize, cx: &mut Context<Self>) {
        let query = self.flow_query.clone();
        let cursor = if direction < 0 {
            query[..self.flow_input_selection.start]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index)
        } else {
            let current = self.flow_input_selection.end;
            query[current..]
                .char_indices()
                .next()
                .map_or(query.len(), |(offset, ch)| current + offset + ch.len_utf8())
        };
        self.flow_input_selection = cursor..cursor;
        self.flow_marked_range = None;
        cx.notify();
    }

    fn flow_select_all(&mut self, cx: &mut Context<Self>) {
        self.flow_input_selection = 0..self.flow_query.len();
        self.flow_marked_range = None;
        cx.notify();
    }

    fn flow_focus_first_node(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rows) = self.flow_rows() else {
            return;
        };
        if let Some(row) = rows.nodes.first() {
            self.flow_node_active = Some(row.key());
            self.flow_node_list.scroll_to_reveal_item(0);
            self.flow_node_focus.focus(window, cx);
        } else if let Some(row) = rows.edges.first() {
            self.flow_edge_active = Some(row.key());
            self.flow_edge_list.scroll_to_reveal_item(0);
            self.flow_edge_focus.focus(window, cx);
        }
        cx.notify();
    }

    fn move_flow_node(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rows) = self.flow_rows() else {
            return;
        };
        if rows.nodes.is_empty() {
            return;
        }
        let current = self
            .flow_node_active
            .as_ref()
            .and_then(|active| rows.nodes.iter().position(|row| row.key() == *active));
        let next = current.map_or(0, |current| {
            (current as isize + delta).clamp(0, rows.nodes.len().saturating_sub(1) as isize)
                as usize
        });
        self.flow_node_active = Some(rows.nodes[next].key());
        self.flow_node_list.scroll_to_reveal_item(next);
        self.flow_node_focus.focus(window, cx);
        cx.notify();
    }

    fn move_flow_edge(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rows) = self.flow_rows() else {
            return;
        };
        if rows.edges.is_empty() {
            return;
        }
        let current = self
            .flow_edge_active
            .as_ref()
            .and_then(|active| rows.edges.iter().position(|row| row.key() == *active));
        let next = current.map_or(0, |current| {
            (current as isize + delta).clamp(0, rows.edges.len().saturating_sub(1) as isize)
                as usize
        });
        self.flow_edge_active = Some(rows.edges[next].key());
        self.flow_edge_list.scroll_to_reveal_item(next);
        self.flow_edge_focus.focus(window, cx);
        cx.notify();
    }

    fn select_flow_node(
        &mut self,
        key: flow::NodeKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rows) = self.flow_rows() else {
            return;
        };
        if !flow::contains_node(&rows, &key) {
            return;
        }
        self.flow_node_active = Some(key);
        self.flow_node_focus.focus(window, cx);
        cx.notify();
    }

    fn select_flow_edge(
        &mut self,
        key: flow::EdgeKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rows) = self.flow_rows() else {
            return;
        };
        if !flow::contains_edge(&rows, &key) {
            return;
        }
        self.flow_edge_active = Some(key);
        self.flow_edge_focus.focus(window, cx);
        cx.notify();
    }

    /// View the dock's active candidate.
    fn activate_dock_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(identity) = self.dock_active.clone() else {
            return;
        };
        self.view_candidate(identity, window, cx);
    }
}

struct SearchInputRegistration {
    entity: Entity<Shell>,
}

impl IntoElement for SearchInputRegistration {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SearchInputRegistration {
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
        let focus = self.entity.read(cx).rail_input_focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
    }
}

/// Binds GPUI's focused text input to the Flow search field on the same entity.
struct FlowInputRegistration {
    entity: Entity<Shell>,
}

impl IntoElement for FlowInputRegistration {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FlowInputRegistration {
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
        let focus = self.entity.read(cx).flow_input_focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
    }
}

/// Translate a UTF-16 text-input range into the byte range of `current`.
///
/// The IME delivers ranges in UTF-16 units while the query is stored as UTF-8,
/// so a marked or replacement range must be converted through the same
/// conversion the caret uses. A `None` range falls back to the marked range and
/// then to the current selection, which is exactly the composition contract.
/// Whether a recorded Stop intent belongs to the task/session this window is
/// currently authorized to display. One predicate for the control and palette.
fn stop_scope_matches(
    active: &stop::ActiveStop,
    authorized_text: bool,
    rail_selection_pending: bool,
    text: &ui_model::WorkbenchText,
) -> bool {
    authorized_text
        && !rail_selection_pending
        && active.session_generation == text.session_generation
        && text.task.as_ref() == Some(&active.task)
}

fn ime_byte_range(
    current: &str,
    utf16_range: Option<Range<usize>>,
    marked_range: Option<Range<usize>>,
    selection: &Range<usize>,
) -> Range<usize> {
    utf16_range
        .map(|range| byte_offset(current, range.start)..byte_offset(current, range.end))
        .or_else(|| marked_range.clone())
        .unwrap_or_else(|| selection.clone())
}

/// Splice `new_text` over a byte range, returning the new text and caret byte offset.
fn ime_splice(current: &str, range: &Range<usize>, new_text: &str) -> (String, usize) {
    let mut replacement =
        String::with_capacity(current.len() - (range.end - range.start) + new_text.len());
    replacement.push_str(&current[..range.start]);
    replacement.push_str(new_text);
    replacement.push_str(&current[range.end..]);
    let caret = range.start + new_text.len();
    (replacement, caret)
}

fn utf16_offset(text: &str, byte_offset: usize) -> usize {
    text.char_indices()
        .take_while(|(index, _)| *index < byte_offset)
        .map(|(_, character)| character.len_utf16())
        .sum()
}

fn byte_offset(text: &str, utf16_offset: usize) -> usize {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        if units >= utf16_offset {
            return index;
        }
        units += character.len_utf16();
        if units > utf16_offset {
            return index + character.len_utf8();
        }
    }
    text.len()
}

impl EntityInputHandler for Shell {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = if self.palette_focus.is_focused(window) {
            self.palette_query.clone()
        } else if self.flow_input_focus.is_focused(window) {
            self.flow_query.clone()
        } else {
            self.rail_state.query().to_owned()
        };
        let start = byte_offset(&text, range_utf16.start);
        let end = byte_offset(&text, range_utf16.end);
        actual_range.replace(utf16_offset(&text, start)..utf16_offset(&text, end));
        Some(text[start..end].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (text, selection) = if self.palette_focus.is_focused(window) {
            (
                self.palette_query.clone(),
                self.palette_input_selection.clone(),
            )
        } else if self.flow_input_focus.is_focused(window) {
            (self.flow_query.clone(), self.flow_input_selection.clone())
        } else {
            (
                self.rail_state.query().to_owned(),
                self.rail_input_selection.clone(),
            )
        };
        Some(UTF16Selection {
            range: utf16_offset(&text, selection.start)..utf16_offset(&text, selection.end),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let (text, range) = if self.palette_focus.is_focused(window) {
            let range = self.palette_marked_range.as_ref()?;
            (self.palette_query.as_str(), range)
        } else if self.flow_input_focus.is_focused(window) {
            let range = self.flow_marked_range.as_ref()?;
            (self.flow_query.as_str(), range)
        } else {
            let range = self.rail_marked_range.as_ref()?;
            (self.rail_state.query(), range)
        };
        Some(utf16_offset(text, range.start)..utf16_offset(text, range.end))
    }

    fn unmark_text(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if self.palette_focus.is_focused(window) {
            self.palette_marked_range = None;
        } else if self.flow_input_focus.is_focused(window) {
            self.flow_marked_range = None;
        } else {
            self.rail_marked_range = None;
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_focus.is_focused(window) {
            let current = self.palette_query.clone();
            let range = ime_byte_range(
                &current,
                range_utf16,
                self.palette_marked_range.clone(),
                &self.palette_input_selection,
            );
            let (replacement, cursor) = ime_splice(&current, &range, new_text);
            self.palette_set_query(&replacement, cx);
            self.palette_input_selection = cursor..cursor;
            self.palette_marked_range = None;
            return;
        }
        if self.flow_input_focus.is_focused(window) {
            let current = self.flow_query.clone();
            let range = ime_byte_range(
                &current,
                range_utf16,
                self.flow_marked_range.clone(),
                &self.flow_input_selection,
            );
            let (replacement, cursor) = ime_splice(&current, &range, new_text);
            self.set_flow_query(&replacement, cx);
            self.flow_input_selection = cursor..cursor;
            self.flow_marked_range = None;
            return;
        }
        let current = self.rail_state.query().to_owned();
        let range = ime_byte_range(
            &current,
            range_utf16,
            self.rail_marked_range.clone(),
            &self.rail_input_selection,
        );
        let (replacement, cursor) = ime_splice(&current, &range, new_text);
        self.set_rail_query(&replacement, cx);
        self.rail_input_selection = cursor..cursor;
        self.rail_marked_range = None;
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let palette = self.palette_focus.is_focused(window);
        let flow = self.flow_input_focus.is_focused(window);
        let (current, marked, selection) = if palette {
            (
                self.palette_query.clone(),
                self.palette_marked_range.clone(),
                self.palette_input_selection.clone(),
            )
        } else if flow {
            (
                self.flow_query.clone(),
                self.flow_marked_range.clone(),
                self.flow_input_selection.clone(),
            )
        } else {
            (
                self.rail_state.query().to_owned(),
                self.rail_marked_range.clone(),
                self.rail_input_selection.clone(),
            )
        };
        let range = ime_byte_range(&current, range_utf16, marked, &selection);
        let (replacement, caret) = ime_splice(&current, &range, new_text);
        let new_marked =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        let new_selection = new_selected_range_utf16
            .map(|selected| {
                let start = byte_offset(new_text, selected.start);
                let end = byte_offset(new_text, selected.end);
                range.start + start..range.start + end
            })
            .unwrap_or_else(|| caret..caret);
        if palette {
            self.palette_set_query(&replacement, cx);
            self.palette_marked_range = new_marked;
            self.palette_input_selection = new_selection;
        } else if flow {
            self.set_flow_query(&replacement, cx);
            self.flow_marked_range = new_marked;
            self.flow_input_selection = new_selection;
        } else {
            self.set_rail_query(&replacement, cx);
            self.rail_marked_range = new_marked;
            self.rail_input_selection = new_selection;
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }
    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = self
            .workbench
            .as_ref()
            .map(|w| w.core.navigation.surface())
            .unwrap_or_default();
        let live = self
            .handle
            .as_ref()
            .and_then(|handle| handle.current().ok())
            .and_then(|lease| lease.notice());
        // Revalidate at scene creation: only text whose exact publication identity
        // is still live may be rendered. Window-local command identity survives
        // ordinary publication-version changes but never authorizes stale display.
        self.cache.revalidate(live);
        let authorized_text = self.cache.is_authorized(live);
        if authorized_text {
            self.synchronize_stop_context();
        }
        // Reconcile shell-local strip state with the task scope and the rows that are about to be
        // rendered: a task switch invalidates the previous task's state at once, a resolved
        // condition must never persist, and a reappearing identity starts clean (ADR-0011 §3).
        self.reconcile_strip_state();
        let strip_scope = self.rail_state.selected().cloned();
        let text = self.cache.text();
        // One resolved token set feeds every layout and text input below; a
        // surface reads these adapters instead of a constant, and nothing here
        // resolves a second set.
        let body_text = TextSpec::ui(&self.tokens, TextStyle::Body);
        let xs_text = TextSpec::ui(&self.tokens, TextStyle::Xs);
        let rail_width = shell_style::rail_width(&self.tokens);
        let inspector_width = shell_style::inspector_width(&self.tokens);
        let focus_outline = shell_style::focus_outline(&self.tokens);
        // A usable scale factor yields the physical hairline; an unusable one
        // yields no separator at all (fail closed, never a substituted width).
        let hairline = shell_style::separator(&self.tokens, window.scale_factor()).ok();
        if let Ok(mut preferences) = self.preferences.lock() {
            let bounds = window.bounds();
            preferences.window = Some(WindowGeometry {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                dpr: window.scale_factor(),
                maximized: window.is_maximized(),
            });
            preferences.surface = surface;
            if authorized_text {
                if let Some(task) = text
                    .task
                    .clone()
                    .or_else(|| self.rail_state.selected().cloned())
                {
                    preferences.last_task_id = Some(task);
                }
                if self.restore_presentation_done {
                    preferences.viewed = self
                        .workbench
                        .as_ref()
                        .and_then(|workbench| workbench.presentation.viewed())
                        .map(|viewed| viewed.0.clone());
                    preferences.compare = self
                        .workbench
                        .as_ref()
                        .and_then(|workbench| workbench.presentation.compare_pair())
                        .map(|pair| [pair[0].0.clone(), pair[1].0.clone()]);
                }
            }
        }
        let local_stop = self.stop_intent.active.as_ref().filter(|active| {
            stop_scope_matches(active, authorized_text, self.rail_selection_pending, text)
        });
        let command_view = local_stop
            .and_then(|active| active.command_id.as_ref())
            .and_then(|command_id| text.command_views.get(command_id));
        let retryable_admission = self.stop_retryable_admission();
        let (stop_availability, receipt_availability) =
            self.action_availability(authorized_text, text);
        let stop_enabled = stop_availability.enabled;
        let stop_disabled_reason: Arc<str> = stop_availability.reason;
        let feedback = local_stop
            .map(|active| active.receipt_feedback())
            .unwrap_or(ReceiptFeedback::Idle);
        let receipt_note = feedback.note();
        // Retaining the identity is independent from the authorized receipt state:
        // a Pending/Accepted view must not hide a bounded reference refusal.
        let retention_warning = command_view.and(local_stop).and_then(|active| {
            if let StopProgress::RetentionError(display) = &active.progress {
                Some(display)
            } else {
                None
            }
        });
        let manual_intervention_required = retention_warning
            .is_some_and(|display| display.manual_intervention)
            || feedback.requires_manual_intervention()
            || command_view
                .map(|display| display.manual_intervention)
                .unwrap_or_else(|| {
                    if let Some(active) = local_stop {
                        match &active.progress {
                            StopProgress::Retaining | StopProgress::Retained => true,
                            StopProgress::AdmissionError(display)
                            | StopProgress::RetentionError(display) => display.manual_intervention,
                            StopProgress::Dispatching => true,
                        }
                    } else {
                        self.stop_intent.active.as_ref().is_some_and(|active| {
                            active.command_id.is_some() || !active.retryable_admission
                        })
                    }
                });
        let (stop_label, stop_detail): (SharedString, SharedString) = if let Some(display) =
            command_view
        {
            (
                SharedString::from(display.label.clone()),
                SharedString::from(display.detail.clone()),
            )
        } else if let Some(active) = local_stop {
            match &active.progress {
                    StopProgress::Dispatching => (
                        "停止要求への応答を待っています".into(),
                        "送信済みの可能性があるため再送しません。タスクの終了は別に通知されます。"
                            .into(),
                    ),
                    StopProgress::Retaining => (
                        "停止要求を受け付けました。確認用の情報を保持中です".into(),
                        "受付状況の表示を待っています。受け付けたことは、タスクが終了したことを意味しません。".into(),
                    ),
                    StopProgress::Retained => (
                        "停止要求を受け付けました".into(),
                        "閲覧が認可された最新の受付状況を待っています。終了したとは判断しません。"
                            .into(),
                    ),
                    StopProgress::AdmissionError(display)
                    | StopProgress::RetentionError(display) => (
                        SharedString::from(display.label.clone()),
                        SharedString::from(display.detail.clone()),
                    ),
                }
        } else if !authorized_text {
            (
                "停止要求の状況を確認できません".into(),
                "現在表示できる情報がないため、以前の要求の状況は表示しません。".into(),
            )
        } else if let Some(active) = self.stop_intent.active.as_ref() {
            if active.command_id.is_none() && active.retryable_admission {
                (
                        "停止要求は受け付けられていません".into(),
                        "容量制限により受付前に拒否されました。新しい停止要求を送るには、もう一度操作してください。"
                            .into(),
                    )
            } else {
                (
                        "この停止要求の状況は表示できません".into(),
                        "別のタスクまたは接続で送った要求で、実行済みの可能性があります。状況の確認が必要です。"
                            .into(),
                    )
            }
        } else {
            (
                "この画面から停止要求は送っていません".into(),
                "停止要求を1回送ります。受付状況とタスクの終了結果は別に確認します。".into(),
            )
        };
        let reconcile_enabled = receipt_availability.enabled;
        let reconcile_disabled_reason: Arc<str> = receipt_availability.reason;
        // Availability is derived once, before later mutable borrows, and the
        // overlay reads only this owned snapshot.
        let palette_facts = self.palette_facts(authorized_text, text);
        let title = SharedString::from(text.title.clone());
        let graph = text.graph.clone();
        let mut flow_rows = None;
        if surface == Surface::Flow {
            if authorized_text {
                let revision = flow::revision_key(&graph);
                let revision_changed = self.flow_revision_key.as_ref() != Some(&revision);
                if revision_changed {
                    self.flow_revision_key = Some(revision.clone());
                    self.flow_node_active = None;
                    self.flow_edge_active = None;
                    self.flow_admitted = None;
                    self.flow_pending = None;
                    if let Ok(mut job) = self.flow_jobs.lock() {
                        *job = None;
                    }
                    self.flow_node_list.reset(0);
                    self.flow_edge_list.reset(0);
                }
                let key = flow::AdmissionKey {
                    session_generation: text.session_generation,
                    publication_version: text.publication_version,
                    epoch: text.epoch.clone(),
                    revision,
                    query: self.flow_query.clone(),
                };
                flow_rows = self
                    .flow_admitted
                    .as_ref()
                    .filter(|(admitted, _)| *admitted == key)
                    .map(|(_, rows)| Arc::clone(rows));
                if flow_rows.is_none() {
                    self.flow_node_active = None;
                    self.flow_edge_active = None;
                    if self.flow_pending.as_ref() != Some(&key) {
                        let generation = self.flow_request_generation.next_generation();
                        if let Ok(mut job) = self.flow_jobs.lock() {
                            *job = Some(flow::Job {
                                generation,
                                key: key.clone(),
                                graphs: graph.clone(),
                            });
                        }
                        self.flow_pending = Some(key);
                        self.flow_request_generation.request();
                    }
                }
                if flow_rows.is_none() {
                    if self.flow_node_list.item_count() != 0 {
                        self.flow_node_list.reset(0);
                    }
                    if self.flow_edge_list.item_count() != 0 {
                        self.flow_edge_list.reset(0);
                    }
                }
            } else {
                self.flow_revision_key = None;
                self.flow_admitted = None;
                self.flow_pending = None;
                if let Ok(mut job) = self.flow_jobs.lock() {
                    *job = None;
                }
                self.flow_node_active = None;
                self.flow_edge_active = None;
                if self.flow_node_list.item_count() != 0 {
                    self.flow_node_list.reset(0);
                }
                if self.flow_edge_list.item_count() != 0 {
                    self.flow_edge_list.reset(0);
                }
            }
        }
        let artifacts = text.artifacts.clone();
        let candidate_groups = text.candidate_groups.clone();
        let formal_candidates = text.formal_candidates.clone();
        let viewed = self
            .workbench
            .as_ref()
            .and_then(|w| w.presentation.viewed())
            .map(|v| v.0.clone());
        let inspector_view = text.inspector.view(viewed.as_ref());
        let inspector_task = text.task.clone();
        // The cached display can still describe the previous Task while its
        // replacement is derived, or after the selected identity changed; the
        // inspector withholds and invalidates disclosure in both cases.
        let inspector_withheld = inspector::scope(
            authorized_text,
            self.rail_selection_pending,
            text.task.as_ref(),
            self.rail_state.selected(),
        );
        // Rows derived for another task are never shown, actionable or focusable: the in-flight
        // window after a switch still holds the previous task's text.
        let intervention_strip = intervention::scoped_strip(
            text.interventions.strip(),
            text.task.as_ref(),
            strip_scope.as_ref(),
        );
        let compare_pair = self
            .workbench
            .as_ref()
            .and_then(|workbench| workbench.presentation.compare_pair());
        let pending_side_a: Option<desktop_protocol::CandidateRef> = self
            .workbench
            .as_ref()
            .and_then(|workbench| workbench.presentation.compare_side(CompareSide::A))
            .map(|candidate| candidate.0.clone());
        let changing_side = self
            .workbench
            .as_ref()
            .and_then(|workbench| workbench.presentation.changing_side());
        let surface_notice = self.surface_notice.clone();
        let viewed_row = viewed.as_ref().and_then(|identity| {
            artifacts
                .iter()
                .find(|candidate| candidate.identity == *identity)
        });
        // D-4: a held reference whose identity disappeared is explicitly unavailable,
        // never silently replaced by another candidate.
        let viewed_missing = viewed.is_some() && viewed_row.is_none();
        let viewed_label: SharedString = if viewed_missing {
            "閲覧中の候補は、現在閲覧できる情報に含まれていません。別の候補を選んでください。"
                .into()
        } else {
            viewed_row
                .map(|candidate| SharedString::from(candidate.label.clone()))
                .unwrap_or_else(|| "下の候補一覧から、表示する候補を選んでください".into())
        };
        let side_a_exists = compare_pair.is_some() || pending_side_a.is_some();
        let side_b_exists = compare_pair.is_some();
        let compare_summary: SharedString = if let Some(side) = changing_side {
            format!(
                "比較する{}側を変更します。下の一覧から候補を選んでください。",
                side.label()
            )
            .into()
        } else if compare_pair.is_some() {
            "比較対象はこの画面での選択です。正式な採用結果は実行基盤から通知されます。".into()
        } else if pending_side_a.is_some() {
            "A側の候補を選択済みです。下の一覧から別の候補を選ぶと比較対象がそろいます。".into()
        } else {
            "比較対象が未選択です。下の一覧から候補を2つ選んでください。".into()
        };
        let compare_panel = div()
            .id("compare-selection")
            .role(Role::Group)
            .aria_label("この画面での比較対象。正式な採用結果ではありません")
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().flex().gap_4().children((0..2_usize).map(|index| {
                let side = if index == 0 {
                    CompareSide::A
                } else {
                    CompareSide::B
                };
                let reference = compare_pair
                    .map(|pair| &pair[side.index()].0)
                    .or(if index == 0 {
                        pending_side_a.as_ref()
                    } else {
                        None
                    });
                let row = reference.and_then(|identity| {
                    artifacts
                        .iter()
                        .find(|candidate| candidate.identity == *identity)
                });
                let available = reference.is_none() || row.is_some();
                let label: SharedString = match (reference, row) {
                    (None, _) => "未選択".into(),
                    (Some(_), Some(row)) => SharedString::from(row.label.clone()),
                    (Some(_), None) => {
                        "この候補は、現在閲覧できる情報に含まれていません。"
                            .into()
                    }
                };
                let identity_label = reference
                    .map(|identity| SharedString::from(identity.candidate_id.as_str()))
                    .unwrap_or_else(|| SharedString::from("none"));
                div()
                    .id(("compare-side", index))
                    .role(Role::Group)
                    .aria_label(SharedString::from(format!(
                        "比較する{}側：{}",
                        side.label(),
                        label
                    )))
                    .aria_value(identity_label)
                    .flex_1()
                    .min_w_0()
                    .p_4()
                    .border_1()
                    .border_color(rgb(if available { 0x626870 } else { 0xd08a6a }))
                    .bg(rgb(0x202328))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(SharedString::from(format!("比較する{}側", side.label())))
                    .child(label)
            })))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child({
                        let help = if side_a_exists {
                            "押したあとに候補一覧から選ぶと、比較するA側だけを置き換えます。閲覧中の候補は変わりません。"
                        } else {
                            "比較するA側はまだ選ばれていません。下の一覧から候補を選ぶと、この操作が使えます。"
                        };
                        div()
                            .id("compare-change-a")
                            .role(Role::Button)
                            .aria_label("比較するA側の候補を変更")
                            .aria_description(help)
                            .tooltip({
                                let style = xs_text.clone();
                                move |_, cx| help::tooltip(help.into(), style.clone(), inspector_width, cx)
                            })
                            .px_4()
                            .py_2()
                            .border_1()
                            .border_color(rgb(if side_a_exists { 0x626870 } else { 0x343940 }))
                            .when(side_a_exists, |element| element.cursor_pointer())
                            .child("A側を変更")
                            .when(side_a_exists, |element| {
                                element.on_click(cx.listener(|this, _, _, cx| {
                                    this.change_compare_side(CompareSide::A, cx);
                                }))
                            })
                    })
                    .child({
                        let help = if side_b_exists {
                            "押したあとに候補一覧から選ぶと、比較するB側だけを置き換えます。閲覧中の候補は変わりません。"
                        } else {
                            "比較するB側はまだ選ばれていません。A側とB側の候補がそろうと、この操作が使えます。"
                        };
                        div()
                            .id("compare-change-b")
                            .role(Role::Button)
                            .aria_label("比較するB側の候補を変更")
                            .aria_description(help)
                            .tooltip({
                                let style = xs_text.clone();
                                move |_, cx| help::tooltip(help.into(), style.clone(), inspector_width, cx)
                            })
                            .px_4()
                            .py_2()
                            .border_1()
                            .border_color(rgb(if side_b_exists { 0x626870 } else { 0x343940 }))
                            .when(side_b_exists, |element| element.cursor_pointer())
                            .child("B側を変更")
                            .when(side_b_exists, |element| {
                                element.on_click(cx.listener(|this, _, _, cx| {
                                    this.change_compare_side(CompareSide::B, cx);
                                }))
                            })
                    })
                    .child(
                        div()
                            .id("compare-close")
                            .role(Role::Button)
                            .aria_label("比較を閉じて閲覧中の候補に戻る")
                            .aria_description("比較画面を閉じ、閲覧中の候補の画面に戻ります。正式な採用結果は変更しません。")
                            .tooltip({
                                let style = xs_text.clone();
                                move |_, cx| help::tooltip("比較画面を閉じ、閲覧中の候補の画面に戻ります。正式な採用結果は変更しません。".into(), style.clone(), inspector_width, cx)
                            })
                            .px_4()
                            .py_2()
                            .border_1()
                            .border_color(rgb(0x626870))
                            .cursor_pointer()
                            .child("比較を閉じる")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_compare(cx);
                            })),
                    ),
            )
            .child(compare_summary);
        let diagnostics_open = self
            .workbench
            .as_ref()
            .is_some_and(|w| w.presentation.diagnostics_open);
        let diagnostic_text = text.diagnostics.clone();
        let mode = self.error.clone().unwrap_or_else(|| text.mode.clone());
        let axis_cell = |id: &'static str, label: &'static str, value: &Arc<str>| {
            let value = if self.rail_selection_pending && id != "state-axis-connection" {
                SharedString::from("選択したタスクを更新中")
            } else {
                SharedString::new(Arc::clone(value))
            };
            let accessible = SharedString::from(format!("{label}: {}", value.as_ref()));
            let description = match id {
                "state-axis-task-status" => {
                    "タスク全体の状態です。処理段階や成果物の利用状態とは別に通知されます。"
                }
                "state-axis-inner-phase" => {
                    "実行元が通知した処理段階の表記です。未知の表記を、この画面で別の意味に置き換えることはありません。"
                }
                "state-axis-iteration" => {
                    "実行元が通知した現在の反復回数です。候補を閲覧しても変わりません。"
                }
                "state-axis-artifact-availability" => {
                    "成果物が利用できるかどうかの通知です。画像表示機能の実装状況や品質の確認結果とは異なります。"
                }
                "state-axis-connection" => {
                    "実行元との接続と情報の鮮度です。接続が切れても、タスクの失敗とは判断しません。"
                }
                _ => "実行元が通知した正式な結果です。この画面で閲覧中・比較中の候補とは別です。",
            };
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .id(id)
                .role(Role::Label)
                .aria_label(accessible.clone())
                .aria_value(accessible)
                .aria_description(description)
                .tooltip({
                    let style = xs_text.clone();
                    move |_, cx| {
                        help::tooltip(description.into(), style.clone(), inspector_width, cx)
                    }
                })
                .child(
                    xs_text
                        .clone()
                        .apply(div())
                        .text_color(rgb(0x9da7b1))
                        .child(label),
                )
                .child(value)
        };
        let axis_row_one = div()
            .flex()
            .gap_4()
            .child(axis_cell(
                "state-axis-task-status",
                "タスクの状態",
                &text.axes.task_status,
            ))
            .child(axis_cell(
                "state-axis-inner-phase",
                "処理段階",
                &text.axes.inner_phase,
            ))
            .child(axis_cell(
                "state-axis-iteration",
                "反復回数",
                &text.axes.iteration,
            ));
        let axis_row_two = div()
            .flex()
            .gap_4()
            .child(axis_cell(
                "state-axis-artifact-availability",
                "成果物の利用状態",
                &text.axes.artifact_availability,
            ))
            .child(axis_cell(
                "state-axis-connection",
                "接続・情報の鮮度",
                &text.axes.connection,
            ))
            .child(axis_cell(
                "state-axis-formal-outcome",
                "正式な結果",
                &text.axes.formal_outcome,
            ));
        let surface_content = match surface {
            Surface::Artifact => {
                "現在の版では、成果物の画像を表示できません。候補の説明情報のみを表示しています。"
            }
            Surface::Compare => "",
            Surface::Flow => {
                "テスト用データの処理構造を一覧表示します。実際の実行基盤への接続や図による表示は未実装です。"
            }
        };
        let flow_graph_empty = graph.is_empty();
        let flow_updating = authorized_text && flow_rows.is_none() && self.flow_pending.is_some();
        let flow_node_rows = flow_rows
            .as_ref()
            .map(|rows| Arc::clone(&rows.nodes))
            .unwrap_or_default();
        let flow_edge_rows = flow_rows
            .as_ref()
            .map(|rows| Arc::clone(&rows.edges))
            .unwrap_or_default();
        let flow_node_focused = self.flow_node_focus.is_focused(window);
        let flow_edge_focused = self.flow_edge_focus.is_focused(window);
        let flow_node_active = self.flow_node_active.clone();
        let flow_edge_active = self.flow_edge_active.clone();
        let flow_search_focused = self.flow_input_focus.is_focused(window);
        let flow_search_display = if self.flow_query.is_empty() && !flow_search_focused {
            "処理と接続関係を検索…".to_owned()
        } else {
            self.flow_query.clone()
        };
        let flow_headers = flow_rows
            .as_ref()
            .map(|rows| {
                rows.graphs
                    .iter()
                    .enumerate()
                    .map(|(index, graph)| {
                        div()
                            .id(("flow-revision", index))
                            .role(Role::Label)
                            .aria_label(SharedString::from(flow::revision_label(graph)))
                            .aria_value(SharedString::from(flow::revision_label(graph)))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(SharedString::from(flow::revision_label(graph)))
                            .child(SharedString::from(format!(
                                "表示情報の版 {}（処理構造そのものの版とは別です）",
                                graph.source_revision.as_str()
                            )))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let flow_node_list_element = (!flow_node_rows.is_empty()).then(|| {
            let rows = Arc::clone(&flow_node_rows);
            let active = flow_node_active.clone();
            let shell = cx.entity();
            list(self.flow_node_list.clone(), move |index, _window, _app| {
                let Some(row) = rows.get(index).cloned() else {
                    return div().into_any_element();
                };
                let node = Arc::clone(row.node());
                let graph = Arc::clone(&row.graph);
                let key = row.key();
                let is_active = active.as_ref() == Some(&key);
                let layer = node.layer.as_deref().unwrap_or("階層情報なし");
                let aria = format!(
                    "処理：{}。階層：{}。{}。ノードID：{}。",
                    node.label,
                    layer,
                    flow::revision_label(&graph),
                    node.identity.as_str()
                );
                let element_id = SharedString::from(format!(
                    "flow-node-{}:{}",
                    graph.graph_ref.graph_id.as_str(),
                    node.identity.as_str()
                ));
                let click_key = key.clone();
                let click_shell = shell.clone();
                div()
                    .id(element_id.clone())
                    .accessibility_id(element_id)
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(aria))
                    .when(is_active && flow_node_focused, |element| {
                        element.aria_active_descendant()
                    })
                    .flex()
                    .flex_col()
                    .gap_0()
                    .p_1()
                    .border_1()
                    .border_color(rgb(if is_active && flow_node_focused {
                        0xe6c278
                    } else {
                        0x626870
                    }))
                    .bg(rgb(if is_active { 0x343a42 } else { 0x25292e }))
                    .child(SharedString::from(format!("{} · {}", node.label, layer)))
                    .child(SharedString::from(node.identity.as_str()))
                    .on_click(move |_, window, app| {
                        let key = click_key.clone();
                        click_shell.update(app, |this, cx| {
                            this.select_flow_node(key, window, cx);
                        });
                    })
                    .into_any_element()
            })
            .size_full()
        });
        let flow_edge_list_element = (!flow_edge_rows.is_empty()).then(|| {
            let rows = Arc::clone(&flow_edge_rows);
            let active = flow_edge_active.clone();
            let shell = cx.entity();
            list(self.flow_edge_list.clone(), move |index, _window, _app| {
                let Some(row) = rows.get(index).cloned() else {
                    return div().into_any_element();
                };
                let edge = Arc::clone(row.edge());
                let graph = Arc::clone(&row.graph);
                let key = row.key();
                let is_active = active.as_ref() == Some(&key);
                let kind = edge.kind.as_deref().unwrap_or("種類の情報なし");
                let aria = format!(
                    "接続：{}。{}から{}へ。{}。接続ID：{}。接続元ノードID：{}。接続先ノードID：{}。",
                    kind,
                    edge.from_label,
                    edge.to_label,
                    flow::revision_label(&graph),
                    edge.identity.as_str(),
                    edge.from.as_str(),
                    edge.to.as_str()
                );
                let element_id = SharedString::from(format!(
                    "flow-edge-{}:{}",
                    graph.graph_ref.graph_id.as_str(),
                    edge.identity.as_str()
                ));
                let click_key = key.clone();
                let click_shell = shell.clone();
                div()
                    .id(element_id.clone())
                    .accessibility_id(element_id)
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(aria))
                    .when(is_active && flow_edge_focused, |element| {
                        element.aria_active_descendant()
                    })
                    .flex()
                    .flex_col()
                    .gap_0()
                    .p_1()
                    .border_1()
                    .border_color(rgb(if is_active && flow_edge_focused {
                        0xe6c278
                    } else {
                        0x626870
                    }))
                    .bg(rgb(if is_active { 0x343a42 } else { 0x25292e }))
                    .child(SharedString::from(format!(
                        "{} · {} → {}",
                        kind, edge.from_label, edge.to_label
                    )))
                    .child(SharedString::from(edge.identity.as_str()))
                    .on_click(move |_, window, app| {
                        let key = click_key.clone();
                        click_shell.update(app, |this, cx| {
                            this.select_flow_edge(key, window, cx);
                        });
                    })
                    .into_any_element()
            })
            .size_full()
        });
        let flow_node_empty_message = if !authorized_text {
            "現在閲覧できる情報がないため、処理構造を表示できません。"
        } else if flow_updating {
            "処理構造の表示を更新中…"
        } else if flow_graph_empty {
            "現在閲覧できる情報に、処理構造は含まれていません。"
        } else if self.flow_query.trim().is_empty() {
            "この版には処理の項目がありません。"
        } else {
            "検索条件に一致する処理はありません。"
        };
        let flow_edge_empty_message = if !authorized_text {
            "現在閲覧できる情報がないため、処理構造を表示できません。"
        } else if flow_updating {
            "処理構造の表示を更新中…"
        } else if flow_graph_empty {
            "現在閲覧できる情報に、処理構造は含まれていません。"
        } else if self.flow_query.trim().is_empty() {
            "この版には接続関係がありません。"
        } else {
            "検索条件に一致する接続関係はありません。"
        };
        let flow_panel = div()
            .id("flow-inspection")
            .role(Role::Region)
            .aria_label("処理の流れと接続関係")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("flow-search")
                    .role(Role::TextInput)
                    .aria_label("処理と接続関係を検索")
                    .aria_value(SharedString::from(self.flow_query.clone()))
                    .key_context("FlowSearchInput")
                    .track_focus(&self.flow_input_focus)
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(rgb(if flow_search_focused {
                        0xe6c278
                    } else {
                        0x626870
                    }))
                    .bg(rgb(0x17191c))
                    .relative()
                    .child(SharedString::from(flow_search_display))
                    .child(div().absolute().size_full().child(FlowInputRegistration {
                        entity: cx.entity(),
                    }))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.flow_input_focus.focus(window, cx)),
                    )
                    .on_action(cx.listener(|this, _: &FlowEnter, window, cx| {
                        this.flow_focus_first_node(window, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &FlowBackspace, _, cx| this.flow_backspace(cx)),
                    )
                    .on_action(cx.listener(|this, _: &FlowDelete, _, cx| this.flow_delete(cx)))
                    .on_action(
                        cx.listener(|this, _: &FlowLeft, _, cx| this.flow_move_cursor(-1, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &FlowRight, _, cx| this.flow_move_cursor(1, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &FlowSelectAll, _, cx| this.flow_select_all(cx)),
                    ),
            )
            .children(flow_headers)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(SharedString::from(format!(
                                "処理（{}件）",
                                flow_node_rows.len()
                            )))
                            .child(
                                div()
                                    .id("flow-node-list")
                                    .role(Role::List)
                                    .aria_label("処理の一覧")
                                    .key_context("FlowNodeList")
                                    .track_focus(&self.flow_node_focus)
                                    .flex_1()
                                    .min_h_0()
                                    .when_some(flow_node_list_element, |element, list| {
                                        element.child(list)
                                    })
                                    .on_action(cx.listener(|this, _: &FlowNodeUp, window, cx| {
                                        this.move_flow_node(-1, window, cx);
                                    }))
                                    .on_action(cx.listener(
                                        |this, _: &FlowNodeDown, window, cx| {
                                            this.move_flow_node(1, window, cx);
                                        },
                                    )),
                            )
                            .children(flow_node_rows.is_empty().then(|| {
                                div()
                                    .id("flow-node-empty")
                                    .role(Role::Label)
                                    .child(SharedString::from(flow_node_empty_message))
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(SharedString::from(format!(
                                "接続関係（{}件）",
                                flow_edge_rows.len()
                            )))
                            .child(
                                div()
                                    .id("flow-edge-list")
                                    .role(Role::List)
                                    .aria_label("接続関係の一覧")
                                    .key_context("FlowEdgeList")
                                    .track_focus(&self.flow_edge_focus)
                                    .flex_1()
                                    .min_h_0()
                                    .when_some(flow_edge_list_element, |element, list| {
                                        element.child(list)
                                    })
                                    .on_action(cx.listener(|this, _: &FlowEdgeUp, window, cx| {
                                        this.move_flow_edge(-1, window, cx);
                                    }))
                                    .on_action(cx.listener(
                                        |this, _: &FlowEdgeDown, window, cx| {
                                            this.move_flow_edge(1, window, cx);
                                        },
                                    )),
                            )
                            .children(flow_edge_rows.is_empty().then(|| {
                                div()
                                    .id("flow-edge-empty")
                                    .role(Role::Label)
                                    .child(SharedString::from(flow_edge_empty_message))
                            })),
                    ),
            );
        let stop_focused = self.stop_focus.is_focused(window);
        let stop_description: SharedString = if stop_enabled && !retryable_admission {
            "現在操作が認められているタスクへ、停止要求を1回送ります。受付と終了は別に確認します。"
                .into()
        } else {
            SharedString::new(Arc::clone(&stop_disabled_reason))
        };
        let stop_button = div()
            .id("stop-request")
            .role(Role::Button)
            .aria_label(if stop_enabled && retryable_admission {
                "受付前の容量制限による拒否後に、新しい停止要求を送る"
            } else if stop_enabled {
                "タスクを停止"
            } else {
                "タスクを停止（現在は操作できません）"
            })
            .aria_keyshortcuts("Enter Space")
            .aria_description(stop_description.clone())
            .tooltip({
                let style = xs_text.clone();
                move |_, cx| {
                    help::tooltip(stop_description.clone(), style.clone(), inspector_width, cx)
                }
            })
            .track_focus(&self.stop_focus)
            .px_4()
            .py_2()
            .border_1()
            .border_color(rgb(if stop_focused {
                0xe6c278
            } else if stop_enabled {
                0x91464a
            } else {
                0x51565d
            }))
            .bg(rgb(if stop_enabled { 0x5b2a2f } else { 0x313438 }))
            .text_color(rgb(if stop_enabled { 0xf4dfe0 } else { 0x9da3aa }))
            .child(if stop_enabled && retryable_admission {
                "新しい停止要求を送る"
            } else {
                "タスクを停止"
            });
        let stop_button = if stop_enabled {
            stop_button
                .cursor_pointer()
                .on_action(cx.listener(|this, _: &Activate, window, cx| {
                    this.start_stop(window, cx);
                }))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.start_stop(window, cx);
                }))
        } else {
            stop_button
        };
        let reconcile_focused = self.reconcile_focus.is_focused(window);
        let reconcile_description: Arc<str> = if reconcile_enabled {
            "停止要求の受付状況だけを確認します。停止要求の送信や再送は行いません。".into()
        } else {
            Arc::clone(&reconcile_disabled_reason)
        };
        let reconcile_button = div()
            .id("stop-reconcile")
            .role(Role::Button)
            .aria_label(if reconcile_enabled {
                "停止要求の受付状況を確認"
            } else {
                "停止要求の受付状況を確認（現在は操作できません）"
            })
            .aria_keyshortcuts("Enter Space")
            .aria_description(SharedString::new(Arc::clone(&reconcile_description)))
            .tooltip({
                let text = SharedString::new(Arc::clone(&reconcile_description));
                let style = xs_text.clone();
                move |_, cx| help::tooltip(text.clone(), style.clone(), inspector_width, cx)
            })
            .track_focus(&self.reconcile_focus)
            .px_4()
            .py_2()
            .border_1()
            .border_color(rgb(if reconcile_focused {
                0xe6c278
            } else if reconcile_enabled {
                0x626870
            } else {
                0x51565d
            }))
            .bg(rgb(if reconcile_enabled {
                0x343a42
            } else {
                0x313438
            }))
            .text_color(rgb(if reconcile_enabled {
                0xe6e8eb
            } else {
                0x9da3aa
            }))
            .child("停止の受付状況");
        let reconcile_button = if reconcile_enabled {
            reconcile_button
                .cursor_pointer()
                .on_action(cx.listener(|this, _: &Activate, window, cx| {
                    this.start_receipt_check(window, cx);
                }))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.start_receipt_check(window, cx);
                }))
        } else {
            reconcile_button
        };
        let stop_controls = div()
            .id("stop-controls")
            .role(Role::Group)
            .aria_label("停止の操作と現在の受付状況")
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap_2()
                    .child(accessibility::DisabledState {
                        inner: stop_button,
                        disabled: !stop_enabled,
                    })
                    .child(accessibility::DisabledState {
                        inner: reconcile_button,
                        disabled: !reconcile_enabled,
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        xs_text
                            .clone()
                            .apply(div())
                            .id("stop-status")
                            .role(Role::Label)
                            .aria_label(stop_label.clone())
                            .aria_value(stop_label.clone())
                            .aria_description(stop_detail.clone())
                            .tooltip({
                                let text = stop_detail.clone();
                                let style = xs_text.clone();
                                move |_, cx| {
                                    help::tooltip(text.clone(), style.clone(), inspector_width, cx)
                                }
                            })
                            .text_color(rgb(0xe6c278))
                            .child(stop_label),
                    )
                    .when(manual_intervention_required, |element| {
                        element
                            .child(
                                xs_text
                                    .clone()
                                    .apply(div())
                                    .id("stop-detail")
                                    .role(Role::Label)
                                    .aria_label(stop_detail.clone())
                                    .aria_value(stop_detail.clone())
                                    .child(stop_detail),
                            )
                            .child("自動で停止を再送しません。状況の確認が必要です。")
                    })
                    .children(retention_warning.map(|display| {
                        div()
                            .text_color(rgb(0xe6c278))
                            .child(SharedString::from(display.label.clone()))
                            .child(SharedString::from(display.detail.clone()))
                    }))
                    .children(receipt_note.map(|(label, detail)| {
                        xs_text
                            .clone()
                            .apply(div())
                            .id("stop-receipt-note")
                            .role(Role::Label)
                            .aria_label(SharedString::from(label))
                            .aria_description(detail)
                            .tooltip({
                                let style = xs_text.clone();
                                move |_, cx| {
                                    help::tooltip(detail.into(), style.clone(), inspector_width, cx)
                                }
                            })
                            .text_color(rgb(0xbcc6d0))
                            .child(SharedString::from(label))
                            .child(SharedString::from(detail))
                    })),
            );
        self.dock_frame = self.dock_frame.saturating_add(1);
        let dock_frame = self.dock_frame;
        let previous_dock_mounts = self.dock_mounts.swap(0, Ordering::Relaxed);
        let dock_mounts = Arc::clone(&self.dock_mounts);
        let dock_trace = self.dock_trace_enabled;
        let dock_trace_lines = Arc::clone(&self.dock_trace_lines);
        // The native ScrollHandle is the only scroll-position authority. GPUI
        // translates content by a negative x offset when moving right; overscan
        // changes only the mounted slice, never the viewport's position.
        let dock_row_width = px(dock::ROW_WIDTH);
        let dock_count = artifacts.len();
        let dock_viewport = f32::from(self.candidate_scroll.bounds().size.width);
        let dock_admitted = authorized_text && !self.rail_selection_pending;
        if dock_admitted && !Arc::ptr_eq(&artifacts, &self.rendered_candidates) {
            let previous_active_index = self.dock_active.as_ref().and_then(|active| {
                self.rendered_candidates
                    .iter()
                    .position(|row| row.identity == *active)
            });
            if let Some((identity, relative)) = &self.dock_anchor
                && let Some(index) = artifacts.iter().position(|row| row.identity == *identity)
            {
                let offset = dock::clamp_offset(
                    dock_count,
                    dock_viewport,
                    -(index as f32 * dock::STRIDE + f32::from(*relative)),
                );
                self.candidate_scroll
                    .set_offset(gpui::point(px(offset), px(0.0)));
            }
            let active_still_present = self
                .dock_active
                .as_ref()
                .is_some_and(|active| artifacts.iter().any(|row| row.identity == *active));
            if self.dock_active.is_none() {
                self.dock_active = viewed
                    .as_ref()
                    .and_then(|identity| artifacts.iter().find(|row| row.identity == *identity))
                    .or_else(|| artifacts.first())
                    .map(|row| row.identity.clone());
            } else if !active_still_present {
                // D-12: focus/cursor moves to the nearest valid row; selection is
                // untouched by this reconciliation.
                self.dock_active = dock::nearest_index(previous_active_index, dock_count)
                    .and_then(|index| artifacts.get(index))
                    .map(|row| row.identity.clone());
            }
        }
        // Drop withheld metadata, not the user's local identity/scroll intent.
        self.rendered_candidates = Arc::clone(&artifacts);
        let dock_window = dock::DockWindow::new(
            dock_count,
            dock_viewport,
            f32::from(self.candidate_scroll.offset().x),
        );
        let dock_first = dock_window.first;
        let dock_end = dock_window.end;
        let dock_top = dock_window.visible;
        if dock_admitted {
            self.dock_anchor = artifacts.get(dock_top).map(|row| {
                (
                    row.identity.clone(),
                    px(dock_window.scroll - dock_top as f32 * dock::STRIDE),
                )
            });
        }
        let dock_active = self.dock_active.clone();
        // A dock row takes keyboard focus, so the container owns the ring: it is
        // lit while focus is anywhere inside the Dock, not only on the container.
        let dock_focused =
            self.dock_focus.is_focused(window) || self.dock_focus.contains_focused(window, cx);
        // Keyboard focus is one indicator: the approved outline on the focusable
        // Dock container. Row borders stay structural at one physical pixel.
        let (dock_focus_width, dock_focus_color) =
            shell_style::dock_focus_ring(&self.tokens, dock_focused);
        let row_border = shell_style::dock_row_border(&self.tokens, window.scale_factor()).ok();
        // Every row carries its authoritative iteration label; the first row of
        // a group is additionally marked as a group boundary, so a bounded window
        // deep inside a long iteration still shows which iteration it belongs to.
        let dock_group_of = |index: usize| -> (Arc<str>, Arc<str>, bool) {
            let slot = candidate_groups.partition_point(|group| group.start <= index);
            candidate_groups
                .get(slot.saturating_sub(1))
                .filter(|group| index < group.start + group.rows.len())
                .map(|group| {
                    (
                        Arc::clone(&group.label),
                        Arc::clone(&group.short_label),
                        index == group.start,
                    )
                })
                .unwrap_or_else(|| ("反復情報なし".into(), "反復 ?".into(), false))
        };
        let candidate_help = match changing_side {
            Some(CompareSide::A) => {
                "この候補で比較するA側を置き換えます。閲覧中の候補は変わりません。"
            }
            Some(CompareSide::B) => {
                "この候補で比較するB側を置き換えます。閲覧中の候補は変わりません。"
            }
            None if compare_pair.is_some() => {
                "この候補を閲覧します。比較対象と正式な採用結果は変わりません。"
            }
            None if pending_side_a.is_some() => {
                "1つ目と異なる候補を選ぶと比較対象がそろいます。正式な採用ではありません。"
            }
            None => {
                "この候補を閲覧し、比較する1つ目の候補にします。別の候補を選ぶと比較対象がそろいます。"
            }
        };
        let candidate_rows = artifacts
            .iter()
            .enumerate()
            .skip(dock_first)
            .take(dock_end.saturating_sub(dock_first))
            .map(|(index, candidate)| {
                let identity = candidate.identity.clone();
                let label = candidate.label.clone();
                let selected = viewed.as_ref() == Some(&identity);
                let active = dock_active.as_ref() == Some(&identity);
                let compared =
                    compare_pair.is_some_and(|pair| pair.iter().any(|side| side.0 == identity));
                let formal = formal_candidates
                    .iter()
                    .any(|reference| reference == &identity);
                let state_label = match (selected, compared, active) {
                    (true, true, true) => "閲覧中・比較対象・キーボード操作対象",
                    (true, true, false) => "閲覧中・比較対象",
                    (true, false, true) => "閲覧中・キーボード操作対象",
                    (false, true, true) => "比較対象・キーボード操作対象",
                    (true, false, false) => "閲覧中",
                    (false, true, false) => "比較対象",
                    (false, false, true) => "キーボード操作対象",
                    (false, false, false) => "",
                };
                let mounted = dock_mounts.fetch_add(1, Ordering::Relaxed) + 1;
                trace::emit(
                    dock_trace,
                    &dock_trace_lines,
                    trace::MAX_DOCK_LINES,
                    "candidate_dock_mount",
                    || {
                        format!(
                            "frame={} mounted={} index={} candidate_id={} viewed={} active={}",
                            dock_frame,
                            mounted,
                            index,
                            identity.candidate_id.as_str(),
                            selected,
                            active
                        )
                    },
                );
                let activate_identity = identity.clone();
                let (group_label, group_short, group_start) = dock_group_of(index);
                trace::emit(
                    dock_trace,
                    &dock_trace_lines,
                    trace::MAX_DOCK_LINES,
                    "candidate_dock_group",
                    || {
                        format!(
                            "frame={} index={} task_id={} candidate_id={} group_start={} group={}",
                            dock_frame,
                            index,
                            identity.task_id.as_str(),
                            identity.candidate_id.as_str(),
                            group_start,
                            group_short.as_ref()
                        )
                    },
                );
                div()
                    .id(SharedString::from(candidate.element_key.clone()))
                    .role(Role::Button)
                    .accessibility_id(SharedString::from(candidate.element_key.clone()))
                    .aria_label(SharedString::from(format!(
                        "{} — {} — {} {}{}",
                        identity.task_id.as_str(),
                        group_label,
                        label,
                        state_label,
                        if formal {
                            " 正式採用（実行基盤から通知）"
                        } else {
                            ""
                        }
                    )))
                    .aria_description(candidate_help)
                    .tooltip({
                        let style = xs_text.clone();
                        move |_, cx| {
                            help::tooltip(candidate_help.into(), style.clone(), inspector_width, cx)
                        }
                    })
                    .when(active && dock_focused, |element| {
                        element.aria_active_descendant()
                    })
                    .w(dock_row_width)
                    .when(index + 1 < dock_count, |element| element.mr(px(dock::GAP)))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_2()
                    .py_2()
                    .when_some(row_border, |element, width| element.border(width))
                    .border_color(rgb(shell_style::dock_row_color(selected)))
                    .cursor_pointer()
                    .child(
                        div()
                            .w(px(DOCK_GROUP_GUTTER_PX))
                            .flex_none()
                            .when_some(hairline, |element, width| element.border_r(width))
                            .border_color(rgb(if group_start { 0xaeccec } else { 0x3c4046 }))
                            .px_1()
                            .truncate()
                            .child(SharedString::from(group_short)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_1()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(SharedString::from(format!(
                                "{}{}",
                                if group_start { "▸ " } else { "" },
                                label
                            ))))
                            .child(xs_text.clone().apply(div()).truncate().child(state_label))
                            .when(formal, |element| {
                                element.child(
                                    xs_text
                                        .clone()
                                        .apply(div())
                                        .text_color(rgb(0xe6c278))
                                        .truncate()
                                        .child(SharedString::from("正式採用（実行基盤から通知）")),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.view_candidate(activate_identity.clone(), window, cx);
                    }))
            })
            .collect::<Vec<_>>();
        trace::emit(
            dock_trace,
            &dock_trace_lines,
            trace::MAX_DOCK_LINES,
            "candidate_dock_state",
            || {
                format!(
                    "frame={} previous_frame_mount_callbacks={} logical_count={} window_start={} window_end={} window_top={} offset_px={} anchor={} active={} viewed={} admitted={} display_version={} live_version={} viewport_px={} anchor_offset_px={}",
                    dock_frame,
                    previous_dock_mounts,
                    dock_count,
                    dock_first,
                    dock_end,
                    dock_top,
                    f32::from(self.candidate_scroll.offset().x),
                    self.dock_anchor
                        .as_ref()
                        .map(|(identity, _)| identity.candidate_id.as_str())
                        .unwrap_or("none"),
                    dock_active
                        .as_ref()
                        .map(|identity| identity.candidate_id.as_str())
                        .unwrap_or("none"),
                    viewed
                        .as_ref()
                        .map(|identity| identity.candidate_id.as_str())
                        .unwrap_or("none"),
                    dock_admitted,
                    text.publication_version,
                    live.map_or(0, |notice| notice.publication_version),
                    dock_viewport,
                    self.dock_anchor
                        .as_ref()
                        .map_or(0.0, |(_, offset)| f32::from(*offset)),
                )
            },
        );
        let dock_left_spacer = px(dock_window.left);
        let dock_right_spacer = px(dock_window.right);
        self.rail_state.revalidate(live);
        if let (Some(search), Some(collection)) =
            (self.rail_state.results(), self.rail_state.collection())
        {
            let offset = self.rail_list.logical_scroll_top();
            self.rail_anchor = search
                .indices
                .get(offset.item_ix)
                .and_then(|row_index| collection.rows.get(*row_index))
                .map(|row| (row.identity.clone(), offset.offset_in_item));
        }
        let rail_collection = self.rail_state.collection().cloned();
        let rail_search = self.rail_state.results().cloned();
        let rail_count = rail_search
            .as_ref()
            .map_or(0, |search| search.indices.len());
        let intervention_strip = self.intervention_strip(intervention_strip, window, cx);
        let inspector_panel = self.inspector_panel(
            inspector_view,
            viewed_label.clone(),
            viewed_row.is_some(),
            inspector::PanelScope {
                task: inspector_task.as_ref(),
                withheld: inspector_withheld,
            },
            window,
            cx,
        );
        let query_text = self.rail_state.query().to_owned();
        let query_display =
            if self.rail_input_focus.is_focused(window) && self.rail_input_selection.is_empty() {
                format!(
                    "{}│{}",
                    &query_text[..self.rail_input_selection.start],
                    &query_text[self.rail_input_selection.end..],
                )
            } else {
                query_text.to_owned()
            };
        let rail_active = self.rail_active.clone();
        let rail_selected = self.rail_state.selected().cloned();
        let rail_mounts = self.rail_mounts.clone();
        let trace_rail = self.rail_trace_enabled;
        let previous_mounts = self.rail_mounts.swap(0, Ordering::Relaxed);
        self.rail_frame = self.rail_frame.saturating_add(1);
        let rail_frame = self.rail_frame;
        let rail_focused = self.rail_focus.is_focused(window);
        let shell_entity = cx.entity();
        let rail_trace_lines = self.rail_trace_lines.clone();
        let rail_list = rail_collection.clone().zip(rail_search.clone()).map(|(collection, search)| {
            let xs_text = xs_text.clone();
            list(self.rail_list.clone(), move |index, _window, _app| {
                let Some(row_index) = search.indices.get(index).copied() else {
                    return div().into_any_element();
                };
                let Some(row) = collection.rows.get(row_index).cloned() else {
                    return div().into_any_element();
                };
                let task = row.identity.clone();
                let stable_id = SharedString::from(format!("task-row-{}", task.as_str()));
                let active = rail_active.as_ref() == Some(&task);
                let selected = rail_selected.as_ref() == Some(&task);
                let mounted = rail_mounts.fetch_add(1, Ordering::Relaxed) + 1;
                trace::emit(
                    trace_rail,
                    &rail_trace_lines,
                    trace::MAX_RAIL_LINES,
                    "task_rail_mount",
                    || {
                        format!(
                            "frame={} mounted={} task_id={} active={} selected={} query_present={} result_index={}",
                            rail_frame,
                            mounted,
                            task.as_str(),
                            active,
                            selected,
                            !search.query.is_empty(),
                            index
                        )
                    },
                );
                let identity = task.clone();
                div()
                    .id(stable_id.clone())
                    .accessibility_id(stable_id)
                    .role(Role::ListItem)
                    .aria_selected(selected)
                    .aria_label(SharedString::new(format!("{} — {}", row.display_id, row.title)))
                    .when(active && rail_focused, |element| element.aria_active_descendant())
                    .children(
                        [
                            selected.then_some((
                                "選択中",
                                format!("task-row-marker-selected-{}", task.as_str()),
                            )),
                            active.then_some((
                                "キーボード操作対象",
                                format!("task-row-marker-active-{}", task.as_str()),
                            )),
                        ]
                        .into_iter()
                        .flatten()
                        .map(|(text, id)| {
                            xs_text
                                .clone()
                                .apply(div())
                                .id(id)
                                .role(Role::Label)
                                .aria_label(text)
                                .aria_value(text)
                                .text_color(rgb(0xe6e8eb))
                                .child(text)
                        }),
                    )
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(rgb(if active && rail_focused { 0xe6c278 } else if selected { 0xaeccec } else { 0x626870 }))
                    .bg(rgb(if selected { 0x343a42 } else { 0x25292e }))
                    .child(SharedString::new(Arc::clone(&row.display_id)))
                    .child(SharedString::new(Arc::clone(&row.title)))
                    .child(SharedString::from(format!(
                        "{}・処理段階 {}・反復 {}",
                        task_status_label(&row.task_status),
                        row.inner_phase,
                        row.iteration_index.map_or("情報なし".to_owned(), |value| value.to_string())
                    )))
                    .children(
                        [
                            row.attention.stale_connection.then_some("接続情報が古い状態です"),
                            row.attention.unsettled_command.then_some("操作の結果が未確定です"),
                            row.attention.unavailable_reference.then_some("参照先を利用できません"),
                        ]
                        .into_iter()
                        .flatten()
                        .map(|label| {
                            xs_text
                                .clone()
                                .apply(div())
                                .child(format!("この画面からの案内：{label}"))
                        }),
                    )
                    .on_click({
                        let shell_entity = shell_entity.clone();
                        move |_, window, app| {
                            shell_entity.update(app, |this, cx| this.select_rail_task(&identity, window, cx));
                        }
                    })
                    .into_any_element()
            })
        });
        let rail_state_diagnostic = self.rail_list.logical_scroll_top();
        let rail_anchor = rail_collection
            .as_ref()
            .and_then(|collection| {
                rail_search.as_ref().and_then(|search| {
                    search
                        .indices
                        .get(rail_state_diagnostic.item_ix)
                        .and_then(|row_index| collection.rows.get(*row_index))
                        .map(|row| row.identity.as_str().to_owned())
                })
            })
            .unwrap_or_else(|| "none".to_owned());
        let result_notice = rail_search.as_ref().and(live);
        trace::emit(
            trace_rail,
            &self.rail_trace_lines,
            trace::MAX_RAIL_LINES,
            "task_rail_state",
            || {
                format!(
                    "frame={} previous_frame_mount_callbacks={} logical_count={} result_count={} anchor_task={} offset_px={:?} selected={} active={} query_generation={} result_publication={}:{} query_present={}",
                    rail_frame,
                    previous_mounts,
                    self.rail_state
                        .collection()
                        .map_or(0, |collection| collection.rows.len()),
                    rail_count,
                    rail_anchor,
                    rail_state_diagnostic.offset_in_item,
                    self.rail_state.selected().map_or("none", TaskId::as_str),
                    self.rail_active.as_ref().map_or("none", TaskId::as_str),
                    self.rail_state.query_generation(),
                    result_notice.map_or(0, |notice| notice.session_generation),
                    result_notice.map_or(0, |notice| notice.publication_version),
                    rail_search
                        .as_ref()
                        .is_some_and(|search| !search.query.is_empty())
                )
            },
        );

        let palette_overlay = self.palette_overlay(&palette_facts, cx);

        div()
            .id("cos-shell")
            .role(Role::Application)
            .aria_label("COS Desktop 作業画面")
            .track_focus(&self.focus)
            .relative()
            .on_action(cx.listener(|this, _: &PaletteToggle, window, cx| {
                this.palette_toggle(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &Artifact, _, cx| this.navigate(Surface::Artifact, cx)),
            )
            .on_action(cx.listener(|this, _: &Compare, _, cx| this.navigate(Surface::Compare, cx)))
            .on_action(cx.listener(|this, _: &Flow, _, cx| this.navigate(Surface::Flow, cx)))
            .on_action(cx.listener(|this, _: &NextSurface, _, cx| {
                this.advance_surface(cx);
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
            .text_size(body_text.size)
            .line_height(body_text.line_height)
            .font_weight(body_text.weight)
            .font(body_text.font.clone())
            .child(
                div()
                    .id("context-bar")
                    .role(Role::Region)
                    .aria_label("現在のタスクと状態")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_5()
                    .py_2()
                    .bg(rgb(0x24272c))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().flex_none().child("COS"))
                            .child(
                                div()
                                    .id("context-title")
                                    .role(Role::Label)
                                    .aria_label(if self.rail_selection_pending {
                                        SharedString::from("選択したタスクを更新中…")
                                    } else {
                                        title.clone()
                                    })
                                    .aria_value(if self.rail_selection_pending {
                                        SharedString::from("選択したタスクを更新中…")
                                    } else {
                                        title.clone()
                                    })
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(if self.rail_selection_pending {
                                        "選択したタスクを更新中…".into()
                                    } else {
                                        title.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .id("observation-source")
                                    .role(Role::Label)
                                    .aria_label(SharedString::from(mode.clone()))
                                    .aria_value(SharedString::from(mode.clone()))
                                    .flex_none()
                                    .child(mode),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .text_color(rgb(0xbcc6d0))
                            .child(axis_row_one)
                            .child(axis_row_two),
                    )
                    .child(stop_controls),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("task-rail")
                            .role(Role::Region)
                            .aria_label("タスク一覧")
                            .w(rail_width)
                            .flex_none()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .bg(rgb(0x202328))
                            .child("タスク一覧")
                            .child(
                                div()
                                    .id("task-search")
                                    .role(Role::TextInput)
                                    .accessibility_id("task-search")
                                    .aria_label("タスクを検索")
                                    .aria_value(SharedString::from(query_text.clone()))
                                    .key_context("TaskRailInput")
                                    .track_focus(&self.rail_input_focus)
                                    .flex()
                                    .items_center()
                                    .px_2()
                                    .py_1()
                                    .border_1()
                                    .border_color(rgb(0x626870))
                                    .bg(rgb(0x17191c))
                                    .relative()
                                    .child(SharedString::from(if query_text.is_empty()
                                        && !self.rail_input_focus.is_focused(window)
                                    {
                                        "タスクを検索…".to_owned()
                                    } else {
                                        query_display
                                    }))
                                    .child(
                                        div()
                                            .absolute()
                                            .size_full()
                                            .child(SearchInputRegistration { entity: cx.entity() }),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| this.rail_input_focus.focus(window, cx)))
                                    .on_action(cx.listener(|_, _: &Activate, _, _| {}))
                                    .on_action(cx.listener(|_, _: &RailEnter, _, _| {}))
                                    .on_action(cx.listener(|this, _: &RailBackspace, _, cx| this.rail_backspace(cx)))
                                    .on_action(cx.listener(|this, _: &RailDelete, _, cx| this.rail_delete(cx)))
                                    .on_action(cx.listener(|this, _: &RailLeft, _, cx| this.rail_move_cursor(-1, cx)))
                                    .on_action(cx.listener(|this, _: &RailRight, _, cx| this.rail_move_cursor(1, cx)))
                                    .on_action(cx.listener(|this, _: &RailSelectAll, _, cx| {
                                        this.rail_input_selection = 0..this.rail_state.query().len();
                                        cx.notify();
                                    }))
                                    .on_action(cx.listener(|_, _: &RailSpace, _, _| {})),
                            )
                            .child(
                                div()
                                    .id("task-rail-list")
                                    .role(Role::List)
                                    .key_context("TaskRailList")
                                    .track_focus(&self.rail_focus)
                                    .flex_1()
                                    .min_h_0()
                                    .when_some(rail_list, |element, list| element.child(list.size_full()))
                                    .on_action(cx.listener(|this, _: &RailUp, window, cx| {
                                        this.move_rail_active(-1, window, cx);
                                    }))
                                    .on_action(cx.listener(|this, _: &RailDown, window, cx| {
                                        this.move_rail_active(1, window, cx);
                                    }))
                                    .on_action(cx.listener(|this, _: &RailEnter, window, cx| {
                                        if let Some(task) = this.rail_active.clone() {
                                            this.select_rail_task(&task, window, cx);
                                        }
                                    }))
                                    .on_action(cx.listener(|_, _: &RailSpace, _, _| {}))
                                    .on_action(cx.listener(|_, _: &Activate, _, _| {}))
                            )
                            .when(rail_search.is_some() && rail_count == 0 && !self.rail_state.query().trim().is_empty(), |element| {
                                element.child(SharedString::from(format!("「{}」に一致するタスクはありません。", self.rail_state.query())))
                            })
                            .when(rail_collection.is_some() && rail_search.is_none(), |element| {
                                element.child("タスクの検索結果を更新中…")
                            })
                            .when(rail_collection.as_ref().is_some_and(|collection| collection.rows.is_empty()) && self.rail_state.query().trim().is_empty(), |element| {
                                element.child("受信したタスクはありません")
                            })
                            .when(self.rail_state.collection().is_some_and(|collection| collection.scope_has_unsettled_commands), |element| {
                                element.child("この画面からの案内：結果が未確定の操作があります。対象のタスクは推測しません。")
                            })
                            .when(rail_collection.is_none(), |element| {
                                element.child("現在閲覧できるタスク一覧がないため、情報を表示していません")
                            }),
                    )
                    .child(
                        div()
                            .id("work-surface")
                            .aria_label(SharedString::from(format!(
                                "作業領域：{}",
                                surface.label()
                            )))
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .child("作業領域")
                            .child(intervention_strip)
                            .when_some(surface_notice.clone(), |element, notice| {
                                element.child(
                                    div()
                                        .id("surface-notice")
                                        .role(Role::Label)
                                        .aria_label(SharedString::from(notice.clone()))
                                        .aria_value(SharedString::from(notice.clone()))
                                        .text_color(rgb(0xe6c278))
                                        .child(SharedString::from(notice)),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    .when(surface == Surface::Flow, |element| {
                                        element.child(flow_panel)
                                    })
                                    .when(surface == Surface::Compare, |element| {
                                        element.child(compare_panel)
                                    })
                                    .when(
                                        surface != Surface::Flow && surface != Surface::Compare,
                                        |element| {
                                            element.child(
                                                div()
                                                    .w_full()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(div().w_full().min_w_0().child(surface_content)),
                                            )
                                        },
                                    ),
                            )
                            .children(
                                (surface == Surface::Artifact).then(|| div().child(viewed_label)),
                            )
                    )
                    .child(
                        div()
                            .id("inspector")
                            .role(Role::Region)
                            .aria_label("候補の詳細")
                            .w(inspector_width)
                            .flex_none()
                            .h_full()
                            .min_h_0()
                            .overflow_hidden()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .bg(rgb(0x202328))
                            .child(div().flex_none().child("候補の詳細"))
                            .child(
                                div()
                                    .id("inspector-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_3()
                                            .when_some(self.restore_notice.clone(), |element, notice| {
                                                element.child(
                                                    div()
                                                        .id("restore-notice")
                                                        .role(Role::Label)
                                                        .aria_label(SharedString::from(notice.clone()))
                                                        .aria_value(SharedString::from(notice.clone()))
                                                        .child(SharedString::from(notice)),
                                                )
                                            })
                                            .child(inspector_panel)
                                            .child(if diagnostics_open {
                                                diagnostic_text
                                            } else {
                                                "診断情報：Ctrl+D".into()
                                            }),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .id("loop-dock")
                    .role(Role::Region)
                    .aria_label("反復ごとの候補一覧")
                    .p_4()
                    .bg(rgb(0x272b30))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .id("dock-ownership")
                            .role(Role::Label)
                            .aria_label(
                                "反復ごとの候補一覧。閲覧中と比較対象はこの画面の選択です。正式採用は実行基盤から通知されます。",
                            )
                            .aria_value(
                                "反復ごとの候補一覧。閲覧中と比較対象はこの画面の選択です。正式採用は実行基盤から通知されます。",
                            )
                            .child(
                                "反復ごとの候補 — 閲覧中・比較対象はこの画面での選択です。正式採用とは異なります。",
                            ),
                    )
                    .child(
                        div()
                            .id("candidate-rail")
                            .role(Role::Group)
                            .aria_label("候補一覧")
                            .key_context("CandidateDock")
                            .track_focus(&self.dock_focus)
                            .border(dock_focus_width)
                            .border_color(rgb(dock_focus_color))
                            .overflow_x_scroll()
                            .track_scroll(&self.candidate_scroll)
                            .flex()
                            .on_action(cx.listener(|this, _: &DockNext, window, cx| {
                                this.move_dock_active(1, window, cx);
                            }))
                            .on_action(cx.listener(|this, _: &DockPrevious, window, cx| {
                                this.move_dock_active(-1, window, cx);
                            }))
                            .on_action(cx.listener(|this, _: &DockActivate, window, cx| {
                                this.activate_dock_active(window, cx);
                            }))
                            .children((dock_count == 0).then(|| {
                                let message = if dock_admitted {
                                    "現在閲覧できる情報に候補はありません"
                                } else {
                                    "閲覧が認可された最新の情報を待っているため、候補を表示していません"
                                };
                                div().id("candidate-dock-availability").role(Role::Label)
                                    .aria_label(message).aria_value(message).p_2().child(message)
                            }))
                            .child(div().w(dock_left_spacer).flex_none())
                            .children(candidate_rows)
                            .child(div().w(dock_right_spacer).flex_none()),
                    ),
            )
            .child(
                div()
                    .id("surface-switcher")
                    .role(Role::Group)
                    .aria_label("作業画面の切り替え")
                    .p_3()
                    .flex()
                    .gap_3()
                    .children(Surface::ALL.into_iter().enumerate().map(|(index, item)| {
                        let focused = self.surface_focus[index].is_focused(window);
                        let compare_unavailable = item == Surface::Compare
                            && dock::compare_unavailable_reason(artifacts.len()).is_some();
                        let label = if compare_unavailable {
                            format!("{} — 候補を2つ選ぶ必要があります", item.label())
                        } else {
                            format!("{}  Ctrl+{}", item.label(), index + 1)
                        };
                        let description = match item {
                            Surface::Artifact => "閲覧中の候補を扱う画面に切り替えます。",
                            Surface::Compare if compare_unavailable => {
                                "2つの候補を比較する画面です。現在は候補が2つに満たないため開けません。"
                            }
                            Surface::Compare => "2つの候補を比較する画面に切り替えます。比較対象は正式な採用結果とは異なります。",
                            Surface::Flow => "処理のつながりを必要なときだけ確認する画面に切り替えます。",
                        };
                        div()
                            .id(("surface", index))
                            .role(Role::Button)
                            .aria_label(SharedString::from(label.clone()))
                            .aria_description(description)
                            .tooltip({
                                let style = xs_text.clone();
                                move |_, cx| help::tooltip(description.into(), style.clone(), inspector_width, cx)
                            })
                            .track_focus(&self.surface_focus[index])
                            .px_5()
                            .py_2()
                            .border(focus_outline)
                            .border_color(rgb(if focused { 0xe6c278 } else { 0x343940 }))
                            .bg(rgb(if surface == item { 0x3c5268 } else { 0x24272c }))
                            .when(compare_unavailable, |element| {
                                element.text_color(rgb(0x8b949e))
                            })
                            .when(!compare_unavailable, |element| element.cursor_pointer())
                            .child(SharedString::from(label))
                            .when(!compare_unavailable, |element| {
                                element.on_click(cx.listener(move |this, _, window, cx| {
                                    this.surface_focus[index].focus(window, cx);
                                    this.navigate(item, cx);
                                }))
                            })
                    })),
            )
            .child(palette_overlay)
    }
}

/// IME composition contract at the shell's text-input boundary (DC-MP-AX1 / AX05).
///
/// The handler receives UTF-16 ranges while the query is UTF-8, so a composition
/// must translate ranges, splice marked text and keep the caret consistent.
/// These tests call the same helpers the handler uses, so a regression in the
/// conversion or splice fails here without a window or a real IME.
#[cfg(test)]
mod ime_tests {
    use super::{byte_offset, ime_byte_range, ime_splice, utf16_offset};

    const JAPANESE: &str = "画像の候補";
    const ASTRAL: &str = "a🖼b";

    #[test]
    fn byte_and_utf16_offsets_round_trip_across_scripts() {
        for text in [JAPANESE, ASTRAL, "plain ascii", "mañana", "日本a語"] {
            for boundary in 0..=text.len() {
                if !text.is_char_boundary(boundary) {
                    continue;
                }
                let units = utf16_offset(text, boundary);
                assert_eq!(
                    byte_offset(text, units),
                    boundary,
                    "round trip failed for {text:?} at byte {boundary}"
                );
            }
            assert_eq!(
                byte_offset(text, utf16_offset(text, text.len())),
                text.len()
            );
        }
    }

    #[test]
    fn a_surrogate_pair_range_never_splits_a_character() {
        // The emoji occupies bytes 1..5 and UTF-16 units 1..3.
        assert_eq!(byte_offset(ASTRAL, 1), 1);
        assert_eq!(byte_offset(ASTRAL, 3), 5);
        let range = ime_byte_range(ASTRAL, Some(1..3), None, &(0..0));
        assert_eq!(range, 1..5);
        let (text, caret) = ime_splice(ASTRAL, &range, "x");
        assert_eq!(text, "axb");
        assert_eq!(caret, 2);
        assert!(text.is_char_boundary(caret));
    }

    #[test]
    fn composition_marks_then_commits_japanese_text() {
        // First composition event: nothing is selected or marked yet.
        let range = ime_byte_range("", None, None, &(0..0));
        let (text, caret) = ime_splice("", &range, "画");
        assert_eq!((text.as_str(), caret), ("画", 3));
        let marked = Some(range.start..range.start + "画".len());

        // Second event: the IME replaces the marked range (UTF-16 units 0..1).
        let range = ime_byte_range("画", Some(0..1), marked, &(caret..caret));
        let (text, caret) = ime_splice("画", &range, "画像");
        assert_eq!((text.as_str(), caret), ("画像", 6));
        assert_eq!(utf16_offset(&text, caret), 2);

        // Commit: the marked range is replaced by the same committed text.
        let marked = Some(0.."画像".len());
        let range = ime_byte_range("画像", None, marked, &(caret..caret));
        let (text, caret) = ime_splice("画像", &range, "画像");
        assert_eq!((text.as_str(), caret), ("画像", 6));
    }

    #[test]
    fn cancelling_composition_removes_only_the_marked_text() {
        let current = JAPANESE;
        // UTF-16 units 1..3 are the second and third characters (bytes 3..9).
        let range = ime_byte_range(current, Some(1..3), None, &(0..0));
        assert_eq!(&current[range.clone()], "像の");
        let (text, caret) = ime_splice(current, &range, "");
        assert_eq!(text, "画候補");
        assert_eq!(caret, 3);
        assert!(text.is_char_boundary(caret));
        assert_eq!(utf16_offset(&text, caret), 1);
    }

    #[test]
    fn replacement_respects_utf16_ranges_after_multibyte_edits() {
        let current = "a画像b";
        // UTF-16 units 1..3 select the two Japanese characters (bytes 1..7).
        let range = ime_byte_range(current, Some(1..3), None, &(0..0));
        assert_eq!(&current[range.clone()], "画像");
        let (text, caret) = ime_splice(current, &range, "語");
        assert_eq!(text, "a語b");
        assert_eq!(caret, 4);
        assert_eq!(byte_offset(&text, utf16_offset(&text, caret)), caret);

        // A range that spans the trailing ASCII character selects exactly that text.
        let range = ime_byte_range(current, Some(2..4), None, &(0..0));
        assert_eq!(&current[range.clone()], "像b");
        let (text, caret) = ime_splice(current, &range, "語");
        assert_eq!(text, "a画語");
        assert_eq!(caret, 7);
    }
}
