//! Vizia/Skia port of `shared-ui/src/canvas.rs`'s drawing programs, scoped
//! to what Equilibrium uses (5-band spectrum bars, stereo peak meter,
//! goniometer). Same duplication rationale as `plugins/lucent/src/vizia_canvas.rs`
//! — not in `shared-ui` yet (iced-only crate, still used by the other 5
//! plugins), moves to a shared crate only after the second Vizia port
//! (Aurum) per CLAP-vault features/2026-07-04-truce-2.0-upgrade-plan.md.
//!
//! `GoniometerView`/`StereoMeterView`/`fmt_db` are a byte-identical copy of
//! Lucent's port (generic over samples/peaks, no Equilibrium-specific
//! change needed). `EqSpectrumView` is new: a 1:1 translation of
//! `plugins/equilibrium/src/editor.rs`'s old `EqSpectrumCanvas` (iced
//! `canvas::Frame`/`Path`/`Stroke`) to `vg::Canvas`/`Paint`/`Path` (Skia).

use std::sync::{Arc, Mutex};
use vizia::prelude::*;
use vizia::vg;

pub(crate) fn col(r: f32, g: f32, b: f32, a: f32) -> vg::Color {
    vg::Color::from_argb(
        (a.clamp(0.0, 1.0) * 255.0) as u8,
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

pub(crate) fn rgb(r: f32, g: f32, b: f32) -> vg::Color {
    col(r, g, b, 1.0)
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

fn line(canvas: &vg::Canvas, x1: f32, y1: f32, x2: f32, y2: f32, color: vg::Color, width: f32) {
    canvas.draw_line((x1, y1), (x2, y2), &stroke_paint(color, width));
}

fn fill_text(canvas: &vg::Canvas, text: &str, x: f32, y: f32, size: f32, color: vg::Color) {
    let mut f = vg::Font::default();
    f.set_size(size);
    canvas.draw_str(text, (x, y), &f, &fill_paint(color));
}

// =============================================================================
// Stereo Meter — port of shared-ui's StereoMeterCanvas + output_level_block
// =============================================================================

pub struct StereoMeterView {
    pub peak_l: f32,
    pub peak_r: f32,
    pub hold_l: f32,
    pub hold_r: f32,
    pub balance: f32,
}

impl StereoMeterView {
    pub fn new(cx: &mut Context, peak_l: f32, peak_r: f32, hold_l: f32, hold_r: f32, balance: f32) -> Handle<'_, Self> {
        Self { peak_l, peak_r, hold_l, hold_r, balance }.build(cx, |_| {})
    }
}

impl View for StereoMeterView {
    fn element(&self) -> Option<&'static str> {
        Some("stereo-meter")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        canvas.translate((b.x, b.y));
        let (w, h) = (b.width(), b.height());
        let gap = 36.0;
        let bar_w = (w - gap) / 2.0;
        let min_db = -60.0f32;
        let max_db = 6.0f32;
        let db_range = max_db - min_db;

        canvas.draw_rect(vg::Rect::new(0.0, 0.0, w, h), &fill_paint(rgb(0.08, 0.08, 0.08)));

        for (idx, (peak_db, hold_db)) in [(self.peak_l, self.hold_l), (self.peak_r, self.hold_r)]
            .iter()
            .enumerate()
        {
            let x = if idx == 0 { 0.0 } else { bar_w + gap };
            let norm_peak = ((*peak_db - min_db) / db_range).clamp(0.0, 1.0);
            let bar_h = h * norm_peak;
            let color = if *peak_db > 0.0 {
                rgb(1.0, 0.25, 0.25)
            } else if *peak_db > -6.0 {
                rgb(1.0, 0.55, 0.1)
            } else {
                rgb(0.0, 0.75, 0.3)
            };
            canvas.draw_rect(
                vg::Rect::new(x + 1.0, h - bar_h, x + bar_w - 1.0, h),
                &fill_paint(color),
            );
            if *hold_db > min_db {
                let norm_hold = ((*hold_db - min_db) / db_range).clamp(0.0, 1.0);
                let hold_y = h - h * norm_hold;
                line(canvas, x, hold_y, x + bar_w, hold_y, vg::Color::WHITE, 1.5);
            }
            let label = if idx == 0 { "L" } else { "R" };
            fill_text(canvas, label, x + bar_w * 0.5 - 3.0, h - 14.0, 9.0, col(1.0, 1.0, 1.0, 0.45));
        }

        let gx = bar_w;
        let center_x = gx + gap * 0.5;
        for (db_val, label) in [(-3.0f32, "-3"), (-6.0, "-6"), (-12.0, "-12"), (-24.0, "-24"), (-48.0, "-48")] {
            let norm = ((db_val - min_db) / db_range).clamp(0.0, 1.0);
            let y = h - h * norm;
            let tick_half = if label == "-3" || label == "-6" { 5.0 } else { 3.0 };
            line(canvas, center_x - tick_half, y, center_x + tick_half, y, col(1.0, 1.0, 1.0, 0.45), 0.8);
            fill_text(canvas, label, gx + 1.0, y - 5.0, 8.0, col(1.0, 1.0, 1.0, 0.60));
        }

        let balance_norm = self.balance.clamp(-1.0, 1.0);
        let cursor_x = center_x - balance_norm * (gap * 0.35);
        let cursor_y = h * 0.82;
        let cursor_color = if balance_norm.abs() < 0.08 {
            rgb(0.0, 0.85, 0.35)
        } else {
            rgb(1.0, 0.45, 0.1)
        };
        canvas.draw_circle((cursor_x, cursor_y), 3.5, &fill_paint(cursor_color));
        line(canvas, center_x, cursor_y - 5.0, center_x, cursor_y + 5.0, col(1.0, 1.0, 1.0, 0.12), 0.8);
    }
}

pub fn fmt_db(v: f32) -> String {
    if v <= -60.0 { "-inf".to_string() } else { format!("{v:.1}") }
}

// =============================================================================
// Goniometer (Vectorscope)
// =============================================================================

pub struct GoniometerView {
    pub samples: Arc<Mutex<Vec<[f32; 2]>>>,
    pub write_pos: usize,
    pub correlation: f32,
}

impl GoniometerView {
    pub fn new(
        cx: &mut Context,
        samples: Arc<Mutex<Vec<[f32; 2]>>>,
        write_pos: impl Res<usize>,
        correlation: impl Res<f32>,
    ) -> Handle<'_, Self> {
        Self {
            samples,
            write_pos: write_pos.get_value(cx),
            correlation: correlation.get_value(cx),
        }
        .build(cx, |_| {})
    }
}

impl View for GoniometerView {
    fn element(&self) -> Option<&'static str> {
        Some("goniometer")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        canvas.translate((b.x, b.y));
        let (w, h) = (b.width(), b.height());
        let (cx_, cy) = (w * 0.5, h * 0.5);
        let scale = cx_.min(cy) * 0.9;

        canvas.draw_rect(vg::Rect::new(0.0, 0.0, w, h), &fill_paint(rgb(0.06, 0.06, 0.06)));

        let grid = col(1.0, 1.0, 1.0, 0.08);
        line(canvas, cx_, 0.0, cx_, h, grid, 1.0);
        line(canvas, 0.0, cy, w, cy, grid, 1.0);
        line(canvas, 0.0, 0.0, w, h, grid, 1.0);
        line(canvas, w, 0.0, 0.0, h, grid, 1.0);
        canvas.draw_circle((cx_, cy), scale, &stroke_paint(col(1.0, 1.0, 1.0, 0.06), 1.0));

        if let Ok(samples) = self.samples.lock() {
            let n = samples.len();
            if n > 0 {
                let draw_count = n.min(2048);
                let third = draw_count / 3;
                let wp = self.write_pos % n;
                let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;

                for group in 0..3u8 {
                    let alpha = match group {
                        0 => 0.12,
                        1 => 0.30,
                        _ => 0.72,
                    };
                    let dot_color = col(0.1, 0.9, 0.5, alpha);
                    let start = group as usize * third;
                    let end = if group == 2 { draw_count } else { (group as usize + 1) * third };

                    let paint = fill_paint(dot_color);
                    for k in start..end {
                        let age = draw_count - 1 - k;
                        let idx = (wp + n - age - 1) % n;
                        let [l, r] = samples[idx];
                        let m = (l + r) * inv_sqrt2;
                        let s = (l - r) * inv_sqrt2;
                        let sx = cx_ - s * scale;
                        let sy = cy - m * scale;
                        if sx >= 0.0 && sx <= w && sy >= 0.0 && sy <= h {
                            canvas.draw_circle((sx, sy), 0.9, &paint);
                        }
                    }
                }
            }
        }

        let corr = self.correlation.clamp(-1.0, 1.0);
        let dot_color = if corr > 0.7 {
            rgb(0.0, 0.85, 0.35)
        } else if corr >= 0.0 {
            rgb(1.0, 0.45, 0.1)
        } else {
            rgb(0.9, 0.2, 0.2)
        };
        let (dot_x, dot_y) = (8.0, h - 8.0);
        canvas.draw_circle((dot_x, dot_y), 3.5, &fill_paint(dot_color));
        let sign = if corr >= 0.0 { "+" } else { "" };
        fill_text(canvas, &format!("{sign}{corr:.2}"), dot_x + 7.0, dot_y - 5.5, 9.0, rgb(1.0, 0.65, 0.3));
    }
}

// =============================================================================
// 5-Band Spectrum — port of the old iced `EqSpectrumCanvas`
// =============================================================================

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

        canvas.draw_rect(vg::Rect::new(0.0, 0.0, width, height), &fill_paint(rgb(0.08, 0.08, 0.08)));

        // Pink noise tilt, halved from the original +3dB/band (was tuned for pure
        // pink noise, over-boosted the Air bar on real mix material — 2026-07-03).
        // ponytail: flat per-band step, not true dB/octave against band center freq.
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

        for b in 0..5 {
            let col_x = b as f32 * col_width;

            let bar_alpha = if self.listen_samples > 0.0 { 0.12 } else { 0.55 };
            if !is_silent {
                let peak_db_t = self.band_levels[b].max(-50.0) + TILT[b];
                let norm_band_db = peak_db_t - band_avg;
                let bar_top_y = db_to_y(norm_band_db);
                let bar_h = (height - bar_top_y).max(0.0);
                canvas.draw_rect(
                    vg::Rect::new(col_x + 5.0, bar_top_y, col_x + col_width - 5.0, bar_top_y + bar_h),
                    &fill_paint(col(1.0, 0.45, 0.1, bar_alpha)),
                );
            }

            if self.listen_samples <= 100.0 {
                let target_db = self.target_levels[b].max(-30.0) + TILT[b];
                let target_sum: f32 = (0..5).map(|i| self.target_levels[i].max(-30.0) + TILT[i]).sum();
                let target_avg = target_sum / 5.0;
                let norm_target_db = target_db - target_avg;
                let tolerance = self.target_tolerances[b];

                let target_y = db_to_y(norm_target_db);
                let upper_y = db_to_y(norm_target_db + tolerance);
                let lower_y = db_to_y(norm_target_db - tolerance);
                let corridor_h = (lower_y - upper_y).max(2.0);

                canvas.draw_rect(
                    vg::Rect::new(col_x + 1.0, upper_y, col_x + col_width - 1.0, upper_y + corridor_h),
                    &fill_paint(col(1.0, 1.0, 1.0, 0.15)),
                );
                line(canvas, col_x, target_y, col_x + col_width, target_y, col(1.0, 1.0, 1.0, 0.55), 1.0);
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
                    vg::Rect::new(col_x + 1.0, upper_y, col_x + col_width - 1.0, upper_y + tolerance_h),
                    &fill_paint(col(1.0, 0.3, 0.3, 0.12)),
                );

                let listen_tolerance = self.listen_tolerances[b];
                let l_upper_y = db_to_y(norm_listen_db + listen_tolerance);
                let l_lower_y = db_to_y(norm_listen_db - listen_tolerance);
                let l_corridor_h = (l_lower_y - l_upper_y).max(2.0);
                canvas.draw_rect(
                    vg::Rect::new(col_x + 1.0, l_upper_y, col_x + col_width - 1.0, l_upper_y + l_corridor_h),
                    &fill_paint(col(0.5, 0.5, 1.0, 0.10)),
                );

                line(canvas, col_x, listen_y, col_x + col_width, listen_y, col(1.0, 0.3, 0.3, 0.7), 1.5);
            }
        }
    }
}
