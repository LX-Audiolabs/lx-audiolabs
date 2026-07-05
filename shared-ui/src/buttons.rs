//! Shared button styles — amber-when-active toggle, dark-red "danger" (reset),
//! and small variants used near-identically across all plugins before this
//! was extracted. One pixel/color tweak here now applies everywhere instead
//! of N inline copies (the pre-Vizia iced port had this as `toggle_button`/
//! `output_tools_strip` in shared-ui, never carried over during the port).
//!
//! Reactivity (re-reading the param's active state on change) stays the
//! caller's job via `Binding`/`Memo` — each plugin's `ParamLens<P>` is a
//! different generic type, not worth threading through here. This module
//! only builds the styled `Button` itself.

use vizia::prelude::*;

pub const BUTTON_HEIGHT: f32 = 22.0;
pub const BUTTON_HEIGHT_SMALL: f32 = 18.0;
pub const KNOB_SIZE: f32 = 40.0;
pub const SLIDER_HEIGHT: f32 = 20.0;

pub const AMBER: Color = Color::rgb(255, 115, 26);
pub const IDLE_BG: Color = Color::rgb(38, 38, 38);
pub const DANGER_BG: Color = Color::rgb(51, 20, 20);
pub const DANGER_TEXT: Color = Color::rgb(230, 128, 128);

/// Amber-when-active, dark-grey-when-inactive toggle — Bypass, Mono, Delta,
/// Solo, Listen, Pre-Master, EQ band-type, mode-cycle buttons, etc.
pub fn toggle_button<'a>(
    cx: &'a mut Context,
    label: &'static str,
    active: bool,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(11.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT))
        .background_color(if active { AMBER } else { IDLE_BG })
}

/// Same colors, smaller footprint — slope selectors (A/B/C), SPLIT/CLIP.
pub fn toggle_button_small<'a>(
    cx: &'a mut Context,
    label: &'static str,
    active: bool,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(9.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT_SMALL))
        .background_color(if active { AMBER } else { IDLE_BG })
}

/// Dark-red — RESET is the one button that's deliberately not amber-when-active.
pub fn danger_button<'a>(
    cx: &'a mut Context,
    label: &'static str,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(11.0).color(DANGER_TEXT))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT))
        .background_color(DANGER_BG)
}
