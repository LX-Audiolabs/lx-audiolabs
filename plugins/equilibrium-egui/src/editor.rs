//! Egui prototype UI for Equilibrium (framework-compat evaluation against
//! the shipping truce-vizia/shared-ui port in `plugins/equilibrium`).
//!
//! Deliberately NOT feature-complete - this exists to gauge how egui feels
//! against a real DSP/param surface, not to ship. Everything is wired:
//! 5-band Gain/Width/Pan/Solo, output gain, mono
//! floor, pre-master, auto loud, mono/delta/bypass, reset, goniometer) is
//! wired to the real params and `SharedState` telemetry.
//!
//! egui is immediate-mode: every widget re-reads its bound param/atomic on
//! every frame call, so there's no Signal/Binding/Ticker machinery here at
//! all - `build()` runs once per frame and that's the whole state model.
//!
//! `lx_hslider`/`lx_toggle`/`lx_button` below are direct ports of
//! `shared-ui`'s `HSliderView`/`toggle_button`/`push_button_big` draw+drag
//! logic (`shared-ui/src/widgets.rs`, `shared-ui/src/buttons.rs`) onto
//! `egui::Painter` + `PluginContext::{get_param, set_param, automate}`,
//! instead of using truce-egui's own `param_slider`/`param_toggle` look -
//! keeps the visual language identical to the shipping Vizia UI so the two
//! editors are actually comparable. `param_knob` (truce-egui's built-in) is
//! kept as-is for the knobs - its arc + white-dot indicator already matches
//! what `shared-ui::KnobView` draws closely enough that a port wasn't worth
//! it for a prototype.

use std::path::PathBuf;
use std::sync::{atomic::Ordering, Arc};


use egui::{Color32, FontId, RichText, Sense, Stroke};
use truce_core::editor::{PluginContext, PluginContextReadF32};
use truce_egui::EditorUi;
use truce_egui::theme::{HEADER_BG, HEADER_TEXT};
use truce_egui::widgets::param_knob;

use shared_analysis::{
    Profile, SharedState, DEFAULT_TOLERANCES, SPECTRUM_BINS,
    export_preset_to_markdown, get_plugin_dir, list_custom_presets, load_config, save_config,
};
use crate::{EquilibriumParams, EquilibriumParamsParamId as K};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const GAIN_IDS: [K; 5] = [K::LowGain, K::BassGain, K::MidGain, K::HighMidGain, K::HighGain];
const WIDTH_IDS: [K; 5] = [K::LowWidth, K::BassWidth, K::MidWidth, K::HighMidWidth, K::HighWidth];
const PAN_IDS: [K; 5] = [K::LowPan, K::BassPan, K::MidPan, K::HighMidPan, K::HighPan];
const SOLO_IDS: [K; 5] = [K::SoloLow, K::SoloBass, K::SoloMid, K::SoloHighMid, K::SoloHigh];
const BAND_NAMES: [&str; 5] = ["Sub", "Bass", "Mid", "Pres", "Air"];
const BAND_HZ: [&str; 5] = ["0-80Hz", "80-300Hz", "300Hz-2kHz", "2-6kHz", ">6kHz"];
const TILT: [f32; 5] = [-1.5, 0.0, 1.5, 3.0, 4.5];

const AMBER: Color32 = Color32::from_rgb(255, 115, 26);
const IDLE_BG: Color32 = Color32::from_rgb(38, 38, 38);
const HOVER_BG: Color32 = Color32::from_rgb(64, 64, 64);
const DANGER_BG: Color32 = Color32::from_rgb(51, 20, 20);
const DANGER_TEXT: Color32 = Color32::from_rgb(230, 128, 128);
const DARK_BG: Color32 = Color32::from_rgb(15, 15, 15);

type PresetEntry = (String, Option<PathBuf>, Profile);

/// Persistent editor state for the egui Equilibrium prototype. Holds the
/// preset/vault data so it isn't reloaded from disk every frame.
pub struct EditorState {
    params: Arc<EquilibriumParams>,
    presets: Vec<PresetEntry>,
    vault_path: Option<String>,
    selected_preset: Option<usize>,
    preset_name_input: String,
    vault_path_input: String,
    show_setup: bool,
    prev_snap_active: bool,
    snap_blink_counter: u32,
}

impl EditorState {
    pub fn new(params: Arc<EquilibriumParams>) -> Self {
        let config = load_config("Equilibrium");
        let vault_path = config.vault_path.clone();
        let mut presets = build_factory_presets();
        let custom = list_custom_presets("Equilibrium", vault_path.as_deref());
        for (name, path, profile) in custom {
            presets.push((name, Some(path), profile));
        }

        let selected_preset = if !presets.is_empty() { Some(0) } else { None };
        if let Some(idx) = selected_preset {
            let p = &presets[idx].2;
            for b in 0..5 {
                params.shared.target_levels[b].store(p.bands[b], Ordering::Release);
                params.shared.target_tolerances[b].store(p.tolerances[b], Ordering::Release);
            }
            params.shared.selected_preset_index.store(idx, Ordering::Release);
        }

        Self {
            params,
            presets,
            vault_path: vault_path.clone(),
            selected_preset,
            preset_name_input: String::new(),
            vault_path_input: vault_path.unwrap_or_default(),
            show_setup: false,
            prev_snap_active: false,
            snap_blink_counter: 0,
        }
    }

    fn reload_presets(&mut self) {
        let mut presets = build_factory_presets();
        let custom = list_custom_presets("Equilibrium", self.vault_path.as_deref());
        for (name, path, profile) in custom {
            presets.push((name, Some(path), profile));
        }
        self.presets = presets;
        // Keep selection valid.
        self.selected_preset = self.selected_preset.filter(|&i| i < self.presets.len()).or_else(|| {
            if self.presets.is_empty() { None } else { Some(0) }
        });
    }

    fn select_preset(&mut self, idx: usize) {
        if idx >= self.presets.len() {
            return;
        }
        self.selected_preset = Some(idx);
        let p = &self.presets[idx].2;
        for b in 0..5 {
            self.params.shared.target_levels[b].store(p.bands[b], Ordering::Release);
            self.params.shared.target_tolerances[b].store(p.tolerances[b], Ordering::Release);
        }
        self.params.shared.selected_preset_index.store(idx, Ordering::Release);
    }

    fn save_current_preset(&mut self, ctx: &PluginContext<EquilibriumParams>) {
        let name = self.preset_name_input.trim();
        if name.is_empty() {
            return;
        }
        let dir = match self.vault_path.as_deref() {
            Some(vp) if !vp.is_empty() => PathBuf::from(vp),
            _ => get_plugin_dir("Equilibrium").join("presets"),
        };
        let _ = std::fs::create_dir_all(&dir);
        let safe = name.replace(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_', "");
        let fp = dir.join(format!("{safe}.md"));

        let profile = Profile {
            name: name.to_string(),
            bands: [
                ctx.get_param_plain(K::LowGain) as f32,
                ctx.get_param_plain(K::BassGain) as f32,
                ctx.get_param_plain(K::MidGain) as f32,
                ctx.get_param_plain(K::HighMidGain) as f32,
                ctx.get_param_plain(K::HighGain) as f32,
            ],
            tolerances: DEFAULT_TOLERANCES,
            pans: [
                ctx.get_param_plain(K::LowPan) as f32,
                ctx.get_param_plain(K::BassPan) as f32,
                ctx.get_param_plain(K::MidPan) as f32,
                ctx.get_param_plain(K::HighMidPan) as f32,
                ctx.get_param_plain(K::HighPan) as f32,
            ],
            widths: [
                ctx.get_param_plain(K::LowWidth) as f32,
                ctx.get_param_plain(K::BassWidth) as f32,
                ctx.get_param_plain(K::MidWidth) as f32,
                ctx.get_param_plain(K::HighMidWidth) as f32,
                ctx.get_param_plain(K::HighWidth) as f32,
            ],
            mono_floor_hz: ctx.get_param_plain(K::MonoFloor) as f32,
            ..Profile::default()
        };

        let md = export_preset_to_markdown(&profile);
        if std::fs::write(&fp, &md).is_ok() {
            self.reload_presets();
            self.preset_name_input.clear();
        }
    }

    fn apply_vault_path(&mut self) {
        let path = self.vault_path_input.trim().to_string();
        if path.is_empty() {
            return;
        }
        let mut cfg = load_config("Equilibrium");
        cfg.vault_path = Some(path.clone());
        let _ = save_config("Equilibrium", &cfg);
        self.vault_path = Some(path);
        self.reload_presets();
        self.show_setup = false;
    }

    fn apply_analysis(&self) {
        if !self.params.listen_active.value() {
            return;
        }
        let shared = &self.params.shared;
        let listen_samples = shared.listen_samples.load(Ordering::Acquire);
        if listen_samples <= 100.0 {
            return;
        }
        for b in 0..5 {
            let lvl = shared.listen_levels[b].load(Ordering::Acquire);
            let tol = shared.listen_tolerances[b].load(Ordering::Acquire);
            shared.target_levels[b].store(lvl, Ordering::Release);
            shared.target_tolerances[b].store(tol, Ordering::Release);
        }
    }

    fn reset_analysis(&self) {
        let shared = &self.params.shared;
        shared.reset_analysis.store(true, Ordering::Release);
        shared.listen_samples.store(0.0, Ordering::Release);
        for b in 0..5 {
            shared.listen_levels[b].store(-90.0, Ordering::Release);
            shared.listen_tolerances[b].store(0.0, Ordering::Release);
        }
    }

    fn trigger_snap(&mut self) {
        let shared = &self.params.shared;
        shared.snap_active.store(true, Ordering::Release);
        shared.snap_phase.store(1, Ordering::Release);
        self.prev_snap_active = true;
    }

    /// Call every frame. Detects when the DSP finished a SNAP measurement and
    /// writes the resulting markdown snapshot to the vault.
    fn finish_snap_if_done(&mut self) {
        let shared = &self.params.shared;
        let snap_active = shared.snap_active.load(Ordering::Acquire);
        if self.prev_snap_active && !snap_active {
            self.snap_blink_counter = 0;
            if let Some(vp) = self.vault_path.as_deref() {
                if !vp.is_empty() {
                    let stereo = shared.snap_stereo_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0f32; SPECTRUM_BINS]);
                    let mono = shared.snap_mono_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0f32; SPECTRUM_BINS]);
                    let delta = shared.snap_delta_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0f32; SPECTRUM_BINS]);
                    let sr = shared.sample_rate.load(Ordering::Acquire);
                    let band_levels: [f32; 5] = std::array::from_fn(|b| shared.band_levels[b].load(Ordering::Acquire));
                    let corr = shared.phase_correlation.load(Ordering::Acquire);
                    let pl = shared.output_peak_l.load(Ordering::Acquire);
                    let pr = shared.output_peak_r.load(Ordering::Acquire);
                    let md = snap_markdown(&stereo, &mono, &delta, band_levels, corr, pl, pr, sr);
                    let fname = snap_filename(vp);
                    let _ = std::fs::write(std::path::Path::new(vp).join(&fname), &md);
                }
            }
        } else if snap_active {
            self.snap_blink_counter = self.snap_blink_counter.wrapping_add(1);
        }
        self.prev_snap_active = snap_active;
    }
}

fn build_factory_presets() -> Vec<PresetEntry> {
    vec![(
        "Pink Noise".to_string(),
        None,
        Profile {
            name: "Pink Noise".to_string(),
            bands: [1.5, 0.0, -1.5, -3.0, -4.5],
            tolerances: DEFAULT_TOLERANCES,
            pans: [0.0; 5],
            widths: [100.0; 5],
            mono_floor_hz: 0.0,
            ..Profile::default()
        },
    )]
}

impl EditorUi<EquilibriumParams> for EditorState {
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &PluginContext<EquilibriumParams>) {
        build(ui, ctx, self);
    }
}

/// Normalizes a plain value against a param's known linear range, for the
/// manual `PluginContext::automate` calls RESET needs (widgets normalize
/// internally, but a bulk reset isn't going through a widget).
fn param_norm(id: K, plain: f64) -> f64 {
    let (min, max) = match id {
        K::LowGain | K::BassGain | K::MidGain | K::HighMidGain | K::HighGain | K::OutputGain => (-12.0, 12.0),
        K::LowWidth | K::BassWidth | K::MidWidth | K::HighMidWidth | K::HighWidth => (0.0, 150.0),
        K::LowPan | K::BassPan | K::MidPan | K::HighMidPan | K::HighPan => (-1.0, 1.0),
        K::MonoFloor => (0.0, 300.0),
        K::PreMasterTargetDb => (-6.0, -3.0),
        _ => (0.0, 1.0),
    };
    ((plain - min) / (max - min)).clamp(0.0, 1.0)
}

pub fn build(ui: &mut egui::Ui, ctx: &PluginContext<EquilibriumParams>, app: &mut EditorState) {
    // Meters/telemetry live in atomics written by process() every block -
    // keep the editor repainting so they animate instead of updating only
    // on user interaction.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));

    let params = ctx.params().clone();
    let shared = params.shared.clone();

    // ─── SNAP completion check ──────────────────────────────────────────────
    // Detect when the DSP finished a measurement and write the markdown file.
    app.finish_snap_if_done();

    // ─── Auto Loud offset application ───────────────────────────────────────
    // If a measurement just finished, apply the computed gain offset to the
    // Output Gain parameter. This mirrors the Vizia editor's tick() logic.
    // Guarded against Pre-Master because the two modes are mutually exclusive.
    let pre_master_active = params.pre_master_active.value();
    let measuring = shared.auto_loud_measuring.load(Ordering::Acquire);
    if !measuring && !pre_master_active {
        let offset = shared.auto_loud_gain_offset.load(Ordering::Acquire);
        if offset.abs() > 0.05 {
            let cur_db = params.output_gain.raw_target() as f32;
            let new_db = (cur_db + offset).clamp(-12.0, 12.0);
            ctx.automate(K::OutputGain, param_norm(K::OutputGain, f64::from(new_db)));
            shared.auto_loud_gain_offset.store(0.0, Ordering::Release);
        }
    }

    // ─── 1:1 Vizia layout skeleton ──────────────────────────────────────────
    // Header: 50 px, footer: 110 px, left sidebar: 180 px, right sidebar: 155 px.
    // Central panel fills the remaining 655×500 px main area.
    let frame_none = egui::Frame::NONE;

    egui::Panel::top("eq_header")
        .exact_size(50.0)
        .resizable(false)
        .frame(frame_none.fill(HEADER_BG))
        .show_inside(ui, |ui| header_ui(ui, ctx));

    egui::Panel::bottom("eq_footer")
        .exact_size(110.0)
        .resizable(false)
        .frame(frame_none.fill(DARK_BG))
        .show_inside(ui, |ui| footer_ui(ui, ctx, app, pre_master_active, measuring));

    egui::Panel::left("eq_left_sidebar")
        .exact_size(180.0)
        .resizable(false)
        .frame(frame_none.fill(DARK_BG))
        .show_inside(ui, |ui| left_sidebar_ui(ui, app, ctx));

    egui::Panel::right("eq_right_sidebar")
        .exact_size(155.0)
        .resizable(false)
        .frame(frame_none.fill(DARK_BG))
        .show_inside(ui, |ui| right_sidebar_ui(ui, ctx, &shared, pre_master_active, measuring));

    egui::CentralPanel::default()
        .frame(frame_none.fill(DARK_BG))
        .show_inside(ui, |ui| main_ui(ui, ctx, &shared));
}

fn format_pan(pan: f32) -> String {
    if pan.abs() < 0.01 {
        "C".into()
    } else if pan < 0.0 {
        format!("L {:.0}%", -pan * 100.0)
    } else {
        format!("R {:.0}%", pan * 100.0)
    }
}

// ─── 1:1 Vizia layout regions ───────────────────────────────────────────────

fn header_ui(ui: &mut egui::Ui, ctx: &PluginContext<EquilibriumParams>) {
    ui.horizontal_centered(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new("LX").size(20.0).color(AMBER).strong());
        ui.label(RichText::new("AUDIOLABS").size(20.0).color(HEADER_TEXT).strong());
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(14.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("EQUILIBRIUM (egui proto)").size(13.0).color(AMBER));
            ui.label(RichText::new(format!("v{VERSION}")).size(10.0).color(Color32::GRAY));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            lx_toggle(ui, ctx, K::BypassActive, "BYPASS");
            lx_toggle(ui, ctx, K::DeltaActive, "DELTA");
            lx_toggle(ui, ctx, K::MonoActive, "MONO");
        });
    });
}

fn left_sidebar_ui(ui: &mut egui::Ui, app: &mut EditorState, ctx: &PluginContext<EquilibriumParams>) {
    ui.vertical(|ui| {
        ui.set_width(168.0);
        ui.add_space(8.0);
        ui.label(RichText::new("TARGET PROFILES").size(11.0).color(AMBER).strong());
        ui.add_space(6.0);

        // Save-preset input + SAVE button
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut app.preset_name_input).desired_width(95.0));
            if ui.add(egui::Button::new("SAVE").fill(IDLE_BG)).clicked() {
                app.save_current_preset(ctx);
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("VAULT SETUP").fill(IDLE_BG)).clicked() {
                app.show_setup = !app.show_setup;
            }
            let snap_active = app.params.shared.snap_active.load(Ordering::Acquire);
            let no_vault = app.vault_path.as_deref().map(|s| s.is_empty()).unwrap_or(true);
            let snap_color = if snap_active { AMBER } else { IDLE_BG };
            let snap_label = if no_vault { "SET VAULT" } else { "SNAP" };
            if ui.add(egui::Button::new(snap_label).fill(snap_color)).clicked() && !snap_active {
                if no_vault {
                    app.show_setup = true;
                } else {
                    app.trigger_snap();
                }
            }
        });

        // Vault setup form
        if app.show_setup {
            ui.add_space(6.0);
            ui.label(RichText::new("Vault path:").size(10.0).color(Color32::from_gray(180)));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut app.vault_path_input).desired_width(110.0));
                if ui.add(egui::Button::new("SET").fill(IDLE_BG)).clicked() {
                    app.apply_vault_path();
                }
            });
        }

        ui.add_space(6.0);
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let sel = app.selected_preset;

            // Factory presets
            let factory: Vec<(usize, String)> = app.presets.iter().enumerate()
                .filter(|(_, (_, p, _))| p.is_none())
                .map(|(i, (n, _, _))| (i, n.clone()))
                .collect();
            if !factory.is_empty() {
                ui.label(RichText::new("── Factory ──").size(10.0).color(AMBER));
                for (idx, name) in factory {
                    preset_list_item(ui, idx, &name, sel, app);
                }
            }

            // User/Vault presets
            let user: Vec<(usize, String)> = app.presets.iter().enumerate()
                .filter(|(_, (_, p, _))| p.is_some())
                .map(|(i, (n, _, _))| (i, n.clone()))
                .collect();
            if !user.is_empty() {
                ui.label(RichText::new("── Vault Presets ──").size(10.0).color(AMBER));
                for (idx, name) in user {
                    preset_list_item(ui, idx, &name, sel, app);
                }
            }
        });
    });
}

fn preset_list_item(ui: &mut egui::Ui, idx: usize, name: &str, selected: Option<usize>, app: &mut EditorState) {
    let is_sel = selected == Some(idx);
    let text = RichText::new(name).size(10.0).color(if is_sel { Color32::WHITE } else { Color32::from_gray(190) });
    let response = ui.add(
        egui::Button::new(text)
            .selected(is_sel)
            .fill(if is_sel { Color32::from_rgb(46, 36, 24) } else { Color32::TRANSPARENT })
            .frame(false),
    );
    if response.clicked() {
        app.select_preset(idx);
    }
}

fn main_ui(ui: &mut egui::Ui, ctx: &PluginContext<EquilibriumParams>, shared: &Arc<SharedState>) {
    ui.vertical(|ui| {
        spectrum_view(ui, shared);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);
            for b in 0..5 {
                ui.vertical(|ui| {
                    ui.set_width(95.0);
                    ui.label(RichText::new(format!("{} ({})", BAND_NAMES[b], BAND_HZ[b])).size(10.0).color(AMBER));

                    ui.label(RichText::new("Gain").size(9.0).color(Color32::from_gray(190)));
                    let gain = lx_hslider(ui, ctx, GAIN_IDS[b], -12.0, 12.0, 0.0);
                    ui.label(RichText::new(format!("{gain:.1} dB")).size(9.0).color(Color32::from_gray(200)));

                    ui.label(RichText::new("Width").size(9.0).color(Color32::from_gray(190)));
                    let width = lx_hslider(ui, ctx, WIDTH_IDS[b], 0.0, 150.0, 100.0);
                    ui.label(RichText::new(format!("{width:.0}%")).size(9.0).color(Color32::from_gray(200)));

                    ui.label(RichText::new("Pan").size(9.0).color(Color32::from_gray(190)));
                    let pan = lx_hslider(ui, ctx, PAN_IDS[b], -1.0, 1.0, 0.0);
                    ui.label(RichText::new(format_pan(pan)).size(9.0).color(Color32::from_gray(200)));

                    ui.add_space(4.0);
                    lx_toggle(ui, ctx, SOLO_IDS[b], "SOLO");
                });
            }
        });
    });
}

fn right_sidebar_ui(
    ui: &mut egui::Ui,
    ctx: &PluginContext<EquilibriumParams>,
    shared: &Arc<SharedState>,
    pre_master_active: bool,
    measuring: bool,
) {
    ui.vertical(|ui| {
        ui.set_width(143.0);
        ui.label(RichText::new("OUTPUT").size(11.0).color(AMBER));
        param_knob(ui, ctx, K::OutputGain, "Out Gain");
        ui.add_space(6.0);
        peak_meters(ui, shared);

        ui.add_space(10.0);
        lx_toggle(ui, ctx, K::PreMasterActive, "PRE-MASTER");
        let pre_target = lx_hslider(ui, ctx, K::PreMasterTargetDb, -6.0, -3.0, -3.0);
        ui.label(RichText::new(format!("Target: {pre_target:.1} dB")).size(9.0).color(Color32::from_gray(200)));

        ui.add_space(8.0);
        let auto_loud_available = !pre_master_active && !measuring;
        let auto_loud_label = if measuring { "MEASURING..." } else { "AUTO LOUD" };
        let auto_loud_response = lx_button(ui, auto_loud_label, false, false);
        if !auto_loud_available {
            auto_loud_response.clone().on_hover_text("Auto Loud is disabled while Pre-Master is active.");
        }
        if auto_loud_response.clicked() && auto_loud_available {
            shared.auto_loud_trigger.store(true, Ordering::Release);
        }

        ui.add_space(8.0);
        ui.label(RichText::new("GONIOMETER").size(10.0).color(Color32::from_gray(150)));
        goniometer_view(ui, shared);
    });
}

fn footer_ui(
    ui: &mut egui::Ui,
    ctx: &PluginContext<EquilibriumParams>,
    app: &mut EditorState,
    _pre_master_active: bool,
    _measuring: bool,
) {
    ui.horizontal(|ui| {
        ui.set_height(94.0);
        ui.add_space(8.0);

        // LISTEN / APPLY / RESET ANALYSIS
        ui.vertical(|ui| {
            ui.label(RichText::new("ANALYSIS").size(10.0).color(Color32::from_gray(150)));
            let listen_active = ctx.get_param(K::ListenActive) > 0.5;
            ui.horizontal(|ui| {
                if lx_button(ui, "LISTEN", listen_active, false).clicked() {
                    ctx.automate(K::ListenActive, if listen_active { 0.0 } else { 1.0 });
                }
                if lx_button(ui, "APPLY", false, false).clicked() {
                    app.apply_analysis();
                }
                if lx_button(ui, "RESET ANALYSIS", false, false).clicked() {
                    app.reset_analysis();
                }
            });
        });

        ui.separator();
        ui.add_space(8.0);

        // Mono Floor knob (moved from right sidebar to match Vizia layout)
        ui.vertical(|ui| {
            ui.label(RichText::new("MONO FLOOR").size(10.0).color(AMBER));
            param_knob(ui, ctx, K::MonoFloor, "Floor");
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if lx_button(ui, "RESET", false, true).clicked() {
                reset_all(ctx);
            }
        });
    });
}

// ─── lx_hslider - port of shared_ui::HSliderView ────────────────────────────

/// Horizontal drag slider matching `shared_ui::HSliderView`'s exact
/// geometry/colors (4px track, amber fill, 5px white handle) - value maps
/// directly from cursor X position, same as the Vizia original. Returns the
/// current plain value (post-drag, if this frame changed it) for the
/// caller's readout label.
fn lx_hslider(ui: &mut egui::Ui, state: &PluginContext<EquilibriumParams>, id: K, min: f32, max: f32, default: f32) -> f32 {
    let desired = egui::vec2(ui.available_width().max(60.0), 20.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let span = max - min;
    let bipolar = min < 0.0 && max > 0.0;
    let center_norm = if bipolar { ((0.0 - min) / span).clamp(0.0, 1.0) } else { 0.0 };

    if response.double_clicked() || response.secondary_clicked() {
        state.automate(id, (((default - min) / span) as f64).clamp(0.0, 1.0));
    }
    if response.drag_started() {
        state.begin_edit(id);
    }
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let n = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        state.set_param(id, f64::from(n));
    }
    if response.drag_stopped() {
        state.end_edit(id);
    }

    let norm = state.get_param(id);
    let plain = min + norm * span;

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let track_h = 4.0;
        let ty = rect.center().y - track_h / 2.0;
        let track = egui::Rect::from_min_max(egui::pos2(rect.left(), ty), egui::pos2(rect.right(), ty + track_h));
        painter.rect_filled(track, 0.0, Color32::from_gray(56));

        if bipolar {
            let cx_px = rect.left() + center_norm * rect.width();
            let hx = rect.left() + norm * rect.width();
            let (l, r) = if hx >= cx_px { (cx_px, hx) } else { (hx, cx_px) };
            if r - l > 0.5 {
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(l, ty), egui::pos2(r, ty + track_h)), 0.0, AMBER);
            }
        } else {
            let fill_x = rect.left() + norm * rect.width();
            if fill_x - rect.left() > 0.5 {
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(rect.left(), ty), egui::pos2(fill_x, ty + track_h)), 0.0, AMBER);
            }
        }

        let hx = (rect.left() + norm * rect.width()).clamp(rect.left() + 3.0, rect.right() - 3.0);
        let handle_y = rect.center().y;
        painter.circle_filled(egui::pos2(hx, handle_y), 5.0, Color32::WHITE);

        if response.hovered() || response.dragged() {
            painter.circle_stroke(egui::pos2(hx, handle_y), 6.0, Stroke::new(1.2, AMBER.gamma_multiply(0.7)));
        }
    }

    plain
}

// ─── lx_toggle / lx_button - port of shared_ui::buttons ─────────────────────

/// Amber-when-active toggle bound directly to a bool param, matching
/// `shared_ui::toggle_button`'s look (dark grey idle, amber active,
/// lighter-grey hover).
fn lx_toggle(ui: &mut egui::Ui, state: &PluginContext<EquilibriumParams>, id: K, label: &str) -> egui::Response {
    let active = state.get_param(id) > 0.5;
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::monospace(11.0), Color32::WHITE);
    let size = galley.size() + egui::vec2(16.0, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if response.clicked() {
        state.automate(id, if active { 0.0 } else { 1.0 });
    }

    if ui.is_rect_visible(rect) {
        let bg = if active {
            AMBER
        } else if response.hovered() {
            HOVER_BG
        } else {
            IDLE_BG
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, bg);
        painter.galley(rect.center() - galley.size() * 0.5, galley, Color32::WHITE);
    }
    response
}

/// Plain push-button / danger-button, matching `shared_ui::push_button_big`
/// / `danger_button_big` - not bound to a param, caller checks `.clicked()`.
fn lx_button(ui: &mut egui::Ui, label: &str, active: bool, danger: bool) -> egui::Response {
    let text_color = if danger { DANGER_TEXT } else { Color32::WHITE };
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::monospace(12.0), text_color);
    let size = galley.size() + egui::vec2(20.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let bg = if active {
            AMBER
        } else {
            let idle = if danger { DANGER_BG } else { IDLE_BG };
            if response.hovered() { idle.gamma_multiply(1.4) } else { idle }
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, bg);
        painter.galley(rect.center() - galley.size() * 0.5, galley, text_color);
    }
    response
}

// ─── Spectrum / meters / goniometer - custom egui::Painter views ───────────

/// Custom-painted 5-band spectrum. Ports `EqSpectrumView` from
/// `plugins/equilibrium/src/vizia_canvas.rs` to `egui::Painter`.
fn spectrum_view(ui: &mut egui::Ui, shared: &Arc<SharedState>) {
    let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 180.0), egui::Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 2.0, Color32::from_rgb(20, 20, 20));

    let width = rect.width();
    let height = rect.height();
    let col_w = width / 5.0;

    let min_db = -30.0f32;
    let max_db = 12.0f32;
    let db_range = max_db - min_db;

    let db_to_y = |db: f32| {
        let norm = ((db - min_db) / db_range).clamp(0.0, 1.0);
        rect.bottom() - norm * height
    };

    // Grid lines
    for &db in &[-30.0f32, -24.0, -18.0, -12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = db_to_y(db);
        let is_major = db == -30.0 || db == -18.0 || db == -6.0 || db == 6.0;
        let alpha = if is_major { 0.20 } else { 0.10 };
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.0, Color32::from_white_alpha((alpha * 255.0) as u8)),
        );
    }
    for i in 1..5 {
        let x = rect.left() + i as f32 * col_w;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0, Color32::from_white_alpha(13)),
        );
    }

    // Read atomics once per frame
    let band_levels: [f32; 5] = std::array::from_fn(|b| shared.band_levels[b].load(Ordering::Acquire));
    let target_levels: [f32; 5] = std::array::from_fn(|b| shared.target_levels[b].load(Ordering::Acquire));
    let target_tolerances: [f32; 5] = std::array::from_fn(|b| shared.target_tolerances[b].load(Ordering::Acquire));
    let listen_levels: [f32; 5] = std::array::from_fn(|b| shared.listen_levels[b].load(Ordering::Acquire));
    let listen_tolerances: [f32; 5] = std::array::from_fn(|b| shared.listen_tolerances[b].load(Ordering::Acquire));
    let listen_min: [f32; 5] = std::array::from_fn(|b| shared.listen_level_min[b].load(Ordering::Acquire));
    let listen_max: [f32; 5] = std::array::from_fn(|b| shared.listen_level_max[b].load(Ordering::Acquire));
    let listen_samples = shared.listen_samples.load(Ordering::Acquire);

    let raw_band_avg: f32 = band_levels.iter().sum::<f32>() / 5.0;
    let is_silent = raw_band_avg <= -70.0;

    let listen_sum: f32 = (0..5).map(|b| listen_levels[b].max(-50.0) + TILT[b]).sum();
    let listen_avg = listen_sum / 5.0;
    let band_sum: f32 = (0..5).map(|b| band_levels[b].max(-50.0) + TILT[b]).sum();
    let band_avg = band_sum / 5.0;
    let target_sum: f32 = (0..5).map(|b| (target_levels[b] + TILT[b]).max(-30.0)).sum();
    let target_avg = target_sum / 5.0;

    for b in 0..5 {
        let col_x = rect.left() + b as f32 * col_w;

        // Live band bars
        let bar_alpha = if listen_samples > 0.0 { 0.12 } else { 0.55 };
        if !is_silent {
            let peak_db_t = band_levels[b].max(-50.0) + TILT[b];
            let norm_band_db = peak_db_t - band_avg;
            let bar_top_y = db_to_y(norm_band_db);
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(col_x + 5.0, bar_top_y), egui::pos2(col_x + col_w - 5.0, rect.bottom())),
                1.0,
                Color32::from_rgba_unmultiplied(255, 115, 26, (bar_alpha * 255.0) as u8),
            );
        }

        // Target profile overlay (when not actively listening)
        if listen_samples <= 100.0 {
            let target_db = (target_levels[b] + TILT[b]).max(-30.0);
            let norm_target_db = target_db - target_avg;
            let tolerance = target_tolerances[b];

            let target_y = db_to_y(norm_target_db);
            let upper_y = db_to_y(norm_target_db + tolerance);
            let lower_y = db_to_y(norm_target_db - tolerance);
            let corridor_h = (lower_y - upper_y).max(2.0);

            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(col_x + 1.0, upper_y), egui::pos2(col_x + col_w - 1.0, upper_y + corridor_h)),
                1.0,
                Color32::from_white_alpha(38),
            );
            painter.line_segment(
                [egui::pos2(col_x, target_y), egui::pos2(col_x + col_w, target_y)],
                Stroke::new(1.0, Color32::from_white_alpha(140)),
            );
        }

        // Listen analysis overlay (while/after listening)
        if listen_samples > 100.0 {
            let listen_db = listen_levels[b].max(-50.0) + TILT[b];
            let norm_listen_db = listen_db - listen_avg;
            let listen_y = db_to_y(norm_listen_db);

            let min_db_l = listen_min[b].max(-50.0) + TILT[b];
            let max_db_l = listen_max[b].max(-50.0) + TILT[b];
            let norm_min = min_db_l - listen_avg;
            let norm_max = max_db_l - listen_avg;
            let upper_y = db_to_y(norm_max);
            let lower_y = db_to_y(norm_min);
            let range_h = (lower_y - upper_y).max(2.0);

            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(col_x + 1.0, upper_y), egui::pos2(col_x + col_w - 1.0, upper_y + range_h)),
                1.0,
                Color32::from_rgba_unmultiplied(255, 77, 77, 31),
            );

            let listen_tol = listen_tolerances[b];
            let l_upper_y = db_to_y(norm_listen_db + listen_tol);
            let l_lower_y = db_to_y(norm_listen_db - listen_tol);
            let l_corridor_h = (l_lower_y - l_upper_y).max(2.0);

            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(col_x + 1.0, l_upper_y), egui::pos2(col_x + col_w - 1.0, l_upper_y + l_corridor_h)),
                1.0,
                Color32::from_rgba_unmultiplied(128, 128, 255, 26),
            );

            painter.line_segment(
                [egui::pos2(col_x, listen_y), egui::pos2(col_x + col_w, listen_y)],
                Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 77, 77, 178)),
            );
        }
    }
}

/// L/R peak meter with hold line, dB ticks and balance cursor —
/// ported from `shared_ui::StereoMeterView`.
fn peak_meters(ui: &mut egui::Ui, shared: &Arc<SharedState>) {
    let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 60.0), egui::Sense::click());
    let rect = resp.rect;
    if resp.clicked() {
        shared.reset_peak.store(true, Ordering::Release);
    }

    let gap = 36.0f32;
    let bar_w = (rect.width() - gap) / 2.0;
    let min_db = -60.0f32;
    let max_db = 6.0f32;
    let db_range = max_db - min_db;

    painter.rect_filled(rect, 1.0, Color32::from_rgb(20, 20, 20));

    let db_to_y = |db: f32| {
        let norm = ((db - min_db) / db_range).clamp(0.0, 1.0);
        rect.bottom() - norm * rect.height()
    };

    // Bars + hold lines
    for (i, (peak, hold)) in [
        (shared.output_peak_l.load(Ordering::Acquire), shared.peak_hold_l.load(Ordering::Acquire)),
        (shared.output_peak_r.load(Ordering::Acquire), shared.peak_hold_r.load(Ordering::Acquire)),
    ]
    .into_iter()
    .enumerate()
    {
        let x0 = rect.left() + if i == 0 { 0.0 } else { bar_w + gap };
        let bar_rect = egui::Rect::from_min_max(egui::pos2(x0 + 1.0, rect.top()), egui::pos2(x0 + bar_w - 1.0, rect.bottom()));
        painter.rect_filled(bar_rect, 1.0, Color32::from_rgb(15, 15, 15));

        let bar_color = if peak > 0.0 {
            Color32::from_rgb(255, 64, 64)
        } else if peak > -6.0 {
            AMBER
        } else {
            Color32::from_rgb(0, 191, 77)
        };

        let norm_peak = ((peak - min_db) / db_range).clamp(0.0, 1.0);
        let bar_h = rect.height() * norm_peak;
        let y = rect.bottom() - bar_h;
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0 + 1.0, y), egui::pos2(x0 + bar_w - 1.0, rect.bottom())),
            1.0,
            bar_color,
        );

        if hold > min_db {
            let norm_hold = ((hold - min_db) / db_range).clamp(0.0, 1.0);
            let hy = rect.bottom() - rect.height() * norm_hold;
            painter.line_segment([egui::pos2(x0, hy), egui::pos2(x0 + bar_w, hy)], Stroke::new(1.5, Color32::WHITE));
        }

        let label = if i == 0 { "L" } else { "R" };
        painter.text(
            egui::pos2(x0 + bar_w * 0.5, rect.bottom() - 6.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            FontId::monospace(9.0),
            Color32::from_white_alpha(115),
        );
    }

    // Center dB ticks
    let gx = rect.left() + bar_w;
    let center_x = gx + gap * 0.5;
    for (db_val, label) in [(-3.0f32, "-3"), (-6.0, "-6"), (-12.0, "-12"), (-24.0, "-24"), (-48.0, "-48")] {
        let y = db_to_y(db_val);
        let tick_half = if label == "-3" || label == "-6" { 5.0 } else { 3.0 };
        painter.line_segment(
            [egui::pos2(center_x - tick_half, y), egui::pos2(center_x + tick_half, y)],
            Stroke::new(0.8, Color32::from_white_alpha(115)),
        );
        painter.text(
            egui::pos2(gx + 1.0, y - 3.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            FontId::monospace(8.0),
            Color32::from_white_alpha(153),
        );
    }

    // Balance cursor
    let balance = shared.balance.load(Ordering::Acquire).clamp(-1.0, 1.0);
    let cursor_x = center_x - balance * (gap * 0.35);
    let cursor_y = rect.height() * 0.82 + rect.top();
    let cursor_color = if balance.abs() < 0.08 {
        Color32::from_rgb(0, 217, 89)
    } else {
        AMBER
    };
    painter.circle_filled(egui::pos2(cursor_x, cursor_y), 3.5, cursor_color);
    painter.line_segment(
        [egui::pos2(center_x, cursor_y - 5.0), egui::pos2(center_x, cursor_y + 5.0)],
        Stroke::new(0.8, Color32::from_white_alpha(31)),
    );
}

/// Lissajous vectorscope, ported from `shared_ui::canvas::GoniometerView`.
/// Batches the dot trail into one `egui::Mesh` per age-group instead of
/// issuing individual `circle_filled` calls.
fn goniometer_view(ui: &mut egui::Ui, shared: &Arc<SharedState>) {
    let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 120.0), egui::Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 0.0, Color32::from_rgb(15, 15, 15));

    let (cx, cy) = (rect.center().x, rect.center().y);
    let scale = rect.width().min(rect.height()) * 0.5 * 0.9;

    let grid = Color32::from_white_alpha(20);
    painter.line_segment([egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())], Stroke::new(1.0, grid));
    painter.line_segment([egui::pos2(rect.left(), cy), egui::pos2(rect.right(), cy)], Stroke::new(1.0, grid));
    painter.line_segment([rect.left_top(), rect.right_bottom()], Stroke::new(1.0, grid));
    painter.line_segment([rect.right_top(), rect.left_bottom()], Stroke::new(1.0, grid));
    painter.circle_stroke(egui::pos2(cx, cy), scale, Stroke::new(1.0, Color32::from_white_alpha(15)));

    if let Ok(samples) = shared.scope_samples.try_lock() {
        let n = samples.len();
        if n > 0 {
            let draw_count = n.min(2048);
            let third = draw_count / 3;
            let wp = shared.scope_write_pos.load(Ordering::Acquire) % n;
            let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;

            for group in 0..3u8 {
                let alpha = match group {
                    0 => 0.12,
                    1 => 0.30,
                    _ => 0.72,
                };
                let dot_color = Color32::from_rgba_unmultiplied(26, 230, 128, (alpha * 255.0) as u8);
                let start = group as usize * third;
                let end = if group == 2 { draw_count } else { (group as usize + 1) * third };

                let mut mesh = egui::Mesh::default();
                mesh.translate(rect.left_top().to_vec2());

                for k in start..end {
                    let age = draw_count - 1 - k;
                    let idx = (wp + n - age - 1) % n;
                    let [l, r] = samples[idx];
                    let m = (l + r) * inv_sqrt2;
                    let s = (l - r) * inv_sqrt2;
                    let sx = cx - s * scale;
                    let sy = cy - m * scale;
                    if sx >= rect.left() && sx <= rect.right() && sy >= rect.top() && sy <= rect.bottom() {
                        add_point_to_mesh(&mut mesh, sx - rect.left(), sy - rect.top(), 0.9, dot_color);
                    }
                }

                if !mesh.indices.is_empty() {
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
        }
    }

    let corr = shared.phase_correlation.load(Ordering::Acquire).clamp(-1.0, 1.0);
    let dot_color = if corr > 0.7 {
        Color32::from_rgb(0, 191, 77)
    } else if corr >= 0.0 {
        AMBER
    } else {
        Color32::from_rgb(255, 64, 64)
    };
    let (dx, dy) = (rect.left() + 8.0, rect.bottom() - 8.0);
    painter.circle_filled(egui::pos2(dx, dy), 3.5, dot_color);
    let sign = if corr >= 0.0 { "+" } else { "" };
    painter.text(egui::pos2(dx + 7.0, dy - 8.0), egui::Align2::LEFT_CENTER, format!("{sign}{corr:.2}"), FontId::monospace(9.0), Color32::from_rgb(255, 166, 77));
}

/// Add a small axis-aligned square (two triangles) to an `egui::Mesh`.
fn add_point_to_mesh(mesh: &mut egui::Mesh, x: f32, y: f32, radius: f32, color: Color32) {
    let base = mesh.vertices.len() as u32;
    let uv = egui::epaint::WHITE_UV;
    mesh.vertices.push(egui::epaint::Vertex { pos: egui::pos2(x - radius, y - radius), uv, color });
    mesh.vertices.push(egui::epaint::Vertex { pos: egui::pos2(x + radius, y - radius), uv, color });
    mesh.vertices.push(egui::epaint::Vertex { pos: egui::pos2(x - radius, y + radius), uv, color });
    mesh.vertices.push(egui::epaint::Vertex { pos: egui::pos2(x + radius, y + radius), uv, color });
    mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
}

fn reset_all(ctx: &PluginContext<EquilibriumParams>) {
    let shared = ctx.params().shared.clone();
    shared.auto_loud_gain_offset.store(0.0, Ordering::Release);
    shared.auto_loud_measuring.store(false, Ordering::Release);

    for (id, val) in [
        (K::LowGain, 0.0f64), (K::BassGain, 0.0), (K::MidGain, 0.0), (K::HighMidGain, 0.0), (K::HighGain, 0.0),
        (K::LowWidth, 100.0), (K::BassWidth, 100.0), (K::MidWidth, 100.0), (K::HighMidWidth, 100.0), (K::HighWidth, 100.0),
        (K::LowPan, 0.0), (K::BassPan, 0.0), (K::MidPan, 0.0), (K::HighMidPan, 0.0), (K::HighPan, 0.0),
        (K::OutputGain, 0.0), (K::MonoFloor, 0.0), (K::PreMasterTargetDb, -3.0),
    ] {
        ctx.automate(id, param_norm(id, val));
    }
    for id in [K::MonoActive, K::DeltaActive, K::BypassActive, K::PreMasterActive, K::ListenActive].into_iter().chain(SOLO_IDS) {
        ctx.automate(id, 0.0);
    }
}

fn snap_filename(vault_path: &str) -> String {
    let dir = std::path::Path::new(vault_path);
    let mut max_n = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let s = e.file_name().to_string_lossy().into_owned();
            if let Some(inner) = s.strip_prefix("SNAPSHOT-").and_then(|r| r.strip_suffix(".md"))
                && let Ok(n) = inner.parse::<u32>() {
                    max_n = max_n.max(n);
                }
        }
    }
    format!("SNAPSHOT-{:03}.md", max_n + 1)
}

fn snap_markdown(stereo: &[f32], mono: &[f32], delta: &[f32], band_levels: [f32; 5], corr: f32, pl: f32, pr: f32, sr: f32) -> String {
    let fft_sz = 2048.0;
    let freqs: &[f32] = &[20.0, 40.0, 80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10000.0, 16000.0, 20000.0];
    let tbl = |s: &[f32]| {
        freqs
            .iter()
            .map(|&f| {
                let bin = ((f * fft_sz / sr) as usize).min(s.len().saturating_sub(1));
                format!("| {} | {:.1} |", if f >= 1000.0 { format!("{:.0}k", f / 1000.0) } else { format!("{:.0}", f) }, s[bin])
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "---\nplugin: equilibrium\ntype: snapshot\n---\n\n# Equilibrium Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n\n\
        ## 5-Band\n| Band | Pegel |\n|------|-------|\n\
        | Low | {b0:.1} dB |\n| Bass | {b1:.1} dB |\n| Mid | {b2:.1} dB |\n| Hi-Mid | {b3:.1} dB |\n| High | {b4:.1} dB |\n",
        pl = pl, pr = pr, co = corr, st = tbl(stereo), mn = tbl(mono), dt = tbl(delta),
        b0 = band_levels[0], b1 = band_levels[1], b2 = band_levels[2], b3 = band_levels[3], b4 = band_levels[4],
    )
}
