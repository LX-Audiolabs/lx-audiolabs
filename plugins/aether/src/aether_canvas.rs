//! Vizia/Skia EQ curve canvas for Aether — renders the 5-band Harman
//! target frequency-response curve as a filled area + line on a log-
//! frequency grid. Stateless draw-only View: all data arrives via struct
//! fields reinstantiated by the caller inside a `Binding`.
//!
//! ponytail: inline helpers instead of pulling in vizia_canvas.rs from Lucent
//! — Aether only needs this one simple curve view, not spectrum/goniometer.

use vizia::prelude::*;
use vizia::vg;

use lx_ui::{col, fill_paint, fill_text, stroke_paint};

pub struct EqCurveView {
    pub points: Vec<(f32, f32)>, // (x_norm 0..1, db) — 240 points
    pub db_min: f32,
    pub db_max: f32,
}

impl EqCurveView {
    pub fn new(cx: &mut Context, points: Vec<(f32, f32)>) -> Handle<'_, Self> {
        Self {
            points,
            db_min: -12.0,
            db_max: 12.0,
        }
        .build(cx, |_| {})
    }
}

impl View for EqCurveView {
    fn element(&self) -> Option<&'static str> {
        Some("eq-curve")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        if self.points.is_empty() || b.width() < 2.0 {
            return;
        }
        canvas.translate((b.x, b.y));
        let (w, h) = (b.width(), b.height());

        // Background
        canvas.draw_rect(
            vg::Rect::new(0.0, 0.0, w, h),
            &fill_paint(col(0.08, 0.08, 0.08, 1.0)),
        );

        // Grid lines
        let db_range = self.db_max - self.db_min;
        let db_to_y = |db: f32| -> f32 {
            let norm = ((db - self.db_min) / db_range).clamp(0.0, 1.0);
            h - norm * h
        };

        // Only label a subset of grid lines so the bottom dB text does not overlap
        // the Hz labels and parameter section directly below the curve.
        for db in [6, 3, 0, -3, -6, -9] {
            let y = db_to_y(db as f32);
            let alpha = if db == 0 { 0.25 } else { 0.10 };
            let grid = stroke_paint(col(1.0, 1.0, 1.0, alpha), 0.6);
            canvas.draw_line((0.0, y), (w, y), &grid);

            // Keep the top label inside the canvas; for all others baseline sits just above the grid line.
            let label_y = if db == 6 { 8.0 } else { y - 2.0 };
            fill_text(
                canvas,
                &format!("{db:+}"),
                2.0,
                label_y,
                9.0,
                col(1.0, 1.0, 1.0, 0.35),
            );
        }

        // Frequency labels
        for (frac, label) in [(0.0, "20"), (0.33, "200"), (0.67, "2k"), (1.0, "20k")] {
            let x = frac * w;
            let text_x = if frac == 0.0 {
                x + 2.0
            } else if frac == 1.0 {
                (x - 22.0).max(0.0)
            } else {
                x - 8.0
            };
            fill_text(
                canvas,
                &format!("{label}Hz"),
                text_x,
                h - 3.0,
                8.0,
                col(1.0, 1.0, 1.0, 0.35),
            );
        }

        // Zero-db line
        let zero_y = db_to_y(0.0);
        canvas.draw_line(
            (0.0, zero_y),
            (w, zero_y),
            &stroke_paint(col(1.0, 1.0, 1.0, 0.18), 0.8),
        );

        // Curve line
        let mut line_path = vg::PathBuilder::new();
        let first_y = db_to_y(self.points[0].1);
        line_path.move_to((0.0, first_y));
        for (xn, db) in &self.points {
            let x = xn * w;
            let y = db_to_y(*db);
            line_path.line_to((x, y));
        }
        canvas.draw_path(
            &line_path.detach(),
            &stroke_paint(col(1.0, 0.45, 0.1, 1.0), 1.5),
        );
    }
}
