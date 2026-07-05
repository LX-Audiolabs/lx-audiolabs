//! Vizia/Skia EQ curve canvas for Aether — renders the 5-band Harman
//! target frequency-response curve as a filled area + line on a log-
//! frequency grid. Stateless draw-only View: all data arrives via struct
//! fields reinstantiated by the caller inside a `Binding`.
//!
//! ponytail: inline helpers instead of pulling in vizia_canvas.rs from Lucent
//! — Aether only needs this one simple curve view, not spectrum/goniometer.

use vizia::prelude::*;
use vizia::vg;

fn col(r: f32, g: f32, b: f32, a: f32) -> vg::Color {
    vg::Color::from_argb(
        (a.clamp(0.0, 1.0) * 255.0) as u8,
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

fn stroke_paint(color: vg::Color, width: f32) -> vg::Paint {
    let mut p = vg::Paint::default();
    p.set_anti_alias(true);
    p.set_color(color);
    p.set_style(vg::PaintStyle::Stroke);
    p.set_stroke_width(width);
    p
}

fn fill_paint(color: vg::Color) -> vg::Paint {
    let mut p = vg::Paint::default();
    p.set_anti_alias(true);
    p.set_color(color);
    p.set_style(vg::PaintStyle::Fill);
    p
}

fn fill_text(canvas: &vg::Canvas, text: &str, x: f32, y: f32, size: f32, color: vg::Color) {
    let mut f = vg::Font::default();
    f.set_size(size);
    canvas.draw_str(text, (x, y), &f, &fill_paint(color));
}

pub struct EqCurveView {
    pub points: Vec<(f32, f32)>, // (x_norm 0..1, db) — 240 points
    pub db_min: f32,
    pub db_max: f32,
}

impl EqCurveView {
    pub fn new(cx: &mut Context, points: Vec<(f32, f32)>) -> Handle<'_, Self> {
        Self { points, db_min: -12.0, db_max: 12.0 }.build(cx, |_| {})
    }
}

impl View for EqCurveView {
    fn element(&self) -> Option<&'static str> {
        Some("eq-curve")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        if self.points.is_empty() || b.width() < 2.0 { return; }
        canvas.translate((b.x, b.y));
        let (w, h) = (b.width(), b.height());

        // Background
        canvas.draw_rect(vg::Rect::new(0.0, 0.0, w, h), &fill_paint(col(0.08, 0.08, 0.08, 1.0)));

        // Grid lines
        let db_range = self.db_max - self.db_min;
        for db in [-12, -9, -6, -3, 0, 3, 6, 9] {
            let y = h - h * (db as f32 - self.db_min + 12.0) / (db_range + 12.0);
            let alpha = if db == 0 { 0.25 } else { 0.10 };
            let grid = stroke_paint(col(1.0, 1.0, 1.0, alpha), 0.6);
            canvas.draw_line((0.0, y), (w, y), &grid);

            let mut label_font = vg::Font::default();
            label_font.set_size(9.0);
            canvas.draw_str(&format!("{db:+}"), (2.0, y - 2.0), &label_font, &fill_paint(col(1.0, 1.0, 1.0, 0.35)));
        }

        // Frequency labels
        for (freq, label) in [(0.0, "20"), (0.33, "200"), (0.67, "2k"), (1.0, "20k")] {
            let x = freq * w;
            fill_text(canvas, &format!("{label}Hz"), x - 10.0, h - 3.0, 8.0, col(1.0, 1.0, 1.0, 0.35));
        }

        // Zero-db line
        let zero_y = h - h * (-self.db_min + 12.0) / (db_range + 12.0);
        canvas.draw_line((0.0, zero_y), (w, zero_y), &stroke_paint(col(1.0, 1.0, 1.0, 0.18), 0.8));

        // Curve — filled area under the line
        let mut path = vg::PathBuilder::new();
        let first_y = h - h * (self.points[0].1 - self.db_min + 12.0) / (db_range + 12.0);
        path.move_to((0.0, h)); // bottom-left corner
        path.move_to((0.0, first_y));
        for (xn, db) in &self.points {
            let x = xn * w;
            let y = h - h * (db - self.db_min + 12.0) / (db_range + 12.0);
            path.line_to((x, y));
        }
        path.line_to((w, h)); // bottom-right corner
        path.close();

        let mut fill = vg::Paint::default();
        fill.set_anti_alias(true);
        fill.set_color(col(1.0, 0.45, 0.1, 0.12));
        canvas.draw_path(&path.detach(), &fill);

        // Curve line
        let mut line_path = vg::PathBuilder::new();
        let first_y = h - h * (self.points[0].1 - self.db_min + 12.0) / (db_range + 12.0);
        line_path.move_to((0.0, first_y));
        for (xn, db) in &self.points {
            let x = xn * w;
            let y = h - h * (db - self.db_min + 12.0) / (db_range + 12.0);
            line_path.line_to((x, y));
        }
        canvas.draw_path(&line_path.detach(), &stroke_paint(col(1.0, 0.45, 0.1, 1.0), 1.5));
    }
}
