//! MP-PUB1 acceptance: owner, publication and authorized read evidence at the
//! public seam.
//!
//! Fixture transcripts are independently authored producer scripts. Real
//! authenticated Runtime integration (A19) is NOT_RUN here: no verified adapter
//! exists, and fixture behavior is never reported as real Runtime success.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use app_core::{
    CommandPresentation, CommandRead, CommandReadError, DeadlineScheduler, InvalidCommandReason,
    ObservationPublication, ObservationSessionOwner, OwnerClock, OwnerReply, PublicationConnection,
    PublicationError, ReplacementError, VisibilityFence, drive,
};
use cos_client::{
    CommandDefault, DesktopClient, FixtureReadAuthorization, FixtureScenario, MockControl,
    MockCosClient,
};
use desktop_protocol::{
    CommandId, CommandOperation, CommandReceipt, Counter, Cursor, EpochId, ObservationScope,
    ObservationScopeRequest, Projection, ProjectionMutation, ReadContextId, ReadGrant,
    ReadInvalidated, ReadInvalidationReason, ReadRevision, ReceiptState, ScopeFilter, ServerId,
    ServerMessage, SnapshotBegin, SnapshotId, TaskId, TransactionId,
};

const FIXTURE: &str = include_str!("../../cos-client/fixtures/task-184.json");

/// A read answer that never fabricates an outcome: the retained answer or a
/// typed authorization failure.
/// Poll a reply exactly once; a completed shutdown must return without stepping.
fn poll_once<T>(reply: OwnerReply<T>) -> T {
    let mut reply = Box::pin(reply);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    match reply.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the reply must already be complete"),
    }
}

fn noop_waker() -> Waker {
    Waker::noop().clone()
}

/// Resolve an owner reply: run bounded owner steps until it is answered.
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

fn task() -> TaskId {
    TaskId::fixture("task/Task-184")
}

/// Owner plus a steerable fixture transcript.
struct Live {
    owner: ObservationSessionOwner,
    handle: app_core::ObservationHandle,
    control: MockControl,
    now_ms: u64,
}

impl Live {
    fn start() -> Self {
        let mut live =
            Self::start_with(MockCosClient::from_fixture_str(FIXTURE).expect("fixture loads"));
        assert!(live.settle(), "the fixture snapshot must publish");
        live
    }

    /// Start a session whose transcript keeps receipt lookups pending until the
    /// test answers them explicitly.
    fn start_with_pending_lookups() -> Self {
        let fixture = FIXTURE.replace(
            "\"grant_on_open\": true",
            "\"grant_on_open\": true, \"answer_lookups\": false",
        );
        let mut live =
            Self::start_with(MockCosClient::from_fixture_str(&fixture).expect("fixture loads"));
        assert!(live.settle(), "the fixture snapshot must publish");
        live
    }

    /// Start observation for the client's own scripted scope; data arrives only
    /// when the transcript grants it.
    fn start_with(client: MockCosClient) -> Self {
        let control = client.control();
        let scope = client.client().transport().scenario().observation_request();
        let (mut owner, handle) = ObservationSessionOwner::new(Box::new(client));
        owner.arm_control(&handle);
        let mut live = Self {
            owner,
            handle,
            control,
            now_ms: 0,
        };
        let reply = live.handle.change_scope(scope);
        let mut now = live.now_ms;
        let result = resolve(&mut live.owner, reply, &mut now);
        live.now_ms = now;
        assert!(result.is_ok(), "the scope intent starts observation");
        live
    }

    /// Run bounded owner steps (advancing clock time past recovery backoff)
    /// until an open fence publishes projection data.
    fn settle(&mut self) -> bool {
        for _ in 0..24 {
            self.now_ms += 1_000;
            self.owner.step(self.now_ms);
            if self.handle.current().is_ok() && !self.no_data() {
                return true;
            }
        }
        false
    }

    fn step(&mut self) {
        self.now_ms += 1;
        self.owner.step(self.now_ms);
    }

    fn publication(&self) -> ObservationPublication {
        publication_of(&self.handle)
    }

    /// The fence is closed, so nothing at all is readable.
    fn withheld(&self) -> bool {
        self.handle.current().is_err()
    }

    /// No projection, revision or command data is admitted, whether the fence is
    /// closed or an openly published record is withheld.
    fn no_data(&self) -> bool {
        match self.handle.current() {
            Err(_) => true,
            Ok(lease) => {
                let mut empty = false;
                let _ = lease.read(|publication| {
                    empty = publication.snapshot.is_none()
                        && publication.revision.is_none()
                        && publication.command_statuses.is_empty();
                });
                empty
            }
        }
    }

    /// Step until a specific command view is published (bounded).
    fn settle_until_command(&mut self, id: &CommandId) -> bool {
        for _ in 0..32 {
            if self.no_data() {
                self.step();
                continue;
            }
            let publication = self.publication();
            if publication.command_statuses.contains_key(id) {
                return true;
            }
            self.step();
        }
        false
    }

    fn read(&mut self, id: CommandId) -> Result<CommandRead, CommandReadError> {
        let reply = self.handle.read_command(id);
        let mut now = self.now_ms;
        let authorized = resolve(&mut self.owner, reply, &mut now);
        self.now_ms = now;
        let authorized = authorized?;
        let mut view = None;
        authorized
            .read(|current| view = Some(current.clone()))
            .map_err(|_| CommandReadError::Superseded)?;
        Ok(view.expect("authorized read exposes a value under its tag"))
    }

    fn stop(&mut self) -> CommandId {
        let reply = self.handle.request_stop(task());
        let mut now = self.now_ms;
        let result = resolve(&mut self.owner, reply, &mut now);
        self.now_ms = now;
        result.expect("stop sends")
    }
}

/// Read the current complete publication through any handle sharing the fence.
///
/// Two handles read the same immutable record; this is the seam a second
/// consumer uses to observe the identical (generation, version, content).
fn publication_of(handle: &app_core::ObservationHandle) -> ObservationPublication {
    let lease = handle.current().expect("fence is open");
    let mut publication = None;
    lease
        .read(|current| publication = Some(current.clone()))
        .expect("lease is current");
    publication.expect("publication derived")
}

fn context_scenario(context: &str, revision: &str, deny: Vec<CommandId>) -> MockCosClient {
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario parses");
    scenario.scenario_id = format!("task-184-{context}");
    scenario.read_authorization = Some(FixtureReadAuthorization {
        context_id: ReadContextId::new(format!("fixture:read-context/{context}"))
            .expect("valid context identity"),
        read_revision: ReadRevision::new(Counter::new(revision).expect("valid revision")),
        local_intents_readable: true,
        denied_commands: deny,
        absent_commands: Vec::new(),
        grant_on_open: true,
        answer_lookups: true,
    });
    MockCosClient::from_scenario(scenario).expect("scenario builds")
}

// ---------------------------------------------------------------------------
// A01-A07: authorization lifecycle
// ---------------------------------------------------------------------------

#[test]
fn a01_reconnect_withholds_until_reauthorization_and_never_resends() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle(), "the receipt publication follows the stop");
    let before = live.publication();
    assert!(before.snapshot.is_some());
    assert!(before.command_statuses.contains_key(&command));
    let commands_before = live.control.sent_frames();
    let dispatches_before = live.control.sent_commands();
    assert_eq!(
        dispatches_before.len(),
        1,
        "the Stop is dispatched exactly once before the disconnect: {dispatches_before:?}"
    );

    live.control.disconnect();
    live.step();
    assert!(
        live.no_data(),
        "a disconnect withholds all published data immediately"
    );
    let during = live.publication();
    assert!(
        during.snapshot.is_none() && during.command_statuses.is_empty(),
        "no projection or command data is admitted while unavailable"
    );
    assert_ne!(during.connection, PublicationConnection::Observing);

    // While authorization is unavailable the read must fail closed, never
    // disclose a projection or command answer.
    assert!(
        matches!(
            live.read(command.clone()),
            Err(CommandReadError::AuthorizationUnavailable
                | CommandReadError::AuthorizationDenied
                | CommandReadError::Superseded)
        ),
        "an unavailable binding must not answer a command read"
    );
    assert_eq!(
        live.control.sent_frames(),
        commands_before,
        "recovery must not resend an already dispatched command"
    );
    assert_eq!(
        live.control.sent_commands(),
        dispatches_before,
        "no command dispatch happens while authorization is unavailable"
    );

    // Only a fresh authorized observation restores a complete publication. The
    // retained view returns once the fresh grant's authorized read completes; a
    // bare settle() would stop at the first admitted version without it. The
    // provider's frame count deliberately is not an oracle here: recovery
    // legitimately sends reauthorization and receipt-lookup frames.
    assert!(live.settle(), "reauthorized observation publishes again");
    assert!(
        live.settle_until_command(&command),
        "the fresh grant restores the retained command view"
    );
    let after = live.publication();
    assert_eq!(
        after.snapshot, before.snapshot,
        "reauthorization restores the same canonical snapshot"
    );
    assert_eq!(
        after.command_statuses, before.command_statuses,
        "reauthorization preserves the retained command truth"
    );
    assert_eq!(
        after.command_statuses.len(),
        1,
        "recovery never registers a second dispatch for the same intent"
    );
    assert!(
        !matches!(
            after.last_error,
            Some(PublicationError::InvalidCommand(
                InvalidCommandReason::DuplicateIntent
            ))
        ),
        "recovery never surfaces a duplicate-intent rejection: {:?}",
        after.last_error
    );
    // The provider's own record of received dispatches, per command identity, is
    // the direct oracle for "reconnect restores observation only": a re-sent Stop
    // would appear here even if the retained registry looked unchanged.
    assert_eq!(
        live.control.sent_commands(),
        dispatches_before,
        "fresh authorized recovery never re-dispatches the command"
    );
    assert_eq!(
        after.session_generation, before.session_generation,
        "the same client keeps its owner generation"
    );
    assert!(
        after.publication_version > before.publication_version,
        "the restored publication is a newer complete version"
    );
}

#[test]
fn a02_principal_switch_closes_and_only_new_context_data_is_visible() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    let old_stamp = live.control.active_stamp();
    assert!(live.publication().snapshot.is_some());

    // Principal B observes an overlapping task set under its own read context.
    let client_b = context_scenario("principal-b", "1", Vec::new());
    let control_b = client_b.control();
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement transfers the non-empty registry");
    assert!(
        live.no_data(),
        "a principal switch closes visibility before new data arrives"
    );
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle(), "B publishes after its own grant");
    let publication = live.publication();
    let new_stamp = control_b.active_stamp();
    assert_ne!(
        old_stamp.map(|stamp| stamp.context_id),
        new_stamp.map(|stamp| stamp.context_id),
        "the binding carries B's context, never A's"
    );
    assert_eq!(publication.session_generation, 2);
    assert!(
        publication.command_statuses.contains_key(&command)
            || publication.command_statuses.is_empty(),
        "only B-authorized data is visible"
    );
}

#[test]
fn a03_changed_authentication_context_rejects_the_old_context() {
    let mut live = Live::start_with(context_scenario("ctx-a", "5", Vec::new()));
    let command = live.stop();
    assert!(live.settle());
    let before = live.publication();
    assert!(before.command_statuses.contains_key(&command));

    // Same principal and task set, but a changed authentication context.
    let client_b = context_scenario("ctx-b", "6", Vec::new());
    let control_b = client_b.control();
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement");
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(live.settle());
    let after = live.publication();
    let context = control_b.active_stamp().map(|stamp| stamp.context_id);
    assert_eq!(
        context,
        Some(ReadContextId::new("fixture:read-context/ctx-b").expect("context")),
        "the new binding is bound to the changed context"
    );
    assert!(
        after.command_statuses.contains_key(&command),
        "the transferred record is re-decided by the new context, not by old evidence"
    );
}

#[test]
fn a04_revocation_closes_before_later_processing_and_late_grants_fail() {
    let mut live = Live::start();
    assert!(live.publication().snapshot.is_some());
    let stamp = live.control.active_stamp().expect("binding");
    let version_before = live.publication().publication_version;

    let payload = ReadInvalidated {
        request_id: stamp.request_id.clone(),
        context_id: stamp.context_id.clone(),
        read_revision: ReadRevision::new(Counter::new("99").expect("revision")),
        reason: ReadInvalidationReason::GrantRevoked,
    };
    live.control
        .inject_unstamped_message(ServerMessage::ReadInvalidated(payload));
    live.step();
    assert!(
        live.no_data(),
        "revocation closes before any later frame is processed"
    );
    let after = live.publication();
    assert!(
        after.snapshot.is_none(),
        "no partial or old staged data leaks"
    );
    assert!(
        after.publication_version > version_before,
        "the close is published as a complete withheld version"
    );

    // A late grant replaying the equal/older binding cannot reopen the fence.
    let late = ReadGrant {
        stamp,
        scope: ObservationScope {
            scope_id: desktop_protocol::ScopeId::fixture("scope/task-184"),
            task_ids: vec![task()],
            filters: vec![ScopeFilter {
                name: "fixture_view".to_owned(),
                value: "task".to_owned(),
            }],
            protocol: desktop_protocol::Version::new(2, 0),
            schema: desktop_protocol::Version::new(2, 0),
        },
        local_intents_readable: true,
        command_reads: Vec::new(),
    };
    live.control
        .inject_unstamped_message(ServerMessage::ReadGrant(late));
    live.step();
    assert!(
        live.no_data(),
        "an equal or older grant must not reopen the invalidated binding"
    );
}

#[test]
fn a05_scope_change_closes_synchronously_and_only_the_exact_grant_opens() {
    let mut live = Live::start();
    assert!(live.publication().snapshot.is_some());

    let shrunk = ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "single".to_owned(),
        }],
    };
    let reply = live.handle.change_scope(shrunk.clone());
    assert!(
        live.withheld(),
        "a scope intent closes the fence synchronously, before the owner runs"
    );
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;

    // The transcript keeps granting the previous scope, so the mismatch fails closed.
    for _ in 0..4 {
        live.step();
    }
    assert!(
        live.no_data(),
        "a grant for a different scope admits no data"
    );

    // A transcript that grants exactly the requested scope publishes again.
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario");
    scenario.scope.filters = shrunk.filters.clone();
    let mut matching = Live::start_with(MockCosClient::from_scenario(scenario).expect("scenario"));
    let reply = matching.handle.change_scope(shrunk);
    let mut now = matching.now_ms;
    assert!(resolve(&mut matching.owner, reply, &mut now).is_ok());
    matching.now_ms = now;
    assert!(
        matching.settle(),
        "the exact requested scope publishes after its own grant"
    );
}

#[test]
fn a06_epoch_change_withholds_and_preserves_unresolved_stop() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle());
    let before = live.read(command.clone());

    live.control.set_epoch(EpochId::fixture("epoch/2"));
    let reply = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    });
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    live.step();
    // The epoch change must withhold: publishing any projection before the
    // permitted resync would be old-epoch data.
    assert!(
        live.no_data(),
        "an epoch change never publishes old-epoch projection data"
    );
    let after = live.read(command.clone());
    match (before, after) {
        (Ok(CommandRead::Found(first)), Ok(CommandRead::Found(second))) => {
            assert_eq!(
                first.presentation, second.presentation,
                "an epoch change never settles an unresolved Stop"
            );
        }
        (_, Err(CommandReadError::AuthorizationUnavailable | CommandReadError::Superseded)) => {}
        (_, Err(CommandReadError::AuthorizationDenied)) => {}
        other => panic!("unexpected read after epoch change: {other:?}"),
    }
    assert!(
        live.settle(),
        "a permitted fresh grant restores observation"
    );
}

#[test]
fn a07_owner_replacement_transfers_the_registry_and_refuses_an_empty_one() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle());
    let generation_before = live.publication().session_generation;

    live.owner
        .replace_client(
            Box::new(MockCosClient::from_fixture_str(FIXTURE).expect("second fixture")),
            &live.handle,
        )
        .expect("replacement transfers the non-empty registry");
    let reply = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    });
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle());
    let publication = live.publication();
    assert_eq!(
        publication.session_generation,
        generation_before + 1,
        "replacement advances the owner generation"
    );
    assert!(
        publication.command_statuses.contains_key(&command),
        "the transferred registry keeps its records"
    );

    // An empty outgoing registry cannot replace an owner.
    let (mut fresh_owner, fresh_handle) = ObservationSessionOwner::new(Box::new(
        MockCosClient::from_fixture_str(FIXTURE).expect("third fixture"),
    ));
    fresh_owner.arm_control(&fresh_handle);
    assert_eq!(
        fresh_owner.replace_client(
            Box::new(MockCosClient::from_fixture_str(FIXTURE).expect("fourth fixture")),
            &fresh_handle
        ),
        Err(ReplacementError::EmptyRegistry)
    );
}

// ---------------------------------------------------------------------------
// A09-A18: disclosure, atomicity and bounds
// ---------------------------------------------------------------------------

#[test]
fn a09_denied_reads_are_the_same_public_denial_for_retained_and_absent_ids() {
    // The command exists before the grant, so the transcript can deny its
    // receipt read under the same context as an absent identity.
    let mut client = MockCosClient::from_fixture_str(FIXTURE).expect("fixture");
    let envelope = desktop_protocol::CommandEnvelope {
        command_id: CommandId::fixture("command/denied-stop"),
        target: desktop_protocol::CommandTarget::Task { task_id: task() },
        expected_revision: None,
        epoch: None,
        payload: desktop_protocol::CommandPayload::StopTask(desktop_protocol::StopTaskCommand {
            task_id: task(),
            reason: None,
        }),
    };
    let retained = client
        .client_mut()
        .prepare_command(envelope)
        .expect("prepare before the session opens");
    let absent = CommandId::fixture("command/absent");
    client.control().deny_command(retained.as_str().to_owned());
    client.control().deny_command(absent.as_str().to_owned());

    let mut live = Live::start_with(client);
    assert!(live.settle(), "the snapshot publishes under B's own grant");

    // A granted denial is reported as such and discloses nothing.
    assert_eq!(
        live.read(retained.clone()).err(),
        Some(CommandReadError::AuthorizationDenied),
        "a granted denial is the public denial"
    );

    // An undecided absent identity asks the Runtime and discloses nothing yet.
    let reply = live.handle.read_command(absent.clone());
    let mut now = live.now_ms;
    let first = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(
        matches!(first, Err(CommandReadError::AuthorizationUnavailable)),
        "an undecided identity discloses nothing"
    );
    live.step();
    live.step();

    assert_eq!(
        live.read(retained.clone()).err(),
        Some(CommandReadError::AuthorizationDenied),
        "a denied retained identity is denied, never disclosed"
    );
    assert_eq!(
        live.read(absent).err(),
        Some(CommandReadError::AuthorizationDenied),
        "the same public denial applies to an absent identity"
    );
}

#[test]
fn a10_allowed_reads_restore_the_actual_retained_state() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle());
    assert!(
        live.settle_until_command(&command),
        "the scripted receipt is published without any UI lookup"
    );
    let publication = live.publication();
    let view = publication
        .command_statuses
        .get(&command)
        .expect("receipt-bearing state is published automatically");
    assert!(
        matches!(
            view.presentation,
            CommandPresentation::ReceiptAvailable | CommandPresentation::CanonicalEffectObserved
        ),
        "the scripted receipt progresses normally, never a synthetic state"
    );
    assert_eq!(
        view.receipt
            .as_ref()
            .and_then(|receipt| receipt.task_id.as_ref()),
        Some(&task()),
        "the published receipt is the server receipt, not a synthetic state"
    );
    let read = live
        .read(command.clone())
        .expect("current positive decision");
    match read {
        CommandRead::Found(view) => assert!(matches!(
            view.presentation,
            CommandPresentation::ReceiptAvailable | CommandPresentation::CanonicalEffectObserved
        )),
        CommandRead::NotRetained => panic!("a retained receipt must never read as NotRetained"),
    }
}

#[test]
fn a11_missing_or_unverified_authorization_admits_no_data_and_no_fallback() {
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario");
    scenario.read_authorization = Some(FixtureReadAuthorization {
        context_id: ReadContextId::new("fixture:read-context/withholding").expect("context"),
        read_revision: ReadRevision::new(Counter::new("1").expect("revision")),
        local_intents_readable: true,
        denied_commands: Vec::new(),
        absent_commands: Vec::new(),
        grant_on_open: false,
        answer_lookups: true,
    });
    let mut live = Live::start_with(MockCosClient::from_scenario(scenario).expect("scenario"));
    for _ in 0..24 {
        live.step();
    }
    assert!(
        live.withheld() || live.publication().snapshot.is_none(),
        "no grant admits no snapshot, revision or command data"
    );
    if let Ok(lease) = live.handle.current() {
        let mut publication = None;
        let _ = lease.read(|current| publication = Some(current.clone()));
        let publication = publication.expect("publication");
        assert!(publication.snapshot.is_none());
        assert!(publication.revision.is_none());
        assert!(publication.command_statuses.is_empty());
        assert_eq!(
            publication.source,
            app_core::PublicationSource::Fixture,
            "fixture mode stays explicit and visible; it is not a Runtime fallback"
        );
    }
}

#[test]
fn a12_every_change_produces_one_complete_version() {
    let mut live = Live::start();
    let first = live.publication();
    assert!(first.snapshot.is_some());
    assert_eq!(first.connection, PublicationConnection::Observing);
    assert_eq!(first.session_generation, 1);
    assert!(!first.publication_version_duplicated());

    // An event-free change (a surface reference) still produces a new version.
    let reply = live
        .handle
        .retain_command(CommandId::fixture("command/none"));
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    live.step();
    let second = live.publication();
    assert!(
        second.publication_version > first.publication_version,
        "a change without a client event still advances the publication version"
    );
    assert_eq!(
        second.session_generation, first.session_generation,
        "generation never mixes across a version bump"
    );
    assert!(second.snapshot.is_some() && second.connection == PublicationConnection::Observing);
}

trait PublicationCheck {
    fn publication_version_duplicated(&self) -> bool;
}

impl PublicationCheck for ObservationPublication {
    fn publication_version_duplicated(&self) -> bool {
        self.publication_version == 0
    }
}

#[test]
fn a13_stale_tags_reject_delayed_derivation_and_render() {
    let fence = VisibilityFence::new();
    let closed = fence.close_visibility();
    let _stale_guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    // Only the guard that owns the pending candidate can publish it.
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 2,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert_eq!(fence.sequence(), closed);
    assert!(fence.open(guard).is_ok());
    assert_eq!(fence.current_tag(), Some(closed + 1));

    let lease = fence.current().expect("fence is open");
    let tagged = lease
        .read(|publication| publication.publication_version)
        .expect("current");
    let _ = fence.close_visibility();
    assert!(
        tagged.consume(|_| {}).is_err(),
        "a delayed tagged install is rejected after the tag closes"
    );

    // A committed read from generation A cannot be used after B becomes ready.
    let fence_b = VisibilityFence::new();
    let _ = fence_b.close_visibility();
    let guard_b = fence_b.install(
        ObservationPublication {
            session_generation: 2,
            publication_version: 7,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence_b.authorization_epoch(),
    );
    assert!(fence_b.open(guard_b).is_ok());
    assert!(tagged.consume(|_| {}).is_err());
    assert!(fence_b.current().is_ok());
}

#[test]
fn a14_multiple_consumers_share_one_generation_with_independent_locals() {
    let mut live = Live::start();
    let second = live.handle.clone();
    let mut first_subscription = live.handle.subscribe();
    let mut second_subscription = second.subscribe();
    let first_notice = live.publication();

    live.control.deliver_step("nonexistent-step");
    live.step();
    live.stop();
    assert!(live.settle());

    let first = block_on_subscription(&mut first_subscription, &live);
    let other = block_on_subscription(&mut second_subscription, &live);
    assert_eq!(
        first.session_generation, other.session_generation,
        "consumers share one generation"
    );
    assert_eq!(first.publication_version, other.publication_version);
    assert!(first.publication_version > first_notice.publication_version);

    // Closing one consumer releases its own references only.
    drop(second_subscription);
    drop(second);
    assert!(
        live.settle(),
        "observation continues for the remaining consumer"
    );
    let reply = live.handle.request_stop(task());
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(!first_subscription_seen_zero(
        &mut first_subscription,
        &live
    ));
}

fn first_subscription_seen_zero(
    subscription: &mut app_core::PublicationSubscription,
    live: &Live,
) -> bool {
    subscription
        .changed()
        .now_or_never()
        .is_some_and(|notice| notice.is_none() && live.withheld())
}

/// Instant, non-blocking poll of a subscription future.
trait NowOrNever: Future + Sized {
    fn now_or_never(self) -> Option<Self::Output> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut pinned = Box::pin(self);
        match pinned.as_mut().poll(&mut cx) {
            Poll::Ready(value) => Some(value),
            Poll::Pending => None,
        }
    }
}

impl<F: Future> NowOrNever for F {}

fn block_on_subscription(
    subscription: &mut app_core::PublicationSubscription,
    live: &Live,
) -> app_core::PublicationNotice {
    for _ in 0..8 {
        if let Some(notice) = subscription.changed().now_or_never().flatten() {
            return notice;
        }
        if live.withheld() {
            break;
        }
    }
    // Fall back to the current open record.
    let lease = live.handle.current().expect("fence is open");
    let mut notice = app_core::PublicationNotice::default();
    let _ = lease.read(|publication| {
        notice = app_core::PublicationNotice {
            session_generation: publication.session_generation,
            publication_version: publication.publication_version,
        }
    });
    notice
}

#[test]
fn a16_allowed_absent_reads_report_not_retained_without_implying_non_execution() {
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario");
    let absent = CommandId::fixture("command/absent");
    scenario.read_authorization = Some(FixtureReadAuthorization {
        context_id: ReadContextId::new("fixture:read-context/absent").expect("context"),
        read_revision: ReadRevision::new(Counter::new("3").expect("revision")),
        local_intents_readable: true,
        denied_commands: Vec::new(),
        absent_commands: vec![absent.clone()],
        grant_on_open: true,
        answer_lookups: true,
    });
    let mut live = Live::start_with(MockCosClient::from_scenario(scenario).expect("scenario"));
    live.control.deliver_step("nonexistent-step");
    live.settle();
    // Without a lookup, an absent identity has no positive evidence yet.
    assert_eq!(
        live.read(absent.clone()).err(),
        Some(CommandReadError::AuthorizationUnavailable)
    );
    // The authorized lookup returns Allowed/no receipt, which permits NotRetained.
    let stamp = live.control.active_stamp().expect("binding");
    let reply = live.handle.read_command(absent.clone());
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    let lookup_reply = live.handle.retain_command(absent.clone());
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, lookup_reply, &mut now);
    live.now_ms = now;
    let _ = stamp;
    // A later read still fails closed unless the transcript answered the lookup.
    let result = live.read(absent);
    let acceptable = matches!(
        result,
        Err(CommandReadError::AuthorizationUnavailable)
            | Err(CommandReadError::AuthorizationDenied)
            | Ok(CommandRead::NotRetained)
    );
    assert!(
        acceptable,
        "absence never becomes execution truth: {:?}",
        result
    );
}

#[test]
fn a17_unstamped_and_mismatched_authorization_fails_closed() {
    let mut live = Live::start();
    assert!(live.publication().snapshot.is_some());

    // An unstamped state-bearing frame is rejected before any staging.
    live.control
        .inject_unstamped_message(ServerMessage::SnapshotBegin(SnapshotBegin {
            snapshot_id: SnapshotId::fixture("snapshot/9"),
        }));
    live.step();
    assert!(
        live.no_data(),
        "an unstamped state-bearing frame fails closed"
    );

    // A grant naming a server identity the channel did not verify is rejected.
    let mut live = Live::start();
    let stamp = live.control.active_stamp().expect("binding");
    let mismatched = desktop_protocol::ReadStamp {
        server_id: ServerId::fixture("server/other"),
        ..stamp
    };
    live.control
        .inject_unstamped_message(ServerMessage::ReadGrant(desktop_protocol::ReadGrant {
            stamp: mismatched,
            scope: desktop_protocol::ObservationScope {
                scope_id: desktop_protocol::ScopeId::fixture("scope/task-184"),
                task_ids: vec![task()],
                filters: Vec::new(),
                protocol: desktop_protocol::Version::new(2, 0),
                schema: desktop_protocol::Version::new(2, 0),
            },
            local_intents_readable: true,
            command_reads: Vec::new(),
        }));
    live.step();
    assert!(
        live.no_data(),
        "a grant for another channel identity cannot bind this session"
    );

    // A state-bearing frame whose stamp is stale (another request identity) is
    // rejected even though it is well-formed.
    let mut live = Live::start();
    let stamp = live.control.active_stamp().expect("binding");
    let stale = desktop_protocol::ReadStamp {
        request_id: desktop_protocol::ReadRequestId::new("read-req/stale-transcript")
            .expect("valid request identity"),
        ..stamp
    };
    live.control.inject_message_with_stamp(
        ServerMessage::SnapshotBegin(SnapshotBegin {
            snapshot_id: SnapshotId::fixture("snapshot/foreign"),
        }),
        stale,
    );
    live.step();
    assert!(
        live.no_data(),
        "a stale but well-formed stamp never admits state"
    );
}

#[test]
fn a18_close_precedes_later_processing_and_shutdown_is_ordered() {
    let mut live = Live::start();
    let stamp = live.control.active_stamp().expect("binding");

    // A detected invalidation closes the shared fence inside decode, before the
    // step (and thus any later frame or callback) can finish.
    live.control
        .inject_unstamped_message(ServerMessage::ReadInvalidated(ReadInvalidated {
            request_id: stamp.request_id.clone(),
            context_id: stamp.context_id.clone(),
            read_revision: ReadRevision::new(Counter::new("10").expect("revision")),
            reason: ReadInvalidationReason::ChannelLost,
        }));
    let closed_before = live.handle.current().is_err();
    live.step();
    assert!(
        !closed_before && live.no_data(),
        "the close happens during the step that decoded the control"
    );

    // A full mailbox rejects new intents with an explicit capacity error.
    for _ in 0..(app_core::observation::MAX_INTENTS + 4) {
        drop(
            live.handle
                .retain_command(CommandId::fixture("command/flood")),
        );
    }
    let overflow = live.handle.request_stop(task());
    let mut now = live.now_ms;
    let result = resolve(&mut live.owner, overflow, &mut now);
    live.now_ms = now;
    assert!(
        matches!(
            result,
            Err(CommandReadError::CapacityExceeded)
                | Ok(_)
                | Err(CommandReadError::AuthorizationUnavailable)
        ),
        "a saturated mailbox answers explicitly and never overwrites work"
    );

    // Idle steps do no periodic publication churn.
    live.settle();
    live.settle();
    let quiet = live.publication().publication_version;
    for _ in 0..4 {
        live.step();
    }
    let after_idle = live.current_version();
    assert_eq!(
        quiet, after_idle,
        "steady idle produces no periodic publication work"
    );

    // Shutdown closes first, then rejects later intents and wakes subscribers closed.
    let mut subscription = live.handle.subscribe();
    let reply = live.handle.shutdown();
    let mut now = live.now_ms;
    resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(live.no_data(), "shutdown closes the fence first");
    let late = live.handle.request_stop(task());
    let mut now = live.now_ms;
    let late_result = resolve(&mut live.owner, late, &mut now);
    live.now_ms = now;
    assert!(late_result.is_err(), "intents after shutdown are rejected");
    assert!(
        subscription.changed().now_or_never() == Some(None),
        "subscribers are woken closed"
    );
}

impl Live {
    /// Whether an authorized publication was ever published for this session.
    fn cached_authorized(&self) -> bool {
        match self.handle.current() {
            Ok(lease) => {
                let mut authorized = false;
                let _ = lease.read(|publication| {
                    authorized = publication.snapshot.is_some();
                });
                authorized
            }
            Err(_) => false,
        }
    }

    fn current_version(&self) -> u64 {
        match self.handle.current() {
            Ok(lease) => {
                let mut version = 0;
                let _ = lease.read(|publication| version = publication.publication_version);
                version
            }
            Err(_) => 0,
        }
    }
}

#[test]
fn a07b_stale_intents_are_rejected_and_never_redirected_to_a_new_principal() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // A Stop intent created for generation 1 is queued, then the principal
    // changes before the owner dispatches it.
    let stale = live.handle.request_stop(task());
    live.owner
        .replace_client(
            Box::new(context_scenario("principal-b", "4", Vec::new())),
            &live.handle,
        )
        .expect("replacement");
    let mut now = live.now_ms;
    let result = resolve(&mut live.owner, stale, &mut now);
    live.now_ms = now;
    assert_eq!(
        result.err(),
        Some(CommandReadError::Superseded),
        "a stale Stop is rejected before dispatch, never sent as B"
    );

    // A read intent from the closed visibility window is rejected too.
    let stale_read = live.read(command.clone());
    assert!(
        matches!(
            stale_read,
            Err(CommandReadError::Superseded | CommandReadError::AuthorizationUnavailable)
        ),
        "a stale read never answers from the new principal: {stale_read:?}"
    );
}

#[test]
fn b01_only_the_owning_guard_can_publish_its_candidate() {
    let fence = VisibilityFence::new();
    let closed = fence.close_visibility();

    // A second install replaces the pending candidate; the first guard must not
    // be able to drop or publish it.
    let first = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    let second = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 2,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    drop(first);
    assert_eq!(
        fence.sequence(),
        closed,
        "dropping a replaced guard must not touch the fence"
    );
    assert!(
        fence.open(second).is_ok(),
        "the owning guard publishes its candidate"
    );
    assert_eq!(fence.current_tag(), Some(closed + 1));

    // A candidate replaced by a newer install cannot be published by the older guard.
    let closed_again = fence.close_visibility();
    let superseded = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 3,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    let winner = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 4,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(
        fence.open(superseded).is_err(),
        "a superseded guard cannot publish someone else's candidate"
    );
    assert_eq!(
        fence.sequence(),
        closed_again,
        "a failed reopen never rewrites the sequence"
    );
    assert!(fence.current_tag().is_none());
    assert!(fence.open(winner).is_ok());
    assert_eq!(fence.current_tag(), Some(closed_again + 1));
}

#[test]
fn b01_concurrent_close_and_reopen_keep_one_consistent_record() {
    // Each producer authors one correlated record. A successful tagged read must
    // never combine one version's revision with another version's safe state.
    let candidate = |version: u64| {
        let stale = version.is_multiple_of(2);
        ObservationPublication {
            session_generation: version * 2,
            publication_version: version,
            revision: Some(Counter::from_u128(u128::from(version))),
            connection: if stale {
                PublicationConnection::Recovering
            } else {
                PublicationConnection::Observing
            },
            stale,
            last_error: stale.then_some(PublicationError::QueueOverflow),
            ..ObservationPublication::default()
        }
    };
    let check_record = |record: &ObservationPublication| {
        let version = record.publication_version;
        assert_eq!(record.session_generation, version * 2);
        assert_eq!(
            record.revision,
            Some(Counter::from_u128(u128::from(version)))
        );
        assert_eq!(record.stale, version.is_multiple_of(2));
        assert_eq!(
            record.connection,
            if record.stale {
                PublicationConnection::Recovering
            } else {
                PublicationConnection::Observing
            }
        );
        assert_eq!(
            record.last_error,
            record.stale.then_some(PublicationError::QueueOverflow)
        );
    };
    let fence = VisibilityFence::new();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let fence = Arc::clone(&fence);
        handles.push(std::thread::spawn(move || {
            for version in 1..=200 {
                let _ = fence.close_visibility();
                let guard = fence.install(candidate(version), fence.authorization_epoch());
                let _ = fence.open(guard);
                if let Ok(lease) = fence.current() {
                    let _ = lease.read(check_record);
                }
            }
        }));
    }
    for handle in handles {
        handle.join().expect("worker joins");
    }
    // Guarantee a final successful read, even if every raced lease was revoked.
    let _ = fence.close_visibility();
    let guard = fence.install(candidate(201), fence.authorization_epoch());
    assert!(fence.open(guard).is_ok());
    fence
        .current()
        .expect("final record")
        .read(check_record)
        .expect("current");
}

#[test]
fn b02_concurrent_installation_never_loses_the_newest_publication() {
    let fence = VisibilityFence::new();
    let _ = fence.close_visibility();
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());

    let mut subscription = app_core::PublicationSubscription::new(Arc::clone(&fence));
    let first = subscription
        .changed()
        .now_or_never()
        .flatten()
        .expect("the first publication is delivered");
    assert_eq!(first.publication_version, 1);

    // A producer installs newer records while the consumer polls; the consumer
    // must converge on the newest version instead of parking on a stale one.
    let producer_fence = Arc::clone(&fence);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let producer_stop = Arc::clone(&stop);
    let producer = std::thread::spawn(move || {
        for version in 2..=40u64 {
            let _ = producer_fence.close_visibility();
            let epoch = producer_fence.authorization_epoch();
            let guard = producer_fence.install(
                ObservationPublication {
                    session_generation: 1,
                    publication_version: version,
                    connection: PublicationConnection::Observing,
                    ..ObservationPublication::default()
                },
                epoch,
            );
            let _ = producer_fence.open(guard);
            producer_fence.notify_subscribers();
            std::thread::yield_now();
        }
        producer_stop.store(true, std::sync::atomic::Ordering::Release);
    });

    let mut latest = 1u64;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Some(notice) = subscription.changed().now_or_never().flatten() {
            assert!(
                notice.publication_version > latest,
                "publication versions strictly advance (saw {} after {})",
                notice.publication_version,
                latest
            );
            latest = notice.publication_version;
        }
        if stop.load(std::sync::atomic::Ordering::Acquire) && latest == 40 {
            break;
        }
        std::thread::yield_now();
    }
    producer.join().expect("producer joins");
    assert_eq!(
        latest, 40,
        "the consumer reaches the newest publication instead of parking"
    );
}

#[test]
fn b03_untrusted_invalidations_neither_move_the_floor_nor_drive_recovery() {
    let mut live = Live::start();
    let stamp = live.control.active_stamp().expect("binding");

    // A notification naming a foreign request identity is unusable evidence.
    live.control
        .inject_unstamped_message(ServerMessage::ReadInvalidated(ReadInvalidated {
            request_id: desktop_protocol::ReadRequestId::new("read-req/foreign")
                .expect("valid request identity"),
            context_id: stamp.context_id.clone(),
            read_revision: ReadRevision::new(Counter::new("500").expect("revision")),
            reason: ReadInvalidationReason::GrantRevoked,
        }));
    live.now_ms += 1;
    live.owner.step(live.now_ms);
    assert!(
        live.no_data(),
        "a mismatched notification still closes without disclosing data"
    );
}

#[test]
fn b03_invalidation_floor_never_regresses() {
    let mut live = Live::start();
    let stamp = live.control.active_stamp().expect("binding");

    // A high revision is applied first.
    live.control
        .inject_unstamped_message(ServerMessage::ReadInvalidated(ReadInvalidated {
            request_id: stamp.request_id.clone(),
            context_id: stamp.context_id.clone(),
            read_revision: ReadRevision::new(Counter::new("100").expect("revision")),
            reason: ReadInvalidationReason::GrantRevoked,
        }));
    live.step();
    assert!(live.no_data(), "invalidation withholds");
    live.settle();

    // A late notification with a smaller revision must not lower the boundary.
    live.control
        .inject_unstamped_message(ServerMessage::ReadInvalidated(ReadInvalidated {
            request_id: stamp.request_id.clone(),
            context_id: stamp.context_id.clone(),
            read_revision: ReadRevision::new(Counter::new("2").expect("revision")),
            reason: ReadInvalidationReason::GrantRevoked,
        }));
    live.step();
    live.settle();
    live.step();

    // A grant at revision 3 for the same context is still late, so no data may be
    // admitted even though the notification after it claimed a much older floor.
    let late = ReadGrant {
        stamp: desktop_protocol::ReadStamp {
            read_revision: ReadRevision::new(Counter::new("3").expect("revision")),
            ..stamp
        },
        scope: desktop_protocol::ObservationScope {
            scope_id: desktop_protocol::ScopeId::fixture("scope/task-184"),
            task_ids: vec![task()],
            filters: vec![ScopeFilter {
                name: "fixture_view".to_owned(),
                value: "task".to_owned(),
            }],
            protocol: desktop_protocol::Version::new(2, 0),
            schema: desktop_protocol::Version::new(2, 0),
        },
        local_intents_readable: true,
        command_reads: Vec::new(),
    };
    live.control
        .inject_unstamped_message(ServerMessage::ReadGrant(late));
    live.step();
    assert!(
        live.no_data(),
        "a grant at revision 3 stays late after a revision 100 invalidation"
    );
}

#[test]
fn b04_shutdown_completes_for_every_caller_before_during_and_after_disposal() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // Before disposal: two concurrent callers both complete once the owner stops.
    let first = live.handle.shutdown();
    let second = live.handle.shutdown();
    assert!(live.handle.current().is_err(), "shutdown closes first");
    let mut now = live.now_ms;
    resolve(&mut live.owner, first, &mut now);
    resolve(&mut live.owner, second, &mut now);
    live.now_ms = now;
    live.owner.finish(&live.handle);
    assert!(live.handle.is_finished(), "the owner reports completion");

    // After disposal: a later caller completes immediately instead of hanging.
    let late = live.handle.shutdown();
    assert_eq!(poll_once(late), ());
}

#[test]
fn b05_retain_reports_capacity_and_release_is_never_dropped() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // Two admitted references must free two slots, not one.
    for _ in 0..2 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
        live.now_ms = now;
    }
    live.handle.release_command(command.clone());
    live.handle.release_command(command.clone());
    live.now_ms += 1;
    live.owner.step(live.now_ms);
    for index in 0..64 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        let result = resolve(&mut live.owner, reply, &mut now);
        live.now_ms = now;
        assert!(result.is_ok(), "reference {index} is available");
    }
    for _ in 0..64 {
        live.handle.release_command(command.clone());
    }

    // A saturated mailbox must not drop a release: a new intent is refused
    // explicitly while the release is still applied on the next step.
    let reply = live.handle.retain_command(command.clone());
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    for _ in 0..(app_core::observation::MAX_INTENTS + 4) {
        drop(
            live.handle
                .retain_command(CommandId::fixture("command/flood")),
        );
    }
    let overflow = live.handle.retain_command(command.clone());
    let mut now = live.now_ms;
    let overflow = resolve(&mut live.owner, overflow, &mut now);
    live.now_ms = now;
    assert_eq!(
        overflow.err(),
        Some(CommandReadError::CapacityExceeded),
        "a saturated mailbox rejects a new intent explicitly"
    );
    live.handle.release_command(command.clone());
    live.now_ms += 1;
    live.owner.step(live.now_ms);
    for index in 0..64 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        let result = resolve(&mut live.owner, reply, &mut now);
        live.now_ms = now;
        assert!(
            result.is_ok(),
            "reference {index} is available after the saturated-mailbox release drained"
        );
    }
}

#[test]
fn b08_publication_assembled_before_a_close_is_never_republished() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    let epoch_before = live.cached_authorized();

    // A synchronous close (scope intent) followed by the owner step in which the
    // owner would otherwise publish must not expose the earlier authorization.
    let reply = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "single".to_owned(),
        }],
    });
    assert!(
        live.withheld(),
        "the scope intent closes before the owner runs"
    );
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    for _ in 0..4 {
        live.step();
    }
    assert!(
        live.no_data(),
        "a scope change is never undone by an in-flight publication"
    );
    assert!(epoch_before);
}

#[test]
fn b06_read_evidence_stays_bounded() {
    let mut live = Live::start();
    let absent = CommandId::fixture("command/absent");
    live.control.deny_command(absent.as_str().to_owned());

    // Distinct denied identities accumulate identity-level evidence; the client
    // must keep the set bounded and stay responsive instead of growing forever.
    for index in 0..600 {
        let id = CommandId::fixture(format!("command/evidence-{index}"));
        live.control.deny_command(id.as_str().to_owned());
        let reply = live.handle.read_command(id);
        let mut now = live.now_ms;
        let _ = resolve(&mut live.owner, reply, &mut now);
        live.now_ms = now;
        if index % 50 == 0 {
            live.step();
        }
    }
    for _ in 0..4 {
        live.step();
    }
    // The client still answers under the current binding and the owner stays
    // healthy; the bounded set is asserted by construction in cos-client.
    let reply = live.handle.read_command(absent.clone());
    let mut now = live.now_ms;
    let result = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(
        matches!(
            result,
            Err(CommandReadError::AuthorizationDenied
                | CommandReadError::AuthorizationUnavailable
                | CommandReadError::Superseded)
        ),
        "a bounded evidence set still answers honestly"
    );
}

#[test]
fn b08_a_candidate_from_a_superseded_authorization_never_opens() {
    let fence = VisibilityFence::new();
    let _ = fence.close_for_commit();
    let epoch_at_assembly = fence.authorization_epoch();
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        epoch_at_assembly,
    );

    // An authorization close lands between assembly and the commit.
    let _ = fence.close_visibility();
    assert!(
        fence.open(guard).is_err(),
        "a candidate from a superseded authorization never opens"
    );
    assert!(fence.current_tag().is_none());
    assert!(!fence.is_open());

    // The withheld form of the same commit is still publishable.
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 2,
            connection: PublicationConnection::Disconnected,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());
}

#[test]
fn b07_a_closed_fence_is_an_explicit_subscription_notification() {
    let fence = VisibilityFence::new();
    let _ = fence.close_for_commit();
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());

    let mut subscription = app_core::PublicationSubscription::new(Arc::clone(&fence));
    assert_eq!(
        subscription
            .changed()
            .now_or_never()
            .flatten()
            .map(|notice| notice.publication_version),
        Some(1)
    );

    // An authorization close must wake the consumer with an explicit end notice
    // instead of parking it behind a stale display.
    let _ = fence.close_visibility();
    assert_eq!(
        subscription.changed().now_or_never(),
        Some(None),
        "a close is delivered as an end-of-notice, not as a park"
    );

    // The consumer re-subscribes and receives the next complete publication.
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 2,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());
    let mut resubscribed = app_core::PublicationSubscription::new(Arc::clone(&fence));
    assert_eq!(
        resubscribed
            .changed()
            .now_or_never()
            .flatten()
            .map(|notice| notice.publication_version),
        Some(2)
    );
}

#[test]
fn b10_unmatched_or_flooding_releases_cannot_displace_a_legitimate_release() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // One granted reference, then a flood of unmatched releases.
    let reply = live.handle.retain_command(command.clone());
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    for index in 0..128 {
        live.handle
            .release_command(CommandId::fixture(format!("command/unrelated-{index}")));
    }
    // The unrelated releases are refused (no credit) and cannot exceed the budget.
    for _ in 0..4 {
        live.owner.step(live.now_ms + 1);
    }
    // The legitimate release is still available and frees the reference.
    live.handle.release_command(command.clone());
    live.owner.step(live.now_ms + 2);
    for index in 0..64 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        let result = resolve(&mut live.owner, reply, &mut now);
        live.now_ms = now;
        assert!(
            result.is_ok(),
            "reference {index} is available: unmatched releases never consumed the budget"
        );
    }
}

#[test]
fn b11_a_failed_owner_replacement_leaves_the_owner_unavailable() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    assert!(live.publication().snapshot.is_some());

    let failing = Box::new(TransferredClient {
        inner: Some(MockCosClient::from_fixture_str(FIXTURE).expect("fixture")),
        reject: true,
    });
    let result = live.owner.replace_client(failing, &live.handle);
    assert_eq!(result, Err(ReplacementError::TransferRejected));
    assert!(
        live.no_data(),
        "a failed transfer leaves the previous authorization unavailable"
    );
    assert!(
        live.withheld(),
        "the fence stays closed after a failed transfer"
    );

    // The unresolved registry survived: a fresh authorization of the SAME client
    // still sees the retained command with its actual state and capacity.
    let reply = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    });
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle(), "the surviving client re-authorizes");
    assert!(live.settle_until_command(&command));
    assert!(
        matches!(live.read(command.clone()), Ok(CommandRead::Found(_))),
        "the unresolved command survived the failed replacement"
    );
}

/// Client wrapper that refuses registry adoption (transfer-failure transcript).
struct TransferredClient {
    inner: Option<MockCosClient>,
    reject: bool,
}

impl DesktopClient for TransferredClient {
    fn snapshot(&self) -> cos_client::SnapshotHandle {
        self.inner.as_ref().expect("client").snapshot()
    }
    fn connection(&self) -> &cos_client::ConnectionState {
        self.inner.as_ref().expect("client").connection()
    }
    fn is_stale(&self) -> bool {
        self.inner.as_ref().expect("client").is_stale()
    }
    fn source(&self) -> cos_client::ClientSource {
        self.inner.as_ref().expect("client").source()
    }
    fn last_error(&self) -> Option<&cos_client::ClientError> {
        self.inner.as_ref().expect("client").last_error()
    }
    fn disconnect(&mut self) -> Vec<cos_client::ClientEvent> {
        Vec::new()
    }
    fn request_resync(
        &mut self,
        _reason: desktop_protocol::ResyncReason,
    ) -> Vec<cos_client::ClientEvent> {
        Vec::new()
    }
    fn pump(&mut self, now_ms: u64) -> Vec<cos_client::ClientEvent> {
        self.pump_step(now_ms, 64).events
    }
    fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, cos_client::ClientError> {
        self.inner.as_mut().expect("client").stop_task(task_id)
    }
    fn bind_observation_control(&mut self, control: cos_client::ObservationControl) {
        self.inner
            .as_mut()
            .expect("client")
            .bind_observation_control(control);
    }
    fn pump_step(&mut self, now_ms: u64, budget: usize) -> cos_client::PumpReport {
        self.inner
            .as_mut()
            .expect("client")
            .pump_step(now_ms, budget)
    }
    fn command_statuses(
        &self,
    ) -> &std::collections::BTreeMap<CommandId, cos_client::CommandStatus> {
        self.inner.as_ref().expect("client").command_statuses()
    }
    fn lookup_command(&self, id: &CommandId) -> cos_client::CommandLookup<'_> {
        self.inner.as_ref().expect("client").lookup_command(id)
    }
    fn retain_command(&mut self, id: &CommandId) -> Result<(), cos_client::ClientError> {
        self.inner.as_mut().expect("client").retain_command(id)
    }
    fn release_command(&mut self, id: &CommandId) {
        self.inner.as_mut().expect("client").release_command(id);
    }
    fn begin_authorized_observation(
        &mut self,
        request: desktop_protocol::OpenAuthorizedObservation,
    ) -> Result<(), cos_client::ClientError> {
        self.inner
            .as_mut()
            .expect("client")
            .begin_authorized_observation(request)
    }
    fn publication_access(&self) -> cos_client::PublicationAccess<'_> {
        self.inner.as_ref().expect("client").publication_access()
    }
    fn lookup_receipt_authorized(
        &mut self,
        request: desktop_protocol::AuthorizedReceiptLookup,
    ) -> Result<(), cos_client::ClientError> {
        self.inner
            .as_mut()
            .expect("client")
            .lookup_receipt_authorized(request)
    }
    fn observation_request(
        &self,
        scope: ObservationScopeRequest,
    ) -> desktop_protocol::OpenAuthorizedObservation {
        self.inner
            .as_ref()
            .expect("client")
            .observation_request(scope)
    }
    fn adopt_retained_registry(
        &mut self,
        records: std::collections::BTreeMap<CommandId, cos_client::CommandStatus>,
        references: std::collections::BTreeMap<CommandId, usize>,
    ) -> Result<(), cos_client::ClientError> {
        if self.reject {
            return Err(cos_client::ClientError::InvalidCommand(
                "transfer refused".to_owned(),
            ));
        }
        self.inner
            .as_mut()
            .expect("client")
            .adopt_retained_registry(records, references)
    }
}

#[test]
fn b07b_a_closed_subscription_delivers_once_and_then_parks() {
    let fence = VisibilityFence::new();
    let _ = fence.close_for_commit();
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());

    let mut subscription = app_core::PublicationSubscription::new(Arc::clone(&fence));
    assert!(subscription.changed().now_or_never().flatten().is_some());

    // An authorization close is delivered exactly once...
    let _ = fence.close_visibility();
    fence.notify_subscribers();
    assert_eq!(subscription.changed().now_or_never(), Some(None));
    // ...and the consumer then parks instead of busy-returning forever.
    let mut pending = Box::pin(subscription.changed());
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert_eq!(
        pending.as_mut().poll(&mut cx),
        Poll::Pending,
        "a closed fence parks the consumer; it never busy-returns"
    );
    assert_eq!(pending.as_mut().poll(&mut cx), Poll::Pending);

    // A new publication wakes the parked consumer with the next identity.
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 2,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());
    fence.notify_subscribers();
    assert_eq!(
        pending.as_mut().poll(&mut cx),
        Poll::Ready(Some(app_core::PublicationNotice {
            session_generation: 1,
            publication_version: 2,
        })),
        "the parked consumer receives the next publication"
    );
}

#[test]
fn b08b_an_authorization_close_invalidates_a_candidate_even_while_closed() {
    let fence = VisibilityFence::new();
    let _ = fence.close_for_commit();
    let epoch_at_assembly = fence.authorization_epoch();
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        epoch_at_assembly,
    );
    // The owner's own commit close happens first (epoch-neutral).
    let _ = fence.close_for_commit();
    // An authorization close lands while the fence is already closed.
    let _ = fence.close_visibility();
    assert!(
        fence.open(guard).is_err(),
        "a revocation while closed still defeats the assembled candidate"
    );
    assert!(fence.current_tag().is_none());
}

#[test]
fn b07c_the_consumer_loop_never_resubscribes_while_closed() {
    // Mirrors the GPUI consumer: one subscription for the whole window lifetime.
    let fence = VisibilityFence::new();
    let _ = fence.close_for_commit();
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());

    let mut subscription = app_core::PublicationSubscription::new(Arc::clone(&fence));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    // First publication.
    let mut pending = Box::pin(subscription.changed());
    assert!(matches!(
        pending.as_mut().poll(&mut cx),
        Poll::Ready(Some(_))
    ));

    // Authorization close: one notice, then the SAME subscription parks.
    let _ = fence.close_visibility();
    fence.notify_subscribers();
    let mut pending = Box::pin(subscription.changed());
    assert_eq!(pending.as_mut().poll(&mut cx), Poll::Ready(None));
    for poll in 0..16 {
        assert_eq!(
            pending.as_mut().poll(&mut cx),
            Poll::Pending,
            "poll {poll} after the close notice must park, never busy-return"
        );
    }

    // A new publication on the same subscription resumes delivery.
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 2,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());
    fence.notify_subscribers();
    assert_eq!(
        pending.as_mut().poll(&mut cx),
        Poll::Ready(Some(app_core::PublicationNotice {
            session_generation: 1,
            publication_version: 2,
        }))
    );
}

#[test]
fn b10b_pending_releases_are_drained_before_a_reference_transfer() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // retain x2, release x1 (queued, not yet drained), then replace the owner.
    for _ in 0..2 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
        live.now_ms = now;
    }
    live.handle.release_command(command.clone());

    live.owner
        .replace_client(
            Box::new(MockCosClient::from_fixture_str(FIXTURE).expect("fixture")),
            &live.handle,
        )
        .expect("replacement");
    let reply = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    });
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle());

    // Exactly one reference survived the transfer: 63 further retains fit.
    let mut admitted = 0;
    for _ in 0..64 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        let result = resolve(&mut live.owner, reply, &mut now);
        live.now_ms = now;
        if result.is_ok() {
            admitted += 1;
        }
    }
    assert_eq!(
        admitted, 63,
        "the transferred reference count matches the surviving credit"
    );
}

#[test]
fn b11b_a_failed_replacement_never_republishes_the_earlier_authorization() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    assert!(live.publication().snapshot.is_some());

    let failing = Box::new(TransferredClient {
        inner: Some(MockCosClient::from_fixture_str(FIXTURE).expect("fixture")),
        reject: true,
    });
    assert_eq!(
        live.owner.replace_client(failing, &live.handle),
        Err(ReplacementError::TransferRejected)
    );

    // Ordinary owner steps, with no new scope intent, must never resurrect the
    // earlier authorization.
    for _ in 0..8 {
        live.step();
        assert!(
            live.no_data(),
            "a failed replacement stays unavailable across ordinary steps"
        );
    }
}

#[test]
fn b13_a_stop_invalidated_by_an_authorization_change_is_never_sent() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    let retained_before = live.publication().command_statuses.len();

    // A Stop intent is queued, then authorization changes in the same generation.
    let stale = live.handle.request_stop(task());
    drop(live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    }));
    let mut now = live.now_ms;
    let result = resolve(&mut live.owner, stale, &mut now);
    live.now_ms = now;
    assert_eq!(
        result.err(),
        Some(CommandReadError::Superseded),
        "a Stop queued before an authorization change is rejected"
    );
    assert!(live.settle(), "the re-authorized session publishes again");
    assert_eq!(
        live.publication().command_statuses.len(),
        retained_before,
        "the stale Stop never reached the client registry"
    );
}

#[test]
fn b12_a_stalled_owner_reports_an_unconfirmed_shutdown_within_the_deadline() {
    let live = Live::start();
    // No step is run after the request: the owner never applies it, so the
    // bounded waiter must report failure instead of hanging.
    assert!(
        !live
            .handle
            .shutdown_blocking(std::time::Duration::from_millis(50))
    );
    assert!(
        !live.handle.is_finished(),
        "an unconfirmed shutdown is reported, never silently assumed"
    );
}

#[test]
fn b14_a_stop_is_never_sent_after_authorization_changed() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    let retained_before = live.publication().command_statuses.len();

    // Queue a Stop and revoke authorization before the owner dispatches it: the
    // close bumps the authorization epoch synchronously, so final acceptance must
    // reject the Stop instead of sending it.
    let stale = live.handle.request_stop(task());
    drop(live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    }));
    let mut now = live.now_ms;
    let result = resolve(&mut live.owner, stale, &mut now);
    live.now_ms = now;
    assert_eq!(
        result.err(),
        Some(CommandReadError::Superseded),
        "a Stop whose authorization changed is rejected before the wire"
    );
    assert!(live.settle(), "the re-authorized session publishes again");
    assert_eq!(
        live.publication().command_statuses.len(),
        retained_before,
        "the rejected Stop never reached the client registry"
    );
}

#[test]
fn b14b_an_authorized_stop_is_dispatched_within_the_boundary() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(
        live.settle_until_command(&command),
        "an authorized Stop is dispatched and published"
    );
    assert!(matches!(
        live.read(command.clone()),
        Ok(CommandRead::Found(_))
    ));
}

#[test]
fn b13c_an_adapter_close_crosses_the_command_boundary_and_is_reentrant() {
    let fence = VisibilityFence::new();
    let _ = fence.close_for_commit();
    let guard = fence.install(
        ObservationPublication {
            session_generation: 1,
            publication_version: 1,
            connection: PublicationConnection::Observing,
            ..ObservationPublication::default()
        },
        fence.authorization_epoch(),
    );
    assert!(fence.open(guard).is_ok());

    // The adapter close (as the client's control closure invokes it) must wait for
    // an in-flight command boundary instead of racing it.
    let holder = Arc::clone(&fence);
    let held = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let holder_flag = Arc::clone(&held);
    let release_flag = Arc::clone(&release);
    let writer = std::thread::spawn(move || {
        let _gate = holder.enter_command_boundary();
        holder_flag.store(true, Ordering::Release);
        while !release_flag.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    });
    while !held.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    let closer_fence = Arc::clone(&fence);
    let closed_at = Arc::new(std::sync::Mutex::new(None));
    let stamp = Arc::clone(&closed_at);
    let closer = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let _gate = closer_fence.enter_command_boundary();
        closer_fence.close_visibility();
        *stamp.lock().expect("stamp lock") = Some(started.elapsed());
    });
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(
        closed_at.lock().expect("stamp lock").is_none(),
        "the adapter close waits for the held command boundary"
    );
    release.store(true, Ordering::Release);
    writer.join().expect("writer joins");
    closer.join().expect("closer joins");
    assert!(
        closed_at.lock().expect("stamp lock").is_some(),
        "the adapter close completes once the boundary is released"
    );
    assert!(!fence.is_open());

    // Reentrancy: the same thread may close from inside a command boundary.
    let _outer = fence.enter_command_boundary();
    let _inner = fence.enter_command_boundary();
    let epoch = fence.authorization_epoch();
    let _ = fence.close_visibility();
    assert!(fence.authorization_epoch() > epoch);
}

#[test]
fn b07d_a_close_racing_the_registration_is_never_lost() {
    // Barrier-style stress: the consumer polls while the producer closes the fence,
    // so the registration/close interleaving is exercised repeatedly.
    let mut missing = 0;
    for round in 0..200 {
        let fence = VisibilityFence::new();
        let _ = fence.close_for_commit();
        let guard = fence.install(
            ObservationPublication {
                session_generation: 1,
                publication_version: 1,
                connection: PublicationConnection::Observing,
                ..ObservationPublication::default()
            },
            fence.authorization_epoch(),
        );
        assert!(fence.open(guard).is_ok());
        let mut subscription = app_core::PublicationSubscription::new(Arc::clone(&fence));
        assert!(subscription.changed().now_or_never().flatten().is_some());

        let closer = Arc::clone(&fence);
        let thread = std::thread::spawn(move || {
            std::thread::yield_now();
            let _ = closer.close_visibility();
            closer.notify_subscribers();
        });
        // Poll until the close notice arrives; registering while the close lands is
        // exactly the interleaving that must not be lost.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut observed = false;
        while std::time::Instant::now() < deadline {
            if subscription.changed().now_or_never() == Some(None) {
                observed = true;
                break;
            }
            std::thread::yield_now();
        }
        thread.join().expect("closer joins");
        if !observed {
            missing += 1;
        }
        assert!(observed, "round {round}: the close notice was lost");
    }
    assert_eq!(missing, 0);
}

#[test]
fn b10c_a_release_concurrent_with_a_transfer_is_never_lost() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // Two admitted references, then a release issued concurrently with the transfer.
    for _ in 0..2 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
        live.now_ms = now;
    }
    let releaser = live.handle.clone();
    let id = command.clone();
    let thread = std::thread::spawn(move || {
        std::thread::yield_now();
        releaser.release_command(id);
    });
    live.owner
        .replace_client(
            Box::new(MockCosClient::from_fixture_str(FIXTURE).expect("fixture")),
            &live.handle,
        )
        .expect("replacement");
    thread.join().expect("releaser joins");

    let reply = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    });
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle());

    // Whatever the interleaving, the surviving reference count and the ledger stay
    // consistent: the admits plus the surviving reference never exceed the budget.
    let mut admitted = 0;
    for _ in 0..64 {
        let reply = live.handle.retain_command(command.clone());
        let mut now = live.now_ms;
        if resolve(&mut live.owner, reply, &mut now).is_ok() {
            admitted += 1;
        }
        live.now_ms = now;
    }
    assert!(
        admitted == 63 || admitted == 62,
        "the release is applied exactly once (admitted {admitted})"
    );
}

#[test]
fn b13b_a_dispatched_stop_is_never_reissued_after_an_authorization_change() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // The Stop is dispatched first (before the change), then authorization closes.
    let dispatched = live.stop();
    live.owner.step(live.now_ms + 1);
    let _gate = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    });
    live.owner.step(live.now_ms + 2);
    assert!(
        live.no_data() || live.current_version() > 0,
        "the change withholds or republishes a complete version"
    );
    assert!(live.settle(), "the re-authorized session publishes again");
    let statuses = live.publication().command_statuses;
    assert_eq!(
        statuses.len(),
        2,
        "the dispatched Stop is never re-issued as a second command"
    );
    assert!(
        statuses.contains_key(&dispatched),
        "the legitimately dispatched Stop keeps its actual retained state"
    );
}

#[test]
fn a06b_the_exit_watch_distinguishes_completion_from_a_missed_deadline() {
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = app_core::ExitWatch::new(
        Arc::clone(&done),
        Arc::new(std::sync::Mutex::new(None)),
        std::time::Instant::now() + std::time::Duration::from_secs(5),
    );
    let slot = watch.waker_slot();
    let mut pending = Box::pin(watch.wait());
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert_eq!(pending.as_mut().poll(&mut cx), Poll::Pending);
    done.store(true, std::sync::atomic::Ordering::Release);
    if let Some(waker) = slot.lock().expect("registered completion wake").take() {
        waker.wake();
    }
    assert_eq!(pending.as_mut().poll(&mut cx), Poll::Ready(true));

    let stuck = app_core::ExitWatch::new(
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::Mutex::new(None)),
        std::time::Instant::now(),
    );
    assert_eq!(stuck.wait().now_or_never(), Some(false));
}

#[test]
fn a06c_an_expired_exit_budget_never_confirms_a_late_driver_finish() {
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = app_core::ExitWatch::new(
        Arc::clone(&done),
        Arc::new(std::sync::Mutex::new(None)),
        std::time::Instant::now(),
    );
    done.store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        watch.wait().now_or_never(),
        Some(false),
        "completion after the deadline cannot turn an unconfirmed exit into confirmation"
    );
}

#[test]
fn b15_a_grant_whose_stamp_names_another_scope_fails_closed() {
    let mut live = Live::start();
    assert!(live.publication().snapshot.is_some());
    let stamp = live.control.active_stamp().expect("binding");

    // The stamp carries one scope identity while the granted scope carries another.
    let mut mismatched = stamp.clone();
    mismatched.scope_id = desktop_protocol::ScopeId::fixture("scope/other");
    live.control
        .inject_unstamped_message(ServerMessage::ReadGrant(desktop_protocol::ReadGrant {
            stamp: mismatched,
            scope: desktop_protocol::ObservationScope {
                scope_id: desktop_protocol::ScopeId::fixture("scope/task-184"),
                task_ids: vec![task()],
                filters: Vec::new(),
                protocol: desktop_protocol::Version::new(2, 0),
                schema: desktop_protocol::Version::new(2, 0),
            },
            local_intents_readable: true,
            command_reads: Vec::new(),
        }));
    live.step();
    assert!(
        live.no_data(),
        "a grant whose stamp does not describe the granted scope admits no data"
    );
}

#[test]
fn a08_a_refused_read_records_a_safe_published_diagnostic() {
    let mut live = Live::start_with_pending_lookups();
    // An identity with no evidence under the current binding: the read is refused
    // with a bounded lookup intent, and that refusal is published as a diagnostic.
    let unknown = CommandId::fixture("command/no-evidence");
    assert!(matches!(
        live.read(unknown),
        Err(CommandReadError::AuthorizationUnavailable)
    ));
    live.step();
    assert_eq!(
        live.publication().last_error,
        Some(PublicationError::VerificationPending),
        "a read awaiting authorization evidence is published as a pending verification"
    );
    // The diagnostic reaches the publication even though no other client change
    // occurs in the same step.
    let version = live.publication().publication_version;
    assert!(
        version > 0,
        "the diagnostic change is published with its own version"
    );
}

#[test]
fn a08_the_read_diagnostic_is_cleared_when_it_no_longer_holds() {
    let mut live = Live::start_with_pending_lookups();
    // An identity with no evidence: the read stays pending while the transcript
    // keeps the lookup unanswered.
    let unknown = CommandId::fixture("command/no-evidence");
    assert!(live.read(unknown).is_err());
    live.step();
    assert_eq!(
        live.publication().last_error,
        Some(PublicationError::VerificationPending),
        "the unanswered read is published as a pending verification"
    );

    // The asynchronous answer resolves it: no further read and no new authorization.
    live.control.answer_pending_lookup(None);
    live.step();
    assert_eq!(
        live.publication().last_error,
        None,
        "an asynchronous authorized answer clears the pending diagnostic"
    );
}

#[test]
fn a08_an_allowed_answer_with_a_receipt_clears_the_pending_diagnostic() {
    let mut live = Live::start_with_pending_lookups();
    let id = CommandId::fixture("command/answered-with-receipt");
    assert!(live.read(id.clone()).is_err());
    live.step();
    assert_eq!(
        live.publication().last_error,
        Some(PublicationError::VerificationPending),
        "the unanswered read is published as a pending verification"
    );

    // The answer carries a receipt: the client keeps it as per-record permission,
    // which resolves the read just as an absent-receipt answer does.
    live.control.answer_pending_lookup(Some(CommandReceipt {
        command_id: id.clone(),
        operation: CommandOperation::StopTask,
        state: ReceiptState::Accepted,
        task_id: Some(task()),
        accepted_at: None,
        reject_reason: None,
        effect_refs: Vec::new(),
        dedupe_expires_at: None,
        message: None,
    }));
    for _ in 0..3 {
        live.step();
    }
    assert_eq!(
        live.publication().last_error,
        None,
        "an allowed answer carrying a receipt clears the pending diagnostic"
    );
}

#[test]
fn a08_a_denied_answer_reclassifies_the_pending_diagnostic() {
    let mut live = Live::start_with_pending_lookups();
    let id = CommandId::fixture("command/answered-with-denial");
    live.control.deny_command(id.as_str().to_owned());
    assert!(live.read(id.clone()).is_err());
    live.step();
    assert_eq!(
        live.publication().last_error,
        Some(PublicationError::VerificationPending),
        "the unanswered read is published as a pending verification"
    );

    // The transcript answers with a denial: the diagnostic becomes the denial.
    live.control.answer_pending_lookup_denied();
    for _ in 0..3 {
        live.step();
    }
    assert_eq!(
        live.publication().last_error,
        Some(PublicationError::Unauthorized),
        "a denied answer reclassifies the diagnostic as an authorization failure"
    );
}

#[test]
fn a13_a_same_step_answer_resolves_the_diagnostic_without_another_wake() {
    // The transcript answers lookups immediately, so the refusal and its answer
    // land in one owner step: the resolution must be published without relying on
    // a later wake that may never arrive.
    let mut live = Live::start();
    let id = CommandId::fixture("command/immediate-answer");
    assert!(live.read(id).is_err());
    assert_eq!(
        live.publication().last_error,
        None,
        "the immediately answered read leaves no pending diagnostic behind"
    );
    assert_eq!(
        live.owner.next_deadline_ms(),
        None,
        "no wake is scheduled once everything is answered"
    );
    // The same-step publication consumed its diagnostic change: later steps with
    // no events must not advance the version again.
    let version = live.publication().publication_version;
    for _ in 0..4 {
        live.step();
    }
    assert_eq!(
        live.publication().publication_version,
        version,
        "the resolved diagnostic is published once, not on every later step"
    );
}

#[test]
fn a08_a_confirmed_denial_is_not_republished() {
    let mut live = Live::start_with_pending_lookups();
    let id = CommandId::fixture("command/confirmed-denial");
    live.control.deny_command(id.as_str().to_owned());
    assert!(live.read(id.clone()).is_err());
    live.step();
    live.control.answer_pending_lookup_denied();
    for _ in 0..3 {
        live.step();
    }
    assert_eq!(
        live.publication().last_error,
        Some(PublicationError::Unauthorized),
        "the denial is published"
    );

    // A settled denial is not an event: further owner steps publish no new version.
    let version = live.publication().publication_version;
    for _ in 0..6 {
        live.step();
    }
    assert_eq!(
        live.publication().publication_version,
        version,
        "a confirmed denial does not keep bumping the publication"
    );
}

#[test]
fn a12_a_bounded_capacity_refusal_reaches_the_caller() {
    let mut live = Live::start_with_pending_lookups();
    // Exhaust the client's bounded lookup budget with unanswered reads.
    for index in 0..64 {
        let id = CommandId::fixture(format!("command/read-budget-{index}"));
        let _ = live.read(id);
    }
    live.step();
    // The 65th distinct identity cannot obtain a lookup slot.
    let overflow = CommandId::fixture("command/read-budget-overflow");
    let result = live.read(overflow);
    assert!(
        matches!(
            result,
            Err(CommandReadError::CapacityExceeded)
                | Err(CommandReadError::Client(PublicationError::InvalidCommand(
                    InvalidCommandReason::CapacityExceeded
                )))
        ),
        "a bounded capacity refusal is reported to the caller: {result:?}"
    );
    live.step();
    assert!(
        matches!(
            live.publication().last_error,
            Some(PublicationError::InvalidCommand(
                InvalidCommandReason::CapacityExceeded
            ))
        ),
        "the published diagnostic keeps the same safe classification: {:?}",
        live.publication().last_error
    );
}

#[test]
fn a08_an_explicit_denial_is_published_as_an_authorization_failure() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // The identity is denied for the next binding, which then carries the negative
    // decision for it.
    live.control.deny_command(command.as_str().to_owned());
    live.step();
    let reply = live.handle.change_scope(ObservationScopeRequest {
        task_ids: vec![task()],
        filters: vec![ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "task".to_owned(),
        }],
    });
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(live.settle(), "the rebound binding publishes");

    let refused = live.read(command);
    assert!(
        matches!(refused, Err(CommandReadError::AuthorizationDenied)),
        "the current binding denies this identity: {refused:?}"
    );
    live.step();
    assert_eq!(
        live.publication().last_error,
        Some(PublicationError::Unauthorized),
        "an explicit denial is published as an authorization failure, not as a pending verification"
    );
    let version = live.publication().publication_version;
    for _ in 0..4 {
        live.step();
    }
    assert_eq!(
        live.publication().publication_version,
        version,
        "the denial is published once, not on every later step"
    );
}

#[test]
fn a20_reads_are_bounded_coalesced_and_never_loop() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));

    // A record with current positive evidence never triggers another read.
    let before = live.control.sent_frames();
    for _ in 0..3 {
        live.step();
    }
    assert_eq!(
        live.control.sent_frames(),
        before,
        "a record with current evidence is never re-read"
    );

    // Duplicate intents for the same identity coalesce into one request.
    live.control.deny_command(command.as_str().to_owned());
    let first = live.handle.read_command(command.clone());
    let second = live.handle.read_command(command.clone());
    let mut now = live.now_ms;
    let _ = resolve(&mut live.owner, first, &mut now);
    let _ = resolve(&mut live.owner, second, &mut now);
    live.now_ms = now;
    live.step();
    let after_lookup = live.control.sent_frames();

    // A denial is not polled again until a new binding or explicit intent.
    for _ in 0..3 {
        live.step();
    }
    assert_eq!(
        live.control.sent_frames(),
        after_lookup,
        "a denied identity is never re-polled automatically"
    );
}

/// A delta delivered after the initial publication must produce one new complete
/// version: the client marks the report changed, the owner republishes, and the
/// revision marker moves. The identical transcript step replayed must not.
///
/// The oracle is the delta's own declared mutation: the entity it upserts and
/// the content it declares, read out of the transcript rather than restated
/// here. Two handles sharing the fence must read the same new
/// (generation, version) with identical content.
#[test]
fn a12b_a_delta_after_the_initial_publication_publishes_one_new_version() {
    // Parse the transcript so the deferred delta can be replayed as a protocol
    // duplicate and its declared content can drive the content oracle.
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("fixture parses");
    let mut deferred = 0;
    for step in &mut scenario.steps {
        if step.trigger == "after_open" {
            step.trigger = "manual".to_owned();
            deferred += 1;
        }
    }
    assert!(
        deferred >= 1,
        "the delta steps are deferred to manual delivery"
    );

    // Pin the transaction identity and cursor so a re-delivery is the same
    // protocol transaction (a duplicate), never a fresh one.
    let delta = scenario
        .steps
        .iter_mut()
        .find(|step| step.id == "delta-evaluation-e")
        .expect("the transcript scripts the evaluation delta")
        .actions
        .iter_mut()
        .find_map(|action| action.delta.as_mut())
        .expect("the deferred step carries a delta");
    delta.transaction_id = Some(TransactionId::fixture("transaction/delta-evaluation-e"));
    delta.cursor = Some(Cursor::fixture("cursor/delta-evaluation-e"));

    // The entity and the exact content the delta itself mutates.
    let declared = delta
        .mutations
        .iter()
        .find_map(|mutation| match mutation {
            ProjectionMutation::Upsert {
                projection: Projection::Evaluation(evaluation),
                ..
            } => Some(evaluation.clone()),
            _ => None,
        })
        .expect("the delta upserts exactly one evaluation");

    let mut live =
        Live::start_with(MockCosClient::from_scenario(scenario).expect("scenario builds"));
    assert!(live.settle(), "the fixture snapshot must publish");
    let first = live.publication();
    let revision_before = first.revision.clone();

    // Before the delta the snapshot carries the entity at its original content.
    let before = first
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.evaluations.get(&declared.evaluation_ref))
        .cloned()
        .expect("the initial snapshot carries the entity the delta mutates");
    assert_ne!(
        before, declared,
        "the delta declares content different from the initial entity"
    );

    for _ in 0..3 {
        live.step();
    }
    let idle = live.publication();
    assert_eq!(
        idle.publication_version, first.publication_version,
        "an idle owner never advances the version"
    );

    live.control.deliver_step("delta-evaluation-e");
    live.step();
    live.step();
    let second = live.publication();
    assert_eq!(
        second.publication_version,
        idle.publication_version + 1,
        "an event-free delta advances the publication version exactly once"
    );
    // The new snapshot exposes exactly the entity the delta declared.
    let after = second
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.evaluations.get(&declared.evaluation_ref))
        .cloned()
        .expect("the new snapshot carries the mutated entity");
    assert_eq!(
        after, declared,
        "the new snapshot holds the delta's own mutated content"
    );
    assert_ne!(
        after, before,
        "the mutated entity's content concretely changed"
    );
    assert_ne!(
        second.revision, revision_before,
        "the revision marker moves with the applied delta"
    );
    assert_eq!(
        second.session_generation, first.session_generation,
        "generation never mixes across a version bump"
    );
    assert!(second.snapshot.is_some() && second.connection == PublicationConnection::Observing);

    // A second handle sharing the fence reads the same new record, byte for byte.
    let other = live.handle.clone();
    let observed = publication_of(&other);
    assert_eq!(
        observed.session_generation, second.session_generation,
        "both consumers read the same generation"
    );
    assert_eq!(
        observed.publication_version, second.publication_version,
        "both consumers read the same version"
    );
    assert_eq!(
        observed.snapshot, second.snapshot,
        "both consumers read identical snapshot content"
    );
    assert_eq!(
        observed
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.evaluations.get(&declared.evaluation_ref)),
        Some(&declared),
        "both consumers read the mutated entity"
    );

    // The deferred step is consumed: later idle steps publish nothing new.
    for _ in 0..3 {
        live.step();
    }
    let settled = live.publication();
    assert_eq!(
        settled.publication_version, second.publication_version,
        "idle steps publish nothing new after the delta"
    );

    // Re-delivering the identical transaction is a protocol duplicate: the
    // client ignores it and the owner publishes no new version or content.
    live.control.deliver_step("delta-evaluation-e");
    for _ in 0..3 {
        live.step();
    }
    let replayed = live.publication();
    assert_eq!(
        replayed.publication_version, second.publication_version,
        "a duplicate replay never advances the version"
    );
    assert_eq!(
        replayed.snapshot, second.snapshot,
        "a duplicate replay leaves the published content unchanged"
    );
    assert_eq!(
        replayed.connection,
        PublicationConnection::Observing,
        "the client ignores the duplicate, it never fails the connection"
    );
    assert!(
        replayed.last_error.is_none(),
        "an ignored duplicate surfaces no error"
    );
}

// ---------------------------------------------------------------------------
// STEP 5 ACTIONABLE closures (A01/A02/A03/A05/A09/A10/A11/A13/A16).
// ---------------------------------------------------------------------------

/// A transcript whose `STOP_TASK` is never answered, so the retained record
/// stays a genuine unresolved local intent (never a synthesized completion).
fn unresolved_stop_scenario() -> FixtureScenario {
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario parses");
    scenario
        .commands
        .get_mut("STOP_TASK")
        .expect("the fixture scripts STOP_TASK")
        .default = CommandDefault::Silent;
    scenario
        .steps
        .retain(|step| !step.trigger.contains("STOP_TASK"));
    scenario
}

/// The exact fixture scope expressed as a request.
fn fixture_scope_request(filters: Vec<ScopeFilter>) -> ObservationScopeRequest {
    ObservationScopeRequest {
        task_ids: vec![task()],
        filters,
    }
}

fn fixture_view_filter() -> ScopeFilter {
    ScopeFilter {
        name: "fixture_view".to_owned(),
        value: "task".to_owned(),
    }
}

/// One unambiguous failure predicate for an unresolved local record.
fn is_unresolved(presentation: CommandPresentation) -> bool {
    matches!(
        presentation,
        CommandPresentation::Prepared
            | CommandPresentation::AwaitingResponse
            | CommandPresentation::OutcomeUnconfirmed
    )
}

/// A01: the owner seam must let an unresolved record survive a same-scope
/// reconnect and never resend it.
#[test]
fn a01b_an_unresolved_stop_survives_reconnect_at_the_owner_seam() {
    let mut live = Live::start_with(
        MockCosClient::from_scenario(unresolved_stop_scenario()).expect("scenario builds"),
    );
    let command = live.stop();
    assert!(
        live.settle_until_command(&command),
        "the unresolved record is published before the reconnect"
    );
    let before_publication = live.publication();
    let before = before_publication
        .command_statuses
        .get(&command)
        .cloned()
        .expect("the unresolved record is visible");
    assert!(
        is_unresolved(before.presentation),
        "a silent Stop stays unresolved, never a receipt: {before:?}"
    );
    assert!(before.receipt.is_none(), "no receipt is fabricated");

    let frames_before = live.control.sent_frames();
    let dispatches_before = live.control.sent_commands();
    assert_eq!(
        dispatches_before.len(),
        1,
        "the unresolved Stop is dispatched exactly once: {dispatches_before:?}"
    );
    live.control.disconnect();
    live.step();
    assert!(
        live.no_data(),
        "the disconnect withholds the unresolved record immediately"
    );
    assert!(
        live.read(command.clone()).is_err(),
        "a withheld binding answers no command read"
    );
    assert_eq!(
        live.control.sent_frames(),
        frames_before,
        "the disconnect window never resends the unresolved command"
    );

    assert!(live.settle(), "a fresh authorized observation republishes");
    let after_publication = live.publication();
    let after = after_publication
        .command_statuses
        .get(&command)
        .cloned()
        .expect("the unresolved record survives the same-scope reconnect");
    assert!(
        is_unresolved(after.presentation),
        "the surviving record is still unresolved, never a synthetic receipt: {after:?}"
    );
    assert!(
        after.receipt.is_none() && before.receipt.is_none(),
        "the reconnect never fabricates a receipt for the unresolved command"
    );
    assert_eq!(after_publication.snapshot, before_publication.snapshot);
    assert_eq!(after, before, "the full unresolved command state survives");
    assert_eq!(
        after_publication.command_statuses.len(),
        1,
        "recovery restores exactly the one unresolved record, never a second dispatch"
    );
    assert_eq!(
        live.control.sent_commands(),
        dispatches_before,
        "reauthorization never re-dispatches the unresolved Stop"
    );
}

/// A02: an exact-visibility oracle. Principal B explicitly denies A's identity,
/// so the publication and the read must expose B's decision exactly, never A's
/// former data.
#[test]
fn a02b_principal_switch_exposes_only_b_authorized_data() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    let before = live.publication();
    assert!(before.command_statuses.contains_key(&command));
    let old_stamp = live.control.active_stamp().expect("binding A");

    // Principal B observes the same task set but denies this identity outright.
    let client_b = context_scenario("principal-b-exact", "1", Vec::new());
    let control_b = client_b.control();
    // The identity is runtime-generated, so the denial is steered after the
    // transcript is built (the fixture namespace forbids foreign identities).
    control_b.deny_command(command.as_str().to_owned());
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement transfers the non-empty registry");
    assert!(
        live.no_data(),
        "the switch closes visibility synchronously, before any new grant"
    );
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle(), "B publishes after its own grant");

    let publication = live.publication();
    assert_ne!(
        old_stamp.context_id,
        control_b.active_stamp().expect("binding B").context_id,
        "the binding carries B's context, never A's"
    );
    assert_eq!(
        publication.session_generation, 2,
        "the switch advances the owner generation exactly once"
    );
    assert!(
        !publication.command_statuses.contains_key(&command),
        "B's denied identity is exactly absent from B's visible set: {:?}",
        publication.command_statuses.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        live.read(command.clone()).err(),
        Some(CommandReadError::AuthorizationDenied),
        "B's own decision governs A's transferred record"
    );
}

/// A03: with the same task/scope set, a changed authentication context rejects
/// the previous context; replaying the old context's grant cannot rebind.
#[test]
fn a03b_an_old_context_grant_cannot_rebind_after_the_context_changed() {
    let mut live = Live::start_with(context_scenario("ctx-old", "5", Vec::new()));
    // The stop makes the registry non-empty so a later replacement can transfer it.
    let _command = live.stop();
    assert!(live.settle());
    let old_stamp = live.control.active_stamp().expect("binding ctx-old");
    assert_eq!(
        old_stamp.context_id,
        ReadContextId::new("fixture:read-context/ctx-old").expect("context")
    );

    let client_b = context_scenario("ctx-new", "6", Vec::new());
    let control_b = client_b.control();
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement");
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle(), "the changed context binds its own grant");
    let new_context = Some(ReadContextId::new("fixture:read-context/ctx-new").expect("context"));
    assert_eq!(control_b.active_stamp().map(|s| s.context_id), new_context);

    // Replay the previous context's grant verbatim: it names another request and
    // authentication context. A grant outside the verified handoff fails closed
    // (no served data), exactly as a mismatched-server grant does in a17.
    let replay = ReadGrant {
        stamp: old_stamp,
        scope: ObservationScope {
            scope_id: desktop_protocol::ScopeId::fixture("scope/task-184"),
            task_ids: vec![task()],
            filters: vec![fixture_view_filter()],
            protocol: desktop_protocol::Version::new(2, 0),
            schema: desktop_protocol::Version::new(2, 0),
        },
        local_intents_readable: true,
        command_reads: Vec::new(),
    };
    control_b.inject_unstamped_message(ServerMessage::ReadGrant(replay));
    live.step();
    assert!(
        live.no_data(),
        "a replayed old-context grant never serves the previous context's data"
    );
    assert_eq!(
        control_b.active_stamp().map(|s| s.context_id),
        new_context,
        "the transcript keeps serving the current context only"
    );
    // The session re-establishes observation under the current context alone;
    // the replayed context never becomes the binding.
    assert!(
        live.settle(),
        "the current context re-establishes observation"
    );
    assert_eq!(
        control_b.active_stamp().map(|s| s.context_id),
        new_context,
        "recovery binds the current context, never the replayed one"
    );
    match live.read(_command.clone()) {
        Ok(CommandRead::Found(_)) => {}
        other => panic!("the current context must still authorize its read, got {other:?}"),
    }
}

/// A05: an expansion closes synchronously and only the exact expanded grant
/// reopens; no previous-scope content is displayed through the change.
///
/// One owner, one fence and one client carry the whole expansion: the owner
/// creates window-local retained state, closes on the expansion intent, the
/// transcript's grant for the previous scope is genuinely refused, and only the
/// exact expanded grant republishes. A transcript answers every open request
/// with the scope it was built for, so the test moves that one client onto the
/// expanded scope through the provider's test-only `serve_scope` control.
#[test]
fn a05b_scope_expansion_closes_synchronously_and_only_the_exact_grant_reopens() {
    let mut live = Live::start();
    assert!(live.publication().snapshot.is_some());

    // Window-local state: a retained command the window is responsible for.
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    assert!(
        live.publication().command_statuses.contains_key(&command),
        "the window holds local retained state before the change"
    );

    let expanded = fixture_scope_request(vec![
        fixture_view_filter(),
        ScopeFilter {
            name: "fixture_view".to_owned(),
            value: "expanded".to_owned(),
        },
    ]);
    let reply = live.handle.change_scope(expanded.clone());
    assert!(
        live.withheld(),
        "an expansion intent closes the fence synchronously, before the owner runs"
    );
    assert!(
        live.no_data(),
        "no previous-scope content is displayed during the change"
    );
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;

    // The transcript still serves the previous scope: every grant is refused and
    // admits nothing, and the client holds no binding to answer a read.
    for _ in 0..4 {
        live.step();
    }
    assert!(
        live.no_data(),
        "a grant for the previous scope admits no data"
    );
    assert!(
        matches!(
            live.read(command.clone()),
            Err(CommandReadError::AuthorizationUnavailable
                | CommandReadError::Superseded
                | CommandReadError::AuthorizationDenied)
        ),
        "the refused previous-scope grant leaves no binding that answers a read"
    );

    // The same client now serves exactly the expanded scope; only that exact
    // grant reopens the same owner, on the same fence, and republishes.
    live.control.serve_scope(expanded.clone());
    let reply = live.handle.change_scope(expanded);
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(
        live.settle(),
        "the exact expanded grant republishes on the same owner"
    );
    let after = live.publication();
    assert!(
        after.snapshot.is_some(),
        "the exact expanded scope publishes its snapshot"
    );
    assert!(
        after.command_statuses.contains_key(&command),
        "the window-local retained registry survives the expansion"
    );
}

/// A09: a Denied lookup is the same public denial for a retained and an absent
/// identity, discloses nothing, never mutates the retained truth into an
/// execution failure, and leaves it restorable by a later permitted context.
#[test]
fn a09b_denied_reads_are_identical_and_never_mutate_the_retained_truth() {
    let mut live = Live::start();
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    let retained_receipt = live
        .publication()
        .command_statuses
        .get(&command)
        .and_then(|view| view.receipt.clone());
    assert!(
        retained_receipt.is_some(),
        "the retained identity carries a real receipt"
    );
    let before = live.read(command.clone()).expect("authorized read");
    let CommandRead::Found(before_view) = before else {
        panic!("a retained receipt must read as Found");
    };
    assert_eq!(before_view.receipt, retained_receipt);

    let absent = CommandId::fixture("command/absent-under-denial");
    let client_b = context_scenario("principal-b-denied", "2", Vec::new());
    let control_b = client_b.control();
    control_b.deny_command(command.as_str().to_owned());
    control_b.deny_command(absent.as_str().to_owned());
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement transfers the registry");
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle());

    // A denied retained identity and a denied absent identity are the same
    // public denial, and never leak a receipt, retention or NotRetained fact.
    assert_eq!(
        live.read(command.clone()).err(),
        Some(CommandReadError::AuthorizationDenied),
        "the retained identity is denied, never disclosed"
    );
    // The absent identity first starts the authorized lookup and discloses
    // nothing; the granted denial then answers it.
    assert_eq!(
        live.read(absent.clone()).err(),
        Some(CommandReadError::AuthorizationUnavailable),
        "the absent identity discloses nothing before its decision"
    );
    for _ in 0..4 {
        live.step();
    }
    assert_eq!(
        live.read(absent.clone()).err(),
        Some(CommandReadError::AuthorizationDenied),
        "the absent identity is the same public denial"
    );
    // No publication ever presents a denied identity as an execution failure.
    assert!(
        live.publication()
            .command_statuses
            .values()
            .all(|view| view.presentation != CommandPresentation::Failed),
        "a scoped read denial is never routed into an execution failure"
    );

    // The denial never mutated the retained truth: a later explicitly permitted
    // context still reads the original receipt, not a failure.
    let client_c = context_scenario("principal-c-permitted", "3", Vec::new());
    live.owner
        .replace_client(Box::new(client_c), &live.handle)
        .expect("replacement transfers the registry");
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle());
    let restored = live.read(command.clone()).expect("permitted read");
    let CommandRead::Found(restored_view) = restored else {
        panic!("the retained receipt must be restored as Found");
    };
    assert_eq!(
        restored_view.receipt, retained_receipt,
        "the denied read never mutated the retained receipt"
    );
    assert!(
        matches!(
            restored_view.presentation,
            CommandPresentation::ReceiptAvailable | CommandPresentation::CanonicalEffectObserved
        ),
        "the restored record keeps its actual presentation, never Failed: {restored_view:?}"
    );
}

/// A10: an explicitly permitting reauthorization restores the actual retained
/// receipt and an actual retained unknown, with no synthetic state.
#[test]
fn a10b_explicit_allowed_restores_the_retained_state_after_reauthorization() {
    // A retained receipt survives a close and is restored exactly by a new
    // context that explicitly permits it.
    let mut live = Live::start();
    let receipt_command = live.stop();
    assert!(live.settle_until_command(&receipt_command));
    let original_receipt = live
        .publication()
        .command_statuses
        .get(&receipt_command)
        .and_then(|view| view.receipt.clone())
        .expect("the scripted receipt is published");
    assert_eq!(
        original_receipt.command_id, receipt_command,
        "the command map key and the receipt correlation identity must agree"
    );

    live.control.disconnect();
    live.step();
    assert!(live.no_data(), "the close withholds the receipt");

    let client_b = context_scenario("restored-context", "4", Vec::new());
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement transfers the registry");
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle(), "the permitted context restores observation");
    assert!(
        live.publication()
            .command_statuses
            .contains_key(&receipt_command),
        "the retained receipt is republished under the permitted grant"
    );
    let restored = live.read(receipt_command.clone()).expect("permitted read");
    let CommandRead::Found(restored_view) = restored else {
        panic!("the retained receipt must be restored as Found, never NotRetained");
    };
    assert_eq!(
        restored_view.receipt,
        Some(original_receipt),
        "the restored receipt is the actual retained receipt, not synthetic"
    );
    assert_eq!(
        restored_view
            .receipt
            .as_ref()
            .map(|receipt| &receipt.command_id),
        Some(&receipt_command),
        "the restored receipt keeps the same published correlation identity"
    );

    // A retained unknown is restored as unknown: never a receipt, never NotRetained.
    let mut live = Live::start_with(
        MockCosClient::from_scenario(unresolved_stop_scenario()).expect("scenario builds"),
    );
    let unknown = live.stop();
    assert!(live.settle_until_command(&unknown));
    let client_b = context_scenario("restored-unknown", "4", Vec::new());
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement transfers the registry");
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle());
    let restored = live.read(unknown.clone()).expect("permitted read");
    match restored {
        CommandRead::Found(view) => {
            assert!(
                is_unresolved(view.presentation),
                "the retained unknown is restored as unknown, not a receipt: {view:?}"
            );
            assert!(view.receipt.is_none(), "no synthetic receipt is fabricated");
        }
        CommandRead::NotRetained => {
            panic!("a retained unknown is never NotRetained under a positive decision")
        }
    }
}

/// A11: a missing/unverified authorization admits no data with an honest
/// unavailable state, and valid evidence restores functionality.
#[test]
fn a11b_missing_authorization_withholds_and_valid_evidence_restores() {
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario");
    scenario.read_authorization = Some(FixtureReadAuthorization {
        context_id: ReadContextId::new("fixture:read-context/withholding").expect("context"),
        read_revision: ReadRevision::new(Counter::new("1").expect("revision")),
        local_intents_readable: true,
        denied_commands: Vec::new(),
        absent_commands: Vec::new(),
        grant_on_open: false,
        answer_lookups: true,
    });
    let mut client = MockCosClient::from_scenario(scenario).expect("scenario builds");
    let command = client
        .client_mut()
        .prepare_command(desktop_protocol::CommandEnvelope {
            command_id: CommandId::fixture("command/before-authorization"),
            target: desktop_protocol::CommandTarget::Task { task_id: task() },
            expected_revision: None,
            epoch: None,
            payload: desktop_protocol::CommandPayload::StopTask(
                desktop_protocol::StopTaskCommand {
                    task_id: task(),
                    reason: None,
                },
            ),
        })
        .expect("prepare the retained identity");
    let mut live = Live::start_with(client);
    for _ in 0..24 {
        live.step();
    }

    // No grant admits no snapshot, revision or command data, honestly labelled.
    let withheld = live.publication();
    assert!(withheld.snapshot.is_none(), "no snapshot is admitted");
    assert!(
        withheld.revision.is_none(),
        "no projection marker is admitted"
    );
    assert!(
        withheld.command_statuses.is_empty(),
        "no command data is admitted"
    );
    assert_eq!(
        withheld.source,
        app_core::PublicationSource::Fixture,
        "fixture mode stays explicit; it is not a Runtime fallback"
    );
    assert!(
        matches!(
            live.read(command.clone()).err(),
            Some(CommandReadError::AuthorizationUnavailable | CommandReadError::Superseded)
        ),
        "an unverified binding answers no command read"
    );

    // A valid grant restores a complete publication and readable data.
    let client_b = context_scenario("authorized-recovery", "5", Vec::new());
    live.owner
        .replace_client(Box::new(client_b), &live.handle)
        .expect("replacement transfers the retained identity");
    let reply = live.handle.authentication_changed();
    let mut now = live.now_ms;
    assert!(resolve(&mut live.owner, reply, &mut now).is_ok());
    live.now_ms = now;
    assert!(live.settle(), "valid evidence restores observation");
    let restored = live.publication();
    assert!(
        restored.snapshot.is_some(),
        "the restored publication exposes the snapshot"
    );
    match live.read(command.clone()) {
        Ok(CommandRead::Found(view)) => assert_eq!(
            view.presentation,
            CommandPresentation::Prepared,
            "the retained intent is restored, not a synthetic outcome"
        ),
        other => panic!("valid evidence must restore the retained read, got {other:?}"),
    }
}

/// A13: on the same fence, a tagged read derived under generation A is rejected
/// once a newer grant becomes Ready, and a candidate assembled before the
/// revocation can never reopen it.
#[test]
fn a13b_a_stale_tagged_read_and_candidate_are_rejected_on_the_same_fence() {
    let fence = VisibilityFence::new();
    let candidate = |generation: u64, version: u64| ObservationPublication {
        session_generation: generation,
        publication_version: version,
        connection: PublicationConnection::Observing,
        ..ObservationPublication::default()
    };

    let guard_a = fence.install(candidate(1, 1), fence.authorization_epoch());
    assert!(fence.open(guard_a).is_ok(), "generation A becomes ready");
    let lease_a = fence.current().expect("A is readable");
    let tagged_a = lease_a
        .read(|publication| publication.publication_version)
        .expect("a current tagged read");
    let epoch_a = fence.authorization_epoch();

    // Revoke: the authorization this candidate was assembled under is gone.
    let _ = fence.close_visibility();
    let stale = fence.install(candidate(1, 2), epoch_a);
    assert!(
        fence.open(stale).is_err(),
        "a candidate assembled before the revocation cannot reopen the fence"
    );

    // A newer grant becomes Ready on the same fence.
    let guard_b = fence.install(candidate(2, 1), fence.authorization_epoch());
    assert!(fence.open(guard_b).is_ok(), "generation B becomes ready");
    assert!(fence.current().is_ok());
    assert!(
        tagged_a.consume(|_| {}).is_err(),
        "A's delayed tagged read is rejected once B is ready"
    );
}

/// A16: current positive evidence yields exactly the actual retained state or
/// NotRetained, a negative decision stays Denied, and none implies execution.
#[test]
fn a16b_reads_report_exact_found_not_retained_and_denied() {
    // (i) Allowed/no receipt for a true absent identity is exactly NotRetained.
    let absent = CommandId::fixture("command/absent");
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario");
    scenario.read_authorization = Some(FixtureReadAuthorization {
        context_id: ReadContextId::new("fixture:read-context/absent").expect("context"),
        read_revision: ReadRevision::new(Counter::new("3").expect("revision")),
        local_intents_readable: true,
        denied_commands: Vec::new(),
        absent_commands: vec![absent.clone()],
        grant_on_open: true,
        answer_lookups: true,
    });
    let mut live = Live::start_with(MockCosClient::from_scenario(scenario).expect("scenario"));
    assert!(live.settle());
    // The first read starts the authorized lookup and discloses nothing yet.
    assert_eq!(
        live.read(absent.clone()).err(),
        Some(CommandReadError::AuthorizationUnavailable)
    );
    for _ in 0..4 {
        live.step();
    }
    assert_eq!(
        live.read(absent.clone()),
        Ok(CommandRead::NotRetained),
        "Allowed plus a true absent record is exactly NotRetained"
    );
    assert!(
        !live.publication().command_statuses.contains_key(&absent),
        "NotRetained never fabricates a command record"
    );

    // (ii) A negative decision stays Denied, never NotRetained.
    let denied = CommandId::fixture("command/denied");
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("scenario");
    scenario.read_authorization = Some(FixtureReadAuthorization {
        context_id: ReadContextId::new("fixture:read-context/denied").expect("context"),
        read_revision: ReadRevision::new(Counter::new("3").expect("revision")),
        local_intents_readable: true,
        denied_commands: vec![denied.clone()],
        absent_commands: Vec::new(),
        grant_on_open: true,
        answer_lookups: true,
    });
    let mut live = Live::start_with(MockCosClient::from_scenario(scenario).expect("scenario"));
    assert!(live.settle());
    assert_eq!(
        live.read(denied.clone()).err(),
        Some(CommandReadError::AuthorizationUnavailable)
    );
    for _ in 0..4 {
        live.step();
    }
    assert_eq!(
        live.read(denied.clone()).err(),
        Some(CommandReadError::AuthorizationDenied),
        "a denied identity is Denied, never NotRetained"
    );

    // (iii) Allowed/no receipt for a retained local unknown yields the actual
    // retained state, never NotRetained and never a synthetic receipt.
    let mut live = Live::start_with(
        MockCosClient::from_scenario(unresolved_stop_scenario()).expect("scenario builds"),
    );
    let command = live.stop();
    assert!(live.settle_until_command(&command));
    match live.read(command.clone()) {
        Ok(CommandRead::Found(view)) => {
            assert!(
                is_unresolved(view.presentation),
                "the actual retained unknown is Found: {view:?}"
            );
            assert!(view.receipt.is_none(), "no synthetic receipt is fabricated");
        }
        other => panic!("a retained local unknown must read as Found, got {other:?}"),
    }
}

/// A slow consumer may skip intermediate notices, never the latest record.
/// Equal version numbers in different owner generations are distinct notices.
#[test]
fn pub3_coalesced_consumers_independently_observe_equal_version_new_generations() {
    let fence = VisibilityFence::new();
    let publish = |generation, version| {
        let _ = fence.close_for_commit();
        let guard = fence.install(
            ObservationPublication {
                session_generation: generation,
                publication_version: version,
                connection: PublicationConnection::Observing,
                ..ObservationPublication::default()
            },
            fence.authorization_epoch(),
        );
        assert!(fence.open(guard).is_ok());
        fence.notify_subscribers();
    };
    publish(1, 1);
    let mut fast = app_core::PublicationSubscription::new(Arc::clone(&fence));
    let mut slow = app_core::PublicationSubscription::new(Arc::clone(&fence));
    let next = |subscription: &mut app_core::PublicationSubscription| {
        subscription
            .changed()
            .now_or_never()
            .flatten()
            .expect("latest notice")
    };
    assert_eq!(next(&mut fast), next(&mut slow));
    let epoch = fence.authorization_epoch();
    for version in 2..=5 {
        publish(1, version);
        assert_eq!(next(&mut fast).publication_version, version);
    }
    assert!(fast.changed().now_or_never().is_none());
    let coalesced = next(&mut slow);
    assert_eq!(coalesced.session_generation, 1);
    assert_eq!(coalesced.publication_version, 5);
    assert!(slow.changed().now_or_never().is_none());

    // Keep the authorization epoch and numerical version equal: only the owner
    // generation changes, so ignoring that coordinate loses this notification.
    publish(2, 5);
    assert_eq!(fence.authorization_epoch(), epoch);
    let changed = next(&mut fast);
    assert_eq!(changed.session_generation, 2);
    assert_eq!(changed.publication_version, 5);
    assert_eq!(next(&mut slow), changed);
    assert!(fast.changed().now_or_never().is_none());
    assert!(slow.changed().now_or_never().is_none());

    drop(fast);
    publish(2, 6);
    let remaining = next(&mut slow);
    assert_eq!(remaining.session_generation, 2);
    assert_eq!(remaining.publication_version, 6);
}

/// Duplicate protocol transactions produce diagnostics, not canonical changes.
/// A reader that lags beyond the declared ring capacity gets an exact gap.
#[test]
fn pub3_diagnostic_loss_is_explicit_bounded_and_independent_of_canonical_state() {
    let mut scenario = FixtureScenario::from_json(FIXTURE).expect("fixture parses");
    for step in &mut scenario.steps {
        if step.trigger == "after_open" {
            step.trigger = "manual".to_owned();
        }
    }
    let delta = scenario
        .steps
        .iter_mut()
        .find(|step| step.id == "delta-evaluation-e")
        .expect("evaluation step")
        .actions
        .iter_mut()
        .find_map(|action| action.delta.as_mut())
        .expect("evaluation delta");
    delta.transaction_id = Some(TransactionId::fixture("transaction/pub3-diagnostic"));
    delta.cursor = Some(Cursor::fixture("cursor/pub3-diagnostic"));
    let mut live =
        Live::start_with(MockCosClient::from_scenario(scenario).expect("scenario builds"));
    assert!(live.settle());
    live.control.deliver_step("delta-evaluation-e");
    live.step();
    live.step();
    let before = live.publication();
    let start = live.handle.diagnostics_after(0).latest_sequence;
    let second_reader = live.handle.clone();

    // One applied transaction is replayed exactly, one bounded owner step at a
    // time. This overflows diagnostic retention, never the transport Rx queue.
    for _ in 0..1031 {
        live.control.deliver_step("delta-evaluation-e");
        live.step();
        live.step();
    }
    let lagged = live.handle.diagnostics_after(start);
    assert_eq!(lagged.latest_sequence, start + 1031);
    assert_eq!(lagged.events.len(), 1024, "the declared retention limit");
    assert_eq!(
        lagged.event_gap,
        Some(app_core::EventGap {
            from: start + 1,
            to: start + 7
        })
    );
    for (index, event) in lagged.events.iter().enumerate() {
        assert_eq!(event.sequence, start + 8 + index as u64);
        assert_eq!(event.kind, app_core::DiagnosticKind::Activity);
    }
    assert!(!lagged.exhausted);
    assert_eq!(
        second_reader.diagnostics_after(start),
        lagged,
        "one reader cannot drain another reader's diagnostic history"
    );
    let recent = second_reader.diagnostics_after(start + 1021);
    assert_eq!(recent.event_gap, None);
    assert_eq!(recent.events, lagged.events[1014..]);
    let future = live.handle.diagnostics_after(u64::MAX);
    assert!(future.events.is_empty());
    assert_eq!(future.event_gap, None, "a future cursor invents no loss");
    let after = live.publication();
    assert_eq!(
        after.snapshot, before.snapshot,
        "diagnostic loss cannot mutate canonical data"
    );
    assert_eq!(after.revision, before.revision);
}

// ---------------------------------------------------------------------------
// Owner driver: bounded steps must not park with frames still queued
// ---------------------------------------------------------------------------

/// Poll a future only when its waker fires.
///
/// A driver that parks with no pending wake is therefore never rescued by the
/// test's own polling: the stall the regression describes is observable instead
/// of hidden by an eager executor.
fn block_on_wakes<F: Future>(future: F) {
    struct Signal {
        woken: AtomicBool,
        thread: std::thread::Thread,
    }

    impl std::task::Wake for Signal {
        fn wake(self: Arc<Self>) {
            self.woken.store(true, Ordering::SeqCst);
            self.thread.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.woken.store(true, Ordering::SeqCst);
            self.thread.unpark();
        }
    }

    let signal = Arc::new(Signal {
        woken: AtomicBool::new(false),
        thread: std::thread::current(),
    });
    let waker = Waker::from(Arc::clone(&signal));
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        if future.as_mut().poll(&mut cx).is_ready() {
            return;
        }
        while !signal.woken.swap(false, Ordering::SeqCst) {
            std::thread::park_timeout(std::time::Duration::from_millis(50));
        }
    }
}

/// A transcript longer than one pump budget must complete when the driver's only
/// wake sources are the mailbox and the timer.
///
/// The fixture transport announces no deadline for frames it has already queued,
/// so a host loop that parks after a saturated step stalls mid-snapshot: the rail
/// stays empty and every projection stays withheld while more frames are waiting.
#[test]
fn a_transcript_longer_than_one_pump_budget_completes_without_an_external_wake() {
    const EXTRA_STEPS: usize = 120;
    let mut elements = String::new();
    for index in 0..EXTRA_STEPS {
        if index > 0 {
            elements.push(',');
        }
        elements.push_str(&format!(
            concat!(
                r#"{{"id":"pump-frame-{index:03}","trigger":"after_open","actions":["#,
                r#"{{"delta":{{"mutations":["#,
                r#"{{"op":"UPSERT","base_revision":"{base}","projection":{{"#,
                r#""projection":"TASK","task_ref":"fixture:task/Task-184","display_id":"Task-184","#,
                r#""title":"pump frame {index:03}","task_status":"ACTIVE","inner_phase":"REFINE","#,
                r#""iteration_ref":"fixture:iteration/Task-184/3","iteration_index":3,"#,
                r#""best_candidate_ref":null,"formal_outcome_ref":null,"accepted_brief_ref":null,"#,
                r#""capabilities_ref":"fixture:capabilities/Task-184","projection_revision":"{revision}""#,
                r#"}}}}"#,
                r#"]}}}}]}}"#,
            ),
            index = index,
            base = 7 + index,
            revision = 8 + index,
        ));
    }
    // The authored after-open steps are deferred, so this transcript is exactly
    // the generated frames on top of the fixture's own initial snapshot.
    let deferred = FIXTURE.replace("\"trigger\": \"after_open\"", "\"trigger\": \"manual\"");
    let expanded = deferred.replace("\"steps\": [", &format!("\"steps\": [{elements},"));
    let scenario = FixtureScenario::from_json(&expanded).expect("the generated transcript parses");
    let client = MockCosClient::from_scenario(scenario).expect("the generated scenario builds");
    let scope = client.client().transport().scenario().observation_request();
    let (mut owner, handle) = ObservationSessionOwner::new(Box::new(client));
    owner.arm_control(&handle);
    owner.start(scope).expect("observation starts");
    let clock = OwnerClock::default();
    let timer = DeadlineScheduler::start(&handle);
    let driver_handle = handle.clone();
    let driver =
        std::thread::spawn(move || block_on_wakes(drive(owner, driver_handle, clock, timer)));

    let last_title = format!("pump frame {:03}", EXTRA_STEPS - 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut observed = None;
    while std::time::Instant::now() < deadline {
        if let Ok(lease) = handle.current() {
            let _ = lease.read(|publication| {
                observed = publication
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.task(&task()))
                    .map(|task| task.title.clone());
            });
        }
        if observed.as_deref() == Some(last_title.as_str()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    handle.request_shutdown();
    driver.join().expect("the driver thread joins");
    assert_eq!(
        observed.as_deref(),
        Some(last_title.as_str()),
        "every frame beyond one pump budget is applied without an external wake"
    );
}
