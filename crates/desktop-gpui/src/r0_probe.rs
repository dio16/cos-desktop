//! Explicit-feature development observations, not Domain state or a diagnostics API.
//! Only the application thread records fixed-size events; export is owned by the binary.

use app_core::navigation::Surface;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::Instant;

/// Local presentation facts; no canonical revision or private identity is invented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    SurfaceChanged(Surface),
    Notify,
}

/// One source-side observation. Its serial is assigned by the downstream observer.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub at: Instant,
    pub window: u64,
    pub kind: EventKind,
}

/// Shared finite-capture admission and exact local queue accounting.
pub struct Control {
    origin: Instant,
    active: AtomicBool,
    stop_ns_plus_one: AtomicU64,
    enqueued: AtomicU64,
    dropped: AtomicU64,
    in_flight: AtomicUsize,
}

impl Control {
    pub fn new(origin: Instant) -> Self {
        Self {
            origin,
            active: AtomicBool::new(false),
            stop_ns_plus_one: AtomicU64::new(0),
            enqueued: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            in_flight: AtomicUsize::new(0),
        }
    }

    /// Start only after the background sink has been validated.
    pub fn start(&self) {
        if self.stop_ns_plus_one.load(Ordering::Acquire) == 0 {
            self.active.store(true, Ordering::Release);
        }
    }

    /// Close admission synchronously; never wait for a worker on the GUI thread.
    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
        let at = Instant::now()
            .checked_duration_since(self.origin)
            .and_then(|d| u64::try_from(d.as_nanos()).ok())
            .and_then(|n| n.checked_add(1));
        if let Some(at) = at {
            let _ =
                self.stop_ns_plus_one
                    .compare_exchange(0, at, Ordering::AcqRel, Ordering::Acquire);
        }
    }

    pub fn stopped_at_ns(&self) -> Option<u64> {
        self.stop_ns_plus_one.load(Ordering::Acquire).checked_sub(1)
    }

    /// Consistent after admission is closed and `in_flight == 0`.
    pub fn accounting(&self) -> (u64, u64, usize) {
        (
            self.enqueued.load(Ordering::Acquire),
            self.dropped.load(Ordering::Acquire),
            self.in_flight.load(Ordering::Acquire),
        )
    }
}

struct Binding {
    sender: SyncSender<Event>,
    control: Arc<Control>,
}

thread_local! {
    static BINDING: RefCell<Option<Binding>> = const { RefCell::new(None) };
}

/// Main-thread lease: do not move attachment/cleanup to an unrelated worker.
pub struct Lease {
    control: Arc<Control>,
    _main_thread: PhantomData<Rc<()>>,
}

impl Lease {
    pub fn stop(&self) {
        self.control.stop();
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.control.stop();
        BINDING.with(|binding| {
            *binding.borrow_mut() = None;
        });
    }
}

/// Install once on the owning application thread. No producer-side formatting or I/O.
pub fn attach(sender: SyncSender<Event>, control: Arc<Control>) -> Result<Lease, &'static str> {
    BINDING.with(|binding| {
        let mut binding = binding.borrow_mut();
        if binding.is_some() {
            return Err("R0 presentation probe is already attached");
        }
        *binding = Some(Binding {
            sender,
            control: Arc::clone(&control),
        });
        Ok(Lease {
            control,
            _main_thread: PhantomData,
        })
    })
}

/// Record only at an actual mutation or notify call. Full queues drop, never wait.
pub(crate) fn record(window: u64, kind: EventKind) {
    BINDING.with(|binding| {
        let binding = binding.borrow();
        let Some(binding) = binding.as_ref() else {
            return;
        };
        let control = &binding.control;
        control.in_flight.fetch_add(1, Ordering::AcqRel);
        if control.active.load(Ordering::Acquire)
            && control.stop_ns_plus_one.load(Ordering::Acquire) == 0
        {
            let event = Event {
                at: Instant::now(),
                window,
                kind,
            };
            match binding.sender.try_send(event) {
                Ok(()) => {
                    control.enqueued.fetch_add(1, Ordering::Release);
                }
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                    control.dropped.fetch_add(1, Ordering::Release);
                }
            }
        }
        control.in_flight.fetch_sub(1, Ordering::Release);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_full_probe_queue_preserves_accepted_events_and_accounts_for_loss() -> Result<(), String> {
        let (sender, receiver) = mpsc::sync_channel(2);
        let control = Arc::new(Control::new(Instant::now()));
        let lease = attach(sender, Arc::clone(&control))?;
        control.start();
        record(11, EventKind::SurfaceChanged(Surface::Compare));
        record(11, EventKind::Notify);
        record(11, EventKind::SurfaceChanged(Surface::Flow));
        lease.stop();
        record(11, EventKind::Notify);
        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, EventKind::SurfaceChanged(Surface::Compare));
        assert_eq!(events[1].kind, EventKind::Notify);
        assert_eq!(control.accounting(), (2, 1, 0));
        assert!(control.stopped_at_ns().is_some());
        Ok(())
    }
}
