//! Runtime reconnect supervision.
//!
//! The observation client owns one live session. When the Runtime disappears and
//! reappears at the same discovery path — a restart, a new epoch, a revoked
//! credential — that session is dead: the client reaches a terminal
//! `Unauthorized`/`Incompatible` state and its bounded recovery budget cannot be
//! reset from inside. This module decides *when* the Desktop has to re-discover
//! the Runtime, re-authorize and install a completely fresh client, and the
//! composition root supplies the *how* through [`RuntimeReconnect`].
//!
//! It is deliberately GPUI-free and clock-free: [`ReconnectSchedule`] is a
//! deterministic decision machine driven by explicit milliseconds, and
//! [`ReconnectWorker`] applies it to one handle. The only thread is
//! [`ReconnectSupervisor`], which ticks the worker and nothing else.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cos_client::DesktopClient;

use crate::observation::ObservationHandle;

/// Why one reconnect attempt could not produce an observation client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectUnavailable {
    /// No valid discovery descriptor at the expected path, or no usable origin.
    Discovery,
    /// The credential could not be resolved, refreshed or issued.
    Authorization,
    /// The observation client itself could not be constructed for the origin.
    Client,
}

impl ReconnectUnavailable {
    /// Safe label for logs and diagnostics; never carries a credential or a path.
    pub fn label(self) -> &'static str {
        match self {
            Self::Discovery => "the Runtime discovery descriptor is unavailable",
            Self::Authorization => "the Runtime credential is unavailable",
            Self::Client => "the observation client could not be built",
        }
    }
}

/// Composition-root hook that rebuilds one observation client.
///
/// The implementation re-reads the discovery descriptor, re-authorizes the
/// Desktop principal and builds a fresh client; app-core never performs that I/O
/// and only decides when a fresh attempt is due.
pub trait RuntimeReconnect: Send + Sync {
    /// One complete discovery + authorization + client-build attempt.
    fn connect(&self) -> Result<Box<dyn DesktopClient>, ReconnectUnavailable>;
}

/// Timing policy for reconnect supervision.
#[derive(Clone, Copy, Debug)]
pub struct ReconnectPolicy {
    /// Granularity of the supervisor's wake-ups.
    pub tick: Duration,
    /// First delay after the session becomes unavailable.
    pub min_delay: Duration,
    /// Upper bound of the exponential backoff (never an attempt budget: the
    /// Desktop keeps retrying while it is running, which is exactly what a
    /// long Runtime outage needs).
    pub max_delay: Duration,
    /// Time an installed client gets to reach an observing publication before the
    /// attempt counts as failed.
    pub settle: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            tick: Duration::from_millis(250),
            min_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(15),
            settle: Duration::from_secs(10),
        }
    }
}

impl ReconnectPolicy {
    fn min_delay_ms(&self) -> u64 {
        self.min_delay.as_millis().max(1) as u64
    }

    fn max_delay_ms(&self) -> u64 {
        (self.max_delay.as_millis() as u64).max(self.min_delay_ms())
    }

    fn settle_ms(&self) -> u64 {
        self.settle.as_millis() as u64
    }

    /// Deterministic exponential backoff for attempt `attempt` (1-based).
    fn delay_ms(&self, attempt: u32) -> u64 {
        let min = self.min_delay_ms();
        let factor = 1u64 << attempt.saturating_sub(1).min(16);
        min.saturating_mul(factor).min(self.max_delay_ms())
    }
}

/// What one supervision step produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectStep {
    /// Nothing was due.
    Idle,
    /// An attempt was due and the provider could not build a client.
    Unavailable(ReconnectUnavailable),
    /// A fresh client was installed and the observation is expected to reopen.
    Installed,
}

enum Decision {
    Idle,
    Attempt,
}

/// Deterministic reconnect decision machine.
///
/// It carries no clock and performs no I/O: the caller passes the observed
/// milliseconds and whether the publication currently carries an observing
/// session, which makes the whole policy testable without threads or sleeps.
#[derive(Debug)]
pub struct ReconnectSchedule {
    policy: ReconnectPolicy,
    /// Completed attempts since the session became unavailable.
    attempts: u32,
    /// Next time (caller milliseconds) an attempt may start.
    next_at_ms: u64,
    /// Set by the explicit reconnect action.
    forced: bool,
    /// When the last install happened; the attempt is unresolved until the
    /// client either observes or the settle window expires.
    settled_at_ms: Option<u64>,
    started: bool,
}

impl ReconnectSchedule {
    /// Build a schedule for one policy.
    pub fn new(policy: ReconnectPolicy) -> Self {
        Self {
            policy,
            attempts: 0,
            next_at_ms: 0,
            forced: false,
            settled_at_ms: None,
            started: false,
        }
    }

    /// The explicit reconnect action: attempt at the next step, without waiting
    /// for the pending backoff.
    pub fn request_now(&mut self) {
        self.forced = true;
    }

    /// Completed attempts since the last observed session.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// One observation of the current publication availability.
    fn decide(&mut self, now_ms: u64, available: bool) -> Decision {
        if !self.started {
            self.started = true;
            self.next_at_ms = now_ms.saturating_add(self.policy.min_delay_ms());
        }
        if std::mem::take(&mut self.forced) {
            // The explicit action is a standing override of the schedule: one
            // immediate attempt, whether or not a session is currently observing.
            self.settled_at_ms = None;
            return Decision::Attempt;
        }
        if available {
            // An observing session: nothing to recover, and the next outage starts
            // again from the first delay instead of an inherited long backoff.
            self.attempts = 0;
            self.settled_at_ms = None;
            self.next_at_ms = now_ms.saturating_add(self.policy.min_delay_ms());
            return Decision::Idle;
        }
        if let Some(settled) = self.settled_at_ms {
            let until = settled.saturating_add(self.policy.settle_ms());
            if now_ms < until {
                // The replacement is still connecting: never race it with another
                // client (each attempt re-issues a credential and opens a session).
                return Decision::Idle;
            }
            // Installed but never observed: count it as a failed attempt.
            self.settled_at_ms = None;
            self.attempts = self.attempts.saturating_add(1);
            self.next_at_ms = now_ms.saturating_add(self.policy.delay_ms(self.attempts));
            return Decision::Idle;
        }
        if now_ms < self.next_at_ms {
            return Decision::Idle;
        }
        Decision::Attempt
    }

    /// A fresh client was installed; wait for it to observe (or for the settle
    /// window to expire before another attempt).
    fn installed(&mut self, now_ms: u64) {
        self.settled_at_ms = Some(now_ms);
    }

    /// The provider could not build a client this time.
    fn failed(&mut self, now_ms: u64) {
        self.settled_at_ms = None;
        self.attempts = self.attempts.saturating_add(1);
        self.next_at_ms = now_ms.saturating_add(self.policy.delay_ms(self.attempts));
    }
}

/// Applies one [`ReconnectSchedule`] to one observation handle.
///
/// A step never blocks on the owner: the replacement is queued through the same
/// owner mailbox every other intent uses, and the settle window decides whether
/// the attempt actually reopened the observation.
pub struct ReconnectWorker {
    handle: ObservationHandle,
    provider: Arc<dyn RuntimeReconnect>,
    schedule: ReconnectSchedule,
    request: Arc<AtomicBool>,
    report: Option<Arc<dyn Fn(ReconnectStep) + Send + Sync>>,
}

impl ReconnectWorker {
    /// Build one worker over a handle and its composition-root provider.
    pub fn new(
        handle: ObservationHandle,
        provider: Arc<dyn RuntimeReconnect>,
        policy: ReconnectPolicy,
        request: Arc<AtomicBool>,
        report: Option<Arc<dyn Fn(ReconnectStep) + Send + Sync>>,
    ) -> Self {
        Self {
            handle,
            provider,
            schedule: ReconnectSchedule::new(policy),
            request,
            report,
        }
    }

    /// Run one bounded supervision step at caller-supplied milliseconds.
    pub fn step(&mut self, now_ms: u64) -> ReconnectStep {
        if self.request.swap(false, Ordering::AcqRel) {
            self.schedule.request_now();
        }
        let available = self.handle.observation_available();
        let step = match self.schedule.decide(now_ms, available) {
            Decision::Idle => ReconnectStep::Idle,
            Decision::Attempt => match self.provider.connect() {
                Ok(client) => {
                    let _reply = self.handle.replace_client(client);
                    self.schedule.installed(now_ms);
                    ReconnectStep::Installed
                }
                Err(reason) => {
                    self.schedule.failed(now_ms);
                    ReconnectStep::Unavailable(reason)
                }
            },
        };
        if step != ReconnectStep::Idle
            && let Some(report) = self.report.as_ref()
        {
            report(step);
        }
        step
    }

    /// Completed attempts since the last observing publication.
    pub fn attempts(&self) -> u32 {
        self.schedule.attempts()
    }
}

/// One background thread that ticks a [`ReconnectWorker`].
///
/// It stops on drop and when the owner has finished. The join is bounded: a
/// thread still blocked inside one discovery/authorization attempt at shutdown is
/// detached instead of delaying the exit handshake (the process is exiting and
/// the attempt is already bounded by its own timeouts).
pub struct ReconnectSupervisor {
    stop: Arc<AtomicBool>,
    request: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ReconnectSupervisor {
    /// Start supervising one handle.
    pub fn start(
        handle: ObservationHandle,
        provider: Arc<dyn RuntimeReconnect>,
        policy: ReconnectPolicy,
        report: Option<Arc<dyn Fn(ReconnectStep) + Send + Sync>>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let request = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_request = Arc::clone(&request);
        let join = std::thread::Builder::new()
            .name("cos-runtime-reconnect".to_owned())
            .spawn(move || {
                let mut worker =
                    ReconnectWorker::new(handle, provider, policy, worker_request, report);
                let started = std::time::Instant::now();
                let slice = Duration::from_millis(50);
                while !worker_stop.load(Ordering::Acquire) && !worker.handle.is_finished() {
                    let now_ms = started.elapsed().as_millis() as u64;
                    let _ = worker.step(now_ms);
                    // Wake in slices so shutdown never waits a whole tick.
                    let mut slept = Duration::ZERO;
                    while slept < policy.tick && !worker_stop.load(Ordering::Acquire) {
                        let step = slice.min(policy.tick - slept);
                        std::thread::sleep(step);
                        slept += step;
                    }
                }
            })
            .ok();
        Self {
            stop,
            request,
            join,
        }
    }

    /// The explicit reconnect action: force one attempt now instead of waiting for
    /// the pending backoff.
    pub fn request_now(&self) {
        self.request.store(true, Ordering::Release);
    }
}

impl Drop for ReconnectSupervisor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let Some(join) = self.join.take() else {
            return;
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !join.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if join.is_finished() {
            let _ = join.join();
        }
        // Otherwise the thread is inside one bounded attempt; detach it rather
        // than delay the exit handshake.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ReconnectPolicy {
        ReconnectPolicy {
            tick: Duration::from_millis(1),
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(800),
            settle: Duration::from_millis(1_000),
        }
    }

    fn attempt(schedule: &mut ReconnectSchedule, now_ms: u64) -> bool {
        matches!(schedule.decide(now_ms, false), Decision::Attempt)
    }

    #[test]
    fn an_available_session_is_never_reconnected() {
        let mut schedule = ReconnectSchedule::new(policy());
        for now in [0, 1, 500, 5_000] {
            assert!(matches!(schedule.decide(now, true), Decision::Idle));
        }
        assert_eq!(schedule.attempts(), 0);
    }

    #[test]
    fn the_first_attempt_waits_for_the_first_delay() {
        let mut schedule = ReconnectSchedule::new(policy());
        assert!(!attempt(&mut schedule, 0));
        assert!(!attempt(&mut schedule, 50));
        assert!(attempt(&mut schedule, 100));
    }

    #[test]
    fn failed_attempts_back_off_up_to_the_cap() {
        let mut schedule = ReconnectSchedule::new(policy());
        assert!(!attempt(&mut schedule, 0));
        assert!(attempt(&mut schedule, 100));
        schedule.failed(100);
        assert!(!attempt(&mut schedule, 199));
        assert!(attempt(&mut schedule, 300), "second attempt after 200 ms");
        schedule.failed(300);
        assert!(attempt(&mut schedule, 700), "third attempt after 400 ms");
        schedule.failed(700);
        assert!(attempt(&mut schedule, 1_500), "fourth attempt after 800 ms");
        schedule.failed(1_500);
        assert!(
            attempt(&mut schedule, 2_300),
            "the cap keeps the delay at 800 ms"
        );
    }

    #[test]
    fn an_installed_client_gets_the_settle_window_before_another_attempt() {
        let mut schedule = ReconnectSchedule::new(policy());
        assert!(!attempt(&mut schedule, 0));
        assert!(attempt(&mut schedule, 100));
        schedule.installed(100);
        assert!(
            !attempt(&mut schedule, 200),
            "the fresh client is still connecting"
        );
        assert!(!attempt(&mut schedule, 1_099));
        assert!(
            !attempt(&mut schedule, 1_100),
            "the expired settle window counts a failure and waits its backoff"
        );
        assert!(
            attempt(&mut schedule, 1_300),
            "retry after the backoff delay"
        );
    }

    #[test]
    fn an_observing_publication_resets_the_backoff() {
        let mut schedule = ReconnectSchedule::new(policy());
        assert!(!attempt(&mut schedule, 0));
        assert!(attempt(&mut schedule, 100));
        schedule.failed(100);
        schedule.failed(300);
        assert_eq!(schedule.attempts(), 2);
        // The session recovers.
        assert!(matches!(schedule.decide(400, true), Decision::Idle));
        assert_eq!(schedule.attempts(), 0);
        // The next outage starts from the first delay, not the inherited backoff.
        assert!(!attempt(&mut schedule, 450));
        assert!(attempt(&mut schedule, 500));
    }

    #[test]
    fn the_explicit_action_forces_an_immediate_attempt() {
        let mut schedule = ReconnectSchedule::new(policy());
        assert!(!attempt(&mut schedule, 0));
        assert!(attempt(&mut schedule, 100));
        schedule.failed(100);
        assert!(!attempt(&mut schedule, 150), "the backoff is pending");
        schedule.request_now();
        assert!(
            attempt(&mut schedule, 151),
            "the explicit action never waits for the pending backoff"
        );
        // The force is consumed once, not a standing bypass of the policy.
        assert!(!attempt(&mut schedule, 152));
    }
}
