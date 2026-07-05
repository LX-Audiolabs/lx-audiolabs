//! Vizia/Skia port of `shared-ui/src/widgets.rs`'s `KnobProgram` +
//! `knob_gesture` - the only widget-with-interaction Lucent's editor uses
//! (the rest of its UI is buttons/text, plain Vizia primitives, no custom
//! canvas needed). Same duplication rationale as `vizia_canvas.rs`: lives
//! here until the Vizia pilot proves out, not in a shared crate yet.
//!
//! iced's `canvas::Program` splits drawing (`draw`, given `&State`) from
//! interaction (`update`, given `&mut State`) because iced rebuilds the
//! `Program` value fresh every `view()` call - the `State` is the only
//! thing that persists across frames. Vizia's `View` is the persistent
//! object itself (built once, mutated in place), so drag state lives as
//! plain fields instead of a separate `State` type, and `event()` takes
//! `&mut self` directly.
//!
//! API names below (`cx.mouse()`, `cx.capture()`/`release()`,
//! `cx.hovered()`/`current()`, `event.map()`, `WindowEvent::MouseDown`)
//! verified against the actual `vizia_core`/`vizia_input` 0.4.0 source
//! before writing this, not guessed.

use std::time::Instant;
use vizia::prelude::*;
use vizia::vg;

/// DAW-automation gesture lifecycle - identical shape to
/// `shared_ui::widgets::Gesture`, duplicated here for the same reason as
/// the rest of this module.
#[derive(Debug, Clone, Copy)]
pub enum Gesture {
    Start,
    Change(f32),
    End,
}

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
    /// `DrawContext` has no public hover query (unlike `EventContext`), so
    /// hover state is tracked here from `MouseEnter`/`MouseLeave` instead.
    hovered: bool,
    on_gesture: Box<dyn Fn(&mut EventContext, Gesture)>,
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

fn knob_arc(canvas: &vg::Canvas, cx: f32, cy: f32, inner_r: f32, outer_r: f32, a_start: f32, a_end: f32, color: vg::Color) {
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

fn rgba(r: f32, g: f32, b: f32, a: f32) -> vg::Color {
    vg::Color::from_argb((a * 255.0) as u8, (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
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
                    let default_val = self.min + self.default_norm * (self.max - self.min);
                    (self.on_gesture)(cx, Gesture::Change(default_val));
                    meta.consume();
                }
            }
            WindowEvent::MouseDown(MouseButton::Left) => {
                if cx.hovered() == cx.current() {
                    // Double-click reset - same workaround shared-ui's
                    // KnobProgram carries for truce-iced's right-click
                    // delivery bug. Right-click itself works fine here
                    // (handled above), kept for parity / muscle memory.
                    let now = Instant::now();
                    if let Some(last) = self.last_click {
                        if now.duration_since(last).as_millis() < 400 {
                            self.last_click = None;
                            self.dragging = false;
                            let default_val = self.min + self.default_norm * (self.max - self.min);
                            (self.on_gesture)(cx, Gesture::Change(default_val));
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
                    let dy = self.drag_start_y - *y;
                    let new_norm = (self.drag_start_norm + dy / 200.0).clamp(0.0, 1.0);
                    self.value_norm = new_norm;
                    let new_val = self.min + new_norm * (self.max - self.min);
                    (self.on_gesture)(cx, Gesture::Change(new_val));
                    cx.needs_redraw();
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
        bg.set_color(rgba(0.14, 0.14, 0.14, 1.0));
        canvas.draw_circle((kcx, kcy), r, &bg);

        let a_start = std::f32::consts::PI * (2.0 / 3.0);
        let a_sweep = std::f32::consts::PI * (5.0 / 3.0);
        let r_inner = r * 0.72;
        let r_outer = r * 0.96;

        knob_arc(canvas, kcx, kcy, r_inner, r_outer, a_start, a_start + a_sweep, rgba(0.22, 0.22, 0.22, 1.0));

        let a_center = a_start + a_sweep * 0.5;

        if self.bipolar {
            if (self.value_norm - 0.5).abs() > 0.005 {
                let (arc_s, arc_e) = if self.value_norm > 0.5 {
                    (a_center, a_start + self.value_norm * a_sweep)
                } else {
                    (a_start + self.value_norm * a_sweep, a_center)
                };
                knob_arc(canvas, kcx, kcy, r_inner, r_outer, arc_s, arc_e, rgba(1.0, 0.45, 0.1, 1.0));
            }
            let mut stroke = vg::Paint::default();
            stroke.set_anti_alias(true);
            stroke.set_style(vg::PaintStyle::Stroke);
            stroke.set_stroke_width(1.5);
            stroke.set_color(rgba(1.0, 1.0, 1.0, 0.25));
            canvas.draw_line(
                (kcx + r * 0.68 * a_center.cos(), kcy + r * 0.68 * a_center.sin()),
                (kcx + r_outer * a_center.cos(), kcy + r_outer * a_center.sin()),
                &stroke,
            );
        } else if self.value_norm > 0.005 {
            knob_arc(canvas, kcx, kcy, r_inner, r_outer, a_start, a_start + self.value_norm * a_sweep, rgba(1.0, 0.45, 0.1, 1.0));
        }

        let a_ind = a_start + self.value_norm * a_sweep;
        let ind_r = r * 0.52;
        let mut dot = vg::Paint::default();
        dot.set_anti_alias(true);
        dot.set_color(vg::Color::WHITE);
        canvas.draw_circle((kcx + ind_r * a_ind.cos(), kcy + ind_r * a_ind.sin()), 2.5, &dot);

        if self.hovered {
            let mut hover_ring = vg::Paint::default();
            hover_ring.set_anti_alias(true);
            hover_ring.set_style(vg::PaintStyle::Stroke);
            hover_ring.set_stroke_width(1.2);
            hover_ring.set_color(rgba(1.0, 0.45, 0.1, 0.5));
            canvas.draw_circle((kcx, kcy), r, &hover_ring);
        }
    }
}

/// Formats a knob's real-unit value for the readout label under it -
/// same three-tier formatting `knob_gesture` used (k-suffix above 1000,
/// no decimals above 100, 2 decimals below).
pub fn format_knob_value(value: f32, max: f32) -> String {
    if max >= 1000.0 && value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else if max >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}
