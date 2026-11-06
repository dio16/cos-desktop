//! Explicit local fixture only: `cargo run --release -p cos-desktop --example image_viewer -- --fixture`
//! `--preview` chooses bounded preview-only mode. Neither mode is Runtime/product acceptance.
#[path = "support/image_fixture.rs"]
mod image_fixture;
use desktop_gpui::image_viewer::{ImageViewer, ViewerMode};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, Render, Role, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use std::sync::Arc;

struct Demo {
    viewer: Entity<ImageViewer>,
    revision: u64,
}
impl Demo {
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revision = self.revision.saturating_add(1);
        let revision = self.revision;
        match image_fixture::fixture_request_for("landmark-png", revision, 1) {
            Ok(request) => self.viewer.update(cx, |viewer, cx| {
                viewer.load_generated(request, window, cx, move |cancel| {
                    image_fixture::generate_fixture_png(revision, cancel).map(Arc::<[u8]>::from)
                })
            }),
            Err(error) => eprintln!("fixture request refused: {error}"),
        }
    }
}
impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child("Fixture — 2048×2048 RGB8 PNG / ローカル実験。実行基盤の成果物ではありません。")
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        div()
                            .id("next-revision")
                            .role(Role::Button)
                            .aria_label("次の画像版を読み込む")
                            .p_2()
                            .border_1()
                            .child("次の版（保持上限に達すると利用不可）")
                            .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                    )
                    .child(
                        div()
                            .id("viewer-close")
                            .role(Role::Button)
                            .aria_label("画像を閉じて終了")
                            .p_2()
                            .border_1()
                            .child("画像を閉じて終了")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.viewer.update(cx, |viewer, cx| viewer.close(cx));
                                window.remove_window();
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.viewer.clone()))
    }
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.as_slice() {
        [arg] if arg == "--fixture" => ViewerMode::ExperimentalTiles,
        [arg] if arg == "--preview" => ViewerMode::PreviewOnly,
        _ => {
            eprintln!("Usage: image_viewer --fixture | --preview");
            std::process::exit(2);
        }
    };
    gpui_platform::application().run(move |cx: &mut App| {
        desktop_gpui::image_viewer::bind_keys(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1100.0), px(800.0)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("MP-IMG5 Local Fixture Viewer".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let viewer = cx.new(|cx| ImageViewer::new(mode, window, cx));
                let weak = viewer.downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    let _ = weak.update(cx, |viewer, cx| viewer.close(cx));
                    true
                });
                cx.new(|cx| {
                    let mut demo = Demo {
                        viewer,
                        revision: 0,
                    };
                    demo.load(window, cx);
                    demo
                })
            },
        );
        if result.is_err() {
            eprintln!("native viewer unavailable");
            cx.quit();
        }
    });
}
