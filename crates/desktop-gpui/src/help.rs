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
    scheme: crate::contrast::ContrastScheme,
    colors: crate::contrast::ShellColors,
    cx: &mut App,
) -> AnyView {
    cx.new(|_| Explanation {
        text,
        style,
        max_width,
        scheme,
        colors,
    })
    .into()
}

struct Explanation {
    text: SharedString,
    style: TextSpec,
    max_width: Pixels,
    scheme: crate::contrast::ContrastScheme,
    colors: crate::contrast::ShellColors,
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
            .border_color(rgb(self.colors.border))
            .bg(rgb(crate::contrast::role_color(
                self.scheme,
                0x24272c,
                self.colors.raised_face,
            )))
            .text_color(rgb(crate::contrast::readable_text(
                &self.colors,
                self.colors.text,
                crate::contrast::role_color(self.scheme, 0x24272c, self.colors.raised_face),
            )))
            .child(self.text.clone())
    }
}
