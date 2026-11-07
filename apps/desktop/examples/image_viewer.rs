//! Explicit local fixture only: `cargo run --release -p cos-desktop --example image_viewer -- --fixture`
//! `--preview` chooses bounded preview-only mode. Neither mode is Runtime/product acceptance.
#[path = "support/image_fixture.rs"]
mod image_fixture;
use desktop_gpui::image_viewer::{ImageViewer, ViewerMode};
#[cfg(feature = "gpu-recovery-test")]
use gpui::GpuDeviceRecoveryTest;
use gpui::{
    App, AppContext, Bounds, Context, Entity, GpuDeviceState, IntoElement, Render, Role,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use std::sync::Arc;

struct Demo {
    viewer: Entity<ImageViewer>,
    revision: u64,
    status: SharedString,
}

fn device_reason(state: GpuDeviceState) -> SharedString {
    match state {
        GpuDeviceState::Unsupported => {
            "GPU表示を利用できません: このバックエンドでは画像表示が未対応です。".into()
        }
        GpuDeviceState::Recovering { generation } => {
            format!("GPUデバイスを復旧中です（世代 {generation}）。").into()
        }
        GpuDeviceState::Unavailable { generation } => {
            format!("GPU表示を利用できません（世代 {generation}）。アプリを再起動してください。")
                .into()
        }
        GpuDeviceState::Ready { generation } => {
            format!("画像要求にはGPU世代 {generation} の準備が必要です。").into()
        }
    }
}
impl Demo {
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revision = self.revision.saturating_add(1);
        let revision = self.revision;
        let generation = match cx.gpu_device_state() {
            GpuDeviceState::Ready { generation } => generation,
            state => {
                self.viewer.update(cx, |viewer, cx| {
                    viewer.unavailable(device_reason(state), cx)
                });
                return;
            }
        };
        match image_fixture::fixture_request_for("landmark-png", revision, generation) {
            Ok(request) => self.viewer.update(cx, |viewer, cx| {
                viewer.load_generated(request, window, cx, move |cancel| {
                    image_fixture::generate_fixture_png(revision, cancel).map(Arc::<[u8]>::from)
                })
            }),
            Err(error) => {
                self.status = format!("Fixture request refused: {error}").into();
                cx.notify();
            }
        }
    }

    #[cfg(feature = "gpu-recovery-test")]
    fn request_backend_recovery(
        &mut self,
        mode: GpuDeviceRecoveryTest,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.status = if cx.request_gpu_device_recreation(mode) {
            label.into()
        } else {
            "Test-support backend recreation is unsupported here.".into()
        };
        cx.notify();
    }

    fn render_controls(&self, cx: &Context<Self>) -> impl IntoElement {
        #[cfg(feature = "gpu-recovery-test")]
        {
            div()
                .flex()
                .gap_3()
                .child(
                    div()
                        .id("gpu-recreate")
                        .role(Role::Button)
                        .aria_label("テスト支援: GPUバックエンドを再作成（物理リセットではありません）")
                        .p_2()
                        .border_1()
                        .child("Backend: Recreate — test-support only; not a physical reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.request_backend_recovery(
                                GpuDeviceRecoveryTest::Recreate,
                                "Backend recreation requested; test-support only, not a physical reset.",
                                cx,
                            )
                        })),
                )
                .child(
                    div()
                        .id("gpu-fail")
                        .role(Role::Button)
                        .aria_label("テスト支援: GPUバックエンド再作成の制御失敗（物理リセットではありません）")
                        .p_2()
                        .border_1()
                        .child("Backend: Fail — controlled failure; not a physical reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.request_backend_recovery(
                                GpuDeviceRecoveryTest::Fail,
                                "Controlled backend factory failure requested; not a physical reset.",
                                cx,
                            )
                        })),
                )
        }
        #[cfg(not(feature = "gpu-recovery-test"))]
        {
            let _ = cx;
            div().child("Backend recovery controls require --features gpu-recovery-test")
        }
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.clone();
        let recovery_controls = self.render_controls(cx);
        div()
            .size_full()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child("Fixture — 2048×2048 RGB8 PNG / ローカル実験。実行基盤の成果物ではありません。")
            .child(status)
            .child(recovery_controls)
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
                let viewer =
                    cx.new(|cx| ImageViewer::new(mode, "MP-IMG5 Local Fixture Viewer", window, cx));
                let weak = viewer.downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    let _ = weak.update(cx, |viewer, cx| viewer.close(cx));
                    true
                });
                cx.new(|cx| {
                    let mut demo = Demo {
                        viewer,
                        revision: 0,
                        status:
                            "Native GPU state is read from GPUI; Runtime input remains unavailable."
                                .into(),
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
