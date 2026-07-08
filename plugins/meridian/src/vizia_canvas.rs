//! Meridian-specific compressor gain-reduction mini-graph.
//! Generic views (Goniometer, StereoMeter, Spectrum) live in shared-ui.

use shared_ui::{col, fill_paint, line, rgb, stroke_paint};
use vizia::prelude::*;
use vizia::vg;

pub struct CompressorEnvelopeView {
    pub history: Vec<f32>,
    pub current: f32,
    pub peak_hold: f32,
}

impl CompressorEnvelopeView {
    pub fn new(cx: &mut Context, data: Self) -> Handle<'_, Self> {
        data.build(cx, |_| {})
    }
}

impl View for CompressorEnvelopeView {
    fn element(&self) -> Option<&'static str> {
        Some("comp-envelope")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        canvas.translate((b.x, b.y));
        let (w, h) = (b.width(), b.height());
        let max_gr = 12.0f32;
        let margin = 2.0f32;

        canvas.draw_rect(
            vg::Rect::new(0.0, 0.0, w, h),
            &fill_paint(rgb(0.08, 0.08, 0.08)),
        );

        let n = if self.history.is_empty() {
            1
        } else {
            self.history.len() + 1
        };
        let x_step = (w - margin * 2.0) / (n - 1).max(1) as f32;
        let val_to_y = |val: f32| h - margin - (val / max_gr).clamp(0.0, 1.0) * (h - margin * 2.0);

        let mut points: Vec<(f32, f32)> = Vec::with_capacity(n);
        for (i, &val) in self.history.iter().enumerate() {
            points.push((margin + i as f32 * x_step, val_to_y(val)));
        }
        points.push((
            margin + self.history.len() as f32 * x_step,
            val_to_y(self.current),
        ));

        if points.len() >= 2 {
            let mut fb = vg::PathBuilder::new();
            fb.move_to((margin, margin));
            for &(x, y) in &points {
                fb.line_to((x, y));
            }
            let last_x = points.last().map(|p| p.0).unwrap_or(w - margin);
            fb.line_to((last_x, h - margin));
            fb.line_to((margin, h - margin));
            fb.close();
            canvas.draw_path(&fb.detach(), &fill_paint(col(1.0, 0.35, 0.15, 0.18)));

            let mut lb = vg::PathBuilder::new();
            lb.move_to(points[0]);
            for &(x, y) in &points[1..] {
                lb.line_to((x, y));
            }
            canvas.draw_path(&lb.detach(), &stroke_paint(rgb(1.0, 0.4, 0.2), 1.2));
        }

        line(
            canvas,
            margin,
            margin,
            w - margin,
            margin,
            col(1.0, 1.0, 1.0, 0.1),
            0.5,
        );
        let y6 = margin + (h - margin * 2.0) * (6.0 / max_gr);
        line(
            canvas,
            margin,
            y6,
            w - margin,
            y6,
            col(1.0, 1.0, 1.0, 0.06),
            0.5,
        );

        if self.peak_hold > 0.1 {
            let py = val_to_y(self.peak_hold);
            let mut x = margin;
            while x < w - margin {
                let ex = (x + 4.0).min(w - margin);
                line(canvas, x, py, ex, py, col(1.0, 0.65, 0.15, 0.55), 1.0);
                x += 7.0;
            }
        }
    }
}
