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
