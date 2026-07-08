//! Vizia widget library — extracted from Equilibrium's `vizia_widgets.rs`.
//! KnobView, HSliderView, Gesture enum, and format utilities.

use std::time::Instant;
use vizia::prelude::*;
use vizia::vg;

use crate::canvas::col;

/// Shorthand for the boxed callback used by gesture-aware widgets.
type GestureCallback = Box<dyn Fn(&mut EventContext, Gesture)>;

/// DAW-automation gesture lifecycle.
#[derive(Debug, Clone, Copy)]
pub enum Gesture {
    Start,
    Change(f32),
    End,
}

// ─── KnobView ────────────────────────────────────────────────────────────────

pub struct KnobView {
    pub value_norm: f32,
    pub default_norm: f32,
    pub min: f32,
    pub max: f32,
    pub bipolar: bool,
    dragging: bool,
    drag_start_y: f32,
    drag_start_norm: f32,
    last_click: Option<Instant>,
    /// Tracked manually because Vizia's `hovered` check compares against
    /// `cx.current()`, which isn't always the knob itself during drag.
    hovered: bool,
    on_gesture: GestureCallback,
}

impl KnobView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cx: &mut Context,
        value_norm: f32,
        default_norm: f32,
        min: f32,
        max: f32,
        bipolar: bool,
        on_gesture: impl Fn(&mut EventContext, Gesture) + 'static,
    ) -> Handle<'_, Self> {
        Self {
            value_norm,
            default_norm,
            min,
            max,
            bipolar,
            dragging: false,
            drag_start_y: 0.0,
            drag_start_norm: 0.0,
            last_click: None,
            hovered: false,
            on_gesture: Box::new(on_gesture),
        }
        .build(cx, |_| {})
    }
}

#[allow(clippy::too_many_arguments)]
fn knob_arc(
    canvas: &vg::Canvas,
    cx: f32,
    cy: f32,
    inner_r: f32,
    outer_r: f32,
    a_start: f32,
    a_end: f32,
    color: vg::Color,
) {
    const N: usize = 48;
    let da = (a_end - a_start) / N as f32;
    let mut path = vg::PathBuilder::new();
    path.move_to((cx + outer_r * a_start.cos(), cy + outer_r * a_start.sin()));
    for i in 1..=N {
        let a = a_start + da * i as f32;
        path.line_to((cx + outer_r * a.cos(), cy + outer_r * a.sin()));
    }
    for i in (0..=N).rev() {
        let a = a_start + da * i as f32;
        path.line_to((cx + inner_r * a.cos(), cy + inner_r * a.sin()));
    }
    path.close();
    let mut paint = vg::Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(color);
    canvas.draw_path(&path.detach(), &paint);
}

impl View for KnobView {
    fn element(&self) -> Option<&'static str> {
        Some("knob")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event: &WindowEvent, meta| match window_event {
            WindowEvent::MouseDown(MouseButton::Right) => {
                if cx.hovered() == cx.current() {
                    self.dragging = false;
                    self.value_norm = self.default_norm;
                    let default_val = self.min + self.default_norm * (self.max - self.min);
                    (self.on_gesture)(cx, Gesture::Change(default_val));
                    cx.needs_redraw();
                    meta.consume();
                }
            }
            WindowEvent::MouseDown(MouseButton::Left) => {
                if cx.hovered() == cx.current() {
                    let now = Instant::now();
                    if let Some(last) = self.last_click {
                        if now.duration_since(last).as_millis() < 400 {
                            self.last_click = None;
                            self.dragging = false;
                            self.value_norm = self.default_norm;
                            let default_val = self.min + self.default_norm * (self.max - self.min);
                            (self.on_gesture)(cx, Gesture::Change(default_val));
                            cx.needs_redraw();
                            meta.consume();
                            return;
                        }
                    }
                    self.last_click = Some(now);
                    self.dragging = true;
                    self.drag_start_y = cx.mouse().cursor_y;
                    self.drag_start_norm = self.value_norm;
                    (self.on_gesture)(cx, Gesture::Start);
                    cx.capture();
                    meta.consume();
                }
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if self.dragging {
                    self.dragging = false;
                    (self.on_gesture)(cx, Gesture::End);
                    cx.release();
                    meta.consume();
                }
            }
            WindowEvent::MouseMove(_x, y) => {
                if self.dragging {
                    // Defensive: if the button-up was missed (e.g. the host
                    // stole focus mid-drag) `dragging` would otherwise stay
                    // stuck true forever, ignoring all input until a fresh
                    // click - end the drag as soon as we notice the button
                    // isn't actually held anymore.
                    if cx.mouse().left.state != MouseButtonState::Pressed {
                        self.dragging = false;
                        (self.on_gesture)(cx, Gesture::End);
                        cx.release();
                    } else {
                        let dy = self.drag_start_y - *y;
                        let new_norm = (self.drag_start_norm + dy / 200.0).clamp(0.0, 1.0);
                        self.value_norm = new_norm;
                        let new_val = self.min + new_norm * (self.max - self.min);
                        (self.on_gesture)(cx, Gesture::Change(new_val));
                        cx.needs_redraw();
                    }
                }
            }
            WindowEvent::MouseEnter => {
                self.hovered = true;
                cx.needs_redraw();
            }
            WindowEvent::MouseLeave => {
                self.hovered = false;
                cx.needs_redraw();
            }
            _ => {}
        });
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        canvas.translate((b.x, b.y));
        let (kcx, kcy) = (b.width() / 2.0, b.height() / 2.0);
        let r = (b.width().min(b.height()) / 2.0 - 3.0).max(8.0);

        let mut bg = vg::Paint::default();
        bg.set_anti_alias(true);
        bg.set_color(col(0.14, 0.14, 0.14, 1.0));
        canvas.draw_circle((kcx, kcy), r, &bg);

        let a_start = std::f32::consts::PI * (2.0 / 3.0);
        let a_sweep = std::f32::consts::PI * (5.0 / 3.0);
        let r_inner = r * 0.72;
        let r_outer = r * 0.96;

        knob_arc(
            canvas,
            kcx,
            kcy,
            r_inner,
            r_outer,
            a_start,
            a_start + a_sweep,
            col(0.22, 0.22, 0.22, 1.0),
        );

        let a_center = a_start + a_sweep * 0.5;

        if self.bipolar {
            if (self.value_norm - 0.5).abs() > 0.005 {
                let (arc_s, arc_e) = if self.value_norm > 0.5 {
                    (a_center, a_start + self.value_norm * a_sweep)
                } else {
                    (a_start + self.value_norm * a_sweep, a_center)
                };
                knob_arc(
                    canvas,
                    kcx,
                    kcy,
                    r_inner,
                    r_outer,
                    arc_s,
                    arc_e,
                    col(1.0, 0.45, 0.1, 1.0),
                );
            }
            let mut stroke = vg::Paint::default();
            stroke.set_anti_alias(true);
            stroke.set_style(vg::PaintStyle::Stroke);
            stroke.set_stroke_width(1.5);
            stroke.set_color(col(1.0, 1.0, 1.0, 0.25));
            canvas.draw_line(
                (
                    kcx + r * 0.68 * a_center.cos(),
                    kcy + r * 0.68 * a_center.sin(),
                ),
                (
                    kcx + r_outer * a_center.cos(),
                    kcy + r_outer * a_center.sin(),
                ),
                &stroke,
            );
        } else if self.value_norm > 0.005 {
            knob_arc(
                canvas,
                kcx,
                kcy,
                r_inner,
                r_outer,
                a_start,
                a_start + self.value_norm * a_sweep,
                col(1.0, 0.45, 0.1, 1.0),
            );
        }

        let a_ind = a_start + self.value_norm * a_sweep;
        let ind_r = r * 0.52;
        let mut dot = vg::Paint::default();
        dot.set_anti_alias(true);
        dot.set_color(vg::Color::WHITE);
        canvas.draw_circle(
            (kcx + ind_r * a_ind.cos(), kcy + ind_r * a_ind.sin()),
            2.5,
            &dot,
        );

        if self.hovered {
            let mut hover_ring = vg::Paint::default();
            hover_ring.set_anti_alias(true);
            hover_ring.set_style(vg::PaintStyle::Stroke);
            hover_ring.set_stroke_width(1.2);
            hover_ring.set_color(col(1.0, 0.45, 0.1, 0.5));
            canvas.draw_circle((kcx, kcy), r, &hover_ring);
        }
    }
}

// ─── format_knob_value ───────────────────────────────────────────────────────

/// Formats a knob's real-unit value for the readout label.
pub fn format_knob_value(value: f32, max: f32) -> String {
    if max >= 1000.0 && value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else if max >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

// ─── HSliderView ─────────────────────────────────────────────────────────────

/// Horizontal drag slider — value maps directly from cursor X position.
pub struct HSliderView {
    pub value_norm: f32,
    pub default_norm: f32,
    pub min: f32,
    pub max: f32,
    pub bipolar: bool,
    pub center_norm: f32,
    dragging: bool,
    last_click: Option<Instant>,
    hovered: bool,
    on_gesture: GestureCallback,
}

impl HSliderView {
    pub fn new(
        cx: &mut Context,
        min: f32,
        max: f32,
        value: f32,
        default: f32,
        on_gesture: impl Fn(&mut EventContext, Gesture) + 'static,
    ) -> Handle<'_, Self> {
        let span = max - min;
        let value_norm = if span.abs() < 1e-9 {
            0.0
        } else {
            ((value - min) / span).clamp(0.0, 1.0)
        };
        let default_norm = if span.abs() < 1e-9 {
            0.0
        } else {
            ((default - min) / span).clamp(0.0, 1.0)
        };
        let bipolar = min < 0.0 && max > 0.0;
        let center_norm = if bipolar {
            ((0.0 - min) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            value_norm,
            default_norm,
            min,
            max,
            bipolar,
            center_norm,
            dragging: false,
            last_click: None,
            hovered: false,
            on_gesture: Box::new(on_gesture),
        }
        .build(cx, |_| {})
    }

    fn val_at(&self, cx: &EventContext, x: f32) -> f32 {
        let bounds = cx.bounds();
        let n = if bounds.w > 0.0 {
            ((x - bounds.x) / bounds.w).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.min + n * (self.max - self.min)
    }
}

impl View for HSliderView {
    fn element(&self) -> Option<&'static str> {
        Some("hslider")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event: &WindowEvent, meta| match window_event {
            WindowEvent::MouseDown(MouseButton::Right) => {
                if cx.hovered() == cx.current() {
                    self.dragging = false;
                    self.value_norm = self.default_norm;
                    let default_val = self.min + self.default_norm * (self.max - self.min);
                    (self.on_gesture)(cx, Gesture::Change(default_val));
                    cx.needs_redraw();
                    meta.consume();
                }
            }
            WindowEvent::MouseDown(MouseButton::Left) => {
                if cx.hovered() == cx.current() {
                    let now = Instant::now();
                    if let Some(last) = self.last_click {
                        if now.duration_since(last).as_millis() < 400 {
                            self.last_click = None;
                            self.dragging = false;
                            self.value_norm = self.default_norm;
                            let default_val = self.min + self.default_norm * (self.max - self.min);
                            (self.on_gesture)(cx, Gesture::Change(default_val));
                            cx.needs_redraw();
                            meta.consume();
                            return;
                        }
                    }
                    self.last_click = Some(now);
                    self.dragging = true;
                    (self.on_gesture)(cx, Gesture::Start);
                    cx.capture();
                    meta.consume();
                }
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if self.dragging {
                    self.dragging = false;
                    (self.on_gesture)(cx, Gesture::End);
                    cx.release();
                    meta.consume();
                }
            }
            WindowEvent::MouseMove(x, _y) => {
                if self.dragging {
                    if cx.mouse().left.state != MouseButtonState::Pressed {
                        self.dragging = false;
                        (self.on_gesture)(cx, Gesture::End);
                        cx.release();
                    } else {
                        let new_val = self.val_at(cx, *x);
                        let span = self.max - self.min;
                        self.value_norm = if span.abs() < 1e-9 {
                            0.0
                        } else {
                            ((new_val - self.min) / span).clamp(0.0, 1.0)
                        };
                        (self.on_gesture)(cx, Gesture::Change(new_val));
                        cx.needs_redraw();
                    }
                }
            }
            WindowEvent::MouseEnter => {
                self.hovered = true;
                cx.needs_redraw();
            }
            WindowEvent::MouseLeave => {
                self.hovered = false;
                cx.needs_redraw();
            }
            _ => {}
        });
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        canvas.translate((b.x, b.y));
        let (w, h) = (b.width(), b.height());
        let track_h = 4.0;
        let ty = (h - track_h) / 2.0;

        let mut bg = vg::Paint::default();
        bg.set_anti_alias(true);
        bg.set_color(col(0.22, 0.22, 0.22, 1.0));
        canvas.draw_rect(vg::Rect::new(0.0, ty, w, ty + track_h), &bg);

        let mut amber = vg::Paint::default();
        amber.set_anti_alias(true);
        amber.set_color(col(1.0, 0.45, 0.1, 1.0));

        if self.bipolar {
            let cx_px = self.center_norm * w;
            let hx = (self.value_norm * w).clamp(0.0, w);
            let (left, right) = if hx >= cx_px {
                (cx_px, hx)
            } else {
                (hx, cx_px)
            };
            if right - left > 0.5 {
                canvas.draw_rect(vg::Rect::new(left, ty, right, ty + track_h), &amber);
            }
        } else {
            let fill_w = (self.value_norm * w).clamp(0.0, w);
            if fill_w > 0.5 {
                canvas.draw_rect(vg::Rect::new(0.0, ty, fill_w, ty + track_h), &amber);
            }
        }

        let hx = (self.value_norm * w).clamp(3.0, w - 3.0);
        let mut handle = vg::Paint::default();
        handle.set_anti_alias(true);
        handle.set_color(vg::Color::WHITE);
        canvas.draw_circle((hx, h / 2.0), 5.0, &handle);

        if self.hovered {
            let mut ring = vg::Paint::default();
            ring.set_anti_alias(true);
            ring.set_style(vg::PaintStyle::Stroke);
            ring.set_stroke_width(1.2);
            ring.set_color(col(1.0, 0.45, 0.1, 0.6));
            canvas.draw_circle((hx, h / 2.0), 6.0, &ring);
        }
    }
}
