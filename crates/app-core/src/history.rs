//! Bounded task history publication (DC-MP-NAV3).
//!
//! The owner thread performs one authenticated keyset read at a time and
//! publishes a bounded, immutable view of the pages it has actually received.
//! This is presentation state, never a canonical history store:
//!
//! * only run-level fields the Runtime projected are kept;
//! * the view is invalidated on every scope/authorization/client change, so a
//!   previous context's rows can never be rendered under a new one;
//! * a page result that no longer matches the current request sequence is
//!   discarded instead of being published as the current view;
//! * the entries are bounded and a further page is never appended past the
//!   bound, so "load older" stops at an explicit bound instead of silently
//!   truncating the timeline into a false end.
//!
//! `Idle` means "never loaded for this task"; it is deliberately distinct from
//! `Empty`, which only an authoritative served page can produce.

use std::sync::Arc;

use cos_client::{HistoryCursor, HistoryDocument, HistoryFetchError, HistoryItem, HistoryQuery};
use desktop_protocol::TaskId;

/// Maximum run-level entries one published view keeps.
pub const HISTORY_MAX_ENTRIES: usize = 200;
/// Page size requested for both the first page and every continuation.
pub const HISTORY_PAGE_LIMIT: usize = cos_client::HISTORY_LIMIT_DEFAULT;

/// Closed value-free history failure classes kept in the publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryError {
    /// The task is gone or outside the granted scope (`404`).
    NotFound,
    /// The observation binding must be re-established (`409`).
    Conflict,
    /// The Runtime refused the limit or cursor (`422`).
    InvalidRequest,
    /// The served document did not match the closed contract.
    Malformed,
    /// The served document exceeded the bounded body size.
    Oversized,
    /// The exchange failed (I/O, timeout).
    Unavailable,
    /// No authenticated history read exists in this mode.
    Unsupported,
}

impl HistoryError {
    fn from_fetch_error(error: HistoryFetchError) -> Self {
        match error {
            HistoryFetchError::NotFound => Self::NotFound,
            HistoryFetchError::Conflict => Self::Conflict,
            HistoryFetchError::InvalidRequest => Self::InvalidRequest,
            HistoryFetchError::Malformed => Self::Malformed,
            HistoryFetchError::Oversized => Self::Oversized,
            HistoryFetchError::Unsupported => Self::Unsupported,
            HistoryFetchError::Unauthorized
            | HistoryFetchError::Forbidden
            | HistoryFetchError::Unavailable => Self::Unavailable,
        }
    }
}

/// Published status of one task's history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryStatus {
    /// Never requested for the current context; nothing may be rendered as absent.
    #[default]
    Idle,
    /// A read is in flight. Previously loaded rows for the same task stay visible.
    Loading,
    /// A page was loaded and a further page exists.
    Loaded,
    /// The Runtime served an authoritative empty history.
    Empty,
    /// Every page was loaded; the last page carried no continuation.
    End,
    /// The read failed with a value-free category.
    Error(HistoryError),
    /// The credential/context is not authorized to read this task's history.
    Unauthorized,
}

impl HistoryStatus {
    /// Whether an authoritative result has been served for the current request.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Loaded | Self::Empty | Self::End)
    }

    /// Whether the shown rows came from at least one served page.
    pub fn has_rows(self) -> bool {
        self.is_settled()
    }
}

/// One run-level history entry; only Runtime-projected fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Runtime record reference for the run.
    pub record_ref: Arc<str>,
    /// Run identity.
    pub run_id: Arc<str>,
    /// Iteration index the run belongs to.
    pub iteration: u64,
    /// Inner phase label recorded for the run.
    pub phase: Arc<str>,
    /// Execution plan identity; `None` means the Runtime recorded none.
    pub plan_id: Option<Arc<str>>,
    /// Runtime-issued creation timestamp.
    pub created_at: Arc<str>,
}

impl HistoryEntry {
    fn from_item(item: &HistoryItem) -> Self {
        Self {
            record_ref: Arc::from(item.record_ref.as_str()),
            run_id: Arc::from(item.run_id.as_str()),
            iteration: item.iteration,
            phase: Arc::from(item.phase.as_str()),
            plan_id: (!item.plan_id.is_empty()).then(|| Arc::from(item.plan_id.as_str())),
            created_at: Arc::from(item.created_at.as_str()),
        }
    }
}

/// Immutable bounded view of one task's loaded history pages.
#[derive(Clone, Debug, Default)]
pub struct HistoryPublication {
    /// Task the view belongs to; `None` before any request.
    pub task_ref: Option<TaskId>,
    /// Current status.
    pub status: HistoryStatus,
    /// Loaded run-level entries, newest first.
    pub entries: Arc<[HistoryEntry]>,
    /// Whether the Runtime holds a further page.
    pub has_more: bool,
    /// Keyset continuation for the next page, when one exists.
    pub next_cursor: Option<HistoryCursor>,
    /// Whether the bounded entry ceiling stopped further appends.
    ///
    /// A bound is an explicit state, never an end: `has_more` may stay true
    /// while older data remains unloaded.
    pub at_bound: bool,
    /// Owner sequence of the request that produced this view.
    pub sequence: u64,
}

impl HistoryPublication {
    /// Whether the view belongs to `task`.
    pub fn is_for(&self, task: &TaskId) -> bool {
        self.task_ref.as_ref() == Some(task)
    }
}

/// Owner-side accumulator: request sequencing, page merging and the bound.
///
/// It performs no I/O and owns no canonical facts: every entry is copied from a
/// served document, and only the current request sequence may publish.
pub(crate) struct HistoryAccumulator {
    publication: Arc<HistoryPublication>,
    sequence: u64,
}

impl Default for HistoryAccumulator {
    fn default() -> Self {
        Self {
            publication: Arc::new(HistoryPublication::default()),
            sequence: 0,
        }
    }
}

impl HistoryAccumulator {
    /// Current immutable view.
    pub(crate) fn publication(&self) -> Arc<HistoryPublication> {
        Arc::clone(&self.publication)
    }

    /// Discard everything (scope/authorization/client change, shutdown).
    pub(crate) fn reset(&mut self) {
        self.sequence = self.sequence.saturating_add(1);
        self.publication = Arc::new(HistoryPublication {
            sequence: self.sequence,
            ..HistoryPublication::default()
        });
    }

    /// Begin one request: bump the sequence and publish `Loading`.
    ///
    /// Rows already loaded for the *same* task stay visible while its older
    /// page loads; another task's rows are dropped immediately so a task switch
    /// never renders the previous task's history.
    pub(crate) fn begin(&mut self, task: &TaskId) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        let same_task = self.publication.is_for(task);
        let previous = &self.publication;
        self.publication = Arc::new(HistoryPublication {
            task_ref: Some(task.clone()),
            status: HistoryStatus::Loading,
            entries: if same_task {
                Arc::clone(&previous.entries)
            } else {
                Arc::from([])
            },
            has_more: same_task && previous.has_more,
            next_cursor: same_task.then(|| previous.next_cursor.clone()).flatten(),
            at_bound: same_task && previous.at_bound,
            sequence: self.sequence,
        });
        self.sequence
    }

    /// Apply one served page if `sequence` is still current.
    ///
    /// Returns false when the answer is stale; the publication then keeps the
    /// newer request's state untouched. A continuation page that repeats an
    /// already-loaded record or exceeds the entry bound is refused without
    /// mutating the view (a partial merge is never presented as complete).
    pub(crate) fn apply(
        &mut self,
        sequence: u64,
        document: &HistoryDocument,
        first_page: bool,
    ) -> bool {
        if sequence != self.sequence {
            return false;
        }
        let Ok(task) = document.task() else {
            return self.fail(sequence, HistoryError::Malformed);
        };
        if !self.publication.is_for(&task) {
            return false;
        }
        let incoming: Vec<HistoryEntry> =
            document.items.iter().map(HistoryEntry::from_item).collect();
        if !first_page && incoming.is_empty() && document.has_more {
            // A continuation with no rows but a further page is a contract
            // violation: appending it would loop without ever advancing.
            return self.fail(sequence, HistoryError::Malformed);
        }
        if !first_page {
            let repeated = incoming.iter().any(|entry| {
                self.publication
                    .entries
                    .iter()
                    .any(|known| known.record_ref == entry.record_ref)
            });
            if repeated {
                return self.fail(sequence, HistoryError::Malformed);
            }
        }
        let base: Arc<[HistoryEntry]> = if first_page {
            Arc::from([])
        } else {
            Arc::clone(&self.publication.entries)
        };
        let at_bound = base.len().saturating_add(incoming.len()) > HISTORY_MAX_ENTRIES;
        if at_bound {
            // Keep exactly what was already shown; the bound is explicit and the
            // remaining pages stay visibly unloaded instead of becoming an end.
            self.publication = Arc::new(HistoryPublication {
                task_ref: Some(task),
                status: HistoryStatus::Loaded,
                entries: base,
                has_more: true,
                next_cursor: self.publication.next_cursor.clone(),
                at_bound: true,
                sequence: self.sequence,
            });
            return true;
        }
        let mut entries: Vec<HistoryEntry> = Vec::with_capacity(base.len() + incoming.len());
        entries.extend(base.iter().cloned());
        entries.extend(incoming);
        let has_more = document.has_more;
        let status = if entries.is_empty() && !has_more {
            HistoryStatus::Empty
        } else if has_more {
            HistoryStatus::Loaded
        } else {
            HistoryStatus::End
        };
        let at_bound = has_more && entries.len() >= HISTORY_MAX_ENTRIES;
        self.publication = Arc::new(HistoryPublication {
            task_ref: Some(task),
            status,
            entries: Arc::from(entries),
            has_more,
            next_cursor: document.next_cursor(),
            at_bound,
            sequence: self.sequence,
        });
        true
    }

    /// Publish one value-free failure if `sequence` is still current.
    pub(crate) fn fail(&mut self, sequence: u64, error: HistoryError) -> bool {
        if sequence != self.sequence {
            return false;
        }
        let previous = &self.publication;
        self.publication = Arc::new(HistoryPublication {
            task_ref: previous.task_ref.clone(),
            status: HistoryStatus::Error(error),
            entries: Arc::clone(&previous.entries),
            has_more: previous.has_more,
            next_cursor: previous.next_cursor.clone(),
            at_bound: previous.at_bound,
            sequence: self.sequence,
        });
        true
    }

    /// Publish an unauthorized/forbidden read if `sequence` is still current.
    pub(crate) fn unauthorized(&mut self, sequence: u64) -> bool {
        if sequence != self.sequence {
            return false;
        }
        let previous = &self.publication;
        self.publication = Arc::new(HistoryPublication {
            task_ref: previous.task_ref.clone(),
            status: HistoryStatus::Unauthorized,
            entries: Arc::clone(&previous.entries),
            has_more: previous.has_more,
            next_cursor: previous.next_cursor.clone(),
            at_bound: previous.at_bound,
            sequence: self.sequence,
        });
        true
    }

    /// Publish one value-free client failure if `sequence` is still current.
    pub(crate) fn failed_with(&mut self, sequence: u64, error: HistoryFetchError) -> bool {
        match error {
            HistoryFetchError::Unauthorized | HistoryFetchError::Forbidden => {
                self.unauthorized(sequence)
            }
            other => self.fail(sequence, HistoryError::from_fetch_error(other)),
        }
    }
}

/// The page size and continuation one handle request asks for.
///
/// The handle API is bounded: the query type has already validated the `1..=20`
/// limit and the cursor bound, so the owner never widens or reinterprets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    /// Validated client query.
    pub query: HistoryQuery,
}

impl HistoryRequest {
    /// First page of one task's history.
    pub fn first(task: TaskId) -> Self {
        Self {
            query: HistoryQuery::first(task),
        }
    }

    /// Continuation of the published view, when one is offered.
    pub fn older(publication: &HistoryPublication, task: TaskId) -> Option<Self> {
        let cursor = publication.next_cursor.clone()?;
        Some(Self {
            query: HistoryQuery::new(task, HISTORY_PAGE_LIMIT, Some(cursor)).ok()?,
        })
    }
}

/// Value-free acknowledgement of one history request.
///
/// The handle API carries no page and no server text: the caller renders the
/// publication and this enum only says whether its request became current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryFetchReply {
    /// The answer was applied to the current publication (or a page request
    /// started the load); the view is authoritative for this sequence.
    Published,
    /// A newer request or an authorization reset owns the publication now.
    Superseded,
    /// The owner is shutting down or the mailbox is closed.
    Closed,
}

#[cfg(test)]
mod tests {
    use super::{HISTORY_MAX_ENTRIES, HistoryAccumulator, HistoryError, HistoryStatus};
    use cos_client::{HistoryDocument, HistoryItem, HistoryQuery, HistoryServer, HistoryStamp};
    use desktop_protocol::TaskId;

    fn task(label: &str) -> TaskId {
        TaskId::new(format!("task/{label}")).expect("task id")
    }

    fn item(label: &str, iteration: u64) -> HistoryItem {
        HistoryItem {
            task_ref: label.to_owned(),
            record_ref: format!("{label}:{iteration}"),
            run_id: format!("{label}/run-{iteration}"),
            iteration,
            phase: "REFINE".to_owned(),
            plan_id: String::new(),
            created_at: "2026-11-08T00:00:00Z".to_owned(),
        }
    }

    fn document(task_label: &str, items: Vec<HistoryItem>, next: Option<&str>) -> HistoryDocument {
        HistoryDocument {
            schema_version: 1,
            server: HistoryServer {
                server_identity: "server".to_owned(),
                epoch: "epoch".to_owned(),
            },
            stamp: HistoryStamp {
                request_id: "request".to_owned(),
                server_id: "server".to_owned(),
                epoch: "epoch".to_owned(),
                context_id: "context".to_owned(),
                read_revision: "1".to_owned(),
                scope_id: "scope".to_owned(),
            },
            task_ref: task_label.to_owned(),
            items,
            next_cursor: next.map(str::to_owned),
            has_more: next.is_some(),
        }
    }

    #[test]
    fn an_unloaded_view_is_idle_and_never_empty() {
        let accumulator = HistoryAccumulator::default();
        let publication = accumulator.publication();
        assert_eq!(publication.status, HistoryStatus::Idle);
        assert!(publication.entries.is_empty());
        assert_ne!(publication.status, HistoryStatus::Empty);
    }

    #[test]
    fn a_served_empty_page_is_authoritative_and_distinct_from_idle() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(sequence, &document("task/Task-184", Vec::new(), None), true));
        let publication = accumulator.publication();
        assert_eq!(publication.status, HistoryStatus::Empty);
        assert!(publication.entries.is_empty());
    }

    #[test]
    fn pages_append_by_cursor_and_end_only_without_a_continuation() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document(
                "task/Task-184",
                vec![item("task/Task-184", 3)],
                Some("cursor-1")
            ),
            true
        ));
        let publication = accumulator.publication();
        assert_eq!(publication.status, HistoryStatus::Loaded);
        assert!(publication.has_more);
        assert_eq!(
            publication.next_cursor.as_ref().map(|c| c.as_str()),
            Some("cursor-1")
        );

        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document("task/Task-184", vec![item("task/Task-184", 2)], None),
            false
        ));
        let publication = accumulator.publication();
        assert_eq!(publication.status, HistoryStatus::End);
        assert!(!publication.has_more);
        assert_eq!(publication.entries.len(), 2);
        assert_eq!(publication.entries[0].iteration, 3);
        assert_eq!(publication.entries[1].iteration, 2);
    }

    #[test]
    fn a_stale_page_is_discarded_without_touching_the_current_view() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let stale = accumulator.begin(&task);
        let current = accumulator.begin(&task);
        assert_ne!(stale, current);
        assert!(accumulator.apply(
            current,
            &document(
                "task/Task-184",
                vec![item("task/Task-184", 5)],
                Some("cursor-5")
            ),
            true
        ));
        let before = accumulator.publication();
        assert!(
            !accumulator.apply(
                stale,
                &document(
                    "task/Task-184",
                    vec![item("task/Task-184", 9)],
                    Some("cursor-9")
                ),
                true
            ),
            "a stale answer must not publish"
        );
        let after = accumulator.publication();
        assert_eq!(after.entries, before.entries);
        assert_eq!(after.next_cursor, before.next_cursor);
        assert_eq!(after.status, HistoryStatus::Loaded);
        assert_eq!(after.sequence, current);
    }

    #[test]
    fn a_continuation_repeating_a_loaded_record_is_refused() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document(
                "task/Task-184",
                vec![item("task/Task-184", 3)],
                Some("cursor-1")
            ),
            true
        ));
        let sequence = accumulator.begin(&task);
        assert!(
            accumulator.apply(
                sequence,
                &document("task/Task-184", vec![item("task/Task-184", 3)], None),
                false
            ),
            "the refusal is published as a closed error, not applied as rows"
        );
        let publication = accumulator.publication();
        assert_eq!(
            publication.status,
            HistoryStatus::Error(HistoryError::Malformed)
        );
        assert_eq!(publication.entries.len(), 1);
    }

    #[test]
    fn the_entry_bound_is_explicit_and_never_turned_into_an_end() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        let page: Vec<HistoryItem> = (0..HISTORY_MAX_ENTRIES as u64)
            .map(|iteration| item("task/Task-184", iteration))
            .collect();
        assert!(accumulator.apply(
            sequence,
            &document("task/Task-184", page, Some("cursor-199")),
            true
        ));
        let publication = accumulator.publication();
        assert_eq!(publication.status, HistoryStatus::Loaded);
        assert!(publication.at_bound, "the ceiling is explicit");
        assert!(
            publication.has_more,
            "older data still exists and is not an end"
        );

        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document("task/Task-184", vec![item("task/Task-184", 999)], None),
            false
        ));
        let publication = accumulator.publication();
        assert_eq!(publication.entries.len(), HISTORY_MAX_ENTRIES);
        assert!(publication.at_bound);
        assert!(publication.has_more);
        assert_ne!(publication.status, HistoryStatus::End);
        assert_eq!(
            publication.next_cursor.as_ref().map(|c| c.as_str()),
            Some("cursor-199")
        );
    }

    #[test]
    fn failures_are_closed_categories_and_unauthorized_is_distinct() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        assert!(accumulator.failed_with(sequence, cos_client::HistoryFetchError::NotFound));
        assert_eq!(
            accumulator.publication().status,
            HistoryStatus::Error(HistoryError::NotFound)
        );
        let sequence = accumulator.begin(&task);
        assert!(accumulator.failed_with(sequence, cos_client::HistoryFetchError::Unauthorized));
        assert_eq!(
            accumulator.publication().status,
            HistoryStatus::Unauthorized
        );
    }

    #[test]
    fn reset_drops_rows_and_returns_to_idle() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document("task/Task-184", vec![item("task/Task-184", 1)], None),
            true
        ));
        accumulator.reset();
        let publication = accumulator.publication();
        assert_eq!(publication.status, HistoryStatus::Idle);
        assert_eq!(publication.task_ref, None);
        assert!(publication.entries.is_empty());
    }

    #[test]
    fn a_new_task_never_keeps_the_previous_tasks_rows_while_loading() {
        let mut accumulator = HistoryAccumulator::default();
        let first = task("Task-184");
        let sequence = accumulator.begin(&first);
        assert!(accumulator.apply(
            sequence,
            &document("task/Task-184", vec![item("task/Task-184", 1)], None),
            true
        ));
        let other = task("Task-777");
        let _ = accumulator.begin(&other);
        let publication = accumulator.publication();
        assert_eq!(publication.status, HistoryStatus::Loading);
        assert_eq!(publication.task_ref, Some(other));
        assert!(publication.entries.is_empty(), "no cross-task row leaks");
    }

    #[test]
    fn older_requests_only_a_loaded_continuation() {
        use super::HistoryRequest;
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document(
                "task/Task-184",
                vec![item("task/Task-184", 1)],
                Some("cursor-1")
            ),
            true
        ));
        let request = HistoryRequest::older(&accumulator.publication(), task.clone())
            .expect("continuation requested");
        assert_eq!(request.query.cursor().map(|c| c.as_str()), Some("cursor-1"));
        assert_eq!(request.query.limit(), super::HISTORY_PAGE_LIMIT);
        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document("task/Task-184", Vec::new(), None),
            false
        ));
        assert!(
            HistoryRequest::older(&accumulator.publication(), task).is_none(),
            "an end offers no load-older action"
        );
    }

    #[test]
    fn an_absent_plan_id_is_kept_absent() {
        let mut accumulator = HistoryAccumulator::default();
        let task = task("Task-184");
        let sequence = accumulator.begin(&task);
        assert!(accumulator.apply(
            sequence,
            &document("task/Task-184", vec![item("task/Task-184", 1)], None),
            true
        ));
        assert_eq!(accumulator.publication().entries[0].plan_id, None);
    }

    #[test]
    fn the_default_query_is_bounded_by_the_client_contract() {
        let query = HistoryQuery::first(task("Task-184"));
        assert_eq!(query.limit(), super::HISTORY_PAGE_LIMIT);
        assert!(query.cursor().is_none());
    }
}
