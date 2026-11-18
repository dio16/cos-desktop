//! app-core history owner tests (DC-MP-NAV3).
//!
//! The owner thread performs bounded authenticated history reads and publishes
//! an immutable view. These tests exercise the public seam: request sequencing,
//! the unloaded/empty/end distinction, unauthorized and failure classes, and
//! invalidation on scope/authentication/shutdown changes. The client is a
//! scripted `DesktopClient`; no fixture page is ever synthesized.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use app_core::{
    HistoryEntry, HistoryFetchReply, HistoryPublication, HistoryRequest, HistoryStatus,
    ObservationHandle, ObservationSessionOwner, OwnerReply,
};
use cos_client::{
    ClientError, ClientEvent, ClientSource, CommandLookup, CommandStatus, DesktopClient,
    HistoryCursor, HistoryDocument, HistoryFetchError, HistoryItem, HistoryQuery, HistoryServer,
    HistoryStamp, MockCosClient, ObservationControl, PublicationAccess, PumpReport, SnapshotHandle,
};
use desktop_protocol::{
    CommandId, ObservationScopeRequest, OpenAuthorizedObservation, ScopeFilter, TaskId,
};

fn noop_waker() -> Waker {
    Waker::noop().clone()
}

fn poll_once<T>(reply: OwnerReply<T>) -> T {
    let mut reply = Box::pin(reply);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    match reply.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the reply must already be complete"),
    }
}

fn resolve<T>(owner: &mut ObservationSessionOwner, reply: OwnerReply<T>, now_ms: &mut u64) -> T {
    let mut reply = Box::pin(reply);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    for _ in 0..32 {
        if let Poll::Ready(value) = reply.as_mut().poll(&mut cx) {
            return value;
        }
        *now_ms += 1;
        owner.step(*now_ms);
    }
    panic!("owner reply was never answered");
}

fn task(label: &str) -> TaskId {
    TaskId::new(format!("task/{label}")).expect("task id")
}

fn scope() -> ObservationScopeRequest {
    ObservationScopeRequest {
        task_ids: Vec::new(),
        filters: vec![ScopeFilter {
            name: "view".to_owned(),
            value: "all".to_owned(),
        }],
    }
}

fn item(task_label: &str, iteration: u64) -> HistoryItem {
    HistoryItem {
        task_ref: format!("task/{task_label}"),
        record_ref: format!("task/{task_label}:{iteration}"),
        run_id: format!("task/{task_label}/run-{iteration}"),
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
        task_ref: format!("task/{task_label}"),
        items,
        next_cursor: next.map(str::to_owned),
        has_more: next.is_some(),
    }
}

/// Scripted owner-thread client: every history request pops one answer.
struct ScriptedHistoryClient {
    inner: MockCosClient,
    replies: VecDeque<Result<HistoryDocument, HistoryFetchError>>,
    requests: Vec<HistoryQuery>,
}

impl ScriptedHistoryClient {
    fn new(replies: Vec<Result<HistoryDocument, HistoryFetchError>>) -> Self {
        Self {
            inner: MockCosClient::from_fixture_str(include_str!(
                "../../cos-client/fixtures/task-184.json"
            ))
            .expect("authorized fixture"),
            replies: replies.into(),
            requests: Vec::new(),
        }
    }
}

impl DesktopClient for ScriptedHistoryClient {
    fn snapshot(&self) -> SnapshotHandle {
        self.inner.snapshot()
    }
    fn connection(&self) -> &cos_client::ConnectionState {
        self.inner.connection()
    }
    fn is_stale(&self) -> bool {
        self.inner.is_stale()
    }
    fn source(&self) -> ClientSource {
        self.inner.source()
    }
    fn last_error(&self) -> Option<&ClientError> {
        self.inner.last_error()
    }
    fn pump(&mut self, now_ms: u64) -> Vec<ClientEvent> {
        self.inner.pump(now_ms)
    }
    fn disconnect(&mut self) -> Vec<ClientEvent> {
        self.inner.disconnect()
    }
    fn request_resync(&mut self, reason: desktop_protocol::ResyncReason) -> Vec<ClientEvent> {
        self.inner.request_resync(reason)
    }
    fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, ClientError> {
        self.inner.stop_task(task_id)
    }
    fn fetch_catalog(&mut self) -> Result<cos_client::CatalogDocument, ClientError> {
        self.inner.fetch_catalog()
    }
    fn fetch_history(
        &mut self,
        request: &HistoryQuery,
    ) -> Result<HistoryDocument, HistoryFetchError> {
        self.requests.push(request.clone());
        self.replies
            .pop_front()
            .unwrap_or(Err(HistoryFetchError::Unsupported))
    }
    fn create_task(
        &mut self,
        envelope: desktop_protocol::CommandEnvelope,
    ) -> Result<CommandId, ClientError> {
        self.inner.create_task(envelope)
    }
    fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport {
        self.inner.pump_step(now_ms, budget)
    }
    fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus> {
        self.inner.command_statuses()
    }
    fn lookup_command(&self, id: &CommandId) -> CommandLookup<'_> {
        self.inner.lookup_command(id)
    }
    fn retain_command(&mut self, id: &CommandId) -> Result<(), ClientError> {
        self.inner.retain_command(id)
    }
    fn release_command(&mut self, id: &CommandId) {
        self.inner.release_command(id);
    }
    fn begin_authorized_observation(
        &mut self,
        request: OpenAuthorizedObservation,
    ) -> Result<(), ClientError> {
        self.inner.begin_authorized_observation(request)
    }
    fn publication_access(&self) -> PublicationAccess<'_> {
        self.inner.publication_access()
    }
    fn lookup_receipt_authorized(
        &mut self,
        request: desktop_protocol::AuthorizedReceiptLookup,
    ) -> Result<(), ClientError> {
        self.inner.lookup_receipt_authorized(request)
    }
    fn bind_observation_control(&mut self, control: ObservationControl) {
        self.inner.bind_observation_control(control);
    }
    fn observation_request(&self, scope: ObservationScopeRequest) -> OpenAuthorizedObservation {
        self.inner.observation_request(scope)
    }
    fn adopt_retained_registry(
        &mut self,
        records: BTreeMap<CommandId, CommandStatus>,
        references: BTreeMap<CommandId, usize>,
    ) -> Result<(), ClientError> {
        self.inner.adopt_retained_registry(records, references)
    }
}

struct Live {
    owner: ObservationSessionOwner,
    handle: ObservationHandle,
    now_ms: u64,
}

impl Live {
    fn start(replies: Vec<Result<HistoryDocument, HistoryFetchError>>) -> Self {
        let client = ScriptedHistoryClient::new(replies);
        let scope = client
            .inner
            .client()
            .transport()
            .scenario()
            .observation_request();
        let (mut owner, handle) = ObservationSessionOwner::new(Box::new(client));
        owner.arm_control(&handle);
        let mut live = Self {
            owner,
            handle,
            now_ms: 0,
        };
        let reply = live.handle.change_scope(scope);
        resolve(&mut live.owner, reply, &mut live.now_ms).expect("start observation");
        live.settle();
        live
    }

    fn fetch(&mut self, request: HistoryRequest) -> HistoryFetchReply {
        let reply = self.handle.fetch_history(request.query);
        let mut now = self.now_ms;
        let value = resolve(&mut self.owner, reply, &mut now);
        self.now_ms = now;
        value
    }

    fn history(&self) -> Option<Arc<HistoryPublication>> {
        let tagged = self.handle.history().ok()?;
        let mut publication = None;
        tagged
            .consume(|value| publication = Some(Arc::clone(value)))
            .ok()?;
        publication
    }

    fn settle(&mut self) {
        for _ in 0..24 {
            if self
                .handle
                .current()
                .ok()
                .and_then(|lease| lease.read_context())
                .is_some()
            {
                return;
            }
            self.now_ms += 1_000;
            self.owner.step(self.now_ms);
        }
        panic!("authorized observation did not settle");
    }
}

#[test]
fn a_served_page_publishes_rows_and_offers_an_explicit_continuation() {
    let mut live = Live::start(vec![
        Ok(document(
            "Task-184",
            vec![item("Task-184", 2), item("Task-184", 1)],
            Some("c1"),
        )),
        Ok(document("Task-184", vec![item("Task-184", 0)], None)),
    ]);
    assert_eq!(
        live.fetch(HistoryRequest::first(task("Task-184"))),
        HistoryFetchReply::Published
    );
    let page = live.history().expect("authorized publication");
    assert_eq!(page.task_ref, Some(task("Task-184")));
    assert_eq!(page.status, HistoryStatus::Loaded);
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].iteration, 2);
    assert_eq!(page.entries[0].plan_id, None, "an absent plan stays absent");
    assert!(page.has_more);
    assert_eq!(
        page.next_cursor.as_ref().map(HistoryCursor::as_str),
        Some("c1")
    );
    assert!(!page.at_bound);

    let older = HistoryRequest::older(&page, task("Task-184")).expect("continuation");
    assert_eq!(older.query.cursor().map(HistoryCursor::as_str), Some("c1"));
    assert_eq!(
        live.fetch(older),
        HistoryFetchReply::Published,
        "the second page is applied"
    );
    let end = live.history().expect("authorized publication");
    assert_eq!(end.status, HistoryStatus::End);
    assert_eq!(end.entries.len(), 3);
    assert!(!end.has_more);
    assert_eq!(end.next_cursor, None);
    assert!(
        HistoryRequest::older(&end, task("Task-184")).is_none(),
        "an authoritative end offers no load-older action"
    );
}

#[test]
fn an_authoritative_empty_page_is_empty_not_unloaded() {
    let mut live = Live::start(vec![Ok(document("Task-184", Vec::new(), None))]);
    // Before any request the view is unloaded, never an empty history.
    assert!(
        live.history().is_none(),
        "unloaded history is not an authoritative empty page"
    );
    assert_eq!(
        live.fetch(HistoryRequest::first(task("Task-184"))),
        HistoryFetchReply::Published
    );
    let publication = live.history().expect("authorized publication");
    assert_eq!(publication.status, HistoryStatus::Empty);
    assert!(publication.entries.is_empty());
    assert!(publication.status.is_settled());
    assert!(!publication.has_more);
}

#[test]
fn a_unauthorized_read_publishes_unauthorized_without_rows() {
    let mut live = Live::start(vec![Err(HistoryFetchError::Unauthorized)]);
    assert_eq!(
        live.fetch(HistoryRequest::first(task("Task-184"))),
        HistoryFetchReply::Published
    );
    let publication = live.history().expect("authorized publication");
    assert_eq!(publication.status, HistoryStatus::Unauthorized);
    assert!(publication.entries.is_empty());
    assert_eq!(publication.task_ref, Some(task("Task-184")));
}

#[test]
fn closed_failures_are_explicit_and_keep_no_partial_rows() {
    let cases = [
        (
            HistoryFetchError::NotFound,
            HistoryStatus::Error(app_core::HistoryError::NotFound),
        ),
        (
            HistoryFetchError::Conflict,
            HistoryStatus::Error(app_core::HistoryError::Conflict),
        ),
        (
            HistoryFetchError::InvalidRequest,
            HistoryStatus::Error(app_core::HistoryError::InvalidRequest),
        ),
        (
            HistoryFetchError::Malformed,
            HistoryStatus::Error(app_core::HistoryError::Malformed),
        ),
        (
            HistoryFetchError::Oversized,
            HistoryStatus::Error(app_core::HistoryError::Oversized),
        ),
        (
            HistoryFetchError::Unavailable,
            HistoryStatus::Error(app_core::HistoryError::Unavailable),
        ),
        (
            HistoryFetchError::Unsupported,
            HistoryStatus::Error(app_core::HistoryError::Unsupported),
        ),
        (HistoryFetchError::Forbidden, HistoryStatus::Unauthorized),
    ];
    for (error, expected) in cases {
        let mut live = Live::start(vec![Err(error)]);
        assert_eq!(
            live.fetch(HistoryRequest::first(task("Task-184"))),
            HistoryFetchReply::Published
        );
        let publication = live.history().expect("authorized publication");
        assert_eq!(publication.status, expected);
        assert!(publication.entries.is_empty());
    }
}

#[test]
fn a_scope_change_invalidates_the_loaded_history() {
    let mut live = Live::start(vec![Ok(document(
        "Task-184",
        vec![item("Task-184", 1)],
        None,
    ))]);
    assert_eq!(
        live.fetch(HistoryRequest::first(task("Task-184"))),
        HistoryFetchReply::Published
    );
    assert_eq!(
        live.history()
            .expect("authorized publication")
            .entries
            .len(),
        1
    );

    let retained = live.handle.history().expect("authorized history");
    let reply = live.handle.change_scope(scope());
    assert!(live.handle.history().is_err(), "scope close is immediate");
    assert!(
        retained
            .consume(|_| panic!("closed history disclosed"))
            .is_err()
    );
    let mut now = live.now_ms;
    let result = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(result.is_ok(), "the scope intent is accepted");
}

#[test]
fn an_authentication_change_invalidates_the_loaded_history() {
    let mut live = Live::start(vec![Ok(document(
        "Task-184",
        vec![item("Task-184", 1)],
        None,
    ))]);
    assert_eq!(
        live.fetch(HistoryRequest::first(task("Task-184"))),
        HistoryFetchReply::Published
    );

    let retained = live.handle.history().expect("authorized history");
    let reply = live.handle.authentication_changed();
    assert!(
        live.handle.history().is_err(),
        "authentication close is immediate"
    );
    assert!(
        retained
            .consume(|_| panic!("closed history disclosed"))
            .is_err(),
        "a read taken before the close cannot be consumed after it"
    );
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(
        live.handle.history().is_err(),
        "the owner's cleared publication is still withheld under the closed boundary"
    );
}

#[test]
fn a_closed_owner_answers_closed_instead_of_publishing() {
    let mut live = Live::start(vec![Ok(document(
        "Task-184",
        vec![item("Task-184", 1)],
        None,
    ))]);
    let shutdown = live.handle.shutdown();
    let mut now = live.now_ms;
    resolve(&mut live.owner, shutdown, &mut now);
    assert!(live.handle.history().is_err());
    let reply = live
        .handle
        .fetch_history(HistoryRequest::first(task("Task-184")).query);
    assert_eq!(poll_once(reply), HistoryFetchReply::Closed);
}

#[test]
fn every_request_is_bounded_to_the_runtime_page_size() {
    let mut live = Live::start(vec![
        Ok(document("Task-184", vec![item("Task-184", 1)], Some("c1"))),
        Ok(document("Task-184", vec![item("Task-184", 0)], None)),
    ]);
    let first = HistoryRequest::first(task("Task-184"));
    let _ = live.fetch(first);
    let publication = live.history().expect("authorized publication");
    let older = HistoryRequest::older(&publication, task("Task-184")).expect("continuation");
    assert_eq!(older.query.limit(), cos_client::HISTORY_LIMIT_MAX);
    let _ = live.fetch(older);
    // The scripted client records exactly one bounded request per fetch; a
    // limit outside 1..=20 cannot be constructed at all.
    let _ = HistoryQuery::new(task("Task-184"), 0, None).expect_err("limit below the bound");
    let _ = HistoryQuery::new(task("Task-184"), 21, None).expect_err("limit above the bound");
}

#[test]
fn a_history_entry_keeps_only_run_level_fields() {
    let entry = HistoryEntry {
        record_ref: "run-1:1".into(),
        run_id: "run-1".into(),
        iteration: 1,
        phase: "REFINE".into(),
        plan_id: None,
        created_at: "2026-11-08T00:00:00Z".into(),
    };
    // No canonical fact beyond the Runtime-projected run-level fields exists here.
    let clone = entry.clone();
    assert_eq!(clone.iteration, 1);
    assert_eq!(clone.plan_id, None);
}
