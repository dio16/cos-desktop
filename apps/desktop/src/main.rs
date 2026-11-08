mod app;
mod contrast_probe;
mod credential_issuance;
mod credentials;
mod instance;
#[cfg(all(feature = "r0-s2b-instrumentation", windows))]
mod r0_s2b;
mod runtime;
mod window;
#[cfg(all(feature = "r0-s2b-instrumentation", not(windows)))]
compile_error!("r0-s2b-instrumentation currently supports native Windows only");

#[cfg(feature = "r0-s2b-instrumentation")]
use app::Mode;
use app::Startup;
use app_core::AppCore;
use cos_client::{DesktopClient, MockCosClient};
use desktop_gpui::Shell;
use gpui::App;
use std::path::PathBuf;

fn main() {
    // Register before any work so an ordinary Rust panic is stored best-effort;
    // the payload and backtrace are never stored or printed by this hook.
    app_core::panic_report::install(env!("CARGO_PKG_VERSION"));
    let exit_outcome: window::ExitOutcome =
        std::sync::Arc::new(std::sync::Mutex::new(window::ExitReport::default()));
    let default_fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/cos-client/fixtures/task-184.json");
    let endpoint = std::env::var(runtime::ENDPOINT_ENV).ok();
    let startup = match Startup::parse(std::env::args().skip(1), endpoint.clone(), default_fixture)
    {
        Ok(startup) => startup,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    #[cfg(feature = "r0-s2b-instrumentation")]
    if !matches!(&startup.mode, Mode::Fixture { .. }) {
        eprintln!("R0 S2B instrumented builds require explicit isolated fixture mode");
        std::process::exit(2);
    }
    let _instance = match instance::InstanceGuard::acquire() {
        Ok(instance) => instance,
        Err(instance::AcquireError::AlreadyRunning) => {
            eprintln!("COS Desktop is already running; second launch refused");
            std::process::exit(3);
        }
        Err(instance::AcquireError::Unavailable(reason)) => {
            eprintln!(
                "COS Desktop instance ownership unavailable; startup refused ({})",
                reason.token()
            );
            std::process::exit(2);
        }
    };
    // One resolution point for the whole process: the shell receives this
    // resolved set and never reads a token constant of its own.
    let tokens = match app::resolve_tokens() {
        Ok(tokens) => tokens,
        Err(error) => {
            eprintln!("Desktop token resolution failed: {error}");
            std::process::exit(2);
        }
    };
    // The Runtime credential is resolved once: the environment override wins,
    // then the protected store (MP-SEC1). It is never rendered, logged or placed
    // in a diagnostic; a 401 on this session deletes the stored entry.
    let runtime_token = std::env::var(runtime::TOKEN_ENV).ok();
    let credential_store = credentials::default_store();
    // One bounded acquisition/rotation boundary per session. The issuance secret
    // is read only for a real Runtime launch (a fixture run never touches the
    // variable), used only on the validated loopback origin, and then dropped.
    // The fetched token is persisted by the protected store and never rendered.
    // A failure is an explicit fixed label, and `plan` below stays the only
    // launch decision.
    if matches!(&startup.mode, app::Mode::Runtime { .. })
        && let Some(configured) = endpoint.as_deref()
        && let Some(origin) = runtime::validated_origin(configured)
    {
        let issuance_secret = std::env::var(credential_issuance::ISSUANCE_SECRET_ENV).ok();
        let mut exchange = credential_issuance::UreqExchange::new(&origin);
        if let Err(error) = credential_issuance::prepare(
            &origin,
            runtime_token.as_deref(),
            issuance_secret.as_deref(),
            credential_store.as_ref(),
            &mut exchange,
            std::time::SystemTime::now(),
        ) {
            eprintln!(
                "Runtime credential acquisition unavailable: {}",
                error.label()
            );
        }
        drop(issuance_secret);
    }
    let launch = runtime::plan(
        &startup.mode,
        endpoint.as_deref(),
        runtime_token.as_deref(),
        credential_store.as_ref(),
    );
    drop(runtime_token);
    let observation_store = std::sync::Arc::clone(&credential_store);
    let outcome = std::sync::Arc::clone(&exit_outcome);
    let teardown: std::sync::Arc<std::sync::Mutex<Option<runtime::RuntimeTeardown>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let teardown_for_app = std::sync::Arc::clone(&teardown);
    #[cfg(all(feature = "r0-s2b-instrumentation", windows))]
    let capture = r0_s2b::Report::default();
    #[cfg(all(feature = "r0-s2b-instrumentation", windows))]
    let capture_for_app = capture.clone();
    gpui_platform::application().run(move |cx: &mut App| {
        #[cfg(all(feature = "r0-s2b-instrumentation", windows))]
        if r0_s2b::install(cx, capture_for_app).is_err() {
            eprintln!("R0 S2B observer installation failed");
            cx.quit();
            return;
        }
        desktop_gpui::bind_keys(cx);
        let (handle, core, host) = match launch {
            runtime::LaunchPlan::Fixture { path } => match MockCosClient::from_fixture_file(path) {
                Ok(client) => {
                    let scope = client.client().transport().scenario().observation_request();
                    let client: Box<dyn DesktopClient> = Box::new(client);
                    let (host, handle) = window::ObservationHost::start(cx, client, scope);
                    match handle {
                        Some(handle) => (
                            Some(handle.clone()),
                            AppCore::with_observation(handle),
                            Some(host),
                        ),
                        None => (None, AppCore::disconnected(false), Some(host)),
                    }
                }
                Err(_) => {
                    eprintln!("Fixture load failed; no Runtime fallback");
                    cx.quit();
                    return;
                }
            },
            runtime::LaunchPlan::Runtime(settings) => {
                match runtime::observation(settings, std::sync::Arc::clone(&observation_store)) {
                    Ok(observation) => {
                        let runtime::RuntimeObservation {
                            client,
                            scope,
                            teardown: observation_teardown,
                        } = observation;
                        if let Some(session_teardown) = observation_teardown
                            && let Ok(mut slot) = teardown_for_app.lock()
                        {
                            *slot = Some(session_teardown);
                        }
                        // The same host drives either source; the UI never learns
                        // which client implementation it is.
                        let (host, handle) = window::ObservationHost::start(cx, client, scope);
                        match handle {
                            Some(handle) => (
                                Some(handle.clone()),
                                AppCore::with_observation(handle),
                                Some(host),
                            ),
                            None => (None, AppCore::disconnected(true), Some(host)),
                        }
                    }
                    Err(reason) => {
                        // Explicitly unavailable, never a silent fixture fallback.
                        eprintln!("Runtime observation unavailable: {}", reason.label());
                        (None, AppCore::disconnected(true), None)
                    }
                }
            }
            runtime::LaunchPlan::RuntimeUnavailable {
                endpoint_configured,
                reason,
            } => {
                // Explicitly unavailable, never a silent fixture fallback.
                eprintln!("Runtime observation unavailable: {}", reason.label());
                (None, AppCore::disconnected(endpoint_configured), None)
            }
        };
        // Register the read-only High Contrast probe before the shell opens. The probe
        // never changes an OS setting; the operator toggles High Contrast manually.
        desktop_gpui::contrast::set_platform_probe(std::sync::Arc::new(contrast_probe::read));
        let opened = window::open(
            cx,
            move |window, cx, preferences| {
                Shell::new(
                    core,
                    handle,
                    window,
                    cx,
                    preferences,
                    tokens,
                    env!("CARGO_PKG_VERSION"),
                )
            },
            host,
            outcome,
        );
        if let Err(error) = opened {
            eprintln!("{error}");
            cx.quit();
        }
    });
    // The app loop has stopped. Emit the exit-handshake outcome before any
    // teardown work: the bounded revoke may take its full 1.5 s budget and may
    // fail, but it must never hide the confirmation lines.
    finish_shutdown(
        &exit_outcome,
        |line| eprintln!("{line}"),
        || {
            if let Ok(mut slot) = teardown.lock()
                && let Some(session_teardown) = slot.take()
            {
                session_teardown.revoke();
            }
            Ok::<(), &'static str>(())
        },
    );
    #[cfg(all(feature = "r0-s2b-instrumentation", windows))]
    if !capture.committed() {
        eprintln!("R0 S2B capture completion is unconfirmed");
        std::process::exit(1);
    }
}

/// The fixed exit-handshake lines for one report, in emission order.
fn exit_outcome_lines(report: &window::ExitReport) -> [&'static str; 2] {
    let observation = match report.observation {
        window::ObservationExit::Confirmed => "observation shutdown confirmed",
        window::ObservationExit::Unconfirmed => {
            "observation shutdown NOT confirmed within the exit budget"
        }
        window::ObservationExit::NotApplicable => {
            "observation shutdown not applicable (no observation driver)"
        }
        window::ObservationExit::NotObserved => {
            "observation shutdown not observed (quit callback not observed)"
        }
    };
    let preferences = match report.preferences {
        window::PreferencesExit::Saved => "preferences save confirmed",
        window::PreferencesExit::Failed => "preferences save failed",
        window::PreferencesExit::Unconfirmed => {
            "preferences save NOT confirmed within the exit budget"
        }
        window::PreferencesExit::NotObserved => {
            "preferences save not observed (quit callback not observed)"
        }
    };
    [observation, preferences]
}

/// Emit the exit handshake first, then run the bounded teardown revoke.
///
/// The order is the contract: both confirmation lines are written before the
/// revoke can block for its budget or fail, so a slow or failing revoke can never
/// hide the shutdown evidence. The revoke result is ignored here because the
/// revoke path reports its own fixed label.
fn finish_shutdown<F, G>(
    exit_outcome: &std::sync::Arc<std::sync::Mutex<window::ExitReport>>,
    mut emit: G,
    revoke: F,
) where
    F: FnOnce() -> Result<(), &'static str>,
    G: FnMut(&'static str),
{
    match exit_outcome.lock() {
        Ok(report) => {
            for line in exit_outcome_lines(&report) {
                emit(line);
            }
        }
        Err(_) => emit("Desktop shutdown outcomes unavailable"),
    }
    let _ = revoke();
}

#[cfg(test)]
mod tests {
    use super::finish_shutdown;
    use crate::window::{ExitOutcome, ExitReport, ObservationExit, PreferencesExit};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn confirmed() -> ExitOutcome {
        Arc::new(Mutex::new(ExitReport {
            observation: ObservationExit::Confirmed,
            preferences: PreferencesExit::Saved,
        }))
    }

    #[test]
    fn the_exit_handshake_is_observable_before_a_slow_revoke_returns() {
        let outcome = confirmed();
        let emitted = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let seen_before_revoke = Arc::new(Mutex::new(None::<usize>));
        let emitted_for_revoke = Arc::clone(&emitted);
        let seen = Arc::clone(&seen_before_revoke);
        let started = Instant::now();
        finish_shutdown(
            &outcome,
            {
                let emitted = Arc::clone(&emitted);
                move |line| emitted.lock().expect("emitted").push(line)
            },
            move || {
                *seen.lock().expect("seen") =
                    Some(emitted_for_revoke.lock().expect("emitted").len());
                std::thread::sleep(Duration::from_millis(250));
                Ok(())
            },
        );
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the slow revoke really ran"
        );
        assert_eq!(
            *seen_before_revoke.lock().expect("seen"),
            Some(2),
            "both confirmation lines were emitted before the revoke started"
        );
        assert_eq!(
            emitted.lock().expect("emitted").as_slice(),
            [
                "observation shutdown confirmed",
                "preferences save confirmed"
            ]
        );
    }

    #[test]
    fn a_failing_revoke_cannot_suppress_the_exit_handshake() {
        let outcome = confirmed();
        let emitted = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let emitted_for_revoke = Arc::clone(&emitted);
        finish_shutdown(
            &outcome,
            {
                let emitted = Arc::clone(&emitted);
                move |line| emitted.lock().expect("emitted").push(line)
            },
            move || {
                assert_eq!(
                    emitted_for_revoke.lock().expect("emitted").len(),
                    2,
                    "the handshake is already observable when the revoke fails"
                );
                Err("revoke failed")
            },
        );
        assert_eq!(emitted.lock().expect("emitted").len(), 2);
    }

    #[test]
    fn a_poisoned_report_lock_still_precedes_the_revoke() {
        let outcome = Arc::new(Mutex::new(ExitReport::default()));
        let poison = Arc::clone(&outcome);
        let _ = std::thread::spawn(move || {
            let _guard = poison.lock().expect("poison lock");
            panic!("poison the report lock");
        })
        .join();
        assert!(outcome.lock().is_err(), "the report lock is poisoned");
        let sink = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let sink_for_revoke = Arc::clone(&sink);
        finish_shutdown(
            &outcome,
            {
                let sink = Arc::clone(&sink);
                move |line| sink.lock().expect("sink").push(line)
            },
            move || {
                assert_eq!(
                    sink_for_revoke.lock().expect("sink").as_slice(),
                    ["Desktop shutdown outcomes unavailable"]
                );
                Ok(())
            },
        );
    }
}
