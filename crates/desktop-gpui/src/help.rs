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
    cx: &mut App,
) -> AnyView {
    cx.new(|_| Explanation {
        text,
        style,
        max_width,
    })
    .into()
}

struct Explanation {
    text: SharedString,
    style: TextSpec,
    max_width: Pixels,
}

impl Render for Explanation {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.style
            .clone()
            .apply(div())
            .id("control-explanation")
            .role(Role::Label)
            .aria_label(self.text.clone())
            .max_w(self.max_width)
            .p_3()
            .border_1()
            .border_color(rgb(0x626870))
            .bg(rgb(0x24272c))
            .text_color(rgb(0xe6e8eb))
            .child(self.text.clone())
    }
}
