//! Close-only visibility fence, atomic publication records and tagged reads.
//!
//! One process-wide fence owns the sequence that every publication and every
//! borrowed read is tagged with. Closing is immediate and idempotent; reopening
//! is a compare-and-swap from the exact expected closed sequence to its next
//! even tag, and only after a complete validated candidate was installed while
//! closed. Sequences are never reused and exhaustion leaves the fence closed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, TryLockError};
use std::task::{Context, Poll, Waker};

use crate::observation::{ObservationPublication, PublicationError, ReadUnavailable};

/// Bounded number of registered publication subscribers.
pub const MAX_SUBSCRIBERS: usize = 8;

struct Record {
    tag: u64,
    /// Unique installation token, so a guard can only ever drop its own candidate.
    installation: u64,
    /// Authorization epoch the candidate was assembled under.
    authorization_epoch: u64,
    publication: Arc<ObservationPublication>,
}

#[derive(Default)]
struct Subscribers {
    wakers: Vec<Waker>,
    closed: bool,
}

#[derive(Default)]
struct GateState {
    owner: Option<std::thread::ThreadId>,
    depth: usize,
}

/// Reentrant guard for the authorization/dispatch boundary.
///
/// The guard is intentionally `!Send`: it may only be released by the thread that
/// acquired it, so a guard can never keep the gate held from another thread.
pub struct CommandGateGuard<'a> {
    fence: &'a VisibilityFence,
    thread: std::thread::ThreadId,
    _thread_bound: std::marker::PhantomData<*const ()>,
}

impl Drop for CommandGateGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.fence.command_gate.lock().expect("command gate");
        if state.owner != Some(self.thread) {
            return;
        }
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0 {
            state.owner = None;
        }
    }
}

/// One complete immutable publication plus its fence tag.
pub struct VisibilityFence {
    state: RwLock<Option<Arc<Record>>>,
    installing: Mutex<Option<Arc<Record>>>,
    sequence: AtomicU64,
    closed: AtomicBool,
    exhausted: AtomicBool,
    /// Monotonic token source for installations.
    installations: AtomicU64,
    /// Serializes authorization closes against command-intent dispatch, so a
    /// Stop is either dispatched before the close or rejected by it.
    command_gate: Mutex<GateState>,
    /// Advances on every transition to closed: an authorization generation the
    /// publication path can validate against, distinct from the fence sequence.
    authorization_epoch: AtomicU64,
    subscribers: Mutex<Subscribers>,
}

/// Proof that the fence was closed while a candidate was installed.
///
/// Not `Copy`: the installation slot stays reserved until this value is
/// consumed by a reopening CAS or dropped. A dropped guard discards the
/// candidate, so a later reopen can never expose a stale record.
pub struct ClosedFence {
    fence: Option<Arc<VisibilityFence>>,
    /// Unique token of the candidate this guard owns.
    installation: u64,
    expected_open: u64,
    closed: bool,
}

impl ClosedFence {
    fn consumed() -> Self {
        Self {
            fence: None,
            installation: 0,
            expected_open: 0,
            closed: false,
        }
    }
}

impl Drop for ClosedFence {
    fn drop(&mut self) {
        if !self.closed {
            return;
        }
        let Some(fence) = self.fence.take() else {
            return;
        };
        // Only the guard that still owns the pending candidate may drop it; a
        // guard whose candidate was already replaced or published is a no-op.
        if let Ok(mut installing) = fence.installing.lock()
            && installing
                .as_ref()
                .is_some_and(|pending| pending.installation == self.installation)
        {
            *installing = None;
        }
    }
}

impl VisibilityFence {
    /// Create a closed fence at sequence 1.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new(None),
            installing: Mutex::new(None),
            sequence: AtomicU64::new(1),
            closed: AtomicBool::new(true),
            exhausted: AtomicBool::new(false),
            installations: AtomicU64::new(0),
            command_gate: Mutex::new(GateState::default()),
            authorization_epoch: AtomicU64::new(0),
            subscribers: Mutex::new(Subscribers::default()),
        })
    }

    /// Current fence sequence, whether open or closed.
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// Whether public data may be read right now.
    pub fn is_open(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }

    /// Close the fence: advance to a fresh odd sequence and wake subscribers.
    ///
    /// Idempotent while already closed and returns the current sequence, so a
    /// consumer can read the shared sequence without reopening.
    pub fn close_visibility(&self) -> u64 {
        self.close_internal(true)
    }

    /// Close as part of a publication commit.
    ///
    /// This close does not invalidate authorization: the commit path closes before
    /// installing its own candidate, and only an independent authorization close
    /// (invalidation, scope or principal intent) may defeat that candidate.
    pub fn close_for_commit(&self) -> u64 {
        self.close_internal(false)
    }

    fn close_internal(&self, invalidate_authorization: bool) -> u64 {
        let mut installing = self.installing.lock().expect("installation lock");
        *installing = None;
        *self.state.write().expect("fence lock") = None;
        if invalidate_authorization {
            // Every authorization close invalidates candidates assembled before it,
            // including while the fence is already closed: a commit close must not
            // hide a later revocation.
            self.authorization_epoch.fetch_add(1, Ordering::AcqRel);
        }
        if self.closed.load(Ordering::Acquire) {
            // Idempotent close for the sequence: report it without advancing.
            return self.sequence.load(Ordering::Acquire);
        }
        let current = self.sequence.load(Ordering::Acquire);
        let next = self.next_odd(current);
        self.sequence.store(next, Ordering::Release);
        self.closed.store(true, Ordering::Release);
        let mut subscribers = self.subscribers.lock().expect("subscriber lock");
        for waker in subscribers.wakers.drain(..) {
            waker.wake();
        }
        next
    }

    /// Enter the authorization/dispatch boundary.
    ///
    /// The caller either closes authorization and enqueues an intent, or validates
    /// and dispatches a command intent, but never both at once. The gate is
    /// reentrant for the owning thread because an adapter may synchronously close
    /// authorization from inside a dispatch.
    pub fn enter_command_boundary(&self) -> CommandGateGuard<'_> {
        let current = std::thread::current().id();
        loop {
            let mut state = self.command_gate.lock().expect("command gate");
            if state.owner == Some(current) {
                state.depth += 1;
                return CommandGateGuard {
                    fence: self,
                    thread: current,
                    _thread_bound: std::marker::PhantomData,
                };
            }
            if state.owner.is_none() {
                state.owner = Some(current);
                state.depth = 1;
                return CommandGateGuard {
                    fence: self,
                    thread: current,
                    _thread_bound: std::marker::PhantomData,
                };
            }
            drop(state);
            std::thread::yield_now();
        }
    }

    /// Authorization generation the publication path must validate against.
    pub fn authorization_epoch(&self) -> u64 {
        self.authorization_epoch.load(Ordering::Acquire)
    }

    fn next_odd(&self, current: u64) -> u64 {
        if current >= u64::MAX - 2 {
            self.exhausted.store(true, Ordering::Release);
            return current;
        }
        (current + 1) | 1
    }

    /// Install a complete candidate while closed; the tag follows the close sequence.
    /// Install a candidate that is only publishable while the authorization epoch
    /// it was assembled under is still current.
    pub fn install(
        self: &Arc<Self>,
        publication: ObservationPublication,
        authorization_epoch: u64,
    ) -> ClosedFence {
        let mut installing = self.installing.lock().expect("installation lock");
        let expected_open = self.sequence.load(Ordering::Acquire) + 1;
        let installation = self.installations.fetch_add(1, Ordering::AcqRel) + 1;
        let guard = ClosedFence {
            fence: Some(Arc::clone(self)),
            installation,
            expected_open,
            closed: true,
        };
        // Replace any pending candidate: its guard can no longer drop ours.
        *installing = None;
        if self.exhausted.load(Ordering::Acquire) || !self.closed.load(Ordering::Acquire) {
            return guard;
        }
        *installing = Some(Arc::new(Record {
            tag: expected_open,
            installation,
            authorization_epoch,
            publication: Arc::new(publication),
        }));
        guard
    }

    /// Reopen the fence with the guard that owns the pending candidate.
    ///
    /// The guard is consumed: only the installation that produced a candidate can
    /// publish it, and only from the exact closed sequence it was installed at.
    /// Candidate, sequence and closed flag are committed in one critical section
    /// shared with close and install.
    pub fn open(&self, guard: ClosedFence) -> Result<(), ClosedFence> {
        let mut installing = self.installing.lock().expect("installation lock");
        if !guard.closed {
            return Err(ClosedFence::consumed());
        }
        if self.exhausted.load(Ordering::Acquire) || !self.closed.load(Ordering::Acquire) {
            return Err(ClosedFence::consumed());
        }
        let opened = self.sequence.load(Ordering::Acquire);
        if guard.expected_open != opened + 1 {
            return Err(ClosedFence::consumed());
        }
        let Some(record) = installing.as_ref() else {
            return Err(ClosedFence::consumed());
        };
        if record.installation != guard.installation || record.tag != opened + 1 {
            // A foreign candidate must not be published; stay closed.
            return Err(ClosedFence::consumed());
        }
        if record.authorization_epoch != self.authorization_epoch.load(Ordering::Acquire) {
            // The authorization this candidate was assembled under is gone: a close
            // that happened after assembly must never be undone by publishing it.
            installing.take();
            return Err(ClosedFence::consumed());
        }
        let record = installing.take().expect("candidate checked above");
        *self.state.write().expect("fence lock") = Some(record);
        self.sequence.store(opened + 1, Ordering::Release);
        self.closed.store(false, Ordering::Release);
        Ok(())
    }

    /// Current open tag, when the fence is open and internally consistent.
    ///
    /// The sequence, the record tag and the closed flag must agree; a mismatch is
    /// reported as closed rather than as a usable tag.
    pub fn current_tag(&self) -> Option<u64> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let state = self.state.try_read().ok()?;
        let record = state.as_ref()?;
        let sequence = self.sequence.load(Ordering::Acquire);
        (sequence == record.tag && !self.closed.load(Ordering::Acquire)).then_some(record.tag)
    }

    /// Borrow the current complete publication, when open.
    pub fn current(self: &Arc<Self>) -> Result<ObservationLease, ReadUnavailable> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ReadUnavailable::Superseded);
        }
        let state = match self.state.try_read() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Err(ReadUnavailable::Superseded),
            Err(TryLockError::Poisoned(_)) => return Err(ReadUnavailable::Closed),
        };
        let Some(record) = state.as_ref() else {
            return Err(if self.exhausted.load(Ordering::Acquire) {
                ReadUnavailable::Exhausted
            } else {
                ReadUnavailable::Superseded
            });
        };
        if self.closed.load(Ordering::Acquire)
            || self.sequence.load(Ordering::Acquire) != record.tag
        {
            return Err(ReadUnavailable::Superseded);
        }
        Ok(ObservationLease {
            fence: Arc::clone(self),
            tag: record.tag,
        })
    }

    /// One complete publication, read under its exact tag.
    fn with_record<R>(
        &self,
        tag: u64,
        derive: impl FnOnce(&Arc<ObservationPublication>) -> R,
    ) -> Option<R> {
        let state = self.state.try_read().ok()?;
        let record = state.as_ref()?;
        if record.tag != tag {
            return None;
        }
        Some(derive(&record.publication))
    }

    /// Register a subscriber waker, bounded by [`MAX_SUBSCRIBERS`].
    fn register(&self, waker: &Waker) -> bool {
        let mut subscribers = self.subscribers.lock().expect("subscriber lock");
        if subscribers.closed {
            waker.wake_by_ref();
            return false;
        }
        if subscribers.wakers.len() >= MAX_SUBSCRIBERS
            && !subscribers
                .wakers
                .iter()
                .any(|existing| existing.will_wake(waker))
        {
            return false;
        }
        if !subscribers
            .wakers
            .iter()
            .any(|existing| existing.will_wake(waker))
        {
            subscribers.wakers.push(waker.clone());
        }
        true
    }

    /// Wake every subscriber because a new publication was installed.
    pub fn notify_subscribers(&self) {
        let mut subscribers = self.subscribers.lock().expect("subscriber lock");
        for waker in subscribers.wakers.drain(..) {
            waker.wake();
        }
    }

    /// Close subscriptions permanently (final shutdown).
    pub fn close_subscriptions(&self) {
        let mut subscribers = self.subscribers.lock().expect("subscriber lock");
        subscribers.closed = true;
        for waker in subscribers.wakers.drain(..) {
            waker.wake();
        }
    }
}

/// A subscription to publication changes with a single pending notice slot.
pub struct PublicationSubscription {
    fence: Arc<VisibilityFence>,
    /// Last observed (generation, version) pair.
    ///
    /// A version alone is not an identity: replacement restarts versions inside
    /// a newer generation, so both coordinates are compared.
    seen: (u64, u64),
    /// Whether the close notice for the current closed period was delivered.
    close_reported: bool,
}

impl PublicationSubscription {
    /// Start a subscription at the current fence state.
    pub fn new(fence: Arc<VisibilityFence>) -> Self {
        Self {
            fence,
            seen: (0, 0),
            close_reported: false,
        }
    }

    /// Await the next complete publication version.
    pub fn changed(&mut self) -> PublicationChange<'_> {
        PublicationChange { subscription: self }
    }
}

/// Future returned by [`PublicationSubscription::changed`].
pub struct PublicationChange<'a> {
    subscription: &'a mut PublicationSubscription,
}

impl Future for PublicationChange<'_> {
    type Output = Option<crate::observation::PublicationNotice>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if !this.subscription.fence.is_open() {
            // The close is delivered exactly once so the consumer can drop cached
            // authorized data; afterwards the subscription parks until the fence
            // reopens or the final shutdown closes it. It never busy-returns.
            let shutdown = this
                .subscription
                .fence
                .subscribers
                .lock()
                .map(|subscribers| subscribers.closed)
                .unwrap_or(true);
            if shutdown {
                return Poll::Ready(None);
            }
            if !this.subscription.close_reported {
                this.subscription.close_reported = true;
                return Poll::Ready(None);
            }
            if !this.subscription.fence.register(cx.waker()) {
                return Poll::Ready(None);
            }
            if this.subscription.fence.is_open() {
                // Reopened while registering: fall through to the open path.
            } else {
                return Poll::Pending;
            }
        }
        this.subscription.close_reported = false;
        match this.subscription.fence.current() {
            Ok(lease) => {
                if let Some(notice) = lease.notice() {
                    let identity = (notice.session_generation, notice.publication_version);
                    if identity != this.subscription.seen {
                        this.subscription.seen = identity;
                        return Poll::Ready(Some(notice));
                    }
                }
            }
            Err(_) => {
                let closed = this
                    .subscription
                    .fence
                    .subscribers
                    .lock()
                    .map(|subscribers| subscribers.closed)
                    .unwrap_or(true);
                if closed {
                    return Poll::Ready(None);
                }
            }
        }
        if !this.subscription.fence.register(cx.waker()) {
            // Saturated: report the current state instead of stalling.
            return Poll::Ready(None);
        }
        // Re-check after registering: neither a publication nor a close that landed
        // while the waker was being registered may be lost.
        if !this.subscription.fence.is_open() {
            let shutdown = this
                .subscription
                .fence
                .subscribers
                .lock()
                .map(|subscribers| subscribers.closed)
                .unwrap_or(true);
            if shutdown {
                return Poll::Ready(None);
            }
            if !this.subscription.close_reported {
                this.subscription.close_reported = true;
                return Poll::Ready(None);
            }
            return Poll::Pending;
        }
        if let Some(notice) = this
            .subscription
            .fence
            .current()
            .ok()
            .and_then(|lease| lease.notice())
        {
            let identity = (notice.session_generation, notice.publication_version);
            if identity != this.subscription.seen {
                this.subscription.seen = identity;
                return Poll::Ready(Some(notice));
            }
        }
        Poll::Pending
    }
}

/// A tagged lease over one complete publication.
pub struct ObservationLease {
    fence: Arc<VisibilityFence>,
    tag: u64,
}

impl ObservationLease {
    /// Whether this lease still matches the open fence record.
    pub fn is_current(&self) -> bool {
        self.fence.current_tag() == Some(self.tag)
    }

    /// Derive a tagged value; unavailability never panics and never blocks.
    pub fn read<R>(
        &self,
        derive: impl FnOnce(&ObservationPublication) -> R,
    ) -> Result<TaggedRead<R>, ReadUnavailable> {
        if !self.is_current() {
            return Err(ReadUnavailable::Superseded);
        }
        let value = self
            .fence
            .with_record(self.tag, |publication| derive(publication))
            .ok_or(ReadUnavailable::Superseded)?;
        Ok(TaggedRead {
            fence: Arc::clone(&self.fence),
            tag: self.tag,
            value,
        })
    }
    /// Notice of the leased publication version.
    /// `None` means the lease no longer refers to the open record: a stale lease
    /// must never look like a valid notice with a default version.
    pub fn notice(&self) -> Option<crate::observation::PublicationNotice> {
        self.fence.with_record(self.tag, |publication| {
            crate::observation::PublicationNotice {
                session_generation: publication.session_generation,
                publication_version: publication.publication_version,
            }
        })
    }

    /// Safe error category of the leased publication.
    pub fn last_error(&self) -> Option<PublicationError> {
        self.fence
            .with_record(self.tag, |publication| publication.last_error.clone())
            .flatten()
    }
}

/// A derived value that may only be used while its tag is still current.
pub struct TaggedRead<T> {
    fence: Arc<VisibilityFence>,
    tag: u64,
    value: T,
}

impl<T> TaggedRead<T> {
    /// Use the derived value after a final currency check.
    pub fn consume(&self, use_current: impl FnOnce(&T)) -> Result<(), ReadUnavailable> {
        if self.fence.current_tag() != Some(self.tag) {
            return Err(ReadUnavailable::Superseded);
        }
        use_current(&self.value);
        Ok(())
    }
}

/// A tagged authorized command read.
pub struct AuthorizedCommandRead {
    inner: TaggedRead<crate::observation::CommandRead>,
}

impl AuthorizedCommandRead {
    pub(crate) fn new(inner: TaggedRead<crate::observation::CommandRead>) -> Self {
        Self { inner }
    }

    /// Derive a value from the authorized read under its tag.
    pub fn read<R>(
        &self,
        derive: impl FnOnce(&crate::observation::CommandRead) -> R,
    ) -> Result<TaggedRead<R>, ReadUnavailable> {
        if !self.inner.is_current() {
            return Err(ReadUnavailable::Superseded);
        }
        let mut derived = None;
        self.inner
            .consume(|value| derived = Some(derive(value)))
            .map_err(|_| ReadUnavailable::Superseded)?;
        self.inner
            .tag_read(derived.expect("derive ran under a current tag"))
    }
}

impl<T> TaggedRead<T> {
    pub(crate) fn is_current(&self) -> bool {
        self.fence.current_tag() == Some(self.tag)
    }

    pub(crate) fn tag_read<R>(&self, value: R) -> Result<TaggedRead<R>, ReadUnavailable> {
        if !self.is_current() {
            return Err(ReadUnavailable::Superseded);
        }
        Ok(TaggedRead {
            fence: Arc::clone(&self.fence),
            tag: self.tag,
            value,
        })
    }
}
