//! Vizia port of Aether's Iced editor. Architecture note:
//!
//! Vizia is retained-mode + fine-grained reactive. `Binding::new(cx, signal, |cx| {...})`
//! rebuilds its subtree whenever `signal` changes. So telemetry (input peak,
//! preset refresh) lives in a `Signal<Telemetry>` updated by the `Ticker` View
//! every ~33ms. EQ text inputs and KnobViews are built once outside any Binding,
//! so typing and dragging survive across ticks.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use std::path::PathBuf;

use vizia::prelude::*;
use vizia::vg;

use shared_analysis::SharedState;
use shared_dsp::Biquad;
use truce_vizia::ParamLens;

use crate::aether_canvas::EqCurveView;
use shared_ui::{Gesture, KnobView};
use crate::{AetherParams, AetherParamsParamId};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn col(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color::rgba((r.clamp(0.0, 1.0) * 255.0) as u8, (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8, (a.clamp(0.0, 1.0) * 255.0) as u8)
}
fn rgb(r: f32, g: f32, b: f32) -> Color { col(r, g, b, 1.0) }
const AMBER: Color = Color::rgb(255, 140, 26);

// ─── Band defs + helpers ────────────────────────────────────────────────────

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const BAND_DEF: [(f32, f32, i32); 5] = [
    (105.0, 0.7, 1), (300.0, 1.0, 2), (1200.0, 1.0, 2), (4000.0, 1.0, 2), (10000.0, 0.7, 3),
];

fn eq_freq_val(params: &AetherParams, i: usize) -> f32 {
    [&params.eq1_freq, &params.eq2_freq, &params.eq3_freq, &params.eq4_freq, &params.eq5_freq][i].raw_target() as f32
}
fn eq_gain_val(params: &AetherParams, i: usize) -> f32 {
    [&params.eq1_gain, &params.eq2_gain, &params.eq3_gain, &params.eq4_gain, &params.eq5_gain][i].raw_target() as f32
}
fn eq_q_val(params: &AetherParams, i: usize) -> f32 {
    [&params.eq1_q, &params.eq2_q, &params.eq3_q, &params.eq4_q, &params.eq5_q][i].raw_target() as f32
}
fn eq_type_val(params: &AetherParams, i: usize) -> i32 {
    [&params.eq1_type, &params.eq2_type, &params.eq3_type, &params.eq4_type, &params.eq5_type][i].value_i32()
}

fn set_eq_freq(lens: &ParamLens<AetherParams>, i: usize, v: f32) {
    let id = [AetherParamsParamId::Eq1Freq, AetherParamsParamId::Eq2Freq, AetherParamsParamId::Eq3Freq, AetherParamsParamId::Eq4Freq, AetherParamsParamId::Eq5Freq][i];
    let norm = ((v / 20.0).log10() / 3.0).clamp(0.0, 1.0);
    lens.automate(id, norm as f64);
}
fn set_eq_gain(lens: &ParamLens<AetherParams>, i: usize, v: f32) {
    let id = [AetherParamsParamId::Eq1Gain, AetherParamsParamId::Eq2Gain, AetherParamsParamId::Eq3Gain, AetherParamsParamId::Eq4Gain, AetherParamsParamId::Eq5Gain][i];
    let norm = ((v + 12.0) / 24.0).clamp(0.0, 1.0);
    lens.automate(id, norm as f64);
}
fn set_eq_q(lens: &ParamLens<AetherParams>, i: usize, v: f32) {
    let id = [AetherParamsParamId::Eq1Q, AetherParamsParamId::Eq2Q, AetherParamsParamId::Eq3Q, AetherParamsParamId::Eq4Q, AetherParamsParamId::Eq5Q][i];
    let norm = ((v / 0.3).log10() / (8.0_f32 / 0.3).log10()).clamp(0.0, 1.0);
    lens.automate(id, norm as f64);
}
fn set_eq_type(lens: &ParamLens<AetherParams>, i: usize, v: i32) {
    let id = [AetherParamsParamId::Eq1Type, AetherParamsParamId::Eq2Type, AetherParamsParamId::Eq3Type, AetherParamsParamId::Eq4Type, AetherParamsParamId::Eq5Type][i];
    lens.automate(id, v as f64 / 3.0);
}

fn eq_curve_points(params: &AetherParams, sr: f32) -> Vec<(f32, f32)> {
    let mut bands: [Biquad; 5] = std::array::from_fn(|_| Biquad::new());
    for i in 0..5 {
        crate::set_band(&mut bands[i], eq_type_val(params, i), eq_freq_val(params, i), eq_gain_val(params, i), eq_q_val(params, i), sr);
    }
    const N: usize = 240;
    (0..N).map(|i| {
        let f = 20.0f32 * 1000.0f32.powf(i as f32 / (N - 1) as f32);
        (i as f32 / (N - 1) as f32, bands.iter().map(|b| b.magnitude_db(f, sr)).sum())
    }).collect()
}

// ─── Preset types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AetherProfile {
    pub name: String,
    pub bands: [(i32, f32, f32, f32); 5],
    pub cf_angle: f32, pub cf_amount: f32, pub cf_realism: i32,
    pub blend: f32, pub gain: f32,
}

pub(crate) fn harman_flat_profile() -> AetherProfile {
    AetherProfile { name: "Harman Flat".into(),
        bands: [(1,105.0,0.0,0.7),(2,300.0,0.0,1.0),(2,1200.0,0.0,1.0),(2,4000.0,0.0,1.0),(3,10000.0,0.0,0.7)],
        cf_angle:60.0, cf_amount:0.0, cf_realism:0, blend:100.0, gain:0.0 }
}

fn default_presets() -> Vec<(String, Option<PathBuf>, AetherProfile)> {
    vec![("Harman Flat".into(), None, harman_flat_profile())]
}

// ─── Preset parser ───────────────────────────────────────────────────────────

fn parse_aether_preset(content: &str) -> Option<AetherProfile> {
    match shared_analysis::preset_plugin_name(content).as_deref() { Some("aether") => {} _ => return None }
    let mut bands = [(1i32,105.0f32,0.0f32,0.7f32);5];
    let mut cf_angle=60.0f32; let mut cf_amount=0.0f32; let mut cf_realism=0i32;
    let mut blend=100.0f32; let mut gain=0.0f32; let mut name=String::new();
    let mut has_freq=[false;5]; let mut has_gain=[false;5];
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('|') {
            let parts: Vec<&str> = t.split('|').map(|s|s.trim()).collect();
            if parts.len()>=4 {
                match parts[1].to_lowercase().as_str() {
                    s if s.starts_with("eq")&&s.contains("type") => {
                        if let Some(bi)=s.chars().find(|c|c.is_ascii_digit()).and_then(|c|c.to_digit(10)) {
                            let idx=(bi as usize).saturating_sub(1).min(4);
                            bands[idx].0=match parts[2]{"LSC"|"LS"=>1,"PK"|"PEQ"=>2,"HSC"|"HS"=>3,_=>0};
                        }
                    }
                    s if s.starts_with("eq")&&s.contains("freq") => {
                        if let Some(bi)=s.chars().find(|c|c.is_ascii_digit()).and_then(|c|c.to_digit(10)) {
                            let idx=(bi as usize).saturating_sub(1).min(4);
                            if let Ok(v)=parts[2].parse(){bands[idx].1=v;has_freq[idx]=true;}
                        }
                    }
                    s if s.starts_with("eq")&&s.contains("gain") => {
                        if let Some(bi)=s.chars().find(|c|c.is_ascii_digit()).and_then(|c|c.to_digit(10)) {
                            let idx=(bi as usize).saturating_sub(1).min(4);
                            if let Ok(v)=parts[2].parse(){bands[idx].2=v;has_gain[idx]=true;}
                        }
                    }
                    s if s.starts_with("eq")&&s.contains('q') => {
                        if let Some(bi)=s.chars().find(|c|c.is_ascii_digit()).and_then(|c|c.to_digit(10)) {
                            let idx=(bi as usize).saturating_sub(1).min(4);
                            if let Ok(v)=parts[2].parse(){bands[idx].3=v;}
                        }
                    }
                    "crossfeed angle"=>{if let Ok(v)=parts[2].parse(){cf_angle=v;}}
                    "crossfeed amount"=>{if let Ok(v)=parts[2].parse(){cf_amount=v;}}
                    "crossfeed realism"=>{cf_realism=match parts[2]{"LIFELIKE"=>1,"HYPERREAL"|"HYPERREALISTIC"=>2,_=>0};}
                    "blend"=>{if let Ok(v)=parts[2].parse(){blend=v;}}
                    "gain"=>{if let Ok(v)=parts[2].parse(){gain=v;}}
                    _=>{}
                }
            }
        }
        if t.starts_with("# ")&&!t.starts_with("## "){name=t.trim_start_matches("# ").trim().to_string();}
    }
    if has_freq.iter().all(|&h|h)&&has_gain.iter().all(|&h|h) {
        Some(AetherProfile{name,bands,cf_angle,cf_amount,cf_realism,blend,gain})
    }else{None}
}

fn scan_aether_presets(dir: &std::path::Path) -> Vec<(String, PathBuf, AetherProfile)> {
    let mut v=Vec::new();
    if let Ok(entries)=std::fs::read_dir(dir){for e in entries.flatten(){let p=e.path();
        if p.extension().is_some_and(|x|x=="md")&& let Ok(c)=std::fs::read_to_string(&p)
            && let Some(mut pf)=parse_aether_preset(&c){
                if pf.name.is_empty(){pf.name=p.file_stem().and_then(|s|s.to_str()).unwrap_or("Unnamed").to_string();}
                v.push((pf.name.clone(),p,pf));
            }
    }}
    v
}

fn apply_profile(_params: &AetherParams, lens: &ParamLens<AetherParams>, p: &AetherProfile) {
    for i in 0..5 { let(tc,fc,gn,q)=p.bands[i]; set_eq_freq(lens,i,fc);set_eq_gain(lens,i,gn);set_eq_q(lens,i,q);set_eq_type(lens,i,tc); }
    lens.automate(AetherParamsParamId::Blend, (p.blend as f64 / 100.0).clamp(0.0, 1.0));
    lens.automate(AetherParamsParamId::CfAngle, ((p.cf_angle as f64 - 30.0) / 45.0).clamp(0.0, 1.0));
    lens.automate(AetherParamsParamId::CfAmount, (p.cf_amount as f64 / 100.0).clamp(0.0, 1.0));
    lens.automate(AetherParamsParamId::CfRealism, p.cf_realism as f64 / 2.0);
    lens.automate(AetherParamsParamId::Gain, ((p.gain as f64 + 12.0) / 24.0).clamp(0.0, 1.0));
}

fn save_last_preset(vault_path: &Option<String>, name: &str) {
    let mut cfg = shared_analysis::load_config("Aether");
    cfg.vault_path = vault_path.clone();
    cfg.last_preset = Some(name.to_string());
    let _ = shared_analysis::save_config("Aether", &cfg);
}

fn resolve_last_preset(presets: &[(String, Option<PathBuf>, AetherProfile)], config: &shared_analysis::PluginConfig) -> Option<AetherProfile> {
    let name = config.last_preset.as_ref()?;
    presets.iter().find(|(n,_,_)| n == name).map(|(_,_,p)| p.clone())
}

fn find_profile(name: &str, vault_path: &Option<String>) -> Option<AetherProfile> {
    for (n, _, p) in default_presets() { if n == name { return Some(p); } }
    if let Some(vp) = vault_path {
        for (n, _, p) in scan_aether_presets(std::path::Path::new(vp)) { if n == name { return Some(p); } }
    }
    None
}

fn build_profile_md(params: &AetherParams) -> String {
    let mut s = String::from("---\nplugin: aether\ntype: preset\n---\n\n> Warning: Do NOT modify column names or table structure.\n\n## Parameter\n\n| Parameter | Wert | Einheit |\n|---|---|---|\n");
    for i in 0..5 {
        s.push_str(&format!("| EQ{} Type | {} | |\n", i+1, crate::band_type_label(eq_type_val(params,i))));
        s.push_str(&format!("| EQ{} Freq | {:.0} | Hz |\n", i+1, eq_freq_val(params,i)));
        s.push_str(&format!("| EQ{} Gain | {:.1} | dB |\n", i+1, eq_gain_val(params,i)));
        s.push_str(&format!("| EQ{} Q | {:.2} | |\n", i+1, eq_q_val(params,i)));
    }
    s.push_str(&format!("| Crossfeed Angle | {:.0} | ° |\n", params.cf_angle.raw_target() as f32));
    s.push_str(&format!("| Crossfeed Amount | {:.0} | % |\n", params.cf_amount.raw_target() as f32));
    s.push_str(&format!("| Crossfeed Realism | {} | |\n", crate::realism_label(params.cf_realism.value_i32())));
    s.push_str(&format!("| Blend | {:.0} | % |\n", params.blend.raw_target() as f32));
    s.push_str(&format!("| Gain | {:.1} | dB |\n", params.gain.raw_target() as f32));
    s
}

// ─── Telemetry ─────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Telemetry {
    curve_points: Vec<(f32, f32)>,
    in_peak: f32,
    in_peak_hold: f32,
    preset_names: Vec<String>,
}

struct TickAccum {
    in_peak_hold: f32,
    in_peak_hold_ticks: u32,
    preset_refresh_counter: u32,
}

// ─── Ticker ────────────────────────────────────────────────────────────────

struct Ticker {
    params: Arc<AetherParams>,
    shared: Arc<SharedState>,
    telemetry: Signal<Telemetry>,
    accum: Rc<RefCell<TickAccum>>,
    vault_path: Signal<Option<String>>,
    last_tick: RefCell<Instant>,
}

impl Ticker {
    fn new(cx: &mut Context, params: Arc<AetherParams>, shared: Arc<SharedState>,
           telemetry: Signal<Telemetry>, accum: Rc<RefCell<TickAccum>>,
           vault_path: Signal<Option<String>>) -> Handle<'_, Self> {
        Self { params, shared, telemetry, accum, vault_path, last_tick: RefCell::new(Instant::now()) }.build(cx, |_|{})
    }
}

const TICK_MS: Duration = Duration::from_millis(33);

impl View for Ticker {
    fn element(&self) -> Option<&'static str> { Some("ticker") }
    fn draw(&self, cx: &mut DrawContext, _canvas: &vg::Canvas) {
        let due = { let mut lt = self.last_tick.borrow_mut();
            if Instant::now().duration_since(*lt) >= TICK_MS { *lt = Instant::now(); true } else { false }
        };
        if due {
            let mut acc = self.accum.borrow_mut();
            let in_peak = self.shared.input_peak.load(Ordering::Relaxed);
            if in_peak > acc.in_peak_hold { acc.in_peak_hold = in_peak; acc.in_peak_hold_ticks = 90; }
            else if acc.in_peak_hold_ticks > 0 { acc.in_peak_hold_ticks -= 1; }
            else { acc.in_peak_hold = (acc.in_peak_hold - 0.5).max(in_peak); }
            acc.preset_refresh_counter += 1;

            let sr = self.shared.sample_rate.load(Ordering::Relaxed).max(1.0);
            let curve = eq_curve_points(&self.params, sr);

            let mut preset_names: Vec<String> = default_presets().into_iter().map(|(n,_,_)|n).collect();
            if let Some(ref vp) = self.vault_path.get() {
                for (name, _, _) in scan_aether_presets(std::path::Path::new(&vp)) { preset_names.push(name); }
            }

            self.telemetry.update(|t| {
                t.curve_points = curve; t.in_peak = in_peak; t.in_peak_hold = acc.in_peak_hold; t.preset_names = preset_names;
            });
        }
        cx.needs_redraw();
    }
}

// ─── UI ─────────────────────────────────────────────────────────────────────

/// Small push button used by the SETUP form only (CANCEL).
fn small_button(cx: &mut Context, label: &'static str, on_press: impl Fn(&mut EventContext) + 'static + Send + Sync) {
    Button::new(cx, move |cx| Label::new(cx, label).font_size(11.0))
        .on_press(on_press).width(Pixels(60.0)).height(Pixels(26.0));
}

#[allow(clippy::too_many_arguments)]
pub fn build(cx: &mut Context, lens: ParamLens<AetherParams>, params: Arc<AetherParams>, shared: Arc<SharedState>) {
    let config = shared_analysis::load_config("Aether");
    let mut presets = default_presets();
    let vault_path_init = config.vault_path.clone();
    let vault_path_init_for_signal = vault_path_init.clone();
    if let Some(vp) = &vault_path_init {
        for (name, path, profile) in scan_aether_presets(std::path::Path::new(&vp)) { presets.push((name, Some(path), profile)); }
    }
    let preset_names_init: Vec<String> = presets.iter().map(|(n,_,_)|n.clone()).collect();
    let preset_names_for_signal = preset_names_init.clone();

    let preset_name_signal = Signal::new(config.last_preset.clone().unwrap_or_default());

    if let Some(last) = resolve_last_preset(&presets, &config) {
        apply_profile(&params, &lens, &last);
    }

    let vault_path_signal = Signal::new(vault_path_init_for_signal);
    let show_setup = Signal::new(false);
    let vault_path_input = Signal::new(config.vault_path.unwrap_or_default());

    let telemetry = Signal::new(Telemetry { curve_points: Vec::new(), in_peak: -90.0, in_peak_hold: -90.0, preset_names: preset_names_for_signal });
    let accum = Rc::new(RefCell::new(TickAccum { in_peak_hold: -90.0, in_peak_hold_ticks: 0, preset_refresh_counter: 0 }));
    let ui_gen = Signal::new(0u32);

    Ticker::new(cx, params.clone(), shared.clone(), telemetry, accum, vault_path_signal)
        .width(Pixels(1.0)).height(Pixels(1.0));

    let lens_for_body = lens.clone();
    let params_for_body = params.clone();
    let shared_for_body = shared.clone();
    let bypass_sig = lens.value_signal(AetherParamsParamId::Bypass);
    VStack::new(cx, move |cx| {
        // ── HEADER ──────────────────────────────────────────────────────────
        HStack::new(cx, move |cx| {
            HStack::new(cx, |cx| {
                Label::new(cx, "LX").font_size(20.0).color(rgb(1.0, 0.45, 0.1));
                Label::new(cx, "AUDIOLABS").font_size(20.0).color(Color::white());
                Element::new(cx).width(Pixels(14.0));
                Element::new(cx).width(Pixels(1.0)).height(Pixels(28.0)).background_color(col(0.18, 0.22, 0.22, 1.0));
                Element::new(cx).width(Pixels(14.0));
                VStack::new(cx, |cx| {
                    Label::new(cx, "AETHER").font_size(13.0).color(rgb(1.0, 0.65, 0.3));
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

            // Preset Dropdown with arrow + Textbox
            let preset_opts = Signal::new(preset_names_init.clone());
            let names_for_popup = preset_opts.clone();
            let selected_preset_name = preset_name_signal;
            let lens_preset = lens.clone();
            let params_preset = params.clone();
            let vp_p = vault_path_signal;
            // Common look for the preset dropdown trigger and the adjacent name textbox.
            const PRESET_W: f32 = 130.0;
            const PRESET_H: f32 = 22.0;
            const PRESET_BG: Color = Color::rgb(230, 230, 230);
            const PRESET_BORDER: Color = Color::rgb(140, 140, 140);
            const PRESET_TEXT: Color = Color::rgb(40, 40, 40);
            const PRESET_ARROW: Color = Color::rgb(100, 100, 100);

            Dropdown::new(
                cx,
                // Trigger: styled box showing current preset name + down arrow
                move |cx| {
                    let trigger_text = Memo::new(move |_| {
                        let n = selected_preset_name.get();
                        if n.is_empty() { "Select preset".to_string() } else { n }
                    });
                    HStack::new(cx, move |cx| {
                        Label::new(cx, trigger_text)
                            .font_size(11.0)
                            .color(PRESET_TEXT)
                            .hoverable(false);
                        Element::new(cx).width(Stretch(1.0)).hoverable(false);
                        Label::new(cx, "▼")
                            .font_size(8.0)
                            .color(PRESET_ARROW)
                            .hoverable(false);
                    })
                    .width(Pixels(PRESET_W))
                    .height(Pixels(PRESET_H))
                    .padding(Pixels(4.0))
                    .background_color(PRESET_BG)
                    .border_color(PRESET_BORDER)
                    .border_width(Pixels(1.0))
                    .corner_radius(Pixels(2.0))
                    .alignment(Alignment::Center)
                    .on_press(|cx| cx.emit(PopupEvent::Switch));
                },
                // Popup: scrollable list of presets
                move |cx| {
                    let names = names_for_popup.get();
                    let vp = vp_p;
                    let params = params_preset.clone();
                    let lens = lens_preset.clone();
                    let sel_name = selected_preset_name;
                    ScrollView::new(cx, move |cx| {
                        VStack::new(cx, move |cx| {
                            for name in &names {
                                let name_clone = name.clone();
                                let vp_c = vp.clone();
                                let params_c = params.clone();
                                let lens_c = lens.clone();
                                let sel_c = sel_name;
                                let name_for_press = name_clone.clone();
                                HStack::new(cx, move |cx| {
                                    Label::new(cx, name_clone)
                                        .font_size(11.0)
                                        .color(Color::black())
                                        .hoverable(false);
                                })
                                .width(Pixels(PRESET_W))
                                .height(Pixels(20.0))
                                .padding(Pixels(4.0))
                                .background_color(Color::white())
                                .alignment(Alignment::Center)
                                .on_press(move |cx| {
                                    let n = name_for_press.clone();
                                    if let Some(pf) = find_profile(&n, &vp_c.get()) {
                                        apply_profile(&params_c, &lens_c, &pf);
                                    }
                                    sel_c.set(n.clone());
                                    save_last_preset(&vp_c.get(), &n);
                                    cx.emit(PopupEvent::Close);
                                });
                            }
                        })
                        .width(Auto)
                        .height(Auto);
                    })
                    .width(Pixels(PRESET_W + 16.0))
                    .height(Auto)
                    .max_height(Pixels(160.0))
                    .background_color(Color::white());
                },
            )
            .width(Pixels(PRESET_W))
            .height(Pixels(PRESET_H));

            let preset_name_edit = preset_name_signal;
            Textbox::new(cx, preset_name_signal)
                .on_edit(move |_cx, text| preset_name_edit.set(text))
                .width(Pixels(PRESET_W))
                .height(Pixels(PRESET_H))
                .padding(Pixels(4.0))
                .font_size(11.0)
                .background_color(PRESET_BG)
                .border_color(PRESET_BORDER);

            Element::new(cx).width(Pixels(6.0));

            // Buttons: SAVE, SETUP, BYPASS — all from shared-ui
            let params_save = params.clone();
            let vp_save = vault_path_signal;
            shared_ui::push_button_big(cx, "SAVE", move |_cx| {
                let name = preset_name_signal.get();
                if !name.is_empty() {
                    let md = build_profile_md(&params_save);
                    let dir = match vp_save.get() {
                        Some(vp) if !vp.is_empty() => std::path::PathBuf::from(vp),
                        _ => shared_analysis::get_plugin_dir("Aether").join("presets"),
                    };
                    let _ = std::fs::create_dir_all(&dir);
                    let fp = dir.join(format!("{name}.md"));
                    if std::fs::write(&fp, md).is_ok() {
                        save_last_preset(&vp_save.get(), &name);
                        let mut names = preset_opts.get();
                        if !names.contains(&name) {
                            names.push(name.clone());
                            preset_opts.set(names.clone());
                        }
                        if let Some(idx) = names.iter().position(|n| n == &name) { preset_name_signal.set(names[idx].clone()); }
                    }
                }
            });

            shared_ui::push_button_big(cx, "SETUP", move |_cx| { show_setup.update(|v| *v = !*v); });

            // BYPASS — standard shared-ui toggle, amber when active
            {
                let sig = bypass_sig;
                let lens_bypass = lens.clone();
                Binding::new(cx, sig, move |cx| {
                    let active = lens_bypass.get(AetherParamsParamId::Bypass) > 0.5;
                    let lens_bypass = lens_bypass.clone();
                    shared_ui::toggle_button_big(cx, "BYPASS", active, move |_cx| {
                        let now = lens_bypass.get(AetherParamsParamId::Bypass) <= 0.5;
                        let norm = if now { 1.0 } else { 0.0 };
                        lens_bypass.automate(AetherParamsParamId::Bypass, norm);
                        sig.set(norm as f32);
                    });
                });
            }
        })
        .width(Stretch(1.0)).height(Pixels(50.0)).padding(Pixels(10.0))
        .alignment(Alignment::Center).background_color(rgb(0.08,0.08,0.08)).horizontal_gap(Pixels(4.0));

        // Setup or main
        let ui_gen_for_binding = ui_gen.clone();
        Binding::new(cx, show_setup, move |cx| {
            if show_setup.get() { build_setup(cx, vault_path_input, vault_path_signal, show_setup); }
            else {
                let telemetry_c = telemetry.clone();
                let lens_c = lens_for_body.clone();
                let params_c = params_for_body.clone();
                let shared_c = shared_for_body.clone();
                let ui_gen_c = ui_gen.clone();
                Binding::new(cx, ui_gen_for_binding, move |cx| {
                    build_main(cx, telemetry_c.clone(), lens_c.clone(), params_c.clone(), shared_c.clone(), ui_gen_c.clone(), bypass_sig);
                });
            }
        });
    })
    .width(Pixels(720.0)).height(Pixels(395.0)).background_color(rgb(0.09,0.09,0.09));
}

fn build_setup(cx: &mut Context, vault_input: Signal<String>, vault_path: Signal<Option<String>>, show_setup: Signal<bool>) {
    VStack::new(cx, move |cx| {
        VStack::new(cx, move |cx| {
            Label::new(cx, "LX AUDIOLABS - SETUP").font_size(18.0).color(Color::white());
            Label::new(cx, "Configure your Vault path for Aether:").font_size(12.0).color(Color::white());
            Textbox::new(cx, vault_input)
                .placeholder("Enter Vault absolute path...")
                .on_edit(move |_cx, text| vault_input.set(text))
                .width(Stretch(1.0));
            HStack::new(cx, move |cx| {
                small_button(cx, "SAVE", move |_cx| {
                    let p = vault_input.get().trim().to_string();
                    let new = if p.is_empty() { None } else { Some(p) };
                    vault_path.set(new.clone());
                    let mut cfg = shared_analysis::load_config("Aether");
                    cfg.vault_path = new;
                    let _ = shared_analysis::save_config("Aether", &cfg);
                    show_setup.set(false);
                });
                small_button(cx, "CANCEL", move |_cx| show_setup.set(false));
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
    })
    .width(Stretch(1.0))
    .height(Stretch(1.0))
    .alignment(Alignment::Center)
    .background_color(rgb(0.08,0.08,0.08));
}

fn build_main(cx: &mut Context, telemetry: Signal<Telemetry>, lens: ParamLens<AetherParams>, params: Arc<AetherParams>, _shared: Arc<SharedState>, ui_gen: Signal<u32>, bypass_sig: Signal<f32>) {
    VStack::new(cx, move |cx| {
        // EQ curve
        Binding::new(cx, telemetry, move |cx| {
            let t = telemetry.get();
            EqCurveView::new(cx, t.curve_points).width(Stretch(1.0)).height(Pixels(90.0));
        });

        Label::new(cx, "5-Band Harman — Enter values from AutoEQ.app").font_size(11.0).color(AMBER);

        HStack::new(cx, move |cx| {
            build_eq_section(cx, &lens, &params);
            separator(cx);
            build_blend_reset(cx, &lens, &params, ui_gen, bypass_sig);
            separator(cx);
            build_crossfeed(cx, &lens, &params);
            separator(cx);
            build_io_section(cx, telemetry, &lens, &params);
        }).width(Stretch(1.0)).height(Stretch(1.0));
    })
    .width(Stretch(1.0)).height(Stretch(1.0)).padding(Pixels(12.0)).vertical_gap(Pixels(4.0)).background_color(rgb(0.08,0.08,0.08));
}

fn build_eq_section(cx: &mut Context, lens: &ParamLens<AetherParams>, params: &Arc<AetherParams>) {
    let lens = lens.clone(); let params = params.clone();
    VStack::new(cx, move |cx| {
        Label::new(cx, "EQ").font_size(10.0).color(col(0.75,0.75,0.75,1.0));
        HStack::new(cx, move |cx| {
            for i in 0..5usize { build_band_column(cx, i, &lens, &params); }
        }).width(Auto).horizontal_gap(Pixels(6.0)).alignment(Alignment::Center);
    }).width(Pixels(358.0)).height(Auto).padding(Pixels(4.0)).vertical_gap(Pixels(4.0)).alignment(Alignment::Center);
}

fn build_band_column(cx: &mut Context, i: usize, lens: &ParamLens<AetherParams>, params: &Arc<AetherParams>) {
    let l = lens.clone(); let p = params.clone();
    VStack::new(cx, move |cx| {
        let type_signal = Signal::new(eq_type_val(&p, i));
        let l1 = l.clone();
        Button::new(cx, move |cx| {
            Label::new(cx, Memo::new(move |_| crate::band_type_label(type_signal.get()))).font_size(9.0)
        })
            .on_press(move |_cx| { let n = (type_signal.get() + 1) % 4; set_eq_type(&l1, i, n); type_signal.set(n); })
            .width(Pixels(56.0)).height(Pixels(shared_ui::BUTTON_HEIGHT))
            .background_color(Memo::new(move |_| {
                if type_signal.get() != 0 { shared_ui::AMBER } else { shared_ui::IDLE_BG }
            }));

        Label::new(cx, "FREQ").font_size(8.0).color(col(0.6,0.6,0.6,1.0));
        let l2 = l.clone();
        let freq_s = Signal::new(format!("{:.0}", eq_freq_val(&p, i)));
        Textbox::new(cx, freq_s).on_edit(move |_cx, s| {
            if let Ok(v) = s.trim().parse::<f32>() { set_eq_freq(&l2, i, v.clamp(FREQ_MIN, FREQ_MAX)); }
        }).width(Pixels(56.0)).height(Pixels(20.0)).font_size(11.0);

        Label::new(cx, "GAIN").font_size(8.0).color(col(0.6,0.6,0.6,1.0));
        let l3 = l.clone();
        let gain_s = Signal::new(format!("{:.1}", eq_gain_val(&p, i)));
        Textbox::new(cx, gain_s).on_edit(move |_cx, s| {
            if let Ok(v) = s.trim().parse::<f32>() { set_eq_gain(&l3, i, v.clamp(-12.0, 12.0)); }
        }).width(Pixels(56.0)).height(Pixels(20.0)).font_size(11.0);

        Label::new(cx, "Q").font_size(8.0).color(col(0.6,0.6,0.6,1.0));
        let l4 = l;
        let q_s = Signal::new(format!("{:.2}", eq_q_val(&p, i)));
        Textbox::new(cx, q_s).on_edit(move |_cx, s| {
            if let Ok(v) = s.trim().parse::<f32>() { set_eq_q(&l4, i, v.clamp(0.3, 8.0)); }
        }).width(Pixels(56.0)).height(Pixels(20.0)).font_size(11.0);
    }).width(Auto).height(Auto).vertical_gap(Pixels(2.0)).alignment(Alignment::Center);
}

fn build_blend_reset(cx: &mut Context, lens: &ParamLens<AetherParams>, params: &Arc<AetherParams>, ui_gen: Signal<u32>, bypass_sig: Signal<f32>) {
    let l = lens.clone(); let p = params.clone();
    VStack::new(cx, move |cx| {
        Label::new(cx, "HARMAN BLEND").font_size(9.0).color(col(0.7,0.7,0.7,1.0));
        let blend_display = Signal::new(p.blend.raw_target() as f32);
        let l1 = l.clone();
        KnobView::new(cx, p.blend.raw_target() as f32 / 100.0, 1.0, 0.0, 100.0, false, move |_cx, g| {
            if let Gesture::Change(v) = g {
                l1.automate(AetherParamsParamId::Blend, (v/100.0).clamp(0.0,1.0) as f64);
                blend_display.set(v.clamp(0.0, 100.0));
            }
        }).width(Pixels(40.0)).height(Pixels(40.0));
        Binding::new(cx, blend_display, move |cx| {
            let v = blend_display.get();
            Label::new(cx, format!("{v:.0}%")).font_size(9.0).color(col(0.8,0.8,0.8,1.0));
        });
        Element::new(cx).height(Pixels(60.0));

        let lr = l.clone();
        shared_ui::danger_button(cx, "RESET", move |_cx| {
            for i in 0..5 { let(fd,qd,td)=BAND_DEF[i]; set_eq_freq(&lr,i,fd);set_eq_gain(&lr,i,0.0);set_eq_q(&lr,i,qd);set_eq_type(&lr,i,td); }
            lr.automate(AetherParamsParamId::Blend, 1.0);
            lr.automate(AetherParamsParamId::CfAngle, 30.0/45.0);
            lr.automate(AetherParamsParamId::CfAmount, 0.0);
            lr.automate(AetherParamsParamId::CfRealism, 0.0);
            lr.automate(AetherParamsParamId::Gain, 0.5);
            lr.automate(AetherParamsParamId::Bypass, 0.0);
            bypass_sig.set(0.0);
            ui_gen.update(|g| *g = g.wrapping_add(1));
        })
        .width(Pixels(60.0));
    }).width(Pixels(104.0)).height(Auto).vertical_gap(Pixels(4.0)).alignment(Alignment::Center);
}

fn build_crossfeed(cx: &mut Context, lens: &ParamLens<AetherParams>, params: &Arc<AetherParams>) {
    let l1 = lens.clone(); let p1 = params.clone();
    let l2 = lens.clone(); let p2 = params.clone();
    let l3 = lens.clone(); let p3 = params.clone();
    VStack::new(cx, move |cx| {
        Label::new(cx, "CROSSFEED").font_size(10.0).color(col(0.75,0.75,0.75,1.0));
        HStack::new(cx, move |cx| {
            VStack::new(cx, move |cx| {
                Label::new(cx, "ANGLE").font_size(10.0).color(col(0.7,0.7,0.7,1.0));
                let angle_display = Signal::new(p1.cf_angle.raw_target() as f32);
                let la = l1.clone();
                KnobView::new(cx, (p1.cf_angle.raw_target() as f32 - 30.0) / 45.0, (60.0-30.0)/45.0, 30.0, 75.0, false,
                    move |_cx, g| {
                        if let Gesture::Change(v) = g {
                            la.automate(AetherParamsParamId::CfAngle, ((v-30.0)/45.0).clamp(0.0,1.0) as f64);
                            angle_display.set(v.clamp(30.0, 75.0));
                        }
                    }).width(Pixels(40.0)).height(Pixels(40.0));
                Binding::new(cx, angle_display, move |cx| {
                    let v = angle_display.get();
                    Label::new(cx, format!("{v:.0}°")).font_size(9.0).color(col(0.8,0.8,0.8,1.0));
                });
            }).width(Auto).vertical_gap(Pixels(2.0)).alignment(Alignment::Center);

            VStack::new(cx, move |cx| {
                Label::new(cx, "AMOUNT").font_size(10.0).color(col(0.7,0.7,0.7,1.0));
                let amount_display = Signal::new(p2.cf_amount.raw_target() as f32);
                let lb = l2.clone();
                KnobView::new(cx, p2.cf_amount.raw_target() as f32 / 100.0, 0.0, 0.0, 100.0, false,
                    move |_cx, g| {
                        if let Gesture::Change(v) = g {
                            lb.automate(AetherParamsParamId::CfAmount, (v/100.0).clamp(0.0,1.0) as f64);
                            amount_display.set(v.clamp(0.0, 100.0));
                        }
                    }).width(Pixels(40.0)).height(Pixels(40.0));
                Binding::new(cx, amount_display, move |cx| {
                    let v = amount_display.get();
                    Label::new(cx, format!("{v:.0}%")).font_size(9.0).color(col(0.8,0.8,0.8,1.0));
                });
            }).width(Auto).vertical_gap(Pixels(2.0)).alignment(Alignment::Center);
        }).width(Auto).horizontal_gap(Pixels(6.0)).alignment(Alignment::Center);

        Element::new(cx).height(Pixels(10.0));

        let realism_signal = Signal::new(p3.cf_realism.value_i32());
        Button::new(cx, move |cx| {
            Label::new(cx, Memo::new(move |_| crate::realism_label(realism_signal.get()))).font_size(9.0)
        })
            .on_press(move |_cx| { let n = (realism_signal.get() + 1) % 3; l3.automate(AetherParamsParamId::CfRealism, n as f64 / 2.0); realism_signal.set(n); })
            .width(Pixels(110.0)).height(Pixels(shared_ui::BUTTON_HEIGHT))
            .background_color(Memo::new(move |_| {
                if realism_signal.get() != 0 { shared_ui::AMBER } else { shared_ui::IDLE_BG }
            }));
    }).width(Pixels(131.0)).height(Auto).vertical_gap(Pixels(4.0)).alignment(Alignment::Center);
}

fn build_io_section(cx: &mut Context, telemetry: Signal<Telemetry>, lens: &ParamLens<AetherParams>, params: &Arc<AetherParams>) {
    let l = lens.clone(); let p = params.clone();
    VStack::new(cx, move |cx| {
        Binding::new(cx, telemetry, move |cx| {
            let t = telemetry.get();
            VStack::new(cx, move |cx| {
                Label::new(cx, "INPUT").font_size(9.0).color(col(0.7,0.7,0.7,1.0));
                let fast = if t.in_peak <= -90.0 { "--".to_string() } else { format!("{:.1} dB", t.in_peak) };
                Label::new(cx, fast).font_size(14.0).color(col(0.85,0.85,0.85,1.0));
                let hold = if t.in_peak_hold <= -90.0 { "--".to_string() } else { format!("pk {:.1} dB", t.in_peak_hold) };
                Label::new(cx, hold).font_size(10.0).color(AMBER);
            }).width(Auto).vertical_gap(Pixels(2.0)).alignment(Alignment::Center);
        });

        Element::new(cx).height(Pixels(35.0));

        Label::new(cx, "GAIN").font_size(14.0).color(col(0.7,0.7,0.7,1.0));
        let gain_display = Signal::new(p.gain.raw_target() as f32);
        let l1 = l.clone();
        KnobView::new(cx, (p.gain.raw_target() as f32 + 12.0) / 24.0, 0.5, -12.0, 12.0, true,
            move |_cx, g| {
                if let Gesture::Change(v) = g {
                    l1.automate(AetherParamsParamId::Gain, ((v+12.0)/24.0).clamp(0.0,1.0) as f64);
                    gain_display.set(v.clamp(-12.0, 12.0));
                }
            }).width(Pixels(40.0)).height(Pixels(40.0));
        Binding::new(cx, gain_display, move |cx| {
            let v = gain_display.get();
            Label::new(cx, format!("{v:.1} dB")).font_size(9.0).color(col(0.8,0.8,0.8,1.0));
        });
    }).width(Pixels(106.0)).height(Auto).vertical_gap(Pixels(4.0)).alignment(Alignment::Center);
}

fn separator(cx: &mut Context) {
    Element::new(cx).width(Pixels(1.0)).height(Pixels(185.0)).background_color(col(0.12,0.16,0.16,1.0));
}
