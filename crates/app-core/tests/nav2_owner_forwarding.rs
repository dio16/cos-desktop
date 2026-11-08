//! NAV2 owner forwarding tests: the catalog read and create submission are
//! routed through the observation owner thread and return their real result,
//! not just a trait-forwarding stub.

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use app_core::observation::ClosedClient;
use app_core::{
    Catalog, CatalogDocument, CatalogExecutionLimits, CatalogGraphRef, CatalogInputLimits,
    CatalogRecipe, CatalogRecipeRef, CatalogServer, CommandReadError, ObservationHandle,
    ObservationSessionOwner,
};
use cos_client::{
    ClientError, ClientEvent, CommandLookup, CommandStatus, ConnectionState, DesktopClient,
    PublicationAccess, PumpReport, SnapshotHandle,
};
use desktop_protocol::{
    AuthorizedReceiptLookup, CommandEnvelope, CommandId, CommandPayload, CommandTarget,
    CreateTaskCommand, GraphId, GraphRef, NamedParameter, ObservationScopeRequest,
    OpenAuthorizedObservation, ParameterValue, RecipeId, RecipeRef, ResyncReason, TaskId,
};

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut context = Context::from_waker(std::task::Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

struct RecordingClient {
    inner: ClosedClient,
    catalog: Result<CatalogDocument, ClientError>,
    create_error: Option<ClientError>,
    created: Arc<Mutex<Vec<CommandEnvelope>>>,
}

impl DesktopClient for RecordingClient {
    fn snapshot(&self) -> SnapshotHandle {
        self.inner.snapshot()
    }
    fn connection(&self) -> &ConnectionState {
        self.inner.connection()
    }
    fn is_stale(&self) -> bool {
        self.inner.is_stale()
    }
    fn source(&self) -> cos_client::ClientSource {
        self.inner.source()
    }
    fn last_error(&self) -> Option<&ClientError> {
        self.inner.last_error()
    }
    fn disconnect(&mut self) -> Vec<ClientEvent> {
        self.inner.disconnect()
    }
    fn request_resync(&mut self, reason: ResyncReason) -> Vec<ClientEvent> {
        self.inner.request_resync(reason)
    }
    fn pump(&mut self, now_ms: u64) -> Vec<ClientEvent> {
        self.inner.pump(now_ms)
    }
    fn stop_task(&mut self, task_id: &TaskId) -> Result<CommandId, ClientError> {
        self.inner.stop_task(task_id)
    }
    fn fetch_catalog(&mut self) -> Result<CatalogDocument, ClientError> {
        self.catalog.clone()
    }
    fn create_task(&mut self, envelope: CommandEnvelope) -> Result<CommandId, ClientError> {
        if let Some(error) = &self.create_error {
            return Err(error.clone());
        }
        let id = envelope.command_id.clone();
        self.created.lock().expect("created").push(envelope);
        Ok(id)
    }
    fn bind_observation_control(&mut self, control: cos_client::ObservationControl) {
        self.inner.bind_observation_control(control);
    }
    fn pump_step(&mut self, now_ms: u64, budget: usize) -> PumpReport {
        self.inner.pump_step(now_ms, budget)
    }
    fn command_statuses(&self) -> &std::collections::BTreeMap<CommandId, CommandStatus> {
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
        _request: OpenAuthorizedObservation,
    ) -> Result<(), ClientError> {
        Ok(())
    }
    fn publication_access(&self) -> PublicationAccess<'_> {
        self.inner.publication_access()
    }
    fn lookup_receipt_authorized(
        &mut self,
        request: AuthorizedReceiptLookup,
    ) -> Result<(), ClientError> {
        self.inner.lookup_receipt_authorized(request)
    }
    fn observation_request(
        &self,
        scope: ObservationScopeRequest,
    ) -> OpenAuthorizedObservation {
        self.inner.observation_request(scope)
    }
    fn adopt_retained_registry(
        &mut self,
        records: std::collections::BTreeMap<CommandId, CommandStatus>,
        references: std::collections::BTreeMap<CommandId, usize>,
    ) -> Result<(), ClientError> {
        self.inner.adopt_retained_registry(records, references)
    }
}

fn catalog_document() -> CatalogDocument {
    let digest = format!("sha256:{}", "a".repeat(64));
    CatalogDocument {
        schema_version: 1,
        server: CatalogServer {
            server_identity: "cos-runtime-orchestrator".to_owned(),
            epoch: "epoch-1".to_owned(),
        },
        catalog: Catalog {
            schema_version: 1,
            can_submit: true,
            reason_codes: Vec::new(),
            recipes: vec![CatalogRecipe {
                recipe_ref: CatalogRecipeRef {
                    recipe_id: "recipe-1".to_owned(),
                    source_harness_version_id: None,
                    source_harness_digest: None,
                    graph_ref: CatalogGraphRef {
                        graph_id: "graph-1".to_owned(),
                        graph_version: 3,
                        graph_digest: digest,
                    },
                    profile_ref: None,
                    projection_digest: None,
                },
                label: "Still".to_owned(),
                description: "One still".to_owned(),
                input_limits: CatalogInputLimits {
                    max_images: Some(2),
                },
                accepts_image_references: false,
                execution_limits: CatalogExecutionLimits::default(),
                constraint_summary: "none".to_owned(),
                parameters: Vec::new(),
            }],
        },
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

struct RunningOwner {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for RunningOwner {
    fn drop(&mut self) {
        self.stop
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn start_owner(client: RecordingClient) -> (ObservationHandle, RunningOwner) {
    let (mut owner, handle) = ObservationSessionOwner::new(Box::new(client));
    owner.arm_control(&handle);
    owner
        .start(ObservationScopeRequest {
            task_ids: Vec::new(),
            filters: Vec::new(),
        })
        .expect("owner starts");
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut now = 0u64;
            while !stop.load(std::sync::atomic::Ordering::Acquire) {
                owner.step(now);
                now += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    (
        handle,
        RunningOwner {
            stop,
            thread: Some(thread),
        },
    )
}

#[test]
fn fetch_catalog_forwards_through_the_owner_and_returns_the_document() {
    let (handle, _owner) = start_owner(RecordingClient {
        inner: ClosedClient,
        catalog: Ok(catalog_document()),
        create_error: None,
        created: Arc::new(Mutex::new(Vec::new())),
    });
    let document = block_on(handle.fetch_catalog()).expect("catalog");
    assert_eq!(document.server.epoch, "epoch-1");
    assert_eq!(document.catalog.recipes.len(), 1);
}

#[test]
fn fetch_catalog_maps_a_client_error_to_the_command_read_category() {
    let (handle, _owner) = start_owner(RecordingClient {
        inner: ClosedClient,
        catalog: Err(ClientError::Transport("test".to_owned())),
        create_error: None,
        created: Arc::new(Mutex::new(Vec::new())),
    });
    let error = block_on(handle.fetch_catalog()).expect_err("refused");
    assert!(matches!(
        error,
        CommandReadError::Client(app_core::PublicationError::Transport)
    ));
}

#[test]
fn create_task_forwards_the_envelope_and_returns_the_command_id() {
    let created = Arc::new(Mutex::new(Vec::new()));
    let (handle, _owner) = start_owner(RecordingClient {
        inner: ClosedClient,
        catalog: Ok(catalog_document()),
        create_error: None,
        created: Arc::clone(&created),
    });
    let command_id = CommandId::generate();
    let envelope = CommandEnvelope {
        command_id: command_id.clone(),
        target: CommandTarget::Creation {
            recipe_ref: recipe_ref(),
        },
        expected_revision: None,
        epoch: Some(desktop_protocol::EpochId::new("epoch-1").expect("epoch")),
        payload: CommandPayload::CreateTask(CreateTaskCommand {
            objective_text: "goal".to_owned(),
            text_prompt: None,
            reference_artifact_refs: Vec::new(),
            parameter_values: vec![NamedParameter {
                name: "seed".to_owned(),
                value: ParameterValue::Integer(7),
            }],
        }),
    };
    let returned = block_on(handle.request_create_task(envelope)).expect("submitted");
    assert_eq!(returned, command_id);
    let recorded = created.lock().expect("created");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].command_id, command_id);
    assert_eq!(
        recorded[0].epoch,
        Some(desktop_protocol::EpochId::new("epoch-1").expect("epoch"))
    );
    assert!(matches!(
        recorded[0].payload,
        CommandPayload::CreateTask(_)
    ));
}

#[test]
fn create_task_maps_a_local_invalid_command_to_the_safe_category() {
    let (handle, _owner) = start_owner(RecordingClient {
        inner: ClosedClient,
        catalog: Ok(catalog_document()),
        create_error: Some(ClientError::InvalidCommand(
            "CommandCapacityExceeded".to_owned(),
        )),
        created: Arc::new(Mutex::new(Vec::new())),
    });
    let envelope = CommandEnvelope {
        command_id: CommandId::generate(),
        target: CommandTarget::Task {
            task_id: TaskId::new("task-1").expect("task"),
        },
        expected_revision: None,
        epoch: None,
        payload: CommandPayload::CreateTask(CreateTaskCommand {
            objective_text: "goal".to_owned(),
            text_prompt: None,
            reference_artifact_refs: Vec::new(),
            parameter_values: Vec::new(),
        }),
    };
    let error = block_on(handle.request_create_task(envelope)).expect_err("refused");
    assert!(matches!(
        error,
        CommandReadError::Client(app_core::PublicationError::InvalidCommand(
            app_core::InvalidCommandReason::CapacityExceeded
        ))
    ));
}

#[test]
fn owner_rejects_a_second_create_reply_for_a_stale_intent() {
    // Guard the expected error category surface: a closed channel refuses the
    // intent instead of fabricating a submitted command.
    let (handle, _owner) = start_owner(RecordingClient {
        inner: ClosedClient,
        catalog: Ok(catalog_document()),
        create_error: Some(ClientError::NotConnected("closed".to_owned())),
        created: Arc::new(Mutex::new(Vec::new())),
    });
    let envelope = CommandEnvelope {
        command_id: CommandId::generate(),
        target: CommandTarget::Task {
            task_id: TaskId::new("task-1").expect("task"),
        },
        expected_revision: None,
        epoch: None,
        payload: CommandPayload::CreateTask(CreateTaskCommand {
            objective_text: "goal".to_owned(),
            text_prompt: None,
            reference_artifact_refs: Vec::new(),
            parameter_values: Vec::new(),
        }),
    };
    let error = block_on(handle.request_create_task(envelope)).expect_err("refused");
    assert!(matches!(
        error,
        CommandReadError::Client(app_core::PublicationError::NotConnected)
    ));
}
