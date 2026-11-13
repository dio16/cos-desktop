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
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;
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
    let Some(path) = path else {
        return false;
    };
    // Clone under the lock and write the clone: the render path takes the same
    // mutex, and a filesystem write must never happen inside that critical
    // section (PROCESS_MODEL: no blocking I/O or lock wait on the UI thread).
    let snapshot = match preferences.lock() {
        Ok(preferences) => preferences.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    save_atomic(path, &snapshot).is_ok()
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
    cx.on_app_quit(move |_cx| {
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
        // Flush the permitted local preferences here, synchronously. The whole quit
        // future is polled for at most `SHUTDOWN_TIMEOUT` (200 ms), so scheduling a
        // background write and then watching a completion flag cannot guarantee a
        // confirmed flush; the write is one small atomic file performed while the UI
        // is already quitting, and its actual result is what the handshake reports.
        let preferences_outcome =
            if persist_preferences(save_path.take().as_deref(), &save_preferences) {
                PreferencesExit::Saved
            } else {
                PreferencesExit::Failed
            };
        if let Ok(mut report) = exit_outcome.lock() {
            report.preferences = preferences_outcome;
        }
        let outcome = Arc::clone(&exit_outcome);
        async move {
            let observation = match observation {
                Some((host, watch)) => host.confirm_shutdown(watch).await,
                None => ObservationExit::NotApplicable,
            };
            if let Ok(mut report) = outcome.lock() {
                report.observation = observation;
            }
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

    /// The quit path reports the flush it actually performed.
    #[test]
    fn a_completed_flush_is_reported_saved() {
        let profile = Profile::new();
        let path = profile.0.join("preferences.json");
        let preferences = Mutex::new(Preferences::default());
        assert!(persist_preferences(Some(&path), &preferences));
        let outcome = if persist_preferences(Some(&path), &preferences) {
            PreferencesExit::Saved
        } else {
            PreferencesExit::Failed
        };
        assert_eq!(outcome, PreferencesExit::Saved);
        assert!(
            path.is_file(),
            "the permitted local preferences were flushed"
        );
        // A missing profile path is an explicit failure, never a silent success.
        assert!(!persist_preferences(None, &preferences));
    }

    /// A poisoned preference lock must not turn the flush into a panic or a false
    /// failure: the value it guards is local presentation state.
    #[test]
    fn a_poisoned_preference_lock_still_flushes() {
        let profile = Profile::new();
        let path = profile.0.join("preferences.json");
        let preferences = Arc::new(Mutex::new(Preferences::default()));
        let poisoned = Arc::clone(&preferences);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("test lock");
            panic!("poison the preference lock");
        })
        .join();
        assert!(preferences.is_poisoned(), "the lock is poisoned");
        assert!(persist_preferences(Some(&path), &preferences));
        assert!(path.is_file());
    }

    /// A failed atomic save keeps the previous file and is reported as a failure.
    #[test]
    fn a_failed_atomic_save_preserves_previous_bytes() {
        let profile = Profile::new();
        // Replacing the nonempty profile directory with a file must fail.
        std::fs::write(profile.0.join("keep"), b"prior bytes").expect("existing contents");
        let preferences = Mutex::new(Preferences::default());
        assert!(!persist_preferences(Some(&profile.0), &preferences));
        assert_eq!(
            std::fs::read(profile.0.join("keep")).expect("preserved bytes"),
            b"prior bytes"
        );
    }
}
