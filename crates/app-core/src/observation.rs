//! The single process-wide observation session owner, its published evidence and
//! the app-core presentation contract consumed by every window.
//!
//! The owner owns the client, the pump loop and the visibility fence. Windows
//! never own or pump a client: they subscribe, read a complete publication under
//! a tag and route intents through the handle. All derivation happens off the
//! main thread.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use cos_client::{
    ClientError, ClientEvent, ClientSource, CommandLookup, CommandStatus, ConnectionState,
    DesktopClient, ObservationControl, PublicationAccess, SnapshotHandle,
};
use desktop_protocol::{
    CommandId, CommandReceipt, Counter, ErrorCode, ObservationScopeRequest, ReadStamp,
    ResyncReason, TaskId,
};

use crate::diagnostics::{DiagnosticBatch, DiagnosticKind, DiagnosticRing};

use crate::fence::{
    AuthorizedCommandRead, ObservationLease, PublicationSubscription, VisibilityFence,
};

/// Bounded intent mailbox shared by every surface.
pub const MAX_INTENTS: usize = 64;

/// Bounded number of registered shutdown waiters.
pub const MAX_SHUTDOWN_WAITERS: usize = 16;

/// Upper bound on repeated releases tracked for one identity.
pub const MAX_SURFACE_REFERENCES: usize = 64;

/// Bounded default pump budget per owner step.
pub const PUMP_BUDGET: usize = 64;

/// Safe reasons a publication withholds all projection and command data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadUnavailable {
    /// No verified channel and no current complete grant.
    AuthorizationUnavailable,
    /// Synchronization or a fresh grant has not completed.
    Synchronizing,
    /// The binding was invalidated or replaced.
    Superseded,
    /// The observation has been shut down.
    Closed,
    /// A non-reused counter could not be advanced.
    Exhausted,
}

/// Safe publication error category; never raw identity, grant or remote text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicationError {
    /// Transport failure.
    Transport,
    /// Malformed or contradictory protocol content.
    Protocol,
    /// No mutually supported version pair.
    VersionIncompatible,
    /// Authorization failure.
    Unauthorized,
    /// A read is waiting for authorization evidence; no data is disclosed yet.
    VerificationPending,
    /// Bounded receive-queue overflow.
    QueueOverflow,
    /// A frame exceeded the negotiated limit.
    MessageTooLarge,
    /// The session is not usable for this operation.
    NotConnected,
    /// A command intent was rejected locally.
    InvalidCommand(InvalidCommandReason),
    /// Structured server error, reduced to its code.
    Server(ErrorCode),
    /// Explicit fixture definition failure.
    Fixture,
}

/// Safe local command rejection category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidCommandReason {
    /// The bounded unsettled or tombstone capacity is exhausted.
    CapacityExceeded,
    /// The same intent identity was already used.
    DuplicateIntent,
    /// The bounded reference budget is exhausted.
    ReferenceLimit,
    /// Another local rejection.
    Other,
}

impl PublicationError {
    fn from_client(error: &ClientError) -> Self {
        match error {
            ClientError::Transport(_) => Self::Transport,
            ClientError::Protocol(_) => Self::Protocol,
            ClientError::VersionIncompatible(_) => Self::VersionIncompatible,
            ClientError::QueueOverflow => Self::QueueOverflow,
            ClientError::MessageTooLarge { .. } => Self::MessageTooLarge,
            ClientError::NotConnected(_) => Self::NotConnected,
            ClientError::InvalidCommand(message) => Self::InvalidCommand(match message.as_str() {
                "CommandCapacityExceeded" => InvalidCommandReason::CapacityExceeded,
                "CommandReferenceLimitExceeded" => InvalidCommandReason::ReferenceLimit,
                other if other.contains("already prepared") => {
                    InvalidCommandReason::DuplicateIntent
                }
                _ => InvalidCommandReason::Other,
            }),
            ClientError::Server(error) => Self::Server(error.code.clone()),
            ClientError::Fixture(_) => Self::Fixture,
        }
    }
}

/// Safe connection category of one publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationConnection {
    /// No session.
    Disconnected,
    /// Handshake in progress.
    Connecting,
    /// Live observation.
    Observing,
    /// Bounded recovery in progress.
    Recovering,
    /// Authorization failure.
    Unauthorized,
    /// No mutually supported version pair.
    Incompatible,
}

/// Safe data-source category of one publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationSource {
    /// A real server claimed and verified by the adapter.
    Runtime,
    /// An explicit scripted fixture transcript.
    Fixture,
    /// No trustworthy source.
    Unavailable,
}

/// Presentation of one retained command; never a derived Domain outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandPresentation {
    /// Local intent recorded, not sent.
    Prepared,
    /// Delivery attempted.
    AwaitingResponse,
    /// A receipt is available.
    ReceiptAvailable,
    /// A canonical effect references this command.
    CanonicalEffectObserved,
    /// The outcome is unknown.
    OutcomeUnconfirmed,
    /// Explicitly unsupported.
    Unsupported,
    /// Definitive failure.
    Failed,
}

/// One authorized command view.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandReadView {
    /// Presentation state.
    pub presentation: CommandPresentation,
    /// Actual receipt, when one exists and is readable.
    pub receipt: Option<CommandReceipt>,
    /// Safe issue category.
    pub issue: Option<PublicationError>,
}

/// Authorized read answer.
// Transient per-intent answer, not stored in bulk: boxing would allocate per read.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum CommandRead {
    /// The actual retained state under a current positive decision.
    Found(CommandReadView),
    /// No retained record, only with current positive read evidence.
    NotRetained,
}

/// How an authorized answer resolved an outstanding read diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadResolution {
    /// The read is answered under the current binding.
    Resolved,
    /// The current binding refuses this identity.
    Denied,
}

/// Typed failure of an owner-routed read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandReadError {
    /// No current validated binding.
    AuthorizationUnavailable,
    /// A current negative decision forbids disclosure.
    AuthorizationDenied,
    /// The binding was invalidated or replaced.
    Superseded,
    /// The bounded read budget or registry capacity is exhausted.
    CapacityExceeded,
    /// Safe client error category.
    Client(PublicationError),
}

/// Why an owner replacement could not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplacementError {
    /// The outgoing registry was empty; replacement by an empty registry is refused.
    EmptyRegistry,
    /// The replacement client refused the transferred registry.
    TransferRejected,
}

/// Version coordinates of one complete publication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublicationNotice {
    /// Owner generation of the record.
    pub session_generation: u64,
    /// Monotonic publication version within that generation.
    pub publication_version: u64,
}

/// One complete immutable publication.
#[derive(Clone, Debug)]
pub struct ObservationPublication {
    /// Authoritative projection snapshot, when one is admitted.
    pub snapshot: Option<SnapshotHandle>,
    /// Owner-local projection-commit marker, not Domain truth.
    pub revision: Option<Counter>,
    /// Safe connection category.
    pub connection: PublicationConnection,
    /// Safe source category.
    pub source: PublicationSource,
    /// Whether the visible projection may be stale.
    pub stale: bool,
    /// Safe error category.
    pub last_error: Option<PublicationError>,
    /// Authorized command views.
    pub command_statuses: BTreeMap<CommandId, CommandReadView>,
    /// Owner generation.
    pub session_generation: u64,
    /// Monotonic publication version.
    pub publication_version: u64,
}

impl Default for ObservationPublication {
    fn default() -> Self {
        Self {
            snapshot: None,
            revision: None,
            connection: PublicationConnection::Disconnected,
            source: PublicationSource::Unavailable,
            stale: true,
            last_error: None,
            command_statuses: BTreeMap::new(),
            session_generation: 0,
            publication_version: 0,
        }
    }
}

/// Creation generation, authorization epoch and visibility sequence of one intent.
///
/// A write intent is invalidated by any authorization change in its generation; a
/// read additionally requires the same visibility window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IntentTag {
    generation: u64,
    visibility_sequence: u64,
    authorization_epoch: u64,
}

/// One bounded owner intent.
enum ObservationCommand {
    ChangeScope {
        scope: ObservationScopeRequest,
        reply: Reply<Result<(), CommandReadError>>,
    },
    AuthenticationChanged {
        reply: Reply<Result<(), CommandReadError>>,
    },
    RequestStop {
        task: TaskId,
        tag: IntentTag,
        reply: Reply<Result<CommandId, CommandReadError>>,
    },
    ReadCommand {
        id: CommandId,
        tag: IntentTag,
        reply: Reply<Result<AuthorizedCommandRead, CommandReadError>>,
    },
    Retain {
        id: CommandId,
        tag: IntentTag,
        reply: Reply<Result<(), CommandReadError>>,
    },
}

type SharedReply<T> = Arc<Mutex<ReplyState<T>>>;

struct ReplyState<T> {
    value: Option<T>,
    waker: Option<Waker>,
    answered: bool,
}

/// A bounded one-response future resolved by the owner.
/// Shared shutdown completion plus the value producer for this reply type.
type ShutdownWait<T> = (Arc<Mutex<ShutdownCompletion>>, fn() -> T);

pub struct OwnerReply<T> {
    state: Option<SharedReply<T>>,
    shutdown: Option<ShutdownWait<T>>,
}

impl OwnerReply<()> {
    /// Wait on the shared shutdown completion; the value is permanent.
    fn shutdown(completion: Arc<Mutex<ShutdownCompletion>>, already_done: bool) -> Self {
        if already_done {
            let state: SharedReply<()> = Arc::new(Mutex::new(ReplyState {
                value: Some(()),
                waker: None,
                answered: true,
            }));
            return Self {
                state: Some(state),
                shutdown: None,
            };
        }
        fn unit() {}
        Self {
            state: None,
            shutdown: Some((completion, unit)),
        }
    }
}

impl<T> OwnerReply<T> {
    fn new() -> (Self, Reply<T>) {
        let state: SharedReply<T> = Arc::new(Mutex::new(ReplyState {
            value: None,
            waker: None,
            answered: false,
        }));
        (
            Self {
                state: Some(Arc::clone(&state)),
                shutdown: None,
            },
            Reply { state },
        )
    }
}

impl<T> Future for OwnerReply<T> {
    type Output = T;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        if let Some((shutdown, ready)) = this.shutdown.as_ref() {
            let mut completion = shutdown.lock().expect("shutdown lock");
            if completion.done {
                return Poll::Ready(ready());
            }
            if completion.wakers.len() >= MAX_SHUTDOWN_WAITERS {
                // Bounded waiter list: recycle the oldest waiter so it re-polls and
                // re-registers instead of waiting forever.
                let oldest = completion.wakers.remove(0);
                oldest.wake();
            }
            completion.wakers.push(cx.waker().clone());
            return Poll::Pending;
        }
        let state = this.state.as_ref().expect("reply state is present");
        let mut state = state.lock().expect("reply lock");
        if let Some(value) = state.value.take() {
            return Poll::Ready(value);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

struct Reply<T> {
    state: SharedReply<T>,
}

impl<T> Reply<T> {
    fn resolve(&self, value: T) {
        let mut state = self.state.lock().expect("reply lock");
        state.value = Some(value);
        state.answered = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

#[derive(Default)]
struct Mailbox {
    intents: VecDeque<ObservationCommand>,
    closed: bool,
}

/// The single process-wide observation session owner.
pub struct ObservationSessionOwner {
    client: Box<dyn DesktopClient>,
    fence: Arc<VisibilityFence>,
    mailbox: Arc<Mutex<Mailbox>>,
    diagnostics: Arc<Mutex<DiagnosticRing>>,
    scope: Option<ObservationScopeRequest>,
    generation: u64,
    /// Generation cell shared with every handle for intent tagging.
    generation_cell: Arc<AtomicU64>,
    /// Dedicated shutdown completion shared with every surface.
    shutdown: Arc<Mutex<ShutdownCompletion>>,
    /// Credits and pending releases under one lock.
    references: Arc<Mutex<ReferenceLedger>>,
    publication_version: u64,
    /// Strong last-snapshot reference: identity comparison without ABA reuse.
    last_snapshot: Option<SnapshotHandle>,
    projection_marker: Option<Counter>,
    projection_counter: u128,
    /// Set by a failed owner replacement: no authorization-derived publication may
    /// be admitted until a fresh authorization succeeds.
    awaiting_reauthorization: bool,
    /// Safe diagnostic from the most recent refused read intent, with the identity
    /// whose outstanding authorization it describes.
    last_read_error: Option<(CommandId, PublicationError)>,
    /// Whether that diagnostic changed since the last publication.
    diagnostic_dirty: bool,
    /// Whether the diagnostic was recorded in this step and must be published
    /// before an already-arrived answer can clear it.
    diagnostic_fresh: bool,
    deadline_ms: Option<u64>,
    /// Whether the last step consumed its whole pump budget, so more frames may
    /// be queued without any further wake.
    pump_saturated: bool,
    shutdown_applied: bool,
}

impl std::fmt::Debug for ObservationSessionOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObservationSessionOwner")
            .field("generation", &self.generation)
            .field("publication_version", &self.publication_version)
            .field("shutdown_applied", &self.shutdown_applied)
            .finish_non_exhaustive()
    }
}

/// Waker slot shared with the async driver.
#[derive(Default)]
pub struct WakeSlot {
    waker: Mutex<Option<Waker>>,
    notified: AtomicBool,
}

impl WakeSlot {
    /// Wake the owner task.
    pub fn notify(&self) {
        self.notified.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().expect("wake lock").take() {
            waker.wake();
        }
    }

    fn poll_cleared(&self, cx: &mut Context<'_>) -> bool {
        // Register the waker first: a notify() racing between the flag check and
        // the registration would otherwise be lost. The flag is re-checked after
        // registration so both interleavings are covered.
        {
            let mut slot = self.waker.lock().expect("wake lock");
            if self.notified.swap(false, Ordering::AcqRel) {
                return true;
            }
            *slot = Some(cx.waker().clone());
        }
        self.notified.swap(false, Ordering::AcqRel)
    }
}

/// A waker that notifies the owner task.
pub fn wake_waker(slot: Arc<WakeSlot>) -> Waker {
    struct SlotWaker(Arc<WakeSlot>);
    impl std::task::Wake for SlotWaker {
        fn wake(self: Arc<Self>) {
            self.0.notify();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.notify();
        }
    }
    Waker::from(Arc::new(SlotWaker(slot)))
}

/// Opaque owner handle shared by every surface.
pub struct ObservationHandle {
    mailbox: Arc<Mutex<Mailbox>>,
    diagnostics: Arc<Mutex<DiagnosticRing>>,
    fence: Arc<VisibilityFence>,
    wake: Arc<WakeSlot>,
    owner_done: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    /// Dedicated shutdown completion, independent of the bounded mailbox.
    shutdown: Arc<Mutex<ShutdownCompletion>>,
    /// Credits and pending releases under one lock.
    references: Arc<Mutex<ReferenceLedger>>,
}

/// Credits (references the owner granted) and releases awaiting application.
#[derive(Default)]
pub struct ReferenceLedger {
    credits: BTreeMap<CommandId, usize>,
    pending: BTreeMap<CommandId, usize>,
}

/// Completion state of the single process-wide shutdown.
#[derive(Default)]
struct ShutdownCompletion {
    requested: bool,
    done: bool,
    wakers: Vec<Waker>,
}

impl std::fmt::Debug for ShutdownCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShutdownCompletion")
            .field("requested", &self.requested)
            .field("done", &self.done)
            .field("waiters", &self.wakers.len())
            .finish()
    }
}

impl Clone for ObservationHandle {
    fn clone(&self) -> Self {
        Self {
            mailbox: Arc::clone(&self.mailbox),
            diagnostics: Arc::clone(&self.diagnostics),
            fence: Arc::clone(&self.fence),
            wake: Arc::clone(&self.wake),
            owner_done: Arc::clone(&self.owner_done),
            generation: Arc::clone(&self.generation),
            shutdown: Arc::clone(&self.shutdown),
            references: Arc::clone(&self.references),
        }
    }
}

impl ObservationHandle {
    /// Read retained safe diagnostics strictly after a per-reader sequence cursor.
    ///
    /// This is a nondestructive diagnostic snapshot, not an authorization,
    /// publication, command-truth or correctness channel.
    pub fn diagnostics_after(&self, after_sequence: u64) -> DiagnosticBatch {
        lock_diagnostics(&self.diagnostics).read_after(after_sequence)
    }

    /// Subscribe to complete publication versions.
    pub fn subscribe(&self) -> PublicationSubscription {
        PublicationSubscription::new(Arc::clone(&self.fence))
    }

    /// Current complete publication under a lease.
    pub fn current(&self) -> Result<ObservationLease, ReadUnavailable> {
        self.fence.current()
    }

    /// Change the observation scope (local scope intent; closes synchronously).
    pub fn change_scope(
        &self,
        scope: ObservationScopeRequest,
    ) -> OwnerReply<Result<(), CommandReadError>> {
        // A scope intent closes the shared fence before any queued I/O, inside the
        // boundary that serializes it against command dispatch.
        let _gate = self.fence.enter_command_boundary();
        let _ = self.fence.close_visibility();
        let (reply, resolver) = OwnerReply::new();
        self.enqueue(ObservationCommand::ChangeScope {
            scope,
            reply: resolver,
        });
        reply
    }

    /// Signal a principal or authentication-context change.
    pub fn authentication_changed(&self) -> OwnerReply<Result<(), CommandReadError>> {
        let _gate = self.fence.enter_command_boundary();
        let _ = self.fence.close_visibility();
        let (reply, resolver) = OwnerReply::new();
        self.enqueue(ObservationCommand::AuthenticationChanged { reply: resolver });
        reply
    }

    /// Route a Stop request through the owner.
    pub fn request_stop(&self, task: TaskId) -> OwnerReply<Result<CommandId, CommandReadError>> {
        let (reply, resolver) = OwnerReply::new();
        let tag = self.intent_tag();
        self.enqueue(ObservationCommand::RequestStop {
            task,
            tag,
            reply: resolver,
        });
        reply
    }

    /// Creation tag for a new intent.
    fn intent_tag(&self) -> IntentTag {
        IntentTag {
            generation: self.generation.load(Ordering::Acquire),
            visibility_sequence: self.fence.sequence(),
            authorization_epoch: self.fence.authorization_epoch(),
        }
    }

    /// Read one command under the current authorization.
    pub fn read_command(
        &self,
        id: CommandId,
    ) -> OwnerReply<Result<AuthorizedCommandRead, CommandReadError>> {
        let (reply, resolver) = OwnerReply::new();
        let tag = self.intent_tag();
        self.enqueue(ObservationCommand::ReadCommand {
            id,
            tag,
            reply: resolver,
        });
        reply
    }

    /// Retain one surface reference to a command record.
    pub fn retain_command(&self, id: CommandId) -> OwnerReply<Result<(), CommandReadError>> {
        let (reply, resolver) = OwnerReply::new();
        let tag = self.intent_tag();
        self.enqueue(ObservationCommand::Retain {
            id,
            tag,
            reply: resolver,
        });
        reply
    }

    /// Release one surface reference to a command record.
    ///
    /// Releases travel on a dedicated counted queue, so a saturated intent mailbox
    /// can never leak a reference and repeated retains are released as often as
    /// they were requested.
    pub fn release_command(&self, id: CommandId) {
        {
            // One ledger lock covers credits and pending releases: only a reference
            // the owner granted may be released, the queue stays within the bounded
            // budget, and a transfer cannot observe a half-applied release.
            let mut ledger = self.references.lock().expect("reference ledger");
            let Some(credit) = ledger.credits.get_mut(&id) else {
                return;
            };
            if *credit == 0 {
                return;
            }
            *credit -= 1;
            if *credit == 0 {
                ledger.credits.remove(&id);
            }
            let pending = ledger.pending.entry(id.clone()).or_insert(0);
            *pending += 1;
        }
        self.wake.notify();
    }

    /// Final shutdown: close first, then reject and dispose.
    ///
    /// The fence closes synchronously and the request travels on a dedicated slot,
    /// so a saturated intent mailbox can neither swallow nor falsely complete it.
    pub fn shutdown(&self) -> OwnerReply<()> {
        self.request_shutdown();
        let already_done = self.shutdown.lock().expect("shutdown lock").done;
        OwnerReply::shutdown(Arc::clone(&self.shutdown), already_done)
    }

    /// Request shutdown without awaiting completion.
    pub fn request_shutdown(&self) {
        let _ = self.fence.close_visibility();
        {
            let mut completion = self.shutdown.lock().expect("shutdown lock");
            completion.requested = true;
        }
        self.wake.notify();
    }

    /// Request shutdown and wait, bounded, until the owner reports disposal.
    ///
    /// Used by the application exit path, where the final window close must be
    /// followed by a confirmed owner stop before the process quits.
    pub fn shutdown_blocking(&self, timeout: std::time::Duration) -> bool {
        let _ = self.fence.close_visibility();
        {
            let mut completion = self.shutdown.lock().expect("shutdown lock");
            completion.requested = true;
        }
        self.wake.notify();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.owner_done.load(Ordering::Acquire) {
                return true;
            }
            if self.shutdown.lock().expect("shutdown lock").done {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Whether the owner loop has finished.
    pub fn is_finished(&self) -> bool {
        self.owner_done.load(Ordering::Acquire)
    }

    /// Wake the owner task (timers, adapter readiness).
    pub fn wake(&self) {
        self.wake.notify();
    }

    /// Shared wake slot for the host driver.
    pub fn wake_slot(&self) -> Arc<WakeSlot> {
        Arc::clone(&self.wake)
    }

    /// Enqueue one bounded intent; never overwrites a dispatched command.
    fn enqueue(&self, command: ObservationCommand) {
        {
            let mut mailbox = self.mailbox.lock().expect("mailbox lock");
            if mailbox.closed || mailbox.intents.len() >= MAX_INTENTS {
                drop(mailbox);
                reject(command);
                return;
            }
            mailbox.intents.push_back(command);
        }
        self.wake.notify();
    }
}

/// Answer a stale intent explicitly: it is never redirected to another context.
fn reject_stale(command: ObservationCommand) {
    match command {
        ObservationCommand::RequestStop { reply, .. } => {
            reply.resolve(Err(CommandReadError::Superseded));
        }
        ObservationCommand::ReadCommand { reply, .. } => {
            reply.resolve(Err(CommandReadError::Superseded));
        }
        ObservationCommand::Retain { reply, .. } => {
            reply.resolve(Err(CommandReadError::Superseded));
        }
        ObservationCommand::ChangeScope { .. }
        | ObservationCommand::AuthenticationChanged { .. } => {}
    }
}

fn reject(command: ObservationCommand) {
    match command {
        ObservationCommand::ChangeScope { reply, .. } => {
            reply.resolve(Err(CommandReadError::AuthorizationUnavailable));
        }
        ObservationCommand::AuthenticationChanged { reply } => {
            reply.resolve(Err(CommandReadError::AuthorizationUnavailable));
        }
        ObservationCommand::RequestStop { reply, .. } => {
            reply.resolve(Err(CommandReadError::CapacityExceeded));
        }
        ObservationCommand::ReadCommand { reply, .. } => {
            reply.resolve(Err(CommandReadError::CapacityExceeded));
        }
        ObservationCommand::Retain { reply, .. } => {
            reply.resolve(Err(CommandReadError::CapacityExceeded));
        }
    }
}

impl ObservationSessionOwner {
    /// Build an owner around one client and install its visibility fence.
    ///
    /// Not a window execution path: hosts drive [`Self::step`] from a bounded
    /// background loop.
    pub fn new(client: Box<dyn DesktopClient>) -> (Self, ObservationHandle) {
        let fence = VisibilityFence::new();
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let diagnostics = Arc::new(Mutex::new(DiagnosticRing::new()));
        let wake = Arc::new(WakeSlot::default());
        let owner_done = Arc::new(AtomicBool::new(false));
        let generation_cell = Arc::new(AtomicU64::new(1));
        let shutdown: Arc<Mutex<ShutdownCompletion>> =
            Arc::new(Mutex::new(ShutdownCompletion::default()));
        let references: Arc<Mutex<ReferenceLedger>> =
            Arc::new(Mutex::new(ReferenceLedger::default()));
        let owner = Self {
            client,
            fence: Arc::clone(&fence),
            mailbox: Arc::clone(&mailbox),
            diagnostics: Arc::clone(&diagnostics),
            scope: None,
            generation: 1,
            generation_cell: Arc::clone(&generation_cell),
            shutdown: Arc::clone(&shutdown),
            references: Arc::clone(&references),
            publication_version: 0,
            last_snapshot: None,
            projection_marker: None,
            projection_counter: 0,
            awaiting_reauthorization: false,
            last_read_error: None,
            diagnostic_dirty: false,
            diagnostic_fresh: false,
            deadline_ms: None,
            pump_saturated: false,
            shutdown_applied: false,
        };
        let handle = ObservationHandle {
            mailbox,
            diagnostics,
            fence,
            wake,
            owner_done,
            generation: generation_cell,
            shutdown,
            references,
        };
        (owner, handle)
    }

    /// Arm the shared close-only control the client must call.
    pub fn arm_control(&mut self, handle: &ObservationHandle) {
        self.client.bind_observation_control(ObservationControl {
            wake: wake_waker(Arc::clone(&handle.wake)),
            close_visibility: close_closure(handle),
        });
    }

    /// Start observing an initial scope (first scope intent).
    pub fn start(&mut self, scope: ObservationScopeRequest) -> Result<(), CommandReadError> {
        self.begin_scope(scope)
    }

    /// Replace the client while transferring the retained registry.
    ///
    /// Exactly one owner remains; the replacement starts unreadable and a
    /// failed transfer stays unavailable.
    pub fn replace_client(
        &mut self,
        mut next: Box<dyn DesktopClient>,
        handle: &ObservationHandle,
    ) -> Result<(), ReplacementError> {
        // Replacement closes first: the old authorized publication must never stay
        // readable, whether or not the transfer succeeds.
        let _ = self.fence.close_visibility();
        self.last_snapshot = None;
        self.projection_marker = None;
        // One boundary covers release application, the registry snapshot and the
        // reference snapshot: a release arriving meanwhile waits, and a reference
        // can neither be released twice nor dropped during the transfer.
        let (records, references) = {
            let mut ledger = self.references.lock().expect("reference ledger");
            let pending = std::mem::take(&mut ledger.pending);
            for (id, count) in pending {
                for _ in 0..count {
                    self.client.release_command(&id);
                }
            }
            let records = self.client.command_statuses().clone();
            let references: BTreeMap<CommandId, usize> = ledger
                .credits
                .iter()
                .filter(|(_, count)| **count > 0)
                .map(|(id, count)| (id.clone(), *count))
                .collect();
            (records, references)
        };
        let transferred = if records.is_empty() {
            Err(ReplacementError::EmptyRegistry)
        } else {
            next.adopt_retained_registry(records, references)
                .map_err(|_| ReplacementError::TransferRejected)
        };
        if let Err(error) = transferred {
            // A failed transfer keeps the previous client and its unresolved
            // registry untouched, cancels its observation binding and marks the
            // owner unavailable so no later step can republish the earlier
            // authorization before a fresh authorization succeeds.
            self.awaiting_reauthorization = true;
            self.client.disconnect();
            self.last_snapshot = None;
            self.projection_marker = None;
            let _ = self.fence.close_visibility();
            return Err(error);
        }
        self.generation += 1;
        self.generation_cell
            .store(self.generation, Ordering::Release);
        self.publication_version = 0;
        self.projection_counter = 0;
        self.client = next;
        self.arm_control(handle);
        self.publish(true);
        Ok(())
    }

    /// Run one bounded owner step; returns true once the owner has shut down.
    pub fn step(&mut self, now_ms: u64) -> bool {
        self.drain_releases();
        if self.apply_shutdown() {
            return true;
        }
        let intent_changed = self.drain_mailbox(now_ms);
        if self.shutdown_applied {
            return true;
        }
        let report = self.client.pump_step(now_ms, PUMP_BUDGET);
        self.pump_saturated = report.frames >= PUMP_BUDGET;
        self.record_client_events(&report.events);
        self.deadline_ms = report.next_deadline_ms;
        // An asynchronous authorized answer resolves the diagnostic it provoked,
        // but only after that diagnostic has been published once.
        let fresh = std::mem::take(&mut self.diagnostic_fresh);
        if !fresh {
            // A diagnostic published earlier resolves from this step on.
            let _ = self.apply_read_resolution();
        }
        let diagnostic_changed = std::mem::take(&mut self.diagnostic_dirty);
        // A withheld record is not a projection change: only admitted projection
        // data moves the owner-local commit marker, while a read diagnostic must
        // still reach the publication.
        self.publish(intent_changed || report.changed || diagnostic_changed);
        // The answer may already have arrived inside this very step (a bounded
        // synchronous fixture or a fast Runtime): publish the resolved diagnostic
        // immediately instead of waiting for a wake that may never come.
        if fresh && self.apply_read_resolution() {
            self.publish(true);
            // The change just published is consumed: no unpublished diagnostic
            // change may remain to advance the version again on a later step.
            self.diagnostic_dirty = false;
            self.diagnostic_fresh = false;
        }
        false
    }

    fn record_client_events(&self, events: &[ClientEvent]) {
        if events.is_empty() {
            return;
        }
        let mut diagnostics = lock_diagnostics(&self.diagnostics);
        for event in events {
            let kind = match event {
                ClientEvent::StateChanged(state) => {
                    DiagnosticKind::Connection(connection_category(state))
                }
                ClientEvent::Error(error) => {
                    DiagnosticKind::Error(PublicationError::from_client(error))
                }
                ClientEvent::ResyncRequested { .. } => DiagnosticKind::Resync,
                _ => DiagnosticKind::Activity,
            };
            diagnostics.append(kind);
        }
    }

    /// Next deadline reported by the last bounded step, for the host timer.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.deadline_ms
    }

    /// Whether the last step consumed its whole pump budget.
    ///
    /// A saturated step may leave more frames queued in the transport while the
    /// client reports no deadline, so a host loop must step again instead of
    /// parking on a wake that no one will send.
    pub fn pump_saturated(&self) -> bool {
        self.pump_saturated
    }

    /// Mark the owner loop finished after its final step.
    pub fn finish(self, handle: &ObservationHandle) {
        self.fence.close_subscriptions();
        handle.owner_done.store(true, Ordering::Release);
    }

    fn begin_scope(&mut self, scope: ObservationScopeRequest) -> Result<(), CommandReadError> {
        let result = self.begin_authorized(scope);
        if result.is_ok() {
            self.publish(true);
        }
        result
    }

    fn begin_authorized(&mut self, scope: ObservationScopeRequest) -> Result<(), CommandReadError> {
        // A new binding replaces the authorization the previous diagnostic described.
        self.clear_read_diagnostic();
        self.fence.close_visibility();
        self.last_snapshot = None;
        self.projection_marker = None;
        let request = self.client.observation_request(scope.clone());
        let result = self
            .client
            .begin_authorized_observation(request)
            .map_err(|error| CommandReadError::Client(PublicationError::from_client(&error)));
        if result.is_ok() {
            self.scope = Some(scope);
            self.awaiting_reauthorization = false;
        }
        result
    }

    /// Drain queued intents; returns whether any intent changed observable state.
    fn drain_mailbox(&mut self, now_ms: u64) -> bool {
        let mut changed = false;
        loop {
            let command = {
                let mut mailbox = self.mailbox.lock().expect("mailbox lock");
                let command = mailbox.intents.pop_front();
                drop(mailbox);
                command
            };
            let Some(command) = command else {
                return changed;
            };
            if self.intent_changes_publication(&command) {
                changed = true;
            }
            if !self.intent_allowed(&command) {
                reject_stale(command);
                continue;
            }
            // Final acceptance and dispatch share the boundary with authorization
            // closes: a Stop is dispatched before the close or rejected by it.
            // Final acceptance and dispatch share the command boundary: a Stop is
            // therefore either dispatched before an authorization close or rejected
            // by it, never sent after the revocation. The boundary only wraps
            // bounded work because the transport contract requires `send` to
            // enqueue output without waiting on network or filesystem.
            let fence = Arc::clone(&self.fence);
            let gate = fence.enter_command_boundary();
            if !self.intent_allowed(&command) {
                drop(gate);
                reject_stale(command);
                continue;
            }
            self.handle_intent(command, now_ms);
            drop(gate);
            if self.shutdown_applied {
                return changed;
            }
        }
    }

    /// Whether an intent is still valid for this generation, authorization and
    /// visibility window.
    ///
    /// A write intent (Stop, Retain) is invalidated by any authorization change in
    /// its generation; a read additionally requires the same visibility window.
    fn intent_tag_valid(&self, tag: IntentTag, read_only: bool) -> bool {
        if tag.generation != self.generation {
            return false;
        }
        if tag.authorization_epoch != self.fence.authorization_epoch() {
            return false;
        }
        if read_only {
            return tag.visibility_sequence == self.fence.sequence();
        }
        true
    }

    fn intent_changes_publication(&self, command: &ObservationCommand) -> bool {
        matches!(
            command,
            ObservationCommand::ChangeScope { .. }
                | ObservationCommand::AuthenticationChanged { .. }
                | ObservationCommand::RequestStop { .. }
                | ObservationCommand::Retain { .. }
        )
    }

    /// Clear the read diagnostic once the condition it described no longer holds.
    fn clear_read_diagnostic(&mut self) {
        if self.last_read_error.take().is_some() {
            self.diagnostic_dirty = true;
        }
    }

    /// Apply the outstanding read resolution; true when the diagnostic changed.
    fn apply_read_resolution(&mut self) -> bool {
        let Some((id, _)) = self.last_read_error.clone() else {
            return false;
        };
        let before = self.last_read_error.clone();
        match self.read_diagnostic_resolution(&id) {
            // The read is answered: the diagnostic no longer describes it.
            Some(ReadResolution::Resolved) => self.clear_read_diagnostic(),
            // The read is refused for this identity: the diagnostic becomes the
            // authorization failure it now reports.
            Some(ReadResolution::Denied) => {
                self.record_read_diagnostic(&id, &CommandReadError::AuthorizationDenied)
            }
            None => {}
        }
        self.last_read_error != before
    }

    /// How the client's own authorized answer resolved an outstanding read
    /// diagnostic, without a further read or a new authorization.
    fn read_diagnostic_resolution(&self, id: &CommandId) -> Option<ReadResolution> {
        let cos_client::PublicationAccess::Validated {
            stamp,
            read_evidence,
            ..
        } = self.client.publication_access()
        else {
            return None;
        };
        // A recorded receipt turns into per-record permission: it resolves the read.
        if let cos_client::CommandLookup::Found(record) = self.client.lookup_command(id) {
            match record.permission(stamp) {
                Some(true) => return Some(ReadResolution::Resolved),
                Some(false) => return Some(ReadResolution::Denied),
                None => {}
            }
        }
        match read_evidence.get(id) {
            Some((_, cos_client::ReadEvidence::AllowedNoReceipt(held)))
                if held.as_ref() == stamp =>
            {
                Some(ReadResolution::Resolved)
            }
            Some((_, cos_client::ReadEvidence::Denied(held))) if held.as_ref() == stamp => {
                Some(ReadResolution::Denied)
            }
            _ => None,
        }
    }

    /// Record the safe diagnostic of a refused read without disclosing data.
    fn record_read_diagnostic(&mut self, id: &CommandId, error: &CommandReadError) {
        let diagnostic = match error {
            CommandReadError::AuthorizationDenied => PublicationError::Unauthorized,
            CommandReadError::AuthorizationUnavailable => PublicationError::VerificationPending,
            CommandReadError::Superseded
            | CommandReadError::CapacityExceeded
            | CommandReadError::Client(_) => return,
        };
        // A confirmed diagnostic is never re-recorded: only an actual change may
        // mark the publication dirty.
        if self.last_read_error.as_ref() == Some(&(id.clone(), diagnostic.clone())) {
            return;
        }
        self.last_read_error = Some((id.clone(), diagnostic));
        self.diagnostic_dirty = true;
        self.diagnostic_fresh = true;
    }

    /// Reject a stale Stop/read/retain before dispatch, never redirecting it.
    fn intent_allowed(&self, command: &ObservationCommand) -> bool {
        match command {
            ObservationCommand::RequestStop { tag, .. } => self.intent_tag_valid(*tag, false),
            ObservationCommand::ReadCommand { tag, .. } => self.intent_tag_valid(*tag, true),
            ObservationCommand::Retain { tag, .. } => self.intent_tag_valid(*tag, false),
            ObservationCommand::ChangeScope { .. }
            | ObservationCommand::AuthenticationChanged { .. } => true,
        }
    }

    fn handle_intent(&mut self, command: ObservationCommand, now_ms: u64) {
        match command {
            ObservationCommand::ChangeScope { scope, reply } => {
                let result = self.begin_scope(scope);
                reply.resolve(result);
                let _ = now_ms;
            }
            ObservationCommand::AuthenticationChanged { reply } => {
                let result = match self.scope.clone() {
                    Some(scope) => self.begin_scope(scope),
                    None => Err(CommandReadError::AuthorizationUnavailable),
                };
                reply.resolve(result);
            }
            ObservationCommand::RequestStop { task, reply, .. } => {
                let result = self.client.stop_task(&task).map_err(|error| {
                    CommandReadError::Client(PublicationError::from_client(&error))
                });
                self.publish(true);
                reply.resolve(result);
            }
            ObservationCommand::ReadCommand { id, reply, .. } => {
                reply.resolve(self.read_command(&id));
            }
            ObservationCommand::Retain { id, reply, .. } => {
                let result = self.client.retain_command(&id).map_err(|error| {
                    CommandReadError::Client(PublicationError::from_client(&error))
                });
                if result.is_ok() {
                    // Credit the granted reference: only a real reference may be
                    // released later, and the pending queue stays bounded by it.
                    let mut ledger = self.references.lock().expect("reference ledger");
                    let entry = ledger.credits.entry(id.clone()).or_insert(0);
                    *entry = entry.saturating_add(1);
                }
                reply.resolve(result);
            }
        }
    }

    /// Apply a pending shutdown request; returns true once disposal is complete.
    fn apply_shutdown(&mut self) -> bool {
        let requested = {
            let completion = self.shutdown.lock().expect("shutdown lock");
            completion.requested && !completion.done
        };
        if !requested {
            return false;
        }
        self.shutdown_applied = true;
        self.fence.close_visibility();
        self.fence.close_subscriptions();
        {
            let mut mailbox = self.mailbox.lock().expect("mailbox lock");
            mailbox.closed = true;
            while let Some(pending) = mailbox.intents.pop_front() {
                reject(pending);
            }
        }
        // Dispose client state exactly once before completing the request.
        self.client = Box::new(ClosedClient);
        {
            let mut completion = self.shutdown.lock().expect("shutdown lock");
            completion.done = true;
            for waker in completion.wakers.drain(..) {
                waker.wake();
            }
        }
        true
    }

    /// Drain pending surface releases; a deduplicating set cannot be dropped.
    fn drain_releases(&mut self) {
        let mut ledger = self.references.lock().expect("reference ledger");
        let pending = std::mem::take(&mut ledger.pending);
        for (id, count) in pending {
            for _ in 0..count {
                self.client.release_command(&id);
            }
        }
    }

    /// Read one command under the current authorization.
    ///
    /// An identity with no current evidence triggers the same coalesced
    /// authorized read path the owner uses elsewhere and answers with no data for
    /// this call; the later reply then decides the next read.
    fn read_command(&mut self, id: &CommandId) -> Result<AuthorizedCommandRead, CommandReadError> {
        enum Answer {
            Value(Box<CommandRead>),
            Lookup(Box<ReadStamp>),
            Fail(CommandReadError),
        }
        let answer = {
            let mut access = self.client.publication_access();
            match &mut access {
                PublicationAccess::Withheld { reason } => Answer::Fail(match reason {
                    cos_client::ReadUnavailable::Superseded => CommandReadError::Superseded,
                    cos_client::ReadUnavailable::Exhausted => CommandReadError::CapacityExceeded,
                    _ => CommandReadError::AuthorizationUnavailable,
                }),
                PublicationAccess::Validated {
                    stamp,
                    readable_commands,
                    read_evidence,
                    ..
                } => {
                    let stamp = Arc::new((*stamp).clone());
                    let mut found = None;
                    for (candidate, record) in readable_commands.by_ref() {
                        if candidate == id {
                            found = Some(CommandRead::Found(CommandReadView {
                                presentation: presentation_of(record),
                                receipt: record.state().receipt().cloned(),
                                issue: issue_of(record),
                            }));
                            break;
                        }
                    }
                    match found {
                        Some(view) => Answer::Value(Box::new(view)),
                        None => match self.client.lookup_command(id) {
                            CommandLookup::Found(record) => match record.permission(stamp.as_ref())
                            {
                                Some(false) => Answer::Fail(CommandReadError::AuthorizationDenied),
                                Some(true) => {
                                    Answer::Fail(CommandReadError::AuthorizationUnavailable)
                                }
                                None => Answer::Lookup(Box::new(stamp.as_ref().clone())),
                            },
                            CommandLookup::NotRetained => {
                                match read_evidence.get(id).map(|(_, evidence)| evidence) {
                                    Some(cos_client::ReadEvidence::AllowedNoReceipt(held))
                                        if held.as_ref() == stamp.as_ref() =>
                                    {
                                        Answer::Value(Box::new(CommandRead::NotRetained))
                                    }
                                    Some(cos_client::ReadEvidence::Denied(held))
                                        if held.as_ref() == stamp.as_ref() =>
                                    {
                                        Answer::Fail(CommandReadError::AuthorizationDenied)
                                    }
                                    _ => Answer::Lookup(Box::new(stamp.as_ref().clone())),
                                }
                            }
                        },
                    }
                }
            }
        };
        let view = match answer {
            Answer::Fail(error) => {
                if matches!(error, CommandReadError::Superseded) {
                    // The refusal belongs to a binding that no longer exists.
                    self.clear_read_diagnostic();
                } else {
                    // A refused read intent is a safe diagnostic, never a silent drop.
                    self.record_read_diagnostic(id, &error);
                }
                return Err(error);
            }
            Answer::Lookup(stamp) => {
                let request = desktop_protocol::AuthorizedReceiptLookup {
                    lookup_id: cos_client::next_receipt_read_id(),
                    stamp: *stamp,
                    command_id: id.clone(),
                };
                // The lookup itself is the read intent: whether the transport
                // accepts or refuses it, the read is unanswered, and that fact is
                // a safe published diagnostic, never a silent drop.
                match self.client.lookup_receipt_authorized(request) {
                    Ok(()) => {
                        self.last_read_error =
                            Some((id.clone(), PublicationError::VerificationPending));
                        self.diagnostic_dirty = true;
                        self.diagnostic_fresh = true;
                        return Err(CommandReadError::AuthorizationUnavailable);
                    }
                    Err(error) => {
                        // The safe published diagnostic keeps the client's own
                        // classification, and the caller receives it too: a bounded
                        // capacity refusal is never reported as a pending read.
                        let diagnostic = PublicationError::from_client(&error);
                        self.last_read_error = Some((id.clone(), diagnostic.clone()));
                        self.diagnostic_dirty = true;
                        self.diagnostic_fresh = true;
                        let refusal = match (&error, &diagnostic) {
                            (
                                ClientError::InvalidCommand(message),
                                PublicationError::InvalidCommand(
                                    InvalidCommandReason::CapacityExceeded,
                                ),
                            ) if message == "CommandCapacityExceeded" => {
                                CommandReadError::CapacityExceeded
                            }
                            (_, diagnostic) => CommandReadError::Client(diagnostic.clone()),
                        };
                        return Err(refusal);
                    }
                }
            }
            Answer::Value(view) => *view,
        };
        let lease = self.fence.current().map_err(|reason| {
            let error = match reason {
                ReadUnavailable::Superseded => CommandReadError::Superseded,
                ReadUnavailable::Exhausted => CommandReadError::CapacityExceeded,
                _ => CommandReadError::AuthorizationUnavailable,
            };
            if matches!(error, CommandReadError::AuthorizationUnavailable) {
                self.last_read_error = Some((id.clone(), PublicationError::VerificationPending));
                self.diagnostic_dirty = true;
                self.diagnostic_fresh = true;
            }
            error
        })?;
        let tagged = lease
            .read(|_| view)
            .map_err(|_| CommandReadError::Superseded)?;
        // The read was answered: the earlier refusal no longer describes it.
        self.clear_read_diagnostic();
        Ok(AuthorizedCommandRead::new(tagged))
    }

    /// Assemble and install one complete publication.
    fn publish(&mut self, changed: bool) {
        if !changed && self.fence.is_open() {
            return;
        }
        // A failed replacement keeps the owner unavailable until a fresh
        // authorization: no validated publication may be assembled meanwhile.
        if self.awaiting_reauthorization {
            let connection = connection_category(self.client.connection());
            let source = source_category(self.client.source());
            let last_error = self.client.last_error().map(PublicationError::from_client);
            self.publication_version += 1;
            let withheld = ObservationPublication {
                snapshot: None,
                revision: None,
                connection,
                source,
                stale: true,
                last_error,
                command_statuses: BTreeMap::new(),
                session_generation: self.generation,
                publication_version: self.publication_version,
            };
            let _ = self.fence.close_for_commit();
            let guard = self
                .fence
                .install(withheld, self.fence.authorization_epoch());
            if self.fence.open(guard).is_ok() {
                self.fence.notify_subscribers();
            }
            return;
        }
        // Authorization generation used for assembly: a scope change or detected
        // invalidation during assembly must never be undone by this publication.
        let epoch_before = self.fence.authorization_epoch();
        let connection = connection_category(self.client.connection());
        let source = source_category(self.client.source());
        let stale = self.client.is_stale();
        let last_error = self
            .client
            .last_error()
            .map(PublicationError::from_client)
            .or_else(|| {
                self.last_read_error
                    .as_ref()
                    .map(|(_, error)| error.clone())
            });
        let withheld_error = last_error.clone();
        let current_snapshot = self.client.snapshot();
        let mut access = self.client.publication_access();
        let (snapshot, revision, command_statuses) = match &mut access {
            PublicationAccess::Validated {
                readable_commands, ..
            } => {
                let mut statuses = BTreeMap::new();
                for (id, record) in readable_commands.by_ref() {
                    statuses.insert(
                        id.clone(),
                        CommandReadView {
                            presentation: presentation_of(record),
                            receipt: record.state().receipt().cloned(),
                            issue: issue_of(record),
                        },
                    );
                }
                let moved = match &self.last_snapshot {
                    Some(previous) => !Arc::ptr_eq(previous, &current_snapshot),
                    None => true,
                };
                if moved {
                    self.projection_counter += 1;
                    self.projection_marker = Some(Counter::from_u128(self.projection_counter));
                }
                self.last_snapshot = Some(Arc::clone(&current_snapshot));
                (
                    Some(Arc::clone(&current_snapshot)),
                    self.projection_marker.clone(),
                    statuses,
                )
            }
            PublicationAccess::Withheld { .. } => {
                // A withheld publication exposes no projection marker.
                self.projection_marker = None;
                self.last_snapshot = None;
                (None, None, BTreeMap::new())
            }
        };
        self.publication_version += 1;
        let publication = ObservationPublication {
            snapshot,
            revision,
            connection,
            source,
            stale,
            last_error,
            command_statuses,
            session_generation: self.generation,
            publication_version: self.publication_version,
        };
        if self.fence.authorization_epoch() != epoch_before {
            // The authorization changed while this publication was assembled: publish
            // the withheld form instead of reopening with stale authorized data.
            self.publication_version += 1;
            let withheld = ObservationPublication {
                snapshot: None,
                revision: None,
                connection,
                source,
                stale: true,
                last_error: withheld_error,
                command_statuses: BTreeMap::new(),
                session_generation: self.generation,
                publication_version: self.publication_version,
            };
            let _ = self.fence.close_for_commit();
            let guard = self.fence.install(withheld, epoch_before);
            if self.fence.open(guard).is_ok() {
                self.fence.notify_subscribers();
            }
            return;
        }
        let _ = self.fence.close_for_commit();
        let guard = self.fence.install(publication, epoch_before);
        if self.fence.open(guard).is_ok() {
            self.fence.notify_subscribers();
        }
    }
}

fn lock_diagnostics(diagnostics: &Mutex<DiagnosticRing>) -> MutexGuard<'_, DiagnosticRing> {
    match diagnostics.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn close_closure(handle: &ObservationHandle) -> Arc<dyn Fn() -> u64 + Send + Sync> {
    let fence = Arc::clone(&handle.fence);
    // The adapter's synchronous close crosses the same boundary as command
    // dispatch, so an invalidation cannot slip between validation and dispatch.
    Arc::new(move || {
        let _gate = fence.enter_command_boundary();
        fence.close_visibility()
    })
}

fn connection_category(state: &ConnectionState) -> PublicationConnection {
    match state {
        ConnectionState::Disconnected => PublicationConnection::Disconnected,
        ConnectionState::Negotiating | ConnectionState::Synchronizing => {
            PublicationConnection::Connecting
        }
        ConnectionState::Live => PublicationConnection::Observing,
        ConnectionState::Reconnecting { .. } => PublicationConnection::Recovering,
        ConnectionState::Incompatible { .. } => PublicationConnection::Incompatible,
        ConnectionState::Unauthorized { .. } => PublicationConnection::Unauthorized,
    }
}

fn source_category(source: ClientSource) -> PublicationSource {
    match source {
        ClientSource::Runtime { .. } => PublicationSource::Runtime,
        ClientSource::Fixture { .. } => PublicationSource::Fixture,
        ClientSource::Unknown => PublicationSource::Unavailable,
    }
}

fn presentation_of(record: &CommandStatus) -> CommandPresentation {
    match record.state() {
        cos_client::CommandState::Prepared => CommandPresentation::Prepared,
        cos_client::CommandState::Pending { .. } => CommandPresentation::AwaitingResponse,
        cos_client::CommandState::Accepted(_) | cos_client::CommandState::Rejected(_) => {
            CommandPresentation::ReceiptAvailable
        }
        cos_client::CommandState::CanonicalReflected(_) => {
            CommandPresentation::CanonicalEffectObserved
        }
        cos_client::CommandState::ResultUnknown { .. } => CommandPresentation::OutcomeUnconfirmed,
        cos_client::CommandState::Unsupported { .. } => CommandPresentation::Unsupported,
        cos_client::CommandState::Failed { .. } => CommandPresentation::Failed,
    }
}

fn issue_of(record: &CommandStatus) -> Option<PublicationError> {
    match record.state() {
        cos_client::CommandState::Failed { error }
        | cos_client::CommandState::Unsupported { error } => {
            Some(PublicationError::Server(error.code.clone()))
        }
        _ => None,
    }
}

/// Bounded timer that only wakes the owner at its next deadline.
///
/// It performs no client work and is cancelled and joined on shutdown.
pub struct DeadlineScheduler {
    deadline: Arc<Mutex<Option<Instant>>>,
    shutdown: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl DeadlineScheduler {
    /// Start the waker thread for one owner handle.
    pub fn start(handle: &ObservationHandle) -> Self {
        let deadline: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let shutdown = Arc::new(AtomicBool::new(false));
        let slot = Arc::clone(&deadline);
        let stop = Arc::clone(&shutdown);
        let wake = Arc::clone(&handle.wake);
        let join = std::thread::Builder::new()
            .name("cos-observation-timer".to_owned())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let next = *slot.lock().expect("deadline lock");
                    match next {
                        Some(at) => {
                            let now = Instant::now();
                            if now >= at {
                                *slot.lock().expect("deadline lock") = None;
                                wake.notify();
                            } else {
                                let remaining = at - now;
                                std::thread::sleep(remaining.min(Duration::from_millis(25)));
                            }
                        }
                        None => std::thread::sleep(Duration::from_millis(25)),
                    }
                }
            })
            .ok();
        Self {
            deadline,
            shutdown,
            join,
        }
    }

    /// Arm the next wake-up from an owner-reported deadline.
    pub fn arm(&self, deadline_ms: Option<u64>, now_ms: u64) {
        let instant = deadline_ms.map(|deadline| {
            let delta = deadline.saturating_sub(now_ms);
            Instant::now() + Duration::from_millis(delta)
        });
        *self.deadline.lock().expect("deadline lock") = instant;
    }

    /// Cancel and join the waker thread.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for DeadlineScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Bounded exit watcher. A missed deadline or unavailable timer is unconfirmed,
/// even if a driver completion is observed afterward.
pub struct ExitWatch {
    done: Arc<AtomicBool>,
    timed_out: Arc<AtomicBool>,
    deadline: Instant,
    waker: Arc<Mutex<Option<Waker>>>,
    // Dropping the sender wakes a timer whose waiter has already completed.
    _cancel: Option<std::sync::mpsc::SyncSender<()>>,
    _timer: Option<std::thread::JoinHandle<()>>,
}

impl ExitWatch {
    /// Wait against an existing absolute deadline, capped at 5s from now.
    /// Constructing or polling this watcher never extends a caller's deadline.
    pub fn new(done: Arc<AtomicBool>, waker: Arc<Mutex<Option<Waker>>>, deadline: Instant) -> Self {
        Self::with_timer(done, waker, deadline, |deadline, flag, slot, cancel| {
            std::thread::Builder::new()
                .name("cos-exit-watch".to_owned())
                .spawn(move || {
                    Self::finish_timer(deadline, flag, slot, |remaining| {
                        cancel.recv_timeout(remaining)
                    });
                })
        })
    }

    fn finish_timer(
        deadline: Instant,
        flag: Arc<AtomicBool>,
        slot: Arc<Mutex<Option<Waker>>>,
        wait: impl FnOnce(Duration) -> Result<(), std::sync::mpsc::RecvTimeoutError>,
    ) {
        // Thread scheduling consumes the same budget as the caller's wait.
        // Never restart a relative timeout after this worker finally starts.
        let remaining = deadline.saturating_duration_since(Instant::now());
        if matches!(
            wait(remaining),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            flag.store(true, Ordering::Release);
            let mut slot = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(waker) = slot.take() {
                waker.wake();
            }
        }
    }

    // The private OS-thread boundary permits deterministic failure coverage
    // without exhausting the process's thread resources or adding a public API.
    fn with_timer(
        done: Arc<AtomicBool>,
        waker: Arc<Mutex<Option<Waker>>>,
        deadline: Instant,
        spawn: impl FnOnce(
            Instant,
            Arc<AtomicBool>,
            Arc<Mutex<Option<Waker>>>,
            std::sync::mpsc::Receiver<()>,
        ) -> std::io::Result<std::thread::JoinHandle<()>>,
    ) -> Self {
        let now = Instant::now();
        let deadline = deadline.min(now + Duration::from_secs(5));
        let expired = deadline <= now;
        let timed_out = Arc::new(AtomicBool::new(expired));
        let (cancel, timer) = if expired {
            (None, None)
        } else {
            let (sender, receiver) = std::sync::mpsc::sync_channel(0);
            match spawn(
                deadline,
                Arc::clone(&timed_out),
                Arc::clone(&waker),
                receiver,
            ) {
                Ok(timer) => (Some(sender), Some(timer)),
                Err(_) => {
                    timed_out.store(true, Ordering::Release);
                    (None, None)
                }
            }
        };
        Self {
            done,
            timed_out,
            deadline,
            waker,
            _cancel: cancel,
            _timer: timer,
        }
    }

    /// The waker slot the driver side wakes on completion.
    pub fn waker_slot(&self) -> Arc<Mutex<Option<Waker>>> {
        Arc::clone(&self.waker)
    }

    /// Wait for completion before the deadline; `false` means unconfirmed.
    pub async fn wait(self) -> bool {
        std::future::poll_fn(|cx| self.poll_outcome(cx)).await
    }

    fn outcome(&self) -> Option<bool> {
        if self.timed_out.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            return Some(false);
        }
        self.done
            .load(Ordering::Acquire)
            .then(|| !self.timed_out.load(Ordering::Acquire) && Instant::now() < self.deadline)
    }

    fn poll_outcome(&self, cx: &mut Context<'_>) -> Poll<bool> {
        if let Some(outcome) = self.outcome() {
            return Poll::Ready(outcome);
        }
        match self.waker.lock() {
            Ok(mut slot) => *slot = Some(cx.waker().clone()),
            Err(_) => return Poll::Ready(false),
        }
        self.outcome().map_or(Poll::Pending, Poll::Ready)
    }
}

#[cfg(test)]
mod exit_watch_failures {
    use super::*;

    #[test]
    fn timer_creation_failure_never_waits_or_confirms_later_completion() {
        let done = Arc::new(AtomicBool::new(false));
        let watch = ExitWatch::with_timer(
            Arc::clone(&done),
            Arc::new(Mutex::new(None)),
            Instant::now() + Duration::from_secs(500),
            |_, _, _, _| Err(std::io::Error::other("synthetic OS thread-start failure")),
        );
        done.store(true, Ordering::Release);
        let mut future = std::pin::pin!(watch.wait());
        assert_eq!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(false)
        );
    }

    #[test]
    fn delayed_timer_start_spends_no_new_budget_and_wakes_the_waiter() {
        struct WakeCount(std::sync::atomic::AtomicUsize);
        impl std::task::Wake for WakeCount {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::Release);
            }
        }
        let wake = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
        let slot = Arc::new(Mutex::new(Some(Waker::from(Arc::clone(&wake)))));
        let timed_out = Arc::new(AtomicBool::new(false));
        // The worker starts after the absolute deadline. The OS-wait boundary
        // is injected so scheduling speed cannot make this regression flaky.
        ExitWatch::finish_timer(
            Instant::now() - Duration::from_secs(1),
            Arc::clone(&timed_out),
            Arc::clone(&slot),
            |remaining| {
                assert_eq!(remaining, Duration::ZERO);
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            },
        );
        assert!(timed_out.load(Ordering::Acquire));
        assert_eq!(wake.0.load(Ordering::Acquire), 1);
        assert!(slot.lock().expect("test waker lock").is_none());
    }
}

/// Owner-local monotonically increasing millisecond clock.
pub struct OwnerClock {
    start: Instant,
}

impl Default for OwnerClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl OwnerClock {
    /// Milliseconds since the clock started.
    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// Wait until the owner is woken by its mailbox, adapter or timer.
struct WakeWait {
    slot: Arc<WakeSlot>,
}

impl Future for WakeWait {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.slot.poll_cleared(cx) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Drive one owner on the host's background executor until shutdown.
///
/// The host supplies the executor spawn and keeps the returned task alive; the
/// owner performs every client operation off the main thread.
pub async fn drive(
    mut owner: ObservationSessionOwner,
    handle: ObservationHandle,
    clock: OwnerClock,
    mut timer: DeadlineScheduler,
) {
    loop {
        let now_ms = clock.now_ms();
        if owner.step(now_ms) {
            break;
        }
        timer.arm(owner.next_deadline_ms(), clock.now_ms());
        // A step that consumed its whole frame budget may still have queued
        // frames the transport will never announce again (a fixture with more
        // chunks than one budget reports no deadline), so continue immediately
        // rather than parking on a wake that cannot arrive.
        if owner.pump_saturated() {
            continue;
        }
        WakeWait {
            slot: handle.wake_slot(),
        }
        .await;
    }
    owner.finish(&handle);
    timer.shutdown();
}

/// Registry of test/embedding-only clients that never disclose anything.
pub static EMPTY_REGISTRY: LazyLock<BTreeMap<CommandId, CommandStatus>> =
    LazyLock::new(BTreeMap::new);

/// A client placeholder for a failed transfer; never usable.
#[derive(Default)]
pub struct ClosedClient;

impl DesktopClient for ClosedClient {
    fn snapshot(&self) -> SnapshotHandle {
        Arc::new(cos_client::ProjectionState::default())
    }
    fn connection(&self) -> &ConnectionState {
        &ConnectionState::Disconnected
    }
    fn is_stale(&self) -> bool {
        true
    }
    fn source(&self) -> ClientSource {
        ClientSource::Unknown
    }
    fn last_error(&self) -> Option<&ClientError> {
        None
    }
    fn disconnect(&mut self) -> Vec<ClientEvent> {
        Vec::new()
    }
    fn request_resync(&mut self, _reason: ResyncReason) -> Vec<ClientEvent> {
        Vec::new()
    }
    fn pump(&mut self, _now_ms: u64) -> Vec<ClientEvent> {
        Vec::new()
    }
    fn stop_task(&mut self, _task_id: &TaskId) -> Result<CommandId, ClientError> {
        Err(ClientError::NotConnected("owner is unavailable".to_owned()))
    }
    fn bind_observation_control(&mut self, _control: ObservationControl) {}
    fn pump_step(&mut self, _now_ms: u64, _budget: usize) -> cos_client::PumpReport {
        cos_client::PumpReport {
            frames: 0,
            events: Vec::new(),
            changed: false,
            next_deadline_ms: None,
        }
    }
    fn command_statuses(&self) -> &BTreeMap<CommandId, CommandStatus> {
        &EMPTY_REGISTRY
    }
    fn lookup_command(&self, _id: &CommandId) -> CommandLookup<'_> {
        CommandLookup::NotRetained
    }
    fn retain_command(&mut self, _id: &CommandId) -> Result<(), ClientError> {
        Err(ClientError::NotConnected("owner is unavailable".to_owned()))
    }
    fn release_command(&mut self, _id: &CommandId) {}
    fn begin_authorized_observation(
        &mut self,
        _request: desktop_protocol::OpenAuthorizedObservation,
    ) -> Result<(), ClientError> {
        Err(ClientError::NotConnected("owner is unavailable".to_owned()))
    }
    fn publication_access(&self) -> PublicationAccess<'_> {
        PublicationAccess::Withheld {
            reason: cos_client::ReadUnavailable::Closed,
        }
    }
    fn lookup_receipt_authorized(
        &mut self,
        _request: desktop_protocol::AuthorizedReceiptLookup,
    ) -> Result<(), ClientError> {
        Err(ClientError::NotConnected("owner is unavailable".to_owned()))
    }
    fn observation_request(
        &self,
        scope: ObservationScopeRequest,
    ) -> desktop_protocol::OpenAuthorizedObservation {
        desktop_protocol::OpenAuthorizedObservation {
            request_id: cos_client::next_read_request_id(),
            observation: desktop_protocol::OpenObservation {
                scope,
                scope_id: None,
                last_seen_cursor: None,
                epoch: None,
            },
            retained_commands: Vec::new(),
        }
    }
    fn adopt_retained_registry(
        &mut self,
        _records: BTreeMap<CommandId, CommandStatus>,
        _references: BTreeMap<CommandId, usize>,
    ) -> Result<(), ClientError> {
        Err(ClientError::NotConnected("owner is unavailable".to_owned()))
    }
}

/// A18: the owner wake slot must never lose a notification that races the
/// consumer's suspension (register-then-recheck), and repeated notifications
/// coalesce into a single observed wake.
#[cfg(test)]
mod wake_slot_tests {
    use super::{WakeSlot, wake_waker};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Wake, Waker};

    /// A waker that counts how often it is woken by the slot.
    struct CountingWaker(Arc<AtomicUsize>);

    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        (
            Waker::from(Arc::new(CountingWaker(Arc::clone(&count)))),
            count,
        )
    }

    #[test]
    fn a_notification_pending_at_suspension_is_observed_by_the_same_poll() {
        let slot = WakeSlot::default();
        let (waker, count) = counting_waker();
        let mut cx = Context::from_waker(&waker);
        slot.notify();
        assert!(
            slot.poll_cleared(&mut cx),
            "a notification already pending is observed immediately"
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "no waker needed to be registered to observe it"
        );
    }

    #[test]
    fn a_notification_arriving_while_parked_is_never_lost() {
        let slot = WakeSlot::default();
        let (waker, count) = counting_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(
            !slot.poll_cleared(&mut cx),
            "the poll parks and registers its waker"
        );
        slot.notify();
        assert_eq!(count.load(Ordering::SeqCst), 1, "the parked waker is woken");
        assert!(
            slot.poll_cleared(&mut cx),
            "the notification is observed when the consumer resumes"
        );
        assert!(
            !slot.poll_cleared(&mut cx),
            "each notification is consumed exactly once"
        );
    }

    #[test]
    fn repeated_notifications_coalesce_into_one_ready_poll() {
        let slot = WakeSlot::default();
        let (waker, _count) = counting_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(!slot.poll_cleared(&mut cx));
        slot.notify();
        slot.notify();
        slot.notify();
        assert!(
            slot.poll_cleared(&mut cx),
            "coalesced notifications are observed once"
        );
        assert!(!slot.poll_cleared(&mut cx));
    }

    #[test]
    fn the_owner_waker_raises_the_pending_flag() {
        let slot = Arc::new(WakeSlot::default());
        let (probe, _count) = counting_waker();
        let mut cx = Context::from_waker(&probe);
        assert!(!slot.poll_cleared(&mut cx));
        wake_waker(Arc::clone(&slot)).wake_by_ref();
        assert!(
            slot.notified.load(Ordering::Acquire),
            "the owner waker raises the pending flag"
        );
        assert!(slot.poll_cleared(&mut cx));
    }

    #[test]
    fn a_notification_racing_suspension_is_never_lost() {
        // The notifier runs concurrently with the consumer's poll, so the
        // notification may land before registration, inside the registration
        // window, or while parked. Every interleaving must be observed without
        // any further notification.
        for _ in 0..64 {
            let slot = Arc::new(WakeSlot::default());
            let (waker, _count) = counting_waker();
            let mut cx = Context::from_waker(&waker);
            let notifier = {
                let slot = Arc::clone(&slot);
                std::thread::spawn(move || slot.notify())
            };
            let mut observed = false;
            for _ in 0..64 {
                if slot.poll_cleared(&mut cx) {
                    observed = true;
                    break;
                }
                std::thread::yield_now();
            }
            notifier.join().expect("the notifier joins");
            if !observed {
                assert!(
                    slot.poll_cleared(&mut cx),
                    "a notification racing suspension registration is never lost"
                );
            }
        }
    }
}

#[cfg(test)]
mod sec2_safe_categories {
    use super::{InvalidCommandReason, PublicationError};
    use cos_client::ClientError;
    use desktop_protocol::{ErrorCode, ProtocolError};

    const CREDENTIAL: &str = "SEC2-SYNTHETIC-CREDENTIAL-7f3a9c";
    const SIGNED_LOCATOR: &str =
        "https://media.invalid/object?X-Amz-Signature=SEC2-SYNTHETIC-SIGNATURE";
    const PRIVATE_PATH: &str = r"C:\Users\operator\private\SEC2-SYNTHETIC-PATH.png";

    #[test]
    fn structured_server_text_never_enters_a_safe_publication_category() {
        let error = ClientError::Server(Box::new(ProtocolError {
            code: ErrorCode::Forbidden,
            message: format!("{CREDENTIAL} {SIGNED_LOCATOR} {PRIVATE_PATH}"),
            correlation_id: None,
            command_id: None,
            scope_id: None,
            retry_after_ms: None,
            field_errors: Vec::new(),
            current_revision: None,
            resync_reason: None,
        }));

        let safe = PublicationError::from_client(&error);
        assert_eq!(safe, PublicationError::Server(ErrorCode::Forbidden));
        let rendered = format!("{safe:?}");
        for secret in [
            "SEC2-SYNTHETIC-CREDENTIAL",
            "X-Amz-Signature",
            "SEC2-SYNTHETIC-PATH",
        ] {
            assert!(
                !rendered.contains(secret),
                "a safe publication category must not carry server free text: {rendered}"
            );
        }
    }

    #[test]
    fn local_rejection_free_text_reduces_to_a_typed_reason() {
        let safe = PublicationError::from_client(&ClientError::InvalidCommand(format!(
            "{CREDENTIAL} {SIGNED_LOCATOR} {PRIVATE_PATH}"
        )));
        assert_eq!(
            safe,
            PublicationError::InvalidCommand(InvalidCommandReason::Other)
        );
        let rendered = format!("{safe:?}");
        for secret in [
            "SEC2-SYNTHETIC-CREDENTIAL",
            "X-Amz-Signature",
            "SEC2-SYNTHETIC-PATH",
        ] {
            assert!(!rendered.contains(secret));
        }
    }
}
