use app_core::preferences::{
    DisplayRect, Preferences, clamp_geometry, default_path, load, save_atomic,
};
use app_core::{
    DeadlineScheduler, ExitWatch, ObservationHandle, ObservationSessionOwner, OwnerClock, drive,
};
use cos_client::DesktopClient;
use desktop_gpui::Shell;
use desktop_protocol::ObservationScopeRequest;
use gpui::{
    App, AppContext, Bounds, Task, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};
use std::future::{Future, poll_fn};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

/// Observation-only result; no host is distinct from a confirmed driver stop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObservationExit {
    Confirmed,
    Unconfirmed,
    NotApplicable,
    #[default]
    NotObserved,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreferencesExit {
    Saved,
    Failed,
    Unconfirmed,
    #[default]
    NotObserved,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExitReport {
    pub observation: ObservationExit,
    pub preferences: PreferencesExit,
}

pub type ExitOutcome = Arc<Mutex<ExitReport>>;

/// Owns the background observation task so the exit path can confirm its stop.
pub struct ObservationHost {
    handle: Option<ObservationHandle>,
    driver: Option<Task<()>>,
    /// Shared with the retained driver task: set when it has finished.
    driver_done: Arc<AtomicBool>,
    /// Waker slot shared by the exit watcher and the driver's completion.
    exit_waker: Arc<std::sync::Mutex<Option<Waker>>>,
}

impl ObservationHost {
    /// Start observation for one process-wide owner.
    pub fn start(
        cx: &mut App,
        client: Box<dyn DesktopClient>,
        scope: ObservationScopeRequest,
    ) -> (Self, Option<ObservationHandle>) {
        let driver_done = Arc::new(AtomicBool::new(false));
        let exit_waker: Arc<std::sync::Mutex<Option<Waker>>> =
            Arc::new(std::sync::Mutex::new(None));
        let (mut owner, handle) = ObservationSessionOwner::new(client);
        owner.arm_control(&handle);
        if owner.start(scope).is_err() {
            driver_done.store(true, Ordering::Release);
            return (
                Self {
                    handle: None,
                    driver: None,
                    driver_done,
                    exit_waker,
                },
                None,
            );
        }
        let clock = OwnerClock::default();
        let timer = DeadlineScheduler::start(&handle);
        let driver = handle.clone();
        let finished = Arc::clone(&driver_done);
        let exit_wake = Arc::clone(&exit_waker);
        let task = cx.background_executor().spawn(async move {
            drive(owner, driver, clock, timer).await;
            finished.store(true, Ordering::Release);
            if let Ok(mut slot) = exit_wake.lock()
                && let Some(waker) = slot.take()
            {
                waker.wake();
            }
        });
        (
            Self {
                handle: Some(handle.clone()),
                driver: Some(task),
                driver_done,
                exit_waker,
            },
            Some(handle),
        )
    }

    /// Keep the driver alive until the callback-origin deadline is observed.
    async fn confirm_shutdown(self, watch: ExitWatch) -> ObservationExit {
        let _driver = self.driver;
        if watch.wait().await {
            ObservationExit::Confirmed
        } else {
            ObservationExit::Unconfirmed
        }
    }
}

fn persist_preferences(path: Option<&Path>, preferences: &Mutex<Preferences>) -> bool {
    path.is_some_and(|path| {
        preferences
            .lock()
            .is_ok_and(|preferences| save_atomic(path, &preferences).is_ok())
    })
}

async fn confirm_preferences(
    watch: ExitWatch,
    result: Arc<Mutex<Option<bool>>>,
) -> PreferencesExit {
    if !watch.wait().await {
        return PreferencesExit::Unconfirmed;
    }
    match result.lock() {
        Ok(result) => match *result {
            Some(true) => PreferencesExit::Saved,
            Some(false) => PreferencesExit::Failed,
            None => PreferencesExit::Unconfirmed,
        },
        Err(_) => PreferencesExit::Unconfirmed,
    }
}

async fn confirm_exit(
    observation: impl Future<Output = ObservationExit>,
    preferences: impl Future<Output = PreferencesExit>,
    outcome: ExitOutcome,
) {
    let mut observation = std::pin::pin!(observation);
    let mut preferences = std::pin::pin!(preferences);
    let mut observation_done = false;
    let mut preferences_done = false;
    poll_fn(|cx| {
        if !observation_done && let Poll::Ready(result) = observation.as_mut().poll(cx) {
            if let Ok(mut report) = outcome.lock() {
                report.observation = result;
            }
            observation_done = true;
        }
        if !preferences_done && let Poll::Ready(result) = preferences.as_mut().poll(cx) {
            if let Ok(mut report) = outcome.lock() {
                report.preferences = result;
            }
            preferences_done = true;
        }
        if observation_done && preferences_done {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}

pub fn open(
    cx: &mut App,
    shell: impl FnOnce(&mut gpui::Window, &mut gpui::Context<Shell>, Arc<Mutex<Preferences>>) -> Shell
    + 'static,
    host: Option<ObservationHost>,
    outcome: ExitOutcome,
) -> Result<(), String> {
    let path = default_path();
    let loaded = path
        .as_ref()
        .map_or_else(app_core::preferences::LoadedPreferences::default, |path| {
            load(path)
        });
    if let Some(notice) = loaded.notice {
        eprintln!("{notice}");
    }
    let displays = cx
        .displays()
        .iter()
        .map(|display| {
            let bounds = display.visible_bounds();
            DisplayRect {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            }
        })
        .collect::<Vec<_>>();
    let geometry = clamp_geometry(loaded.preferences.window, &displays);
    let bounds = Bounds::new(
        point(px(geometry.x), px(geometry.y)),
        size(px(geometry.width), px(geometry.height)),
    );
    let window_bounds = if geometry.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    };
    let preferences = Arc::new(Mutex::new(loaded.preferences));
    let save_preferences = Arc::clone(&preferences);
    let mut save_path = path;
    let mut exit_host = host;
    let exit_outcome = Arc::clone(&outcome);
    cx.on_app_quit(move |cx| {
        let started = Instant::now();
        let max_budget = gpui::SHUTDOWN_TIMEOUT / 2;
        let observation_budget = std::env::var("COS_DESKTOP_EXIT_BUDGET_MS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .map_or(max_budget, Duration::from_millis)
            .min(max_budget);
        let host = exit_host.take();
        // Close admission synchronously before either timer or filesystem work.
        if let Some(handle) = host.as_ref().and_then(|host| host.handle.as_ref()) {
            handle.request_shutdown();
        }
        if let Ok(mut report) = exit_outcome.lock() {
            report.observation = if host.as_ref().is_some_and(|host| host.driver.is_some()) {
                ObservationExit::Unconfirmed
            } else {
                ObservationExit::NotApplicable
            };
            report.preferences = PreferencesExit::Unconfirmed;
        }
        let observation = host.and_then(|host| {
            host.driver.as_ref()?;
            let watch = ExitWatch::new(
                Arc::clone(&host.driver_done),
                Arc::clone(&host.exit_waker),
                started + observation_budget,
            );
            Some((host, watch))
        });
        let saved = Arc::new(Mutex::new(None));
        let done = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Mutex::new(None));
        let save_watch = ExitWatch::new(Arc::clone(&done), Arc::clone(&wake), started + max_budget);
        let path = save_path.take();
        let preferences = Arc::clone(&save_preferences);
        let completion = Arc::clone(&saved);
        let save = cx.background_executor().spawn(async move {
            let success = persist_preferences(path.as_deref(), &preferences);
            if let Ok(mut result) = completion.lock() {
                *result = Some(success);
            }
            done.store(true, Ordering::Release);
            if let Ok(mut slot) = wake.lock()
                && let Some(waker) = slot.take()
            {
                waker.wake();
            }
        });
        // The attempt must start even if the bounded wait finishes first, and a
        // late completion must not be cancelled by dropping this quit future.
        save.detach();
        let outcome = Arc::clone(&exit_outcome);
        async move {
            let observation = async move {
                match observation {
                    Some((host, watch)) => host.confirm_shutdown(watch).await,
                    None => ObservationExit::NotApplicable,
                }
            };
            confirm_exit(observation, confirm_preferences(save_watch, saved), outcome).await;
        }
    })
    .detach();
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    let shell_preferences = Arc::clone(&preferences);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(window_bounds),
            titlebar: Some(TitlebarOptions {
                title: Some("COS Desktop".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(1000.0), px(680.0))),
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| shell(window, cx, Arc::clone(&shell_preferences))),
    )
    .map_err(|_| "Native window creation failed".to_owned())?;
    cx.activate(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::task::Context;

    struct Profile(std::path::PathBuf);

    impl Profile {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "cos-exit-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).expect("isolated test profile");
            Self(path)
        }
    }

    impl Drop for Profile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn preferences_result(path: &Path, deadline: Instant) -> impl Future<Output = PreferencesExit> {
        let saved = persist_preferences(Some(path), &Mutex::new(Preferences::default()));
        confirm_preferences(
            ExitWatch::new(
                Arc::new(AtomicBool::new(true)),
                Arc::new(Mutex::new(None)),
                deadline,
            ),
            Arc::new(Mutex::new(Some(saved))),
        )
    }

    #[test]
    fn saved_preferences_are_reported_while_observation_is_still_pending() {
        let profile = Profile::new();
        let path = profile.0.join("preferences.json");
        let outcome = Arc::new(Mutex::new(ExitReport {
            observation: ObservationExit::Unconfirmed,
            preferences: PreferencesExit::Unconfirmed,
        }));
        // The synchronous save stands in for production's background save. An
        // observation watch caps every deadline at five seconds from its own
        // construction, so the pending window has to start at the poll that
        // asserts it: a cold filesystem write before that poll can consume the
        // whole budget and decide this assertion by machine speed instead of by
        // the reported contract.
        let mut exit = std::pin::pin!(confirm_exit(
            async move {
                let watch = ExitWatch::new(
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(Mutex::new(None)),
                    Instant::now() + Duration::from_secs(5),
                );
                if watch.wait().await {
                    ObservationExit::Confirmed
                } else {
                    ObservationExit::Unconfirmed
                }
            },
            preferences_result(&path, Instant::now() + Duration::from_secs(5)),
            Arc::clone(&outcome),
        ));
        assert_eq!(
            exit.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        );
        assert_eq!(
            *outcome.lock().expect("exit report"),
            ExitReport {
                observation: ObservationExit::Unconfirmed,
                preferences: PreferencesExit::Saved,
            }
        );
    }

    #[test]
    fn a_failed_atomic_save_does_not_erase_observation_confirmation() {
        let profile = Profile::new();
        let outcome = Arc::new(Mutex::new(ExitReport::default()));
        // Replacing the nonempty profile directory with a file must fail.
        std::fs::write(profile.0.join("keep"), b"prior bytes").expect("existing contents");
        let mut exit = std::pin::pin!(confirm_exit(
            async { ObservationExit::Confirmed },
            preferences_result(&profile.0, Instant::now() + Duration::from_secs(5)),
            Arc::clone(&outcome),
        ));
        assert_eq!(
            exit.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(())
        );
        assert_eq!(
            *outcome.lock().expect("exit report"),
            ExitReport {
                observation: ObservationExit::Confirmed,
                preferences: PreferencesExit::Failed,
            }
        );
        assert_eq!(
            std::fs::read(profile.0.join("keep")).expect("preserved bytes"),
            b"prior bytes"
        );
    }

    #[test]
    fn a_save_completed_after_its_deadline_is_never_reported_saved() {
        let profile = Profile::new();
        let path = profile.0.join("preferences.json");
        let mut result = std::pin::pin!(preferences_result(
            &path,
            Instant::now() - Duration::from_secs(1),
        ));
        assert_eq!(
            result
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(PreferencesExit::Unconfirmed)
        );
        assert!(
            path.is_file(),
            "late write completed, not deadline-confirmed"
        );
    }
}
