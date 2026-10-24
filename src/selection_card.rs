//! Shared radio-card presentation for Settings and first-run setup.

use gpui_kit::base::{Radio, RadioGroup};
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Axis, Div, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, div, px, rgb,
};

pub(crate) fn group(id: impl Into<ElementId>) -> RadioGroup {
    RadioGroup::new(id)
        .axis(Axis::Horizontal)
        .flex()
        .flex_row()
        .items_stretch()
        .gap_3()
        .w_full()
}

/// Callers supply selection/focus handlers and content, including optional status text.
pub(crate) fn radio(id: impl Into<ElementId>, selected: bool, disabled: bool, cx: &App) -> Radio {
    let ring = cx.theme().ring;
    let indicator = if disabled {
        cx.theme().muted_foreground.opacity(0.5)
    } else if selected {
        ring
    } else {
        cx.theme().muted_foreground
    };
    Radio::new(id)
        .checked(selected)
        .disabled(disabled)
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .flex_1()
        .min_w_0()
        .px_3()
        .py_3()
        .rounded_md()
        .border_1()
        .border_color(if selected { ring } else { cx.theme().border })
        .bg(if selected {
            cx.theme().accent
        } else {
            cx.theme().background
        })
        .when(!disabled, |card| {
            card.cursor_pointer()
                .hover(move |style| style.border_color(ring))
                .focus(|style| style.border_color(rgb(0x9acfff)))
        })
        .child(
            div()
                .size(px(16.))
                .mt(px(2.))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(indicator)
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |circle| {
                    circle.child(div().size(px(7.)).rounded_full().bg(indicator))
                }),
        )
}

pub(crate) fn content(icon: impl IntoElement, title: &str, description: &str, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .flex_1()
        .min_w_0()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(div().flex_shrink_0().child(icon))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(title.to_owned()),
                ),
        )
        .child(
            div()
                .text_xs()
                .whitespace_normal()
                .text_color(cx.theme().muted_foreground)
                .child(description.to_owned()),
        )
}
