mod app;
mod instance;
#[cfg(all(feature = "r0-s2b-instrumentation", windows))]
mod r0_s2b;
mod window;
#[cfg(all(feature = "r0-s2b-instrumentation", not(windows)))]
compile_error!("r0-s2b-instrumentation currently supports native Windows only");

use app::{Mode, Startup};
use app_core::AppCore;
use cos_client::{DesktopClient, MockCosClient};
use desktop_gpui::Shell;
use gpui::App;
use std::path::PathBuf;

fn main() {
    let exit_outcome: window::ExitOutcome =
        std::sync::Arc::new(std::sync::Mutex::new(window::ExitReport::default()));
    let default_fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/cos-client/fixtures/task-184.json");
    let startup = match Startup::parse(
        std::env::args().skip(1),
        std::env::var("COS_DESKTOP_RUNTIME_ENDPOINT").ok(),
        default_fixture,
    ) {
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
        Err(instance::AcquireError::Unavailable) => {
            eprintln!("COS Desktop instance ownership unavailable; startup refused");
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
    let outcome = std::sync::Arc::clone(&exit_outcome);
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
        let (handle, core, host) = match startup.mode {
            Mode::Runtime {
                endpoint_configured,
            } => {
                // No verified adapter exists yet: explicitly unavailable, never a
                // silent fallback to fixture data.
                (None, AppCore::disconnected(endpoint_configured), None)
            }
            Mode::Fixture { path } => match MockCosClient::from_fixture_file(path) {
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
        };
        let opened = window::open(
            cx,
            move |window, cx, preferences| {
                Shell::new(core, handle, window, cx, preferences, tokens)
            },
            host,
            outcome,
        );
        if let Err(error) = opened {
            eprintln!("{error}");
            cx.quit();
        }
    });
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
