// Meridian — Track and group shaper (truce port).
//
// 5-band EQ with slope control, soft-knee compressor, exciter, tube warmth,
// tilt EQ, stereo width/pan, and Auto Loud LUFS metering.
//
// Signal chain:
//   HPF/LPF → 5-band Series EQ → Tilt → Exciter → Compressor →
//   Warmth → Inflate → Pan → Stereo Width → Mono/Delta → Gain → clamp

use realfft::RealFftPlanner;
use shared_dsp::state_migration;
use std::f32::consts::FRAC_PI_4;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use truce::prelude::*;
use truce_core::editor::Editor;
use truce_core::state::StateLoadError;
use truce_vizia::ViziaEditor;

use shared_analysis::{SCOPE_BUFFER_LEN, SPECTRUM_BINS, SharedState, SnapFFT, SnapMode};
use shared_dsp::{
    AutoLoudMeter, Biquad, Compressor, DBTP_CEILING, FtzDazGuard, LR2Crossover, TiltEq,
};

mod editor;
mod vizia_canvas;

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 660;

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[inline]
fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[inline]
fn gain_to_db(gain: f32) -> f32 {
    if gain < 1e-9 {
        -90.0
    } else {
        20.0 * gain.log10()
    }
}

/// Soft clipping — odd harmonics (Exciter).
#[inline]
fn soft_clip(x: f32) -> f32 {
    let abs_x = x.abs();
    if abs_x <= 1.0 {
        x - (x * x * x) / 3.0
    } else if x > 0.0 {
        2.0 / 3.0
    } else {
        -2.0 / 3.0
    }
}

/// Tube-style saturation — DC bias shifts operating point for even harmonics (Warmth).
#[inline]
fn tube_warm(x: f32) -> f32 {
    const BIAS: f32 = 0.1;
    (x + BIAS).tanh() - BIAS.tanh()
}

/// Approximated Oxford-Inflator-style loudness/density waveshaper (Inflate).
/// Not a Sonnox algorithm clone — the "probability density shifting" process is
/// patented/undocumented. `curve` -50..+50: negative = subtle/tight, 0 = balanced,
/// positive = fat/loud. Drive-varying tanh, always finite for finite input.
/// Fix 2026-07-03: normalize by `drive` (like `tube_warm`), not `tanh(drive)` —
/// the old `/tanh(drive)` normalization forced unity gain at x=1 (full scale) but
/// blew up the small-signal gain (slope at x=0) to `drive/tanh(drive)`, i.e. up to
/// 6.0x at CURVE=+50 and 2.3x already at CURVE=0 ("balanced"). Quiet/mid-level
/// program material got boosted and colored far more than intended — harsh even at
/// neutral. Community reverse-engineering of the real Oxford Inflator (small-signal
/// gain coefficient a1 = 1 + (curve+50)/100, i.e. 1.0..2.0 linear with curve) puts
/// the target gain range at 1.0..2.0. `/drive` normalization gives unity slope at
/// x=0 for the tanh term; multiplying by `gain` (1..2) reproduces that range while
/// keeping curvature/drive (1..6) untouched from the 2026-07-02(b) fix.
#[inline]
fn inflate_shape(x: f32, curve: f32) -> f32 {
    let t = (curve + 50.0) / 100.0; // -50..+50 -> 0..1
    let drive = 1.0 + t * t * 5.0; // quadratic: 0=1 (clean), 0.5=2.25 (gentle), 1=6 (fat/aggressive)
    let gain = 1.0 + t; // 1..2, small-signal gain matching Oxford's a1 range
    (x * drive).tanh() * gain / drive
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct MeridianParams {
    // HPF / LPF
    #[param(
        name = "Low Cut",
        default = 2.0,
        range = "log(2.0, 2000.0)",
        unit = "Hz",
        group = "Filter"
    )]
    pub hpf_freq: FloatParam,
    #[param(
        name = "High Cut",
        default = 35000.0,
        range = "log(200.0, 35000.0)",
        unit = "Hz",
        group = "Filter"
    )]
    pub lpf_freq: FloatParam,
    #[param(
        name = "Cut Slope",
        default = 0,
        range = "discrete(0, 1)",
        group = "Filter"
    )]
    pub cut_slope: IntParam,

    // Bass EQ shelf
    #[param(
        name = "Lo Shelf Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Lo Shelf"
    )]
    pub bass_gain: FloatParam,
    #[param(
        name = "Lo Shelf Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Lo Shelf"
    )]
    pub bass_slope: IntParam,

    // Lo-Mid EQ
    #[param(
        name = "Lo-Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Lo-Mid"
    )]
    pub lo_mid_gain: FloatParam,
    #[param(
        name = "Lo-Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Lo-Mid"
    )]
    pub lo_mid_slope: IntParam,

    // Mid EQ
    #[param(
        name = "Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Mid"
    )]
    pub mid_gain: FloatParam,
    #[param(
        name = "Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Mid"
    )]
    pub mid_slope: IntParam,

    // High EQ
    #[param(
        name = "Hi-Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Hi-Mid"
    )]
    pub high_gain: FloatParam,
    #[param(
        name = "Hi-Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Hi-Mid"
    )]
    pub high_slope: IntParam,

    // Excite (high shelf)
    #[param(
        name = "Hi Shelf Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Hi Shelf"
    )]
    pub excite_gain: FloatParam,
    #[param(
        name = "Hi Shelf Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Hi Shelf"
    )]
    pub excite_slope: IntParam,

    // EQ band frequencies
    #[param(
        name = "Lo Shelf Freq",
        default = 80.0,
        range = "log(40.0, 200.0)",
        unit = "Hz",
        group = "EQ/Lo Shelf"
    )]
    pub eq_freq_1: FloatParam,
    #[param(
        name = "Lo-Mid Freq",
        default = 300.0,
        range = "log(150.0, 800.0)",
        unit = "Hz",
        group = "EQ/Lo-Mid"
    )]
    pub eq_freq_2: FloatParam,
    #[param(
        name = "Mid Freq",
        default = 1000.0,
        range = "log(500.0, 3000.0)",
        unit = "Hz",
        group = "EQ/Mid"
    )]
    pub eq_freq_3: FloatParam,
    #[param(
        name = "Hi-Mid Freq",
        default = 4000.0,
        range = "log(2000.0, 10000.0)",
        unit = "Hz",
        group = "EQ/Hi-Mid"
    )]
    pub eq_freq_4: FloatParam,
    #[param(
        name = "Hi Shelf Freq",
        default = 12000.0,
        range = "log(6000.0, 20000.0)",
        unit = "Hz",
        group = "EQ/Hi Shelf"
    )]
    pub eq_freq_5: FloatParam,

    // Tilt EQ
    #[param(
        name = "Tilt",
        default = 0.0,
        range = "linear(-1.5, 1.5)",
        unit = "dB",
        group = "Tilt"
    )]
    pub tilt_gain: FloatParam,

    // Warmth (tube saturation)
    #[param(
        name = "Warmth Drive",
        default = 0.0,
        range = "linear(0.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Saturator"
    )]
    pub warmth_drive: FloatParam,
    #[param(
        name = "Warmth Mix",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Saturator"
    )]
    pub warmth_mix: FloatParam,

    // Exciter (HF saturation)
    #[param(
        name = "Excite Amount",
        default = 0.0,
        range = "linear(0.0, 30.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Exciter"
    )]
    pub excite_amount: FloatParam,
    #[param(
        name = "Excite Blend",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Exciter"
    )]
    pub excite_blend: FloatParam,
    #[param(
        name = "Excite Freq",
        default = 8000.0,
        range = "log(6000.0, 12000.0)",
        unit = "Hz",
        group = "Exciter"
    )]
    pub excite_freq: FloatParam,

    // Compressor
    #[param(
        name = "Comp Threshold",
        default = 0.0,
        range = "linear(-30.0, 0.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_threshold: FloatParam,
    #[param(
        name = "Comp Mix",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_mix: FloatParam,
    #[param(
        name = "Comp Attack",
        default = 15.0,
        range = "linear(5.0, 50.0)",
        unit = "ms",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_attack: FloatParam,
    #[param(
        name = "Comp Release",
        default = 120.0,
        range = "linear(50.0, 300.0)",
        unit = "ms",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_release: FloatParam,
    #[param(
        name = "Comp Ratio",
        default = 2.0,
        range = "linear(1.5, 4.0)",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_character: FloatParam,
    #[param(
        name = "Comp Makeup",
        default = 0.0,
        range = "linear(0.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_makeup: FloatParam,

    // Inflate (Oxford-Inflator-inspired loudness/density waveshaper)
    #[param(
        name = "Inflate Effect",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Inflate"
    )]
    pub inflate_effect: FloatParam,
    #[param(
        name = "Inflate Curve",
        default = 0.0,
        range = "linear(-50.0, 50.0)",
        smooth = "linear(20)",
        group = "Inflate"
    )]
    pub inflate_curve: FloatParam,
    #[param(name = "Inflate Band Split", default = 0, group = "Inflate")]
    pub inflate_band_split: BoolParam,
    #[param(name = "Inflate Clip", default = 0, group = "Inflate")]
    pub inflate_clip: BoolParam,

    // Stereo Width
    #[param(
        name = "Stereo Width",
        default = 100.0,
        range = "linear(0.0, 200.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub stereo_width: FloatParam,
    // Pan
    #[param(
        name = "Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub pan: FloatParam,
    // Output Gain
    #[param(
        name = "Output Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub output_gain: FloatParam,

    // States
    #[param(name = "Mono Sum", default = 0, group = "Stereo/Routing")]
    pub mono_active: BoolParam,
    #[param(name = "Delta Diff", default = 0, group = "Stereo/Routing")]
    pub delta_active: BoolParam,
    #[param(name = "Bypass", default = 0, group = "Stereo/Routing")]
    pub bypass_active: BoolParam,

    #[skip]
    pub shared: Arc<SharedState>,
}

impl MeridianParams {
    /// Real value display for `unit = "%"` params: our plain values are
    /// already the percent number (e.g. `100.0` means `100%`), not a
    /// 0.0-1.0 fraction. `truce_params::format_param_value`'s built-in
    /// Percent case multiplies by 100 assuming the latter, so it would
    /// show `10000%` for a real 100% value without this override.
    fn fmt_pct(&self, value: f64) -> String {
        format!("{value:.1}%")
    }
}

// ─── Plugin ──────────────────────────────────────────────────────────────────

pub struct Meridian {
    params: Arc<MeridianParams>,

    // HPF/LPF
    hpf_l: Biquad,
    hpf_r: Biquad,
    lpf_l: Biquad,
    lpf_r: Biquad,
    hpf2_l: Biquad,
    hpf2_r: Biquad,
    lpf2_l: Biquad,
    lpf2_r: Biquad,

    // EQ bands
    bass_l: Biquad,
    bass_r: Biquad,
    lo_mid_l: Biquad,
    lo_mid_r: Biquad,
    mid_l: Biquad,
    mid_r: Biquad,
    high_l: Biquad,
    high_r: Biquad,
    excite_l: Biquad,
    excite_r: Biquad,

    tilt_l: TiltEq,
    tilt_r: TiltEq,

    excite_hp_l: Biquad,
    excite_hp_r: Biquad,

    compressor: Compressor,

    // Inflate band-split (LF/MF/HF, Linkwitz-Riley, sums flat)
    xo_inflate_lo_l: LR2Crossover,
    xo_inflate_lo_r: LR2Crossover,
    xo_inflate_hi_l: LR2Crossover,
    xo_inflate_hi_r: LR2Crossover,

    // Crossover analysis (for GUI visualizer)
    xo_bass_mid_l: LR2Crossover,
    xo_bass_mid_r: LR2Crossover,
    xo_low_bass_l: LR2Crossover,
    xo_low_bass_r: LR2Crossover,
    xo_mid_high_l: LR2Crossover,
    xo_mid_high_r: LR2Crossover,
    xo_highmid_high_l: LR2Crossover,
    xo_highmid_high_r: LR2Crossover,

    // Smoothed states
    correlation_decay_coef: f32,
    smoothed_band_power: [f32; 5],
    corr_avg_lr: f32,
    corr_avg_l2: f32,
    corr_avg_r2: f32,
    peak_hold_value: f32,
    peak_hold_l_value: f32,
    peak_hold_r_value: f32,

    // AUTO LOUD
    auto_loud_in: AutoLoudMeter,
    auto_loud_pre_sat: AutoLoudMeter,
    auto_loud_out: AutoLoudMeter,
    pre_sat_buf_l: Vec<f32>,
    pre_sat_buf_r: Vec<f32>,

    // Goniometer
    scope_vis_envelope: f32,

    // FFT
    fft_planner: RealFftPlanner<f32>,
    fft_input: Vec<f32>,
    fft_write_pos: usize,
    fft_hann: Vec<f32>,
    fft_windowed: Vec<f32>,
    fft_output_cache: Vec<realfft::num_complex::Complex<f32>>,

    // SNAP
    snap_fft: SnapFFT,

    // Dirty-flag caches
    cached_hpf_freq: f32,
    cached_lpf_freq: f32,
    cached_cut_slope: i64,
    cached_bass_gain: f32,
    cached_bass_slope: i64,
    cached_lo_mid_gain: f32,
    cached_lo_mid_slope: i64,
    cached_mid_gain: f32,
    cached_mid_slope: i64,
    cached_high_gain: f32,
    cached_high_slope: i64,
    cached_excite_gain: f32,
    cached_excite_slope: i64,
    cached_eq_freq_1: f32,
    cached_eq_freq_2: f32,
    cached_eq_freq_3: f32,
    cached_eq_freq_4: f32,
    cached_eq_freq_5: f32,
    cached_tilt_gain: f32,
    cached_excite_freq: f32,
    cached_sample_rate: f32,
}

impl Meridian {
    pub fn new(params: Arc<MeridianParams>) -> Self {
        let fft_size = SPECTRUM_BINS * 2;
        Self {
            params,
            hpf_l: Biquad::new(),
            hpf_r: Biquad::new(),
            lpf_l: Biquad::new(),
            lpf_r: Biquad::new(),
            hpf2_l: Biquad::new(),
            hpf2_r: Biquad::new(),
            lpf2_l: Biquad::new(),
            lpf2_r: Biquad::new(),
            bass_l: Biquad::new(),
            bass_r: Biquad::new(),
            lo_mid_l: Biquad::new(),
            lo_mid_r: Biquad::new(),
            mid_l: Biquad::new(),
            mid_r: Biquad::new(),
            high_l: Biquad::new(),
            high_r: Biquad::new(),
            excite_l: Biquad::new(),
            excite_r: Biquad::new(),
            tilt_l: TiltEq::new(),
            tilt_r: TiltEq::new(),
            excite_hp_l: Biquad::new(),
            excite_hp_r: Biquad::new(),
            compressor: Compressor::new(),
            xo_inflate_lo_l: LR2Crossover::new(),
            xo_inflate_lo_r: LR2Crossover::new(),
            xo_inflate_hi_l: LR2Crossover::new(),
            xo_inflate_hi_r: LR2Crossover::new(),
            xo_bass_mid_l: LR2Crossover::new(),
            xo_bass_mid_r: LR2Crossover::new(),
            xo_low_bass_l: LR2Crossover::new(),
            xo_low_bass_r: LR2Crossover::new(),
            xo_mid_high_l: LR2Crossover::new(),
            xo_mid_high_r: LR2Crossover::new(),
            xo_highmid_high_l: LR2Crossover::new(),
            xo_highmid_high_r: LR2Crossover::new(),
            correlation_decay_coef: 0.005,
            smoothed_band_power: [0.0; 5],
            corr_avg_lr: 0.0,
            corr_avg_l2: 0.0,
            corr_avg_r2: 0.0,
            peak_hold_value: -90.0,
            peak_hold_l_value: -90.0,
            peak_hold_r_value: -90.0,
            auto_loud_in: AutoLoudMeter::new(44100.0),
            auto_loud_pre_sat: AutoLoudMeter::new(44100.0),
            auto_loud_out: AutoLoudMeter::new(44100.0),
            pre_sat_buf_l: Vec::new(),
            pre_sat_buf_r: Vec::new(),
            scope_vis_envelope: 1e-4,
            fft_planner: RealFftPlanner::new(),
            fft_input: vec![0.0f32; fft_size],
            fft_write_pos: 0,
            fft_hann: (0..fft_size)
                .map(|i| {
                    let n = fft_size;
                    let pi2 = 2.0 * std::f32::consts::PI;
                    let norm = i as f32 / (n - 1) as f32;
                    let a0 = 0.35875;
                    let a1 = 0.48829;
                    let a2 = 0.14128;
                    let a3 = 0.01168;
                    a0 - a1 * (pi2 * norm).cos() + a2 * (2.0 * pi2 * norm).cos()
                        - a3 * (3.0 * pi2 * norm).cos()
                })
                .collect(),
            fft_windowed: vec![0.0f32; fft_size],
            fft_output_cache: vec![
                realfft::num_complex::Complex::new(0.0f32, 0.0f32);
                SPECTRUM_BINS + 1
            ],
            snap_fft: SnapFFT::new(),
            cached_hpf_freq: -999.0,
            cached_lpf_freq: -999.0,
            cached_cut_slope: -999,
            cached_bass_gain: -999.0,
            cached_bass_slope: -999,
            cached_lo_mid_gain: -999.0,
            cached_lo_mid_slope: -999,
            cached_mid_gain: -999.0,
            cached_mid_slope: -999,
            cached_high_gain: -999.0,
            cached_high_slope: -999,
            cached_excite_gain: -999.0,
            cached_excite_slope: -999,
            cached_eq_freq_1: -999.0,
            cached_eq_freq_2: -999.0,
            cached_eq_freq_3: -999.0,
            cached_eq_freq_4: -999.0,
            cached_eq_freq_5: -999.0,
            cached_tilt_gain: -999.0,
            cached_excite_freq: -999.0,
            cached_sample_rate: -999.0,
        }
    }
}

// ─── PluginLogic ─────────────────────────────────────────────────────────────

impl PluginLogic for Meridian {
    type Params = MeridianParams;

    fn reset(&mut self, sr: f64, _max: usize) {
        let sr = sr as f32;

        self.compressor.set_sample_rate(sr);

        self.xo_inflate_lo_l.set_cutoff(300.0, sr);
        self.xo_inflate_lo_r.set_cutoff(300.0, sr);
        self.xo_inflate_hi_l.set_cutoff(3000.0, sr);
        self.xo_inflate_hi_r.set_cutoff(3000.0, sr);

        // Recreate Auto-Loud meters at host sample rate
        self.auto_loud_in = AutoLoudMeter::new(sr);
        self.auto_loud_pre_sat = AutoLoudMeter::new(sr);
        self.auto_loud_out = AutoLoudMeter::new(sr);

        // Crossover frequencies (constant for GUI visualizer)
        for (xo_l, xo_r, fc) in [
            (&mut self.xo_bass_mid_l, &mut self.xo_bass_mid_r, 400.0),
            (&mut self.xo_low_bass_l, &mut self.xo_low_bass_r, 100.0),
            (&mut self.xo_mid_high_l, &mut self.xo_mid_high_r, 1500.0),
            (
                &mut self.xo_highmid_high_l,
                &mut self.xo_highmid_high_r,
                8000.0,
            ),
        ] {
            xo_l.set_cutoff(fc, sr);
            xo_r.set_cutoff(fc, sr);
        }

        self.correlation_decay_coef = 1.0 - (-1.0 / (0.1 * sr)).exp();
        self.cached_sample_rate = sr;

        self.params
            .shared
            .sample_rate
            .store(sr, std::sync::atomic::Ordering::Release);
    }

    fn process(
        &mut self,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        let _ftz = FtzDazGuard::new();

        if buffer.num_input_channels() < 2 {
            return ProcessStatus::Normal;
        }

        let bypass = self.params.bypass_active.value();

        // Reset analysis
        if self
            .params
            .shared
            .reset_analysis
            .swap(false, Ordering::Acquire)
        {
            self.fft_input.fill(0.0);
            self.fft_write_pos = 0;
            if let Ok(mut avg) = self.params.shared.spectrum_avg.try_lock() {
                avg.fill(-90.0);
            }
            if let Ok(mut bins) = self.params.shared.spectrum_bins.try_lock() {
                bins.fill(-90.0);
            }
            self.hpf_l.reset();
            self.hpf_r.reset();
            self.lpf_l.reset();
            self.lpf_r.reset();
            self.hpf2_l.reset();
            self.hpf2_r.reset();
            self.lpf2_l.reset();
            self.lpf2_r.reset();
            self.bass_l.reset();
            self.bass_r.reset();
            self.lo_mid_l.reset();
            self.lo_mid_r.reset();
            self.mid_l.reset();
            self.mid_r.reset();
            self.high_l.reset();
            self.high_r.reset();
            self.excite_l.reset();
            self.excite_r.reset();
            self.tilt_l.reset();
            self.tilt_r.reset();
            self.excite_hp_l.reset();
            self.excite_hp_r.reset();
            self.xo_inflate_lo_l.reset();
            self.xo_inflate_lo_r.reset();
            self.xo_inflate_hi_l.reset();
            self.xo_inflate_hi_r.reset();
            self.xo_bass_mid_l.reset();
            self.xo_bass_mid_r.reset();
            self.xo_low_bass_l.reset();
            self.xo_low_bass_r.reset();
            self.xo_mid_high_l.reset();
            self.xo_mid_high_r.reset();
            self.xo_highmid_high_l.reset();
            self.xo_highmid_high_r.reset();
        }

        let sample_rate = self.params.shared.sample_rate.load(Ordering::Acquire);
        self.compressor.set_sample_rate(sample_rate);

        // Dirty-flag coefficient update
        let hpf_f = self.params.hpf_freq.raw_target() as f32;
        let lpf_f = self.params.lpf_freq.raw_target() as f32;
        let cut_slope_val = self.params.cut_slope.value();
        let bass_gain_val = self.params.bass_gain.raw_target() as f32;
        let bass_slope_val = self.params.bass_slope.value();
        let lo_mid_gain_val = self.params.lo_mid_gain.raw_target() as f32;
        let lo_mid_slope_val = self.params.lo_mid_slope.value();
        let mid_gain_val = self.params.mid_gain.raw_target() as f32;
        let mid_slope_val = self.params.mid_slope.value();
        let high_gain_val = self.params.high_gain.raw_target() as f32;
        let high_slope_val = self.params.high_slope.value();
        let excite_gain_val = self.params.excite_gain.raw_target() as f32;
        let excite_slope_val = self.params.excite_slope.value();
        let eq_f1 = self.params.eq_freq_1.raw_target() as f32;
        let eq_f2 = self.params.eq_freq_2.raw_target() as f32;
        let eq_f3 = self.params.eq_freq_3.raw_target() as f32;
        let eq_f4 = self.params.eq_freq_4.raw_target() as f32;
        let eq_f5 = self.params.eq_freq_5.raw_target() as f32;
        let tilt_db = self.params.tilt_gain.raw_target() as f32;
        let excite_freq = self.params.excite_freq.raw_target() as f32;

        let slope_val = |slope_idx: i64| -> f32 {
            match slope_idx {
                0 => 0.5,
                1 => 1.0,
                _ => 2.0,
            }
        };
        let q_val = |slope_idx: i64| -> f32 {
            match slope_idx {
                0 => 0.4,
                1 => 0.7,
                _ => 1.5,
            }
        };

        let coef_dirty = sample_rate != self.cached_sample_rate;
        self.cached_sample_rate = sample_rate;

        if hpf_f != self.cached_hpf_freq
            || lpf_f != self.cached_lpf_freq
            || cut_slope_val != self.cached_cut_slope
            || coef_dirty
        {
            // Safety: reset filter state on >4 octave jump (right-click reset fix)
            let hpf_jump = if self.cached_hpf_freq > 1.0 && hpf_f > 1.0 {
                (hpf_f / self.cached_hpf_freq).max(self.cached_hpf_freq / hpf_f)
            } else if hpf_f != self.cached_hpf_freq {
                4.1
            } else {
                1.0
            };
            let lpf_jump = if self.cached_lpf_freq > 1.0 && lpf_f > 1.0 {
                (lpf_f / self.cached_lpf_freq).max(self.cached_lpf_freq / lpf_f)
            } else if lpf_f != self.cached_lpf_freq {
                4.1
            } else {
                1.0
            };
            if hpf_jump > 4.0 {
                self.hpf_l.reset();
                self.hpf_r.reset();
                self.hpf2_l.reset();
                self.hpf2_r.reset();
            }
            if lpf_jump > 4.0 {
                self.lpf_l.reset();
                self.lpf_r.reset();
                self.lpf2_l.reset();
                self.lpf2_r.reset();
            }
            self.cached_hpf_freq = hpf_f;
            self.cached_lpf_freq = lpf_f;
            self.cached_cut_slope = cut_slope_val;
            const Q1: f32 = 0.541_196_1;
            const Q2: f32 = 1.306_563;
            if cut_slope_val >= 1 {
                self.hpf_l.set_butterworth_hp_q(hpf_f, Q1, sample_rate);
                self.hpf_r.set_butterworth_hp_q(hpf_f, Q1, sample_rate);
                self.hpf2_l.set_butterworth_hp_q(hpf_f, Q2, sample_rate);
                self.hpf2_r.set_butterworth_hp_q(hpf_f, Q2, sample_rate);
                self.lpf_l.set_butterworth_lp_q(lpf_f, Q1, sample_rate);
                self.lpf_r.set_butterworth_lp_q(lpf_f, Q1, sample_rate);
                self.lpf2_l.set_butterworth_lp_q(lpf_f, Q2, sample_rate);
                self.lpf2_r.set_butterworth_lp_q(lpf_f, Q2, sample_rate);
            } else {
                self.hpf_l.set_butterworth_hp(hpf_f, sample_rate);
                self.hpf_r.set_butterworth_hp(hpf_f, sample_rate);
                self.lpf_l.set_butterworth_lp(lpf_f, sample_rate);
                self.lpf_r.set_butterworth_lp(lpf_f, sample_rate);
                self.hpf2_l.set_identity();
                self.hpf2_r.set_identity();
                self.lpf2_l.set_identity();
                self.lpf2_r.set_identity();
            }
        }

        if bass_gain_val != self.cached_bass_gain
            || bass_slope_val != self.cached_bass_slope
            || eq_f1 != self.cached_eq_freq_1
            || coef_dirty
        {
            self.cached_bass_gain = bass_gain_val;
            self.cached_bass_slope = bass_slope_val;
            self.cached_eq_freq_1 = eq_f1;
            let bass_slope = slope_val(bass_slope_val);
            self.bass_l
                .set_low_shelf(eq_f1, bass_gain_val, bass_slope, sample_rate);
            self.bass_r
                .set_low_shelf(eq_f1, bass_gain_val, bass_slope, sample_rate);
        }

        if lo_mid_gain_val != self.cached_lo_mid_gain
            || lo_mid_slope_val != self.cached_lo_mid_slope
            || eq_f2 != self.cached_eq_freq_2
            || coef_dirty
        {
            self.cached_lo_mid_gain = lo_mid_gain_val;
            self.cached_lo_mid_slope = lo_mid_slope_val;
            self.cached_eq_freq_2 = eq_f2;
            let lo_mid_q = q_val(lo_mid_slope_val);
            self.lo_mid_l
                .set_peaking_eq(eq_f2, lo_mid_gain_val, lo_mid_q, sample_rate);
            self.lo_mid_r
                .set_peaking_eq(eq_f2, lo_mid_gain_val, lo_mid_q, sample_rate);
        }

        if mid_gain_val != self.cached_mid_gain
            || mid_slope_val != self.cached_mid_slope
            || eq_f3 != self.cached_eq_freq_3
            || coef_dirty
        {
            self.cached_mid_gain = mid_gain_val;
            self.cached_mid_slope = mid_slope_val;
            self.cached_eq_freq_3 = eq_f3;
            let mid_q = q_val(mid_slope_val);
            self.mid_l
                .set_peaking_eq(eq_f3, mid_gain_val, mid_q, sample_rate);
            self.mid_r
                .set_peaking_eq(eq_f3, mid_gain_val, mid_q, sample_rate);
        }

        if high_gain_val != self.cached_high_gain
            || high_slope_val != self.cached_high_slope
            || eq_f4 != self.cached_eq_freq_4
            || coef_dirty
        {
            self.cached_high_gain = high_gain_val;
            self.cached_high_slope = high_slope_val;
            self.cached_eq_freq_4 = eq_f4;
            let high_q = q_val(high_slope_val);
            self.high_l
                .set_peaking_eq(eq_f4, high_gain_val, high_q, sample_rate);
            self.high_r
                .set_peaking_eq(eq_f4, high_gain_val, high_q, sample_rate);
        }

        if excite_gain_val != self.cached_excite_gain
            || excite_slope_val != self.cached_excite_slope
            || eq_f5 != self.cached_eq_freq_5
            || coef_dirty
        {
            self.cached_excite_gain = excite_gain_val;
            self.cached_excite_slope = excite_slope_val;
            self.cached_eq_freq_5 = eq_f5;
            let excite_slope = slope_val(excite_slope_val);
            self.excite_l
                .set_high_shelf(eq_f5, excite_gain_val, excite_slope, sample_rate);
            self.excite_r
                .set_high_shelf(eq_f5, excite_gain_val, excite_slope, sample_rate);
        }

        if tilt_db != self.cached_tilt_gain || coef_dirty {
            self.cached_tilt_gain = tilt_db;
            self.tilt_l.set(1000.0, tilt_db, sample_rate);
            self.tilt_r.set(1000.0, tilt_db, sample_rate);
        }

        if excite_freq != self.cached_excite_freq || coef_dirty {
            self.cached_excite_freq = excite_freq;
            self.excite_hp_l
                .set_butterworth_hp(excite_freq, sample_rate);
            self.excite_hp_r
                .set_butterworth_hp(excite_freq, sample_rate);
        }

        // Reset peak
        if self.params.shared.reset_peak.swap(false, Ordering::Release) {
            self.peak_hold_value = -90.0;
            self.peak_hold_l_value = -90.0;
            self.peak_hold_r_value = -90.0;
        }

        // Smoothed parameter values (per-block reads via value(), truce pattern)
        let warmth_drive_db = self.params.warmth_drive.value();
        let warmth_mix_pct = self.params.warmth_mix.value();
        let excite_amt = self.params.excite_amount.value();
        let excite_blend = self.params.excite_blend.value();
        let comp_t = self.params.comp_threshold.value();
        let comp_m = self.params.comp_mix.value();
        let comp_att = self.params.comp_attack.value();
        let comp_rel = self.params.comp_release.value();
        let ratio = self.params.comp_character.value();
        let knee = (1.0 - (ratio - 1.5) / 2.5) * 6.0;
        let comp_makeup_gain = db_to_gain(self.params.comp_makeup.value());

        let inflate_effect = self.params.inflate_effect.value() / 100.0;
        let inflate_curve = self.params.inflate_curve.value();
        let inflate_band_split = self.params.inflate_band_split.value();
        let inflate_clip = self.params.inflate_clip.value();

        let width = self.params.stereo_width.value() / 100.0;
        let pan = self.params.pan.value();
        let out_gain = db_to_gain(self.params.output_gain.value());

        let mut snap_phase = self.params.shared.snap_phase.load(Ordering::Acquire);
        let mono = match snap_phase {
            2 => true,
            3 => false,
            _ => self.params.mono_active.value(),
        };
        let delta = match snap_phase {
            3 => true,
            _ => self.params.delta_active.value(),
        };

        let mut max_out_peak = 0.0f32;
        let mut max_out_peak_l = 0.0f32;
        let mut max_out_peak_r = 0.0f32;
        let mut count_samples: usize = 0;

        let mut block_band_power = [0.0f32; 5];

        if buffer.num_samples() == 0 {
            return ProcessStatus::Normal;
        }
        let num_samples = buffer.num_samples();

        let mut gr_db = 0.0f32;
        let mut max_gr_db = 0.0f32;
        let is_measuring = self
            .params
            .shared
            .auto_loud_measuring
            .load(Ordering::Acquire);

        // Feed input LUFS
        if is_measuring {
            self.auto_loud_in.feed(buffer.input(0), buffer.input(1));
            self.pre_sat_buf_l.clear();
            self.pre_sat_buf_r.clear();
        }

        // Raw pointer setup for output
        let (out0_ptr, out1_ptr): (*mut f32, *mut f32);
        {
            let (_, out0) = buffer.io(0);
            out0_ptr = out0.as_mut_ptr();
        }
        {
            let out1_slice = buffer.output(1);
            out1_ptr = out1_slice.as_mut_ptr();
        }
        #[allow(unsafe_code)]
        let (out0, out1): (&mut [f32], &mut [f32]) = unsafe {
            (
                std::slice::from_raw_parts_mut(out0_ptr, num_samples),
                std::slice::from_raw_parts_mut(out1_ptr, num_samples),
            )
        };

        for i in 0..num_samples {
            count_samples += 1;
            let in_l = buffer.input(0)[i];
            let in_r = buffer.input(1)[i];

            // HPF & LPF
            let mut x_l = self.lpf2_l.process(
                self.lpf_l
                    .process(self.hpf2_l.process(self.hpf_l.process(in_l))),
            );
            let mut x_r = self.lpf2_r.process(
                self.lpf_r
                    .process(self.hpf2_r.process(self.hpf_r.process(in_r))),
            );

            // Series EQ
            x_l = self.excite_l.process(
                self.high_l.process(
                    self.mid_l
                        .process(self.lo_mid_l.process(self.bass_l.process(x_l))),
                ),
            );
            x_r = self.excite_r.process(
                self.high_r.process(
                    self.mid_r
                        .process(self.lo_mid_r.process(self.bass_r.process(x_r))),
                ),
            );

            // Tilt
            x_l = self.tilt_l.process(x_l);
            x_r = self.tilt_r.process(x_r);

            // Exciter
            if excite_amt > 0.0 || excite_blend > 0.0 {
                let high_l = self.excite_hp_l.process(x_l);
                let high_r = self.excite_hp_r.process(x_r);
                let drive = 1.0 + (excite_amt / 30.0) * 59.0;
                let sat_high_l = soft_clip(high_l * drive);
                let sat_high_r = soft_clip(high_r * drive);
                let blend = excite_blend / 100.0;
                x_l += (sat_high_l - high_l) * blend;
                x_r += (sat_high_r - high_r) * blend;
            }

            // Compressor
            let (mut comp_l, mut comp_r) = self.compressor.process(
                x_l, x_r, comp_t, comp_m, comp_att, comp_rel, ratio, knee, &mut gr_db,
            );
            max_gr_db = max_gr_db.max(gr_db);
            comp_l *= comp_makeup_gain;
            comp_r *= comp_makeup_gain;

            // Pre-sat LUFS
            if is_measuring {
                self.pre_sat_buf_l.push(comp_l);
                self.pre_sat_buf_r.push(comp_r);
            }

            // Warmth
            if warmth_drive_db > 0.0 || warmth_mix_pct > 0.0 {
                let drive = db_to_gain(warmth_drive_db);
                let wet_l = tube_warm(comp_l * drive) / drive;
                let wet_r = tube_warm(comp_r * drive) / drive;
                let mix = warmth_mix_pct / 100.0;
                comp_l = comp_l * (1.0 - mix) + wet_l * mix;
                comp_r = comp_r * (1.0 - mix) + wet_r * mix;
            }

            // Inflate (Oxford-Inflator-inspired loudness/density waveshaper)
            if inflate_effect > 0.0 {
                let shape_one = |v: f32| -> f32 {
                    let v = if inflate_clip {
                        v.clamp(-1.0, 1.0)
                    } else {
                        v.clamp(-2.0, 2.0)
                    };
                    inflate_shape(v, inflate_curve)
                };
                let (wet_l, wet_r) = if inflate_band_split {
                    let (lo_l, hi_l) = self.xo_inflate_lo_l.process(comp_l);
                    let (mid_l, top_l) = self.xo_inflate_hi_l.process(hi_l);
                    let (lo_r, hi_r) = self.xo_inflate_lo_r.process(comp_r);
                    let (mid_r, top_r) = self.xo_inflate_hi_r.process(hi_r);
                    (
                        shape_one(lo_l) + shape_one(mid_l) + shape_one(top_l),
                        shape_one(lo_r) + shape_one(mid_r) + shape_one(top_r),
                    )
                } else {
                    (shape_one(comp_l), shape_one(comp_r))
                };
                comp_l = comp_l * (1.0 - inflate_effect) + wet_l * inflate_effect;
                comp_r = comp_r * (1.0 - inflate_effect) + wet_r * inflate_effect;
            }

            // Pan
            let pan_val = pan.clamp(-1.0, 1.0);
            let pan_angle = (pan_val + 1.0) * FRAC_PI_4;
            let raw_l = pan_angle.cos();
            let raw_r = pan_angle.sin();
            let max_raw = raw_l.max(raw_r);
            let pan_norm = if max_raw > 0.001 { 1.0 / max_raw } else { 1.0 };
            let pan_l = raw_l * pan_norm;
            let pan_r = raw_r * pan_norm;
            let mut out_l = comp_l * pan_l;
            let mut out_r = comp_r * pan_r;

            // Stereo Width
            let w = width.clamp(0.0, 2.0);
            let a = 0.5 * (1.0 + w);
            let b = 0.5 * (1.0 - w);
            let width_l = out_l * a + out_r * b;
            let width_r = out_r * a + out_l * b;
            let width_norm = 1.0 / (1.0 + (w - 1.0).max(0.0) * 0.20);
            out_l = width_l * width_norm;
            out_r = width_r * width_norm;

            // Mono
            if mono {
                let m = (out_l + out_r) * 0.5;
                out_l = m;
                out_r = m;
            }

            let mut processed_l = out_l;
            let mut processed_r = out_r;

            // Delta
            if delta {
                processed_l = out_l - in_l;
                processed_r = out_r - in_r;
            }

            processed_l *= out_gain;
            processed_r *= out_gain;

            // Safety clamp
            processed_l = processed_l.clamp(-1.0, 1.0);
            processed_r = processed_r.clamp(-1.0, 1.0);

            // Crossover analysis for visualizer
            let (low_group_l, high_group_l) = self.xo_bass_mid_l.process(processed_l);
            let (band1_l, band2_l) = self.xo_low_bass_l.process(low_group_l);
            let (mid_group_l, super_high_group_l) = self.xo_mid_high_l.process(high_group_l);
            let (band3_l, band4_l) = (mid_group_l, super_high_group_l);
            let (band4_l_split, band5_l) = self.xo_highmid_high_l.process(band4_l);
            let band4_l = band4_l_split;

            let (low_group_r, high_group_r) = self.xo_bass_mid_r.process(processed_r);
            let (band1_r, band2_r) = self.xo_low_bass_r.process(low_group_r);
            let (mid_group_r, super_high_group_r) = self.xo_mid_high_r.process(high_group_r);
            let (band3_r, band4_r) = (mid_group_r, super_high_group_r);
            let (band4_r_split, band5_r) = self.xo_highmid_high_r.process(band4_r);
            let band4_r = band4_r_split;

            let bands_l = [band1_l, band2_l, band3_l, band4_l, band5_l];
            let bands_r = [band1_r, band2_r, band3_r, band4_r, band5_r];
            for b in 0..5 {
                let band_power = (bands_l[b] * bands_l[b] + bands_r[b] * bands_r[b]) * 0.5;
                block_band_power[b] += band_power;
            }

            let (meter_l, meter_r) = if bypass {
                out0[i] = in_l;
                out1[i] = in_r;
                (in_l, in_r)
            } else {
                max_out_peak = max_out_peak.max(processed_l.abs()).max(processed_r.abs());
                max_out_peak_l = max_out_peak_l.max(processed_l.abs());
                max_out_peak_r = max_out_peak_r.max(processed_r.abs());
                out0[i] = processed_l;
                out1[i] = processed_r;
                (processed_l, processed_r)
            };

            let corr_lr = meter_l * meter_r;
            let corr_l2 = meter_l * meter_l;
            let corr_r2 = meter_r * meter_r;
            self.corr_avg_lr = (1.0 - self.correlation_decay_coef) * self.corr_avg_lr
                + self.correlation_decay_coef * corr_lr;
            self.corr_avg_l2 = (1.0 - self.correlation_decay_coef) * self.corr_avg_l2
                + self.correlation_decay_coef * corr_l2;
            self.corr_avg_r2 = (1.0 - self.correlation_decay_coef) * self.corr_avg_r2
                + self.correlation_decay_coef * corr_r2;
        }

        // Gain reduction
        self.params
            .shared
            .gain_reduction
            .store(max_gr_db, Ordering::Release);

        // Smoothed band levels
        let sample_weight = 1.0 / count_samples as f32;
        let buf_coef = 1.0 - (-(num_samples as f32) / (0.1 * sample_rate)).exp();
        for (b, &band_power) in block_band_power.iter().enumerate() {
            let average_band_power = band_power * sample_weight;
            self.smoothed_band_power[b] =
                (1.0 - buf_coef) * self.smoothed_band_power[b] + buf_coef * average_band_power;
            let band_db = gain_to_db(self.smoothed_band_power[b].sqrt());
            self.params.shared.band_levels[b].store(band_db, Ordering::Release);
        }

        // Correlation
        let denom = (self.corr_avg_l2 * self.corr_avg_r2).sqrt();
        let corr = if denom > 1e-6 {
            self.corr_avg_lr / denom
        } else {
            1.0
        };
        self.params
            .shared
            .phase_correlation
            .store(corr, Ordering::Release);

        // Peak meters
        let peak_db = gain_to_db(max_out_peak);
        let peak_l_db = gain_to_db(max_out_peak_l);
        let peak_r_db = gain_to_db(max_out_peak_r);
        self.params
            .shared
            .output_peak
            .store(peak_db, Ordering::Release);
        self.params
            .shared
            .output_peak_l
            .store(peak_l_db, Ordering::Release);
        self.params
            .shared
            .output_peak_r
            .store(peak_r_db, Ordering::Release);
        self.peak_hold_value = self.peak_hold_value.max(peak_db);
        self.peak_hold_l_value = self.peak_hold_l_value.max(peak_l_db);
        self.peak_hold_r_value = self.peak_hold_r_value.max(peak_r_db);
        self.params
            .shared
            .peak_hold
            .store(self.peak_hold_value, Ordering::Release);
        self.params
            .shared
            .peak_hold_l
            .store(self.peak_hold_l_value, Ordering::Release);
        self.params
            .shared
            .peak_hold_r
            .store(self.peak_hold_r_value, Ordering::Release);

        // Balance
        let rms_l = self.corr_avg_l2.sqrt();
        let rms_r = self.corr_avg_r2.sqrt();
        let sum_lr = rms_l + rms_r;
        let balance = if sum_lr > 1e-6 {
            (rms_l - rms_r) / sum_lr
        } else {
            0.0
        };
        self.params.shared.balance.store(balance, Ordering::Release);

        // FFT Spectrum
        {
            let n = num_samples;
            let fft_size = self.fft_input.len();
            for i in 0..n {
                self.fft_input[self.fft_write_pos] = (out0[i] + out1[i]) * 0.5;
                self.fft_write_pos += 1;
                if self.fft_write_pos >= fft_size {
                    let half = fft_size / 2;
                    for j in 0..half {
                        self.fft_input[j] = self.fft_input[j + half];
                    }
                    self.fft_write_pos = half;

                    for i in 0..fft_size {
                        self.fft_windowed[i] = self.fft_input[i] * self.fft_hann[i];
                    }
                    let fft = self.fft_planner.plan_fft_forward(fft_size);
                    fft.process(&mut self.fft_windowed, &mut self.fft_output_cache)
                        .ok();
                }
            }

            // Compute and write spectrum after each buffer
            if let Ok(mut spectrum_frame) = self.params.shared.spectrum_bins.try_lock() {
                shared_analysis::compute_spectrum_bins(
                    &self.fft_output_cache,
                    &mut spectrum_frame,
                    fft_size,
                    sample_rate,
                );
            }

            // Update spectrum_avg (EMA) from spectrum_bins
            if let Ok(mut avg) = self.params.shared.spectrum_avg.try_lock()
                && let Ok(bins) = self.params.shared.spectrum_bins.try_lock()
            {
                let n_bins = SPECTRUM_BINS;
                // Energy-gating: only update EMA if signal above -80 dB
                let frame_energy = bins.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                let gate = energy_db > -80.0;

                if !gate {
                    for sample in self.fft_input.iter_mut() {
                        *sample = 0.0;
                    }
                }

                for k in 0..n_bins {
                    let freq = k as f32 * sample_rate / fft_size as f32;
                    let log_norm = ((freq.max(20.0).ln() - 20.0_f32.ln())
                        / (20000.0_f32.ln() - 20.0_f32.ln()))
                    .clamp(0.0, 1.0);
                    let alpha = 0.02 + (0.10 - 0.02) * log_norm;
                    let input = if gate { bins[k] } else { 0.0 };
                    avg[k] = avg[k] * (1.0 - alpha) + input * alpha;
                }
            }
        }

        // SNAP FFT
        if snap_phase > 0 {
            for i in 0..num_samples {
                let sample = match snap_phase {
                    1 | 2 => (out0[i] + out1[i]) * 0.5,
                    3 => {
                        let out_mono = (out0[i] + out1[i]) * 0.5;
                        let in_mono = (buffer.input(0)[i] + buffer.input(1)[i]) * 0.5;
                        out_mono - in_mono
                    }
                    _ => 0.0,
                };
                if self.snap_fft.push_sample(sample) {
                    let frame = self.snap_fft.compute_fft(sample_rate);
                    let threshold = if snap_phase == 2 || snap_phase == 3 {
                        30
                    } else {
                        60
                    };
                    if self.snap_fft.accumulate_snap(&frame, snap_phase, threshold) {
                        let mode = match snap_phase {
                            1 => SnapMode::Stereo,
                            2 => SnapMode::Mono,
                            _ => SnapMode::Delta,
                        };
                        let snapshot = self.snap_fft.read_snapshot(mode);
                        if let Ok(mut buf) = match mode {
                            SnapMode::Stereo => self.params.shared.snap_stereo_snap.try_lock(),
                            SnapMode::Mono => self.params.shared.snap_mono_snap.try_lock(),
                            SnapMode::Delta => self.params.shared.snap_delta_snap.try_lock(),
                        } {
                            buf.copy_from_slice(&snapshot);
                        }
                        let next_phase = if snap_phase < 3 { snap_phase + 1 } else { 0 };
                        if next_phase == 0 {
                            self.params
                                .shared
                                .snap_active
                                .store(false, Ordering::Release);
                        } else {
                            self.snap_fft.reset_snapshots();
                        }
                        self.params
                            .shared
                            .snap_phase
                            .store(next_phase, Ordering::Release);
                        snap_phase = next_phase;
                    }
                }
            }
        }

        // AUTO LOUD
        if self.params.shared.auto_loud_trigger.load(Ordering::Acquire) {
            self.params
                .shared
                .auto_loud_trigger
                .store(false, Ordering::Release);
            self.params
                .shared
                .auto_loud_measuring
                .store(true, Ordering::Release);
            self.auto_loud_in.reset();
            self.auto_loud_pre_sat.reset();
            self.auto_loud_out.reset();
        }
        if is_measuring {
            if !self.pre_sat_buf_l.is_empty() {
                self.auto_loud_pre_sat
                    .feed(&self.pre_sat_buf_l, &self.pre_sat_buf_r);
            }
            self.auto_loud_out.feed(out0, out1);
            let target_samples = (5.0 * sample_rate as f64) as u64;
            if self.auto_loud_out.sample_count() >= target_samples {
                let in_lufs = self.auto_loud_in.loudness_db();
                let _pre_lufs = self.auto_loud_pre_sat.loudness_db();
                let out_lufs = self.auto_loud_out.loudness_db();
                let out_tp = self.auto_loud_out.true_peak_db();
                let lufs_offset = in_lufs - out_lufs;
                let peak_limit = DBTP_CEILING - out_tp;
                let offset_clamped = lufs_offset.clamp(-24.0, peak_limit);
                self.params
                    .shared
                    .auto_loud_gain_offset
                    .store(offset_clamped, Ordering::Release);
                self.params
                    .shared
                    .auto_loud_measuring
                    .store(false, Ordering::Release);
            }
        }

        // Goniometer scope buffer
        {
            let start_pos = self.params.shared.scope_write_pos.load(Ordering::Acquire);
            if let Ok(mut scope) = self.params.shared.scope_samples.try_lock() {
                let buf_len = SCOPE_BUFFER_LEN;
                let n = num_samples.min(buf_len);
                let block_peak = (0..n)
                    .map(|i| out0[i].abs().max(out1[i].abs()))
                    .fold(0.0f32, f32::max)
                    .max(1e-9);
                let att = 1.0 - (-(n as f32) / (0.005 * sample_rate)).exp();
                let rel = 1.0 - (-(n as f32) / (0.300 * sample_rate)).exp();
                if block_peak > self.scope_vis_envelope {
                    self.scope_vis_envelope += att * (block_peak - self.scope_vis_envelope);
                } else {
                    self.scope_vis_envelope += rel * (block_peak - self.scope_vis_envelope);
                }
                let vis_gain = if self.scope_vis_envelope > 1e-5 {
                    (0.9 / self.scope_vis_envelope).min(20.0)
                } else {
                    0.0
                };
                for i in 0..n {
                    let pos = (start_pos + i) % buf_len;
                    scope[pos] = [out0[i] * vis_gain, out1[i] * vis_gain];
                }
                self.params
                    .shared
                    .scope_write_pos
                    .store((start_pos + n) % buf_len, Ordering::Release);
            }
        }

        ProcessStatus::Normal
    }

    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }
    fn load_state(&mut self, data: &[u8]) -> Result<(), StateLoadError> {
        if let Some(params) = state_migration::try_parse_niceplug_state(data) {
            for (name, value) in params {
                match name.as_str() {
                    "hpf_freq" => self.params.hpf_freq.set_value(value),
                    "lpf_freq" => self.params.lpf_freq.set_value(value),
                    "cut_slope" => self.params.cut_slope.set_value(value as i64),
                    "bass_gain" => self.params.bass_gain.set_value(value),
                    "bass_slope" => self.params.bass_slope.set_value(value as i64),
                    "lo_mid_gain" => self.params.lo_mid_gain.set_value(value),
                    "lo_mid_slope" => self.params.lo_mid_slope.set_value(value as i64),
                    "mid_gain" => self.params.mid_gain.set_value(value),
                    "mid_slope" => self.params.mid_slope.set_value(value as i64),
                    "high_gain" => self.params.high_gain.set_value(value),
                    "high_slope" => self.params.high_slope.set_value(value as i64),
                    "excite_gain" => self.params.excite_gain.set_value(value),
                    "excite_slope" => self.params.excite_slope.set_value(value as i64),
                    "eq_freq_1" => self.params.eq_freq_1.set_value(value),
                    "eq_freq_2" => self.params.eq_freq_2.set_value(value),
                    "eq_freq_3" => self.params.eq_freq_3.set_value(value),
                    "eq_freq_4" => self.params.eq_freq_4.set_value(value),
                    "eq_freq_5" => self.params.eq_freq_5.set_value(value),
                    "tilt_gain" => self.params.tilt_gain.set_value(value),
                    "warmth_drive" => self.params.warmth_drive.set_value(value),
                    "warmth_mix" => self.params.warmth_mix.set_value(value),
                    "excite_amount" => self.params.excite_amount.set_value(value),
                    "excite_blend" => self.params.excite_blend.set_value(value),
                    "excite_freq" => self.params.excite_freq.set_value(value),
                    "comp_threshold" => self.params.comp_threshold.set_value(value),
                    "comp_mix" => self.params.comp_mix.set_value(value),
                    "comp_attack" => self.params.comp_attack.set_value(value),
                    "comp_release" => self.params.comp_release.set_value(value),
                    // Field was renamed comp_ratio -> comp_character after
                    // the last nice-plug build; old sessions still say
                    // "comp_ratio" on the wire.
                    "comp_ratio" | "comp_character" => self.params.comp_character.set_value(value),
                    "comp_makeup" => self.params.comp_makeup.set_value(value),
                    "inflate_effect" => self.params.inflate_effect.set_value(value),
                    "inflate_curve" => self.params.inflate_curve.set_value(value),
                    "inflate_band_split" => self.params.inflate_band_split.set_value(value != 0.0),
                    "inflate_clip" => self.params.inflate_clip.set_value(value != 0.0),
                    "stereo_width" => self.params.stereo_width.set_value(value),
                    "pan" => self.params.pan.set_value(value),
                    "output_gain" => self.params.output_gain.set_value(value),
                    "mono_active" => self.params.mono_active.set_value(value != 0.0),
                    "delta_active" => self.params.delta_active.set_value(value != 0.0),
                    "bypass_active" => self.params.bypass_active.set_value(value != 0.0),
                    _ => {}
                }
            }
        }
        Ok(())
    }
    fn state_changed(&mut self) {}

    fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
        let shared = params.shared.clone();
        ViziaEditor::<MeridianParams>::new(params.clone(), (WINDOW_W, WINDOW_H), move |cx, lens| {
            editor::build(cx, lens, shared.clone(), params.clone())
        })
        .into_editor()
    }
}

truce::plugin! { logic: Meridian, params: MeridianParams }
