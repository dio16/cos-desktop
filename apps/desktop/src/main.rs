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
    // is read once and used only on the validated loopback origin; the fetched
    // token is persisted by the protected store and never rendered. A failure is
    // an explicit fixed label, and `plan` below stays the only launch decision.
    let issuance_secret = std::env::var(credential_issuance::ISSUANCE_SECRET_ENV).ok();
    if matches!(&startup.mode, app::Mode::Runtime { .. })
        && let Some(configured) = endpoint.as_deref()
        && let Some(origin) = runtime::validated_origin(configured)
    {
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
    }
    drop(issuance_secret);
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
    // The app loop has stopped. Revoke the session credential best-effort inside
    // its bounded budget; failures are fixed labels and never extend shutdown.
    if let Ok(mut slot) = teardown.lock()
        && let Some(session_teardown) = slot.take()
    {
        session_teardown.revoke();
    }
    // Report the exit handshake outcome after the app loop returned, so the
    // confirmation is observable even when the process quits immediately.
    match exit_outcome.lock() {
        Ok(report) => {
            match report.observation {
                window::ObservationExit::Confirmed => eprintln!("observation shutdown confirmed"),
                window::ObservationExit::Unconfirmed => {
                    eprintln!("observation shutdown NOT confirmed within the exit budget");
                }
                window::ObservationExit::NotApplicable => {
                    eprintln!("observation shutdown not applicable (no observation driver)");
                }
                window::ObservationExit::NotObserved => {
                    eprintln!("observation shutdown not observed (quit callback not observed)");
                }
            }
            match report.preferences {
                window::PreferencesExit::Saved => eprintln!("preferences save confirmed"),
                window::PreferencesExit::Failed => eprintln!("preferences save failed"),
                window::PreferencesExit::Unconfirmed => {
                    eprintln!("preferences save NOT confirmed within the exit budget");
                }
                window::PreferencesExit::NotObserved => {
                    eprintln!("preferences save not observed (quit callback not observed)");
                }
            }
        }
        Err(_) => eprintln!("Desktop shutdown outcomes unavailable"),
    }
    #[cfg(all(feature = "r0-s2b-instrumentation", windows))]
    if !capture.committed() {
        eprintln!("R0 S2B capture completion is unconfirmed");
        std::process::exit(1);
    }
}
