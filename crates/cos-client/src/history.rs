//! Authenticated bounded task history read (DC-MP-NAV3).
//!
//! The Runtime serves `GET /api/cos/desktop/tasks/{task_ref}/history` as an
//! authenticated keyset page of run-level records. This module owns the closed
//! document shape, the bounded request contract and the value-free failure
//! classes. It performs no UI decision and stores no canonical facts: a caller
//! renders the page it was served and continues only with the server-issued
//! cursor.
//!
//! The continuation contract is all or nothing: `has_more` and `next_cursor`
//! must agree, and a page that says more remains without a cursor is refused as
//! malformed rather than presented as an authoritative end (a truncation is
//! never turned into absence). An empty page with no continuation is the
//! authoritative empty history and is distinct from "not loaded yet", which is
//! a caller-side state this module never invents.

use desktop_protocol::TaskId;
use serde::{Deserialize, Serialize};

/// Smallest page size the Runtime accepts (`limit` 1..=20).
pub const HISTORY_LIMIT_MIN: usize = 1;
/// Largest page size the Runtime accepts (`limit` 1..=20).
pub const HISTORY_LIMIT_MAX: usize = 20;
/// Page size used when the caller does not choose one. Local history defines no
/// default: the Runtime page size is requested explicitly. This constant is the
/// single source for the default request.
pub const HISTORY_LIMIT_DEFAULT: usize = 20;
/// Bound on the number of run-level items one document may carry.
pub const MAX_HISTORY_ITEMS: usize = HISTORY_LIMIT_MAX;
/// Bound on the opaque continuation cursor (`next_cursor` is at most 2048
/// characters on the Runtime side).
pub const MAX_HISTORY_CURSOR_BYTES: usize = 2048;
/// Field bounds taken from the Runtime read document.
pub const MAX_HISTORY_TASK_REF_BYTES: usize = 512;
/// Bound on the record reference field.
pub const MAX_HISTORY_RECORD_REF_BYTES: usize = 1024;
/// Bound on the run identity field.
pub const MAX_HISTORY_RUN_ID_BYTES: usize = 512;
/// Bound on the phase label field.
pub const MAX_HISTORY_PHASE_BYTES: usize = 64;
/// Bound on the plan identity field (empty means "no plan recorded").
pub const MAX_HISTORY_PLAN_ID_BYTES: usize = 512;
/// Bound on the created-at field.
pub const MAX_HISTORY_CREATED_AT_BYTES: usize = 128;
/// Fixed tail of every task-scoped Desktop read route.
pub const HISTORY_ROUTE_SUFFIX: &str = "/history";
/// Prefix of the task-scoped Desktop read routes.
pub const TASK_READ_ROUTE_PREFIX: &str = "/api/cos/desktop/tasks/";

/// Closed value-free failure classes of one history read.
///
/// No server-supplied detail string is retained: a failure is a category, never
/// a copied Runtime message. The categories mirror the route's status classes
/// (`401`/`403`/`404`/`409`/`422`) plus parse and local-transport failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HistoryFetchError {
    /// `401` — the read context or credential is not usable.
    #[error("history read is unauthorized")]
    Unauthorized,
    /// `403` — the credential principal does not own the read context or scope.
    #[error("history read is forbidden")]
    Forbidden,
    /// `404` — the task is outside the granted scope or does not exist.
    #[error("history read target was not found")]
    NotFound,
    /// `409` — the observation binding must be re-established.
    #[error("history read requires resync")]
    Conflict,
    /// `422` — the limit or cursor was refused before domain lookup.
    #[error("history read request is invalid")]
    InvalidRequest,
    /// The document did not match the closed contract.
    #[error("history document is malformed")]
    Malformed,
    /// The response exceeded the bounded body size.
    #[error("history response is oversized")]
    Oversized,
    /// The exchange failed (I/O, timeout, unsupported transport).
    #[error("history read is unavailable")]
    Unavailable,
    /// The transport has no authenticated history read; it never synthesizes one.
    #[error("history read is unsupported by this transport")]
    Unsupported,
}

impl HistoryFetchError {
    /// Map one HTTP status and stable Runtime code to the closed class.
    pub fn from_http(status: u16, code: Option<&str>) -> Self {
        match status {
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            409 => Self::Conflict,
            413 => Self::Oversized,
            422 => Self::InvalidRequest,
            408 | 429 | 500..=599 => Self::Unavailable,
            _ => match code {
                Some("UNAUTHORIZED_CONTEXT") => Self::Unauthorized,
                Some("PRINCIPAL_MISMATCH") => Self::Forbidden,
                Some("RESYNC_REQUIRED") | Some("CURSOR_EXPIRED") => Self::Conflict,
                Some("NOT_FOUND") => Self::NotFound,
                Some("VALIDATION_FAILED") => Self::InvalidRequest,
                Some("JSON_RESPONSE_TOO_LARGE") => Self::Oversized,
                _ => Self::Unavailable,
            },
        }
    }

    /// Whether a later request on the same binding could succeed unchanged.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// One opaque server-issued continuation cursor.
///
/// The cursor is a keyset position, not an offset: it is never derived from an
/// iteration number and never parsed locally. It is only bounded and passed
/// back verbatim.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HistoryCursor(String);

impl HistoryCursor {
    /// Validate and adopt one continuation cursor.
    pub fn new(raw: impl Into<String>) -> Result<Self, HistoryFetchError> {
        let raw = raw.into();
        if raw.is_empty()
            || raw.len() > MAX_HISTORY_CURSOR_BYTES
            || raw.chars().any(char::is_control)
        {
            return Err(HistoryFetchError::InvalidRequest);
        }
        Ok(Self(raw))
    }

    /// Borrow the opaque cursor.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HistoryCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One bounded history request.
///
/// The task, page size and continuation are fixed at construction: the
/// transport cannot widen the page or fabricate a cursor after validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryQuery {
    task_ref: TaskId,
    limit: usize,
    cursor: Option<HistoryCursor>,
}

impl HistoryQuery {
    /// Validate one request. A limit outside `1..=20` and an empty, oversized
    /// or control-bearing cursor are refused locally, so no invalid page is
    /// ever sent.
    pub fn new(
        task_ref: TaskId,
        limit: usize,
        cursor: Option<HistoryCursor>,
    ) -> Result<Self, HistoryFetchError> {
        if !(HISTORY_LIMIT_MIN..=HISTORY_LIMIT_MAX).contains(&limit) {
            return Err(HistoryFetchError::InvalidRequest);
        }
        Ok(Self {
            task_ref,
            limit,
            cursor,
        })
    }

    /// The first page of one task's history at the Runtime page size.
    pub fn first(task_ref: TaskId) -> Self {
        Self {
            task_ref,
            limit: HISTORY_LIMIT_DEFAULT,
            cursor: None,
        }
    }

    /// The task this page belongs to.
    pub fn task_ref(&self) -> &TaskId {
        &self.task_ref
    }

    /// Requested page size (`1..=20`).
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Continuation cursor, when this is not the first page.
    pub fn cursor(&self) -> Option<&HistoryCursor> {
        self.cursor.as_ref()
    }

    /// The next page of the same task after a served page.
    pub fn continuation(&self, page: &HistoryDocument) -> Result<Option<Self>, HistoryFetchError> {
        let Some(cursor) = page.next_cursor() else {
            return Ok(None);
        };
        Ok(Some(Self {
            task_ref: page.task()?,
            limit: self.limit,
            cursor: Some(cursor),
        }))
    }
}

/// Server identity and epoch that served one history page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryServer {
    /// Stable server identity.
    pub server_identity: String,
    /// Runtime epoch that served this page.
    pub epoch: String,
}

/// Authorized read stamp of the served page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryStamp {
    /// Read request identity the Runtime answered.
    pub request_id: String,
    /// Server identity.
    pub server_id: String,
    /// Runtime epoch.
    pub epoch: String,
    /// Read context the page is bound to.
    pub context_id: String,
    /// Read revision of that context.
    pub read_revision: String,
    /// Scope identity of that context.
    pub scope_id: String,
}

/// One authoritative run-level history record.
///
/// Only server-projected fields are represented. Absent optional values stay
/// absent: `plan_id` is a string on the wire and an empty value means "no plan
/// recorded", which a caller renders as explicit unknown rather than inventing
/// one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryItem {
    /// Owning task reference repeated on every item.
    pub task_ref: String,
    /// Runtime record reference for this run.
    pub record_ref: String,
    /// Run identity.
    pub run_id: String,
    /// Iteration index the run belongs to.
    pub iteration: u64,
    /// Inner phase label the Runtime recorded for the run.
    pub phase: String,
    /// Execution plan identity; empty string means none was recorded.
    #[serde(default)]
    pub plan_id: String,
    /// Runtime-issued creation timestamp.
    pub created_at: String,
}

/// One bounded authenticated history page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryDocument {
    /// Document schema version (must be 1).
    pub schema_version: u32,
    /// Server identity and epoch.
    pub server: HistoryServer,
    /// Authorized read stamp.
    pub stamp: HistoryStamp,
    /// Task the page describes.
    pub task_ref: String,
    /// Run-level records, newest first.
    pub items: Vec<HistoryItem>,
    /// Keyset continuation; absent when no further page remains.
    #[serde(default)]
    pub next_cursor: Option<String>,
    /// Whether the Runtime holds a further page.
    pub has_more: bool,
}

impl HistoryDocument {
    /// Validate the closed contract and return the typed task identity.
    ///
    /// This is the one place the continuation invariant is checked:
    /// `has_more == next_cursor.is_some()`. A page that claims more without a
    /// cursor is refused; only an explicit `has_more = false` is an end.
    pub fn validate(&self) -> Result<(), HistoryFetchError> {
        if self.schema_version != 1 {
            return Err(HistoryFetchError::Malformed);
        }
        if self.server.server_identity.is_empty() || self.server.epoch.is_empty() {
            return Err(HistoryFetchError::Malformed);
        }
        if self.stamp.request_id.is_empty()
            || self.stamp.server_id.is_empty()
            || self.stamp.epoch.is_empty()
            || self.stamp.context_id.is_empty()
            || self.stamp.read_revision.is_empty()
            || self.stamp.scope_id.is_empty()
        {
            return Err(HistoryFetchError::Malformed);
        }
        if self.task_ref.is_empty() || self.task_ref.len() > MAX_HISTORY_TASK_REF_BYTES {
            return Err(HistoryFetchError::Malformed);
        }
        if self.items.len() > MAX_HISTORY_ITEMS {
            return Err(HistoryFetchError::Malformed);
        }
        // A further page from an empty page would never advance the keyset; the
        // Runtime never serves it, so it is refused rather than looped on.
        if self.items.is_empty() && self.has_more {
            return Err(HistoryFetchError::Malformed);
        }
        for item in &self.items {
            item.validate(&self.task_ref)?;
        }
        match (&self.next_cursor, self.has_more) {
            (Some(cursor), true) => {
                HistoryCursor::new(cursor.as_str()).map_err(|_| HistoryFetchError::Malformed)?;
            }
            (None, false) => {}
            // A truncation is never presented as an end, and an end never
            // carries a continuation.
            _ => return Err(HistoryFetchError::Malformed),
        }
        Ok(())
    }

    /// Typed task identity of the served page.
    pub fn task(&self) -> Result<TaskId, HistoryFetchError> {
        TaskId::new(self.task_ref.clone()).map_err(|_| HistoryFetchError::Malformed)
    }

    /// Typed continuation cursor, validated with the document.
    pub fn next_cursor(&self) -> Option<HistoryCursor> {
        self.next_cursor
            .as_deref()
            .and_then(|raw| HistoryCursor::new(raw).ok())
    }

    /// Whether the document is an authoritative empty history.
    ///
    /// This is true only for a served first page with no items and no
    /// continuation. It never describes an unloaded page.
    pub fn is_empty_history(&self) -> bool {
        self.items.is_empty() && !self.has_more
    }
}

impl HistoryItem {
    fn validate(&self, document_task: &str) -> Result<(), HistoryFetchError> {
        if self.task_ref != document_task
            || self.task_ref.is_empty()
            || self.task_ref.len() > MAX_HISTORY_TASK_REF_BYTES
        {
            return Err(HistoryFetchError::Malformed);
        }
        if self.record_ref.is_empty() || self.record_ref.len() > MAX_HISTORY_RECORD_REF_BYTES {
            return Err(HistoryFetchError::Malformed);
        }
        if self.run_id.is_empty() || self.run_id.len() > MAX_HISTORY_RUN_ID_BYTES {
            return Err(HistoryFetchError::Malformed);
        }
        if self.phase.is_empty() || self.phase.len() > MAX_HISTORY_PHASE_BYTES {
            return Err(HistoryFetchError::Malformed);
        }
        if self.plan_id.len() > MAX_HISTORY_PLAN_ID_BYTES {
            return Err(HistoryFetchError::Malformed);
        }
        if self.created_at.is_empty() || self.created_at.len() > MAX_HISTORY_CREATED_AT_BYTES {
            return Err(HistoryFetchError::Malformed);
        }
        if [
            &self.record_ref,
            &self.run_id,
            &self.phase,
            &self.created_at,
        ]
        .iter()
        .any(|field| field.chars().any(char::is_control))
        {
            return Err(HistoryFetchError::Malformed);
        }
        Ok(())
    }
}

/// Percent-encode one value for a path segment or query value (RFC 3986
/// unreserved characters stay literal).
pub(crate) fn percent_encode(raw: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => {
                encoded.push('%');
                encoded.push(char::from(HEX[usize::from(other >> 4)]));
                encoded.push(char::from(HEX[usize::from(other & 0x0F)]));
            }
        }
    }
    encoded
}

/// Route for one bounded history page.
pub fn history_path(query: &HistoryQuery) -> String {
    let mut path = format!(
        "{TASK_READ_ROUTE_PREFIX}{}{HISTORY_ROUTE_SUFFIX}?limit={}",
        percent_encode(query.task_ref.as_str()),
        query.limit
    );
    if let Some(cursor) = query.cursor() {
        path.push_str("&cursor=");
        path.push_str(&percent_encode(cursor.as_str()));
    }
    path
}

/// Whether a path addresses a task-scoped authenticated Desktop read.
///
/// The task reference is variable, so the route is matched by its fixed prefix
/// and the `/history` tail; a query string is ignored by the caller's existing
/// rule, and this predicate only decides whether the credential is attached.
pub(crate) fn is_task_read_route(path: &str) -> bool {
    let route = path.split('?').next().unwrap_or(path);
    route.starts_with(TASK_READ_ROUTE_PREFIX) && route.ends_with(HISTORY_ROUTE_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::{
        HISTORY_LIMIT_DEFAULT, HISTORY_LIMIT_MAX, HistoryCursor, HistoryDocument,
        HistoryFetchError, HistoryItem, HistoryQuery, history_path, is_task_read_route,
        percent_encode,
    };
    use desktop_protocol::TaskId;

    fn task() -> TaskId {
        TaskId::new("Task-184").expect("task")
    }

    fn item(task_ref: &str) -> HistoryItem {
        HistoryItem {
            task_ref: task_ref.to_owned(),
            record_ref: "run-1:1".to_owned(),
            run_id: "run-1".to_owned(),
            iteration: 1,
            phase: "REFINE".to_owned(),
            plan_id: String::new(),
            created_at: "2026-11-08T00:00:00Z".to_owned(),
        }
    }

    fn document(
        items: Vec<HistoryItem>,
        next_cursor: Option<&str>,
        has_more: bool,
    ) -> HistoryDocument {
        HistoryDocument {
            schema_version: 1,
            server: super::HistoryServer {
                server_identity: "server".to_owned(),
                epoch: "epoch".to_owned(),
            },
            stamp: super::HistoryStamp {
                request_id: "request".to_owned(),
                server_id: "server".to_owned(),
                epoch: "epoch".to_owned(),
                context_id: "context".to_owned(),
                read_revision: "1".to_owned(),
                scope_id: "scope".to_owned(),
            },
            task_ref: "Task-184".to_owned(),
            items,
            next_cursor: next_cursor.map(str::to_owned),
            has_more,
        }
    }

    #[test]
    fn a_valid_page_validates_and_exposes_its_typed_task_and_cursor() {
        let page = document(vec![item("Task-184")], Some("cursor+1"), true);
        assert_eq!(page.validate(), Ok(()));
        assert_eq!(page.task().expect("task"), task());
        assert_eq!(page.next_cursor().expect("cursor").as_str(), "cursor+1");
        assert!(!page.is_empty_history());
    }

    #[test]
    fn an_authoritative_empty_page_is_not_a_continuation() {
        let page = document(Vec::new(), None, false);
        assert_eq!(page.validate(), Ok(()));
        assert!(page.is_empty_history());
        assert_eq!(page.next_cursor(), None);
    }

    #[test]
    fn a_truncation_without_a_cursor_is_refused_and_never_read_as_an_end() {
        let page = document(vec![item("Task-184")], None, true);
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));
        // A cursor without `has_more` is equally contradictory.
        let page = document(Vec::new(), Some("cursor"), false);
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));
        // An empty page that claims more can never advance the keyset.
        let page = document(Vec::new(), Some("cursor"), true);
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));
    }

    #[test]
    fn every_closed_parse_violation_is_refused() {
        let mut page = document(vec![item("Task-184")], None, false);
        page.schema_version = 2;
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));

        let mut page = document(vec![item("Task-184")], None, false);
        page.items[0].task_ref = "Other".to_owned();
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));

        let mut page = document(vec![item("Task-184")], None, false);
        page.items[0].phase = String::new();
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));

        let mut page = document(vec![item("Task-184")], None, false);
        page.items[0].record_ref = "a\nb".to_owned();
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));

        let mut page = document(vec![item("Task-184")], None, false);
        page.items = (0..=super::MAX_HISTORY_ITEMS)
            .map(|_| item("Task-184"))
            .collect();
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));

        let mut page = document(Vec::new(), Some("bad\ncursor"), true);
        page.stamp.context_id = String::new();
        assert_eq!(page.validate(), Err(HistoryFetchError::Malformed));
    }

    #[test]
    fn the_request_bounds_the_page_and_the_cursor_before_any_exchange() {
        assert_eq!(
            HistoryQuery::new(task(), 0, None),
            Err(HistoryFetchError::InvalidRequest)
        );
        assert_eq!(
            HistoryQuery::new(task(), HISTORY_LIMIT_MAX + 1, None),
            Err(HistoryFetchError::InvalidRequest)
        );
        let query = HistoryQuery::new(task(), HISTORY_LIMIT_MAX, None).expect("valid query");
        assert_eq!(query.limit(), HISTORY_LIMIT_MAX);
        assert_eq!(query.cursor(), None);
        assert_eq!(HistoryQuery::first(task()).limit(), HISTORY_LIMIT_DEFAULT);

        assert_eq!(
            HistoryCursor::new(String::new()),
            Err(HistoryFetchError::InvalidRequest)
        );
        assert_eq!(
            HistoryCursor::new("x".repeat(super::MAX_HISTORY_CURSOR_BYTES + 1)),
            Err(HistoryFetchError::InvalidRequest)
        );
        assert_eq!(
            HistoryCursor::new("cursor\u{7}"),
            Err(HistoryFetchError::InvalidRequest)
        );
    }

    #[test]
    fn continuation_keeps_the_same_task_and_page_size() {
        let first = HistoryQuery::new(task(), 5, None).expect("query");
        let page = document(vec![item("Task-184")], Some("next"), true);
        let next = first
            .continuation(&page)
            .expect("valid continuation")
            .expect("a continuation exists");
        assert_eq!(next.task_ref(), &task());
        assert_eq!(next.limit(), 5);
        assert_eq!(next.cursor().expect("cursor").as_str(), "next");
        let end = document(Vec::new(), None, false);
        assert!(first.continuation(&end).expect("end").is_none());
    }

    #[test]
    fn the_route_carries_the_bounded_limit_and_an_encoded_cursor() {
        let query = HistoryQuery::new(
            task(),
            7,
            Some(HistoryCursor::new("a b/c+d=").expect("cursor")),
        )
        .expect("query");
        assert_eq!(
            history_path(&query),
            "/api/cos/desktop/tasks/Task-184/history?limit=7&cursor=a%20b%2Fc%2Bd%3D"
        );
        assert_eq!(percent_encode("Task-184"), "Task-184");
        assert_eq!(percent_encode("a/b:c"), "a%2Fb%3Ac");
    }

    #[test]
    fn only_task_scoped_history_routes_carry_the_credential() {
        assert!(is_task_read_route(
            "/api/cos/desktop/tasks/Task-184/history?limit=20"
        ));
        assert!(is_task_read_route("/api/cos/desktop/tasks/a%2Fb/history"));
        assert!(!is_task_read_route("/api/cos/desktop/tasks/Task-184"));
        assert!(!is_task_read_route("/api/cos/desktop/catalog"));
        assert!(!is_task_read_route("/api/cos/tasks/Task-184/history"));
    }

    #[test]
    fn an_absent_plan_id_stays_an_empty_string_on_the_wire() {
        let mut page = document(vec![item("Task-184")], None, false);
        page.items[0].plan_id = String::new();
        assert_eq!(page.validate(), Ok(()));
        assert_eq!(page.items[0].plan_id, "");
    }
}
