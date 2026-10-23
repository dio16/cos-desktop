mod app;
mod window;

use app::{Mode, Startup};
use app_core::AppCore;
use cos_client::MockCosClient;
use desktop_gpui::Shell;
use gpui::App;
use std::path::PathBuf;

fn main() {
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
    let fixture = matches!(startup.mode, Mode::Fixture { .. });
    gpui_platform::application().run(move |cx: &mut App| {
        desktop_gpui::bind_keys(cx);
        let load = cx.background_executor().spawn(async move {
            match startup.mode {
                Mode::Runtime {
                    endpoint_configured,
                } => Ok(AppCore::disconnected(endpoint_configured)),
                Mode::Fixture { path } => {
                    let mut client = MockCosClient::from_fixture_file(path)
                        .map_err(|_| "Fixture load failed; no Runtime fallback".to_owned())?;
                    client
                        .connect(0)
                        .map_err(|_| "Fixture negotiation failed".to_owned())?;
                    client.pump(0);
                    Ok(AppCore::with_client(Box::new(client)))
                }
            }
        });
        if let Err(error) =
            window::open(cx, move |window, cx| Shell::new(fixture, load, window, cx))
        {
            eprintln!("{error}");
            cx.quit();
        }
    });
}
