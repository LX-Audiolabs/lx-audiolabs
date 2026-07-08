// Equilibrium — Pre-master spectral balancer (truce port).
//
// 5-band LR2 crossover (80/300/2000/6000 Hz) with per-band Gain,
// Stereo Width (M/S), Pan (constant-power), and Solo.
//
// Signal chain:
//   DC HP@8Hz → [LP@35kHz if sr≥88.2k] → 5-band Crossover
//   → per-band: Gain → M/S Width → Pan → Solo
//   → sum → Mono Floor (Side HPF) → Mono/Delta → Gain → Auto Gain → clamp

use shared_dsp::state_migration;
use std::f32::consts::FRAC_PI_4;
use std::sync::Arc;
use truce::prelude::*;
use truce_core::editor::Editor;
use truce_core::state::StateLoadError;
use truce_vizia::ViziaEditor;

use shared_analysis::{SCOPE_BUFFER_LEN, SharedState, SnapFFT, SnapMode};
use shared_dsp::{AutoLoudMeter, Biquad, DBTP_CEILING, FtzDazGuard, LR2Crossover};

mod editor;
mod vizia_canvas;

const BAND_COUNT: usize = 5;
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

const MINUS_INF_DB: f32 = -90.0;

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct EquilibriumParams {
    // 5 Band Gains
    #[param(
        name = "Sub Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub low_gain: FloatParam,
    #[param(
        name = "Bass Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub bass_gain: FloatParam,
    #[param(
        name = "Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub mid_gain: FloatParam,
    #[param(
        name = "Pres Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub high_mid_gain: FloatParam,
    #[param(
        name = "Air Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub high_gain: FloatParam,

    // 5 Band Widths
    #[param(
        name = "Sub Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub low_width: FloatParam,
    #[param(
        name = "Bass Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub bass_width: FloatParam,
    #[param(
        name = "Mid Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub mid_width: FloatParam,
    #[param(
        name = "Pres Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub high_mid_width: FloatParam,
    #[param(
        name = "Air Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub high_width: FloatParam,

    // 5 Band Pans (-1.0 L to +1.0 R)
    #[param(
        name = "Sub Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub low_pan: FloatParam,
    #[param(
        name = "Bass Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub bass_pan: FloatParam,
    #[param(
        name = "Mid Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub mid_pan: FloatParam,
    #[param(
        name = "Pres Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub high_mid_pan: FloatParam,
    #[param(
        name = "Air Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub high_pan: FloatParam,

    // Mono Floor frequency (0 = off, 1–300 Hz)
    #[param(
        name = "Mono Floor",
        default = 0.0,
        range = "linear(0.0, 300.0)",
        unit = "Hz"
    )]
    pub mono_floor: FloatParam,

    // Output manual gain
    #[param(
        name = "Output Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub output_gain: FloatParam,

    // Solos
    #[param(name = "Solo Sub", default = 0)]
    pub solo_low: BoolParam,
    #[param(name = "Solo Bass", default = 0)]
    pub solo_bass: BoolParam,
    #[param(name = "Solo Mid", default = 0)]
    pub solo_mid: BoolParam,
    #[param(name = "Solo Pres", default = 0)]
    pub solo_high_mid: BoolParam,
    #[param(name = "Solo Air", default = 0)]
    pub solo_high: BoolParam,

    // Modes
    #[param(name = "Mono Sum", default = 0, group = "Monitor")]
    pub mono_active: BoolParam,
    #[param(name = "Delta Diff", default = 0, group = "Monitor")]
    pub delta_active: BoolParam,
    #[param(name = "Listen Profile", default = 0, group = "Monitor")]
    pub listen_active: BoolParam,
    #[param(name = "Auto Loudness", default = 0, group = "Monitor")]
    pub auto_gain_active: BoolParam,
    #[param(name = "Bypass", default = 0, group = "Monitor")]
    pub bypass_active: BoolParam,

    // Pre-Master mode
    #[param(name = "Pre-Master", default = 0, group = "Monitor")]
    pub pre_master_active: BoolParam,
    #[param(name = "Pre-Master Target", default = -3.0, range = "linear(-6.0, -3.0)", unit = "dB")]
    pub pre_master_target_db: FloatParam,

    #[skip]
    pub shared: Arc<SharedState>,
}

impl EquilibriumParams {
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

pub struct Equilibrium {
    params: Arc<EquilibriumParams>,

    // Filters L
    low_cut_l: Biquad,
    high_cut_l: Biquad,
    xo_bass_mid_l: LR2Crossover,
    xo_low_bass_l: LR2Crossover,
    xo_mid_high_l: LR2Crossover,
    xo_highmid_high_l: LR2Crossover,

    // Filters R
    low_cut_r: Biquad,
    high_cut_r: Biquad,
    xo_bass_mid_r: LR2Crossover,
    xo_low_bass_r: LR2Crossover,
    xo_mid_high_r: LR2Crossover,
    xo_highmid_high_r: LR2Crossover,

    // Mono Floor filter (Side HPF)
    mono_floor_filter: Biquad,

    // Temporal smoothing
    rms_decay_coef: f32,
    correlation_decay_coef: f32,

    // Smoothed states
    smoothed_band_power: [f32; BAND_COUNT],
    listen_band_power_sum: [f64; BAND_COUNT],
    listen_sample_count: u64,
    listen_lo_ema: [f64; BAND_COUNT],
    listen_hi_ema: [f64; BAND_COUNT],
    listen_ref_ema: [f64; BAND_COUNT],
    listen_levels_ema: [f32; BAND_COUNT],
    listen_min_ema: [f32; BAND_COUNT],
    listen_max_ema: [f32; BAND_COUNT],

    // Correlation
    corr_avg_lr: f32,
    corr_avg_l2: f32,
    corr_avg_r2: f32,

    // Peak hold
    peak_hold_value: f32,
    peak_hold_l_value: f32,
    peak_hold_r_value: f32,

    // Stereo balance
    smoothed_power_l: f32,
    smoothed_power_r: f32,

    // Auto Gain
    auto_gain_comp: f32,

    // Pre-Master
    pre_master_gain: f32,
    pre_master_active_prev: bool,
    pre_master_measure_peak: f32,
    pre_master_measure_count: u32,

    // Goniometer
    scope_vis_envelope: f32,

    // AUTO LOUD
    auto_loud_in: AutoLoudMeter,
    auto_loud_out: AutoLoudMeter,

    // SNAP FFT
    snap_fft: SnapFFT,

    // Cached parameters (dirty-flag optimization)
    cached_mono_floor_freq: f32,
    cached_sample_rate: f32,
}

impl Equilibrium {
    pub fn new(params: Arc<EquilibriumParams>) -> Self {
        Self {
            params,
            low_cut_l: Biquad::new(),
            high_cut_l: Biquad::new(),
            xo_bass_mid_l: LR2Crossover::new(),
            xo_low_bass_l: LR2Crossover::new(),
            xo_mid_high_l: LR2Crossover::new(),
            xo_highmid_high_l: LR2Crossover::new(),
            low_cut_r: Biquad::new(),
            high_cut_r: Biquad::new(),
            xo_bass_mid_r: LR2Crossover::new(),
            xo_low_bass_r: LR2Crossover::new(),
            xo_mid_high_r: LR2Crossover::new(),
            xo_highmid_high_r: LR2Crossover::new(),
            mono_floor_filter: Biquad::new(),
            rms_decay_coef: 0.001,
            correlation_decay_coef: 0.005,
            smoothed_band_power: [0.0; BAND_COUNT],
            listen_band_power_sum: [0.0; BAND_COUNT],
            listen_sample_count: 0,
            listen_lo_ema: [f64::INFINITY; BAND_COUNT],
            listen_hi_ema: [f64::NEG_INFINITY; BAND_COUNT],
            listen_ref_ema: [0.0; BAND_COUNT],
            listen_levels_ema: [-90.0; BAND_COUNT],
            listen_min_ema: [-90.0; BAND_COUNT],
            listen_max_ema: [-90.0; BAND_COUNT],
            smoothed_power_l: 0.0,
            smoothed_power_r: 0.0,
            corr_avg_lr: 0.0,
            corr_avg_l2: 0.0,
            corr_avg_r2: 0.0,
            peak_hold_value: MINUS_INF_DB,
            peak_hold_l_value: MINUS_INF_DB,
            peak_hold_r_value: MINUS_INF_DB,
            auto_gain_comp: 1.0,
            pre_master_gain: 1.0,
            pre_master_active_prev: false,
            pre_master_measure_peak: 0.0,
            pre_master_measure_count: 0,
            scope_vis_envelope: 1e-4,
            auto_loud_in: AutoLoudMeter::new(44100.0),
            auto_loud_out: AutoLoudMeter::new(44100.0),
            snap_fft: SnapFFT::new(),
            cached_mono_floor_freq: -999.0,
            cached_sample_rate: -999.0,
        }
    }
}

// ─── PluginLogic ─────────────────────────────────────────────────────────────

impl PluginLogic for Equilibrium {
    type Params = EquilibriumParams;

    fn reset(&mut self, sr: f64, _max: usize) {
        let sr = sr as f32;
        self.cached_sample_rate = sr;

        // Recreate Auto-Loud meters at host sample rate
        self.auto_loud_in = AutoLoudMeter::new(sr);
        self.auto_loud_out = AutoLoudMeter::new(sr);

        // DC/infrasonic protection HP @ 8 Hz
        self.low_cut_l.set_butterworth_hp(2.0, sr);
        self.low_cut_r.set_butterworth_hp(2.0, sr);

        // LP @ 35 kHz only at ≥ 88.2 kHz
        if sr >= 88_200.0 {
            self.high_cut_l.set_butterworth_lp(35000.0, sr);
            self.high_cut_r.set_butterworth_lp(35000.0, sr);
        }

        // Crossover frequencies
        for (xo_l, xo_r, fc) in [
            (&mut self.xo_bass_mid_l, &mut self.xo_bass_mid_r, 300.0),
            (&mut self.xo_low_bass_l, &mut self.xo_low_bass_r, 80.0),
            (&mut self.xo_mid_high_l, &mut self.xo_mid_high_r, 2000.0),
            (
                &mut self.xo_highmid_high_l,
                &mut self.xo_highmid_high_r,
                6000.0,
            ),
        ] {
            xo_l.set_cutoff(fc, sr);
            xo_r.set_cutoff(fc, sr);
        }

        // Mono floor if initially active
        let mm_init = self.params.mono_floor.raw_target() as f32;
        if mm_init > 1.0 {
            self.mono_floor_filter.set_butterworth_hp(mm_init, sr);
            self.cached_mono_floor_freq = mm_init;
        }

        self.rms_decay_coef = 1.0 - (-1.0 / (0.5 * sr)).exp();
        self.correlation_decay_coef = 1.0 - (-1.0 / (0.1 * sr)).exp();

        // Reset all filter states
        self.low_cut_l.reset();
        self.low_cut_r.reset();
        self.high_cut_l.reset();
        self.high_cut_r.reset();
        self.xo_bass_mid_l.reset();
        self.xo_bass_mid_r.reset();
        self.xo_low_bass_l.reset();
        self.xo_low_bass_r.reset();
        self.xo_mid_high_l.reset();
        self.xo_mid_high_r.reset();
        self.xo_highmid_high_l.reset();
        self.xo_highmid_high_r.reset();
        self.mono_floor_filter.reset();

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

        let sample_rate = self
            .params
            .shared
            .sample_rate
            .load(std::sync::atomic::Ordering::Acquire);

        // Dirty-flag for mono_floor
        let mono_maker_freq = self.params.mono_floor.raw_target() as f32;
        let coef_dirty = sample_rate != self.cached_sample_rate;
        if coef_dirty {
            self.cached_sample_rate = sample_rate;
        }
        if (mono_maker_freq != self.cached_mono_floor_freq || coef_dirty) && mono_maker_freq > 1.0 {
            self.cached_mono_floor_freq = mono_maker_freq;
            self.mono_floor_filter
                .set_butterworth_hp(mono_maker_freq, sample_rate);
        }

        // Reset peak
        if self
            .params
            .shared
            .reset_peak
            .swap(false, std::sync::atomic::Ordering::Release)
        {
            self.peak_hold_value = MINUS_INF_DB;
            self.peak_hold_l_value = MINUS_INF_DB;
            self.peak_hold_r_value = MINUS_INF_DB;
        }

        // Reset analysis
        if self
            .params
            .shared
            .reset_analysis
            .swap(false, std::sync::atomic::Ordering::Release)
        {
            for b in 0..BAND_COUNT {
                self.listen_band_power_sum[b] = 0.0;
                self.listen_lo_ema[b] = f64::INFINITY;
                self.listen_hi_ema[b] = f64::NEG_INFINITY;
                self.listen_ref_ema[b] = 0.0;
                self.listen_levels_ema[b] = -90.0;
                self.listen_min_ema[b] = -90.0;
                self.listen_max_ema[b] = -90.0;
            }
            self.listen_sample_count = 0;
            self.low_cut_l.reset();
            self.low_cut_r.reset();
            self.high_cut_l.reset();
            self.high_cut_r.reset();
            self.xo_bass_mid_l.reset();
            self.xo_bass_mid_r.reset();
            self.xo_low_bass_l.reset();
            self.xo_low_bass_r.reset();
            self.xo_mid_high_l.reset();
            self.xo_mid_high_r.reset();
            self.xo_highmid_high_l.reset();
            self.xo_highmid_high_r.reset();
            self.mono_floor_filter.reset();
        }

        let any_solo = self.params.solo_low.value()
            || self.params.solo_bass.value()
            || self.params.solo_mid.value()
            || self.params.solo_high_mid.value()
            || self.params.solo_high.value();

        let s_low = if any_solo {
            self.params.solo_low.value()
        } else {
            true
        };
        let s_bass = if any_solo {
            self.params.solo_bass.value()
        } else {
            true
        };
        let s_mid = if any_solo {
            self.params.solo_mid.value()
        } else {
            true
        };
        let s_high_mid = if any_solo {
            self.params.solo_high_mid.value()
        } else {
            true
        };
        let s_high = if any_solo {
            self.params.solo_high.value()
        } else {
            true
        };

        let bypass = self.params.bypass_active.value();

        let mut snap_phase = self
            .params
            .shared
            .snap_phase
            .load(std::sync::atomic::Ordering::Acquire);
        let mono = match snap_phase {
            2 => true,
            _ => self.params.mono_active.value(),
        };
        let delta = match snap_phase {
            3 => true,
            _ => self.params.delta_active.value(),
        };
        let listen = self.params.listen_active.value();
        let auto_gain = self.params.auto_gain_active.value();

        let mut max_out_peak = 0.0f32;
        let mut max_out_peak_l = 0.0f32;
        let mut max_out_peak_r = 0.0f32;
        let mut sum_power_in = 0.0f32;
        let mut sum_power_out = 0.0f32;
        let mut sum_power_l = 0.0f32;
        let mut sum_power_r = 0.0f32;
        let mut count_samples: usize = 0;

        let mut block_band_power = [0.0f32; 5];
        let mut block_input_band_power = [0.0f32; 5];

        // Raw pointers to output buffers — avoids borrow conflicts when both
        // output channels are needed simultaneously (feed, scope, pre-master).
        let num_samples = buffer.num_samples();
        let (out0_ptr, out1_ptr): (*mut f32, *mut f32);
        {
            let (_, out0) = buffer.io(0);
            out0_ptr = out0.as_mut_ptr();
        }
        {
            let out1_slice = buffer.output(1);
            out1_ptr = out1_slice.as_mut_ptr();
        }
        // SAFETY: both pointers are valid, non-aliasing output channels
        #[allow(unsafe_code)]
        let (out0, out1): (&mut [f32], &mut [f32]) = unsafe {
            (
                std::slice::from_raw_parts_mut(out0_ptr, num_samples),
                std::slice::from_raw_parts_mut(out1_ptr, num_samples),
            )
        };

        // Feed input to LUFS meter BEFORE we modify the buffer
        let is_measuring = self
            .params
            .shared
            .auto_loud_measuring
            .load(std::sync::atomic::Ordering::Acquire);
        if is_measuring {
            self.auto_loud_in.feed(buffer.input(0), buffer.input(1));
        }

        for i in 0..num_samples {
            count_samples += 1;
            let in_l = buffer.input(0)[i];
            let in_r = buffer.input(1)[i];

            sum_power_in += in_l * in_l + in_r * in_r;

            // HP @8 Hz always, LP @35 kHz only at ≥ 88.2 kHz
            let dc_l = self.low_cut_l.process(in_l);
            let dc_r = self.low_cut_r.process(in_r);
            let cut_l = if sample_rate >= 88_200.0 {
                self.high_cut_l.process(dc_l)
            } else {
                dc_l
            };
            let cut_r = if sample_rate >= 88_200.0 {
                self.high_cut_r.process(dc_r)
            } else {
                dc_r
            };

            // Crossover tree
            let (low_group_l, high_group_l) = self.xo_bass_mid_l.process_transparent(cut_l);
            let (band1_l, band2_l) = self.xo_low_bass_l.process_transparent(low_group_l);
            let (mid_group_l, super_high_group_l) =
                self.xo_mid_high_l.process_transparent(high_group_l);
            let (band3_l, band4_l_pre) = (mid_group_l, super_high_group_l);
            let (band4_l, band5_l) = self.xo_highmid_high_l.process_transparent(band4_l_pre);

            let (low_group_r, high_group_r) = self.xo_bass_mid_r.process_transparent(cut_r);
            let (band1_r, band2_r) = self.xo_low_bass_r.process_transparent(low_group_r);
            let (mid_group_r, super_high_group_r) =
                self.xo_mid_high_r.process_transparent(high_group_r);
            let (band3_r, band4_r_pre) = (mid_group_r, super_high_group_r);
            let (band4_r, band5_r) = self.xo_highmid_high_r.process_transparent(band4_r_pre);

            let mut bands_l = [band1_l, band2_l, band3_l, band4_l, band5_l];
            let mut bands_r = [band1_r, band2_r, band3_r, band4_r, band5_r];

            let band_gains = [
                db_to_gain(self.params.low_gain.value()),
                db_to_gain(self.params.bass_gain.value()),
                db_to_gain(self.params.mid_gain.value()),
                db_to_gain(self.params.high_mid_gain.value()),
                db_to_gain(self.params.high_gain.value()),
            ];
            let band_widths = [
                self.params.low_width.value() / 100.0,
                self.params.bass_width.value() / 100.0,
                self.params.mid_width.value() / 100.0,
                self.params.high_mid_width.value() / 100.0,
                self.params.high_width.value() / 100.0,
            ];
            let band_pans = [
                self.params.low_pan.value(),
                self.params.bass_pan.value(),
                self.params.mid_pan.value(),
                self.params.high_mid_pan.value(),
                self.params.high_pan.value(),
            ];
            let band_solos = [s_low, s_bass, s_mid, s_high_mid, s_high];

            for b in 0..BAND_COUNT {
                let bl = bands_l[b];
                let br = bands_r[b];

                // Pre-EQ input band power for LISTEN analysis
                let input_power = (bl * bl + br * br) * 0.5;
                block_input_band_power[b] += input_power;

                let mut bl_g = bl * band_gains[b];
                let mut br_g = br * band_gains[b];

                // M/S Width
                let mid = (bl_g + br_g) * 0.5;
                let side = (bl_g - br_g) * 0.5;
                let width_scale = if band_widths[b] > 1.0 {
                    match b {
                        0 => 1.0 + (band_widths[b] - 1.0) * 0.25,
                        1 => 1.0 + (band_widths[b] - 1.0) * 0.65,
                        _ => band_widths[b],
                    }
                } else {
                    band_widths[b]
                };
                let side_w = side * width_scale;
                let width_norm = 1.0 / (1.0 + (width_scale - 1.0).max(0.0) * 0.20);

                // Constant-power pan with center normalization
                let pan_val = band_pans[b].clamp(-1.0, 1.0);
                let pan_angle = (pan_val + 1.0) * FRAC_PI_4;
                let raw_l = pan_angle.cos();
                let raw_r = pan_angle.sin();
                let max_raw = raw_l.max(raw_r);
                let pan_norm = if max_raw > 0.001 { 1.0 / max_raw } else { 1.0 };
                let pan_l = raw_l * pan_norm;
                let pan_r = raw_r * pan_norm;

                bl_g = (mid + side_w) * pan_l * width_norm;
                br_g = (mid - side_w) * pan_r * width_norm;

                // Band power post-EQ (pre-solo)
                let band_power = (bl_g * bl_g + br_g * br_g) * 0.5;
                block_band_power[b] += band_power;

                if !band_solos[b] {
                    bl_g = 0.0;
                    br_g = 0.0;
                }

                bands_l[b] = bl_g;
                bands_r[b] = br_g;
            }

            let mut out_l = bands_l[0] + bands_l[1] + bands_l[2] + bands_l[3] + bands_l[4];
            let mut out_r = bands_r[0] + bands_r[1] + bands_r[2] + bands_r[3] + bands_r[4];

            // Mono Floor (Side HPF)
            if mono_maker_freq > 1.0 {
                let out_mid = (out_l + out_r) * 0.5;
                let out_side = (out_l - out_r) * 0.5;
                let out_side_filtered = self.mono_floor_filter.process(out_side);
                out_l = out_mid + out_side_filtered;
                out_r = out_mid - out_side_filtered;
            }

            if mono {
                let m = (out_l + out_r) * 0.5;
                out_l = m;
                out_r = m;
            }

            let mut processed_l = out_l;
            let mut processed_r = out_r;

            if delta {
                processed_l = out_l - cut_l;
                processed_r = out_r - cut_r;
            }

            let out_gain = db_to_gain(self.params.output_gain.value());
            processed_l *= out_gain;
            processed_r *= out_gain;

            if auto_gain {
                processed_l *= self.auto_gain_comp;
                processed_r *= self.auto_gain_comp;
            }

            // Safety clamp
            processed_l = processed_l.clamp(-1.0, 1.0);
            processed_r = processed_r.clamp(-1.0, 1.0);

            sum_power_out += processed_l * processed_l + processed_r * processed_r;
            sum_power_l += processed_l * processed_l;
            sum_power_r += processed_r * processed_r;

            if bypass {
                out0[i] = in_l;
                out1[i] = in_r;
            } else {
                max_out_peak = max_out_peak.max(processed_l.abs()).max(processed_r.abs());
                max_out_peak_l = max_out_peak_l.max(processed_l.abs());
                max_out_peak_r = max_out_peak_r.max(processed_r.abs());
                out0[i] = processed_l;
                out1[i] = processed_r;
            }

            let (output_l, output_r) = if bypass {
                (in_l, in_r)
            } else {
                (processed_l, processed_r)
            };

            // Correlation
            let corr_lr = output_l * output_r;
            let corr_l2 = output_l * output_l;
            let corr_r2 = output_r * output_r;
            self.corr_avg_lr = (1.0 - self.correlation_decay_coef) * self.corr_avg_lr
                + self.correlation_decay_coef * corr_lr;
            self.corr_avg_l2 = (1.0 - self.correlation_decay_coef) * self.corr_avg_l2
                + self.correlation_decay_coef * corr_l2;
            self.corr_avg_r2 = (1.0 - self.correlation_decay_coef) * self.corr_avg_r2
                + self.correlation_decay_coef * corr_r2;

            // SNAP FFT capture
            if snap_phase > 0 {
                let sample = match snap_phase {
                    1 | 2 => (output_l + output_r) * 0.5,
                    3 => {
                        let out_mono = (output_l + output_r) * 0.5;
                        let in_mono = (in_l + in_r) * 0.5;
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
                                .store(false, std::sync::atomic::Ordering::Release);
                        } else {
                            self.snap_fft.reset_snapshots();
                        }
                        self.params
                            .shared
                            .snap_phase
                            .store(next_phase, std::sync::atomic::Ordering::Release);
                        snap_phase = next_phase;
                    }
                }
            }
        }

        let sample_weight = 1.0 / count_samples as f32;
        let buf_coef = 1.0 - (-(num_samples as f32) / (0.1 * sample_rate)).exp();

        // Stereo balance smoothing
        let avg_power_l = sum_power_l * sample_weight;
        let avg_power_r = sum_power_r * sample_weight;
        self.smoothed_power_l = (1.0 - buf_coef) * self.smoothed_power_l + buf_coef * avg_power_l;
        self.smoothed_power_r = (1.0 - buf_coef) * self.smoothed_power_r + buf_coef * avg_power_r;
        let rms_l = self.smoothed_power_l.sqrt();
        let rms_r = self.smoothed_power_r.sqrt();
        let sum_rms = rms_l + rms_r;
        let balance = if sum_rms > 1e-6 {
            (rms_l - rms_r) / sum_rms
        } else {
            0.0
        };
        self.params
            .shared
            .balance
            .store(balance, std::sync::atomic::Ordering::Release);

        // Band power → dB
        for b in 0..BAND_COUNT {
            let average_band_power = block_band_power[b] * sample_weight;
            self.smoothed_band_power[b] =
                (1.0 - buf_coef) * self.smoothed_band_power[b] + buf_coef * average_band_power;
            let band_db = gain_to_db(self.smoothed_band_power[b].sqrt());
            self.params.shared.band_levels[b].store(band_db, std::sync::atomic::Ordering::Release);

            if listen {
                let pow = block_input_band_power[b] as f64;
                self.listen_band_power_sum[b] += pow;

                let input_avg_pow = block_input_band_power[b] * sample_weight;
                let input_avg_f64 = input_avg_pow as f64;

                self.listen_ref_ema[b] = 0.01 * input_avg_f64 + 0.99 * self.listen_ref_ema[b];
                let gate = (self.listen_ref_ema[b] * 0.01).max(1e-6);

                if input_avg_f64 > gate {
                    if !self.listen_lo_ema[b].is_finite() {
                        self.listen_lo_ema[b] = input_avg_f64;
                        self.listen_hi_ema[b] = input_avg_f64;
                    } else {
                        if input_avg_f64 < self.listen_lo_ema[b] {
                            self.listen_lo_ema[b] += 0.15 * (input_avg_f64 - self.listen_lo_ema[b]);
                        } else {
                            self.listen_lo_ema[b] += 0.02 * (input_avg_f64 - self.listen_lo_ema[b]);
                        }
                        if input_avg_f64 > self.listen_hi_ema[b] {
                            self.listen_hi_ema[b] += 0.15 * (input_avg_f64 - self.listen_hi_ema[b]);
                        } else {
                            self.listen_hi_ema[b] += 0.02 * (input_avg_f64 - self.listen_hi_ema[b]);
                        }
                    }
                }
            }
        }

        // Listen analysis post-processing
        if listen {
            self.listen_sample_count += count_samples as u64;
            self.params.shared.listen_samples.store(
                self.listen_sample_count as f32,
                std::sync::atomic::Ordering::Release,
            );

            if self.listen_sample_count > 0 {
                let div = 1.0 / self.listen_sample_count as f64;
                for b in 0..BAND_COUNT {
                    let avg_pow = self.listen_band_power_sum[b] * div;
                    let lo_pow = if self.listen_lo_ema[b].is_finite() {
                        self.listen_lo_ema[b]
                    } else {
                        avg_pow
                    };
                    let hi_pow = if self.listen_hi_ema[b].is_finite() {
                        self.listen_hi_ema[b]
                    } else {
                        avg_pow
                    };

                    let avg_db = gain_to_db((avg_pow as f32).sqrt());
                    let lo_db = gain_to_db((lo_pow.max(1e-10) as f32).sqrt());
                    let hi_db = gain_to_db((hi_pow.max(1e-10) as f32).sqrt());

                    const ALPHA: f32 = 0.2;
                    self.listen_levels_ema[b] =
                        ALPHA * avg_db + (1.0 - ALPHA) * self.listen_levels_ema[b];
                    let listen_tolerance = (hi_db - lo_db) * 0.5;

                    self.params.shared.listen_levels[b].store(
                        self.listen_levels_ema[b],
                        std::sync::atomic::Ordering::Release,
                    );
                    self.params.shared.listen_level_min[b]
                        .store(lo_db, std::sync::atomic::Ordering::Release);
                    self.params.shared.listen_level_max[b]
                        .store(hi_db, std::sync::atomic::Ordering::Release);
                    self.params.shared.listen_tolerances[b]
                        .store(listen_tolerance, std::sync::atomic::Ordering::Release);
                }
            }
        } else if self.listen_sample_count > 0 {
            self.listen_sample_count = 0;
            self.params
                .shared
                .listen_samples
                .store(0.0, std::sync::atomic::Ordering::Release);
            for b in 0..BAND_COUNT {
                self.listen_band_power_sum[b] = 0.0;
                self.listen_lo_ema[b] = f64::INFINITY;
                self.listen_hi_ema[b] = f64::NEG_INFINITY;
                self.listen_ref_ema[b] = 0.0;
                self.params.shared.listen_tolerances[b]
                    .store(0.0, std::sync::atomic::Ordering::Release);
            }
        }

        // Correlation
        let den = (self.corr_avg_l2 * self.corr_avg_r2).sqrt();
        let correlation = if den > 1e-9 {
            self.corr_avg_lr / den
        } else {
            1.0
        };
        self.params.shared.phase_correlation.store(
            correlation.clamp(-1.0, 1.0),
            std::sync::atomic::Ordering::Release,
        );

        // Peak meters
        let block_peak_db = gain_to_db(max_out_peak);
        self.params
            .shared
            .output_peak
            .store(block_peak_db, std::sync::atomic::Ordering::Release);
        if block_peak_db > self.peak_hold_value {
            self.peak_hold_value = block_peak_db;
        }
        self.params
            .shared
            .peak_hold
            .store(self.peak_hold_value, std::sync::atomic::Ordering::Release);

        let peak_l_db = gain_to_db(max_out_peak_l);
        let peak_r_db = gain_to_db(max_out_peak_r);
        self.params
            .shared
            .output_peak_l
            .store(peak_l_db, std::sync::atomic::Ordering::Release);
        self.params
            .shared
            .output_peak_r
            .store(peak_r_db, std::sync::atomic::Ordering::Release);
        if peak_l_db > self.peak_hold_l_value {
            self.peak_hold_l_value = peak_l_db;
        }
        if peak_r_db > self.peak_hold_r_value {
            self.peak_hold_r_value = peak_r_db;
        }
        self.params
            .shared
            .peak_hold_l
            .store(self.peak_hold_l_value, std::sync::atomic::Ordering::Release);
        self.params
            .shared
            .peak_hold_r
            .store(self.peak_hold_r_value, std::sync::atomic::Ordering::Release);

        // Auto gain
        if auto_gain && count_samples > 0 {
            let avg_power_in = sum_power_in * sample_weight;
            let avg_power_out = sum_power_out * sample_weight;
            if avg_power_out > 1e-9 && avg_power_in > 1e-9 {
                let ratio = (avg_power_in / avg_power_out).sqrt();
                self.auto_gain_comp = 0.95 * self.auto_gain_comp + 0.05 * ratio;
            } else {
                self.auto_gain_comp = 1.0;
            }
        } else {
            self.auto_gain_comp = 1.0;
        }

        // AUTO LOUD
        if self
            .params
            .shared
            .auto_loud_trigger
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.params
                .shared
                .auto_loud_trigger
                .store(false, std::sync::atomic::Ordering::Release);
            self.params
                .shared
                .auto_loud_measuring
                .store(true, std::sync::atomic::Ordering::Release);
            self.auto_loud_in.reset();
            self.auto_loud_out.reset();
        }
        if is_measuring {
            self.auto_loud_out.feed(out0, out1);
            let target_samples = (5.0 * sample_rate as f64) as u64;
            if self.auto_loud_out.sample_count() >= target_samples {
                let in_lufs = self.auto_loud_in.loudness_db();
                let out_lufs = self.auto_loud_out.loudness_db();
                let out_tp = self.auto_loud_out.true_peak_db();
                let lufs_offset = in_lufs - out_lufs;
                let peak_limit = DBTP_CEILING - out_tp;
                let offset_clamped = lufs_offset.clamp(-24.0, peak_limit);
                self.params
                    .shared
                    .auto_loud_gain_offset
                    .store(offset_clamped, std::sync::atomic::Ordering::Release);
                self.params
                    .shared
                    .auto_loud_measuring
                    .store(false, std::sync::atomic::Ordering::Release);
            }
        }

        // PRE-MASTER
        if self.params.pre_master_active.value() {
            let target_linear = db_to_gain(self.params.pre_master_target_db.raw_target() as f32);
            let n = out0.len().min(out1.len());
            let sr_safe = if sample_rate > 0.0 {
                sample_rate
            } else {
                48_000.0
            };
            let measure_samples = (0.200 * sr_safe) as u32;

            if !self.pre_master_active_prev {
                self.pre_master_measure_peak = 0.0;
                self.pre_master_measure_count = 0;
                self.pre_master_gain = 1.0;
                self.pre_master_active_prev = true;
            }

            if self.pre_master_measure_count < measure_samples {
                let mut block_peak = 0.0f32;
                for i in 0..n {
                    block_peak = block_peak.max(out0[i].abs()).max(out1[i].abs());
                }
                self.pre_master_measure_peak = self.pre_master_measure_peak.max(block_peak);
                self.pre_master_measure_count += n as u32;
            }

            if self.pre_master_measure_count >= measure_samples && self.pre_master_gain == 1.0 {
                let gate = db_to_gain(-50.0);
                if self.pre_master_measure_peak > gate {
                    let max_boost = db_to_gain(12.0);
                    let max_cut = db_to_gain(-24.0);
                    self.pre_master_gain =
                        (target_linear / self.pre_master_measure_peak).clamp(max_cut, max_boost);
                } else {
                    self.pre_master_measure_count = 0;
                    self.pre_master_measure_peak = 0.0;
                }
            }

            for i in 0..n {
                out0[i] *= self.pre_master_gain;
                out1[i] *= self.pre_master_gain;
            }

            let mut post_peak = 0.0f32;
            let mut post_peak_l = 0.0f32;
            let mut post_peak_r = 0.0f32;
            for i in 0..n {
                let abs_l = out0[i].abs();
                let abs_r = out1[i].abs();
                post_peak = post_peak.max(abs_l).max(abs_r);
                post_peak_l = post_peak_l.max(abs_l);
                post_peak_r = post_peak_r.max(abs_r);
            }
            let post_db = gain_to_db(post_peak.max(1e-9));
            self.params
                .shared
                .output_peak
                .store(post_db, std::sync::atomic::Ordering::Release);
            if post_db > self.peak_hold_value {
                self.peak_hold_value = post_db;
            }
            self.params
                .shared
                .peak_hold
                .store(self.peak_hold_value, std::sync::atomic::Ordering::Release);
            let post_l_db = gain_to_db(post_peak_l.max(1e-9));
            let post_r_db = gain_to_db(post_peak_r.max(1e-9));
            self.params
                .shared
                .output_peak_l
                .store(post_l_db, std::sync::atomic::Ordering::Release);
            self.params
                .shared
                .output_peak_r
                .store(post_r_db, std::sync::atomic::Ordering::Release);
            if post_l_db > self.peak_hold_l_value {
                self.peak_hold_l_value = post_l_db;
            }
            if post_r_db > self.peak_hold_r_value {
                self.peak_hold_r_value = post_r_db;
            }
            self.params
                .shared
                .peak_hold_l
                .store(self.peak_hold_l_value, std::sync::atomic::Ordering::Release);
            self.params
                .shared
                .peak_hold_r
                .store(self.peak_hold_r_value, std::sync::atomic::Ordering::Release);
        } else {
            self.pre_master_gain = 1.0;
            self.pre_master_active_prev = false;
        }

        // Goniometer scope buffer
        {
            let start_pos = self
                .params
                .shared
                .scope_write_pos
                .load(std::sync::atomic::Ordering::Acquire);
            if let Ok(mut scope) = self.params.shared.scope_samples.try_lock() {
                let buf_len = SCOPE_BUFFER_LEN;
                let n = out0.len().min(out1.len());
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
                self.params.shared.scope_write_pos.store(
                    (start_pos + n) % buf_len,
                    std::sync::atomic::Ordering::Release,
                );
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
                    "low_gain" => self.params.low_gain.set_value(value),
                    "bass_gain" => self.params.bass_gain.set_value(value),
                    "mid_gain" => self.params.mid_gain.set_value(value),
                    "high_mid_gain" => self.params.high_mid_gain.set_value(value),
                    "high_gain" => self.params.high_gain.set_value(value),
                    "low_width" => self.params.low_width.set_value(value),
                    "bass_width" => self.params.bass_width.set_value(value),
                    "mid_width" => self.params.mid_width.set_value(value),
                    "high_mid_width" => self.params.high_mid_width.set_value(value),
                    "high_width" => self.params.high_width.set_value(value),
                    "low_pan" => self.params.low_pan.set_value(value),
                    "bass_pan" => self.params.bass_pan.set_value(value),
                    "mid_pan" => self.params.mid_pan.set_value(value),
                    "high_mid_pan" => self.params.high_mid_pan.set_value(value),
                    "high_pan" => self.params.high_pan.set_value(value),
                    "mono_floor" => self.params.mono_floor.set_value(value),
                    "output_gain" => self.params.output_gain.set_value(value),
                    "solo_low" => self.params.solo_low.set_value(value != 0.0),
                    "solo_bass" => self.params.solo_bass.set_value(value != 0.0),
                    "solo_mid" => self.params.solo_mid.set_value(value != 0.0),
                    "solo_high_mid" => self.params.solo_high_mid.set_value(value != 0.0),
                    "solo_high" => self.params.solo_high.set_value(value != 0.0),
                    "mono_active" => self.params.mono_active.set_value(value != 0.0),
                    "delta_active" => self.params.delta_active.set_value(value != 0.0),
                    "listen_active" => self.params.listen_active.set_value(value != 0.0),
                    "auto_gain_active" => self.params.auto_gain_active.set_value(value != 0.0),
                    "bypass_active" => self.params.bypass_active.set_value(value != 0.0),
                    "pre_master_active" => self.params.pre_master_active.set_value(value != 0.0),
                    "pre_master_target_db" => self.params.pre_master_target_db.set_value(value),
                    _ => {} // Unknown param — skip silently
                }
            }
        }
        Ok(())
    }
    fn state_changed(&mut self) {}

    fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
        // Vizia port (CLAP-vault features/2026-07-04-truce-2.0-upgrade-plan.md).
        // `shared` is captured directly into the setup closure rather than
        // read through `ParamLens` - the band meters/goniometer/preset data
        // live in `EquilibriumParams::shared` (atomics + mutexes written by
        // `process()`), not in the param store `ParamLens` binds to.
        let shared = params.shared.clone();
        ViziaEditor::<EquilibriumParams>::new(
            params.clone(),
            (WINDOW_W, WINDOW_H),
            move |cx, lens| editor::build(cx, lens, shared.clone(), params.clone()),
        )
        .into_editor()
    }
}

truce::plugin! { logic: Equilibrium, params: EquilibriumParams }
