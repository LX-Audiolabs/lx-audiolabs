use truce_iced::iced::widget::canvas::{self, Geometry, Path, Stroke};
use truce_iced::iced::widget::{button, canvas as canvas_widget, column, container, row, Space, Text};
use truce_iced::iced::{Alignment, Border, Color, Element, Font, Length, Point, Rectangle};
use truce_iced::iced::mouse::Cursor;

pub fn bold_font() -> Font {
    Font {
        weight: truce_iced::iced::font::Weight::Bold,
        ..Font::default()
    }
}

/// Slider with right-click reset to default value. Canvas-based for amber styling
/// (cross-plugin consistency). Bipolar: fills from center toward value in amber.
/// Detected automatically: ① `min < 0 < max` → center=0 (Gain, Pan).
/// ② default between 25%–75% of range → center=default (Width 100%).
pub fn reset_slider<'a, Message: Clone + 'a>(
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    default: f32,
    step: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let min = *range.start();
    let max = *range.end();
    let span = max - min;
    let norm = if span.abs() < 1e-9 { 0.5 } else { ((value - min) / span).clamp(0.0, 1.0) };
    let default_norm = if span.abs() < 1e-9 { 0.5 } else { ((default - min) / span).clamp(0.0, 1.0) };
    // Bipolar detection: classic crossing-zero OR default roughly mid-range (25%–75%).
    let bipolar = (min < 0.0 && max > 0.0) || (default_norm > 0.25 && default_norm < 0.75);
    let center_norm = if min < 0.0 && max > 0.0 {
        ((0.0 - min) / span).clamp(0.0, 1.0)
    } else if bipolar {
        default_norm
    } else {
        0.0
    };
    canvas_widget(SliderProgram {
        value_norm: norm,
        default_norm,
        min,
        max,
        step,
        bipolar,
        center_norm,
        on_change: Box::new(move |v| Some(on_change(v))),
    })
    .width(Length::Fill)
    .height(Length::Fixed(18.0))
    .into()
}

/// Horizontal slider with full DAW-automation gestures (see [`Gesture`]). Press = `Start`
/// (begin), drag = `Change(v)` in real units (`min..=max`, linear), release = `End`, and
/// right-click = reset to `default`. Canvas-based for precise begin/end pairing — the
/// gesture-aware sibling of [`reset_slider`].
/// ponytail: a bare click (press+release, no drag) opens/closes an empty gesture and does
/// not jump the value; users drag. Add click-to-jump only if a plugin needs it.
pub fn hslider_gesture<'a, Message: Clone + 'a>(
    min: f32,
    max: f32,
    value: f32,
    default: f32,
    on_gesture: impl Fn(Gesture) -> Message + 'a,
) -> Element<'a, Message> {
    let span = max - min;
    let norm = if span.abs() < 1e-9 { 0.0 } else { ((value - min) / span).clamp(0.0, 1.0) };
    let default_norm = if span.abs() < 1e-9 { 0.0 } else { ((default - min) / span).clamp(0.0, 1.0) };
    let bipolar = (min < 0.0 && max > 0.0) || (default_norm > 0.25 && default_norm < 0.75);
    let center_norm = if min < 0.0 && max > 0.0 {
        ((0.0 - min) / span).clamp(0.0, 1.0)
    } else if bipolar {
        default_norm
    } else {
        0.0
    };
    canvas_widget(HSliderProgram {
        value_norm: norm,
        default_norm,
        min,
        max,
        bipolar,
        center_norm,
        on_gesture: Box::new(move |g| Some(on_gesture(g))),
    })
    .width(Length::Fill)
    .height(Length::Fixed(18.0))
    .into()
}

/// Styled toggle button — amber when active, dark when inactive.
pub fn toggle_button<'a, Message: Clone + 'a>(
    label: &'a str,
    is_active: bool,
    on_press: Message,
) -> Element<'a, Message> {
    button(Text::new(label).size(12).font(bold_font()))
        .on_press(on_press)
        .padding([5, 10])
        .style(move |_theme, status| {
            let bg = if is_active {
                Color::from_rgb(1.0, 0.45, 0.1)
            } else if status == button::Status::Hovered {
                Color::from_rgb(0.25, 0.25, 0.25)
            } else {
                Color::from_rgb(0.15, 0.15, 0.15)
            };
            button::Style {
                background: Some(bg.into()),
                text_color: Color::WHITE,
                border: Border { radius: 2.0.into(), ..Default::default() },
                ..Default::default()
            }
        })
        .into()
}

/// Monitor strip: [ MONO ] [ DELTA ] [ BYPASS ] — top right in all plugins.
pub fn monitor_strip<'a, Message: Clone + 'a>(
    is_mono: bool,
    is_delta: bool,
    is_bypass: bool,
    on_mono: Message,
    on_delta: Message,
    on_bypass: Message,
) -> Element<'a, Message> {
    row![
        toggle_button("MONO",   is_mono,   on_mono),
        toggle_button("DELTA",  is_delta,  on_delta),
        toggle_button("BYPASS", is_bypass, on_bypass),
    ]
    .spacing(4)
    .into()
}

/// Header brand: [LX AUDIOLABS] │ [Plugin Name / Version below]
pub fn header_brand<'a, Message: 'a>(
    plugin_name: &'static str,
    version: &'static str,
) -> Element<'a, Message> {
    row![
        Text::new("LX").font(bold_font()).size(20).color(Color::from_rgb(1.0, 0.45, 0.1)),
        Space::new().width(Length::Fixed(6.0)),
        Text::new("AUDIOLABS").size(20).color(Color::WHITE),
        Space::new().width(Length::Fixed(14.0)),
        container(Space::new())
            .width(Length::Fixed(1.0))
            .height(Length::Fixed(28.0))
            .style(|_theme| container::Style {
                background: Some(Color::from_rgb(0.18, 0.22, 0.22).into()),
                ..Default::default()
            }),
        Space::new().width(Length::Fixed(14.0)),
        column![
            Text::new(plugin_name).font(bold_font()).size(13).color(Color::from_rgb(1.0, 0.65, 0.3)),
            Text::new(format!("v{}", version)).size(10).color(Color::from_rgb(0.5, 0.5, 0.5)),
        ].spacing(2),
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Reset button — shared footer element for all plugins.
pub fn output_tools_strip<'a, Message: Clone + 'a>(
    on_reset: Message,
) -> Element<'a, Message> {
    button(Text::new("RESET").size(12).font(bold_font()))
        .on_press(on_reset)
        .padding([5, 8])
        .style(|_theme, status| button::Style {
            background: Some((if status == button::Status::Hovered {
                Color::from_rgb(0.4, 0.1, 0.1)
            } else {
                Color::from_rgb(0.2, 0.08, 0.08)
            }).into()),
            text_color: Color::from_rgb(0.9, 0.5, 0.5),
            border: Border { radius: 2.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// AT (Analogue Tolerance) footer section — use in Meridian + Aurum only.
pub fn at_block<'a, Message: Clone + 'a>(
    at_active: bool,
    at_amount: f32,
    on_toggle: Message,
    on_amount: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    column![
        Text::new("ANALOGUE TOLERANCE").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
        row![
            column![
                toggle_button("AT", at_active, on_toggle),
                Text::new("ON/OFF").size(9).color(Color::from_rgb(0.4, 0.4, 0.4)),
            ].spacing(2).align_x(Alignment::Center),
            knob("AT AMT", at_amount, 0.0, 100.0, 50.0, on_amount),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
    ]
    .spacing(4)
    .align_x(Alignment::Center)
    .into()
}

/// Standalone AUTO LOUD button.
pub fn auto_loud_button<'a, Message: Clone + 'a>(
    is_measuring: bool,
    is_active: bool,
    disabled: bool,
    on_press: Message,
) -> Element<'a, Message> {
    let label = if is_measuring { "MEASURING..." } else { "AUTO LOUD" };
    let color = if is_measuring {
        Color::from_rgb(1.0, 0.8, 0.0)
    } else if is_active {
        Color::from_rgb(1.0, 0.45, 0.1)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    };
    let btn_base = button(Text::new(label).size(10).font(bold_font()));
    let btn = if disabled { btn_base } else { btn_base.on_press(on_press) };
    btn.padding([3, 6])
        .style(move |_theme, _status| button::Style {
            background: Some((if disabled { Color::from_rgb(0.1, 0.1, 0.1) } else { color }).into()),
            text_color: if disabled { Color::from_rgb(0.35, 0.35, 0.35) } else { Color::WHITE },
            border: Border { radius: 2.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

// =============================================================================
// Rotary Knob Widget
// =============================================================================

/// Drag-to-adjust rotary knob. Right-click resets to default.
/// Drag up = increase, drag down = decrease. 200px covers the full range.
pub fn knob<'a, Message: Clone + 'a>(
    label: &'a str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let norm = if (max - min).abs() < 1e-9 {
        0.0
    } else {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    };
    let default_norm = if (max - min).abs() < 1e-9 {
        0.0
    } else {
        ((default - min) / (max - min)).clamp(0.0, 1.0)
    };

    let display = if max >= 1000.0 && value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else if max >= 100.0 {
        format!("{:.0}", value)
    } else {
        format!("{:.2}", value)
    };

    column![
        canvas_widget(KnobProgram {
            value_norm: norm,
            default_norm,
            min,
            max,
            bipolar: false,
            on_gesture: change_only(on_change),
        })
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(46.0)),
        Text::new(label)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(1.0, 0.65, 0.3)),
    ]
    .align_x(Alignment::Center)
    .spacing(2)
    .into()
}

/// Linear knob with full DAW-automation gestures. Same look/feel as [`knob`], but the
/// callback receives [`Gesture::Start`]/[`Change`]/[`End`] so the plugin can bracket the
/// drag with `begin_set_parameter`/`end_set_parameter` for clean automation-write + undo.
/// `Change(v)` carries the value in real units (`min..=max`). For log/bipolar/curved
/// gesture variants, add them when a plugin needs one.
pub fn knob_gesture<'a, Message: Clone + 'a>(
    label: &'a str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_gesture: impl Fn(Gesture) -> Message + 'a,
) -> Element<'a, Message> {
    let norm = if (max - min).abs() < 1e-9 { 0.0 } else { ((value - min) / (max - min)).clamp(0.0, 1.0) };
    let default_norm = if (max - min).abs() < 1e-9 { 0.0 } else { ((default - min) / (max - min)).clamp(0.0, 1.0) };

    let display = if max >= 1000.0 && value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else if max >= 100.0 {
        format!("{:.0}", value)
    } else {
        format!("{:.2}", value)
    };

    column![
        canvas_widget(KnobProgram {
            value_norm: norm,
            default_norm,
            min,
            max,
            bipolar: false,
            on_gesture: Box::new(move |g| Some(on_gesture(g))),
        })
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(46.0)),
        Text::new(label).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.65, 0.3)),
    ]
    .align_x(Alignment::Center)
    .spacing(2)
    .into()
}

/// Logarithmic knob — equal octave spacing per knob degree. Use for frequency params.
pub fn knob_log<'a, Message: Clone + 'a>(
    label: &'a str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let log_ratio = (max / min).ln();
    let to_norm  = |v: f32| if log_ratio < 1e-9 { 0.0 } else { ((v / min).ln() / log_ratio).clamp(0.0, 1.0) };

    let norm         = to_norm(value.max(min));
    let default_norm = to_norm(default.max(min));

    let (min_, max_) = (min, max);
    let on_change_log = move |fake: f32| {
        let n = ((fake - min_) / (max_ - min_)).clamp(0.0, 1.0);
        on_change(min_ * (max_ / min_).powf(n))
    };

    let display = if value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else {
        format!("{:.0}", value)
    };

    column![
        canvas_widget(KnobProgram {
            value_norm: norm,
            default_norm,
            min,
            max,
            bipolar: false,
            on_gesture: change_only(on_change_log),
        })
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(46.0)),
        Text::new(label)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(1.0, 0.65, 0.3)),
    ]
    .align_x(Alignment::Center)
    .spacing(2)
    .into()
}

/// Gesture-aware logarithmic knob. Like [`knob_log`] but emits [`Gesture`] for
/// clean DAW touch-automation.
pub fn knob_gesture_log<'a, Message: Clone + 'a>(
    label: &'a str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_gesture: impl Fn(Gesture) -> Message + 'a,
) -> Element<'a, Message> {
    let log_ratio = (max / min).ln();
    let to_norm  = |v: f32| if log_ratio < 1e-9 { 0.0 } else { ((v / min).ln() / log_ratio).clamp(0.0, 1.0) };
    let norm         = to_norm(value.max(min));
    let default_norm = to_norm(default.max(min));
    let (min_, max_) = (min, max);
    let on_gesture_log = move |g: Gesture| match g {
        Gesture::Change(fake) => {
            let n = ((fake - min_) / (max_ - min_)).clamp(0.0, 1.0);
            on_gesture(Gesture::Change(min_ * (max_ / min_).powf(n)))
        }
        other => on_gesture(other),
    };
    let display = if value >= 1000.0 { format!("{:.1}k", value / 1000.0) } else { format!("{:.0}", value) };
    column![
        canvas_widget(KnobProgram {
            value_norm: norm, default_norm, min, max, bipolar: false,
            on_gesture: Box::new(move |g| Some(on_gesture_log(g))),
        }).width(Length::Fixed(46.0)).height(Length::Fixed(46.0)),
        Text::new(label).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.65, 0.3)),
    ].align_x(Alignment::Center).spacing(2).into()
}

/// Knob with a custom suffix appended to the value readout (e.g. " :1" for ratios).
pub fn knob_suffixed<'a, Message: Clone + 'a>(
    label: &'a str,
    suffix: &'static str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let norm = if (max - min).abs() < 1e-9 { 0.0 } else { ((value - min) / (max - min)).clamp(0.0, 1.0) };
    let default_norm = if (max - min).abs() < 1e-9 { 0.0 } else { ((default - min) / (max - min)).clamp(0.0, 1.0) };
    let display = format!("{:.1}{}", value, suffix);

    column![
        canvas_widget(KnobProgram {
            value_norm: norm,
            default_norm,
            min,
            max,
            bipolar: false,
            on_gesture: change_only(on_change),
        })
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(46.0)),
        Text::new(label)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(1.0, 0.65, 0.3)),
    ]
    .align_x(Alignment::Center)
    .spacing(2)
    .into()
}

/// Gesture-aware suffixed knob. Like [`knob_suffixed`] but emits [`Gesture`]
/// for clean DAW touch-automation.
pub fn knob_gesture_suffixed<'a, Message: Clone + 'a>(
    label: &'a str,
    suffix: &'static str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_gesture: impl Fn(Gesture) -> Message + 'a,
) -> Element<'a, Message> {
    let norm = if (max - min).abs() < 1e-9 { 0.0 } else { ((value - min) / (max - min)).clamp(0.0, 1.0) };
    let default_norm = if (max - min).abs() < 1e-9 { 0.0 } else { ((default - min) / (max - min)).clamp(0.0, 1.0) };
    let display = format!("{:.1}{}", value, suffix);
    column![
        canvas_widget(KnobProgram {
            value_norm: norm, default_norm, min, max, bipolar: false,
            on_gesture: Box::new(move |g| Some(on_gesture(g))),
        }).width(Length::Fixed(46.0)).height(Length::Fixed(46.0)),
        Text::new(label).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.65, 0.3)),
    ].align_x(Alignment::Center).spacing(2).into()
}

/// Bipolar knob — arc grows from center (noon) in both directions. Right-click resets.
pub fn knob_bipolar<'a, Message: Clone + 'a>(
    label: &'a str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let norm = if (max - min).abs() < 1e-9 { 0.5 } else { ((value - min) / (max - min)).clamp(0.0, 1.0) };
    let default_norm = if (max - min).abs() < 1e-9 { 0.5 } else { ((default - min) / (max - min)).clamp(0.0, 1.0) };
    // Signed +/- readout only for truly signed ranges (min < 0). For a centre-detent
    // range like Width 0–200 (centre 100) a "+100.0" prefix would be misleading.
    let display = if min < 0.0 {
        if value >= 0.0 { format!("+{:.1}", value) } else { format!("{:.1}", value) }
    } else {
        format!("{:.1}", value)
    };

    column![
        canvas_widget(KnobProgram {
            value_norm: norm,
            default_norm,
            min,
            max,
            bipolar: true,
            on_gesture: change_only(on_change),
        })
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(46.0)),
        Text::new(label)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display)
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(1.0, 0.65, 0.3)),
    ]
    .align_x(Alignment::Center)
    .spacing(2)
    .into()
}

/// Gesture-aware bipolar knob. Like [`knob_bipolar`] but emits [`Gesture`]
/// for clean DAW touch-automation.
pub fn knob_gesture_bipolar<'a, Message: Clone + 'a>(
    label: &'a str,
    value: f32,
    min: f32,
    max: f32,
    default: f32,
    on_gesture: impl Fn(Gesture) -> Message + 'a,
) -> Element<'a, Message> {
    let norm = if (max - min).abs() < 1e-9 { 0.5 } else { ((value - min) / (max - min)).clamp(0.0, 1.0) };
    let default_norm = if (max - min).abs() < 1e-9 { 0.5 } else { ((default - min) / (max - min)).clamp(0.0, 1.0) };
    let display = if min < 0.0 {
        if value >= 0.0 { format!("+{:.1}", value) } else { format!("{:.1}", value) }
    } else { format!("{:.1}", value) };
    column![
        canvas_widget(KnobProgram {
            value_norm: norm, default_norm, min, max, bipolar: true,
            on_gesture: Box::new(move |g| Some(on_gesture(g))),
        }).width(Length::Fixed(46.0)).height(Length::Fixed(46.0)),
        Text::new(label).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.65, 0.3)),
    ].align_x(Alignment::Center).spacing(2).into()
}

/// Knob with a caller-supplied non-linear position→value mapping (e.g. an S-curve).
/// `map` takes the knob position (0..1) → value; the dot position is found by
/// numerically inverting `map` (assumes monotonic increasing). Display shows the
/// value with `suffix` (e.g. "°"). Use for params where resolution should bunch
/// around a sweet-spot rather than spread linearly.
pub fn knob_curved<'a, Message: Clone + 'a>(
    label: &'a str,
    suffix: &'static str,
    value: f32,
    default: f32,
    map: fn(f32) -> f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let invert = |target: f32| -> f32 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if map(mid) < target { lo = mid } else { hi = mid }
        }
        0.5 * (lo + hi)
    };
    let norm = invert(value);
    let default_norm = invert(default);
    let display = format!("{:.1}{}", value, suffix);
    let on_change_curved = move |pos: f32| on_change(map(pos.clamp(0.0, 1.0)));

    column![
        canvas_widget(KnobProgram {
            value_norm: norm,
            default_norm,
            min: 0.0,
            max: 1.0,
            bipolar: false,
            on_gesture: change_only(on_change_curved),
        })
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(46.0)),
        Text::new(label).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.65, 0.3)),
    ]
    .align_x(Alignment::Center)
    .spacing(2)
    .into()
}

/// Gesture-aware curved knob. Like [`knob_curved`] but emits [`Gesture`]
/// for clean DAW touch-automation.
pub fn knob_gesture_curved<'a, Message: Clone + 'a>(
    label: &'a str,
    suffix: &'static str,
    value: f32,
    default: f32,
    map: fn(f32) -> f32,
    on_gesture: impl Fn(Gesture) -> Message + 'a,
) -> Element<'a, Message> {
    let invert = |target: f32| -> f32 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if map(mid) < target { lo = mid } else { hi = mid }
        }
        0.5 * (lo + hi)
    };
    let norm = invert(value);
    let default_norm = invert(default);
    let display = format!("{:.1}{}", value, suffix);
    let on_gesture_curved = move |g: Gesture| match g {
        Gesture::Change(pos) => on_gesture(Gesture::Change(map(pos.clamp(0.0, 1.0)))),
        other => on_gesture(other),
    };
    column![
        canvas_widget(KnobProgram {
            value_norm: norm, default_norm, min: 0.0, max: 1.0, bipolar: false,
            on_gesture: Box::new(move |g| Some(on_gesture_curved(g))),
        }).width(Length::Fixed(46.0)).height(Length::Fixed(46.0)),
        Text::new(label).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
        Text::new(display).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.65, 0.3)),
    ].align_x(Alignment::Center).spacing(2).into()
}

/// DAW-automation gesture lifecycle, mirroring iced_audio's gesture model.
/// A drag emits `Start` (mouse-down) → many `Change(v)` (drag) → `End` (mouse-up),
/// so the host records one continuous touch gesture (clean automation-write + single undo)
/// instead of one mini-gesture per move. Plugins map these to
/// `begin_set_parameter` / `set_parameter` / `end_set_parameter`.
#[derive(Debug, Clone, Copy)]
pub enum Gesture {
    Start,
    Change(f32),
    End,
}

/// Adapt a plain `Fn(f32)->Message` (legacy value-only callback) to the gesture
/// callback: only `Change` produces a message, `Start`/`End` are dropped. Lets the
/// existing knob* helpers keep their call sites unchanged.
fn change_only<'a, Message: 'a>(
    f: impl Fn(f32) -> Message + 'a,
) -> Box<dyn Fn(Gesture) -> Option<Message> + 'a> {
    Box::new(move |g| match g {
        Gesture::Change(v) => Some(f(v)),
        _ => None,
    })
}

/// Persistent drag state for KnobProgram.
#[derive(Default, Clone)]
pub struct KnobState {
    dragging: bool,
    drag_start_y: f32,
    drag_start_norm: f32,
    last_click: Option<std::time::Instant>,
}

struct KnobProgram<'a, Message> {
    value_norm: f32,
    default_norm: f32,
    min: f32,
    max: f32,
    bipolar: bool,
    on_gesture: Box<dyn Fn(Gesture) -> Option<Message> + 'a>,
}

fn knob_arc(center: Point, inner_r: f32, outer_r: f32, a_start: f32, a_end: f32) -> Path {
    const N: usize = 48;
    Path::new(|b| {
        let da = (a_end - a_start) / N as f32;
        b.move_to(Point::new(center.x + outer_r * a_start.cos(), center.y + outer_r * a_start.sin()));
        for i in 1..=N {
            let a = a_start + da * i as f32;
            b.line_to(Point::new(center.x + outer_r * a.cos(), center.y + outer_r * a.sin()));
        }
        for i in (0..=N).rev() {
            let a = a_start + da * i as f32;
            b.line_to(Point::new(center.x + inner_r * a.cos(), center.y + inner_r * a.sin()));
        }
        b.close();
    })
}

impl<'a, Message: Clone> canvas::Program<Message> for KnobProgram<'a, Message> {
    type State = KnobState;

    fn update(
        &self,
        state: &mut KnobState,
        event: &truce_iced::iced::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<truce_iced::iced::widget::Action<Message>> {
        use truce_iced::iced::Event;
        use truce_iced::iced::mouse;
        use truce_iced::iced::widget::Action;

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if cursor.is_over(bounds) {
                    state.dragging = false;
                    let default_val = self.min + self.default_norm * (self.max - self.min);
                    // ponytail: reset is one discrete Change (no Start/End bracket); a single
                    // canvas event can only publish one message. Fine for a discrete set.
                    return (self.on_gesture)(Gesture::Change(default_val)).map(Action::publish);
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if cursor.is_over(bounds) {
                    // Double-click to reset (workaround for truce-iced not forwarding Right-click).
                    // If two left-clicks arrive within 400 ms on this widget, reset to default.
                    let now = std::time::Instant::now();
                    if let Some(last) = state.last_click {
                        if now.duration_since(last).as_millis() < 400 {
                            state.last_click = None;
                            state.dragging = false;
                            let default_val = self.min + self.default_norm * (self.max - self.min);
                            return (self.on_gesture)(Gesture::Change(default_val)).map(Action::publish);
                        }
                    }
                    state.last_click = Some(now);
                    if let Some(pos) = cursor.position() {
                        state.dragging = true;
                        state.drag_start_y = pos.y;
                        state.drag_start_norm = self.value_norm;
                        // Emit Start AND capture the pointer for the drag.
                        return Some(match (self.on_gesture)(Gesture::Start) {
                            Some(msg) => Action::publish(msg).and_capture(),
                            None => Action::capture(),
                        });
                    }
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.dragging {
                    state.dragging = false;
                    return (self.on_gesture)(Gesture::End).map(Action::publish);
                }
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if state.dragging {
                    let dy = state.drag_start_y - position.y;
                    let new_norm = (state.drag_start_norm + dy / 200.0).clamp(0.0, 1.0);
                    let new_val = self.min + new_norm * (self.max - self.min);
                    (self.on_gesture)(Gesture::Change(new_val)).map(Action::publish)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &KnobState,
        renderer: &truce_iced::iced::Renderer,
        _theme: &truce_iced::iced::Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let cx = bounds.width / 2.0;
        let cy = bounds.height / 2.0;
        let r = (bounds.width.min(bounds.height) / 2.0 - 3.0).max(8.0);

        frame.fill(&Path::circle(Point::new(cx, cy), r), Color::from_rgb(0.14, 0.14, 0.14));

        let a_start = std::f32::consts::PI * (2.0 / 3.0);
        let a_sweep = std::f32::consts::PI * (5.0 / 3.0);
        let r_inner = r * 0.72;
        let r_outer = r * 0.96;

        frame.fill(
            &knob_arc(Point::new(cx, cy), r_inner, r_outer, a_start, a_start + a_sweep),
            Color::from_rgb(0.22, 0.22, 0.22),
        );

        let a_center = a_start + a_sweep * 0.5;

        if self.bipolar {
            if (self.value_norm - 0.5).abs() > 0.005 {
                let (arc_s, arc_e) = if self.value_norm > 0.5 {
                    (a_center, a_start + self.value_norm * a_sweep)
                } else {
                    (a_start + self.value_norm * a_sweep, a_center)
                };
                frame.fill(
                    &knob_arc(Point::new(cx, cy), r_inner, r_outer, arc_s, arc_e),
                    Color::from_rgb(1.0, 0.45, 0.1),
                );
            }
            frame.stroke(
                &Path::new(|b| {
                    b.move_to(Point::new(cx + r * 0.68 * a_center.cos(), cy + r * 0.68 * a_center.sin()));
                    b.line_to(Point::new(cx + r_outer * a_center.cos(), cy + r_outer * a_center.sin()));
                }),
                Stroke {
                    style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.25)),
                    width: 1.5,
                    ..Default::default()
                },
            );
        } else if self.value_norm > 0.005 {
            frame.fill(
                &knob_arc(Point::new(cx, cy), r_inner, r_outer, a_start, a_start + self.value_norm * a_sweep),
                Color::from_rgb(1.0, 0.45, 0.1),
            );
        }

        let a_ind = a_start + self.value_norm * a_sweep;
        let ind_r = r * 0.52;
        frame.fill(
            &Path::circle(Point::new(cx + ind_r * a_ind.cos(), cy + ind_r * a_ind.sin()), 2.5),
            Color::WHITE,
        );

        if cursor.is_over(bounds) {
            frame.stroke(
                &Path::circle(Point::new(cx, cy), r),
                Stroke {
                    style: canvas::Style::Solid(Color::from_rgba(1.0, 0.45, 0.1, 0.5)),
                    width: 1.2,
                    ..Default::default()
                },
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &KnobState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> truce_iced::iced::mouse::Interaction {
        // Pointer (finger) over the knob, matching the sliders — not the
        // resize/grab cursor which read as "drag to resize".
        if state.dragging || cursor.is_over(bounds) {
            truce_iced::iced::mouse::Interaction::Pointer
        } else {
            truce_iced::iced::mouse::Interaction::default()
        }
    }
}

/// Persistent drag state for the canvas-based `reset_slider`.
#[derive(Default, Clone)]
struct SliderState {
    dragging: bool,
    last_val: f32,
}

struct SliderProgram<'a, Message> {
    value_norm: f32,
    default_norm: f32,
    min: f32,
    max: f32,
    step: f32,
    bipolar: bool,
    center_norm: f32,
    on_change: Box<dyn Fn(f32) -> Option<Message> + 'a>,
}

impl<'a, Message: Clone> canvas::Program<Message> for SliderProgram<'a, Message> {
    type State = SliderState;

    fn update(
        &self,
        state: &mut SliderState,
        event: &truce_iced::iced::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<truce_iced::iced::widget::Action<Message>> {
        use truce_iced::iced::Event;
        use truce_iced::iced::mouse;
        use truce_iced::iced::widget::Action;

        let val_at = |x: f32| {
            let n = ((x - bounds.x) / bounds.width).clamp(0.0, 1.0);
            let raw = self.min + n * (self.max - self.min);
            if self.step > 0.0 {
                (raw / self.step).round() * self.step
            } else {
                raw
            }
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if cursor.is_over(bounds) {
                    state.dragging = false;
                    let default_val = self.min + self.default_norm * (self.max - self.min);
                    return (self.on_change)(default_val).map(Action::publish);
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if cursor.is_over(bounds) {
                    state.dragging = true;
                    let v = val_at(cursor.position_in(bounds).unwrap_or_default().x);
                    state.last_val = v;
                    return (self.on_change)(v).map(Action::publish);
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = false;
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if state.dragging {
                    let v = val_at(position.x);
                    if (v - state.last_val).abs() > f32::EPSILON {
                        state.last_val = v;
                        return (self.on_change)(v).map(Action::publish);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &SliderState,
        renderer: &truce_iced::iced::Renderer,
        _theme: &truce_iced::iced::Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let track_h = 4.0;
        let ty = (h - track_h) / 2.0;
        let amber = Color::from_rgb(1.0, 0.45, 0.1);

        // Track background.
        frame.fill(
            &Path::rectangle(Point::new(0.0, ty), truce_iced::iced::Size::new(w, track_h)),
            Color::from_rgb(0.22, 0.22, 0.22),
        );

        if self.bipolar {
            let cx = self.center_norm * w;
            let hx = (self.value_norm * w).clamp(0.0, w);
            // Filled: from center toward current value.
            let (left, width) = if hx >= cx {
                (cx, hx - cx)
            } else {
                (hx, cx - hx)
            };
            if width > 0.5 {
                frame.fill(
                    &Path::rectangle(Point::new(left, ty), truce_iced::iced::Size::new(width, track_h)),
                    amber,
                );
            }
        } else {
            let fill_w = (self.value_norm * w).clamp(0.0, w);
            if fill_w > 0.5 {
                frame.fill(
                    &Path::rectangle(Point::new(0.0, ty), truce_iced::iced::Size::new(fill_w, track_h)),
                    amber,
                );
            }
        }

        // Handle.
        let hx = (self.value_norm * w).clamp(3.0, w - 3.0);
        frame.fill(&Path::circle(Point::new(hx, h / 2.0), 5.0), Color::WHITE);

        if cursor.is_over(bounds) {
            frame.stroke(
                &Path::circle(Point::new(hx, h / 2.0), 6.0),
                Stroke {
                    style: canvas::Style::Solid(Color::from_rgba(1.0, 0.45, 0.1, 0.6)),
                    width: 1.2,
                    ..Default::default()
                },
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &SliderState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> truce_iced::iced::mouse::Interaction {
        if state.dragging || cursor.is_over(bounds) {
            truce_iced::iced::mouse::Interaction::Pointer
        } else {
            truce_iced::iced::mouse::Interaction::default()
        }
    }
}

/// Persistent drag state for HSliderProgram.
#[derive(Default, Clone)]
pub struct HSliderState {
    dragging: bool,
    last_click: Option<std::time::Instant>,
}

struct HSliderProgram<'a, Message> {
    value_norm: f32,
    default_norm: f32,
    min: f32,
    max: f32,
    bipolar: bool,
    center_norm: f32,
    on_gesture: Box<dyn Fn(Gesture) -> Option<Message> + 'a>,
}

impl<'a, Message: Clone> canvas::Program<Message> for HSliderProgram<'a, Message> {
    type State = HSliderState;

    fn update(
        &self,
        state: &mut HSliderState,
        event: &truce_iced::iced::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<truce_iced::iced::widget::Action<Message>> {
        use truce_iced::iced::Event;
        use truce_iced::iced::mouse;
        use truce_iced::iced::widget::Action;

        let val_at = |x: f32| {
            let n = ((x - bounds.x) / bounds.width).clamp(0.0, 1.0);
            self.min + n * (self.max - self.min)
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if cursor.is_over(bounds) {
                    state.dragging = false;
                    let default_val = self.min + self.default_norm * (self.max - self.min);
                    // ponytail: discrete reset, one Change (no Start/End). Handler wraps.
                    return (self.on_gesture)(Gesture::Change(default_val)).map(Action::publish);
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if cursor.is_over(bounds) {
                    // Double-click to reset (workaround for truce-iced not forwarding Right-click).
                    let now = std::time::Instant::now();
                    if let Some(last) = state.last_click {
                        if now.duration_since(last).as_millis() < 400 {
                            state.last_click = None;
                            state.dragging = false;
                            let default_val = self.min + self.default_norm * (self.max - self.min);
                            return (self.on_gesture)(Gesture::Change(default_val)).map(Action::publish);
                        }
                    }
                    state.last_click = Some(now);
                    state.dragging = true;
                    return Some(match (self.on_gesture)(Gesture::Start) {
                        Some(msg) => Action::publish(msg).and_capture(),
                        None => Action::capture(),
                    });
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.dragging {
                    state.dragging = false;
                    return (self.on_gesture)(Gesture::End).map(Action::publish);
                }
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if state.dragging {
                    (self.on_gesture)(Gesture::Change(val_at(position.x))).map(Action::publish)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &HSliderState,
        renderer: &truce_iced::iced::Renderer,
        _theme: &truce_iced::iced::Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let track_h = 4.0;
        let ty = (h - track_h) / 2.0;

        // Track background.
        frame.fill(
            &Path::rectangle(Point::new(0.0, ty), truce_iced::iced::Size::new(w, track_h)),
            Color::from_rgb(0.22, 0.22, 0.22),
        );
        let amber = Color::from_rgb(1.0, 0.45, 0.1);
        if self.bipolar {
            let cx = self.center_norm * w;
            let hx_val = (self.value_norm * w).clamp(0.0, w);
            let (left, width) = if hx_val >= cx { (cx, hx_val - cx) } else { (hx_val, cx - hx_val) };
            if width > 0.5 {
                frame.fill(
                    &Path::rectangle(Point::new(left, ty), truce_iced::iced::Size::new(width, track_h)),
                    amber,
                );
            }
        } else {
            let fill_w = (self.value_norm * w).clamp(0.0, w);
            if fill_w > 0.5 {
                frame.fill(
                    &Path::rectangle(Point::new(0.0, ty), truce_iced::iced::Size::new(fill_w, track_h)),
                    amber,
                );
            }
        }
        // Handle.
        let hx = (self.value_norm * w).clamp(3.0, w - 3.0);
        frame.fill(&Path::circle(Point::new(hx, h / 2.0), 5.0), Color::WHITE);

        if cursor.is_over(bounds) {
            frame.stroke(
                &Path::circle(Point::new(hx, h / 2.0), 6.0),
                Stroke {
                    style: canvas::Style::Solid(Color::from_rgba(1.0, 0.45, 0.1, 0.6)),
                    width: 1.2,
                    ..Default::default()
                },
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &HSliderState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> truce_iced::iced::mouse::Interaction {
        if state.dragging || cursor.is_over(bounds) {
            truce_iced::iced::mouse::Interaction::Pointer
        } else {
            truce_iced::iced::mouse::Interaction::default()
        }
    }
}

