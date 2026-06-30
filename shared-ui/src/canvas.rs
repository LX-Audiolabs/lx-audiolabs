use nice_plug_iced::iced::widget::canvas::{self, Geometry, Path, Stroke};
use nice_plug_iced::iced::widget::{button, canvas as canvas_widget, column, container, Text};
use nice_plug_iced::iced::{Alignment, Background, Color, Element, Length, Padding, Point, Rectangle, Size};
use nice_plug_iced::iced::mouse::Cursor;
use std::sync::{Arc, Mutex};

use crate::widgets::bold_font;

// =============================================================================
// 1/3-Octave Spectrum Smoothing (shared across Meridian, Lucent, Aurum)
// =============================================================================

/// Smooth a raw FFT spectrum using variable-Q fractional-octave band averaging.
/// Returns `(log_x, db)` points ready for canvas drawing.
/// `log_x` is 0.0–1.0 on a log-frequency scale (20 Hz – 20 kHz).
///
/// The averaging bandwidth smoothly tapers from wide 1/3-octave bands at low
/// frequencies to narrow 1/12-octave bands at the top. This avoids over-smoothing
/// in the highs (where the linear FFT bin spacing packs many bins into a constant
/// octave-band width, causing extreme averaging) while keeping the lows calm.
pub fn smooth_spectrum_third_octave(
    spectrum: &[f32],
    fft_size: usize,
    sample_rate: f32,
) -> Vec<(f32, f32)> {
    if spectrum.is_empty() {
        return Vec::new();
    }

    let log_min = 20.0_f32.ln();
    let log_max = 20000.0_f32.ln();
    let bin_hz = sample_rate / fft_size as f32;

    // Variable-Q taper: band fraction denominator interpolated over log-frequency.
    // Below F_LOW = 1/DENOM_LOW octave (3 = calm, ruhig)
    // Above F_HIGH = 1/DENOM_HIGH octave (20 = detail oben, näher an SPAN)
    const DENOM_LOW: f32 = 3.0;
    const DENOM_HIGH: f32 = 20.0;
    const F_LOW: f32 = 500.0;
    const F_HIGH: f32 = 16000.0;
    let taper_lo = F_LOW.ln();
    let taper_hi = F_HIGH.ln();

    // Output grid: oversamples the finest (1/20-octave) bands ~2.4× for a clean polyline.
    const STEPS: usize = 480; // 10 octaves * 48 = ~1.7 px/point on 800px canvas

    let len = spectrum.len();
    // dB → linear power, so band averaging is energy-based (peak-treu, SPAN-like)
    // instead of dB-averaged (which flattens peaks). Computed once per frame.
    let power: Vec<f32> = spectrum.iter().map(|&db| 10.0_f32.powf(db * 0.1)).collect();

    // Light 3-point moving average on output to eliminate residual bin-boundary steps.
    // Edge bins keep their original value (no padding).
    let mut result: Vec<(f32, f32)> = (0..=STEPS)
        .map(|i| {
            let frac = i as f32 / STEPS as f32;
            let ln_fc = log_min + (log_max - log_min) * frac;
            let fc = ln_fc.exp();

            let t = ((ln_fc - taper_lo) / (taper_hi - taper_lo)).clamp(0.0, 1.0);
            let denom = DENOM_LOW + (DENOM_HIGH - DENOM_LOW) * t;

            let half = 2.0_f32.powf(1.0 / (2.0 * denom));
            // Fractional bin indices — no floor/ceil quantisation, so the value moves
            // continuously as fc sweeps → keine Treppen.
            // Lower bound is bin 1, never bin 0: the DC bin carries the DC offset and
            // window leakage and (unlike its neighbours) gets no down-tilt, so in the
            // power average it would dominate the Sub end and pull the curve up.
            const MIN_BIN: f32 = 1.0;
            let lo = (fc / half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
            let hi = (fc * half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);

            let avg_power = if hi - lo >= 1.0 {
                // Fractional-edge integration over [lo, hi] in the power domain.
                let i0 = lo.floor() as usize;
                let i1 = hi.floor() as usize;
                let mut sum = 0.0f32;
                if i0 == i1 {
                    sum = power[i0] * (hi - lo);
                } else {
                    sum += power[i0] * ((i0 + 1) as f32 - lo); // partial first bin
                    for p in &power[i0 + 1..i1] {
                        sum += *p; // full interior bins
                    }
                    sum += power[i1] * (hi - i1 as f32); // partial last bin
                }
                sum / (hi - lo)
            } else {
                // Band narrower than one bin: linear-interpolate power at fc.
                let pos = (fc / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
                let i0 = pos.floor() as usize;
                let i1 = (i0 + 1).min(len - 1);
                let t_bin = pos - i0 as f32;
                power[i0] * (1.0 - t_bin) + power[i1] * t_bin
            };

            // Back to dB, matching the input's clamp range.
            let avg_db = (10.0 * avg_power.max(1e-12).log10()).clamp(-90.0, 12.0);
            (frac, avg_db)
        })
        .collect();

    // Apply 3-point Hann-weighted moving average to smooth residual bin-boundary steps.
    // The frequency-domain averaging already handles the bulk; this just anti-aliases the output.
    if result.len() >= 3 {
        let smoothed_db: Vec<f32> = result.iter().map(|&(_, db)| db).collect();
        for i in 1..result.len().saturating_sub(1) {
            result[i].1 = smoothed_db[i - 1] * 0.25 + smoothed_db[i] * 0.5 + smoothed_db[i + 1] * 0.25;
        }
    }

    result
}

// =============================================================================
// Correlation meter canvas
// =============================================================================

pub struct CorrelationCanvas {
    pub val: f32,
}

impl CorrelationCanvas {
    pub fn new(val: f32) -> Self {
        Self { val }
    }
}

impl<Message> canvas::Program<Message> for CorrelationCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &nice_plug_iced::iced::Renderer,
        _theme: &nice_plug_iced::iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let height = bounds.height;

        frame.fill(
            &Path::rectangle(
                Point::new(0.0, height * 0.35),
                Size::new(width, height * 0.3),
            ),
            Color::from_rgb(0.08, 0.08, 0.08),
        );
        frame.fill(
            &Path::rectangle(
                Point::new(width * 0.5 - 1.0, 0.0),
                Size::new(2.0, height),
            ),
            Color::from_rgb(0.3, 0.3, 0.3),
        );

        let val = self.val.clamp(-1.0, 1.0);
        let norm_x = (val + 1.0) * 0.5;
        let cursor_x = width * norm_x;

        if val >= 0.0 {
            frame.fill(
                &Path::rectangle(
                    Point::new(width * 0.5, height * 0.35),
                    Size::new(width * (norm_x - 0.5), height * 0.3),
                ),
                Color::from_rgb(0.0, 0.75, 0.3),
            );
        } else {
            frame.fill(
                &Path::rectangle(
                    Point::new(cursor_x, height * 0.35),
                    Size::new(width * (0.5 - norm_x), height * 0.3),
                ),
                Color::from_rgb(1.0, 0.25, 0.25),
            );
        }
        frame.fill(
            &Path::rectangle(
                Point::new(cursor_x - 1.0, 0.0),
                Size::new(2.0, height),
            ),
            Color::from_rgb(1.0, 1.0, 1.0),
        );
        vec![frame.into_geometry()]
    }
}

// =============================================================================
// Balance canvas
// =============================================================================

pub struct BalanceCanvas {
    pub val: f32,
}

impl BalanceCanvas {
    pub fn new(val: f32) -> Self {
        Self { val }
    }
}

impl<Message> canvas::Program<Message> for BalanceCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &nice_plug_iced::iced::Renderer,
        _theme: &nice_plug_iced::iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let height = bounds.height;

        frame.fill(
            &Path::rectangle(Point::new(0.0, height * 0.35), Size::new(width, height * 0.3)),
            Color::from_rgb(0.08, 0.08, 0.08),
        );
        frame.fill(
            &Path::rectangle(Point::new(width * 0.5 - 1.0, 0.0), Size::new(2.0, height)),
            Color::from_rgb(0.3, 0.3, 0.3),
        );

        let val = self.val.clamp(-1.0, 1.0);
        let norm_x = (val + 1.0) * 0.5;
        let cursor_x = width * norm_x;

        if val >= 0.0 {
            frame.fill(
                &Path::rectangle(
                    Point::new(width * 0.5, height * 0.35),
                    Size::new(width * (norm_x - 0.5), height * 0.3),
                ),
                Color::from_rgb(1.0, 0.45, 0.1),
            );
        } else {
            frame.fill(
                &Path::rectangle(
                    Point::new(cursor_x, height * 0.35),
                    Size::new(width * (0.5 - norm_x), height * 0.3),
                ),
                Color::from_rgb(1.0, 0.45, 0.1),
            );
        }
        frame.fill(
            &Path::rectangle(Point::new(cursor_x - 1.0, 0.0), Size::new(2.0, height)),
            Color::from_rgb(1.0, 1.0, 1.0),
        );
        vec![frame.into_geometry()]
    }
}

/// Reusable Balance + Correlation meter block.
pub fn balance_correlation_block<'a, Message: 'a>(
    balance: f32,
    correlation: f32,
) -> Element<'a, Message> {
    column![
        nice_plug_iced::iced::widget::Text::new("Balance").size(10).color(Color::from_rgb(0.6, 0.6, 0.6)),
        canvas_widget(BalanceCanvas::new(balance))
            .width(Length::Fill)
            .height(Length::Fixed(20.0)),
        nice_plug_iced::iced::widget::Text::new("Correlation").size(10).color(Color::from_rgb(0.6, 0.6, 0.6)),
        canvas_widget(CorrelationCanvas::new(correlation))
            .width(Length::Fill)
            .height(Length::Fixed(20.0)),
    ]
    .spacing(4)
    .into()
}

// =============================================================================
// Output Peak canvas (single channel)
// =============================================================================

pub struct OutputPeakCanvas {
    pub peak_db: f32,
    pub hold_db: f32,
}

impl OutputPeakCanvas {
    pub fn new(peak_db: f32, hold_db: f32) -> Self {
        Self { peak_db, hold_db }
    }
}

impl<Message> canvas::Program<Message> for OutputPeakCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &nice_plug_iced::iced::Renderer,
        _theme: &nice_plug_iced::iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let height = bounds.height;

        frame.fill(&Path::rectangle(Point::ORIGIN, bounds.size()), Color::from_rgb(0.08, 0.08, 0.08));

        let min_db = -60.0f32;
        let max_db = 6.0f32;
        let db_range = max_db - min_db;

        let norm_peak = ((self.peak_db - min_db) / db_range).clamp(0.0, 1.0);
        let bar_h = height * norm_peak;

        let color = if self.peak_db > 0.0 {
            Color::from_rgb(1.0, 0.25, 0.25)
        } else if self.peak_db > -6.0 {
            Color::from_rgb(1.0, 0.55, 0.1)
        } else {
            Color::from_rgb(0.0, 0.75, 0.3)
        };

        frame.fill(
            &Path::rectangle(Point::new(1.0, height - bar_h), Size::new(width - 2.0, bar_h)),
            color,
        );

        if self.hold_db > min_db {
            let norm_hold = ((self.hold_db - min_db) / db_range).clamp(0.0, 1.0);
            let hold_y = height - height * norm_hold;
            frame.stroke(
                &Path::line(Point::new(0.0, hold_y), Point::new(width, hold_y)),
                Stroke { style: canvas::Style::Solid(Color::WHITE), width: 1.5, ..Default::default() },
            );
        }
        vec![frame.into_geometry()]
    }
}

// =============================================================================
// Stereo Meter canvas
// =============================================================================

pub struct StereoMeterCanvas {
    pub peak_l: f32,
    pub peak_r: f32,
    pub hold_l: f32,
    pub hold_r: f32,
    pub balance: f32,
}

impl StereoMeterCanvas {
    pub fn new(peak_l: f32, peak_r: f32, hold_l: f32, hold_r: f32, balance: f32) -> Self {
        Self { peak_l, peak_r, hold_l, hold_r, balance }
    }
}

impl<Message> canvas::Program<Message> for StereoMeterCanvas {
    type State = ();
    fn draw(
        &self,
        _state: &Self::State,
        renderer: &nice_plug_iced::iced::Renderer,
        _theme: &nice_plug_iced::iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let gap = 36.0;
        let bar_w = (w - gap) / 2.0;
        let min_db = -60.0f32;
        let max_db = 6.0f32;
        let db_range = max_db - min_db;

        frame.fill(&Path::rectangle(Point::ORIGIN, bounds.size()), Color::from_rgb(0.08, 0.08, 0.08));

        for (idx, (peak_db, hold_db)) in [(self.peak_l, self.hold_l), (self.peak_r, self.hold_r)].iter().enumerate() {
            let x = if idx == 0 { 0.0 } else { bar_w + gap };
            let norm_peak = ((*peak_db - min_db) / db_range).clamp(0.0, 1.0);
            let bar_h = h * norm_peak;
            let color = if *peak_db > 0.0 {
                Color::from_rgb(1.0, 0.25, 0.25)
            } else if *peak_db > -6.0 {
                Color::from_rgb(1.0, 0.55, 0.1)
            } else {
                Color::from_rgb(0.0, 0.75, 0.3)
            };
            frame.fill(
                &Path::rectangle(Point::new(x + 1.0, h - bar_h), Size::new(bar_w - 2.0, bar_h)),
                color,
            );
            if *hold_db > min_db {
                let norm_hold = ((*hold_db - min_db) / db_range).clamp(0.0, 1.0);
                let hold_y = h - h * norm_hold;
                frame.stroke(
                    &Path::line(Point::new(x, hold_y), Point::new(x + bar_w, hold_y)),
                    Stroke { style: canvas::Style::Solid(Color::WHITE), width: 1.5, ..Default::default() },
                );
            }
            let label_txt = if idx == 0 { "L" } else { "R" };
            frame.fill_text(canvas::Text {
                content: label_txt.to_string(),
                position: Point::new(x + bar_w * 0.5 - 3.0, h - 14.0),
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.45),
                size: nice_plug_iced::iced::Pixels(9.0),
                ..canvas::Text::default()
            });
        }

        let gx = bar_w;
        let center_x = gx + gap * 0.5;
        for (db_val, label) in [(-3.0f32, "-3"), (-6.0, "-6"), (-12.0, "-12"), (-24.0, "-24"), (-48.0, "-48")] {
            let norm = ((db_val - min_db) / db_range).clamp(0.0, 1.0);
            let y = h - h * norm;
            let tick_half = if label == "-3" || label == "-6" { 5.0 } else { 3.0 };
            frame.stroke(
                &Path::line(Point::new(center_x - tick_half, y), Point::new(center_x + tick_half, y)),
                Stroke { style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.45)), width: 0.8, ..Default::default() },
            );
            frame.fill_text(canvas::Text {
                content: label.to_string(),
                position: Point::new(gx + 1.0, y - 5.0),
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.60),
                size: nice_plug_iced::iced::Pixels(8.0),
                ..canvas::Text::default()
            });
        }

        let balance_norm = self.balance.clamp(-1.0, 1.0);
        // balance > 0 means L is louder (balance = (rms_l - rms_r)/sum), so the
        // dot must move LEFT toward the L meter. The previous `+` mirrored it
        // (left-heavy signal pushed the dot toward the R meter), the same way the
        // goniometer was mirrored before df08882.
        let cursor_x = center_x - balance_norm * (gap * 0.35);
        let cursor_y = h * 0.82;
        let cursor_color = if balance_norm.abs() < 0.08 {
            Color::from_rgb(0.0, 0.85, 0.35)
        } else {
            Color::from_rgb(1.0, 0.45, 0.1)
        };
        frame.fill(
            &Path::circle(Point::new(cursor_x, cursor_y), 3.5),
            cursor_color,
        );
        frame.stroke(
            &Path::line(Point::new(center_x, cursor_y - 5.0), Point::new(center_x, cursor_y + 5.0)),
            Stroke { style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.12)), width: 0.8, ..Default::default() },
        );

        vec![frame.into_geometry()]
    }
}

/// Output level block — stereo peak meters with dB scale and hold readout.
#[allow(clippy::too_many_arguments)]
pub fn output_level_block<'a, Message: Clone + 'a>(
    peak_l: f32,
    peak_r: f32,
    hold_l: f32,
    hold_r: f32,
    peak_hold_max: f32,
    on_reset: Message,
    balance: f32,
    meter_height: Length,
) -> Element<'a, Message> {
    let fmt_db = |v: f32| if v <= -60.0 { "-inf".to_string() } else { format!("{:.1}", v) };

    let readout_row = nice_plug_iced::iced::widget::row![
        button(Text::new(fmt_db(hold_l)).size(11).font(bold_font()))
            .on_press(on_reset.clone())
            .padding(Padding::ZERO)
            .style(|_theme, status| {
                let c = if status == button::Status::Hovered { Color::WHITE } else { Color::from_rgb(1.0, 0.45, 0.1) };
                button::Style { background: Some(Background::Color(Color::TRANSPARENT)), text_color: c, ..Default::default() }
            }),
        container(Text::new("dB").size(10).font(bold_font()).color(Color::from_rgb(0.8, 0.8, 0.8)))
            .width(Length::Fill)
            .center_x(Length::Fill),
        button(Text::new(fmt_db(hold_r)).size(11).font(bold_font()))
            .on_press(on_reset)
            .padding(Padding::ZERO)
            .style(|_theme, status| {
                let c = if status == button::Status::Hovered { Color::WHITE } else { Color::from_rgb(1.0, 0.45, 0.1) };
                button::Style { background: Some(Background::Color(Color::TRANSPARENT)), text_color: c, ..Default::default() }
            }),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let _ = peak_hold_max;

    column![
        Text::new("OUTPUT PEAK").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
        canvas_widget(StereoMeterCanvas::new(peak_l, peak_r, hold_l, hold_r, balance))
            .width(Length::Fill)
            .height(meter_height),
        readout_row,
    ]
    .spacing(4)
    .height(Length::Fill)
    .into()
}

// =============================================================================
// Goniometer (Vectorscope) — M/S XY plot from scope ring buffer
// =============================================================================

pub struct GoniometerCanvas {
    pub samples: Arc<Mutex<Vec<[f32; 2]>>>,
    pub write_pos: usize,
    pub correlation: f32,
}

impl<Message> canvas::Program<Message> for GoniometerCanvas {
    type State = ();
    fn draw(
        &self,
        _state: &Self::State,
        renderer: &nice_plug_iced::iced::Renderer,
        _theme: &nice_plug_iced::iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let cx = w * 0.5;
        let cy = h * 0.5;
        let scale = cx.min(cy) * 0.9;

        frame.fill(&Path::rectangle(Point::ORIGIN, bounds.size()), Color::from_rgb(0.06, 0.06, 0.06));

        let grid_stroke = Stroke {
            style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.08)),
            width: 1.0,
            ..Default::default()
        };
        frame.stroke(&Path::line(Point::new(cx, 0.0), Point::new(cx, h)), grid_stroke);
        frame.stroke(&Path::line(Point::new(0.0, cy), Point::new(w, cy)), grid_stroke);
        frame.stroke(&Path::line(Point::new(0.0, 0.0), Point::new(w, h)), grid_stroke);
        frame.stroke(&Path::line(Point::new(w, 0.0), Point::new(0.0, h)), grid_stroke);
        frame.stroke(
            &Path::circle(Point::new(cx, cy), scale),
            Stroke { style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.06)), width: 1.0, ..Default::default() },
        );

        if let Ok(samples) = self.samples.lock() {
            let n = samples.len();
            if n > 0 {
                let draw_count = n.min(2048);
                let third = draw_count / 3;
                let wp = self.write_pos % n;

                for group in 0..3u8 {
                    let alpha = match group { 0 => 0.12, 1 => 0.30, _ => 0.72 };
                    let dot_color = Color::from_rgba(0.1, 0.9, 0.5, alpha);
                    let start = group as usize * third;
                    let end = if group == 2 { draw_count } else { (group as usize + 1) * third };
                    for k in start..end {
                        let age = draw_count - 1 - k;
                        let idx = (wp + n - age - 1) % n;
                        let [l, r] = samples[idx];
                        let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
                        let m = (l + r) * inv_sqrt2;
                        let s = (l - r) * inv_sqrt2;
                        // Standard vectorscope convention: L = upper-left, R = upper-right.
                        // Side axis must point left for L-dominant signal, so negate s.
                        let sx = cx - s * scale;
                        let sy = cy - m * scale;
                        if sx >= 0.0 && sx <= w && sy >= 0.0 && sy <= h {
                            frame.fill(&Path::circle(Point::new(sx, sy), 0.9), dot_color);
                        }
                    }
                }
            }
        }

        let corr = self.correlation.clamp(-1.0, 1.0);
        let dot_color = if corr > 0.7 {
            Color::from_rgb(0.0, 0.85, 0.35)
        } else if corr >= 0.0 {
            Color::from_rgb(1.0, 0.45, 0.1)
        } else {
            Color::from_rgb(0.9, 0.2, 0.2)
        };
        let dot_x = 8.0;
        let dot_y = h - 8.0;
        frame.fill(&Path::circle(Point::new(dot_x, dot_y), 3.5), dot_color);
        let sign = if corr >= 0.0 { "+" } else { "" };
        frame.fill_text(canvas::Text {
            content: format!("{}{:.2}", sign, corr),
            position: Point::new(dot_x + 7.0, dot_y - 5.5),
            color: Color::from_rgb(1.0, 0.65, 0.3),
            size: nice_plug_iced::iced::Pixels(9.0),
            ..canvas::Text::default()
        });

        vec![frame.into_geometry()]
    }
}

// =============================================================================
// Unified FFT Spectrum Canvas — shared across Meridian, Lucent, Aurum
// =============================================================================
//
// Rendering only. The DSP side stays per-plugin: temporal smoothing happens in
// each lib.rs, and the EQ-transfer overlay is precomputed + cached in each editor
// (see the eq_params_dirty/settle pattern). The canvas is "dumb": it receives raw
// spectrum bins (smoothed spatially here via smooth_spectrum_third_octave) plus
// optional precomputed overlay points, and draws them. Empty Vecs / `None` draw
// nothing, so a consumer never pays for another plugin's features.

/// One spectrum curve (raw FFT magnitude in dB, pre-smoothing) with its style.
#[derive(Clone)]
pub struct SpectrumCurve {
    pub spectrum: Vec<f32>,
    pub color: Color,
    pub fill_alpha: f32,
    pub line_alpha: f32,
    pub line_width: f32,
}

/// Optional EQ-transfer overlay drawn on its own secondary dB axis (amber line).
/// Points are precomputed *outside* the canvas (editor) as `(log_x 0..1, dB)` so
/// the canvas carries no DSP knowledge. `grid_db` draws short right-edge reference
/// dashes (empty = none); the 0 dB line is emphasized.
#[derive(Clone)]
pub struct EqOverlay {
    pub points: Vec<(f32, f32)>,
    pub min_db: f32,
    pub max_db: f32,
    pub line_color: Color,
    pub fill_alpha: f32,
    pub grid_db: Vec<f32>,
}

/// Axis ranges, sample rate and grid for the spectrum canvas.
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

/// Unified spectrum canvas. Draw order: background → grids → curves (fill+line)
/// → masking overlay → resonance peaks → EQ overlay.
pub struct SpectrumCanvas {
    pub curves: Vec<SpectrumCurve>,
    pub config: SpectrumConfig,
    pub eq_overlay: Option<EqOverlay>,
    /// Lucent: resonance peaks as amber markers `(bin, score)`. Empty = none.
    pub resonance_peaks: Vec<(usize, f32)>,
    /// Lucent: masking collision map (dB per bin), red overlay. Empty = none.
    pub masking: Vec<f32>,
}

impl SpectrumCanvas {
    /// Single-curve spectrum (Meridian Sum, Aurum Mid) with no Lucent overlays.
    pub fn single(spectrum: Vec<f32>, color: Color, fill_alpha: f32, config: SpectrumConfig) -> Self {
        Self {
            curves: vec![SpectrumCurve { spectrum, color, fill_alpha, line_alpha: 0.9, line_width: 1.5 }],
            config,
            eq_overlay: None,
            resonance_peaks: Vec::new(),
            masking: Vec::new(),
        }
    }
}

impl<Message> canvas::Program<Message> for SpectrumCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &nice_plug_iced::iced::Renderer,
        _theme: &nice_plug_iced::iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let height = bounds.height;
        let min_db = self.config.min_db;
        let max_db = self.config.max_db;
        let db_range = max_db - min_db;
        let sample_rate = self.config.sample_rate;

        frame.fill(
            &Path::rectangle(Point::ORIGIN, bounds.size()),
            Color::from_rgb(0.08, 0.08, 0.08),
        );

        let log_freq = |f: f32| -> f32 {
            ((f.ln() - 20.0f32.ln()) / (20000.0f32.ln() - 20.0f32.ln())).clamp(0.0, 1.0)
        };
        let db_to_y = |db: f32| -> f32 {
            let norm = ((db - min_db) / db_range).clamp(0.0, 1.0);
            height - norm * height
        };

        // FFT dB grid lines (horizontal)
        for &db in &self.config.db_grid {
            let y = db_to_y(db);
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(width, y)),
                Stroke {
                    style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.12)),
                    width: 1.0,
                    ..Default::default()
                },
            );
        }

        // Frequency grid lines (vertical)
        for &f in &self.config.freq_grid {
            let x = log_freq(f) * width;
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, height)),
                Stroke {
                    style: canvas::Style::Solid(Color::from_rgba(1.0, 1.0, 1.0, 0.06)),
                    width: 1.0,
                    ..Default::default()
                },
            );
        }

        // EQ overlay right-edge reference dashes (secondary dB axis)
        if let Some(eq) = &self.eq_overlay {
            let eq_range = eq.max_db - eq.min_db;
            for &db in &eq.grid_db {
                let norm = ((db - eq.min_db) / eq_range).clamp(0.0, 1.0);
                let y = height - norm * height;
                let is_zero = db == 0.0;
                frame.stroke(
                    &Path::line(Point::new(width - 12.0, y), Point::new(width, y)),
                    Stroke {
                        style: canvas::Style::Solid(Color::from_rgba(1.0, 0.8, 0.4, if is_zero { 0.18 } else { 0.08 })),
                        width: if is_zero { 1.2 } else { 0.5 },
                        ..Default::default()
                    },
                );
            }
        }

        // Spectrum curves (1/3-octave smoothed, fill + line)
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

            let spec_fill = Path::new(|b| {
                b.move_to(Point::new(first_x, height));
                for &(sx, db) in &smoothed {
                    b.line_to(Point::new(sx * width, db_to_y(db)));
                }
                b.line_to(Point::new(smoothed.last().unwrap().0 * width, height));
                b.close();
            });
            let mut fill_color = curve.color;
            fill_color.a = curve.fill_alpha;
            frame.fill(&spec_fill, fill_color);

            let spec_line = Path::new(|b| {
                b.move_to(Point::new(first_x, db_to_y(smoothed[0].1)));
                for &(sx, db) in &smoothed[1..] {
                    b.line_to(Point::new(sx * width, db_to_y(db)));
                }
            });
            let mut line_color = curve.color;
            line_color.a = curve.line_alpha;
            frame.stroke(
                &spec_line,
                Stroke { style: canvas::Style::Solid(line_color), width: curve.line_width, ..Default::default() },
            );
        }

        // Masking collision overlay (Lucent) — bottom-anchored red area
        if !self.masking.is_empty() {
            let mask_fft_size = (self.masking.len() * 2) as f32;
            let mask_fill = Path::new(|b| {
                b.move_to(Point::new(0.0, height));
                let mut last_x = 0.0;
                for (k, &db) in self.masking.iter().enumerate() {
                    let freq = k as f32 * sample_rate / mask_fft_size;
                    if !(20.0..=20000.0).contains(&freq) {
                        continue;
                    }
                    let x = log_freq(freq) * width;
                    let y = if db > min_db { db_to_y(db) } else { height };
                    b.line_to(Point::new(x, y));
                    last_x = x;
                }
                b.line_to(Point::new(last_x, height));
                b.close();
            });
            frame.fill(&mask_fill, Color::from_rgba(0.95, 0.22, 0.18, 0.50));
        }

        // Resonance peaks (Lucent) — amber vertical markers + diamond
        for (bin, score) in &self.resonance_peaks {
            let freq = *bin as f32 * sample_rate / self.config.fft_size as f32;
            if !(20.0..=20000.0).contains(&freq) {
                continue;
            }
            let x = log_freq(freq) * width;
            let alpha = (score / 20.0).clamp(0.2, 0.9);
            let marker_color = Color::from_rgba(1.0, 0.6, 0.1, alpha);
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, height)),
                Stroke { style: canvas::Style::Solid(marker_color), width: 1.5, ..Default::default() },
            );
            let s = 3.0;
            let my = 5.0;
            let diamond = Path::new(|b| {
                b.move_to(Point::new(x, my - s));
                b.line_to(Point::new(x + s, my));
                b.line_to(Point::new(x, my + s));
                b.line_to(Point::new(x - s, my));
                b.close();
            });
            frame.fill(&diamond, marker_color);
        }

        // EQ-transfer overlay (amber line + soft fill to 0 dB), secondary axis
        if let Some(eq) = &self.eq_overlay {
            if eq.points.len() > 1 {
                let eq_range = eq.max_db - eq.min_db;
                let db_to_y_eq = |db: f32| -> f32 {
                    let norm = ((db - eq.min_db) / eq_range).clamp(0.0, 1.0);
                    height - norm * height
                };
                let zero_y = db_to_y_eq(0.0);

                let eq_fill = Path::new(|b| {
                    b.move_to(Point::new(eq.points[0].0 * width, zero_y));
                    for &(lx, db) in &eq.points {
                        b.line_to(Point::new(lx * width, db_to_y_eq(db)));
                    }
                    b.line_to(Point::new(eq.points.last().unwrap().0 * width, zero_y));
                    b.close();
                });
                let mut fc = eq.line_color;
                fc.a = eq.fill_alpha;
                frame.fill(&eq_fill, fc);

                let eq_line = Path::new(|b| {
                    b.move_to(Point::new(eq.points[0].0 * width, db_to_y_eq(eq.points[0].1)));
                    for &(lx, db) in &eq.points[1..] {
                        b.line_to(Point::new(lx * width, db_to_y_eq(db)));
                    }
                });
                frame.stroke(
                    &eq_line,
                    Stroke { style: canvas::Style::Solid(eq.line_color), width: 1.3, ..Default::default() },
                );
            }
        }

        vec![frame.into_geometry()]
    }
}
