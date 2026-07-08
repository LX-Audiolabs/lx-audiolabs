//! Equilibrium-specific 5-band spectrum view.
//! Generic views (Goniometer, StereoMeter, Spectrum) live in shared-ui.

use shared_ui::{col, fill_paint, line, rgb};
use vizia::prelude::*;
use vizia::vg;

pub struct EqSpectrumView {
    pub band_levels: [f32; 5],
    pub target_levels: [f32; 5],
    pub target_tolerances: [f32; 5],
    pub listen_levels: [f32; 5],
    pub listen_tolerances: [f32; 5],
    pub listen_level_min: [f32; 5],
    pub listen_level_max: [f32; 5],
    pub listen_samples: f32,
}

impl EqSpectrumView {
    pub fn new(cx: &mut Context, data: Self) -> Handle<'_, Self> {
        data.build(cx, |_| {})
    }
}

impl View for EqSpectrumView {
    fn element(&self) -> Option<&'static str> {
        Some("eq-spectrum")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        canvas.translate((b.x, b.y));
        let width = b.width();
        let height = b.height();
        let col_width = width / 5.0;

        canvas.draw_rect(
            vg::Rect::new(0.0, 0.0, width, height),
            &fill_paint(rgb(0.08, 0.08, 0.08)),
        );

        const TILT: [f32; 5] = [-1.5, 0.0, 1.5, 3.0, 4.5];

        let raw_band_avg: f32 = (0..5).map(|b| self.band_levels[b]).sum::<f32>() / 5.0;
        let is_silent = raw_band_avg <= -70.0;

        let mut listen_sum = 0.0;
        let mut band_sum = 0.0;
        for (b, &tilt) in TILT.iter().enumerate() {
            listen_sum += self.listen_levels[b].max(-50.0) + tilt;
            band_sum += self.band_levels[b].max(-50.0) + tilt;
        }
        let listen_avg = listen_sum / 5.0;
        let band_avg = band_sum / 5.0;

        // Target profiles are visual reference shapes and use the same pink-noise
        // display tilt as the live signal so that the built-in Pink Noise preset
        // (the negative of TILT) appears as a flat reference line.
        let target_sum: f32 = (0..5)
            .map(|i| (self.target_levels[i] + TILT[i]).max(-30.0))
            .sum();
        let target_avg = target_sum / 5.0;

        let min_db = -30.0f32;
        let max_db = 12.0f32;
        let db_range = max_db - min_db;

        let db_to_y = |db: f32| {
            let norm = ((db - min_db) / db_range).clamp(0.0, 1.0);
            height - (norm * height)
        };

        for &db in &[-30.0f32, -24.0, -18.0, -12.0, -6.0, 0.0, 6.0, 12.0] {
            let y = db_to_y(db);
            let is_major = db == -30.0 || db == -18.0 || db == -6.0 || db == 6.0;
            let alpha = if is_major { 0.20 } else { 0.10 };
            line(canvas, 0.0, y, width, y, col(1.0, 1.0, 1.0, alpha), 1.0);
        }

        for i in 1..5 {
            let x = i as f32 * col_width;
            line(canvas, x, 0.0, x, height, col(1.0, 1.0, 1.0, 0.05), 1.0);
        }

        for (b, (&level, &tilt)) in self.band_levels.iter().zip(TILT.iter()).enumerate() {
            let col_x = b as f32 * col_width;

            let bar_alpha = if self.listen_samples > 0.0 {
                0.12
            } else {
                0.55
            };
            if !is_silent {
                let peak_db_t = level.max(-50.0) + tilt;
                let norm_band_db = peak_db_t - band_avg;
                let bar_top_y = db_to_y(norm_band_db);
                let bar_h = (height - bar_top_y).max(0.0);
                canvas.draw_rect(
                    vg::Rect::new(
                        col_x + 5.0,
                        bar_top_y,
                        col_x + col_width - 5.0,
                        bar_top_y + bar_h,
                    ),
                    &fill_paint(col(1.0, 0.45, 0.1, bar_alpha)),
                );
            }

            if self.listen_samples <= 100.0 {
                let target_db = (self.target_levels[b] + TILT[b]).max(-30.0);
                let norm_target_db = target_db - target_avg;
                let tolerance = self.target_tolerances[b];

                let target_y = db_to_y(norm_target_db);
                let upper_y = db_to_y(norm_target_db + tolerance);
                let lower_y = db_to_y(norm_target_db - tolerance);
                let corridor_h = (lower_y - upper_y).max(2.0);

                canvas.draw_rect(
                    vg::Rect::new(
                        col_x + 1.0,
                        upper_y,
                        col_x + col_width - 1.0,
                        upper_y + corridor_h,
                    ),
                    &fill_paint(col(1.0, 1.0, 1.0, 0.15)),
                );
                line(
                    canvas,
                    col_x,
                    target_y,
                    col_x + col_width,
                    target_y,
                    col(1.0, 1.0, 1.0, 0.55),
                    1.0,
                );
            }

            if self.listen_samples > 100.0 {
                let listen_db = self.listen_levels[b].max(-50.0) + TILT[b];
                let norm_listen_db = listen_db - listen_avg;
                let listen_y = db_to_y(norm_listen_db);

                let min_db_l = self.listen_level_min[b].max(-50.0) + TILT[b];
                let max_db_l = self.listen_level_max[b].max(-50.0) + TILT[b];
                let norm_min = min_db_l - listen_avg;
                let norm_max = max_db_l - listen_avg;
                let upper_y = db_to_y(norm_max);
                let lower_y = db_to_y(norm_min);
                let tolerance_h = (lower_y - upper_y).max(2.0);

                canvas.draw_rect(
                    vg::Rect::new(
                        col_x + 1.0,
                        upper_y,
                        col_x + col_width - 1.0,
                        upper_y + tolerance_h,
                    ),
                    &fill_paint(col(1.0, 0.3, 0.3, 0.12)),
                );

                let listen_tolerance = self.listen_tolerances[b];
                let l_upper_y = db_to_y(norm_listen_db + listen_tolerance);
                let l_lower_y = db_to_y(norm_listen_db - listen_tolerance);
                let l_corridor_h = (l_lower_y - l_upper_y).max(2.0);
                canvas.draw_rect(
                    vg::Rect::new(
                        col_x + 1.0,
                        l_upper_y,
                        col_x + col_width - 1.0,
                        l_upper_y + l_corridor_h,
                    ),
                    &fill_paint(col(0.5, 0.5, 1.0, 0.10)),
                );

                line(
                    canvas,
                    col_x,
                    listen_y,
                    col_x + col_width,
                    listen_y,
                    col(1.0, 0.3, 0.3, 0.7),
                    1.5,
                );
            }
        }
    }
}
