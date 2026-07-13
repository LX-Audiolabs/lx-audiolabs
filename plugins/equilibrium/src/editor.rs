//! Vizia port of the old iced `editor.rs`. See CLAP-vault
//! `features/2026-07-04-truce-2.0-upgrade-plan.md` for the Ticker/Memo/Binding
//! rationale (same pattern as `plugins/lucent/src/editor.rs`, the pilot this
//! port follows): tick-frequency telemetry lives in one `Signal<Telemetry>`
//! updated every ~33ms by `Ticker` (not `cx.add_timer`/`start_timer` -
//! vizia_core 0.4.0's `modify_timer` has a real infinite-loop bug), passive
//! display regions are wrapped in `Binding`s keyed to it. Drag widgets
//! (sliders/knobs) bind to each param's `ParamLens::value_signal` so
//! `truce-vizia`'s `refresh_params` idle poll repaints host automation without
//! rebuilding unrelated widgets. `params_gen` remains for preset-list
//! refresh and other discrete bulk updates (ResetAll, SelectPreset).
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::Ordering};
use std::time::{Duration, Instant};

use vizia::prelude::*;
use vizia::vg;

use shared_analysis::SharedState;
use truce_vizia::ParamLens;

use crate::vizia_canvas::EqSpectrumView;
use crate::{EquilibriumParams, EquilibriumParamsParamId as K};
use shared_ui::{
    Gesture, GoniometerView, HSliderView, KnobView, StereoMeterView, fmt_db, format_knob_value,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `vizia::prelude::Color` (CSS-style, used by `.color()`/`.background_color()`
/// view modifiers) is a different type from `vg::Color` (Skia, used inside
/// `draw()` - see `vizia_canvas::col`/`rgb`). These are the view-modifier
/// versions, same helper Lucent's `editor.rs` defines for the same reason.
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

const GAIN_IDS: [K; 5] = [
    K::LowGain,
    K::BassGain,
    K::MidGain,
    K::HighMidGain,
    K::HighGain,
];
const WIDTH_IDS: [K; 5] = [
    K::LowWidth,
    K::BassWidth,
    K::MidWidth,
    K::HighMidWidth,
    K::HighWidth,
];
const PAN_IDS: [K; 5] = [K::LowPan, K::BassPan, K::MidPan, K::HighMidPan, K::HighPan];
const SOLO_IDS: [K; 5] = [
    K::SoloLow,
    K::SoloBass,
    K::SoloMid,
    K::SoloHighMid,
    K::SoloHigh,
];
const BAND_NAMES: [&str; 5] = ["Sub", "Bass", "Mid", "Pres", "Air"];
const BAND_HZ: [&str; 5] = ["0-80Hz", "80-300Hz", "300Hz-2kHz", "2-6kHz", ">6kHz"];

fn format_pan(pan: f32) -> String {
    if pan.abs() < 0.01 {
        "C".into()
    } else if pan < 0.0 {
        format!("L {:.0}%", -pan * 100.0)
    } else {
        format!("R {:.0}%", pan * 100.0)
    }
}

// ─── Preset data (framework-independent, unchanged from the iced version) ──

#[derive(Debug, Clone)]
pub struct EqPreset {
    pub bands: [f32; 5],
    pub tolerances: [f32; 5],
    pub pans: [f32; 5],
    pub widths: [f32; 5],
    pub mono_floor_hz: f32,
}

fn load_presets(vault_path: Option<&str>) -> Vec<(String, Option<PathBuf>, EqPreset)> {
    let mut presets = vec![(
        "Pink Noise".to_string(),
        None,
        EqPreset {
            // Band power is normalized per octave in the DSP, so pink noise
            // reads flat; the Pink Noise reference target is therefore flat too.
            bands: [0.0, 0.0, 0.0, 0.0, 0.0],
            tolerances: shared_analysis::DEFAULT_TOLERANCES,
            pans: [0.0; 5],
            widths: [100.0; 5],
            mono_floor_hz: 0.0,
        },
    )];
    let custom = shared_analysis::list_custom_presets("Equilibrium", vault_path);
    for (name, path, profile) in custom {
        presets.push((
            name,
            Some(path),
            EqPreset {
                bands: profile.bands,
                tolerances: profile.tolerances,
                pans: profile.pans,
                widths: profile.widths,
                mono_floor_hz: profile.mono_floor_hz,
            },
        ));
    }
    presets
}

// ─── Telemetry (tick-frequency display state) ───────────────────────────────

#[derive(Clone, PartialEq)]
struct Telemetry {
    band_levels: [f32; 5],
    target_levels: [f32; 5],
    target_tolerances: [f32; 5],
    listen_levels: [f32; 5],
    listen_tolerances: [f32; 5],
    listen_level_min: [f32; 5],
    listen_level_max: [f32; 5],
    listen_samples: f32,
    phase_correlation: f32,
    peak_l: f32,
    peak_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    peak_hold: f32,
    balance: f32,
    auto_loud_measuring: bool,
    snap_active: bool,
    snap_blink_counter: u32,
}

/// Bookkeeping that persists across ticks but never touches the UI directly.
/// Lives in `Arc<Mutex<_>>` (not `Rc<RefCell<_>>` - vizia's `on_press`
/// requires `Send + Sync` closures), shared with every closure that
/// reads/writes presets or the vault path (preset-select, save-preset,
/// vault-setup-save), not just `tick()`.
struct TickAccum {
    presets: Vec<(String, Option<PathBuf>, EqPreset)>,
    vault_path: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn tick(
    shared: &SharedState,
    params: &EquilibriumParams,
    accum: &Arc<Mutex<TickAccum>>,
    telemetry: Signal<Telemetry>,
) -> bool {
    let acc = accum.lock().unwrap();

    let mut band_levels = [0.0f32; 5];
    let mut target_levels = [0.0f32; 5];
    let mut target_tolerances = [0.0f32; 5];
    let mut listen_levels = [0.0f32; 5];
    let mut listen_tolerances = [0.0f32; 5];
    let mut listen_level_min = [0.0f32; 5];
    let mut listen_level_max = [0.0f32; 5];
    for b in 0..5 {
        band_levels[b] = shared.band_levels[b].load(Ordering::Acquire);
        target_levels[b] = shared.target_levels[b].load(Ordering::Acquire);
        target_tolerances[b] = shared.target_tolerances[b].load(Ordering::Acquire);
        listen_levels[b] = shared.listen_levels[b].load(Ordering::Acquire);
        listen_tolerances[b] = shared.listen_tolerances[b].load(Ordering::Acquire);
        listen_level_min[b] = shared.listen_level_min[b].load(Ordering::Acquire);
        listen_level_max[b] = shared.listen_level_max[b].load(Ordering::Acquire);
    }
    let listen_samples = shared.listen_samples.load(Ordering::Acquire);
    let phase_correlation = shared.phase_correlation.load(Ordering::Acquire);
    let peak_l = shared.output_peak_l.load(Ordering::Acquire);
    let peak_r = shared.output_peak_r.load(Ordering::Acquire);
    let peak_hold_l = shared.peak_hold_l.load(Ordering::Acquire);
    let peak_hold_r = shared.peak_hold_r.load(Ordering::Acquire);
    let peak_hold = shared.peak_hold.load(Ordering::Acquire);
    let balance = shared.balance.load(Ordering::Acquire);

    let measuring = shared.auto_loud_measuring.load(Ordering::Acquire);
    if !measuring {
        let offset = shared.auto_loud_gain_offset.load(Ordering::Acquire);
        if offset.abs() > 0.05 {
            let cur = params.output_gain.raw_target() as f32;
            params
                .output_gain
                .set_value((cur + offset).clamp(-12.0, 12.0) as f64);
            shared.auto_loud_gain_offset.store(0.0, Ordering::Release);
        }
    }

    let snap_now = shared.snap_active.load(Ordering::Acquire);
    let vault_path = acc.vault_path.clone();
    // Drop the lock before telemetry.update(): it runs vizia_reactive effects
    // synchronously, and the SNAP button's Memo (build_sidebar) locks `accum`
    // itself - held across the call, it self-deadlocks the UI thread.
    drop(acc);

    let prev = telemetry.get();
    let next = Telemetry {
        band_levels,
        target_levels,
        target_tolerances,
        listen_levels,
        listen_tolerances,
        listen_level_min,
        listen_level_max,
        listen_samples,
        phase_correlation,
        peak_l,
        peak_r,
        peak_hold_l,
        peak_hold_r,
        peak_hold,
        balance,
        auto_loud_measuring: measuring,
        snap_active: snap_now,
        snap_blink_counter: if snap_now {
            prev.snap_blink_counter.wrapping_add(1)
        } else {
            0
        },
    };

    let changed = next != prev;
    if changed {
        telemetry.set(next);
        if !snap_now && prev.snap_active {
            if let Some(vp) = vault_path
                && !vp.is_empty()
            {
                let stereo = shared
                    .snap_stereo_snap
                    .try_lock()
                    .ok()
                    .map(|v| v.clone())
                    .unwrap_or_else(|| vec![-90.0; 1024]);
                let mono = shared
                    .snap_mono_snap
                    .try_lock()
                    .ok()
                    .map(|v| v.clone())
                    .unwrap_or_else(|| vec![-90.0; 1024]);
                let delta = shared
                    .snap_delta_snap
                    .try_lock()
                    .ok()
                    .map(|v| v.clone())
                    .unwrap_or_else(|| vec![-90.0; 1024]);
                let sr = shared.sample_rate.load(Ordering::Acquire);
                let md = snap_markdown(
                    &stereo,
                    &mono,
                    &delta,
                    band_levels,
                    phase_correlation,
                    peak_l,
                    peak_r,
                    sr,
                );
                let fname = snap_filename(&vp);
                let _ = std::fs::write(std::path::Path::new(&vp).join(&fname), &md);
            }
        }
    }

    changed
}

// ─── Ticker (drives `tick()` without vizia_core's buggy timer API) ──────────

struct Ticker {
    shared: Arc<SharedState>,
    params: Arc<EquilibriumParams>,
    accum: Arc<Mutex<TickAccum>>,
    telemetry: Signal<Telemetry>,
    last_tick: RefCell<Instant>,
}

impl Ticker {
    fn new(
        cx: &mut Context,
        shared: Arc<SharedState>,
        params: Arc<EquilibriumParams>,
        accum: Arc<Mutex<TickAccum>>,
        telemetry: Signal<Telemetry>,
    ) -> Handle<'_, Self> {
        Self {
            shared,
            params,
            accum,
            telemetry,
            last_tick: RefCell::new(Instant::now()),
        }
        .build(cx, |_| {})
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
        let profile = shared_ui::ticker_profile_enabled();
        let t0_total = if profile { Some(Instant::now()) } else { None };
        let t0_tick = if profile && due { Some(Instant::now()) } else { None };
        let mut telemetry_changed = false;
        if due {
            telemetry_changed = tick(&self.shared, &self.params, &self.accum, self.telemetry);
        }
        let tick_us = t0_tick.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);
        // Keep the render loop alive so the layer-cached views repaint their
        // dynamic overlays every frame. The telemetry Signal is still only set
        // when values actually change.
        let _ = telemetry_changed;
        cx.needs_redraw();
        let total_us = t0_total.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);
        if profile {
            shared_ui::report_ticker(tick_us, total_us);
        }
    }
}

// ─── Small widget helpers ────────────────────────────────────────────────────

/// Fixed-label toggle button (MONO/DELTA/BYPASS/PRE-MASTER): amber when the
/// bound bool param is active, dark otherwise. Wrapped in a `Binding` on the
/// param's own value signal - fires only on that param's own changes (click
/// or host automation), never on the 33ms tick, so it never hits the
/// rebuild-drops-clicks issue `Memo` was needed for on the tick-driven panels.
fn styled_toggle(cx: &mut Context, lens: ParamLens<EquilibriumParams>, id: K, label: &'static str) {
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

/// Toggle button whose label itself changes with state (SOLO/SOLO ON,
/// LISTEN/LISTEN ON).
fn styled_toggle_dyn(
    cx: &mut Context,
    lens: ParamLens<EquilibriumParams>,
    id: K,
    label_off: &'static str,
    label_on: &'static str,
) {
    let sig = lens.value_signal(id);
    Binding::new(cx, sig, move |cx| {
        let active = lens.get(id) > 0.5;
        let lens = lens.clone();
        shared_ui::toggle_button(
            cx,
            if active { label_on } else { label_off },
            active,
            move |_cx| {
                let now = lens.get(id) <= 0.5;
                let norm = if now { 1.0 } else { 0.0 };
                lens.automate(id, norm);
                sig.set(norm as f32);
            },
        );
    });
}

/// Holds the per-param `Signal<f32>` handles for boolean/toggle parameters so
/// that RESET can push the new value into them and the UI buttons repaint.
struct BoolSignals {
    mono: Signal<f32>,
    delta: Signal<f32>,
    bypass: Signal<f32>,
    pre_master: Signal<f32>,
    listen: Signal<f32>,
    solos: [Signal<f32>; 5],
}

// ─── UI ──────────────────────────────────────────────────────────────────────

pub fn build(
    cx: &mut Context,
    lens: ParamLens<EquilibriumParams>,
    shared: Arc<SharedState>,
    params: Arc<EquilibriumParams>,
) {
    shared_ui::load_theme(cx);
    let config = shared_analysis::load_config("Equilibrium");
    let presets = load_presets(config.vault_path.as_deref());
    let selected_idx = Some(0usize);

    let mut target_levels = [0.0f32; 5];
    let mut target_tolerances = shared_analysis::DEFAULT_TOLERANCES;
    if let Some(idx) = selected_idx {
        let p = &presets[idx].2;
        target_levels = p.bands;
        target_tolerances = p.tolerances;
        for b in 0..5 {
            shared.target_levels[b].store(p.bands[b], Ordering::Release);
            shared.target_tolerances[b].store(p.tolerances[b], Ordering::Release);
        }
        shared.selected_preset_index.store(idx, Ordering::Release);
    }

    let telemetry = Signal::new(Telemetry {
        band_levels: [-90.0; 5],
        target_levels,
        target_tolerances,
        listen_levels: [-90.0; 5],
        listen_tolerances: [0.0; 5],
        listen_level_min: [-90.0; 5],
        listen_level_max: [-90.0; 5],
        listen_samples: 0.0,
        phase_correlation: 1.0,
        peak_l: -90.0,
        peak_r: -90.0,
        peak_hold_l: -90.0,
        peak_hold_r: -90.0,
        peak_hold: -90.0,
        balance: 0.0,
        auto_loud_measuring: false,
        snap_active: false,
        snap_blink_counter: 0,
    });

    let show_setup = Signal::new(false);
    let vault_path_input = Signal::new(config.vault_path.clone().unwrap_or_default());
    let preset_name_input = Signal::new(String::new());
    let selected_preset = Signal::new(selected_idx);
    // Bumped only by discrete actions (ResetAll, SelectPreset, SavePreset,
    // vault-path save) so the preset list and the slider/knob columns
    // rebuild and re-read the freshly-written param/preset values. Never
    // bumped from `tick()` at 33ms - see module doc.
    let params_gen = Signal::new(0u32);

    // Cache the bool param signals so RESET can push into them and force the
    // toggle buttons to repaint. `value_signal()` returns the same handle that
    // the toggle helpers below will use.
    let bool_sigs = BoolSignals {
        mono: lens.value_signal(K::MonoActive),
        delta: lens.value_signal(K::DeltaActive),
        bypass: lens.value_signal(K::BypassActive),
        pre_master: lens.value_signal(K::PreMasterActive),
        listen: lens.value_signal(K::ListenActive),
        solos: [
            lens.value_signal(K::SoloLow),
            lens.value_signal(K::SoloBass),
            lens.value_signal(K::SoloMid),
            lens.value_signal(K::SoloHighMid),
            lens.value_signal(K::SoloHigh),
        ],
    };

    let accum = Arc::new(Mutex::new(TickAccum {
        presets,
        vault_path: config.vault_path,
    }));

    Ticker::new(
        cx,
        shared.clone(),
        params.clone(),
        accum.clone(),
        telemetry,
    )
    .width(Pixels(1.0))
    .height(Pixels(1.0));

    // ── HEADER ──
    let lens_header = lens.clone();
    HStack::new(cx, move |cx| {
        let lens = lens_header;
        HStack::new(cx, |cx| {
            Label::new(cx, "LX")
                .font_size(20.0)
                .color(rgb(1.0, 0.45, 0.1));
            Label::new(cx, "AUDIOLABS")
                .font_size(20.0)
                .color(Color::white());
            Element::new(cx).width(Pixels(14.0));
            Element::new(cx)
                .width(Pixels(1.0))
                .height(Pixels(28.0))
                .background_color(col(0.18, 0.22, 0.22, 1.0));
            Element::new(cx).width(Pixels(14.0));
            VStack::new(cx, |cx| {
                Label::new(cx, "EQUILIBRIUM")
                    .font_size(13.0)
                    .color(rgb(1.0, 0.65, 0.3));
                Label::new(cx, format!("v{VERSION}"))
                    .font_size(10.0)
                    .color(col(0.5, 0.5, 0.5, 1.0));
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
    HStack::new(cx, move |cx| {
        let lens_middle = lens_body.clone();
        let accum_middle = accum_body.clone();
        build_sidebar(
            cx,
            accum_body.clone(),
            telemetry,
            selected_preset,
            preset_name_input,
            show_setup,
            shared_body.clone(),
            lens_body.clone(),
            params_gen,
        );

        VStack::new(cx, move |cx| {
            Binding::new(cx, show_setup, move |cx| {
                if show_setup.get() {
                    build_setup_form(
                        cx,
                        vault_path_input,
                        show_setup,
                        accum_middle.clone(),
                        params_gen,
                    );
                } else {
                    build_main_panel(cx, telemetry, lens_middle.clone());
                }
            });
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0))
        .background_color(rgb(0.06, 0.06, 0.06));

        build_right_sidebar(cx, telemetry, lens_body.clone(), shared_body.clone());
    })
    .width(Stretch(1.0))
    .height(Stretch(1.0));

    build_footer(
        cx,
        telemetry,
        lens,
        shared,
        accum,
        selected_preset,
        params_gen,
        bool_sigs,
    );
}

// ─── Sidebar (TARGET PROFILES) ───────────────────────────────────────────────

fn build_sidebar(
    cx: &mut Context,
    accum: Arc<Mutex<TickAccum>>,
    telemetry: Signal<Telemetry>,
    selected_preset: Signal<Option<usize>>,
    preset_name_input: Signal<String>,
    show_setup: Signal<bool>,
    shared: Arc<SharedState>,
    lens: ParamLens<EquilibriumParams>,
    params_gen: Signal<u32>,
) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "TARGET PROFILES")
            .font_size(14.0)
            .color(Color::white());

        Textbox::new(cx, preset_name_input)
            .placeholder("Preset Name...")
            .on_edit(move |_cx, text| preset_name_input.set(text))
            .width(Stretch(1.0));

        let accum_hs = accum.clone();
        let shared_hs = shared.clone();
        let lens_hs = lens.clone();
        HStack::new(cx, move |cx| {
            let accum_label = accum_hs.clone();
            let shared_press = shared_hs.clone();
            let accum_press = accum_hs.clone();
            Button::new(cx, move |cx| {
                Label::new(
                    cx,
                    Memo::new(move |_| {
                        let t = telemetry.get();
                        let no_vault = accum_label
                            .lock()
                            .unwrap()
                            .vault_path
                            .as_ref()
                            .is_none_or(|v| v.is_empty());
                        if t.snap_active {
                            "ANALYZE...".to_string()
                        } else if no_vault {
                            "SET VAULT".to_string()
                        } else {
                            "SNAP".to_string()
                        }
                    }),
                )
                .font_size(12.0)
                .color(Memo::new(move |_| {
                    let blink = telemetry.get().snap_active
                        && (telemetry.get().snap_blink_counter / 8).is_multiple_of(2);
                    if blink {
                        rgb(1.0, 0.85, 0.3)
                    } else {
                        rgb(1.0, 0.55, 0.1)
                    }
                }))
            })
            .on_press(move |_cx| {
                let no_vault = accum_press
                    .lock()
                    .unwrap()
                    .vault_path
                    .as_ref()
                    .is_none_or(|v| v.is_empty());
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
                let blink = telemetry.get().snap_active
                    && (telemetry.get().snap_blink_counter / 8).is_multiple_of(2);
                if blink {
                    col(0.55, 0.38, 0.05, 1.0)
                } else {
                    col(0.18, 0.18, 0.18, 1.0)
                }
            }));

            let accum_save = accum_hs.clone();
            let lens_save = lens_hs.clone();
            Button::new(cx, |cx| Label::new(cx, "SAVE").font_size(12.0))
                .on_press(move |_cx| {
                    do_save_preset(
                        &accum_save,
                        &telemetry,
                        preset_name_input,
                        &lens_save,
                        params_gen,
                    );
                })
                .width(Stretch(1.0))
                .height(Pixels(34.0))
                .class("lx-btn");
        })
        .width(Stretch(1.0))
        .height(Auto)
        .horizontal_gap(Pixels(4.0));

        Button::new(cx, |cx| Label::new(cx, "VAULT SETUP").font_size(12.0))
            .on_press(move |_cx| show_setup.set(!show_setup.get()))
            .width(Stretch(1.0))
            .height(Pixels(34.0))
            .class("lx-btn");

        let accum_list = accum.clone();
        let lens_list = lens.clone();
        // Keyed to `params_gen`, NOT `telemetry` - the preset list holds
        // Buttons, and `telemetry` updates unconditionally every 33ms from
        // `tick()`. A `Binding` on it would tear down and rebuild every
        // preset Button that often, dropping clicks the same way the
        // header mode button did in Lucent's pilot (see module doc).
        // `params_gen` only bumps on discrete events (preset saved/picked,
        // vault path saved, ~2s periodic reload).
        Binding::new(cx, params_gen, move |cx| {
            let acc = accum_list.lock().unwrap();
            let no_vault = acc.vault_path.as_ref().is_none_or(|v| v.is_empty());
            if no_vault {
                Label::new(cx, "Set Vault-path first")
                    .font_size(9.0)
                    .color(col(1.0, 0.75, 0.2, 1.0));
            }
            let sel = selected_preset.get();
            let factory: Vec<(usize, String)> = acc
                .presets
                .iter()
                .enumerate()
                .filter(|(_, (_, p, _))| p.is_none())
                .map(|(i, (n, _, _))| (i, n.clone()))
                .collect();
            let user: Vec<(usize, String)> = acc
                .presets
                .iter()
                .enumerate()
                .filter(|(_, (_, p, _))| p.is_some())
                .map(|(i, (n, _, _))| (i, n.clone()))
                .collect();
            drop(acc);
            let accum_scroll = accum_list.clone();
            let shared_scroll = shared.clone();
            let lens_scroll = lens_list.clone();
            ScrollView::new(cx, move |cx| {
                if !factory.is_empty() {
                    Label::new(cx, "── Factory ──")
                        .font_size(11.0)
                        .color(rgb(1.0, 0.55, 0.15));
                    for (idx, name) in factory {
                        preset_list_item(
                            cx,
                            idx,
                            name,
                            sel,
                            selected_preset,
                            accum_scroll.clone(),
                            shared_scroll.clone(),
                            lens_scroll.clone(),
                            params_gen,
                        );
                    }
                }
                if !user.is_empty() {
                    Label::new(cx, "── Vault Presets ──")
                        .font_size(11.0)
                        .color(rgb(1.0, 0.55, 0.15));
                    for (idx, name) in user {
                        preset_list_item(
                            cx,
                            idx,
                            name,
                            sel,
                            selected_preset,
                            accum_scroll.clone(),
                            shared_scroll.clone(),
                            lens_scroll.clone(),
                            params_gen,
                        );
                    }
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
    shared: Arc<SharedState>,
    lens: ParamLens<EquilibriumParams>,
    params_gen: Signal<u32>,
) {
    let is_sel = selected == Some(idx);
    Button::new(cx, move |cx| {
        Label::new(cx, format!("> {name}")).font_size(13.0)
    })
    .alignment(Alignment::Left)
    .on_press(move |_cx| {
        let acc = accum.lock().unwrap();
        let Some((_, _, prof)) = acc.presets.get(idx).cloned() else {
            return;
        };
        drop(acc);

        selected_preset.set(Some(idx));
        shared.selected_preset_index.store(idx, Ordering::Release);
        for b in 0..5 {
            shared.target_levels[b].store(prof.bands[b], Ordering::Release);
            shared.target_tolerances[b].store(prof.tolerances[b], Ordering::Release);
        }
        // Bands are the Target Profile (analysis reference line), not a
        // gain correction — only stereo settings apply directly to params.
        for (id, val) in [
            (K::LowWidth, prof.widths[0] as f64),
            (K::BassWidth, prof.widths[1] as f64),
            (K::MidWidth, prof.widths[2] as f64),
            (K::HighMidWidth, prof.widths[3] as f64),
            (K::HighWidth, prof.widths[4] as f64),
            (K::LowPan, prof.pans[0] as f64),
            (K::BassPan, prof.pans[1] as f64),
            (K::MidPan, prof.pans[2] as f64),
            (K::HighMidPan, prof.pans[3] as f64),
            (K::HighPan, prof.pans[4] as f64),
            (K::MonoFloor, prof.mono_floor_hz as f64),
        ] {
            lens.automate(id, param_norm(id, val));
        }
        params_gen.update(|g| *g = g.wrapping_add(1));
    })
    .width(Stretch(1.0))
    .background_color(if is_sel {
        col(0.18, 0.14, 0.08, 1.0)
    } else {
        Color::transparent()
    })
    .color(if is_sel {
        rgb(1.0, 0.45, 0.1)
    } else {
        col(0.9, 0.9, 0.9, 1.0)
    });
}

// ─── Setup form ──────────────────────────────────────────────────────────────

fn build_setup_form(
    cx: &mut Context,
    vault_path_input: Signal<String>,
    show_setup: Signal<bool>,
    accum: Arc<Mutex<TickAccum>>,
    params_gen: Signal<u32>,
) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "LX AUDIOLABS - SETUP")
            .font_size(18.0)
            .color(Color::white());
        Label::new(cx, "Configure your Vault path for Equilibrium:")
            .font_size(12.0)
            .color(Color::white());
        Textbox::new(cx, vault_path_input)
            .placeholder("Enter Vault absolute path...")
            .on_edit(move |_cx, text| vault_path_input.set(text))
            .width(Stretch(1.0));
        HStack::new(cx, move |cx| {
            Button::new(cx, |cx| Label::new(cx, "SAVE"))
                .on_press(move |_cx| {
                    let vp = vault_path_input.get().trim().to_string();
                    if !vp.is_empty() {
                        let mut cfg = shared_analysis::load_config("Equilibrium");
                        cfg.vault_path = Some(vp.clone());
                        let _ = shared_analysis::save_config("Equilibrium", &cfg);
                        let mut acc = accum.lock().unwrap();
                        acc.vault_path = Some(vp.clone());
                        acc.presets = load_presets(Some(&vp));
                        drop(acc);
                        params_gen.update(|g| *g = g.wrapping_add(1));
                        show_setup.set(false);
                    }
                })
                .class("lx-btn");
            Button::new(cx, |cx| Label::new(cx, "CANCEL"))
                .on_press(move |_cx| show_setup.set(false))
                .class("lx-btn");
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

// ─── Main panel (5-band spectrum + slider columns) ──────────────────────────

fn build_main_panel(
    cx: &mut Context,
    telemetry: Signal<Telemetry>,
    lens: ParamLens<EquilibriumParams>,
) {
    // Spectrum + band labels - passive display, safe to rebuild every tick.
    Binding::new(cx, telemetry, move |cx| {
        let t = telemetry.get();
        EqSpectrumView::new(
            cx,
            EqSpectrumView {
                band_levels: t.band_levels,
                target_levels: t.target_levels,
                target_tolerances: t.target_tolerances,
                listen_levels: t.listen_levels,
                listen_tolerances: t.listen_tolerances,
                listen_level_min: t.listen_level_min,
                listen_level_max: t.listen_level_max,
                listen_samples: t.listen_samples,
            },
        )
        .width(Stretch(1.0))
        .height(Stretch(1.0));

        HStack::new(cx, move |cx| {
            for i in 0..5 {
                Label::new(cx, format!("{} ({})", BAND_NAMES[i], BAND_HZ[i]))
                    .font_size(11.0)
                    .color(rgb(1.0, 0.55, 0.15))
                    .width(Stretch(1.0))
                    .alignment(Alignment::Center);
            }
        })
        .width(Stretch(1.0))
        .height(Pixels(20.0));
    });

    // Per-band sliders keyed to each param's value_signal — host automation
    // refreshes only the affected column via truce-vizia::refresh_params.
    let lens_row = lens.clone();
    HStack::new(cx, move |cx| {
        for b in 0..5 {
            let lens_col = lens_row.clone();
            VStack::new(cx, move |cx| {
                Label::new(cx, "Gain")
                    .font_size(12.0)
                    .color(col(0.75, 0.75, 0.75, 1.0));
                Binding::new(cx, lens_col.value_signal(GAIN_IDS[b]), {
                    let lens_col = lens_col.clone();
                    move |cx| {
                        let gain = lens_col.get_plain(GAIN_IDS[b]);
                        let gain_display = Signal::new(gain);
                        let lens_gain = lens_col.clone();
                        HSliderView::new(cx, -12.0, 12.0, gain, 0.0, move |_cx, g| match g {
                            Gesture::Start => lens_gain.begin_edit(GAIN_IDS[b]),
                            Gesture::Change(v) => {
                                let norm = ((v as f64) + 12.0) / 24.0;
                                lens_gain.set(GAIN_IDS[b], norm.clamp(0.0, 1.0));
                                gain_display.set(v);
                            }
                            Gesture::End => lens_gain.end_edit(GAIN_IDS[b]),
                        })
                        .width(Stretch(1.0))
                        .height(Pixels(22.0));
                        Label::new(
                            cx,
                            Memo::new(move |_| format!("{:.1} dB", gain_display.get())),
                        )
                        .font_size(11.0)
                        .color(col(0.8, 0.8, 0.8, 1.0));
                    }
                });

                Label::new(cx, "Width")
                    .font_size(12.0)
                    .color(col(0.75, 0.75, 0.75, 1.0));
                Binding::new(cx, lens_col.value_signal(WIDTH_IDS[b]), {
                    let lens_col = lens_col.clone();
                    move |cx| {
                        let width = lens_col.get_plain(WIDTH_IDS[b]);
                        let width_display = Signal::new(width);
                        let lens_width = lens_col.clone();
                        HSliderView::new(cx, 0.0, 150.0, width, 100.0, move |_cx, g| match g {
                            Gesture::Start => lens_width.begin_edit(WIDTH_IDS[b]),
                            Gesture::Change(v) => {
                                let norm = (v as f64) / 150.0;
                                lens_width.set(WIDTH_IDS[b], norm.clamp(0.0, 1.0));
                                width_display.set(v);
                            }
                            Gesture::End => lens_width.end_edit(WIDTH_IDS[b]),
                        })
                        .width(Stretch(1.0))
                        .height(Pixels(22.0));
                        Label::new(
                            cx,
                            Memo::new(move |_| format!("{:.0}%", width_display.get())),
                        )
                        .font_size(11.0)
                        .color(col(0.8, 0.8, 0.8, 1.0));
                    }
                });

                Label::new(cx, "Pan")
                    .font_size(12.0)
                    .color(col(0.75, 0.75, 0.75, 1.0));
                Binding::new(cx, lens_col.value_signal(PAN_IDS[b]), {
                    let lens_col = lens_col.clone();
                    move |cx| {
                        let pan = lens_col.get_plain(PAN_IDS[b]);
                        let pan_display = Signal::new(pan);
                        let lens_pan = lens_col.clone();
                        HSliderView::new(cx, -1.0, 1.0, pan, 0.0, move |_cx, g| match g {
                            Gesture::Start => lens_pan.begin_edit(PAN_IDS[b]),
                            Gesture::Change(v) => {
                                let norm = ((v as f64) + 1.0) / 2.0;
                                lens_pan.set(PAN_IDS[b], norm.clamp(0.0, 1.0));
                                pan_display.set(v);
                            }
                            Gesture::End => lens_pan.end_edit(PAN_IDS[b]),
                        })
                        .width(Stretch(1.0))
                        .height(Pixels(22.0));
                        Label::new(cx, Memo::new(move |_| format_pan(pan_display.get())))
                            .font_size(11.0)
                            .color(col(0.8, 0.8, 0.8, 1.0));
                    }
                });

                styled_toggle_dyn(cx, lens_col.clone(), SOLO_IDS[b], "SOLO", "SOLO ON");
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
}

// ─── Right sidebar (output level, pre-master, auto loud, goniometer) ───────

fn build_right_sidebar(
    cx: &mut Context,
    telemetry: Signal<Telemetry>,
    lens: ParamLens<EquilibriumParams>,
    shared: Arc<SharedState>,
) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "OUTPUT LEVEL")
            .font_size(12.0)
            .color(col(0.75, 0.75, 0.75, 1.0));

        let lens_hs = lens.clone();
        let shared_hs = shared.clone();
        HStack::new(cx, move |cx| {
            Binding::new(cx, lens_hs.value_signal(K::OutputGain), {
                let lens_hs = lens_hs.clone();
                move |cx| {
                    let out_gain = lens_hs.get_plain(K::OutputGain);
                    let lens_knob = lens_hs.clone();
                    let out_gain_display = Signal::new(out_gain);
                    let norm = ((out_gain + 12.0) / 24.0).clamp(0.0, 1.0);
                    VStack::new(cx, move |cx| {
                        KnobView::new(cx, norm, 0.5, -12.0, 12.0, true, move |_cx, g| match g {
                            Gesture::Start => lens_knob.begin_edit(K::OutputGain),
                            Gesture::Change(v) => {
                                let n = ((v as f64) + 12.0) / 24.0;
                                lens_knob.set(K::OutputGain, n.clamp(0.0, 1.0));
                                out_gain_display.set(v);
                            }
                            Gesture::End => lens_knob.end_edit(K::OutputGain),
                        })
                        .width(Pixels(40.0))
                        .height(Pixels(40.0));
                        Label::new(
                            cx,
                            Memo::new(move |_| format!("{:.1} dB", out_gain_display.get())),
                        )
                        .font_size(10.0)
                        .color(rgb(1.0, 0.65, 0.3));
                        Label::new(cx, "OUT GAIN")
                            .font_size(9.0)
                            .color(col(0.75, 0.75, 0.75, 1.0));
                    })
                    .alignment(Alignment::Center)
                    .width(Auto);
                }
            });

            VStack::new(cx, move |cx| {
                // PRE-MASTER: small toggle for tight right-sidebar layout
                {
                    let sig = lens_hs.value_signal(K::PreMasterActive);
                    let lens_pm = lens_hs.clone();
                    Binding::new(cx, sig, move |cx| {
                        let active = lens_pm.get(K::PreMasterActive) > 0.5;
                        let lens_pm = lens_pm.clone();
                        shared_ui::toggle_button_small(cx, "PRE-MASTER", active, move |_cx| {
                            let now = lens_pm.get(K::PreMasterActive) <= 0.5;
                            let norm = if now { 1.0 } else { 0.0 };
                            lens_pm.automate(K::PreMasterActive, norm);
                            sig.set(norm as f32);
                        });
                    });
                }
                Binding::new(cx, lens_hs.value_signal(K::PreMasterTargetDb), {
                    let lens_hs = lens_hs.clone();
                    move |cx| {
                        let pre_target = lens_hs.get_plain(K::PreMasterTargetDb);
                        let pre_display = Signal::new(pre_target);
                        Label::new(
                            cx,
                            Memo::new(move |_| format!("Target: {:.1} dB", pre_display.get())),
                        )
                        .font_size(10.0)
                        .color(col(0.75, 0.75, 0.75, 1.0));
                        let lens_pre = lens_hs.clone();
                        HSliderView::new(cx, -6.0, -3.0, pre_target, -6.0, move |_cx, g| match g {
                            Gesture::Start => lens_pre.begin_edit(K::PreMasterTargetDb),
                            Gesture::Change(v) => {
                                let n = ((v as f64) + 6.0) / 3.0;
                                lens_pre.set(K::PreMasterTargetDb, n.clamp(0.0, 1.0));
                                pre_display.set(v);
                            }
                            Gesture::End => lens_pre.end_edit(K::PreMasterTargetDb),
                        })
                        .width(Stretch(1.0))
                        .height(Pixels(22.0));
                    }
                });

                // Built once, not inside a `Binding` on `telemetry` - `tick()`
                // calls `telemetry.update(...)` unconditionally every ~33ms
                // (no equality check), so a `Binding` here would tear down
                // and rebuild this Button every tick and drop clicks the
                // same way the header mode button did in Lucent's pilot
                // (see module doc). `Memo` instead updates label/color on
                // this same persistent entity in place; the disabled-while-
                // pre-master-active behaviour is a guard inside `on_press`
                // rather than conditionally attaching the handler.
                let shared_al = shared_hs.clone();
                let lens_al = lens_hs.clone();
                Button::new(cx, move |cx| {
                    Label::new(
                        cx,
                        Memo::new(move |_| {
                            if telemetry.get().auto_loud_measuring {
                                "MEASURING..."
                            } else {
                                "AUTO LOUD"
                            }
                        }),
                    )
                    .font_size(10.0)
                })
                .on_press(move |_cx| {
                    if lens_al.get(K::PreMasterActive) > 0.5 {
                        return;
                    }
                    shared_al.auto_loud_trigger.store(true, Ordering::Release);
                })
                .background_color(Memo::new(move |_| {
                    let t = telemetry.get();
                    let pre_active = lens_hs.get(K::PreMasterActive) > 0.5;
                    let is_active = shared_hs
                        .auto_loud_gain_offset
                        .load(Ordering::Acquire)
                        .abs()
                        > 0.05;
                    if pre_active {
                        col(0.1, 0.1, 0.1, 1.0)
                    } else if t.auto_loud_measuring {
                        rgb(1.0, 0.8, 0.0)
                    } else if is_active {
                        shared_ui::AMBER
                    } else {
                        shared_ui::IDLE_BG
                    }
                }))
                .height(Pixels(shared_ui::BUTTON_HEIGHT));
            })
            .width(Auto)
            .height(Auto)
            .vertical_gap(Pixels(4.0));
        })
        .width(Stretch(1.0))
        .height(Auto)
        .horizontal_gap(Pixels(4.0))
        .alignment(Alignment::Center);

        let shared_reset = shared.clone();
        Binding::new(cx, telemetry, move |cx| {
            let t = telemetry.get();
            StereoMeterView::new(
                cx,
                t.peak_l,
                t.peak_r,
                t.peak_hold_l,
                t.peak_hold_r,
                t.balance,
            )
            .width(Stretch(1.0))
            .height(Pixels(shared_ui::STEREO_METER_HEIGHT));

            let shared_l = shared_reset.clone();
            let shared_r = shared_reset.clone();
            HStack::new(cx, move |cx| {
                Button::new(cx, move |cx| {
                    Label::new(cx, fmt_db(t.peak_hold_l)).font_size(11.0)
                })
                .on_press(move |_cx| shared_l.reset_peak.store(true, Ordering::Release))
                .background_color(Color::transparent())
                .color(rgb(1.0, 0.45, 0.1));
                Element::new(cx).width(Stretch(1.0));
                Label::new(cx, "dB")
                    .font_size(10.0)
                    .color(col(0.8, 0.8, 0.8, 1.0));
                Element::new(cx).width(Stretch(1.0));
                Button::new(cx, move |cx| {
                    Label::new(cx, fmt_db(t.peak_hold_r)).font_size(11.0)
                })
                .on_press(move |_cx| shared_r.reset_peak.store(true, Ordering::Release))
                .background_color(Color::transparent())
                .color(rgb(1.0, 0.45, 0.1));
            })
            .width(Stretch(1.0))
            .height(Auto)
            .alignment(Alignment::Center);
        });

        Element::new(cx).height(Stretch(1.0));

        Label::new(cx, "GONIOMETER")
            .font_size(10.0)
            .color(col(0.6, 0.6, 0.6, 1.0));
        let shared_gonio = shared.clone();
        Binding::new(cx, telemetry, move |cx| {
            let t = telemetry.get();
            GoniometerView::new(
                cx,
                shared_gonio.scope_samples.clone(),
                shared_gonio.scope_write_pos.load(Ordering::Acquire),
                t.phase_correlation,
            )
            .width(Stretch(1.0))
            .height(Pixels(115.0));
        });
    })
    .width(Pixels(155.0))
    .height(Stretch(1.0))
    .padding(Pixels(8.0))
    .vertical_gap(Pixels(6.0))
    .background_color(rgb(0.1, 0.1, 0.1));
}

// ─── Footer (analyze / mono floor / reset all) ──────────────────────────────

fn build_footer(
    cx: &mut Context,
    telemetry: Signal<Telemetry>,
    lens: ParamLens<EquilibriumParams>,
    shared: Arc<SharedState>,
    accum: Arc<Mutex<TickAccum>>,
    selected_preset: Signal<Option<usize>>,
    params_gen: Signal<u32>,
    bool_sigs: BoolSignals,
) {
    let lens_analyze = lens.clone();
    let shared_analyze = shared.clone();
    let lens_mono = lens.clone();
    HStack::new(cx, move |cx| {
        VStack::new(cx, move |cx| {
            Label::new(cx, "ANALYZE")
                .font_size(10.0)
                .color(rgb(1.0, 0.55, 0.15));
            HStack::new(cx, move |cx| {
                // LISTEN: big toggle, amber text always (even when inactive)
                {
                    let sig = bool_sigs.listen;
                    let lens_listen = lens_analyze.clone();
                    Binding::new(cx, sig, move |cx| {
                        let active = lens_listen.get(K::ListenActive) > 0.5;
                        let lens_listen = lens_listen.clone();
                        shared_ui::toggle_button_big_amber_text(
                            cx,
                            if active { "LISTEN ON" } else { "LISTEN" },
                            active,
                            move |_cx| {
                                let now = lens_listen.get(K::ListenActive) <= 0.5;
                                let norm = if now { 1.0 } else { 0.0 };
                                lens_listen.automate(K::ListenActive, norm);
                                sig.set(norm as f32);
                            },
                        )
                        .width(Pixels(110.0));
                    });
                }

                let listen_sig = bool_sigs.listen;
                let shared_apply = shared_analyze.clone();
                let shared_ra = shared_analyze.clone();

                // APPLY / RESET are always clickable so they give hover
                // feedback; the actual work is gated by the current Listen
                // state read fresh from the shared param signal.
                shared_ui::push_button_big(cx, "APPLY ANALYSIS", move |_cx| {
                    if listen_sig.get() <= 0.5 {
                        return;
                    }
                    let t = telemetry.get();
                    if t.listen_samples > 100.0 {
                        for b in 0..5 {
                            shared_apply.target_levels[b]
                                .store(t.listen_levels[b], Ordering::Release);
                            shared_apply.target_tolerances[b]
                                .store(t.listen_tolerances[b], Ordering::Release);
                        }
                    }
                })
                .width(Pixels(120.0));

                shared_ui::push_button_big(cx, "RESET ANALYSIS", move |_cx| {
                    if listen_sig.get() <= 0.5 {
                        return;
                    }
                    shared_ra.reset_analysis.store(true, Ordering::Release);
                    shared_ra.listen_samples.store(0.0, Ordering::Release);
                    for b in 0..5 {
                        shared_ra.listen_levels[b].store(-90.0, Ordering::Release);
                        shared_ra.listen_tolerances[b].store(0.0, Ordering::Release);
                    }
                })
                .width(Pixels(120.0));
            })
            .horizontal_gap(Pixels(8.0))
            .alignment(Alignment::Center)
            .height(Auto);
        })
        .vertical_gap(Pixels(4.0))
        .alignment(Alignment::Center)
        .width(Pixels(376.0))
        .padding(Pixels(5.0));

        Element::new(cx).width(Stretch(1.0));

        VStack::new(cx, move |cx| {
            Label::new(cx, "MONO FLOOR")
                .font_size(10.0)
                .color(rgb(1.0, 0.55, 0.15));
            Binding::new(cx, lens_mono.value_signal(K::MonoFloor), {
                let lens_mono = lens_mono.clone();
                move |cx| {
                    let mf = lens_mono.get_plain(K::MonoFloor);
                    let lens_mf = lens_mono.clone();
                    let mf_display = Signal::new(mf);
                    KnobView::new(
                        cx,
                        (mf / 300.0).clamp(0.0, 1.0),
                        0.0,
                        0.0,
                        300.0,
                        false,
                        move |_cx, g| match g {
                            Gesture::Start => lens_mf.begin_edit(K::MonoFloor),
                            Gesture::Change(v) => {
                                let n = (v as f64) / 300.0;
                                lens_mf.set(K::MonoFloor, n.clamp(0.0, 1.0));
                                mf_display.set(v);
                            }
                            Gesture::End => lens_mf.end_edit(K::MonoFloor),
                        },
                    )
                    .width(Pixels(40.0))
                    .height(Pixels(40.0));
                    Label::new(
                        cx,
                        Memo::new(move |_| {
                            format!("{} Hz", format_knob_value(mf_display.get(), 300.0))
                        }),
                    )
                    .font_size(10.0)
                    .color(rgb(1.0, 0.65, 0.3));
                }
            });
        })
        .vertical_gap(Pixels(4.0))
        .alignment(Alignment::Center)
        .width(Auto)
        .padding(Pixels(5.0));

        shared_ui::danger_button_big(cx, "RESET", move |_cx| {
            reset_all(
                &lens,
                &shared,
                &accum,
                selected_preset,
                params_gen,
                &bool_sigs,
            )
        });
    })
    .width(Stretch(1.0))
    .height(Pixels(110.0))
    .padding_left(Pixels(100.0))
    .padding_right(Pixels(8.0))
    .padding_top(Pixels(8.0))
    .padding_bottom(Pixels(8.0))
    .alignment(Alignment::Center)
    .horizontal_gap(Pixels(10.0))
    .background_color(rgb(0.08, 0.08, 0.08));
}

// ─── Actions ─────────────────────────────────────────────────────────────────

/// Bands/tolerances are the Target Profile (analysis reference line, set via
/// APPLY ANALYSIS or an already-selected preset) - not the current gain
/// knob positions.
fn do_save_preset(
    accum: &Arc<Mutex<TickAccum>>,
    telemetry: &Signal<Telemetry>,
    preset_name_input: Signal<String>,
    lens: &ParamLens<EquilibriumParams>,
    params_gen: Signal<u32>,
) {
    let t = telemetry.get();
    let bands = t.target_levels;
    let tolerances = t.target_tolerances;

    let name_input = preset_name_input.get();
    let mut acc = accum.lock().unwrap();
    let name = if name_input.trim().is_empty() {
        format!("User Preset {}", acc.presets.len() + 1)
    } else {
        name_input.trim().to_string()
    };

    let dir = match &acc.vault_path {
        Some(vp) if !vp.is_empty() => std::path::PathBuf::from(vp),
        _ => shared_analysis::get_plugin_dir("Equilibrium").join("presets"),
    };
    let _ = std::fs::create_dir_all(&dir);
    let safe = name.replace(
        |c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_',
        "",
    );
    let fp = dir.join(format!("{safe}.md"));

    let prof = shared_analysis::Profile {
        name: name.clone(),
        bands,
        tolerances,
        pans: [
            lens.get_plain(K::LowPan),
            lens.get_plain(K::BassPan),
            lens.get_plain(K::MidPan),
            lens.get_plain(K::HighMidPan),
            lens.get_plain(K::HighPan),
        ],
        widths: [
            lens.get_plain(K::LowWidth),
            lens.get_plain(K::BassWidth),
            lens.get_plain(K::MidWidth),
            lens.get_plain(K::HighMidWidth),
            lens.get_plain(K::HighWidth),
        ],
        mono_floor_hz: lens.get_plain(K::MonoFloor),
        ..shared_analysis::Profile::default()
    };
    let md = shared_analysis::export_preset_to_markdown(&prof);
    if std::fs::write(&fp, &md).is_ok() {
        acc.presets = load_presets(acc.vault_path.as_deref());
        // Drop before params_gen.update(): the preset-list Binding it
        // triggers locks `accum` itself - held across that call, it
        // self-deadlocks (same non-reentrant-Mutex issue as tick()'s
        // telemetry.update() call, see comment there).
        drop(acc);
        preset_name_input.set(String::new());
        params_gen.update(|g| *g = g.wrapping_add(1));
    }
}

fn reset_all(
    lens: &ParamLens<EquilibriumParams>,
    shared: &SharedState,
    accum: &Arc<Mutex<TickAccum>>,
    selected_preset: Signal<Option<usize>>,
    params_gen: Signal<u32>,
    bool_sigs: &BoolSignals,
) {
    for (id, val) in [
        (K::LowGain, 0.0f64),
        (K::BassGain, 0.0),
        (K::MidGain, 0.0),
        (K::HighMidGain, 0.0),
        (K::HighGain, 0.0),
        (K::LowWidth, 100.0),
        (K::BassWidth, 100.0),
        (K::MidWidth, 100.0),
        (K::HighMidWidth, 100.0),
        (K::HighWidth, 100.0),
        (K::LowPan, 0.0),
        (K::BassPan, 0.0),
        (K::MidPan, 0.0),
        (K::HighMidPan, 0.0),
        (K::HighPan, 0.0),
        (K::OutputGain, 0.0),
        (K::MonoFloor, 0.0),
        (K::PreMasterTargetDb, -3.0),
    ] {
        let norm = param_norm(id, val);
        lens.automate(id, norm);
    }

    // Reset all bool/toggle parameters to false and push into their Signals so
    // the UI buttons repaint.
    let bool_resets = [
        (K::MonoActive, bool_sigs.mono),
        (K::DeltaActive, bool_sigs.delta),
        (K::BypassActive, bool_sigs.bypass),
        (K::PreMasterActive, bool_sigs.pre_master),
        (K::ListenActive, bool_sigs.listen),
        (K::SoloLow, bool_sigs.solos[0]),
        (K::SoloBass, bool_sigs.solos[1]),
        (K::SoloMid, bool_sigs.solos[2]),
        (K::SoloHighMid, bool_sigs.solos[3]),
        (K::SoloHigh, bool_sigs.solos[4]),
    ];
    for (id, sig) in bool_resets {
        lens.automate(id, 0.0);
        sig.set(0.0);
    }

    let acc = accum.lock().unwrap();
    let (target_levels, target_tolerances) = if let Some(prof) = acc.presets.first().map(|p| &p.2) {
        (prof.bands, prof.tolerances)
    } else {
        ([0.0f32; 5], shared_analysis::DEFAULT_TOLERANCES)
    };
    let has_presets = !acc.presets.is_empty();
    drop(acc);

    for b in 0..5 {
        shared.target_levels[b].store(target_levels[b], Ordering::Release);
        shared.target_tolerances[b].store(target_tolerances[b], Ordering::Release);
    }
    let sel = if has_presets { Some(0) } else { None };
    selected_preset.set(sel);
    if let Some(idx) = sel {
        shared.selected_preset_index.store(idx, Ordering::Release);
    }
    shared.auto_loud_gain_offset.store(0.0, Ordering::Release);
    shared.reset_analysis.store(true, Ordering::Release);

    params_gen.update(|g| *g = g.wrapping_add(1));
}

/// Normalize a plain param value against its known linear range - the
/// ranges are fixed per `EquilibriumParams`' `#[param(range = ...)]`
/// declarations, so this avoids threading `ParamInfo` lookups through
/// `reset_all`'s call sites just to invert a linear range.
fn param_norm(id: K, plain: f64) -> f64 {
    let (min, max) = match id {
        K::LowGain | K::BassGain | K::MidGain | K::HighMidGain | K::HighGain | K::OutputGain => {
            (-12.0, 12.0)
        }
        K::LowWidth | K::BassWidth | K::MidWidth | K::HighMidWidth | K::HighWidth => (0.0, 150.0),
        K::LowPan | K::BassPan | K::MidPan | K::HighMidPan | K::HighPan => (-1.0, 1.0),
        K::MonoFloor => (0.0, 300.0),
        K::PreMasterTargetDb => (-6.0, -3.0),
        _ => (0.0, 1.0),
    };
    ((plain - min) / (max - min)).clamp(0.0, 1.0)
}

// ─── SNAP Helpers (framework-independent, unchanged from the iced version) ──

fn snap_filename(vault_path: &str) -> String {
    let dir = std::path::Path::new(vault_path);
    let mut max_n = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let s = e.file_name().to_string_lossy().into_owned();
            if let Some(inner) = s
                .strip_prefix("SNAPSHOT-")
                .and_then(|r| r.strip_suffix(".md"))
                && let Ok(n) = inner.parse::<u32>()
            {
                max_n = max_n.max(n);
            }
        }
    }
    format!("SNAPSHOT-{:03}.md", max_n + 1)
}

fn snap_markdown(
    stereo: &[f32],
    mono: &[f32],
    delta: &[f32],
    band_levels: [f32; 5],
    corr: f32,
    pl: f32,
    pr: f32,
    sr: f32,
) -> String {
    let fft_sz = 2048.0;
    let freqs: &[f32] = &[
        20.0, 40.0, 80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10000.0, 16000.0, 20000.0,
    ];
    let tbl = |s: &[f32]| {
        freqs
            .iter()
            .map(|&f| {
                let bin = ((f * fft_sz / sr) as usize).min(s.len().saturating_sub(1));
                format!(
                    "| {} | {:.1} |",
                    if f >= 1000.0 {
                        format!("{:.0}k", f / 1000.0)
                    } else {
                        format!("{:.0}", f)
                    },
                    s[bin]
                )
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
        pl = pl,
        pr = pr,
        co = corr,
        st = tbl(stereo),
        mn = tbl(mono),
        dt = tbl(delta),
        b0 = band_levels[0],
        b1 = band_levels[1],
        b2 = band_levels[2],
        b3 = band_levels[3],
        b4 = band_levels[4],
    )
}
