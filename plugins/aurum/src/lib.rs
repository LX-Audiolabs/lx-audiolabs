// Aurum — All-In-One Mastering (truce port with tabs).
//
// Signal chain:
//   IN → [1] Input Monitor → [2] Clipper → [3] M/S EQ → [4] 2-Band Comp
//      → [5] Sweetening EQ → [6] Saturator → [7] MB Limiter
//      → [8] TP Limiter → [MONITOR] → OUT
//
// Tabs:
//   SHAPE (0): Input + Clipper + M/S EQ
//   COLOR (1): 2-Band Comp + Sweetening + Saturator
//   LIMIT (2): MB Limiter + TP Limiter + RMS/LUFS

use truce::prelude::*;
use truce_core::editor::Editor;
use truce_core::state::StateLoadError;
use truce_iced::IcedEditor;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use shared_dsp::{
    Biquad, MsBandLimiter, MsEq, TwoBandCompressor, SweeteningEq,
    MasteringClipper, MasteringSaturator, HarmonicsMode, ToleranceTable,
};
use shared_analysis::SharedState;

mod editor;

const WINDOW_W: u32 = 1100;
const WINDOW_H: u32 = 660;

mod at_tol {
    pub const EQ_SIDE_LO_FC:   usize = 0;
    pub const EQ_SIDE_LO_GAIN: usize = 1;
    pub const EQ_SIDE_HI_FC:   usize = 2;
    pub const EQ_SIDE_HI_GAIN: usize = 3;
    pub const SAT_SIDE_DRIVE:  usize = 4;
    pub const R_TRIM:          usize = 5;
    pub const COUNT:           usize = 6;
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct AurumParams {
    // Monitor
    #[param(name = "Side", default = 0)]
    pub side_active: BoolParam,
    #[param(name = "Mono", default = 0)]
    pub mono_active: BoolParam,
    #[param(name = "Delta", default = 0)]
    pub delta_active: BoolParam,
    #[param(name = "Bypass", default = 0)]
    pub bypass_active: BoolParam,

    // Analogue Tolerance
    #[param(name = "AT", default = 0)]
    pub at_active: BoolParam,
    #[param(name = "AT Amount", default = 50.0, range = "linear(0.0, 100.0)", unit = "%")]
    pub at_amount: FloatParam,

    // Output
    #[param(name = "Output Gain", default = 0.0, range = "linear(-12.0, 12.0)", unit = "dB", smooth = "linear(20)")]
    pub output_gain: FloatParam,
    #[param(name = "Stereo Width", default = 1.0, range = "linear(0.0, 2.0)", smooth = "linear(20)")]
    pub stereo_width: FloatParam,
    #[param(name = "Mono Floor", default = 0.0, range = "linear(0.0, 300.0)", unit = "Hz")]
    pub mono_floor: FloatParam,

    // Clipper
    #[param(name = "Clip Ceiling", default = -1.0, range = "linear(-6.0, -0.1)", unit = "dB", smooth = "linear(20)")]
    pub clip_ceiling: FloatParam,
    #[param(name = "Clip Softness", default = 50.0, range = "linear(0.0, 100.0)", unit = "%", smooth = "linear(20)")]
    pub clip_softness: FloatParam,
    #[param(name = "Clip M/S Mode", default = 0)]
    pub clip_ms_mode: BoolParam,

    // M/S EQ Mid
    #[param(name = "Mid Lo Shelf", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_m_lo_shelf: FloatParam,
    #[param(name = "Mid Lo-Mid", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_m_lo_mid: FloatParam,
    #[param(name = "Mid Hi-Mid", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_m_hi_mid: FloatParam,
    #[param(name = "Mid Hi Shelf", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_m_hi_shelf: FloatParam,

    // M/S EQ Side
    #[param(name = "Side Lo Shelf", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_s_lo_shelf: FloatParam,
    #[param(name = "Side Lo-Mid", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_s_lo_mid: FloatParam,
    #[param(name = "Side Hi-Mid", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_s_hi_mid: FloatParam,
    #[param(name = "Side Hi Shelf", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB")]
    pub eq_s_hi_shelf: FloatParam,

    // 2-Band Compressor
    #[param(name = "Comp Split", default = 200.0, range = "linear(80.0, 500.0)", unit = "Hz")]
    pub comp_split: FloatParam,
    #[param(name = "Comp Link", default = 0)]
    pub comp_link: BoolParam,
    #[param(name = "Thresh Low", default = -12.0, range = "linear(-30.0, 0.0)", unit = "dB")]
    pub comp_thresh_lo: FloatParam,
    #[param(name = "Thresh High", default = -12.0, range = "linear(-30.0, 0.0)", unit = "dB")]
    pub comp_thresh_hi: FloatParam,
    #[param(name = "Comp Ratio", default = 1.5, range = "linear(1.2, 3.0)")]
    pub comp_ratio: FloatParam,
    #[param(name = "Comp Attack", default = 30.0, range = "linear(10.0, 100.0)", unit = "ms")]
    pub comp_attack: FloatParam,
    #[param(name = "Comp Release", default = 150.0, range = "linear(50.0, 500.0)", unit = "ms")]
    pub comp_release: FloatParam,
    #[param(name = "Comp Mix", default = 50.0, range = "linear(0.0, 100.0)", unit = "%")]
    pub comp_mix: FloatParam,

    // Sweetening EQ
    #[param(name = "HPF Freq", default = 24.0, range = "linear(10.0, 60.0)", unit = "Hz")]
    pub sweet_hpf: FloatParam,
    #[param(name = "LPF Freq", default = 35000.0, range = "linear(18000.0, 40000.0)", unit = "Hz")]
    pub sweet_lpf: FloatParam,
    #[param(name = "Sweet Lo Shelf", default = 0.0, range = "linear(-4.0, 4.0)", unit = "dB")]
    pub sweet_lo_shelf: FloatParam,
    #[param(name = "Sweet Hi Shelf", default = 0.0, range = "linear(-4.0, 4.0)", unit = "dB")]
    pub sweet_hi_shelf: FloatParam,

    // M/S Multiband Limiter
    #[param(name = "MB Crossover", default = 250.0, range = "linear(20.0, 500.0)", unit = "Hz")]
    pub mb_crossover:    FloatParam,
    #[param(name = "MB Thresh Mid-Lo", default = -3.0, range = "linear(-18.0, 0.0)", unit = "dB")]
    pub mb_thresh_mid_lo: FloatParam,
    #[param(name = "MB Thresh Mid-Hi", default = -3.0, range = "linear(-18.0, 0.0)", unit = "dB")]
    pub mb_thresh_mid_hi: FloatParam,
    #[param(name = "MB Thresh Side",   default = -6.0, range = "linear(-18.0, 0.0)", unit = "dB")]
    pub mb_thresh_side:   FloatParam,
    #[param(name = "MB Gain Mid-Lo", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB", smooth = "linear(20)")]
    pub mb_gain_mid_lo:  FloatParam,
    #[param(name = "MB Gain Mid-Hi", default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB", smooth = "linear(20)")]
    pub mb_gain_mid_hi:  FloatParam,
    #[param(name = "MB Gain Side",   default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB", smooth = "linear(20)")]
    pub mb_gain_side:    FloatParam,
    #[param(name = "MB Attack Mid-Lo", default = 5.0,  range = "linear(0.1, 50.0)", unit = "ms")]
    pub mb_attack_mid_lo: FloatParam,
    #[param(name = "MB Attack Mid-Hi", default = 2.0,  range = "linear(0.1, 50.0)", unit = "ms")]
    pub mb_attack_mid_hi: FloatParam,
    #[param(name = "MB Attack Side",   default = 5.0,  range = "linear(0.1, 50.0)", unit = "ms")]
    pub mb_attack_side:   FloatParam,
    #[param(name = "MB Release Mid-Lo", default = 100.0, range = "linear(10.0, 500.0)", unit = "ms")]
    pub mb_release_mid_lo: FloatParam,
    #[param(name = "MB Release Mid-Hi", default =  80.0, range = "linear(10.0, 500.0)", unit = "ms")]
    pub mb_release_mid_hi: FloatParam,
    #[param(name = "MB Release Side",   default = 150.0, range = "linear(10.0, 500.0)", unit = "ms")]
    pub mb_release_side:   FloatParam,
    #[param(name = "MB Fader Link", default = 0)]
    pub mb_fader_link:   BoolParam,
    #[param(name = "MB Global Gain",  default = 0.0, range = "linear(-6.0, 6.0)", unit = "dB", smooth = "linear(20)")]
    pub mb_global_gain:  FloatParam,
    #[param(name = "MB Global Thresh", default = 0.0, range = "linear(-18.0, 0.0)", unit = "dB")]
    pub mb_global_thresh: FloatParam,
    #[param(name = "MB Mode Modern", default = 1)] // true=Modern, false=Classic
    pub mb_mode:         BoolParam,

    // True Peak Limiter
    #[param(name = "Lim Ceiling", default = -1.0, range = "linear(-6.0, -0.1)", unit = "dB")]
    pub lim_ceiling: FloatParam,
    #[param(name = "Lim Release", default = 100.0, range = "linear(10.0, 500.0)", unit = "ms")]
    pub lim_release: FloatParam,

    // Saturator
    #[param(name = "Sat M/S Mode", default = 0)]
    pub sat_ms_mode: BoolParam,
    #[param(name = "Drive Stereo", default = 0.0, range = "linear(0.0, 12.0)", unit = "dB", smooth = "linear(20)")]
    pub sat_drive_stereo: FloatParam,
    #[param(name = "Drive Mid", default = 0.0, range = "linear(0.0, 12.0)", unit = "dB", smooth = "linear(20)")]
    pub sat_drive_mid: FloatParam,
    #[param(name = "Drive Side", default = 0.0, range = "linear(0.0, 12.0)", unit = "dB", smooth = "linear(20)")]
    pub sat_drive_side: FloatParam,
    #[param(name = "Sat Mix", default = 20.0, range = "linear(0.0, 60.0)", unit = "%", smooth = "linear(20)")]
    pub sat_mix: FloatParam,
    #[param(name = "Sat Harmonics", default = 2, range = "discrete(0, 2)")] // 0=Even, 1=Odd, 2=Mixed
    pub sat_harmonics: IntParam,

    #[skip]
    pub shared: Arc<SharedState>,

    /// Test-only hook: lets screenshot tests pick which tab
    /// `AurumEditor::new()` starts on. `selected_tab` is pure UI state
    /// with no CLAP-facing equivalent, and `truce_test::screenshot!`
    /// only exposes `.set_param()` / `.setup(|plugin| ..)` (mutates the
    /// plugin, not the editor) before the single-shot render - this is
    /// the smallest way to reach it. Compiled out entirely in release
    /// builds (`#[cfg(test)]`), zero runtime/state-serialization impact.
    #[cfg(test)]
    #[skip]
    pub test_initial_tab: std::sync::atomic::AtomicUsize,
}

// ─── Plugin ───────────────────────────────────────────────────────────────────

pub struct Aurum {
    params: Arc<AurumParams>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    peak_hold_value: f32,
    peak_hold_l_value: f32,
    peak_hold_r_value: f32,
    corr_avg_lr: f32,
    corr_avg_l2: f32,
    corr_avg_r2: f32,
    sample_rate: f32,
    ms_eq: MsEq,
    two_band_comp: TwoBandCompressor,
    sweet_l: SweeteningEq,
    sweet_r: SweeteningEq,
    ms_band_lim: MsBandLimiter,
    lim_gain: f32,
    lim_prev_l: f32,
    lim_prev_r: f32,
    tol: ToleranceTable<{ at_tol::COUNT }>,
    at_sat_side_trim: f32,
    mono_floor_filter: Biquad,
}

impl Aurum {
    pub fn new(params: Arc<AurumParams>) -> Self {
        Self {
            params,
            shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            peak_hold_value: -90.0,
            peak_hold_l_value: -90.0,
            peak_hold_r_value: -90.0,
            corr_avg_lr: 0.0, corr_avg_l2: 1e-9, corr_avg_r2: 1e-9,
            sample_rate: 44100.0,
            ms_eq: MsEq::new(),
            two_band_comp: TwoBandCompressor::new(),
            sweet_l: SweeteningEq::new(), sweet_r: SweeteningEq::new(),
            ms_band_lim: MsBandLimiter::new(),
            lim_gain: 1.0, lim_prev_l: 0.0, lim_prev_r: 0.0,
            tol: ToleranceTable::new(0x4c584c6162415552u64),
            at_sat_side_trim: 0.0,
            mono_floor_filter: Biquad::new(),
        }
    }
}

impl Drop for Aurum {
    fn drop(&mut self) { self.shutdown.store(true, Ordering::Release); }
}

// ─── PluginLogic ──────────────────────────────────────────────────────────────

impl PluginLogic for Aurum {
    fn reset(&mut self, sr: f64, _max: usize) {
        let sr = sr as f32;
        self.sample_rate = sr;
        self.ms_eq.set_sample_rate(sr);
        self.two_band_comp.set_sample_rate(sr);
        self.sweet_l.set_sample_rate(sr);
        self.sweet_r.set_sample_rate(sr);
        self.ms_band_lim.set_sample_rate(sr);
        self.ms_band_lim.set_crossover(self.params.mb_crossover.raw_target() as f32);
        let mf = self.params.mono_floor.raw_target() as f32;
        if mf > 1.0 { self.mono_floor_filter.set_butterworth_hp(mf, sr); }
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _events: &EventList, _ctx: &mut ProcessContext) -> ProcessStatus {
        #[cfg(target_arch = "x86_64")]
        #[allow(deprecated)]
        unsafe { let csr = std::arch::x86_64::_mm_getcsr(); std::arch::x86_64::_mm_setcsr(csr | 0x8040); }

        if self.params.bypass_active.value() { return ProcessStatus::Normal; }

        if self.params.shared.reset_peak.swap(false, Ordering::Relaxed) {
            self.peak_hold_value = -90.0; self.peak_hold_l_value = -90.0; self.peak_hold_r_value = -90.0;
            self.ms_band_lim.reset(); self.lim_gain = 1.0; self.lim_prev_l = 0.0; self.lim_prev_r = 0.0;
        }

        if buffer.num_input_channels() < 2 || buffer.num_samples() == 0 { return ProcessStatus::Normal; }

        let side_mon = self.params.side_active.value();
        let mono = self.params.mono_active.value();
        let delta = self.params.delta_active.value();
        let sr = self.sample_rate;

        let mono_floor_freq = self.params.mono_floor.raw_target() as f32;
        if mono_floor_freq > 1.0 { self.mono_floor_filter.set_butterworth_hp(mono_floor_freq, sr); }

        // AT
        let at = if self.params.at_active.value() { self.params.at_amount.raw_target() as f32 * 0.01 } else { 0.0 };
        let r = |slot: usize, max: f32| -> f32 { self.tol.get(slot) * at * max };
        self.at_sat_side_trim = r(at_tol::SAT_SIDE_DRIVE, 0.010);
        let at_r_trim = r(at_tol::R_TRIM, 0.025);

        // [2] Clipper
        let clip_ms_mode = self.params.clip_ms_mode.value();

        // [3] M/S EQ
        self.ms_eq.mid_bands[0].set_low_shelf(80.0, self.params.eq_m_lo_shelf.raw_target() as f32, 1.0, sr);
        self.ms_eq.mid_bands[1].set_peaking_eq(250.0, self.params.eq_m_lo_mid.raw_target() as f32, 0.7, sr);
        self.ms_eq.mid_bands[2].set_peaking_eq(4000.0, self.params.eq_m_hi_mid.raw_target() as f32, 0.7, sr);
        self.ms_eq.mid_bands[3].set_high_shelf(12000.0, self.params.eq_m_hi_shelf.raw_target() as f32, 1.0, sr);
        self.ms_eq.side_bands[0].set_low_shelf(
            80.0 * (1.0 + r(at_tol::EQ_SIDE_LO_FC, 0.050)),
            self.params.eq_s_lo_shelf.raw_target() as f32 + r(at_tol::EQ_SIDE_LO_GAIN, 0.025), 1.0, sr);
        self.ms_eq.side_bands[1].set_peaking_eq(250.0, self.params.eq_s_lo_mid.raw_target() as f32, 0.7, sr);
        self.ms_eq.side_bands[2].set_peaking_eq(4000.0, self.params.eq_s_hi_mid.raw_target() as f32, 0.7, sr);
        self.ms_eq.side_bands[3].set_high_shelf(
            12000.0 * (1.0 + r(at_tol::EQ_SIDE_HI_FC, 0.050)),
            self.params.eq_s_hi_shelf.raw_target() as f32 + r(at_tol::EQ_SIDE_HI_GAIN, 0.025), 1.0, sr);

        // [4] 2-Band Comp
        let comp_link = self.params.comp_link.value();
        let comp_thresh_lo = self.params.comp_thresh_lo.raw_target() as f32;
        let comp_thresh_hi = if comp_link { comp_thresh_lo } else { self.params.comp_thresh_hi.raw_target() as f32 };
        let comp_ratio = self.params.comp_ratio.raw_target() as f32;
        let comp_attack = self.params.comp_attack.raw_target() as f32;
        let comp_release = self.params.comp_release.raw_target() as f32;
        let comp_mix = self.params.comp_mix.raw_target() as f32;
        self.two_band_comp.set_split_freq(self.params.comp_split.raw_target() as f32);

        // [5] Sweetening
        let sweet_hpf = self.params.sweet_hpf.raw_target() as f32;
        let sweet_lpf = self.params.sweet_lpf.raw_target() as f32;
        let sweet_lo = self.params.sweet_lo_shelf.raw_target() as f32;
        let sweet_hi = self.params.sweet_hi_shelf.raw_target() as f32;
        for sw in [&mut self.sweet_l, &mut self.sweet_r] {
            sw.hpf.set_butterworth_hp(sweet_hpf, sr);
            sw.lpf.set_butterworth_lp(sweet_lpf, sr);
            sw.lo_shelf.set_low_shelf(200.0, sweet_lo, 1.0, sr);
            sw.hi_shelf.set_high_shelf(8000.0, sweet_hi, 1.0, sr);
        }

        // [6] Saturator
        let sat_ms_mode = self.params.sat_ms_mode.value();
        let sat_mode = match self.params.sat_harmonics.value_i32() {
            0 => HarmonicsMode::Even, 1 => HarmonicsMode::Odd, _ => HarmonicsMode::Mixed,
        };

        // [7] MB Limiter
        let mb_crossover = self.params.mb_crossover.raw_target() as f32;
        let mb_classic = !self.params.mb_mode.value();
        let mb_fader_link = self.params.mb_fader_link.value();
        let mb_global_thresh_offset = self.params.mb_global_thresh.raw_target() as f32;
        self.ms_band_lim.set_crossover(mb_crossover);
        let mb_thresh_lo = 10.0_f32.powf((self.params.mb_thresh_mid_lo.raw_target() as f32 + mb_global_thresh_offset) / 20.0);
        let mb_thresh_hi = if mb_fader_link { mb_thresh_lo }
            else { 10.0_f32.powf((self.params.mb_thresh_mid_hi.raw_target() as f32 + mb_global_thresh_offset) / 20.0) };
        let mb_thresh_side = if mb_fader_link { mb_thresh_lo }
            else { 10.0_f32.powf((self.params.mb_thresh_side.raw_target() as f32 + mb_global_thresh_offset) / 20.0) };
        let coeff = |ms: f32| (-1.0_f32 / (ms * 0.001 * sr)).exp();
        let mb_att_lo = coeff(self.params.mb_attack_mid_lo.raw_target() as f32);
        let mb_att_hi = coeff(self.params.mb_attack_mid_hi.raw_target() as f32);
        let mb_att_side = coeff(self.params.mb_attack_side.raw_target() as f32);
        let mb_rel_lo = coeff(self.params.mb_release_mid_lo.raw_target() as f32);
        let mb_rel_hi = coeff(self.params.mb_release_mid_hi.raw_target() as f32);
        let mb_rel_side = coeff(self.params.mb_release_side.raw_target() as f32);

        // TP Limiter
        let lim_ceiling = 10.0_f32.powf(self.params.lim_ceiling.raw_target() as f32 / 20.0);
        let lim_release_coeff = {
            let release_s = self.params.lim_release.raw_target() as f32 * 0.001;
            (-1.0_f32 / (release_s * sr)).exp()
        };

        let num_samples = buffer.num_samples();
        let mut max_peak: f32 = 0.0;
        let mut max_peak_l: f32 = 0.0;
        let mut max_peak_r: f32 = 0.0;
        let mut sum_lr: f32 = 0.0; let mut sum_l2: f32 = 0.0; let mut sum_r2: f32 = 0.0;
        let mut sum_power_l: f32 = 0.0; let mut sum_power_r: f32 = 0.0;

        #[allow(clippy::needless_range_loop)]
        for i in 0..num_samples {
            let in_l = buffer.input(0)[i];
            let in_r = buffer.input(1)[i];

            let clip_ceiling = 10.0_f32.powf(self.params.clip_ceiling.smoother.next(self.params.clip_ceiling.raw_target()) / 20.0);
            let clip_softness = self.params.clip_softness.smoother.next(self.params.clip_softness.raw_target()) / 100.0;
            let sat_drive_stereo = self.params.sat_drive_stereo.smoother.next(self.params.sat_drive_stereo.raw_target());
            let sat_drive_mid = self.params.sat_drive_mid.smoother.next(self.params.sat_drive_mid.raw_target());
            let sat_drive_side = self.params.sat_drive_side.smoother.next(self.params.sat_drive_side.raw_target());
            let sat_mix = self.params.sat_mix.smoother.next(self.params.sat_mix.raw_target()) / 100.0;
            let width = self.params.stereo_width.smoother.next(self.params.stereo_width.raw_target());
            let mb_global_gain = 10.0_f32.powf(self.params.mb_global_gain.smoother.next(self.params.mb_global_gain.raw_target()) / 20.0);
            let mb_makeup_lo = 10.0_f32.powf(self.params.mb_gain_mid_lo.smoother.next(self.params.mb_gain_mid_lo.raw_target()) / 20.0);
            let mb_makeup_hi = 10.0_f32.powf(self.params.mb_gain_mid_hi.smoother.next(self.params.mb_gain_mid_hi.raw_target()) / 20.0);
            let mb_makeup_side = 10.0_f32.powf(self.params.mb_gain_side.smoother.next(self.params.mb_gain_side.raw_target()) / 20.0);
            let out_gain = 10.0_f32.powf(self.params.output_gain.smoother.next(self.params.output_gain.raw_target()) / 20.0);

            // [2] Clipper
            let (cl, cr) = if clip_ms_mode {
                let m = (in_l + in_r) * 0.5; let s = (in_l - in_r) * 0.5;
                let cm = MasteringClipper::process(m, clip_ceiling, clip_softness);
                let cs = MasteringClipper::process(s, clip_ceiling, clip_softness);
                (cm + cs, cm - cs)
            } else {
                (MasteringClipper::process(in_l, clip_ceiling, clip_softness),
                 MasteringClipper::process(in_r, clip_ceiling, clip_softness))
            };

            // [3] M/S EQ
            let mid_eq = (cl + cr) * 0.5; let side_eq = (cl - cr) * 0.5;
            let (mid_eq_out, side_eq_out) = self.ms_eq.process(mid_eq, side_eq);
            let eq_l = mid_eq_out + side_eq_out; let eq_r = mid_eq_out - side_eq_out;

            // [4] 2-Band Comp
            let mut _gr_lo = 0.0; let mut _gr_hi = 0.0;
            let (comp_l, comp_r) = self.two_band_comp.process(eq_l, eq_r, comp_thresh_lo, comp_thresh_hi, comp_ratio, comp_attack, comp_release, comp_mix, &mut _gr_lo, &mut _gr_hi);

            // [5] Sweetening
            let sw_l = self.sweet_l.process(comp_l); let sw_r = self.sweet_r.process(comp_r);

            // [6] Saturator
            let (pre_l, pre_r) = if sat_ms_mode {
                let m = (sw_l + sw_r) * 0.5; let s = (sw_l - sw_r) * 0.5;
                let m_sat = MasteringSaturator::process(m, sat_drive_mid, sat_mix, sat_mode);
                let s_sat = MasteringSaturator::process(s, sat_drive_side * (1.0 + self.at_sat_side_trim), sat_mix, sat_mode);
                (m_sat + s_sat, m_sat - s_sat)
            } else {
                (MasteringSaturator::process(sw_l, sat_drive_stereo, sat_mix, sat_mode),
                 MasteringSaturator::process(sw_r, sat_drive_stereo, sat_mix, sat_mode))
            };

            let mid = (pre_l + pre_r) * 0.5; let side = (pre_l - pre_r) * 0.5 * width;

            // [7] MB Limiter
            let (mid_lim, side_lim) = self.ms_band_lim.process_ms(mid, side,
                mb_thresh_lo, mb_thresh_hi, mb_thresh_side,
                mb_att_lo, mb_att_hi, mb_att_side,
                mb_rel_lo, mb_rel_hi, mb_rel_side,
                mb_makeup_lo, mb_makeup_hi, mb_makeup_side,
                mb_global_gain, mb_classic);

            let mut out_l = mid_lim + side_lim;
            let mut out_r = mid_lim - side_lim;
            out_r *= 1.0 + at_r_trim;

            if mono_floor_freq > 1.0 {
                let mf_mid = (out_l + out_r) * 0.5; let mf_side = (out_l - out_r) * 0.5;
                let side_hp = self.mono_floor_filter.process(mf_side);
                out_l = mf_mid + side_hp; out_r = mf_mid - side_hp;
            }

            if side_mon { let s = (out_l - out_r) * 0.5; out_l = s; out_r = s; }
            else if mono { let m = (out_l + out_r) * 0.5; out_l = m; out_r = m; }

            if delta { out_l -= in_l; out_r -= in_r; }

            out_l *= out_gain; out_r *= out_gain;

            // [8] TP Limiter
            let interp_l = (self.lim_prev_l + out_l) * 0.5; let interp_r = (self.lim_prev_r + out_r) * 0.5;
            let true_peak = out_l.abs().max(out_r.abs()).max(interp_l.abs()).max(interp_r.abs());
            let required_gain = if true_peak > lim_ceiling { lim_ceiling / true_peak } else { 1.0 };
            self.lim_gain = self.lim_gain.min(required_gain);
            out_l *= self.lim_gain; out_r *= self.lim_gain;
            self.lim_gain = (self.lim_gain * lim_release_coeff + (1.0 - lim_release_coeff)).min(1.0);
            self.lim_prev_l = out_l; self.lim_prev_r = out_r;

            max_peak = max_peak.max(out_l.abs()).max(out_r.abs());
            max_peak_l = max_peak_l.max(out_l.abs()); max_peak_r = max_peak_r.max(out_r.abs());
            sum_lr += out_l * out_r; sum_l2 += out_l * out_l; sum_r2 += out_r * out_r;
            sum_power_l += out_l * out_l; sum_power_r += out_r * out_r;
            buffer.output(0)[i] = out_l; buffer.output(1)[i] = out_r;
        }

        let peak_db = if max_peak < 1e-9 { -90.0 } else { 20.0 * max_peak.log10() };
        self.params.shared.output_peak.store(peak_db, Ordering::Relaxed);
        self.peak_hold_value = self.peak_hold_value.max(peak_db);
        self.params.shared.peak_hold.store(self.peak_hold_value, Ordering::Relaxed);

        let peak_l_db = if max_peak_l < 1e-9 { -90.0 } else { 20.0 * max_peak_l.log10() };
        let peak_r_db = if max_peak_r < 1e-9 { -90.0 } else { 20.0 * max_peak_r.log10() };
        self.params.shared.output_peak_l.store(peak_l_db, Ordering::Relaxed);
        self.params.shared.output_peak_r.store(peak_r_db, Ordering::Relaxed);
        if peak_l_db > self.peak_hold_l_value { self.peak_hold_l_value = peak_l_db; }
        if peak_r_db > self.peak_hold_r_value { self.peak_hold_r_value = peak_r_db; }
        self.params.shared.peak_hold_l.store(self.peak_hold_l_value, Ordering::Relaxed);
        self.params.shared.peak_hold_r.store(self.peak_hold_r_value, Ordering::Relaxed);

        let decay = 0.15;
        self.corr_avg_lr = (1.0 - decay) * self.corr_avg_lr + decay * sum_lr;
        self.corr_avg_l2 = ((1.0 - decay) * self.corr_avg_l2 + decay * sum_l2).max(1e-9);
        self.corr_avg_r2 = ((1.0 - decay) * self.corr_avg_r2 + decay * sum_r2).max(1e-9);
        let corr = (self.corr_avg_lr / (self.corr_avg_l2 * self.corr_avg_r2).sqrt()).clamp(-1.0, 1.0);
        self.params.shared.phase_correlation.store(corr, Ordering::Relaxed);
        let total = sum_power_l + sum_power_r;
        if total > 1e-9 { self.params.shared.balance.store((sum_power_l - sum_power_r) / total, Ordering::Relaxed); }

        // Gain reduction
        self.params.shared.gain_reduction.store(if self.lim_gain < 1e-9 { -90.0 } else { 20.0 * self.lim_gain.log10() }, Ordering::Relaxed);

        ProcessStatus::Normal
    }

    fn save_state(&self) -> Vec<u8> { Vec::new() }
    fn load_state(&mut self, _: &[u8]) -> Result<(), StateLoadError> { Ok(()) }
    fn state_changed(&mut self) {}

    fn editor(&self) -> Box<dyn Editor> {
        IcedEditor::<AurumParams, editor::AurumEditor>::new(self.params.clone(), (WINDOW_W, WINDOW_H)).into_editor()
    }
}

truce::plugin! { logic: Aurum, params: AurumParams }

#[cfg(test)]
mod tests {
    use crate::Plugin;
    use std::time::Duration;
    use std::sync::atomic::Ordering;
    use truce_core::PluginExport;

    #[test]
    fn renders_pass_through() {
        use truce_test::{InputSource, assertions, driver};
        let result = driver!(Plugin).duration(Duration::from_millis(50)).input(InputSource::Constant(0.5)).run();
        assertions::assert_no_nans(&result);
        assertions::assert_nonzero(&result);
    }

    #[test]
    fn state_round_trips() { truce_test::assert_state_round_trip::<Plugin>(); }

    #[test]
    fn screenshot_shape_tab() {
        truce_test::screenshot!(Plugin, "screenshots/aurum-shape.png")
            .tolerance(200)
            .run();
    }

    #[test]
    fn screenshot_color_tab() {
        truce_test::screenshot!(Plugin, "screenshots/aurum-color.png")
            .set_param(crate::AurumParamsParamId::CompSplit, 0.5)
            .set_param(crate::AurumParamsParamId::SatDriveStereo, 0.3)
            .setup(|p| p.params().test_initial_tab.store(1, Ordering::Relaxed))
            .tolerance(200)
            .run();
    }

    #[test]
    fn screenshot_limit_tab() {
        truce_test::screenshot!(Plugin, "screenshots/aurum-limit.png")
            .set_param(crate::AurumParamsParamId::MbThreshMidLo, 0.4)
            .set_param(crate::AurumParamsParamId::LimCeiling, 0.7)
            .setup(|p| p.params().test_initial_tab.store(2, Ordering::Relaxed))
            .tolerance(200)
            .run();
    }
}
