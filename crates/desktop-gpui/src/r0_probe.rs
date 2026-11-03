//! Explicit-feature development observations, not Domain state or a diagnostics API.
//! Only the application thread records fixed-size events; export is owned by the binary.

use app_core::navigation::Surface;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

const PRE_ACTIVE_LIMIT: usize = 64;

/// Local presentation facts; no canonical revision or private identity is invented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    SurfaceChanged(Surface),
    Notify,
    WindowCreated(u64),
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
    pending: Vec<Event>,
}

static BINDING: Mutex<Option<Binding>> = Mutex::new(None);

fn binding() -> MutexGuard<'static, Option<Binding>> {
    BINDING.lock().unwrap_or_else(|error| error.into_inner())
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
        *binding() = None;
    }
}

/// Install once on the owning application thread. No producer-side formatting or I/O.
pub fn attach(sender: SyncSender<Event>, control: Arc<Control>) -> Result<Lease, &'static str> {
    let mut slot = binding();
    if slot.is_some() {
        return Err("R0 presentation probe is already attached");
    }
    *slot = Some(Binding {
        sender,
        control: Arc::clone(&control),
        pending: Vec::new(),
    });
    Ok(Lease {
        control,
        _main_thread: PhantomData,
    })
}

/// Record a window-creation marker before the first draw.
pub fn record_window_created(window: u64) {
    record(window, EventKind::WindowCreated(window));
}

/// Flush buffered pre-admission records without requiring a later event.
/// Safe to call from the capture worker after `Control::start()`.
pub fn flush_pending() {
    let mut slot = binding();
    let Some(binding) = slot.as_mut() else {
        return;
    };
    if !binding.control.active.load(Ordering::Acquire)
        || binding.control.stop_ns_plus_one.load(Ordering::Acquire) != 0
    {
        return;
    }
    let Binding {
        sender,
        control,
        pending,
    } = binding;
    for event in pending.drain(..) {
        send(sender, control, event);
    }
}

/// Record only at an actual mutation or notify call. Full queues drop, never wait.
pub(crate) fn record(window: u64, kind: EventKind) {
    let mut slot = binding();
    let Some(binding) = slot.as_mut() else {
        return;
    };
    let control = Arc::clone(&binding.control);
    control.in_flight.fetch_add(1, Ordering::AcqRel);
    let active = control.active.load(Ordering::Acquire);
    let stopped = control.stop_ns_plus_one.load(Ordering::Acquire) != 0;
    if !stopped {
        if active {
            let Binding {
                sender,
                control,
                pending,
            } = binding;
            for event in pending.drain(..) {
                send(sender, control, event);
            }
            send(
                sender,
                control,
                Event {
                    at: Instant::now(),
                    window,
                    kind,
                },
            );
        } else if binding.pending.len() < PRE_ACTIVE_LIMIT {
            binding.pending.push(Event {
                at: Instant::now(),
                window,
                kind,
            });
        } else {
            // Bounded pre-admission buffer: never drop silently.
            control.dropped.fetch_add(1, Ordering::Release);
        }
    }
    control.in_flight.fetch_sub(1, Ordering::Release);
}

fn send(sender: &SyncSender<Event>, control: &Control, event: Event) {
    match sender.try_send(event) {
        Ok(()) => {
            control.enqueued.fetch_add(1, Ordering::Release);
        }
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            control.dropped.fetch_add(1, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_pre_admission_marker_is_flushed_after_start() -> Result<(), String> {
        let (sender, receiver) = mpsc::sync_channel(8);
        let control = Arc::new(Control::new(Instant::now()));
        let _lease = attach(sender, Arc::clone(&control))?;
        record_window_created(11);
        assert_eq!(receiver.try_iter().count(), 0);
        control.start();
        record(11, EventKind::Notify);
        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, EventKind::WindowCreated(11));
        assert_eq!(events[1].kind, EventKind::Notify);
        assert_eq!(control.accounting(), (2, 0, 0));
        Ok(())
    }

    #[test]
    fn flush_pending_drains_without_a_later_event() -> Result<(), String> {
        let (sender, receiver) = mpsc::sync_channel(8);
        let control = Arc::new(Control::new(Instant::now()));
        let _lease = attach(sender, Arc::clone(&control))?;
        record_window_created(11);
        control.start();
        flush_pending();
        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::WindowCreated(11));
        assert_eq!(control.accounting(), (1, 0, 0));
        Ok(())
    }

    #[test]
    fn the_pre_admission_buffer_is_bounded_and_loss_is_counted() -> Result<(), String> {
        let (sender, receiver) = mpsc::sync_channel(200);
        let control = Arc::new(Control::new(Instant::now()));
        let _lease = attach(sender, Arc::clone(&control))?;
        for _ in 0..(PRE_ACTIVE_LIMIT + 1) {
            record_window_created(11);
        }
        control.start();
        record(11, EventKind::Notify);
        assert_eq!(receiver.try_iter().count(), PRE_ACTIVE_LIMIT + 1);
        assert_eq!(control.accounting(), ((PRE_ACTIVE_LIMIT + 1) as u64, 1, 0));
        Ok(())
    }

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
