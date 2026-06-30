// Aether editor — Iced UI truce port.
//
// Layout (monitoring tool, compact):
//   Header : brand + [preset dropdown] [name input] SAVE SETUP | BYPASS
//   Curve  : compact amber EQ curve (mirrors the 5 DSP biquads)
//   Inst   : Amber instruction text
//   Body   : EQ bands (left) | Crossfeed (right)
//   Footer : HARMAN BLEND · INPUT peak · GAIN · RESET

use truce_iced::iced::widget::{button, canvas, column, container, pick_list, row, text_input, Space, Text};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Padding, Subscription};
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::path::PathBuf;

use shared_analysis::SharedState;
use shared_dsp::Biquad;
use shared_ui::{
    bold_font, header_brand, toggle_button, vault_setup_box,
    Gesture, knob_gesture, knob_gesture_bipolar, knob_gesture_curved,
    output_tools_strip,
    SpectrumCanvas, SpectrumConfig, EqOverlay,
};

use crate::{AetherParams, AetherParamsParamId};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const AMBER: Color = Color { r: 1.0, g: 0.55, b: 0.1, a: 1.0 };

fn smoothstep(t: f32) -> f32 { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }

fn angle_from_ui(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.35 { 30.0 + 15.0 * smoothstep(x / 0.35) }
    else if x < 0.75 { 45.0 + 15.0 * smoothstep((x - 0.35) / 0.40) }
    else { 60.0 + 15.0 * smoothstep((x - 0.75) / 0.25) }
}

const BAND_DEF: [(f32, f32, i32); 5] = [
    (105.0, 0.7, 1), (300.0, 1.0, 2), (1200.0, 1.0, 2), (4000.0, 1.0, 2), (10000.0, 0.7, 3),
];
const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;

fn eq_field_id(b: usize, f: usize) -> String { format!("aether-eq-{b}-{f}") }

// ─── Preset Profile ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AetherProfile {
    pub name: String,
    pub bands: [(i32, f32, f32, f32); 5],
    pub cf_angle: f32,
    pub cf_amount: f32,
    pub cf_realism: i32,
    pub blend: f32,
    pub gain: f32,
}

// ─── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AetherMsg {
    Tick,
    EqTextChanged(usize, usize, String),
    EqTypeCycled(usize),
    BlendGesture(Gesture),
    CfAngleGesture(Gesture),
    CfAmountGesture(Gesture),
    RealismCycled,
    GainGesture(Gesture),
    BypassToggled,
    ResetAll,
    SelectPreset(usize),
    PresetNameChanged(String),
    SavePreset,
    SetupToggled,
    VaultPathChanged(String),
    SaveVaultPath,
}

// ─── Editor ──────────────────────────────────────────────────────────────────

pub struct AetherEditor {
    params: Arc<AetherParams>,
    shared_state: Arc<SharedState>,
    vault_path: Option<String>,
    show_setup: bool,
    vault_path_input: String,
    preset_name_input: String,
    presets: Vec<(String, Option<PathBuf>, AetherProfile)>,
    selected_preset_index: Option<usize>,
    eq_text: [[String; 3]; 5],
    in_peak: f32,
    in_peak_hold: f32,
    in_peak_hold_ticks: u32,
    preset_refresh_counter: u32,
}

// ─── Default Presets ─────────────────────────────────────────────────────────

pub(crate) fn harman_flat_profile() -> AetherProfile {
    AetherProfile {
        name: "Harman Flat".into(),
        bands: [(1, 105.0, 0.0, 0.7), (2, 300.0, 0.0, 1.0), (2, 1200.0, 0.0, 1.0), (2, 4000.0, 0.0, 1.0), (3, 10000.0, 0.0, 0.7)],
        cf_angle: 60.0, cf_amount: 0.0, cf_realism: 0, blend: 100.0, gain: 0.0,
    }
}

fn default_presets() -> Vec<(String, Option<PathBuf>, AetherProfile)> {
    vec![("Harman Flat".into(), None, harman_flat_profile())]
}

pub(crate) fn resolve_last_preset() -> Option<AetherProfile> {
    let config = shared_analysis::load_config("Aether");
    let name = config.last_preset?;
    if let Some((_, _, p)) = default_presets().into_iter().find(|(n, _, _)| *n == name) { return Some(p); }
    if let Some(ref vp) = config.vault_path {
        if let Some((_, _, p)) = scan_aether_presets(std::path::Path::new(vp)).into_iter().find(|(n, _, _)| *n == name) { return Some(p); }
    }
    None
}

fn section_header(text: &str) -> Text<'_> {
    Text::new(text).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75))
}

// ─── EQ helpers (uses raw_target so editor and DSP stay in sync) ─────────────

impl AetherEditor {
    fn eq_freq(&self, i: usize) -> f32 {
        let p = &self.params;
        [&p.eq1_freq, &p.eq2_freq, &p.eq3_freq, &p.eq4_freq, &p.eq5_freq][i].raw_target() as f32
    }
    fn eq_gain(&self, i: usize) -> f32 {
        let p = &self.params;
        [&p.eq1_gain, &p.eq2_gain, &p.eq3_gain, &p.eq4_gain, &p.eq5_gain][i].raw_target() as f32
    }
    fn eq_q(&self, i: usize) -> f32 {
        let p = &self.params;
        [&p.eq1_q, &p.eq2_q, &p.eq3_q, &p.eq4_q, &p.eq5_q][i].raw_target() as f32
    }
    fn eq_type(&self, i: usize) -> i32 {
        let p = &self.params;
        [&p.eq1_type, &p.eq2_type, &p.eq3_type, &p.eq4_type, &p.eq5_type][i].value_i32()
    }

    fn eq_curve_points(&self, sr: f32) -> Vec<(f32, f32)> {
        let mut bands: [Biquad; 5] = std::array::from_fn(|_| Biquad::new());
        for i in 0..5 {
            crate::set_band(&mut bands[i], self.eq_type(i), self.eq_freq(i), self.eq_gain(i), self.eq_q(i), sr);
        }
        const N: usize = 240;
        (0..N).map(|i| {
            let f = 20.0f32 * 1000.0f32.powf(i as f32 / (N - 1) as f32);
            (i as f32 / (N - 1) as f32, bands.iter().map(|b| b.magnitude_db(f, sr)).sum())
        }).collect()
    }

    fn eq_text_from_params(&self) -> [[String; 3]; 5] {
        std::array::from_fn(|i| [
            format!("{:.0}", self.eq_freq(i)),
            format!("{:.1}", self.eq_gain(i)),
            format!("{:.2}", self.eq_q(i)),
        ])
    }

    fn band_column<'a>(&'a self, i: usize, _params: &'a ParamCache<AetherParams>) -> Element<'a, Message<AetherMsg>> {
        let tcode = self.eq_type(i);
        let lbl = |t: &'static str| Text::new(t).size(9).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6));
        column![
            toggle_button(crate::band_type_label(tcode), tcode != 0, Message::Plugin(AetherMsg::EqTypeCycled(i))),
            column![lbl("FREQ"), text_input("", &self.eq_text[i][0]).id(eq_field_id(i, 0)).on_input(move |s| Message::Plugin(AetherMsg::EqTextChanged(i, 0, s))).size(12).padding(4).width(Length::Fixed(56.0))].spacing(2).align_x(Alignment::Center),
            column![lbl("GAIN"), text_input("", &self.eq_text[i][1]).id(eq_field_id(i, 1)).on_input(move |s| Message::Plugin(AetherMsg::EqTextChanged(i, 1, s))).size(12).padding(4).width(Length::Fixed(56.0))].spacing(2).align_x(Alignment::Center),
            column![lbl("Q"),    text_input("", &self.eq_text[i][2]).id(eq_field_id(i, 2)).on_input(move |s| Message::Plugin(AetherMsg::EqTextChanged(i, 2, s))).size(12).padding(4).width(Length::Fixed(56.0))].spacing(2).align_x(Alignment::Center),
        ].spacing(4).align_x(Alignment::Center).into()
    }

    fn set_eq_type(&self, i: usize, v: i32, ctx: &PluginContext<AetherParams>) {
        let ids = [AetherParamsParamId::Eq1Type, AetherParamsParamId::Eq2Type, AetherParamsParamId::Eq3Type, AetherParamsParamId::Eq4Type, AetherParamsParamId::Eq5Type];
        ctx.begin_edit(ids[i]); ctx.set_param(ids[i], v as f64 / 3.0); ctx.end_edit(ids[i]);
    }
    fn set_eq_freq(&self, i: usize, v: f32, ctx: &PluginContext<AetherParams>) {
        let ids = [AetherParamsParamId::Eq1Freq, AetherParamsParamId::Eq2Freq, AetherParamsParamId::Eq3Freq, AetherParamsParamId::Eq4Freq, AetherParamsParamId::Eq5Freq];
        // log(20, 20000): normalized = log10(v/20) / log10(1000)
        let norm = ((v / 20.0).log10() / 3.0).clamp(0.0, 1.0);
        ctx.begin_edit(ids[i]); ctx.set_param(ids[i], norm as f64); ctx.end_edit(ids[i]);
    }
    fn set_eq_gain(&self, i: usize, v: f32, ctx: &PluginContext<AetherParams>) {
        let ids = [AetherParamsParamId::Eq1Gain, AetherParamsParamId::Eq2Gain, AetherParamsParamId::Eq3Gain, AetherParamsParamId::Eq4Gain, AetherParamsParamId::Eq5Gain];
        // linear(-12, 12): normalized = (v+12)/24
        let norm = ((v + 12.0) / 24.0).clamp(0.0, 1.0);
        ctx.begin_edit(ids[i]); ctx.set_param(ids[i], norm as f64); ctx.end_edit(ids[i]);
    }
    fn set_eq_q(&self, i: usize, v: f32, ctx: &PluginContext<AetherParams>) {
        let ids = [AetherParamsParamId::Eq1Q, AetherParamsParamId::Eq2Q, AetherParamsParamId::Eq3Q, AetherParamsParamId::Eq4Q, AetherParamsParamId::Eq5Q];
        // log(0.3, 8): normalized = log10(v/0.3) / log10(8/0.3)
        let norm = ((v / 0.3).log10() / (8.0_f32 / 0.3).log10()).clamp(0.0, 1.0);
        ctx.begin_edit(ids[i]); ctx.set_param(ids[i], norm as f64); ctx.end_edit(ids[i]);
    }

    fn gesture_f(&self, id: AetherParamsParamId, g: Gesture, min: f32, max: f32, ctx: &PluginContext<AetherParams>) {
        match g {
            Gesture::Start => ctx.begin_edit(id),
            Gesture::Change(v) => {
                let norm = if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
                ctx.set_param(id, norm as f64);
            }
            Gesture::End => ctx.end_edit(id),
        }
    }

    fn reset_all(&self, ctx: &PluginContext<AetherParams>) {
        for i in 0..5 {
            let (fdef, qdef, tdef) = BAND_DEF[i];
            self.set_eq_freq(i, fdef, ctx);
            self.set_eq_gain(i, 0.0, ctx);
            self.set_eq_q(i, qdef, ctx);
            self.set_eq_type(i, tdef, ctx);
        }
        ctx.begin_edit(AetherParamsParamId::Blend); ctx.set_param(AetherParamsParamId::Blend, 1.0); ctx.end_edit(AetherParamsParamId::Blend);
        ctx.begin_edit(AetherParamsParamId::CfAngle); ctx.set_param(AetherParamsParamId::CfAngle, (30.0/45.0) as f64); ctx.end_edit(AetherParamsParamId::CfAngle);
        ctx.begin_edit(AetherParamsParamId::CfAmount); ctx.set_param(AetherParamsParamId::CfAmount, 0.0); ctx.end_edit(AetherParamsParamId::CfAmount);
        ctx.begin_edit(AetherParamsParamId::CfRealism); ctx.set_param(AetherParamsParamId::CfRealism, 0.0); ctx.end_edit(AetherParamsParamId::CfRealism);
        ctx.begin_edit(AetherParamsParamId::Gain); ctx.set_param(AetherParamsParamId::Gain, 0.5); ctx.end_edit(AetherParamsParamId::Gain);
    }

    fn apply_profile(&self, p: &AetherProfile, ctx: &PluginContext<AetherParams>) {
        for i in 0..5 {
            let (tc, fc, gn, q) = p.bands[i];
            self.set_eq_freq(i, fc, ctx); self.set_eq_gain(i, gn, ctx); self.set_eq_q(i, q, ctx);
            self.set_eq_type(i, tc, ctx);
        }
        ctx.begin_edit(AetherParamsParamId::Blend); ctx.set_param(AetherParamsParamId::Blend, (p.blend as f64 / 100.0).clamp(0.0, 1.0)); ctx.end_edit(AetherParamsParamId::Blend);
        ctx.begin_edit(AetherParamsParamId::CfAngle); ctx.set_param(AetherParamsParamId::CfAngle, ((p.cf_angle as f64 - 30.0) / 45.0).clamp(0.0, 1.0)); ctx.end_edit(AetherParamsParamId::CfAngle);
        ctx.begin_edit(AetherParamsParamId::CfAmount); ctx.set_param(AetherParamsParamId::CfAmount, (p.cf_amount as f64 / 100.0).clamp(0.0, 1.0)); ctx.end_edit(AetherParamsParamId::CfAmount);
        ctx.begin_edit(AetherParamsParamId::CfRealism); ctx.set_param(AetherParamsParamId::CfRealism, p.cf_realism as f64 / 2.0); ctx.end_edit(AetherParamsParamId::CfRealism);
        ctx.begin_edit(AetherParamsParamId::Gain); ctx.set_param(AetherParamsParamId::Gain, ((p.gain as f64 + 12.0) / 24.0).clamp(0.0, 1.0)); ctx.end_edit(AetherParamsParamId::Gain);
    }

    fn build_profile(&self) -> AetherProfile {
        AetherProfile {
            name: self.preset_name_input.clone(),
            bands: [
                (self.eq_type(0), self.eq_freq(0), self.eq_gain(0), self.eq_q(0)),
                (self.eq_type(1), self.eq_freq(1), self.eq_gain(1), self.eq_q(1)),
                (self.eq_type(2), self.eq_freq(2), self.eq_gain(2), self.eq_q(2)),
                (self.eq_type(3), self.eq_freq(3), self.eq_gain(3), self.eq_q(3)),
                (self.eq_type(4), self.eq_freq(4), self.eq_gain(4), self.eq_q(4)),
            ],
            cf_angle: self.params.cf_angle.raw_target() as f32,
            cf_amount: self.params.cf_amount.raw_target() as f32,
            cf_realism: self.params.cf_realism.value_i32(),
            blend: self.params.blend.raw_target() as f32,
            gain: self.params.gain.raw_target() as f32,
        }
    }

    fn build_profile_md(&self) -> String {
        let mut s = String::from(
            "---\nplugin: aether\ntype: preset\n---\n\n\
             > Warning: Do NOT modify column names or table structure. Plugin requires exact format for import. Only the NUMBERS may be changed.\n\n\
             ## Parameter\n\n| Parameter | Wert | Einheit |\n|---|---|---|\n"
        );
        for i in 0..5 {
            s.push_str(&format!("| EQ{} Type | {} | |\n", i+1, crate::band_type_label(self.eq_type(i))));
            s.push_str(&format!("| EQ{} Freq | {:.0} | Hz |\n", i+1, self.eq_freq(i)));
            s.push_str(&format!("| EQ{} Gain | {:.1} | dB |\n", i+1, self.eq_gain(i)));
            s.push_str(&format!("| EQ{} Q | {:.2} | |\n", i+1, self.eq_q(i)));
        }
        s.push_str(&format!("| Crossfeed Angle | {:.0} | ° |\n", self.params.cf_angle.raw_target() as f32));
        s.push_str(&format!("| Crossfeed Amount | {:.0} | % |\n", self.params.cf_amount.raw_target() as f32));
        s.push_str(&format!("| Crossfeed Realism | {} | |\n", crate::realism_label(self.params.cf_realism.value_i32())));
        s.push_str(&format!("| Blend | {:.0} | % |\n", self.params.blend.raw_target() as f32));
        s.push_str(&format!("| Gain | {:.1} | dB |\n", self.params.gain.raw_target() as f32));
        s
    }

    fn save_last_preset(&self, name: &str) {
        let mut cfg = shared_analysis::load_config("Aether");
        cfg.vault_path = self.vault_path.clone();
        cfg.last_preset = Some(name.to_string());
        let _ = shared_analysis::save_config("Aether", &cfg);
    }

    fn small_button<'a>(label: &'a str, msg: AetherMsg) -> Element<'a, Message<AetherMsg>> {
        button(Text::new(label).size(12).font(bold_font()))
            .on_press(Message::Plugin(msg))
            .padding(Padding { top: 5.0, right: 10.0, bottom: 5.0, left: 10.0 })
            .style(|_t, status| {
                let bg = if status == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.15, 0.15, 0.15) };
                button::Style { background: Some(bg.into()), text_color: Color::WHITE, border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default() }
            }).into()
    }

    fn input_reader(&self) -> Element<'_, Message<AetherMsg>> {
        let fast = if self.in_peak <= -90.0 { String::from("--") } else { format!("{:.1} dB", self.in_peak) };
        let hold = if self.in_peak_hold <= -90.0 { String::from("--") } else { format!("{:.1} dB", self.in_peak_hold) };
        container(column![
            Text::new("INPUT").size(10).font(bold_font()).color(Color::from_rgb(0.7, 0.7, 0.7)),
            Text::new(fast).size(15).font(bold_font()).color(Color::from_rgb(0.85, 0.85, 0.85)),
            Text::new(format!("pk {hold}")).size(11).font(bold_font()).color(AMBER),
        ].spacing(2).align_x(Alignment::Center))
        .padding(6).style(|_t| container::Style {
            background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()),
            border: Border { color: Color::from_rgb(0.2, 0.2, 0.2), width: 1.0, radius: 3.0.into() },
            ..Default::default()
        }).into()
    }

    fn vertical_separator() -> Element<'static, Message<AetherMsg>> {
        container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fixed(28.0))
            .style(|_t| container::Style { background: Some(Color::from_rgb(0.18, 0.22, 0.22).into()), ..Default::default() }).into()
    }
}

// ─── Preset Parser ───────────────────────────────────────────────────────────

fn parse_aether_preset(content: &str) -> Option<AetherProfile> {
    match shared_analysis::preset_plugin_name(content).as_deref() {
        Some("aether") => {} _ => return None,
    }
    let mut bands = [(1i32, 105.0f32, 0.0f32, 0.7f32); 5];
    let mut cf_angle = 60.0f32; let mut cf_amount = 0.0f32; let mut cf_realism = 0i32;
    let mut blend = 100.0f32; let mut gain = 0.0f32;
    let mut name = String::new();
    let mut has_freq = [false; 5]; let mut has_gain = [false; 5];

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') {
            let parts: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
            if parts.len() >= 4 {
                let param_name = parts[1].to_lowercase(); let val_str = parts[2];
                match param_name.as_str() {
                    s if s.starts_with("eq") && s.contains("type") => {
                        if let Some(bi) = s.chars().find(|c| c.is_ascii_digit()).and_then(|c| c.to_digit(10)) {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            bands[idx].0 = match val_str { "LSC"|"LS" => 1, "PK"|"PEQ" => 2, "HSC"|"HS" => 3, _ => 0 };
                        }
                    }
                    s if s.starts_with("eq") && s.contains("freq") => {
                        if let Some(bi) = s.chars().find(|c| c.is_ascii_digit()).and_then(|c| c.to_digit(10)) {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            if let Ok(v) = val_str.parse() { bands[idx].1 = v; has_freq[idx] = true; }
                        }
                    }
                    s if s.starts_with("eq") && s.contains("gain") => {
                        if let Some(bi) = s.chars().find(|c| c.is_ascii_digit()).and_then(|c| c.to_digit(10)) {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            if let Ok(v) = val_str.parse() { bands[idx].2 = v; has_gain[idx] = true; }
                        }
                    }
                    s if s.starts_with("eq") && s.contains('q') => {
                        if let Some(bi) = s.chars().find(|c| c.is_ascii_digit()).and_then(|c| c.to_digit(10)) {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            if let Ok(v) = val_str.parse() { bands[idx].3 = v; }
                        }
                    }
                    "crossfeed angle" => { if let Ok(v) = val_str.parse() { cf_angle = v; } }
                    "crossfeed amount" => { if let Ok(v) = val_str.parse() { cf_amount = v; } }
                    "crossfeed realism" => { cf_realism = match val_str { "LIFELIKE" => 1, "HYPERREAL"|"HYPERREALISTIC" => 2, _ => 0 }; }
                    "blend" => { if let Ok(v) = val_str.parse() { blend = v; } }
                    "gain" => { if let Ok(v) = val_str.parse() { gain = v; } }
                    _ => {}
                }
            }
        }
        if trimmed.starts_with("# ") && !trimmed.starts_with("## ") { name = trimmed.trim_start_matches("# ").trim().to_string(); }
    }
    if has_freq.iter().all(|&h| h) && has_gain.iter().all(|&h| h) {
        Some(AetherProfile { name, bands, cf_angle, cf_amount, cf_realism, blend, gain })
    } else { None }
}

fn scan_aether_presets(dir: &std::path::Path) -> Vec<(String, PathBuf, AetherProfile)> {
    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Some(mut profile) = parse_aether_preset(&content) {
                        if profile.name.is_empty() { profile.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unnamed").to_string(); }
                        result.push((profile.name.clone(), path, profile));
                    }
                }
            }
        }
    }
    result
}

// ─── IcedPlugin ───────────────────────────────────────────────────────────────

impl IcedPlugin<AetherParams> for AetherEditor {
    type Message = AetherMsg;

    fn new(params: Arc<AetherParams>) -> Self {
        let config = shared_analysis::load_config("Aether");
        let mut presets = default_presets();
        if let Some(ref vp) = config.vault_path {
            let custom = scan_aether_presets(std::path::Path::new(vp));
            for (name, path, profile) in custom { presets.push((name, Some(path), profile)); }
        }
        let selected_preset_index = config.last_preset.as_ref()
            .and_then(|n| presets.iter().position(|(name, _, _)| name == n))
            .or(Some(0));
        let preset_name_input = config.last_preset.clone().unwrap_or_default();
        let shared_state = params.shared.clone();
        let mut editor = Self {
            params,
            shared_state,
            vault_path: config.vault_path.clone(),
            show_setup: false,
            vault_path_input: config.vault_path.unwrap_or_default(),
            preset_name_input,
            presets,
            selected_preset_index,
            eq_text: std::array::from_fn(|_| std::array::from_fn(|_| String::new())),
            in_peak: -90.0, in_peak_hold: -90.0, in_peak_hold_ticks: 0,
            preset_refresh_counter: 0,
        };
        editor.eq_text = editor.eq_text_from_params();
        editor
    }

    fn subscription(&self) -> Subscription<Message<AetherMsg>> {
        truce_iced::iced::event::listen_raw(|event, _status, _window| {
            use truce_iced::iced::{Event, window::Event as WinEvent};
            match event {
                Event::Window(WinEvent::RedrawRequested(_)) => Some(Message::Plugin(AetherMsg::Tick)),
                _ => None,
            }
        })
    }

    fn needs_redraw(&self) -> bool { true }

    fn update(
        &mut self,
        message: Message<AetherMsg>,
        _params: &ParamCache<AetherParams>,
        ctx: &PluginContext<AetherParams>,
    ) -> truce_iced::iced::Task<Message<AetherMsg>> {
        let Message::Plugin(msg) = message else { return truce_iced::iced::Task::none(); };

        match msg {
            AetherMsg::Tick => {
                self.in_peak = self.shared_state.input_peak.load(Ordering::Relaxed);
                if self.in_peak > self.in_peak_hold { self.in_peak_hold = self.in_peak; self.in_peak_hold_ticks = 90; }
                else if self.in_peak_hold_ticks > 0 { self.in_peak_hold_ticks -= 1; }
                else { self.in_peak_hold = (self.in_peak_hold - 0.5).max(self.in_peak); }
                self.preset_refresh_counter += 1;
                if self.preset_refresh_counter >= 150 {
                    self.preset_refresh_counter = 0;
                    if let Some(ref vp) = self.vault_path {
                        let custom = scan_aether_presets(std::path::Path::new(vp));
                        self.presets.retain(|(_, path, _)| path.is_none());
                        for (name, path, profile) in custom { self.presets.push((name, Some(path), profile)); }
                    }
                }
            }
            AetherMsg::EqTextChanged(b, f, s) => {
                self.eq_text[b][f] = s.clone();
                if let Ok(v) = s.trim().parse::<f32>() {
                    match f {
                        0 => self.set_eq_freq(b, v.clamp(FREQ_MIN, FREQ_MAX), ctx),
                        1 => self.set_eq_gain(b, v.clamp(-12.0, 12.0), ctx),
                        _ => self.set_eq_q(b, v.clamp(0.3, 8.0), ctx),
                    }
                }
            }
            AetherMsg::EqTypeCycled(i) => { let n = (self.eq_type(i) + 1) % 4; self.set_eq_type(i, n, ctx); }
            AetherMsg::BlendGesture(g) => self.gesture_f(AetherParamsParamId::Blend, g, 0.0, 100.0, ctx),
            AetherMsg::CfAngleGesture(g) => self.gesture_f(AetherParamsParamId::CfAngle, g, 30.0, 75.0, ctx),
            AetherMsg::CfAmountGesture(g) => self.gesture_f(AetherParamsParamId::CfAmount, g, 0.0, 100.0, ctx),
            AetherMsg::RealismCycled => {
                let n = (self.params.cf_realism.value_i32() + 1) % 3;
                ctx.begin_edit(AetherParamsParamId::CfRealism);
                ctx.set_param(AetherParamsParamId::CfRealism, n as f64 / 2.0);
                ctx.end_edit(AetherParamsParamId::CfRealism);
            }
            AetherMsg::GainGesture(g) => self.gesture_f(AetherParamsParamId::Gain, g, -12.0, 12.0, ctx),
            AetherMsg::BypassToggled => {
                let v = !self.params.bypass.value();
                ctx.begin_edit(AetherParamsParamId::Bypass);
                ctx.set_param(AetherParamsParamId::Bypass, if v { 1.0 } else { 0.0 });
                ctx.end_edit(AetherParamsParamId::Bypass);
            }
            AetherMsg::ResetAll => { self.reset_all(ctx); self.eq_text = self.eq_text_from_params(); }
            AetherMsg::SelectPreset(idx) => {
                if idx < self.presets.len() {
                    self.selected_preset_index = Some(idx);
                    let profile = self.presets[idx].2.clone();
                    self.apply_profile(&profile, ctx);
                    self.eq_text = self.eq_text_from_params();
                    self.preset_name_input = profile.name.clone();
                    self.save_last_preset(&profile.name);
                }
            }
            AetherMsg::PresetNameChanged(s) => self.preset_name_input = s,
            AetherMsg::SavePreset => {
                let profile = self.build_profile();
                if let Some(ref path) = self.vault_path {
                    let md = self.build_profile_md();
                    let name = profile.name.clone();
                    let fp = std::path::Path::new(path).join(format!("{}.md", name));
                    let p = fp.clone();
                    std::thread::spawn(move || { let _ = std::fs::write(&p, md); });
                    self.presets.push((name.clone(), Some(fp), profile));
                    self.selected_preset_index = Some(self.presets.len() - 1);
                    self.save_last_preset(&name);
                } else { self.show_setup = true; }
            }
            AetherMsg::SetupToggled => {
                self.show_setup = !self.show_setup;
                if self.show_setup { self.vault_path_input = self.vault_path.clone().unwrap_or_default(); }
            }
            AetherMsg::VaultPathChanged(p) => self.vault_path_input = p,
            AetherMsg::SaveVaultPath => {
                let new_path = if self.vault_path_input.trim().is_empty() { None } else { Some(self.vault_path_input.trim().to_string()) };
                self.vault_path = new_path.clone();
                let mut cfg = shared_analysis::load_config("Aether");
                cfg.vault_path = new_path;
                let _ = shared_analysis::save_config("Aether", &cfg);
                self.presets = default_presets();
                if let Some(ref vp) = self.vault_path {
                    let custom = scan_aether_presets(std::path::Path::new(vp));
                    for (name, path, profile) in custom { self.presets.push((name, Some(path), profile)); }
                }
                self.selected_preset_index = Some(0);
                self.show_setup = false;
            }
        }
        truce_iced::iced::Task::none()
    }

    fn view<'a>(&'a self, params: &'a ParamCache<AetherParams>) -> Element<'a, Message<AetherMsg>> {
        let preset_names: Vec<String> = self.presets.iter().map(|(n, _, _)| n.clone()).collect();
        let selected_name: Option<String> = self.selected_preset_index.and_then(|i| preset_names.get(i).cloned());

        let header = container(
            row![
                container(header_brand("Aether", VERSION)).width(Length::Fill),
                pick_list(preset_names.clone(), selected_name.clone(), move |name: String| {
                    let idx = preset_names.iter().position(|n| *n == name).unwrap_or(0);
                    Message::Plugin(AetherMsg::SelectPreset(idx))
                }).width(Length::Fixed(155.0)).text_size(12),
                Space::new().width(Length::Fixed(4.0)),
                text_input("Preset name...", &self.preset_name_input)
                    .on_input(|s| Message::Plugin(AetherMsg::PresetNameChanged(s))).size(12).padding(4).width(Length::Fixed(110.0)),
                Space::new().width(Length::Fixed(4.0)),
                AetherEditor::small_button("SAVE", AetherMsg::SavePreset),
                Space::new().width(Length::Fixed(4.0)),
                AetherEditor::small_button("SETUP", AetherMsg::SetupToggled),
                Space::new().width(Length::Fixed(8.0)),
                AetherEditor::vertical_separator(),
                Space::new().width(Length::Fixed(8.0)),
                toggle_button("BYPASS", self.params.bypass.value(), Message::Plugin(AetherMsg::BypassToggled)),
            ].align_y(Alignment::Center)
        ).width(Length::Fill).height(Length::Fixed(50.0)).padding(10)
        .style(|_t| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        if self.show_setup {
            let setup_box = vault_setup_box("Aether", &self.vault_path_input,
                |s| Message::Plugin(AetherMsg::VaultPathChanged(s)),
                Message::Plugin(AetherMsg::SaveVaultPath),
                Message::Plugin(AetherMsg::SetupToggled),
            );
            return column![header, container(setup_box).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill)
                .style(|_t| container::Style { background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()), ..Default::default() })].into();
        }

        let sr = self.shared_state.sample_rate.load(Ordering::Relaxed);
        let eq_overlay = Some(EqOverlay {
            points: self.eq_curve_points(sr), min_db: -15.0, max_db: 15.0,
            line_color: Color::from_rgba(1.0, 0.6, 0.1, 0.85), fill_alpha: 0.08, grid_db: Vec::new(),
        });
        let sp_cfg = SpectrumConfig {
            sample_rate: sr,
            freq_grid: vec![20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0],
            ..Default::default()
        };
        let curve = container(column![
            canvas(SpectrumCanvas {
                curves: Vec::new(), config: sp_cfg, eq_overlay, resonance_peaks: Vec::new(), masking: Vec::new(),
            }).width(Length::Fill).height(Length::Fixed(90.0)),
            row![
                Text::new("20").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("50").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("100").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("200").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("500").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("1k").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("2k").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("5k").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("10k").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)), Space::new().width(Length::Fill),
                Text::new("20k").size(8).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)),
            ]
        ].spacing(2))
        .width(Length::Fill).height(Length::Fixed(110.0)).padding(6)
        .style(|_t| container::Style {
            background: Some(Color::from_rgb(0.06, 0.06, 0.06).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        let instruction = Text::new("5-Band Harman — Enter values from AutoEQ.app")
            .size(12).font(bold_font()).color(AMBER);

        let eq_section = column![
            section_header("EQ"),
            row![
                self.band_column(0, params), self.band_column(1, params), self.band_column(2, params),
                self.band_column(3, params), self.band_column(4, params),
            ].spacing(6).align_y(Alignment::Center),
        ].spacing(6).align_x(Alignment::Center);

        let blend_reset_section = column![
            column![
                Text::new("HARMAN BLEND").size(10).font(bold_font()).color(Color::from_rgb(0.7, 0.7, 0.7)),
                knob_gesture("", self.params.blend.raw_target() as f32, 0.0, 100.0, 100.0, |g| Message::Plugin(AetherMsg::BlendGesture(g))),
            ].spacing(2).align_x(Alignment::Center),
            Space::new().height(Length::Fixed(56.0)),
            output_tools_strip(Message::Plugin(AetherMsg::ResetAll)),
        ].spacing(4).align_x(Alignment::Center);

        let crossfeed_section = column![
            section_header("CROSSFEED"),
            row![
                knob_gesture_curved("ANGLE", "°", self.params.cf_angle.raw_target() as f32, 60.0, angle_from_ui, |g| Message::Plugin(AetherMsg::CfAngleGesture(g))),
                knob_gesture("AMOUNT", self.params.cf_amount.raw_target() as f32, 0.0, 100.0, 0.0, |g| Message::Plugin(AetherMsg::CfAmountGesture(g))),
            ].spacing(6),
            Space::new().height(Length::Fixed(10.0)),
            toggle_button(crate::realism_label(self.params.cf_realism.value_i32()), true, Message::Plugin(AetherMsg::RealismCycled)),
        ].spacing(4).align_x(Alignment::Center);

        let io_section = column![
            self.input_reader(),
            Space::new().height(Length::Fixed(35.0)),
            knob_gesture_bipolar("GAIN", self.params.gain.raw_target() as f32, -12.0, 12.0, 0.0, |g| Message::Plugin(AetherMsg::GainGesture(g))),
        ].spacing(2).align_x(Alignment::Center);

        let sep = || container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fixed(185.0))
            .style(|_t: &_| container::Style { background: Some(Color::from_rgb(0.12, 0.16, 0.16).into()), ..Default::default() });

        let body_cols = container(row![
            container(eq_section).center_x(Length::Fixed(350.0)), sep(),
            container(blend_reset_section).center_x(Length::Fixed(104.0)), sep(),
            container(crossfeed_section).center_x(Length::Fixed(131.0)), sep(),
            container(io_section).center_x(Length::Fixed(106.0)),
        ].spacing(0).align_y(Alignment::Start)).width(Length::Fill).center_x(Length::Fill);

        let body = container(column![curve, Space::new().height(Length::Fixed(4.0)), instruction, Space::new().height(Length::Fixed(6.0)), body_cols].spacing(0).padding(12))
            .width(Length::Fill).height(Length::Fill)
            .style(|_t| container::Style { background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()), ..Default::default() });

        column![header, body].into()
    }
}
