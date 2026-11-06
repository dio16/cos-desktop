//! Lazy, read-only explanations for existing controls.

use gpui::{
    AnyView, App, Context, IntoElement, Pixels, Render, Role, SharedString, Window, div,
    prelude::*, rgb,
};

use crate::shell_style::TextSpec;

pub(crate) fn tooltip(
    text: SharedString,
    style: TextSpec,
    max_width: Pixels,
    palette: crate::contrast::Palette,
    cx: &mut App,
) -> AnyView {
    cx.new(|_| Explanation {
        text,
        style,
        max_width,
        palette,
    })
    .into()
}

struct Explanation {
    text: SharedString,
    style: TextSpec,
    max_width: Pixels,
    palette: crate::contrast::Palette,
}

impl Render for Explanation {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let surface = self
            .palette
            .color(0x24272c, self.palette.colors.raised_face);
        self.style
            .clone()
            .apply(div())
            .id("control-explanation")
            .role(Role::Label)
            .aria_label(self.text.clone())
            .max_w(self.max_width)
            .p_3()
            .border_1()
            .border_color(rgb(self.palette.colors.border))
            .bg(rgb(surface))
            .text_color(rgb(self
                .palette
                .readable(self.palette.colors.text, surface)))
            .child(self.text.clone())
    }
}
