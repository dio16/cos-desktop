//! NAV2 adapter test: a Brief create intent admitted before the Runtime receipt
//! exists becomes an accepted receipt in the observation publication without
//! creating a local task row.
//!
//! The transcript is an independently authored fixture; it is a delivery-path
//! test, never a claim of authenticated Runtime behaviour.

use std::task::{Context, Poll, Waker};

use app_core::{
    CommandPresentation, CommandRead, ObservationHandle, ObservationPublication,
    ObservationSessionOwner, OwnerReply,
};
use cos_client::{CommandDefault, CommandPolicy, FixtureScenario, MockControl, MockCosClient};
use desktop_protocol::{
    AuthorizedCommandReceipt, CommandEnvelope, CommandId, CommandOperation, CommandPayload,
    CommandReadDecision, CommandReceipt, CommandTarget, CreateTaskCommand, EpochId, GraphId,
    GraphRef, ReceiptState, RecipeId, RecipeRef, ServerMessage, TaskId,
};

const FIXTURE: &str = include_str!("../../cos-client/fixtures/task-184.json");

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

fn publication_of(handle: &ObservationHandle) -> ObservationPublication {
    let lease = handle.current().expect("fence is open");
    let mut publication = None;
    lease
        .read(|current| publication = Some(current.clone()))
        .expect("lease is current");
    publication.expect("publication derived")
}

/// One owner plus the steerable fixture transcript used for the receipt path.
struct Live {
    owner: ObservationSessionOwner,
    handle: ObservationHandle,
    control: MockControl,
    now_ms: u64,
}

impl Live {
    /// Start a session whose CREATE_TASK default is silent, so the command is
    /// admitted as `pending` and no receipt exists until the test delivers it.
    fn start() -> Self {
        let mut scenario = FixtureScenario::from_json(FIXTURE).expect("fixture parses");
        scenario.commands.insert(
            "CREATE_TASK".to_owned(),
            CommandPolicy {
                default: CommandDefault::Silent,
            },
        );
        let client = MockCosClient::from_scenario(scenario).expect("scenario builds");
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
        assert!(live.settle(), "the fixture snapshot must publish");
        live
    }

    fn settle(&mut self) -> bool {
        for _ in 0..24 {
            self.now_ms += 1_000;
            self.owner.step(self.now_ms);
            if self.handle.current().is_ok() {
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

    fn read(&mut self, id: CommandId) -> Result<CommandRead, app_core::CommandReadError> {
        let reply = self.handle.read_command(id);
        let mut now = self.now_ms;
        let authorized = resolve(&mut self.owner, reply, &mut now);
        self.now_ms = now;
        let authorized = authorized?;
        let mut view = None;
        authorized
            .read(|current| view = Some(current.clone()))
            .map_err(|_| app_core::CommandReadError::Superseded)?;
        Ok(view.expect("authorized read exposes a value under its tag"))
    }

    /// Replace the authenticated client with a fresh session while the form and
    /// its handles stay alive: the observation session restarts, the retained
    /// command registry is transferred, and the new session must re-observe.
    fn restart_session(&mut self) {
        let mut scenario = FixtureScenario::from_json(FIXTURE).expect("fixture parses");
        scenario.commands.insert(
            "CREATE_TASK".to_owned(),
            CommandPolicy {
                default: CommandDefault::Silent,
            },
        );
        let next = MockCosClient::from_scenario(scenario).expect("scenario builds");
        self.owner
            .replace_client(Box::new(next), &self.handle)
            .expect("the retained registry transfers");
        self.reauthorize();
    }

    /// Re-authorize the current client; a revocation changes the decisions the
    /// next grant carries for already-retained commands.
    fn reauthorize(&mut self) {
        let reply = self.handle.authentication_changed();
        let mut now = self.now_ms;
        let _ = resolve(&mut self.owner, reply, &mut now);
        self.now_ms = now;
        assert!(self.settle(), "the re-authorized session republishes");
    }

    /// Submit one create intent and deliver its accepted receipt on the
    /// observation session.
    fn submit_and_accept(&mut self, command_id: &CommandId, goal: &str, task: &TaskId) {
        let envelope = create_envelope(goal, command_id.clone());
        let reply = self.handle.request_create_task(envelope);
        let mut now = self.now_ms;
        let submitted = resolve(&mut self.owner, reply, &mut now);
        self.now_ms = now;
        assert_eq!(submitted.expect("admitted"), *command_id);

        let stamp = self.control.active_stamp().expect("active read binding");
        self.control
            .inject_message(ServerMessage::Receipt(AuthorizedCommandReceipt {
                stamp,
                permission: CommandReadDecision {
                    command_id: command_id.clone(),
                    readable: true,
                },
                receipt: CommandReceipt {
                    command_id: command_id.clone(),
                    operation: CommandOperation::CreateTask,
                    state: ReceiptState::Accepted,
                    task_id: Some(task.clone()),
                    accepted_at: None,
                    reject_reason: None,
                    effect_refs: Vec::new(),
                    dedupe_expires_at: None,
                    message: Some("accepted: validated intent recorded".to_owned()),
                },
            }));
        for _ in 0..16 {
            self.step();
            if self.publication().command_statuses.contains_key(command_id) {
                return;
            }
        }
        panic!("the accepted receipt reaches the publication");
    }
}

fn recipe_ref() -> RecipeRef {
    let digest = format!("sha256:{}", "a".repeat(64));
    RecipeRef {
        recipe_id: RecipeId::new("recipe-1").expect("recipe"),
        graph_ref: GraphRef {
            graph_id: GraphId::new("graph-1").expect("graph"),
            version: 3,
            digest: desktop_protocol::ArtifactDigest::new(digest).expect("digest"),
        },
        source_harness_version_id: None,
        source_harness_digest: None,
        profile_ref: None,
        projection_digest: None,
    }
}

fn create_envelope(goal: &str, command_id: CommandId) -> CommandEnvelope {
    CommandEnvelope {
        command_id,
        target: CommandTarget::Creation {
            recipe_ref: recipe_ref(),
        },
        expected_revision: None,
        epoch: Some(EpochId::new("epoch-1").expect("epoch")),
        payload: CommandPayload::CreateTask(CreateTaskCommand {
            objective_text: goal.to_owned(),
            text_prompt: Some("cinematic still".to_owned()),
            reference_artifact_refs: Vec::new(),
            parameter_values: Vec::new(),
        }),
    }
}

#[test]
fn an_accepted_create_receipt_is_published_for_the_brief_adapter() {
    let mut live = Live::start();
    let command_id = CommandId::fixture("command/nav2-brief-create");
    let envelope = create_envelope("NAV2 receipt adapter", command_id.clone());
    let reply = live.handle.request_create_task(envelope);
    let mut now = live.now_ms;
    let submitted = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert_eq!(submitted.expect("admitted"), command_id);

    // The immediate read the Brief surface performs has no receipt yet: the
    // command is still pending. This is the exact state the defect stopped at.
    match live.read(command_id.clone()).expect("pending read") {
        CommandRead::Found(view) => assert_eq!(
            view.presentation,
            CommandPresentation::AwaitingResponse,
            "a just-admitted create command has no receipt yet"
        ),
        CommandRead::NotRetained => panic!("the admitted command must be retained locally"),
    }

    let snapshot_before = live.publication().snapshot;
    let stamp = live.control.active_stamp().expect("active read binding");
    let task = TaskId::new("task-0123456789abcdef0123456789abcdef").expect("task id");
    live.control
        .inject_message(ServerMessage::Receipt(AuthorizedCommandReceipt {
            stamp,
            permission: CommandReadDecision {
                command_id: command_id.clone(),
                readable: true,
            },
            receipt: CommandReceipt {
                command_id: command_id.clone(),
                operation: CommandOperation::CreateTask,
                state: ReceiptState::Accepted,
                task_id: Some(task.clone()),
                accepted_at: None,
                reject_reason: None,
                effect_refs: Vec::new(),
                dedupe_expires_at: None,
                message: Some("accepted: validated intent recorded".to_owned()),
            },
        }));

    let mut found = false;
    for _ in 0..16 {
        live.step();
        if live
            .publication()
            .command_statuses
            .contains_key(&command_id)
        {
            found = true;
            break;
        }
    }
    assert!(found, "the accepted receipt reaches the publication");

    let publication = live.publication();
    let view = publication
        .command_statuses
        .get(&command_id)
        .expect("published receipt view");
    assert_eq!(view.presentation, CommandPresentation::ReceiptAvailable);
    let receipt = view.receipt.as_ref().expect("receipt body");
    assert_eq!(receipt.state, ReceiptState::Accepted);
    assert_eq!(receipt.task_id.as_ref(), Some(&task));
    assert_eq!(
        publication.snapshot, snapshot_before,
        "an accepted receipt is not a canonical task row"
    );
    assert!(
        publication.last_error.is_none(),
        "no diagnostic error is raised for a definitively accepted command"
    );
}

#[test]
fn a_silent_create_command_is_published_as_pending_never_as_accepted() {
    // A just-admitted command is a local intent: it may be disclosed as
    // pending, but no accepted view and no task row may be invented.
    let mut live = Live::start();
    let snapshot_before = live.publication().snapshot;
    let command_id = CommandId::fixture("command/nav2-brief-silent");
    let envelope = create_envelope("NAV2 silent create", command_id.clone());
    let reply = live.handle.request_create_task(envelope);
    let mut now = live.now_ms;
    let submitted = resolve(&mut live.owner, reply, &mut now);
    live.now_ms = now;
    assert!(submitted.is_ok());
    for _ in 0..16 {
        live.step();
    }
    let publication = live.publication();
    let view = publication
        .command_statuses
        .get(&command_id)
        .expect("the local pending intent is disclosed");
    assert_eq!(view.presentation, CommandPresentation::AwaitingResponse);
    assert!(view.receipt.is_none(), "no receipt was delivered");
    assert_eq!(
        publication.snapshot, snapshot_before,
        "an admitted command is not a canonical task row"
    );
    assert!(publication.last_error.is_none());
}

#[test]
fn a_restarted_observation_session_cannot_downgrade_the_accepted_receipt() {
    // A review NIT: after the Runtime accepted the create command, the
    // observation session may restart while the same Brief form stays open. The
    // restarted session re-observes the transferred intent; it must present the
    // same accepted receipt (or nothing), never a pending/rejected/failed answer
    // that would downgrade the acceptance the Runtime already returned.
    let mut live = Live::start();
    let command_id = CommandId::fixture("command/nav2-brief-restart");
    let task = TaskId::new("task-0123456789abcdef0123456789abcdef").expect("task id");
    live.submit_and_accept(&command_id, "NAV2 restart", &task);
    let snapshot_before = live.publication().snapshot;

    let accepted_before = live
        .publication()
        .command_statuses
        .get(&command_id)
        .cloned()
        .expect("accepted receipt published");
    assert_eq!(
        accepted_before.presentation,
        CommandPresentation::ReceiptAvailable
    );
    assert_eq!(
        accepted_before
            .receipt
            .as_ref()
            .map(|receipt| &receipt.state),
        Some(&ReceiptState::Accepted)
    );

    live.restart_session();

    let after = live.publication();
    match after.command_statuses.get(&command_id) {
        Some(view) => {
            assert_eq!(
                view.presentation,
                CommandPresentation::ReceiptAvailable,
                "the restarted session never re-presents the accepted intent as pending"
            );
            let receipt = view
                .receipt
                .as_ref()
                .expect("the receipt survives the restart");
            assert_eq!(
                receipt.state,
                ReceiptState::Accepted,
                "an accepted receipt is terminal across the session restart"
            );
            assert_eq!(
                receipt.task_id.as_ref(),
                Some(&task),
                "the accepted task reference is preserved, never rewritten"
            );
        }
        None => panic!(
            "the restarted session must disclose the transferred accepted receipt (or the \
             publication stays withheld); it must not silently drop it here: {:?}",
            after
                .command_statuses
                .keys()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>()
        ),
    }
    assert_eq!(
        after.snapshot, snapshot_before,
        "the receipt is not a canonical task row, before or after the restart"
    );
    assert!(
        after.last_error.is_none(),
        "a session restart raises no command diagnostic: {:?}",
        after.last_error
    );

    // The authorized read keeps answering the same accepted receipt.
    match live.read(command_id.clone()).expect("authorized read") {
        CommandRead::Found(view) => {
            assert_eq!(view.presentation, CommandPresentation::ReceiptAvailable);
            let receipt = view.receipt.expect("receipt body");
            assert_eq!(receipt.state, ReceiptState::Accepted);
            assert_eq!(receipt.task_id.as_ref(), Some(&task));
        }
        CommandRead::NotRetained => panic!("the restarted session lost the retained acceptance"),
    }
}

#[test]
fn a_revoked_credential_answers_pending_or_denied_never_a_downgrade() {
    // A review NIT: a credential revocation after the acceptance answers
    // `AuthorizationDenied` (or withholds the publication) for the retained
    // command. Both answers are honest disclosures of missing authorization;
    // neither may be published as a pending/rejected/failed command state and
    // neither may invent a canonical task row.
    let mut live = Live::start();
    let command_id = CommandId::fixture("command/nav2-brief-revoked");
    let task = TaskId::new("task-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").expect("task id");
    live.submit_and_accept(&command_id, "NAV2 revocation", &task);
    let snapshot_before = live.publication().snapshot;

    // The grant stops carrying a positive decision for this retained command.
    live.control.deny_command(command_id.as_str().to_owned());
    live.reauthorize();

    let after = live.publication();
    if let Some(view) = after.command_statuses.get(&command_id) {
        assert_eq!(
            view.presentation,
            CommandPresentation::ReceiptAvailable,
            "a revoked read discloses no fabricated pending state"
        );
        assert_eq!(
            view.receipt.as_ref().map(|receipt| &receipt.state),
            Some(&ReceiptState::Accepted),
            "revocation never rewrites an accepted receipt into a rejection"
        );
        panic!("the revoked decision must withhold the retained command, not disclose it");
    }
    assert!(
        !after.command_statuses.values().any(|view| matches!(
            view.presentation,
            CommandPresentation::Failed | CommandPresentation::Unsupported
        )),
        "a revoked credential is not a command failure"
    );
    assert_eq!(
        after.snapshot, snapshot_before,
        "revocation never fabricates a canonical task row"
    );

    // The read answers a typed denial, never a false "not retained".
    match live.read(command_id.clone()) {
        Err(app_core::CommandReadError::AuthorizationDenied) => {}
        Err(other) => panic!("a revoked decision is a public denial, got {other:?}"),
        Ok(CommandRead::NotRetained) => {
            panic!("a retained command must not answer as not retained under revocation")
        }
        Ok(CommandRead::Found(view)) => panic!(
            "a revoked credential disclosed {:?} for the retained command",
            view.presentation
        ),
    }
}

#[test]
fn a_revoked_receipt_is_restored_after_reauthorization_without_a_local_downgrade() {
    // The same form survives a revocation and a later re-authorization: the
    // accepted receipt is disclosed again by the new grant. This is the
    // "reconnect after revocation" path the Brief adapter re-reads, and it must
    // not depend on a locally fabricated state.
    let mut live = Live::start();
    let command_id = CommandId::fixture("command/nav2-brief-restored");
    let task = TaskId::new("task-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").expect("task id");
    live.submit_and_accept(&command_id, "NAV2 restore", &task);
    live.control.deny_command(command_id.as_str().to_owned());
    live.reauthorize();
    assert!(
        !live
            .publication()
            .command_statuses
            .contains_key(&command_id),
        "the revoked decision withholds the command first"
    );

    // A fresh client replaces the revoked session: the transferred registry is
    // re-decided by the new authenticated session.
    live.restart_session();
    let after = live.publication();
    let view = after
        .command_statuses
        .get(&command_id)
        .expect("the re-authorized session discloses the accepted receipt again");
    assert_eq!(view.presentation, CommandPresentation::ReceiptAvailable);
    let receipt = view.receipt.as_ref().expect("receipt body");
    assert_eq!(receipt.state, ReceiptState::Accepted);
    assert_eq!(receipt.task_id.as_ref(), Some(&task));
    assert!(after.last_error.is_none());
}
