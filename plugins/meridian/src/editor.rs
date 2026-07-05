//! Vizia port of the old iced `editor.rs`. See CLAP-vault
//! `features/2026-07-04-truce-2.0-upgrade-plan.md` for the Ticker/Memo/Binding
//! rationale (same pattern as `plugins/lucent/src/editor.rs` and
//! `plugins/equilibrium/src/editor.rs`, which this port follows most closely):
//! tick-frequency telemetry lives in one `Signal<Telemetry>` updated every
//! ~33ms by `Ticker` (not `cx.add_timer`/`start_timer` - vizia_core 0.4.0's
//! `modify_timer` has a real infinite-loop bug), passive display regions
//! (spectrum+EQ-curve, meters, goniometer, GR envelope) are wrapped in
//! `Binding`s keyed to it, and drag widgets (knobs/sliders) are built once
//! outside any tick-driven Binding so a drag survives across ticks. The
//! preset list and the header's MONO/DELTA/BYPASS toggles instead key off
//! `params_gen` / the param's own value signal - bumped only by discrete
//! actions, never by `tick()` - so their Buttons never hit the
//! rebuild-drops-clicks issue documented on `Ticker` below.
//!
//! Every `FloatParam`/`IntParam` write goes through the param's own
//! `.info.range.normalize(plain)` rather than a hand-rolled linear formula:
//! several of Meridian's params (HPF/LPF, the 5 EQ band frequencies, Exciter
//! freq) are `Logarithmic` ranges, and `ParamLens::set`/`automate` both take
//! an already-normalized `[0,1]` value - a hardcoded linear `(v-min)/(max-min)`
//! would silently mis-map every log-range knob.
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::time::{Duration, Instant};

use vizia::prelude::*;
use vizia::vg;

use shared_analysis::SharedState;
use shared_dsp::Biquad;
use truce::prelude::{FloatParam, IntParam};
use truce_vizia::ParamLens;

use shared_ui::{GoniometerView, SpectrumConfig, SpectrumCurve, SpectrumView, StereoMeterView, fmt_db, rgb as vg_rgb, Gesture, HSliderView, KnobView, format_knob_value, EqCurve};
use crate::vizia_canvas::CompressorEnvelopeView;
use crate::{MeridianParams, MeridianParamsParamId as K};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `vizia::prelude::Color` (CSS-style, used by `.color()`/`.background_color()`
/// view modifiers) is a different type from `vg::Color` (Skia, used inside
/// `draw()` - see `vizia_canvas::col`/`rgb`).
fn col(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color::rgba(
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
        (a.clamp(0.0, 1.0) * 255.0) as u8,
    )
}
fn rgb(r: f32, g: f32, b: f32) -> Color {
    col(r, g, b, 1.0)
}

fn short_freq(freq: f32) -> String {
    if freq < 1000.0 { format!("{freq:.0} Hz") } else { format!("{:.1} kHz", freq / 1000.0) }
}

fn slope_char(s: i32) -> &'static str {
    match s { 0 => "A", 1 => "B", _ => "C" }
}

/// Plain fn-pointer field accessors, not closures - `Copy`/`'static` with no
/// borrow to escape, so they can cross into the `'static` `on_gesture`
/// closures `KnobView`/`HSliderView`/`slope_selector` require. The
/// alternative (capturing `&FloatParam` directly) doesn't work: `FloatParam`
/// isn't `Clone`, and a bare reference into `params: Arc<MeridianParams>`
/// can't outlive the function that receives that `Arc` by value.
type FloatField = fn(&MeridianParams) -> &FloatParam;
type IntField = fn(&MeridianParams) -> &IntParam;

const GAIN_IDS: [K; 5] = [K::BassGain, K::LoMidGain, K::MidGain, K::HighGain, K::ExciteGain];
const SLOPE_IDS: [K; 5] = [K::BassSlope, K::LoMidSlope, K::MidSlope, K::HighSlope, K::ExciteSlope];
const FREQ_IDS: [K; 5] = [K::EqFreq1, K::EqFreq2, K::EqFreq3, K::EqFreq4, K::EqFreq5];
const GAIN_FIELDS: [FloatField; 5] = [
    |p| &p.bass_gain, |p| &p.lo_mid_gain, |p| &p.mid_gain, |p| &p.high_gain, |p| &p.excite_gain,
];
const FREQ_FIELDS: [FloatField; 5] = [
    |p| &p.eq_freq_1, |p| &p.eq_freq_2, |p| &p.eq_freq_3, |p| &p.eq_freq_4, |p| &p.eq_freq_5,
];
const SLOPE_FIELDS: [IntField; 5] = [
    |p| &p.bass_slope, |p| &p.lo_mid_slope, |p| &p.mid_slope, |p| &p.high_slope, |p| &p.excite_slope,
];
const FREQ_RANGES: [(f32, f32, f32); 5] = [
    (40.0, 200.0, 80.0),
    (150.0, 800.0, 300.0),
    (500.0, 3000.0, 1000.0),
    (2000.0, 10000.0, 4000.0),
    (6000.0, 20000.0, 12000.0),
];
const BAND_NAMES: [&str; 5] = ["LO SHELF", "LO-MID", "MID", "HI-MID", "HI SHELF"];
const BAND_IS_SHELF: [bool; 5] = [true, false, false, false, true];
const HZ_LABELS: [&str; 5] = ["Sub", "Bass", "Mid", "Presence", "Air"];

// ─── Preset data (framework-independent, unchanged from the iced version) ──

#[derive(Clone, Debug)]
pub struct MeridianProfile {
    pub name: String,
    pub hpf_freq: f32, pub lpf_freq: f32, pub cut_slope: i32,
    pub bass_gain: f32, pub bass_slope: i32,
    pub lo_mid_gain: f32, pub lo_mid_slope: i32,
    pub mid_gain: f32, pub mid_slope: i32,
    pub high_gain: f32, pub high_slope: i32,
    pub excite_gain: f32, pub excite_slope: i32,
    pub eq_freq_1: f32, pub eq_freq_2: f32, pub eq_freq_3: f32, pub eq_freq_4: f32, pub eq_freq_5: f32,
    pub tilt_gain: f32, pub warmth_drive: f32, pub warmth_mix: f32,
    pub excite_amount: f32, pub excite_blend: f32, pub excite_freq: f32,
    pub comp_threshold: f32, pub comp_mix: f32, pub comp_attack: f32, pub comp_release: f32,
    pub comp_character: f32, pub comp_makeup: f32,
    pub inflate_effect: f32, pub inflate_curve: f32, pub inflate_band_split: bool, pub inflate_clip: bool,
    pub stereo_width: f32, pub pan: f32, pub output_gain: f32,
}

impl Default for MeridianProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            hpf_freq: 2.0, lpf_freq: 35000.0, cut_slope: 0,
            bass_gain: 0.0, bass_slope: 1,
            lo_mid_gain: 0.0, lo_mid_slope: 1,
            mid_gain: 0.0, mid_slope: 1,
            high_gain: 0.0, high_slope: 1,
            excite_gain: 0.0, excite_slope: 1,
            eq_freq_1: 80.0, eq_freq_2: 300.0, eq_freq_3: 1000.0, eq_freq_4: 4000.0, eq_freq_5: 12000.0,
            tilt_gain: 0.0, warmth_drive: 0.0, warmth_mix: 0.0,
            excite_amount: 0.0, excite_blend: 0.0, excite_freq: 8000.0,
            comp_threshold: 0.0, comp_mix: 0.0, comp_attack: 15.0, comp_release: 120.0,
            comp_character: 2.0, comp_makeup: 0.0,
            inflate_effect: 0.0, inflate_curve: 0.0, inflate_band_split: false, inflate_clip: false,
            stereo_width: 100.0, pan: 0.0, output_gain: 0.0,
        }
    }
}

fn list_meridian_presets(vault_path: Option<&str>) -> Vec<(String, PathBuf, MeridianProfile)> {
    let mut presets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let local_dir = shared_analysis::get_plugin_dir("Meridian").join("presets");
    let _ = std::fs::create_dir_all(&local_dir);
    let mut scan = |dir: &std::path::Path| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                if path.is_file() && path.extension().is_some_and(|e| e == "md")
                    && !stem.starts_with("SNAPSHOT-") && seen.insert(path.clone())
                    && let Ok(content) = std::fs::read_to_string(&path) {
                        match shared_analysis::preset_plugin_name(&content).as_deref() {
                            Some("meridian") => {}
                            _ => continue,
                        }
                        if let Some(mut prof) = parse_meridian_markdown(&content) {
                            prof.name = stem.clone();
                            presets.push((stem, path, prof));
                        }
                    }
            }
        }
    };
    scan(&local_dir);
    if let Some(vp) = vault_path && !vp.is_empty() { scan(std::path::Path::new(vp)); }
    presets
}

fn parse_meridian_markdown(content: &str) -> Option<MeridianProfile> {
    match shared_analysis::preset_plugin_name(content).as_deref() {
        Some("meridian") => {}
        _ => return None,
    }
    let mut p = MeridianProfile::default();
    let mut has_hpf = false; let mut has_lpf = false;
    let mut has_bass = false; let mut has_mid = false; let mut has_output = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') {
            let parts: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
            if parts.len() >= 4 {
                match parts[1].to_lowercase().as_str() {
                    "hpf" => { if let Ok(v) = parts[2].parse() { p.hpf_freq = v; has_hpf = true; } }
                    "lpf" => { if let Ok(v) = parts[2].parse() { p.lpf_freq = v; has_lpf = true; } }
                    "cut slope" => { p.cut_slope = if parts[2] == "B" { 1 } else { 0 }; }
                    "bass gain" => { if let Ok(v) = parts[2].parse() { p.bass_gain = v; has_bass = true; } }
                    "bass slope" => { p.bass_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 1" => { if let Ok(v) = parts[2].parse() { p.eq_freq_1 = v; } }
                    "lo-mid gain" => { if let Ok(v) = parts[2].parse() { p.lo_mid_gain = v; } }
                    "lo-mid slope" => { p.lo_mid_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 2" => { if let Ok(v) = parts[2].parse() { p.eq_freq_2 = v; } }
                    "mid gain" => { if let Ok(v) = parts[2].parse() { p.mid_gain = v; has_mid = true; } }
                    "mid slope" => { p.mid_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 3" => { if let Ok(v) = parts[2].parse() { p.eq_freq_3 = v; } }
                    "high gain" => { if let Ok(v) = parts[2].parse() { p.high_gain = v; } }
                    "high slope" => { p.high_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 4" => { if let Ok(v) = parts[2].parse() { p.eq_freq_4 = v; } }
                    "excite gain" => { if let Ok(v) = parts[2].parse() { p.excite_gain = v; } }
                    "excite slope" => { p.excite_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 5" => { if let Ok(v) = parts[2].parse() { p.eq_freq_5 = v; } }
                    "comp threshold" => { if let Ok(v) = parts[2].parse() { p.comp_threshold = v; } }
                    "comp mix" => { if let Ok(v) = parts[2].parse() { p.comp_mix = v; } }
                    "comp attack" => { if let Ok(v) = parts[2].parse() { p.comp_attack = v; } }
                    "comp release" => { if let Ok(v) = parts[2].parse() { p.comp_release = v; } }
                    "comp character" => { if let Ok(v) = parts[2].parse() { p.comp_character = v; } }
                    "comp makeup" => { if let Ok(v) = parts[2].parse() { p.comp_makeup = v; } }
                    "inflate effect" => { if let Ok(v) = parts[2].parse() { p.inflate_effect = v; } }
                    "inflate curve" => { if let Ok(v) = parts[2].parse() { p.inflate_curve = v; } }
                    "inflate band split" => { p.inflate_band_split = parts[2] == "On"; }
                    "inflate clip" => { p.inflate_clip = parts[2] == "On"; }
                    "warmth drive" => { if let Ok(v) = parts[2].parse() { p.warmth_drive = v; } }
                    "warmth mix" => { if let Ok(v) = parts[2].parse() { p.warmth_mix = v; } }
                    "excite amount" => { if let Ok(v) = parts[2].parse() { p.excite_amount = v; } }
                    "excite blend" => { if let Ok(v) = parts[2].parse() { p.excite_blend = v; } }
                    "excite freq" => { if let Ok(v) = parts[2].parse() { p.excite_freq = v; } }
                    "tilt" => { if let Ok(v) = parts[2].parse() { p.tilt_gain = v; } }
                    "stereo width" => { if let Ok(v) = parts[2].parse() { p.stereo_width = v; } }
                    "pan" => { if let Ok(v) = parts[2].parse() { p.pan = v; } }
                    "output gain" => { if let Ok(v) = parts[2].parse() { p.output_gain = v; has_output = true; } }
                    _ => {}
                }
            }
        }
    }
    if has_hpf && has_lpf && has_bass && has_mid && has_output { Some(p) } else { None }
}

fn export_meridian_markdown(p: &MeridianProfile) -> String {
    let mut s = String::new();
    s.push_str("---\nplugin: meridian\ntype: preset\n---\n\n");
    s.push_str("> Warning: Do NOT modify column names or table structure.\n\n");
    s.push_str("## Parameter\n\n| Parameter | Wert | Einheit |\n|---|---|---|\n");
    s.push_str(&format!("| HPF | {:.1} | Hz |\n", p.hpf_freq));
    s.push_str(&format!("| LPF | {:.1} | Hz |\n", p.lpf_freq));
    s.push_str(&format!("| Cut Slope | {} | |\n", if p.cut_slope >= 1 { "B" } else { "A" }));
    s.push_str(&format!("| Bass Gain | {:.1} | dB |\n", p.bass_gain));
    s.push_str(&format!("| Bass Slope | {} | |\n", slope_char(p.bass_slope)));
    s.push_str(&format!("| EQ Freq 1 | {:.0} | Hz |\n", p.eq_freq_1));
    s.push_str(&format!("| Lo-Mid Gain | {:.1} | dB |\n", p.lo_mid_gain));
    s.push_str(&format!("| Lo-Mid Slope | {} | |\n", slope_char(p.lo_mid_slope)));
    s.push_str(&format!("| EQ Freq 2 | {:.0} | Hz |\n", p.eq_freq_2));
    s.push_str(&format!("| Mid Gain | {:.1} | dB |\n", p.mid_gain));
    s.push_str(&format!("| Mid Slope | {} | |\n", slope_char(p.mid_slope)));
    s.push_str(&format!("| EQ Freq 3 | {:.0} | Hz |\n", p.eq_freq_3));
    s.push_str(&format!("| High Gain | {:.1} | dB |\n", p.high_gain));
    s.push_str(&format!("| High Slope | {} | |\n", slope_char(p.high_slope)));
    s.push_str(&format!("| EQ Freq 4 | {:.0} | Hz |\n", p.eq_freq_4));
    s.push_str(&format!("| Excite Gain | {:.1} | dB |\n", p.excite_gain));
    s.push_str(&format!("| Excite Slope | {} | |\n", slope_char(p.excite_slope)));
    s.push_str(&format!("| EQ Freq 5 | {:.0} | Hz |\n", p.eq_freq_5));
    s.push_str(&format!("| Comp Threshold | {:.1} | dB |\n", p.comp_threshold));
    s.push_str(&format!("| Comp Mix | {:.1} | % |\n", p.comp_mix));
    s.push_str(&format!("| Comp Attack | {:.1} | ms |\n", p.comp_attack));
    s.push_str(&format!("| Comp Release | {:.1} | ms |\n", p.comp_release));
    s.push_str(&format!("| Comp Character | {:.1} | |\n", p.comp_character));
    s.push_str(&format!("| Comp Makeup | {:.1} | dB |\n", p.comp_makeup));
    s.push_str(&format!("| Inflate Effect | {:.1} | % |\n", p.inflate_effect));
    s.push_str(&format!("| Inflate Curve | {:.1} | |\n", p.inflate_curve));
    s.push_str(&format!("| Inflate Band Split | {} | |\n", if p.inflate_band_split { "On" } else { "Off" }));
    s.push_str(&format!("| Inflate Clip | {} | |\n", if p.inflate_clip { "On" } else { "Off" }));
    s.push_str(&format!("| Warmth Drive | {:.1} | dB |\n", p.warmth_drive));
    s.push_str(&format!("| Warmth Mix | {:.1} | % |\n", p.warmth_mix));
    s.push_str(&format!("| Excite Amount | {:.1} | % |\n", p.excite_amount));
    s.push_str(&format!("| Excite Blend | {:.1} | % |\n", p.excite_blend));
    s.push_str(&format!("| Excite Freq | {:.0} | Hz |\n", p.excite_freq));
    s.push_str(&format!("| Tilt | {:.1} | dB |\n", p.tilt_gain));
    s.push_str(&format!("| Stereo Width | {:.1} | % |\n", p.stereo_width));
    s.push_str(&format!("| Pan | {:.2} | |\n", p.pan));
    s.push_str(&format!("| Output Gain | {:.1} | dB |\n", p.output_gain));
    s
}

/// Apply a preset in one shot via `ParamLens::automate` (begin+set+end),
/// normalizing every value through the param's own `.info.range` - several
/// of these (HPF/LPF, the 5 EQ freqs) are `Logarithmic`, so a hand-rolled
/// linear formula here would silently mis-map them.
fn apply_profile(lens: &ParamLens<MeridianParams>, params: &MeridianParams, profile: &MeridianProfile) {
    let f = |fp: &FloatParam, v: f32| fp.info.range.normalize(v as f64);
    let i = |ip: &IntParam, v: i32| ip.info.range.normalize(v as f64);
    lens.automate(K::HpfFreq, f(&params.hpf_freq, profile.hpf_freq));
    lens.automate(K::LpfFreq, f(&params.lpf_freq, profile.lpf_freq));
    lens.automate(K::CutSlope, i(&params.cut_slope, profile.cut_slope));
    lens.automate(K::BassGain, f(&params.bass_gain, profile.bass_gain));
    lens.automate(K::BassSlope, i(&params.bass_slope, profile.bass_slope));
    lens.automate(K::LoMidGain, f(&params.lo_mid_gain, profile.lo_mid_gain));
    lens.automate(K::LoMidSlope, i(&params.lo_mid_slope, profile.lo_mid_slope));
    lens.automate(K::MidGain, f(&params.mid_gain, profile.mid_gain));
    lens.automate(K::MidSlope, i(&params.mid_slope, profile.mid_slope));
    lens.automate(K::HighGain, f(&params.high_gain, profile.high_gain));
    lens.automate(K::HighSlope, i(&params.high_slope, profile.high_slope));
    lens.automate(K::ExciteGain, f(&params.excite_gain, profile.excite_gain));
    lens.automate(K::ExciteSlope, i(&params.excite_slope, profile.excite_slope));
    lens.automate(K::EqFreq1, f(&params.eq_freq_1, profile.eq_freq_1));
    lens.automate(K::EqFreq2, f(&params.eq_freq_2, profile.eq_freq_2));
    lens.automate(K::EqFreq3, f(&params.eq_freq_3, profile.eq_freq_3));
    lens.automate(K::EqFreq4, f(&params.eq_freq_4, profile.eq_freq_4));
    lens.automate(K::EqFreq5, f(&params.eq_freq_5, profile.eq_freq_5));
    lens.automate(K::TiltGain, f(&params.tilt_gain, profile.tilt_gain));
    lens.automate(K::WarmthDrive, f(&params.warmth_drive, profile.warmth_drive));
    lens.automate(K::WarmthMix, f(&params.warmth_mix, profile.warmth_mix));
    lens.automate(K::ExciteAmount, f(&params.excite_amount, profile.excite_amount));
    lens.automate(K::ExciteBlend, f(&params.excite_blend, profile.excite_blend));
    lens.automate(K::ExciteFreq, f(&params.excite_freq, profile.excite_freq));
    lens.automate(K::CompThreshold, f(&params.comp_threshold, profile.comp_threshold));
    lens.automate(K::CompMix, f(&params.comp_mix, profile.comp_mix));
    lens.automate(K::CompAttack, f(&params.comp_attack, profile.comp_attack));
    lens.automate(K::CompRelease, f(&params.comp_release, profile.comp_release));
    lens.automate(K::CompCharacter, f(&params.comp_character, profile.comp_character));
    lens.automate(K::CompMakeup, f(&params.comp_makeup, profile.comp_makeup));
    lens.automate(K::InflateEffect, f(&params.inflate_effect, profile.inflate_effect));
    lens.automate(K::InflateCurve, f(&params.inflate_curve, profile.inflate_curve));
    lens.automate(K::InflateBandSplit, if profile.inflate_band_split { 1.0 } else { 0.0 });
    lens.automate(K::InflateClip, if profile.inflate_clip { 1.0 } else { 0.0 });
    lens.automate(K::StereoWidth, f(&params.stereo_width, profile.stereo_width));
    lens.automate(K::Pan, f(&params.pan, profile.pan));
    lens.automate(K::OutputGain, f(&params.output_gain, profile.output_gain));
}

// ─── EQ Curve ────────────────────────────────────────────────────────────────

/// Compute the EQ transfer function (amber overlay curve) from current Biquad
/// filter parameters. Byte-identical math to the old iced `compute_eq_curve`,
/// reading slope/cut-slope directly from `params` (no separate mirrored state
/// needed - `params` already reflects the live host/GUI value every tick).
fn compute_eq_curve(params: &MeridianParams, sr: f32) -> Option<EqCurve> {
    if sr < 1.0 { return None; }
    const N: usize = 256;
    let slope_val = |s: i32| -> f32 { match s { 0 => 0.5, 1 => 1.0, _ => 2.0 } };
    let q_val = |s: i32| -> f32 { match s { 0 => 0.4, 1 => 0.7, _ => 1.5 } };

    let cut_slope_sel = params.cut_slope.value() as i32;
    let slope_sel = [
        params.bass_slope.value() as i32,
        params.lo_mid_slope.value() as i32,
        params.mid_slope.value() as i32,
        params.high_slope.value() as i32,
        params.excite_slope.value() as i32,
    ];

    let mut hpf = Biquad::new(); let mut lpf = Biquad::new();
    let mut hpf2 = Biquad::new(); let mut lpf2 = Biquad::new();
    let mut bass = Biquad::new(); let mut lo_mid = Biquad::new();
    let mut mid = Biquad::new(); let mut high = Biquad::new(); let mut excite = Biquad::new();
    let mut tilt_lo = Biquad::new(); let mut tilt_hi = Biquad::new();

    let hpf_f = params.hpf_freq.raw_target() as f32;
    let lpf_f = params.lpf_freq.raw_target() as f32;
    if cut_slope_sel >= 1 {
        const Q1: f32 = 0.541_196_1; const Q2: f32 = 1.306_563;
        hpf.set_butterworth_hp_q(hpf_f, Q1, sr); hpf2.set_butterworth_hp_q(hpf_f, Q2, sr);
        lpf.set_butterworth_lp_q(lpf_f, Q1, sr); lpf2.set_butterworth_lp_q(lpf_f, Q2, sr);
    } else {
        hpf.set_butterworth_hp(hpf_f, sr); lpf.set_butterworth_lp(lpf_f, sr);
        hpf2.set_identity(); lpf2.set_identity();
    }

    bass.set_low_shelf(params.eq_freq_1.raw_target() as f32, params.bass_gain.raw_target() as f32, slope_val(slope_sel[0]), sr);
    lo_mid.set_peaking_eq(params.eq_freq_2.raw_target() as f32, params.lo_mid_gain.raw_target() as f32, q_val(slope_sel[1]), sr);
    mid.set_peaking_eq(params.eq_freq_3.raw_target() as f32, params.mid_gain.raw_target() as f32, q_val(slope_sel[2]), sr);
    high.set_peaking_eq(params.eq_freq_4.raw_target() as f32, params.high_gain.raw_target() as f32, q_val(slope_sel[3]), sr);
    excite.set_high_shelf(params.eq_freq_5.raw_target() as f32, params.excite_gain.raw_target() as f32, slope_val(slope_sel[4]), sr);
    let tilt_db = params.tilt_gain.raw_target() as f32;
    tilt_lo.set_low_shelf(1000.0, tilt_db, 1.0, sr);
    tilt_hi.set_high_shelf(1000.0, -tilt_db, 1.0, sr);

    let points: Vec<(f32, f32)> = (0..N).map(|i| {
        let t = i as f32 / (N - 1) as f32;
        let freq = 20.0f32 * 1000.0f32.powf(t);
        let db = hpf.magnitude_db(freq, sr) + hpf2.magnitude_db(freq, sr)
            + lpf.magnitude_db(freq, sr) + lpf2.magnitude_db(freq, sr)
            + bass.magnitude_db(freq, sr) + lo_mid.magnitude_db(freq, sr)
            + mid.magnitude_db(freq, sr) + high.magnitude_db(freq, sr)
            + excite.magnitude_db(freq, sr) + tilt_lo.magnitude_db(freq, sr)
            + tilt_hi.magnitude_db(freq, sr);
        (t, db.clamp(-24.0, 24.0))
    }).collect();

    Some(EqCurve {
        points,
        min_db: -24.0,
        max_db: 24.0,
        line_color: vg_rgb(1.0, 0.55, 0.05),
        fill_alpha: 0.15,
    })
}

// ─── SNAP Helpers (framework-independent, unchanged from the iced version) ──

fn snap_filename(vault_path: &str) -> String {
    let dir = std::path::Path::new(vault_path);
    let mut max_n = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let s = e.file_name().to_string_lossy().into_owned();
            if let Some(inner) = s.strip_prefix("SNAPSHOT-").and_then(|r| r.strip_suffix(".md"))
                && let Ok(n) = inner.parse::<u32>() { max_n = max_n.max(n); }
        }
    }
    format!("SNAPSHOT-{:03}.md", max_n + 1)
}

fn snap_markdown(stereo: &[f32], mono: &[f32], delta: &[f32], band_levels: [f32; 5], corr: f32, pl: f32, pr: f32, sr: f32) -> String {
    let fft_sz = 2048.0;
    let freqs: &[f32] = &[20.0, 40.0, 80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10000.0, 16000.0, 20000.0];
    let tbl = |s: &[f32]| {
        freqs.iter().map(|&f| {
            let bin = ((f * fft_sz / sr) as usize).min(s.len().saturating_sub(1));
            format!("| {} | {:.1} |", if f >= 1000.0 { format!("{:.0}k", f / 1000.0) } else { format!("{:.0}", f) }, s[bin])
        }).collect::<Vec<_>>().join("\n")
    };
    format!(
        "---\nplugin: meridian\ntype: snapshot\n---\n\n# Meridian Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n\n\
        ## 5-Band\n| Band | Pegel |\n|------|-------|\n\
        | Sub | {b0:.1} dB |\n| Bass | {b1:.1} dB |\n| Mid | {b2:.1} dB |\n| Presence | {b3:.1} dB |\n| Air | {b4:.1} dB |\n",
        pl = pl, pr = pr, co = corr, st = tbl(stereo), mn = tbl(mono), dt = tbl(delta),
        b0 = band_levels[0], b1 = band_levels[1], b2 = band_levels[2], b3 = band_levels[3], b4 = band_levels[4],
    )
}

// ─── Telemetry (tick-frequency display state) ───────────────────────────────

#[derive(Clone)]
struct Telemetry {
    band_levels: [f32; 5],
    phase_correlation: f32,
    balance: f32,
    peak_l: f32,
    peak_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    peak_hold: f32,
    gain_reduction: f32,
    gr_peak_hold: f32,
    gr_history: Vec<f32>,
    auto_loud_measuring: bool,
    snap_active: bool,
    snap_blink_counter: u32,
}

/// Bookkeeping that persists across ticks but never touches the UI directly.
/// `Arc<Mutex<_>>` (not `Rc<RefCell<_>>`) - vizia's `on_press` closures
/// require `Send + Sync`, and every preset/vault-save closure below reads or
/// writes this, not just `tick()`.
struct TickAccum {
    presets: Vec<(String, PathBuf, MeridianProfile)>,
    vault_path: Option<String>,
    preset_refresh_counter: u32,
    gr_peak_hold_ticks: u32,
    /// params_gen is only bumped on discrete user actions (preset load/save,
    /// RESET ALL, Auto Loud, vault-path save) so slider/knob Bindings rebuild
    /// only when necessary. A periodic timer bump caused widgets to rebuild
    /// mid-drag, resetting their internal drag state and making interaction
    /// stutter.
    _reserved: u32,
}

#[allow(clippy::too_many_arguments)]
fn tick(shared: &SharedState, params: &MeridianParams, accum: &Arc<Mutex<TickAccum>>, telemetry: Signal<Telemetry>, params_gen: Signal<u32>) {
    let mut acc = accum.lock().unwrap();

    let mut band_levels = [0.0f32; 5];
    for b in 0..5 {
        band_levels[b] = shared.band_levels[b].load(Ordering::Acquire);
    }
    let phase_correlation = shared.phase_correlation.load(Ordering::Acquire);
    let balance = shared.balance.load(Ordering::Acquire);
    let peak_l = shared.output_peak_l.load(Ordering::Acquire);
    let peak_r = shared.output_peak_r.load(Ordering::Acquire);
    let peak_hold_l = shared.peak_hold_l.load(Ordering::Acquire);
    let peak_hold_r = shared.peak_hold_r.load(Ordering::Acquire);
    let peak_hold = shared.peak_hold.load(Ordering::Acquire);
    let gain_reduction = shared.gain_reduction.load(Ordering::Acquire);

    let prev_gr_history = telemetry.get().gr_history;
    let prev_gr_peak_hold = telemetry.get().gr_peak_hold;
    let mut gr_history = prev_gr_history;
    gr_history.push(gain_reduction);
    if gr_history.len() > 90 { gr_history.remove(0); }
    let gr_peak_hold = if gain_reduction > prev_gr_peak_hold {
        acc.gr_peak_hold_ticks = 90;
        gain_reduction
    } else if acc.gr_peak_hold_ticks > 0 {
        acc.gr_peak_hold_ticks -= 1;
        prev_gr_peak_hold
    } else {
        (prev_gr_peak_hold - 0.15).max(gain_reduction).max(0.0)
    };

    let measuring = shared.auto_loud_measuring.load(Ordering::Acquire);
    let mut auto_loud_applied = false;
    if !measuring {
        let offset = shared.auto_loud_gain_offset.load(Ordering::Acquire);
        if offset.abs() > 0.05 {
            let current = params.output_gain.raw_target() as f32;
            let new_val = (current + offset).clamp(-12.0, 12.0);
            params.output_gain.set_value(new_val as f64);
            shared.auto_loud_gain_offset.store(0.0, Ordering::Release);
            auto_loud_applied = true;
        }
    }

    acc.preset_refresh_counter = acc.preset_refresh_counter.wrapping_add(1);
    let refresh_presets = acc.preset_refresh_counter.is_multiple_of(150);
    if refresh_presets {
        acc.presets = list_meridian_presets(acc.vault_path.as_deref());
    }

    let snap_now = shared.snap_active.load(Ordering::Acquire);
    let vault_path = acc.vault_path.clone();
    let sr = shared.sample_rate.load(Ordering::Acquire);
    // Drop the lock before telemetry.update()/params_gen.update(): vizia_reactive
    // runs pending effects synchronously inside Signal::update, and the SNAP
    // button's Memo (build_sidebar) reads accum.lock() itself - held across
    // either call, it self-deadlocks the UI thread on this same non-reentrant
    // Mutex.
    drop(acc);

    // Bump params_gen only on discrete events. Sliders/knobs live inside
    // `Binding::new(cx, params_gen, ...)` so they rebuild and re-read fresh
    // values on ResetAll / SelectPreset / SavePreset / vault-path save /
    // Auto Loud. A periodic bump here made them rebuild during a drag and
    // stutter.
    if auto_loud_applied || refresh_presets {
        params_gen.update(|g| *g = g.wrapping_add(1));
    }

    telemetry.update(move |t| {
        t.band_levels = band_levels;
        t.phase_correlation = phase_correlation;
        t.balance = balance;
        t.peak_l = peak_l;
        t.peak_r = peak_r;
        t.peak_hold_l = peak_hold_l;
        t.peak_hold_r = peak_hold_r;
        t.peak_hold = peak_hold;
        t.gain_reduction = gain_reduction;
        t.gr_peak_hold = gr_peak_hold;
        t.gr_history = gr_history;
        t.auto_loud_measuring = measuring;

        let was_active = t.snap_active;
        t.snap_active = snap_now;
        if snap_now {
            t.snap_blink_counter = t.snap_blink_counter.wrapping_add(1);
        } else if was_active {
            t.snap_blink_counter = 0;
            if let Some(vp) = vault_path
                && !vp.is_empty() {
                    let stereo = shared.snap_stereo_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let mono = shared.snap_mono_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let delta = shared.snap_delta_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let md = snap_markdown(&stereo, &mono, &delta, band_levels, phase_correlation, peak_l, peak_r, sr);
                    let fname = snap_filename(&vp);
                    let _ = std::fs::write(std::path::Path::new(&vp).join(&fname), &md);
                }
        }
    });
}

// ─── Ticker (drives `tick()` without vizia_core's buggy timer API) ──────────

struct Ticker {
    shared: Arc<SharedState>,
    params: Arc<MeridianParams>,
    accum: Arc<Mutex<TickAccum>>,
    telemetry: Signal<Telemetry>,
    params_gen: Signal<u32>,
    last_tick: RefCell<Instant>,
}

impl Ticker {
    fn new(
        cx: &mut Context,
        shared: Arc<SharedState>,
        params: Arc<MeridianParams>,
        accum: Arc<Mutex<TickAccum>>,
        telemetry: Signal<Telemetry>,
        params_gen: Signal<u32>,
    ) -> Handle<'_, Self> {
        Self { shared, params, accum, telemetry, params_gen, last_tick: RefCell::new(Instant::now()) }.build(cx, |_| {})
    }
}

const TICK_INTERVAL: Duration = Duration::from_millis(33);

impl View for Ticker {
    fn element(&self) -> Option<&'static str> {
        Some("ticker")
    }

    fn draw(&self, cx: &mut DrawContext, _canvas: &vg::Canvas) {
        let now = Instant::now();
        let due = {
            let mut last = self.last_tick.borrow_mut();
            if now.duration_since(*last) >= TICK_INTERVAL {
                *last = now;
                true
            } else {
                false
            }
        };
        if due {
            tick(&self.shared, &self.params, &self.accum, self.telemetry, self.params_gen);
        }
        cx.needs_redraw();
    }
}

// ─── Small widget helpers ────────────────────────────────────────────────────

/// Fixed-label toggle button (MONO/DELTA/BYPASS): amber when the bound bool
/// param is active, dark otherwise. Wrapped in a `Binding` on the param's own
/// value signal - fires only on that param's own changes, never on the 33ms
/// tick, so it never hits the rebuild-drops-clicks issue `Memo` is needed for
/// on the tick-driven panels.
fn styled_toggle(cx: &mut Context, lens: ParamLens<MeridianParams>, id: K, label: &'static str) {
    let sig = lens.value_signal(id);
    Binding::new(cx, sig, move |cx| {
        let active = lens.get(id) > 0.5;
        let lens = lens.clone();
        shared_ui::toggle_button(cx, label, active, move |_cx| {
            let now = lens.get(id) <= 0.5;
            let norm = if now { 1.0 } else { 0.0 };
            lens.automate(id, norm);
            // automate() only writes the backend param store - `sig` is
            // a separate Signal seeded once by value_signal() and never
            // otherwise updated, so without this push this Binding would
            // never refire and the button would never repaint.
            sig.set(norm as f32);
        });
    });
}

/// Discrete slope selector (2 or 3 buttons, A/B[/C]) - `Binding`ed to the
/// param's own value signal for the same reason as `styled_toggle`.
fn slope_selector(cx: &mut Context, lens: ParamLens<MeridianParams>, params: Arc<MeridianParams>, field: IntField, id: K, steps: i32) {
    let sig = lens.value_signal(id);
    let step_norms: Vec<f64> = (0..steps).map(|s| field(&params).info.range.normalize(s as f64)).collect();
    let params_cur = params.clone();
    Binding::new(cx, sig, move |cx| {
        let current = field(&params_cur).value() as i32;
        HStack::new(cx, |cx| {
            for s in 0..steps {
                let is_sel = current == s;
                let lens = lens.clone();
                let norm = step_norms[s as usize];
                shared_ui::toggle_button_small(cx, slope_char(s), is_sel, move |_cx| {
                    lens.automate(id, norm);
                    sig.set(norm as f32);
                });
            }
        })
        .horizontal_gap(Pixels(4.0))
        .height(Auto)
        .width(Auto);
    });
}

// ─── UI ──────────────────────────────────────────────────────────────────────

pub fn build(cx: &mut Context, lens: ParamLens<MeridianParams>, shared: Arc<SharedState>, params: Arc<MeridianParams>) {
    let config = shared_analysis::load_config("Meridian");
    let presets = list_meridian_presets(config.vault_path.as_deref());
    let selected_preset_idx = None::<usize>;

    let telemetry = Signal::new(Telemetry {
        band_levels: [-90.0; 5],
        phase_correlation: 1.0,
        balance: 0.0,
        peak_l: -90.0,
        peak_r: -90.0,
        peak_hold_l: -90.0,
        peak_hold_r: -90.0,
        peak_hold: -90.0,
        gain_reduction: 0.0,
        gr_peak_hold: 0.0,
        gr_history: vec![0.0; 90],
        auto_loud_measuring: false,
        snap_active: false,
        snap_blink_counter: 0,
    });

    let show_setup = Signal::new(false);
    let vault_path_input = Signal::new(config.vault_path.clone().unwrap_or_default());
    let preset_name_input = Signal::new(String::new());
    let selected_preset = Signal::new(selected_preset_idx);
    let params_gen = Signal::new(0u32);

    let accum = Arc::new(Mutex::new(TickAccum {
        presets,
        vault_path: config.vault_path,
        preset_refresh_counter: 0,
        gr_peak_hold_ticks: 0,
        _reserved: 0,
    }));

    Ticker::new(cx, shared.clone(), params.clone(), accum.clone(), telemetry, params_gen).width(Pixels(1.0)).height(Pixels(1.0));

    // ── HEADER ──
    let lens_header = lens.clone();
    HStack::new(cx, move |cx| {
        let lens = lens_header;
        HStack::new(cx, |cx| {
            Label::new(cx, "LX").font_size(20.0).color(rgb(1.0, 0.45, 0.1));
            Label::new(cx, "AUDIOLABS").font_size(20.0).color(Color::white());
            Element::new(cx).width(Pixels(14.0));
            Element::new(cx).width(Pixels(1.0)).height(Pixels(28.0)).background_color(col(0.18, 0.22, 0.22, 1.0));
            Element::new(cx).width(Pixels(14.0));
            VStack::new(cx, |cx| {
                Label::new(cx, "MERIDIAN").font_size(13.0).color(rgb(1.0, 0.65, 0.3));
                Label::new(cx, format!("v{VERSION}")).font_size(10.0).color(col(0.5, 0.5, 0.5, 1.0));
            })
            .width(Auto)
            .height(Auto)
            .vertical_gap(Pixels(2.0));
        })
        .width(Auto)
        .height(Auto)
        .horizontal_gap(Pixels(6.0))
        .alignment(Alignment::Center);

        Element::new(cx).width(Stretch(1.0));

        HStack::new(cx, move |cx| {
            styled_toggle(cx, lens.clone(), K::MonoActive, "MONO");
            styled_toggle(cx, lens.clone(), K::DeltaActive, "DELTA");
            styled_toggle(cx, lens.clone(), K::BypassActive, "BYPASS");
        })
        .width(Auto)
        .height(Auto)
        .horizontal_gap(Pixels(4.0))
        .alignment(Alignment::Center);
    })
    .width(Stretch(1.0))
    .height(Pixels(50.0))
    .padding(Pixels(10.0))
    .alignment(Alignment::Center)
    .background_color(rgb(0.08, 0.08, 0.08));

    let lens_body = lens.clone();
    let shared_body = shared.clone();
    let accum_body = accum.clone();
    let params_body = params.clone();
    HStack::new(cx, move |cx| {
        let lens_middle = lens_body.clone();
        let params_middle = params_body.clone();
        let shared_middle = shared_body.clone();
        let accum_middle = accum_body.clone();
        build_sidebar(cx, accum_body.clone(), telemetry, selected_preset, preset_name_input, show_setup, shared_body.clone(), lens_body.clone(), params_body.clone(), params_gen);

        VStack::new(cx, move |cx| {
            Binding::new(cx, show_setup, move |cx| {
                if show_setup.get() {
                    build_setup_form(cx, vault_path_input, show_setup, accum_middle.clone(), params_gen);
                } else {
                    build_main_panel(cx, telemetry, lens_middle.clone(), params_middle.clone(), shared_middle.clone(), params_gen);
                }
            });
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0))
        .background_color(rgb(0.06, 0.06, 0.06));

        build_right_sidebar(cx, telemetry, lens_body.clone(), params_body.clone(), shared_body.clone(), params_gen);
    })
    .width(Stretch(1.0))
    .height(Stretch(1.0));

    build_footer(cx, telemetry, lens, params, shared, accum, params_gen);
}

// ─── Sidebar (VAULT PRESETS) ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_sidebar(
    cx: &mut Context,
    accum: Arc<Mutex<TickAccum>>,
    telemetry: Signal<Telemetry>,
    selected_preset: Signal<Option<usize>>,
    preset_name_input: Signal<String>,
    show_setup: Signal<bool>,
    shared: Arc<SharedState>,
    lens: ParamLens<MeridianParams>,
    params: Arc<MeridianParams>,
    params_gen: Signal<u32>,
) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "VAULT PRESETS").font_size(14.0).color(Color::white());

        Textbox::new(cx, preset_name_input)
            .placeholder("Preset Name...")
            .on_edit(move |_cx, text| preset_name_input.set(text))
            .width(Stretch(1.0));

        let accum_hs = accum.clone();
        let shared_hs = shared.clone();
        let lens_hs = lens.clone();
        let params_hs = params.clone();
        HStack::new(cx, move |cx| {
            let accum_label = accum_hs.clone();
            let shared_press = shared_hs.clone();
            let accum_press = accum_hs.clone();
            Button::new(cx, move |cx| {
                Label::new(
                    cx,
                    Memo::new(move |_| {
                        let t = telemetry.get();
                        let no_vault = accum_label.lock().unwrap().vault_path.as_ref().is_none_or(|v| v.is_empty());
                        if t.snap_active { "ANALYZE...".to_string() } else if no_vault { "SET VAULT".to_string() } else { "SNAP".to_string() }
                    }),
                )
                .font_size(12.0)
                .color(Memo::new(move |_| {
                    let blink = telemetry.get().snap_active && (telemetry.get().snap_blink_counter / 8).is_multiple_of(2);
                    if blink { rgb(1.0, 0.85, 0.3) } else { rgb(1.0, 0.55, 0.1) }
                }))
            })
            .on_press(move |_cx| {
                let no_vault = accum_press.lock().unwrap().vault_path.as_ref().is_none_or(|v| v.is_empty());
                if no_vault {
                    show_setup.set(true);
                } else {
                    shared_press.snap_active.store(true, Ordering::Release);
                    shared_press.snap_phase.store(1, Ordering::Release);
                }
            })
            .width(Stretch(1.0))
            .height(Pixels(34.0))
            .background_color(Memo::new(move |_| {
                let blink = telemetry.get().snap_active && (telemetry.get().snap_blink_counter / 8).is_multiple_of(2);
                if blink { col(0.55, 0.38, 0.05, 1.0) } else { col(0.18, 0.18, 0.18, 1.0) }
            }));

            let accum_save = accum_hs.clone();
            let lens_save = lens_hs.clone();
            let params_save = params_hs.clone();
            Button::new(cx, |cx| Label::new(cx, "SAVE").font_size(12.0))
                .on_press(move |_cx| {
                    do_save_preset(&accum_save, preset_name_input, &lens_save, &params_save, params_gen);
                })
                .width(Stretch(1.0))
                .height(Pixels(34.0))
                .background_color(col(0.18, 0.18, 0.18, 1.0));
        })
        .width(Stretch(1.0))
        .height(Auto)
        .horizontal_gap(Pixels(4.0));

        Button::new(cx, |cx| Label::new(cx, "VAULT SETUP").font_size(12.0))
            .on_press(move |_cx| show_setup.set(!show_setup.get()))
            .width(Stretch(1.0))
            .height(Pixels(34.0))
            .background_color(col(0.18, 0.18, 0.18, 1.0));

        let accum_list = accum.clone();
        let lens_list = lens.clone();
        let params_list = params.clone();
        Binding::new(cx, params_gen, move |cx| {
            let acc = accum_list.lock().unwrap();
            let no_vault = acc.vault_path.as_ref().is_none_or(|v| v.is_empty());
            if no_vault {
                Label::new(cx, "Set Vault-path first").font_size(9.0).color(col(1.0, 0.75, 0.2, 1.0));
            }
            let sel = selected_preset.get();
            let user: Vec<(usize, String)> = acc.presets.iter().enumerate().map(|(i, (n, _, _))| (i, n.clone())).collect();
            drop(acc);
            let accum_scroll = accum_list.clone();
            let lens_scroll = lens_list.clone();
            let params_scroll = params_list.clone();
            ScrollView::new(cx, move |cx| {
                if !user.is_empty() {
                    Label::new(cx, "── Vault Presets ──").font_size(11.0).color(rgb(1.0, 0.55, 0.15));
                }
                for (idx, name) in user {
                    preset_list_item(cx, idx, name, sel, selected_preset, accum_scroll.clone(), lens_scroll.clone(), params_scroll.clone(), params_gen);
                }
            })
            .height(Stretch(1.0));
        });
    })
    .width(Pixels(180.0))
    .height(Stretch(1.0))
    .padding(Pixels(10.0))
    .vertical_gap(Pixels(10.0))
    .background_color(rgb(0.1, 0.1, 0.1));
}

#[allow(clippy::too_many_arguments)]
fn preset_list_item(
    cx: &mut Context,
    idx: usize,
    name: String,
    selected: Option<usize>,
    selected_preset: Signal<Option<usize>>,
    accum: Arc<Mutex<TickAccum>>,
    lens: ParamLens<MeridianParams>,
    params: Arc<MeridianParams>,
    params_gen: Signal<u32>,
) {
    let is_sel = selected == Some(idx);
    Button::new(cx, move |cx| Label::new(cx, format!("> {name}")).font_size(13.0))
        .alignment(Alignment::Left)
        .on_press(move |_cx| {
            let acc = accum.lock().unwrap();
            let Some((_, _, prof)) = acc.presets.get(idx).cloned() else { return };
            drop(acc);
            selected_preset.set(Some(idx));
            apply_profile(&lens, &params, &prof);
            params_gen.update(|g| *g = g.wrapping_add(1));
        })
        .width(Stretch(1.0))
        .background_color(if is_sel { col(0.18, 0.14, 0.08, 1.0) } else { Color::transparent() })
        .color(if is_sel { rgb(1.0, 0.45, 0.1) } else { col(0.9, 0.9, 0.9, 1.0) });
}

fn do_save_preset(accum: &Arc<Mutex<TickAccum>>, preset_name_input: Signal<String>, lens: &ParamLens<MeridianParams>, params: &MeridianParams, params_gen: Signal<u32>) {
    let name_input = preset_name_input.get();
    let mut acc = accum.lock().unwrap();
    let name = if name_input.trim().is_empty() {
        format!("User Preset {}", acc.presets.len() + 1)
    } else {
        name_input.trim().to_string()
    };

    let prof = MeridianProfile {
        name: name.clone(),
        hpf_freq: lens.get_plain(K::HpfFreq),
        lpf_freq: lens.get_plain(K::LpfFreq),
        cut_slope: params.cut_slope.value() as i32,
        bass_gain: lens.get_plain(K::BassGain),
        bass_slope: params.bass_slope.value() as i32,
        lo_mid_gain: lens.get_plain(K::LoMidGain),
        lo_mid_slope: params.lo_mid_slope.value() as i32,
        mid_gain: lens.get_plain(K::MidGain),
        mid_slope: params.mid_slope.value() as i32,
        high_gain: lens.get_plain(K::HighGain),
        high_slope: params.high_slope.value() as i32,
        excite_gain: lens.get_plain(K::ExciteGain),
        excite_slope: params.excite_slope.value() as i32,
        eq_freq_1: lens.get_plain(K::EqFreq1),
        eq_freq_2: lens.get_plain(K::EqFreq2),
        eq_freq_3: lens.get_plain(K::EqFreq3),
        eq_freq_4: lens.get_plain(K::EqFreq4),
        eq_freq_5: lens.get_plain(K::EqFreq5),
        tilt_gain: lens.get_plain(K::TiltGain),
        warmth_drive: lens.get_plain(K::WarmthDrive),
        warmth_mix: lens.get_plain(K::WarmthMix),
        excite_amount: lens.get_plain(K::ExciteAmount),
        excite_blend: lens.get_plain(K::ExciteBlend),
        excite_freq: lens.get_plain(K::ExciteFreq),
        comp_threshold: lens.get_plain(K::CompThreshold),
        comp_mix: lens.get_plain(K::CompMix),
        comp_attack: lens.get_plain(K::CompAttack),
        comp_release: lens.get_plain(K::CompRelease),
        comp_character: lens.get_plain(K::CompCharacter),
        comp_makeup: lens.get_plain(K::CompMakeup),
        inflate_effect: lens.get_plain(K::InflateEffect),
        inflate_curve: lens.get_plain(K::InflateCurve),
        inflate_band_split: params.inflate_band_split.value(),
        inflate_clip: params.inflate_clip.value(),
        stereo_width: lens.get_plain(K::StereoWidth),
        pan: lens.get_plain(K::Pan),
        output_gain: lens.get_plain(K::OutputGain),
    };

    let dir = match &acc.vault_path {
        Some(vp) if !vp.is_empty() => PathBuf::from(vp),
        _ => shared_analysis::get_plugin_dir("Meridian").join("presets"),
    };
    let _ = std::fs::create_dir_all(&dir);
    let safe_name = name.replace(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_', "");
    let file_path = dir.join(format!("{safe_name}.md"));
    let md = export_meridian_markdown(&prof);
    if std::fs::write(&file_path, md).is_ok() {
        acc.presets = list_meridian_presets(acc.vault_path.as_deref());
        // Drop before params_gen.update(): the preset-list Binding it
        // triggers locks `accum` itself - held across that call, it
        // self-deadlocks (same non-reentrant-Mutex issue as tick()'s
        // telemetry.update() call, see comment there).
        drop(acc);
        preset_name_input.set(String::new());
        params_gen.update(|g| *g = g.wrapping_add(1));
    }
}

// ─── Setup form ──────────────────────────────────────────────────────────────

fn build_setup_form(cx: &mut Context, vault_path_input: Signal<String>, show_setup: Signal<bool>, accum: Arc<Mutex<TickAccum>>, params_gen: Signal<u32>) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "LX AUDIOLABS - SETUP").font_size(18.0).color(Color::white());
        Label::new(cx, "Configure your Vault path for Meridian:").font_size(12.0).color(Color::white());
        Textbox::new(cx, vault_path_input)
            .placeholder("Enter Vault absolute path...")
            .on_edit(move |_cx, text| vault_path_input.set(text))
            .width(Stretch(1.0));
        HStack::new(cx, move |cx| {
            Button::new(cx, |cx| Label::new(cx, "SAVE"))
                .on_press(move |_cx| {
                    let vp = vault_path_input.get().trim().to_string();
                    let new_path = if vp.is_empty() { None } else { Some(vp.clone()) };
                    let cfg = shared_analysis::PluginConfig { vault_path: new_path.clone(), ..Default::default() };
                    if shared_analysis::save_config("Meridian", &cfg).is_ok() {
                        let mut acc = accum.lock().unwrap();
                        acc.vault_path = new_path;
                        acc.presets = list_meridian_presets(acc.vault_path.as_deref());
                        drop(acc);
                        params_gen.update(|g| *g = g.wrapping_add(1));
                        show_setup.set(false);
                    }
                })
                .background_color(col(0.15, 0.15, 0.15, 1.0));
            Button::new(cx, |cx| Label::new(cx, "CANCEL")).on_press(move |_cx| show_setup.set(false)).background_color(col(0.15, 0.15, 0.15, 1.0));
        })
        .horizontal_gap(Pixels(10.0))
        .height(Auto);
    })
    .width(Pixels(600.0))
    .height(Auto)
    .padding(Pixels(20.0))
    .vertical_gap(Pixels(15.0))
    .background_color(col(0.15, 0.15, 0.15, 1.0))
    .border_color(col(0.3, 0.3, 0.3, 1.0))
    .border_width(Pixels(1.0))
    .corner_radius(Pixels(4.0));
}

// ─── Main panel (top strip + spectrum/EQ-curve + 5-band EQ row) ─────────────

fn build_main_panel(cx: &mut Context, telemetry: Signal<Telemetry>, lens: ParamLens<MeridianParams>, params: Arc<MeridianParams>, shared: Arc<SharedState>, params_gen: Signal<u32>) {
    // ── Top strip: Filter / Warmth / Exciter / Tilt ──
    // Wrapped in `Binding(params_gen)` so RESET / preset-load refreshes these
    // drag widgets - they snapshot `lens.get_plain()` once at construction and
    // otherwise never see a value changed from outside their own gesture.
    Binding::new(cx, params_gen, {
    let lens = lens.clone();
    let params = params.clone();
    move |cx| {
    HStack::new(cx, {
        let lens = lens.clone();
        let params = params.clone();
        move |cx| {
            let strip_label = |cx: &mut Context, t: &'static str| {
                Label::new(cx, t).font_size(10.0).color(rgb(1.0, 0.55, 0.15));
            };

            VStack::new(cx, {
                let lens = lens.clone();
                let params = params.clone();
                move |cx| {
                    strip_label(cx, "FILTER");
                    HStack::new(cx, move |cx| {
                        let hpf = lens.get_plain(K::HpfFreq);
                        let hpf_display = Signal::new(hpf);
                        let lens_hpf = lens.clone();
                        let params_hpf = params.clone();
                        VStack::new(cx, move |cx| {
                            KnobView::new(cx, ((hpf.ln() - 2.0f32.ln()) / (2000.0f32.ln() - 2.0f32.ln())).clamp(0.0, 1.0), 0.0, 2.0, 2000.0, false, move |_cx, g| match g {
                                Gesture::Start => lens_hpf.begin_edit(K::HpfFreq),
                                Gesture::Change(v) => {
                                    lens_hpf.set(K::HpfFreq, params_hpf.hpf_freq.info.range.normalize(v as f64));
                                    hpf_display.set(v);
                                }
                                Gesture::End => lens_hpf.end_edit(K::HpfFreq),
                            })
                            .width(Pixels(40.0))
                            .height(Pixels(40.0));
                            Label::new(cx, Memo::new(move |_| short_freq(hpf_display.get()))).font_size(9.0).color(rgb(1.0, 0.65, 0.3));
                            Label::new(cx, "LOW CUT").font_size(9.0).color(col(0.75, 0.75, 0.75, 1.0));
                        })
                        .alignment(Alignment::Center)
                        .width(Pixels(40.0));

                        slope_selector(cx, lens.clone(), params.clone(), |p| &p.cut_slope, K::CutSlope, 2);

                        let lpf = lens.get_plain(K::LpfFreq);
                        let lpf_display = Signal::new(lpf);
                        let lens_lpf = lens.clone();
                        let params_lpf = params.clone();
                        VStack::new(cx, move |cx| {
                            KnobView::new(cx, ((lpf.ln() - 200.0f32.ln()) / (35000.0f32.ln() - 200.0f32.ln())).clamp(0.0, 1.0), 1.0, 200.0, 35000.0, false, move |_cx, g| match g {
                                Gesture::Start => lens_lpf.begin_edit(K::LpfFreq),
                                Gesture::Change(v) => {
                                    lens_lpf.set(K::LpfFreq, params_lpf.lpf_freq.info.range.normalize(v as f64));
                                    lpf_display.set(v);
                                }
                                Gesture::End => lens_lpf.end_edit(K::LpfFreq),
                            })
                            .width(Pixels(40.0))
                            .height(Pixels(40.0));
                            Label::new(cx, Memo::new(move |_| short_freq(lpf_display.get()))).font_size(9.0).color(rgb(1.0, 0.65, 0.3));
                            Label::new(cx, "HIGH CUT").font_size(9.0).color(col(0.75, 0.75, 0.75, 1.0));
                        })
                        .alignment(Alignment::Center)
                        .width(Pixels(40.0));
                    })
                    .horizontal_gap(Pixels(15.0))
                    .alignment(Alignment::Center)
                    .height(Auto);
                }
            })
            .vertical_gap(Pixels(4.0))
            .alignment(Alignment::Center)
            .width(Pixels(185.0));

            vsep(cx);

            linear_knob_group(cx, &lens, &params, "WARMTH", &[
                ("DRIVE", K::WarmthDrive, (|p: &MeridianParams| &p.warmth_drive) as FloatField, 0.0, 12.0),
                ("W/MIX", K::WarmthMix, (|p: &MeridianParams| &p.warmth_mix) as FloatField, 0.0, 100.0),
            ])
            .width(Pixels(110.0));

            vsep(cx);

            linear_knob_group(cx, &lens, &params, "EXCITER", &[
                ("AMT", K::ExciteAmount, (|p: &MeridianParams| &p.excite_amount) as FloatField, 0.0, 30.0),
                ("BLEND", K::ExciteBlend, (|p: &MeridianParams| &p.excite_blend) as FloatField, 0.0, 100.0),
                ("FREQ", K::ExciteFreq, (|p: &MeridianParams| &p.excite_freq) as FloatField, 6000.0, 12000.0),
            ])
            .width(Pixels(160.0));

            vsep(cx);

            VStack::new(cx, {
                let lens = lens.clone();
                let params = params.clone();
                move |cx| {
                    strip_label(cx, "TILT EQ");
                    bipolar_knob(cx, &lens, &params, K::TiltGain, |p| &p.tilt_gain, -1.5, 1.5, 0.0, "SLOPE");
                }
            })
            .vertical_gap(Pixels(4.0))
            .alignment(Alignment::Center)
            .width(Pixels(70.0));
        }
    })
    .width(Stretch(1.0))
    .height(Pixels(100.0))
    .horizontal_gap(Pixels(15.0))
    .alignment(Alignment::Center)
    .padding(Pixels(5.0));
    }});

    // ── Spectrum + EQ curve overlay - passive display, rebuilt every tick ──
    let params_spec = params.clone();
    Binding::new(cx, telemetry, move |cx| {
        let t = telemetry.get();
        let spectrum = shared.spectrum_avg.lock().map(|g| g.clone()).unwrap_or_default();
        let sr = shared.sample_rate.load(Ordering::Acquire);
        let eq_curve = compute_eq_curve(&params_spec, sr);
        SpectrumView::new(cx, SpectrumView {
            curves: vec![SpectrumCurve {
                spectrum,
                color: vg_rgb(0.1, 0.9, 0.7),
                fill_alpha: 0.18,
                line_alpha: 0.85,
                line_width: 1.6,
            }],
            config: SpectrumConfig { sample_rate: sr, ..Default::default() },
            resonance_peaks: Vec::new(),
            masking: Vec::new(),
            eq_curve,
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0));
        let _ = t;
    });

    // ── 5-band EQ row - drag widgets, rebuilt only on `params_gen` (RESET /
    // preset load), same reasoning as the top strip above.
    Binding::new(cx, params_gen, {
        let lens = lens.clone();
        let params = params.clone();
        move |cx| {
    HStack::new(cx, move |cx| {
        for label in HZ_LABELS {
            Label::new(cx, label).font_size(10.0).color(rgb(1.0, 0.55, 0.15)).width(Stretch(1.0)).alignment(Alignment::Center);
        }
    })
    .width(Stretch(1.0))
    .height(Pixels(15.0));

    let lens = lens.clone();
    let params = params.clone();
    HStack::new(cx, move |cx| {
        for b in 0..5 {
            let gain_field = GAIN_FIELDS[b];
            let freq_field = FREQ_FIELDS[b];
            let slope_field = SLOPE_FIELDS[b];
            let (fmin, fmax, fdef) = FREQ_RANGES[b];
            let gain_id = GAIN_IDS[b];
            let freq_id = FREQ_IDS[b];

            let gain = lens.get_plain(gain_id);
            let freq = lens.get_plain(freq_id);
            let gain_display = Signal::new(gain);
            let freq_display = Signal::new(freq);

            let lens_gain = lens.clone();
            let lens_freq = lens.clone();
            let params_gain = params.clone();
            let params_freq = params.clone();
            let lens_slope = lens.clone();
            let params_slope = params.clone();

            VStack::new(cx, move |cx| {
                Label::new(cx, BAND_NAMES[b]).font_size(11.0).color(col(0.85, 0.85, 0.85, 1.0));

                HSliderView::new(cx, -12.0, 12.0, gain, 0.0, move |_cx, g| match g {
                    Gesture::Start => lens_gain.begin_edit(gain_id),
                    Gesture::Change(v) => {
                        lens_gain.set(gain_id, gain_field(&params_gain).info.range.normalize(v as f64));
                        gain_display.set(v);
                    }
                    Gesture::End => lens_gain.end_edit(gain_id),
                })
                .width(Stretch(1.0))
                .height(Pixels(20.0));
                Label::new(cx, Memo::new(move |_| format!("{:.1} dB", gain_display.get()))).font_size(11.0).color(col(0.8, 0.8, 0.8, 1.0));

                HSliderView::new(cx, fmin, fmax, freq, fdef, move |_cx, g| match g {
                    Gesture::Start => lens_freq.begin_edit(freq_id),
                    Gesture::Change(v) => {
                        lens_freq.set(freq_id, freq_field(&params_freq).info.range.normalize(v as f64));
                        freq_display.set(v);
                    }
                    Gesture::End => lens_freq.end_edit(freq_id),
                })
                .width(Stretch(1.0))
                .height(Pixels(20.0));
                Label::new(cx, Memo::new(move |_| short_freq(freq_display.get()))).font_size(10.0).color(rgb(0.7, 0.85, 1.0));

                Label::new(cx, if BAND_IS_SHELF[b] { "Shelf Slope" } else { "Filter Q" }).font_size(10.0).color(col(0.55, 0.55, 0.55, 1.0));
                slope_selector(cx, lens_slope.clone(), params_slope.clone(), slope_field, SLOPE_IDS[b], 3);
            })
            .vertical_gap(Pixels(4.0))
            .alignment(Alignment::Center)
            .width(Stretch(1.0));
        }
    })
    .width(Stretch(1.0))
    .height(Auto)
    .horizontal_gap(Pixels(10.0))
    .padding(Pixels(10.0));
    }});
}

fn vsep(cx: &mut Context) {
    Element::new(cx).width(Pixels(1.0)).height(Stretch(1.0)).background_color(col(1.0, 1.0, 1.0, 0.08));
}

/// A labelled group of plain (unipolar) knobs sharing one strip label -
/// FILTER/WARMTH/EXCITER's shape in the top strip.
fn linear_knob_group<'a>(cx: &'a mut Context, lens: &ParamLens<MeridianParams>, params: &Arc<MeridianParams>, title: &'static str, knobs: &[(&'static str, K, FloatField, f32, f32)]) -> Handle<'a, impl View> {
    let lens = lens.clone();
    let params = params.clone();
    let knobs: Vec<(&'static str, K, FloatField, f32, f32)> = knobs.to_vec();
    VStack::new(cx, move |cx| {
        Label::new(cx, title).font_size(10.0).color(rgb(1.0, 0.55, 0.15));
        HStack::new(cx, move |cx| {
            for (label, id, field, min, max) in knobs.clone() {
                let lens = lens.clone();
                let params = params.clone();
                let value = lens.get_plain(id);
                let display = Signal::new(value);
                let norm = ((value - min) / (max - min)).clamp(0.0, 1.0);
                VStack::new(cx, move |cx| {
                    KnobView::new(cx, norm, 0.0, min, max, false, move |_cx, g| match g {
                        Gesture::Start => lens.begin_edit(id),
                        Gesture::Change(v) => {
                            lens.set(id, field(&params).info.range.normalize(v as f64));
                            display.set(v);
                        }
                        Gesture::End => lens.end_edit(id),
                    })
                    .width(Pixels(40.0))
                    .height(Pixels(40.0));
                    Label::new(cx, Memo::new(move |_| format_knob_value(display.get(), max))).font_size(9.0).color(rgb(1.0, 0.65, 0.3));
                    Label::new(cx, label).font_size(9.0).color(col(0.75, 0.75, 0.75, 1.0));
                })
                .alignment(Alignment::Center)
                .width(Pixels(40.0));
            }
        })
        .horizontal_gap(Pixels(15.0))
        .alignment(Alignment::Center)
        .height(Auto);
    })
    .vertical_gap(Pixels(4.0))
    .alignment(Alignment::Center)
    .width(Auto)
}

/// A single unipolar knob (Inflate Effect) - same shape as one
/// `linear_knob_group` entry, without the group label wrapper.
fn plain_knob(cx: &mut Context, lens: &ParamLens<MeridianParams>, params: &Arc<MeridianParams>, id: K, field: FloatField, min: f32, max: f32, label: &'static str) {
    let lens = lens.clone();
    let params = params.clone();
    let value = lens.get_plain(id);
    let display = Signal::new(value);
    let norm = ((value - min) / (max - min)).clamp(0.0, 1.0);
    KnobView::new(cx, norm, 0.0, min, max, false, move |_cx, g| match g {
        Gesture::Start => lens.begin_edit(id),
        Gesture::Change(v) => {
            lens.set(id, field(&params).info.range.normalize(v as f64));
            display.set(v);
        }
        Gesture::End => lens.end_edit(id),
    })
    .width(Pixels(40.0))
    .height(Pixels(40.0));
    Label::new(cx, Memo::new(move |_| format_knob_value(display.get(), max))).font_size(9.0).color(rgb(1.0, 0.65, 0.3));
    Label::new(cx, label).font_size(9.0).color(col(0.75, 0.75, 0.75, 1.0));
}

/// A single bipolar knob (Tilt, Pan, Width, Inflate Curve, Out Gain).
fn bipolar_knob(cx: &mut Context, lens: &ParamLens<MeridianParams>, params: &Arc<MeridianParams>, id: K, field: FloatField, min: f32, max: f32, default: f32, label: &'static str) {
    let lens = lens.clone();
    let params = params.clone();
    let value = lens.get_plain(id);
    let display = Signal::new(value);
    let norm = ((value - min) / (max - min)).clamp(0.0, 1.0);
    KnobView::new(cx, norm, ((default - min) / (max - min)).clamp(0.0, 1.0), min, max, true, move |_cx, g| match g {
        Gesture::Start => lens.begin_edit(id),
        Gesture::Change(v) => {
            lens.set(id, field(&params).info.range.normalize(v as f64));
            display.set(v);
        }
        Gesture::End => lens.end_edit(id),
    })
    .width(Pixels(40.0))
    .height(Pixels(40.0));
    Label::new(cx, Memo::new(move |_| format_knob_value(display.get(), max.abs().max(min.abs())))).font_size(9.0).color(rgb(1.0, 0.65, 0.3));
    Label::new(cx, label).font_size(9.0).color(col(0.75, 0.75, 0.75, 1.0));
}

// ─── Right sidebar (output level, auto loud, goniometer) ────────────────────

fn build_right_sidebar(cx: &mut Context, telemetry: Signal<Telemetry>, lens: ParamLens<MeridianParams>, params: Arc<MeridianParams>, shared: Arc<SharedState>, params_gen: Signal<u32>) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "OUTPUT LEVEL").font_size(12.0).color(col(0.75, 0.75, 0.75, 1.0));

        HStack::new(cx, {
            let lens = lens.clone();
            let params = params.clone();
            let shared = shared.clone();
            move |cx| {
                // OutputGain can move from outside a knob drag (Auto Loud
                // applies its offset in `tick()`, which bumps `params_gen`
                // for exactly this reason) - re-read the plain value from
                // the param on every bump so the knob doesn't go stale like
                // it did before this Binding existed.
                VStack::new(cx, {
                    let lens = lens.clone();
                    let params = params.clone();
                    move |cx| {
                        Binding::new(cx, params_gen, {
                            let lens = lens.clone();
                            let params = params.clone();
                            move |cx| { bipolar_knob(cx, &lens, &params, K::OutputGain, |p| &p.output_gain, -12.0, 12.0, 0.0, "OUT GAIN"); }
                        });
                    }
                })
                .alignment(Alignment::Center)
                .width(Auto);

                let shared_press = shared.clone();
                let shared_bg = shared.clone();
                VStack::new(cx, move |cx| {
                    Button::new(cx, move |cx| {
                        Label::new(cx, Memo::new(move |_| if telemetry.get().auto_loud_measuring { "MEASURING..." } else { "AUTO LOUD" })).font_size(10.0)
                    })
                    .on_press(move |_cx| shared_press.auto_loud_trigger.store(true, Ordering::Release))
                    .height(Pixels(shared_ui::BUTTON_HEIGHT))
                    .background_color(Memo::new(move |_| {
                        let t = telemetry.get();
                        let is_active = shared_bg.auto_loud_gain_offset.load(Ordering::Acquire).abs() > 0.05;
                        if t.auto_loud_measuring { rgb(1.0, 0.8, 0.0) } else if is_active { shared_ui::AMBER } else { shared_ui::IDLE_BG }
                    }));
                })
                .alignment(Alignment::Center)
                .width(Stretch(1.0));
            }
        })
        .width(Stretch(1.0))
        .height(Auto)
        .horizontal_gap(Pixels(4.0))
        .alignment(Alignment::Center);

        let shared_reset = shared.clone();
        Binding::new(cx, telemetry, move |cx| {
            let t = telemetry.get();
            StereoMeterView::new(cx, t.peak_l, t.peak_r, t.peak_hold_l, t.peak_hold_r, t.balance).width(Stretch(1.0)).height(Pixels(shared_ui::STEREO_METER_HEIGHT));

            let shared_l = shared_reset.clone();
            let shared_r = shared_reset.clone();
            HStack::new(cx, move |cx| {
                Button::new(cx, move |cx| Label::new(cx, fmt_db(t.peak_hold_l)).font_size(11.0))
                    .on_press(move |_cx| shared_l.reset_peak.store(true, Ordering::Release))
                    .background_color(Color::transparent())
                    .color(rgb(1.0, 0.45, 0.1));
                Element::new(cx).width(Stretch(1.0));
                Label::new(cx, "dB").font_size(10.0).color(col(0.8, 0.8, 0.8, 1.0));
                Element::new(cx).width(Stretch(1.0));
                Button::new(cx, move |cx| Label::new(cx, fmt_db(t.peak_hold_r)).font_size(11.0))
                    .on_press(move |_cx| shared_r.reset_peak.store(true, Ordering::Release))
                    .background_color(Color::transparent())
                    .color(rgb(1.0, 0.45, 0.1));
            })
            .width(Stretch(1.0))
            .height(Auto)
            .alignment(Alignment::Center);
        });

        Element::new(cx).height(Stretch(1.0));

        Label::new(cx, "GONIOMETER").font_size(10.0).color(col(0.6, 0.6, 0.6, 1.0));
        let shared_gonio = shared.clone();
        Binding::new(cx, telemetry, move |cx| {
            let t = telemetry.get();
            GoniometerView::new(cx, shared_gonio.scope_samples.clone(), shared_gonio.scope_write_pos.load(Ordering::Acquire), t.phase_correlation)
                .width(Stretch(1.0))
                .height(Pixels(139.0));
        });
    })
    .width(Pixels(155.0))
    .height(Stretch(1.0))
    .padding(Pixels(8.0))
    .vertical_gap(Pixels(6.0))
    .background_color(rgb(0.1, 0.1, 0.1));
}

// ─── Footer (compressor / inflate / stereo / reset) ─────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_footer(
    cx: &mut Context,
    telemetry: Signal<Telemetry>,
    lens: ParamLens<MeridianParams>,
    params: Arc<MeridianParams>,
    shared: Arc<SharedState>,
    accum: Arc<Mutex<TickAccum>>,
    params_gen: Signal<u32>,
) {
    HStack::new(cx, move |cx| {
        // Compressor: knobs in a row, GR envelope below
        VStack::new(cx, {
            let lens = lens.clone();
            let params = params.clone();
            move |cx| {
                Label::new(cx, "COMPRESSOR").font_size(10.0).color(rgb(1.0, 0.55, 0.15));
                // 6 knobs horizontal
                Binding::new(cx, params_gen, {
                    let lens = lens.clone();
                    let params = params.clone();
                    move |cx| {
                HStack::new(cx, {
                    let lens = lens.clone();
                    let params = params.clone();
                    move |cx| {
                for (label, id, field, min, max) in [
                    ("THRESH", K::CompThreshold, (|p: &MeridianParams| &p.comp_threshold) as FloatField, -30.0f32, 0.0f32),
                    ("MIX", K::CompMix, (|p: &MeridianParams| &p.comp_mix) as FloatField, 0.0, 100.0),
                    ("ATTACK", K::CompAttack, (|p: &MeridianParams| &p.comp_attack) as FloatField, 5.0, 50.0),
                    ("RELEASE", K::CompRelease, (|p: &MeridianParams| &p.comp_release) as FloatField, 50.0, 300.0),
                    ("RATIO", K::CompCharacter, (|p: &MeridianParams| &p.comp_character) as FloatField, 1.5, 4.0),
                    ("MAKEUP", K::CompMakeup, (|p: &MeridianParams| &p.comp_makeup) as FloatField, 0.0, 12.0),
                ] {
                    let lens = lens.clone();
                    let params = params.clone();
                    let value = lens.get_plain(id);
                    let display = Signal::new(value);
                    let norm = ((value - min) / (max - min)).clamp(0.0, 1.0);
                    VStack::new(cx, move |cx| {
                        KnobView::new(cx, norm, 0.0, min, max, false, move |_cx, g| match g {
                            Gesture::Start => lens.begin_edit(id),
                            Gesture::Change(v) => {
                                lens.set(id, field(&params).info.range.normalize(v as f64));
                                display.set(v);
                            }
                            Gesture::End => lens.end_edit(id),
                        })
                        .width(Pixels(40.0))
                        .height(Pixels(40.0));
                        Label::new(cx, Memo::new(move |_| format_knob_value(display.get(), max))).font_size(9.0).color(rgb(1.0, 0.65, 0.3));
                        Label::new(cx, label).font_size(9.0).color(col(0.75, 0.75, 0.75, 1.0));
                    })
                    .alignment(Alignment::Center)
                    .width(Pixels(40.0));
                }
                    }
                })
                .horizontal_gap(Pixels(16.0))
                .alignment(Alignment::Center)
                .height(Auto);
                    }
                });
                // GR envelope display below the knobs
                Binding::new(cx, telemetry, move |cx| {
                    let t = telemetry.get();
                    HStack::new(cx, move |cx| {
                        CompressorEnvelopeView::new(cx, CompressorEnvelopeView {
                            history: t.gr_history.clone(),
                            current: t.gain_reduction,
                            peak_hold: t.gr_peak_hold,
                        })
                        .width(Pixels(110.0))
                        .height(Pixels(24.0));
                        VStack::new(cx, move |cx| {
                            Label::new(cx, format!("PK: {:.1}", t.gr_peak_hold)).font_size(9.0).color(rgb(1.0, 0.6, 0.2));
                            Label::new(cx, format!("GR: {:.1}", t.gain_reduction)).font_size(8.0).color(rgb(1.0, 0.3, 0.3));
                        })
                        .alignment(Alignment::Center)
                        .width(Auto);
                    })
                    .horizontal_gap(Pixels(4.0))
                    .alignment(Alignment::Center)
                    .width(Auto)
                    .height(Auto);
                });
            }
        })
        .padding(Pixels(4.0))
        .vertical_gap(Pixels(4.0))
        .alignment(Alignment::Center)
        .width(Pixels(440.0));

        vsep(cx);

        // Inflate
        VStack::new(cx, {
            let lens = lens.clone();
            let params = params.clone();
            move |cx| {
                Label::new(cx, "INFLATE").font_size(10.0).color(rgb(1.0, 0.55, 0.15));
                HStack::new(cx, move |cx| {
                    Binding::new(cx, params_gen, {
                        let lens = lens.clone();
                        let params = params.clone();
                        move |cx| {
                    VStack::new(cx, {
                        let lens = lens.clone();
                        let params = params.clone();
                        move |cx| { plain_knob(cx, &lens, &params, K::InflateEffect, |p| &p.inflate_effect, 0.0, 100.0, "EFFECT"); }
                    })
                    .alignment(Alignment::Center)
                    .width(Pixels(40.0));

                    VStack::new(cx, {
                        let lens = lens.clone();
                        let params = params.clone();
                        move |cx| { bipolar_knob(cx, &lens, &params, K::InflateCurve, |p| &p.inflate_curve, -50.0, 50.0, 0.0, "CURVE"); }
                    })
                    .alignment(Alignment::Center)
                    .width(Pixels(40.0));
                        }
                    });

                    VStack::new(cx, {
                        let lens = lens.clone();
                        move |cx| {
                            styled_toggle_small(cx, lens.clone(), K::InflateBandSplit, "SPLIT");
                            styled_toggle_small(cx, lens.clone(), K::InflateClip, "CLIP");
                        }
                    })
                    .vertical_gap(Pixels(6.0))
                    .width(Auto);
                })
                .horizontal_gap(Pixels(20.0))
                .alignment(Alignment::Center)
                .height(Auto);
            }
        })
        .padding(Pixels(10.0))
        .vertical_gap(Pixels(6.0))
        .alignment(Alignment::Center)
        .width(Pixels(220.0));

        vsep(cx);

        // Stereo / Routing
        VStack::new(cx, {
            let lens = lens.clone();
            let params = params.clone();
            move |cx| {
                Label::new(cx, "STEREO / ROUTING").font_size(10.0).color(rgb(1.0, 0.55, 0.15));
                HStack::new(cx, move |cx| {
                    Binding::new(cx, params_gen, {
                        let lens = lens.clone();
                        let params = params.clone();
                        move |cx| {
                    VStack::new(cx, {
                        let lens = lens.clone();
                        let params = params.clone();
                        move |cx| { bipolar_knob(cx, &lens, &params, K::Pan, |p| &p.pan, -1.0, 1.0, 0.0, "PAN"); }
                    })
                    .alignment(Alignment::Center)
                    .width(Pixels(40.0));
                    VStack::new(cx, {
                        let lens = lens.clone();
                        let params = params.clone();
                        move |cx| { bipolar_knob(cx, &lens, &params, K::StereoWidth, |p| &p.stereo_width, 0.0, 200.0, 100.0, "WIDTH"); }
                    })
                    .alignment(Alignment::Center)
                    .width(Pixels(40.0));
                        }
                    });
                })
                .horizontal_gap(Pixels(20.0))
                .alignment(Alignment::Center)
                .height(Auto);
            }
        })
        .padding(Pixels(10.0))
        .vertical_gap(Pixels(6.0))
        .alignment(Alignment::Center)
        .width(Pixels(185.0));

        vsep(cx);

        shared_ui::danger_button_big(cx, "RESET", move |_cx| reset_all(&lens, &params, &shared, &accum, params_gen))
            .width(Pixels(70.0));
    })
    .width(Stretch(1.0))
    .height(Pixels(110.0))
    .padding(Pixels(8.0))
    .alignment(Alignment::Center)
    .horizontal_gap(Pixels(15.0))
    .background_color(rgb(0.08, 0.08, 0.08));
}

fn styled_toggle_small(cx: &mut Context, lens: ParamLens<MeridianParams>, id: K, label: &'static str) {
    let sig = lens.value_signal(id);
    Binding::new(cx, sig, move |cx| {
        let active = lens.get(id) > 0.5;
        let lens = lens.clone();
        shared_ui::toggle_button_small(cx, label, active, move |_cx| {
            let now = lens.get(id) <= 0.5;
            let norm = if now { 1.0 } else { 0.0 };
            lens.automate(id, norm);
            sig.set(norm as f32);
        })
        .width(Pixels(48.0));
    });
}

fn reset_all(lens: &ParamLens<MeridianParams>, params: &MeridianParams, shared: &SharedState, accum: &Arc<Mutex<TickAccum>>, params_gen: Signal<u32>) {
    let default = MeridianProfile::default();
    apply_profile(lens, params, &default);
    shared.reset_analysis.store(true, Ordering::Release);
    let mut acc = accum.lock().unwrap();
    acc.gr_peak_hold_ticks = 0;
    drop(acc);
    params_gen.update(|g| *g = g.wrapping_add(1));
}

