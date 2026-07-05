//! Vizia/Skia port of `shared-ui/src/canvas.rs`'s drawing programs, scoped
//! to what Lucent uses (Spectrum, Goniometer, Stereo peak meter). Not in
//! `shared-ui` itself yet - that crate is iced-only (see its Cargo.toml) and
//! still used by the other 5 plugins. Once the Vizia pilot proves out this
//! moves to a shared crate; duplicating it here for now is the
//! smaller/safer diff while the outcome is still unproven.
//!
//! Drawing-primitive translation from iced's `canvas::Frame`/`Path`/`Stroke`
//! to vizia's `vg::Canvas`/`Paint`/`Path` (Skia) is 1:1 mechanical - same
//! declarative 2D vector model, different library. No caching
//! (`canvas::Cache`'s equivalent) yet: Skia's rect/line fills are cheap
//! primitives, not lyon tessellation, so redrawing the grid every frame is
//! not the cost problem the goniometer dot-batching one was earlier this
//! project (see CLAP-vault bugs/all/2026-07-04-goniometer-batch-fill-host-freeze).

use std::sync::{Arc, Mutex};
use vizia::prelude::*;
use vizia::vg;

/// iced `Color::from_rgb`/`from_rgba` used 0.0-1.0 floats throughout the
/// ported code below; skia_safe's `Color` is 0-255 argb. Keeps every
/// color literal below an unchanged copy-paste from `shared-ui/canvas.rs`.
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
    // ponytail: plain f32s, not `impl Res<(f32,...,f32)>` - `Res` tuple impls
    // stop at 4 elements, and the one caller already rebuilds this View
    // inside a `Binding`, so there's no reactivity to gain from a 5th tuple slot.
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

/// `fmt_db`/readout row is plain text, not canvas drawing - left to the
/// caller (editor layout) same as the iced version split meter-canvas from
/// button-row.
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

                    // One draw_circle per dot - same reasoning as the iced
                    // version's per-dot fill(): batching overlapping dots
                    // into a single tessellated path was the exact bug in
                    // bugs/all/2026-07-04-goniometer-batch-fill-host-freeze.
                    // Skia doesn't share lyon's fill tessellator, but there's
                    // no reason to re-introduce the same batching shape here
                    // without measuring first.
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
// Unified FFT Spectrum Canvas - Spectrum + Goniometer + masking/resonance
// overlays for Lucent
// =============================================================================

#[derive(Clone)]
pub struct SpectrumCurve {
    pub spectrum: Vec<f32>,
    pub color: vg::Color,
    pub fill_alpha: f32,
    pub line_alpha: f32,
    pub line_width: f32,
}

#[derive(Clone)]
pub struct SpectrumConfig {
    pub min_db: f32,
    pub max_db: f32,
    pub sample_rate: f32,
    pub fft_size: usize,
    pub db_grid: Vec<f32>,
    pub freq_grid: Vec<f32>,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self {
            min_db: -70.0,
            max_db: -18.0,
            sample_rate: 44100.0,
            fft_size: 2048,
            db_grid: vec![-70.0, -50.0, -30.0, -18.0],
            freq_grid: vec![100.0, 1000.0, 10000.0],
        }
    }
}

pub struct SpectrumView {
    pub curves: Vec<SpectrumCurve>,
    pub config: SpectrumConfig,
    pub resonance_peaks: Vec<(usize, f32)>,
    pub masking: Vec<f32>,
}

impl SpectrumView {
    pub fn new(cx: &mut Context, data: Self) -> Handle<'_, Self> {
        data.build(cx, |_| {})
    }
}

impl View for SpectrumView {
    fn element(&self) -> Option<&'static str> {
        Some("spectrum")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &vg::Canvas) {
        let b = cx.bounds();
        canvas.translate((b.x, b.y));
        let (width, height) = (b.width(), b.height());
        let min_db = self.config.min_db;
        let max_db = self.config.max_db;
        let db_range = max_db - min_db;
        let sample_rate = self.config.sample_rate;

        canvas.draw_rect(vg::Rect::new(0.0, 0.0, width, height), &fill_paint(rgb(0.08, 0.08, 0.08)));

        let log_freq = |f: f32| -> f32 {
            ((f.ln() - 20.0f32.ln()) / (20000.0f32.ln() - 20.0f32.ln())).clamp(0.0, 1.0)
        };
        let db_to_y = |db: f32| -> f32 {
            let norm = ((db - min_db) / db_range).clamp(0.0, 1.0);
            height - norm * height
        };

        for &db in &self.config.db_grid {
            let y = db_to_y(db);
            line(canvas, 0.0, y, width, y, col(1.0, 1.0, 1.0, 0.12), 1.0);
        }
        for &f in &self.config.freq_grid {
            let x = log_freq(f) * width;
            line(canvas, x, 0.0, x, height, col(1.0, 1.0, 1.0, 0.06), 1.0);
        }

        for curve in &self.curves {
            if curve.spectrum.is_empty() {
                continue;
            }
            let fft_size = curve.spectrum.len() * 2;
            let smoothed = smooth_spectrum_third_octave(&curve.spectrum, fft_size, sample_rate);
            if smoothed.len() < 2 {
                continue;
            }
            let first_x = smoothed[0].0 * width;

            let mut fill_builder = vg::PathBuilder::new();
            fill_builder.move_to((first_x, height));
            for &(sx, db) in &smoothed {
                fill_builder.line_to((sx * width, db_to_y(db)));
            }
            let last_x = smoothed.last().unwrap().0 * width;
            fill_builder.line_to((last_x, height));
            fill_builder.close();
            let fill_path = fill_builder.detach();
            let (cr, cg, cb) = (
                curve.color.r() as f32 / 255.0,
                curve.color.g() as f32 / 255.0,
                curve.color.b() as f32 / 255.0,
            );
            canvas.draw_path(&fill_path, &fill_paint(col(cr, cg, cb, curve.fill_alpha)));

            let mut line_builder = vg::PathBuilder::new();
            line_builder.move_to((first_x, db_to_y(smoothed[0].1)));
            for &(sx, db) in &smoothed[1..] {
                line_builder.line_to((sx * width, db_to_y(db)));
            }
            let line_path = line_builder.detach();
            canvas.draw_path(&line_path, &stroke_paint(col(cr, cg, cb, curve.line_alpha), curve.line_width));
        }

        if !self.masking.is_empty() {
            let mask_fft_size = (self.masking.len() * 2) as f32;
            for (k, &db) in self.masking.iter().enumerate() {
                if db <= min_db {
                    continue;
                }
                let freq = k as f32 * sample_rate / mask_fft_size;
                if !(20.0..=20000.0).contains(&freq) {
                    continue;
                }
                let x = log_freq(freq) * width;
                let y = db_to_y(db);
                let severity = ((db - min_db) / db_range).clamp(0.0, 1.0);
                let color = col(0.95, 0.22, 0.18, 0.2 + severity * 0.6);
                line(canvas, x, height, x, y, color, 1.5);
            }
        }

        for (bin, score) in &self.resonance_peaks {
            let freq = *bin as f32 * sample_rate / self.config.fft_size as f32;
            if !(20.0..=20000.0).contains(&freq) {
                continue;
            }
            let x = log_freq(freq) * width;
            let alpha = (score / 20.0).clamp(0.2, 0.9);
            let marker_color = col(1.0, 0.6, 0.1, alpha);
            line(canvas, x, 0.0, x, height, marker_color, 1.5);
            let (s, my) = (3.0, 5.0);
            let mut diamond = vg::PathBuilder::new();
            diamond.move_to((x, my - s));
            diamond.line_to((x + s, my));
            diamond.line_to((x, my + s));
            diamond.line_to((x - s, my));
            diamond.close();
            canvas.draw_path(&diamond.detach(), &fill_paint(marker_color));
        }
    }
}

/// 1/3-octave (tapering to 1/20 at the top) fractional-band smoothing.
/// Byte-identical port of `shared-ui::smooth_spectrum_third_octave` - pure
/// math, no iced/vizia types involved, so this is a straight copy.
pub fn smooth_spectrum_third_octave(spectrum: &[f32], fft_size: usize, sample_rate: f32) -> Vec<(f32, f32)> {
    if spectrum.is_empty() {
        return Vec::new();
    }

    let log_min = 20.0_f32.ln();
    let log_max = 20000.0_f32.ln();
    let bin_hz = sample_rate / fft_size as f32;

    const DENOM_LOW: f32 = 3.0;
    const DENOM_HIGH: f32 = 20.0;
    const F_LOW: f32 = 500.0;
    const F_HIGH: f32 = 16000.0;
    let taper_lo = F_LOW.ln();
    let taper_hi = F_HIGH.ln();

    const STEPS: usize = 480;

    let len = spectrum.len();
    let power: Vec<f32> = spectrum.iter().map(|&db| 10.0_f32.powf(db * 0.1)).collect();

    let mut result: Vec<(f32, f32)> = (0..=STEPS)
        .map(|i| {
            let frac = i as f32 / STEPS as f32;
            let ln_fc = log_min + (log_max - log_min) * frac;
            let fc = ln_fc.exp();

            let t = ((ln_fc - taper_lo) / (taper_hi - taper_lo)).clamp(0.0, 1.0);
            let denom = DENOM_LOW + (DENOM_HIGH - DENOM_LOW) * t;

            let half = 2.0_f32.powf(1.0 / (2.0 * denom));
            const MIN_BIN: f32 = 1.0;
            let lo = (fc / half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
            let hi = (fc * half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);

            let avg_power = if hi - lo >= 1.0 {
                let i0 = lo.floor() as usize;
                let i1 = hi.floor() as usize;
                let mut sum = 0.0f32;
                if i0 == i1 {
                    sum = power[i0] * (hi - lo);
                } else {
                    sum += power[i0] * ((i0 + 1) as f32 - lo);
                    for p in &power[i0 + 1..i1] {
                        sum += *p;
                    }
                    sum += power[i1] * (hi - i1 as f32);
                }
                sum / (hi - lo)
            } else {
                let pos = (fc / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
                let i0 = pos.floor() as usize;
                let i1 = (i0 + 1).min(len - 1);
                let t_bin = pos - i0 as f32;
                power[i0] * (1.0 - t_bin) + power[i1] * t_bin
            };

            let avg_db = (10.0 * avg_power.max(1e-12).log10()).clamp(-90.0, 12.0);
            (frac, avg_db)
        })
        .collect();

    if result.len() >= 3 {
        let smoothed_db: Vec<f32> = result.iter().map(|&(_, db)| db).collect();
        for i in 1..result.len().saturating_sub(1) {
            result[i].1 = smoothed_db[i - 1] * 0.25 + smoothed_db[i] * 0.5 + smoothed_db[i + 1] * 0.25;
        }
    }

    result
}
