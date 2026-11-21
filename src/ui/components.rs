//! Small stateless building blocks shared by the views. Colours come from the active palette.

use gpui::{
    Div, ElementId, FontWeight, Hsla, SharedString, Stateful, Svg, div, prelude::*, px, rgb, svg,
};

use crate::assets::Icon;
use crate::theme::p;

pub fn with_alpha(hex: u32, alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(hex).into();
    color.a = alpha;
    color
}

pub fn icon(icon: Icon, size: f32, color: u32) -> Svg {
    svg().path(icon.path()).size(px(size)).flex_none().text_color(rgb(color))
}

pub fn letter_tile(name: &str, key: &str, size: f32) -> Div {
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    let hash = key.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let tint = p().series[hash as usize % p().series.len()];
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(with_alpha(tint, 0.14))
        .border_1()
        .border_color(with_alpha(tint, 0.35))
        .text_color(rgb(tint))
        .font_weight(FontWeight::BOLD)
        .text_size(px(size * 0.45))
        .child(letter)
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonStyle {
    Primary,
    Danger,
    Secondary,
    Ghost,
    Disabled,
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, style: ButtonStyle) -> Stateful<Div> {
    icon_button(id, None, label, style)
}

pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
    style: ButtonStyle,
) -> Stateful<Div> {
    let p = p();
    let (fg, padding_y) = match style {
        ButtonStyle::Primary => (p.on_accent, 2.0),
        ButtonStyle::Danger => (p.on_danger, 2.0),
        ButtonStyle::Secondary => (p.text, 2.0),
        ButtonStyle::Ghost | ButtonStyle::Disabled => (p.muted, 1.5),
    };
    let base = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded_md()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .when(padding_y > 0.0, |b| b.px_3().py(px(padding_y * 4.0)))
        .children(glyph.map(|g| icon(g, 16.0, fg)))
        .child(label.into());
    match style {
        ButtonStyle::Primary => base
            .cursor_pointer()
            .bg(rgb(p.accent))
            .hover(|b| b.opacity(0.9)),
        ButtonStyle::Danger => base
            .cursor_pointer()
            .bg(rgb(p.danger))
            .hover(|b| b.bg(rgb(p.danger_hover))),
        ButtonStyle::Secondary => base
            .cursor_pointer()
            .bg(rgb(p.raised))
            .border_1()
            .border_color(rgb(p.border_strong))
            .hover(|b| b.bg(rgb(p.hover))),
        ButtonStyle::Ghost => base
            .cursor_pointer()
            .hover(|b| b.bg(rgb(p.hover)).text_color(rgb(p.text))),
        // Clicks are still delivered; callers ignore them while busy.
        ButtonStyle::Disabled => base.bg(rgb(p.raised)),
    }
}

/// A square, icon-only button (close, per-row delete).
pub fn square_button(id: impl Into<ElementId>, glyph: Icon, size: f32, tint: u32) -> Stateful<Div> {
    let p = p();
    div()
        .id(id)
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover(|b| b.bg(rgb(p.hover)))
        .child(icon(glyph, size * 0.55, tint))
}

#[derive(Clone, Copy, PartialEq)]
pub enum CheckState {
    Unchecked,
    Checked,
    Mixed,
}

pub fn checkbox(id: impl Into<ElementId>, state: CheckState) -> Stateful<Div> {
    let p = p();
    let base = div()
        .id(id)
        .size(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .border_1()
        .cursor_pointer();
    match state {
        CheckState::Checked => base
            .bg(rgb(p.accent))
            .border_color(rgb(p.accent))
            .child(icon(Icon::Check, 13.0, p.on_accent)),
        CheckState::Mixed => base
            .bg(rgb(p.accent))
            .border_color(rgb(p.accent))
            .child(div().w(px(8.0)).h(px(2.0)).rounded_full().bg(rgb(p.on_accent))),
        CheckState::Unchecked => base
            .border_color(rgb(p.muted))
            .hover(|c| c.border_color(rgb(p.accent))),
    }
}

pub fn switch(on: bool) -> Div {
    let p = p();
    div()
        .w(px(32.0))
        .h(px(18.0))
        .p(px(2.0))
        .flex()
        .flex_none()
        .rounded_full()
        .when(on, |track| track.bg(rgb(p.accent)).justify_end())
        .when(!on, |track| track.bg(rgb(p.border_strong)))
        .child(div().size(px(14.0)).rounded_full().bg(rgb(0xffffff)).shadow_sm())
}

/// A top-bar tab: icon and label, underlined when active. Fills the bar's height so the
/// underline sits on the bar's bottom edge.
pub fn tab(id: impl Into<ElementId>, glyph: Icon, label: &'static str, active: bool) -> Stateful<Div> {
    let p = p();
    let ink = if active { p.text } else { p.muted };
    div()
        .id(id)
        .relative()
        .h_full()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(ink))
        .when(active, |t| t.font_weight(FontWeight::MEDIUM))
        .when(!active, |t| t.hover(|t| t.text_color(rgb(p.text))))
        .child(icon(glyph, 16.0, ink))
        .child(label)
        .child(
            div()
                .absolute()
                .bottom_0()
                .left(px(8.0))
                .right(px(8.0))
                .h(px(2.0))
                .rounded_full()
                .when(active, |line| line.bg(rgb(p.accent))),
        )
}

pub fn badge(label: &'static str, tint: u32) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .bg(with_alpha(tint, 0.12))
        .border_1()
        .border_color(with_alpha(tint, 0.45))
        .text_color(rgb(tint))
        .text_xs()
        .child(label)
}

/// A tinted note. Warnings and errors get an alert icon; anything else is informational.
pub fn callout(text: String, tint: u32) -> Div {
    let p = p();
    let glyph = if tint == p.warning || tint == p.danger { Icon::Alert } else { Icon::Info };
    div()
        .flex()
        .items_start()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(with_alpha(tint, 0.08))
        .border_1()
        .border_color(with_alpha(tint, 0.5))
        .text_sm()
        .child(div().mt(px(2.0)).child(icon(glyph, 15.0, tint)))
        .child(div().flex_1().min_w_0().child(text))
}

/// An icon-only button with the secondary (outlined) look, sized to sit beside a button.
pub fn outlined_icon_button(id: impl Into<ElementId>, glyph: Icon) -> Stateful<Div> {
    let p = p();
    div()
        .id(id)
        .size(px(36.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(rgb(p.raised))
        .border_1()
        .border_color(rgb(p.border_strong))
        .cursor_pointer()
        .hover(|b| b.bg(rgb(p.hover)))
        .child(icon(glyph, 16.0, p.text))
}
