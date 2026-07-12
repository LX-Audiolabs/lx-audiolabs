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
pub const BUTTON_HEIGHT_BIG: f32 = 30.0;
pub const KNOB_SIZE: f32 = 40.0;
pub const SLIDER_HEIGHT: f32 = 20.0;
pub const STEREO_METER_HEIGHT: f32 = 180.0;

pub const AMBER: Color = Color::rgb(255, 115, 26);
pub const IDLE_BG: Color = Color::rgb(38, 38, 38);
pub const DANGER_BG: Color = Color::rgb(51, 20, 20);
pub const DANGER_TEXT: Color = Color::rgb(230, 128, 128);

/// CSS theme for the shared button builders.
///
/// Inline `background_color`/`color` modifiers override CSS pseudo-states in
/// Vizia, so these builders rely on classes for idle/hover/active states.
/// Load once per `Context` with [`load_theme`].
const BUTTON_CSS: &str = r#"
.lx-btn {
    background-color: #262626;
    color: #ffffff;
    transition: background-color 100ms;
}
.lx-btn:hover {
    background-color: #404040;
}
.lx-btn:active {
    background-color: #555555;
}
.lx-btn.active {
    background-color: #ff731a;
    color: #ffffff;
}
.lx-btn.active:hover {
    background-color: #ff8c3f;
}
.lx-btn.active:active {
    background-color: #ffa05a;
}
.lx-btn.danger {
    background-color: #331414;
    color: #e68080;
}
.lx-btn.danger:hover {
    background-color: #4a1c1c;
}
.lx-btn.danger:active {
    background-color: #5c2020;
}
.lx-btn.danger.active {
    background-color: #cc2222;
    color: #ffffff;
}
.lx-btn.danger.active:hover {
    background-color: #dd3333;
}
.lx-btn.danger.active:active {
    background-color: #ee4444;
}
.lx-btn.amber-text {
    color: #ff731a;
}
.lx-btn.active.amber-text {
    color: #ffffff;
}
.lx-btn.disabled {
    background-color: #1a1a1a;
    color: #555555;
}
.lx-btn.disabled:hover {
    background-color: #1a1a1a;
}
"#;

/// Add the shared button stylesheet to the current `Context`.
/// Call once at the top of each plugin editor's `build()` function.
pub fn load_theme(cx: &mut Context) {
    let _ = cx.add_stylesheet(BUTTON_CSS);
}

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
        .class("lx-btn")
        .toggle_class("active", active)
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
        .class("lx-btn")
        .toggle_class("active", active)
}

/// Red-when-active small toggle — MASKING ON/OFF in Lucent.
pub fn toggle_button_small_danger<'a>(
    cx: &'a mut Context,
    label: &'static str,
    active: bool,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(9.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT_SMALL))
        .class("lx-btn")
        .class("danger")
        .toggle_class("active", active)
}

/// Dark-red — RESET is the one button that's deliberately not amber-when-active.
pub fn danger_button<'a>(
    cx: &'a mut Context,
    label: &'static str,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(11.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT))
        .class("lx-btn")
        .class("danger")
}

/// Big amber-when-active toggle — LISTEN, SOLO (footer / main panel).
pub fn toggle_button_big<'a>(
    cx: &'a mut Context,
    label: &'static str,
    active: bool,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(12.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT_BIG))
        .class("lx-btn")
        .toggle_class("active", active)
}

/// Big amber-text-always toggle — LISTEN in footer: text stays amber when
/// inactive so it's visually distinct, but switches to white when active so
/// it doesn't disappear against the amber background.
pub fn toggle_button_big_amber_text<'a>(
    cx: &'a mut Context,
    label: &'static str,
    active: bool,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(12.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT_BIG))
        .class("lx-btn")
        .class("amber-text")
        .toggle_class("active", active)
}

/// Big plain push-button — APPLY ANALYSIS, RESET ANALYSIS, SAVE, VAULT SETUP.
pub fn push_button_big<'a>(
    cx: &'a mut Context,
    label: &'static str,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, |cx| Label::new(cx, label).font_size(12.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT_BIG))
        .class("lx-btn")
}

/// Big dark-red danger button — RESET in footer.
pub fn danger_button_big<'a>(
    cx: &'a mut Context,
    label: &'static str,
    on_press: impl Fn(&mut EventContext) + 'static + Send + Sync,
) -> Handle<'a, impl View> {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(12.0))
        .on_press(on_press)
        .height(Pixels(BUTTON_HEIGHT_BIG))
        .class("lx-btn")
        .class("danger")
}
