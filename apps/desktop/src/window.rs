use app_core::preferences::WindowPreferences;
use desktop_gpui::Shell;
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

pub fn open(
    cx: &mut App,
    shell: impl FnOnce(&mut gpui::Window, &mut gpui::Context<Shell>) -> Shell + 'static,
) -> Result<(), String> {
    let preferences = WindowPreferences::default();
    let bounds = Bounds::centered(
        None,
        size(px(preferences.width), px(preferences.height)),
        cx,
    );
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("COS Desktop".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(1000.0), px(680.0))),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| shell(window, cx)),
    )
    .map_err(|_| "Native window creation failed".to_owned())?;
    cx.activate(true);
    Ok(())
}
