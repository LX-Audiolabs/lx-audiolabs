//! Vizia port of the old iced `editor.rs`. Architecture note (see CLAP-vault
//! `features/2026-07-04-truce-2.0-upgrade-plan.md` for the "why"):
//!
//! Vizia is retained-mode + fine-grained reactive, not Elm/virtual-DOM like
//! iced was. `Binding::new(cx, signal, |cx| {...})` fully tears down and
//! rebuilds its subtree whenever `signal` changes - fine for stateless
//! widgets (Label, Button, our custom draw-only Views), but destructive for
//! *stateful* widgets (`Textbox` mid-edit, `KnobView` mid-drag): rebuilding
//! those every tick would reset cursor/focus/drag state constantly.
//!
//! So telemetry (spectrum, resonance/masking text, meters, relay list,
//! snap blink) lives in one `Signal<Telemetry>` updated by a 33ms root
//! timer (replaces the old `Message::Tick` / `RedrawRequested`
//! subscription), and only the passive display regions (right sidebar,
//! center spectrum+relay-bar+analyzer text, SNAP button) are wrapped in
//! `Binding`s keyed to it. The Name/Vault-path `Textbox`es and the
//! Sensitivity `KnobView` are built once, outside any tick-driven Binding,
//! so typing and dragging survive across ticks. The mode label/button gets
//! its own tiny `Binding` keyed to the param's own `ParamLens` signal
//! instead, so it updates on host automation too.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

use vizia::prelude::*;

use shared_analysis::{SharedState, SPECTRUM_BINS};
use truce_vizia::ParamLens;

use crate::ui::{LucentUiState, RelayData};
use crate::vizia_canvas::{
    GoniometerView, SpectrumConfig, SpectrumCurve, SpectrumView, StereoMeterView, fmt_db,
    rgb as vg_rgb,
};
use crate::vizia_widgets::{Gesture, KnobView, format_knob_value};
use crate::{LucentParams, LucentParamsParamId, read_masking, read_resonance};

const VERSION: &str = env!("CARGO_PKG_VERSION");

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

// ─── Telemetry (tick-frequency display state) ───────────────────────────────

#[derive(Clone)]
struct Telemetry {
    own_spectrum: Vec<f32>,
    relays: Vec<RelayData>,
    resonance_cache_own: Vec<(usize, f32)>,
    resonance_cache_relay: Vec<(usize, f32, Vec<String>)>,
    masking_cache: Vec<f32>,
    resonance_text: String,
    masking_text: String,
    show_resonance: bool,
    show_masking: bool,
    snap_blink: u32,
    peak_l: f32,
    peak_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    peak_hold: f32,
    phase_correlation: f32,
    balance: f32,
}

/// Bookkeeping that must persist across ticks but never touches the UI
/// directly - the display-hold accumulator windows and the relay EMA state
/// (`LucentUiState::sync_relays` needs the previous tick's list to smooth
/// against).
struct TickAccum {
    ui: LucentUiState,
    resonance_acc_own: HashMap<usize, f32>,
    resonance_acc_relay: HashMap<usize, (f32, Vec<String>)>,
    masking_acc: HashMap<usize, (f32, Vec<String>)>,
    display_window_start: Instant,
    vault_path: Option<String>,
}

const DISPLAY_HOLD_MS: u128 = 500;

#[allow(clippy::too_many_arguments)]
fn tick(
    shared: &SharedState,
    params: &LucentParams,
    lens: &ParamLens<LucentParams>,
    instance_key: usize,
    accum: &Rc<RefCell<TickAccum>>,
    telemetry: Signal<Telemetry>,
) {
    let mode = lens.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
    let mut acc = accum.borrow_mut();

    // Non-blocking, matching `process()`'s own `try_lock()` pattern on these
    // same mutexes (see lib.rs) - the GUI timer must never block waiting on
    // the realtime audio thread. On contention, keep last tick's value
    // (`acc.ui.own_spectrum` / `t.masking_cache` below) rather than flashing
    // to empty; a skipped tick at 33ms is invisible, a blocked GUI thread is not.
    let spectrum = shared.spectrum_avg.try_lock().ok().map(|s| s.to_vec());
    let lists = read_resonance(instance_key);
    let masking_cache = shared.masking_map.try_lock().ok().map(|m| m.to_vec());
    let masking_top = read_masking(instance_key);

    for &(bin, score) in &lists.own {
        acc.resonance_acc_own
            .entry(bin)
            .and_modify(|s| if score > *s { *s = score })
            .or_insert(score);
    }
    for (bin, score, names) in &lists.relay {
        acc.resonance_acc_relay
            .entry(*bin)
            .and_modify(|(s, n)| if *score > *s { *s = *score; *n = names.clone(); })
            .or_insert((*score, names.clone()));
    }
    for (bin, db, names) in &masking_top {
        acc.masking_acc
            .entry(*bin)
            .and_modify(|(d, n)| if *db > *d { *d = *db; *n = names.clone(); })
            .or_insert((*db, names.clone()));
    }

    let sample_rate = shared.sample_rate.load(Ordering::Relaxed).max(1.0);
    let refresh_text = acc.display_window_start.elapsed().as_millis() >= DISPLAY_HOLD_MS;
    let new_texts = if refresh_text {
        let rt = format_resonance_text(&acc.resonance_acc_own, &acc.resonance_acc_relay, sample_rate);
        let mt = format_masking_text(mode, &acc.masking_acc, acc.ui.relays.is_empty(), sample_rate);
        acc.resonance_acc_own.clear();
        acc.resonance_acc_relay.clear();
        acc.masking_acc.clear();
        acc.display_window_start = Instant::now();
        Some((rt, mt))
    } else {
        None
    };

    if let Some(spectrum) = spectrum {
        acc.ui.own_spectrum = spectrum;
    }
    if mode != 0 {
        let now_ms = shared_analysis::shm::now_ms();
        let slot = shared.shm_slot.load(Ordering::Acquire);
        let raw = params.name.try_read().map(|n| n.clone()).unwrap_or_default();
        let my_name = if slot >= 0 { shared_analysis::shm::display_name(&raw, slot as u8) } else { raw };
        let feeds = shared_analysis::relay_hub()
            .map(|hub| hub.read_active(&my_name, now_ms))
            .unwrap_or_default();
        acc.ui.sync_relays(feeds);
    } else {
        acc.ui.clear_relays();
    }

    let peak_l = shared.output_peak_l.load(Ordering::Relaxed);
    let peak_r = shared.output_peak_r.load(Ordering::Relaxed);
    let peak_hold_l = shared.peak_hold_l.load(Ordering::Relaxed);
    let peak_hold_r = shared.peak_hold_r.load(Ordering::Relaxed);
    let peak_hold = shared.peak_hold.load(Ordering::Relaxed);
    let phase_correlation = shared.phase_correlation.load(Ordering::Relaxed);
    let balance = shared.balance.load(Ordering::Relaxed);
    let snap_now = shared.snap_active.load(Ordering::Relaxed);

    telemetry.update(|t| {
        t.own_spectrum = acc.ui.own_spectrum.clone();
        t.relays = acc.ui.relays.clone();
        t.resonance_cache_own = lists.own;
        t.resonance_cache_relay = lists.relay;
        if let Some(masking_cache) = masking_cache {
            t.masking_cache = masking_cache;
        }
        if let Some((rt, mt)) = new_texts {
            t.resonance_text = rt;
            t.masking_text = mt;
        }
        t.peak_l = peak_l;
        t.peak_r = peak_r;
        t.peak_hold_l = peak_hold_l;
        t.peak_hold_r = peak_hold_r;
        t.peak_hold = peak_hold;
        t.phase_correlation = phase_correlation;
        t.balance = balance;

        if snap_now {
            t.snap_blink = 72;
        } else if t.snap_blink == 1 {
            if let Some(ref vp) = acc.vault_path {
                if !vp.is_empty() {
                    let stereo = shared.snap_stereo_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let mono = shared.snap_mono_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let delta = shared.snap_delta_snap.try_lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let sr = shared.sample_rate.load(Ordering::Relaxed);
                    let md = snap_markdown(&stereo, &mono, &delta, &acc.ui.relays, phase_correlation, peak_l, peak_r, sr);
                    let fname = snap_filename(vp);
                    let _ = std::fs::write(std::path::Path::new(vp).join(&fname), &md);
                }
            }
        }
        if t.snap_blink > 0 {
            t.snap_blink -= 1;
        }
    });
}

fn format_resonance_text(
    acc_own: &HashMap<usize, f32>,
    acc_relay: &HashMap<usize, (f32, Vec<String>)>,
    sample_rate: f32,
) -> String {
    if acc_own.is_empty() && acc_relay.is_empty() {
        return "No resonances detected".to_string();
    }
    let fft_size = (SPECTRUM_BINS * 2) as f32;

    let mut own: Vec<(usize, f32)> = acc_own.iter().map(|(&bin, &score)| (bin, score)).collect();
    own.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut relay: Vec<(usize, f32, Vec<String>)> = acc_relay
        .iter()
        .map(|(&bin, (score, names))| (bin, *score, names.clone()))
        .collect();
    relay.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let fmt = |peaks: &[(usize, f32)]| -> String {
        peaks
            .iter()
            .take(3)
            .map(|(bin, score)| {
                let freq = *bin as f32 * sample_rate / fft_size;
                format!("{:.0} Hz {:.1}", freq, score)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let fmt_relay = |peaks: &[(usize, f32, Vec<String>)]| -> String {
        peaks
            .iter()
            .take(3)
            .map(|(bin, score, contributors)| {
                let freq = *bin as f32 * sample_rate / fft_size;
                if contributors.is_empty() {
                    format!("{:.0} Hz {:.1}", freq, score)
                } else {
                    format!("{:.0} Hz {:.1} ({})", freq, score, contributors.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines = Vec::new();
    if !own.is_empty() {
        lines.push(format!("Own: {}", fmt(&own)));
    }
    if !relay.is_empty() {
        lines.push(format!("Group: {}", fmt_relay(&relay)));
    }
    lines.join("\n")
}

fn format_masking_text(
    mode: i64,
    acc: &HashMap<usize, (f32, Vec<String>)>,
    relays_empty: bool,
    sample_rate: f32,
) -> String {
    if mode == 0 {
        return "Standalone — no masking".to_string();
    }
    if acc.is_empty() || relays_empty {
        return "No masking detected".to_string();
    }
    let fft_size = (SPECTRUM_BINS * 2) as f32;
    let mut peaks: Vec<(usize, f32, Vec<String>)> =
        acc.iter().map(|(&bin, (db, names))| (bin, *db, names.clone())).collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    peaks.truncate(3);
    peaks
        .iter()
        .map(|(bin, db, contributors)| {
            let freq = *bin as f32 * sample_rate / fft_size;
            if contributors.is_empty() {
                format!("{:.0} Hz  {:.1} dB", freq, db)
            } else {
                format!("{:.0} Hz  {:.1} dB ({})", freq, db, contributors.join("-"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ─── UI ──────────────────────────────────────────────────────────────────────

pub fn build(cx: &mut Context, lens: ParamLens<LucentParams>, shared: Arc<SharedState>, params: Arc<LucentParams>) {
    let instance_key = Arc::as_ptr(&params) as usize;
    let config = shared_analysis::load_config("Lucent");

    let mut initial_name = "Lucent".to_string();
    if let Ok(name) = params.name.read() {
        if !name.is_empty() {
            initial_name = name.clone();
        }
    }

    let telemetry = Signal::new(Telemetry {
        own_spectrum: Vec::new(),
        relays: Vec::new(),
        resonance_cache_own: Vec::new(),
        resonance_cache_relay: Vec::new(),
        masking_cache: Vec::new(),
        resonance_text: "No resonances detected".to_string(),
        masking_text: "No masking detected".to_string(),
        show_resonance: false,
        show_masking: false,
        snap_blink: 0,
        peak_l: -90.0,
        peak_r: -90.0,
        peak_hold_l: -90.0,
        peak_hold_r: -90.0,
        peak_hold: -90.0,
        phase_correlation: 1.0,
        balance: 0.0,
    });
    let setup_visible = Signal::new(false);
    let vault_path_input = Signal::new(config.vault_path.clone().unwrap_or_default());
    let name_signal = Signal::new(initial_name);
    let sensitivity_display = Signal::new(lens.get_plain(LucentParamsParamId::Sensitivity));

    let accum = Rc::new(RefCell::new(TickAccum {
        ui: LucentUiState::new(),
        resonance_acc_own: HashMap::new(),
        resonance_acc_relay: HashMap::new(),
        masking_acc: HashMap::new(),
        display_window_start: Instant::now(),
        vault_path: config.vault_path,
    }));

    {
        let shared_for_timer = shared.clone();
        let params_for_timer = params.clone();
        let lens_for_timer = lens.clone();
        let accum_for_timer = accum.clone();
        let timer = cx.add_timer(Duration::from_millis(33), None, move |_cx, action| {
            if !matches!(action, TimerAction::Tick(_)) {
                return;
            }
            tick(&shared_for_timer, &params_for_timer, &lens_for_timer, instance_key, &accum_for_timer, telemetry);
        });
        cx.start_timer(timer);
    }

    // ── HEADER ──────────────────────────────────────────────────────────────
    let shared_header = shared.clone();
    let lens_header = lens.clone();
    HStack::new(cx, move |cx| {
        let shared = shared_header;
        let lens = lens_header;
        HStack::new(cx, |cx| {
            Label::new(cx, "LX").font_size(20.0).color(rgb(1.0, 0.45, 0.1));
            Label::new(cx, "AUDIOLABS").font_size(20.0).color(Color::white());
        })
        .width(Auto)
        .height(Auto)
        .horizontal_gap(Pixels(6.0))
        .alignment(Alignment::Center);

        Element::new(cx).width(Stretch(1.0));

        Textbox::new(cx, name_signal)
            .on_edit(move |_cx, text| {
                if let Ok(mut n) = params.name.write() {
                    *n = text.clone();
                }
                name_signal.set(text);
            })
            .width(Pixels(130.0));

        Element::new(cx).width(Stretch(1.0));

        let shared_for_reset = shared.clone();
        HStack::new(cx, move |cx| {
            let mode_signal = lens.value_signal(LucentParamsParamId::AnalyzeMode);
            let lens_for_mode = lens.clone();
            Binding::new(cx, mode_signal, move |cx| {
                let mode = lens_for_mode.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
                let label = match mode { 0 => "STANDALONE", 2 => "RELAY", _ => "HYBRID" };
                let lens_press = lens_for_mode.clone();
                Button::new(cx, |cx| Label::new(cx, label).font_size(11.0))
                    .on_press(move |_cx| {
                        let current = lens_press.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
                        let next: i64 = match current { 1 => 0, 0 => 2, _ => 1 };
                        lens_press.automate(LucentParamsParamId::AnalyzeMode, next as f64 / 2.0);
                        lens_press.value_signal(LucentParamsParamId::AnalyzeMode).set(next as f32 / 2.0);
                    })
                    .width(Pixels(110.0))
                    .background_color(col(0.15, 0.1, 0.05, 1.0));
            });

            Button::new(cx, |cx| Label::new(cx, "RESET").font_size(12.0))
                .on_press(move |_cx| {
                    shared_for_reset.reset_peak.store(true, Ordering::Relaxed);
                    shared_for_reset.peak_hold.store(-100.0, Ordering::Relaxed);
                    shared_for_reset.peak_hold_l.store(-100.0, Ordering::Relaxed);
                    shared_for_reset.peak_hold_r.store(-100.0, Ordering::Relaxed);
                })
                .background_color(col(0.2, 0.08, 0.08, 1.0));
        })
        .width(Auto)
        .height(Auto)
        .horizontal_gap(Pixels(6.0))
        .alignment(Alignment::Center);
    })
    .width(Stretch(1.0))
    .height(Pixels(50.0))
    .padding(Pixels(8.0))
    .alignment(Alignment::Center)
    .background_color(rgb(0.08, 0.08, 0.08));

    HStack::new(cx, move |cx| {
        // ── LEFT SIDEBAR ─────────────────────────────────────────────────
        let shared_for_snap = shared.clone();
        VStack::new(cx, move |cx| {
            Label::new(cx, "LX AUDIOLABS").font_size(14.0).color(Color::white());

            let shared_for_snap = shared_for_snap.clone();
            Binding::new(cx, telemetry, move |cx| {
                let t = telemetry.get();
                let blink = t.snap_blink > 0;
                let label = if blink { "ANALYZING..." } else { "SNAP" };
                let bg = if blink { col(0.55, 0.38, 0.05, 1.0) } else { col(0.18, 0.18, 0.18, 1.0) };
                let fg = if blink { rgb(1.0, 0.85, 0.3) } else { rgb(1.0, 0.55, 0.1) };
                let shared_press = shared_for_snap.clone();
                Button::new(cx, move |cx| Label::new(cx, label).font_size(12.0).color(fg))
                    .on_press(move |_cx| {
                        shared_press.snap_active.store(true, Ordering::Relaxed);
                    })
                    .width(Stretch(1.0))
                    .height(Pixels(34.0))
                    .background_color(bg);
            });

            Button::new(cx, |cx| Label::new(cx, "VAULT SETUP").font_size(12.0))
                .on_press(move |_cx| {
                    let now = !setup_visible.get();
                    setup_visible.set(now);
                })
                .width(Stretch(1.0))
                .height(Pixels(34.0))
                .background_color(col(0.18, 0.18, 0.18, 1.0));
        })
        .width(Pixels(180.0))
        .height(Stretch(1.0))
        .padding(Pixels(10.0))
        .vertical_gap(Pixels(10.0))
        .background_color(rgb(0.09, 0.09, 0.09));

        // ── CENTER ──────────────────────────────────────────────────────
        VStack::new(cx, move |cx| {
            Binding::new(cx, setup_visible, move |cx| {
                if setup_visible.get() {
                    build_setup_form(cx, vault_path_input, setup_visible);
                } else {
                    build_main_panel(cx, telemetry, lens.clone(), sensitivity_display);
                }
            });
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0))
        .background_color(rgb(0.08, 0.08, 0.08));

        // ── RIGHT SIDEBAR ─────────────────────────────────────────────────
        let shared_for_gonio = shared.clone();
        VStack::new(cx, move |cx| {
            Label::new(cx, "OUTPUT LEVEL").font_size(12.0).color(col(0.75, 0.75, 0.75, 1.0));

            Binding::new(cx, telemetry, move |cx| {
                let t = telemetry.get();
                VStack::new(cx, move |cx| {
                    StereoMeterView::new(cx, t.peak_l, t.peak_r, t.peak_hold_l, t.peak_hold_r, t.balance)
                        .width(Stretch(1.0))
                        .height(Pixels(70.0));

                    HStack::new(cx, move |cx| {
                        Label::new(cx, fmt_db(t.peak_hold_l)).font_size(11.0).color(rgb(1.0, 0.45, 0.1));
                        Element::new(cx).width(Stretch(1.0));
                        Label::new(cx, "dB").font_size(10.0).color(col(0.8, 0.8, 0.8, 1.0));
                        Element::new(cx).width(Stretch(1.0));
                        Label::new(cx, fmt_db(t.peak_hold_r)).font_size(11.0).color(rgb(1.0, 0.45, 0.1));
                    })
                    .width(Stretch(1.0))
                    .alignment(Alignment::Center);
                })
                .height(Auto)
                .vertical_gap(Pixels(4.0));
            });

            Label::new(cx, "GONIOMETER").font_size(10.0).color(col(0.6, 0.6, 0.6, 1.0));

            Binding::new(cx, telemetry, move |cx| {
                let t = telemetry.get();
                GoniometerView::new(cx, shared_for_gonio.scope_samples.clone(), shared_for_gonio.scope_write_pos.load(Ordering::Acquire), t.phase_correlation)
                    .width(Stretch(1.0))
                    .height(Pixels(139.0));
            });
        })
        .width(Pixels(155.0))
        .height(Stretch(1.0))
        .padding(Pixels(8.0))
        .vertical_gap(Pixels(6.0))
        .background_color(rgb(0.1, 0.1, 0.1));
    })
    .width(Stretch(1.0))
    .height(Stretch(1.0));
}

fn build_setup_form(cx: &mut Context, vault_path_input: Signal<String>, setup_visible: Signal<bool>) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "LX AUDIOLABS - SETUP").font_size(18.0).color(Color::white());
        Label::new(cx, "Configure your Vault path for Lucent:").font_size(12.0).color(Color::white());
        Textbox::new(cx, vault_path_input)
            .placeholder("Enter Vault absolute path...")
            .on_edit(move |_cx, text| vault_path_input.set(text))
            .width(Stretch(1.0));
        HStack::new(cx, move |cx| {
            Button::new(cx, |cx| Label::new(cx, "SAVE"))
                .on_press(move |_cx| {
                    let path = vault_path_input.get();
                    let new_path = if path.trim().is_empty() { None } else { Some(path.trim().to_string()) };
                    let cfg = shared_analysis::PluginConfig { vault_path: new_path, ..Default::default() };
                    let _ = shared_analysis::save_config("Lucent", &cfg);
                    setup_visible.set(false);
                })
                .background_color(col(0.15, 0.15, 0.15, 1.0));
            Button::new(cx, |cx| Label::new(cx, "CANCEL"))
                .on_press(move |_cx| setup_visible.set(false))
                .background_color(col(0.15, 0.15, 0.15, 1.0));
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

fn build_main_panel(
    cx: &mut Context,
    telemetry: Signal<Telemetry>,
    lens: ParamLens<LucentParams>,
    sensitivity_display: Signal<f32>,
) {
    let mode = lens.get_plain(LucentParamsParamId::AnalyzeMode) as i64;

    // Relay bar + spectrum canvas — passive display, safe to rebuild every tick.
    Binding::new(cx, telemetry, move |cx| {
        let t = telemetry.get();

        if mode != 0 {
            let relays_for_bar = t.relays.clone();
            HStack::new(cx, move |cx| {
                Label::new(cx, "RELAYS").font_size(10.0).color(rgb(1.0, 0.55, 0.15));
                if relays_for_bar.is_empty() {
                    Label::new(cx, "— send a relay from another LX plugin —")
                        .font_size(10.0)
                        .color(col(0.4, 0.4, 0.4, 1.0));
                } else {
                    for (idx, relay) in relays_for_bar.iter().enumerate().take(6) {
                        let active = relay.active;
                        let name = relay.name.clone();
                        let telemetry_press = telemetry;
                        Button::new(cx, move |cx| Label::new(cx, name.clone()).font_size(11.0))
                            .on_press(move |_cx| {
                                telemetry_press.update(|t| {
                                    if let Some(r) = t.relays.get_mut(idx) {
                                        r.active = !r.active;
                                    }
                                });
                            })
                            .background_color(if active { rgb(1.0, 0.45, 0.1) } else { col(0.15, 0.15, 0.15, 1.0) });
                    }
                }
            })
            .width(Stretch(1.0))
            .height(Pixels(48.0))
            .padding(Pixels(8.0))
            .horizontal_gap(Pixels(6.0))
            .alignment(Alignment::Center)
            .background_color(rgb(0.09, 0.09, 0.09));
        }

        let curves = if t.relays.is_empty() || mode == 0 {
            vec![SpectrumCurve {
                spectrum: t.own_spectrum.clone(),
                color: vg_rgb(0.1, 0.9, 0.7),
                fill_alpha: 0.18,
                line_alpha: 0.85,
                line_width: 1.2,
            }]
        } else {
            let relay_colors = [
                vg_rgb(1.0, 0.6, 0.2), vg_rgb(0.8, 0.3, 0.3), vg_rgb(0.3, 0.8, 0.5),
                vg_rgb(0.4, 0.6, 1.0), vg_rgb(0.9, 0.7, 0.3), vg_rgb(0.7, 0.4, 0.8),
            ];
            let mut curves = vec![SpectrumCurve {
                spectrum: t.own_spectrum.clone(),
                color: vg_rgb(0.1, 0.9, 0.7),
                fill_alpha: 0.12,
                line_alpha: 0.6,
                line_width: 1.0,
            }];
            for (idx, relay) in t.relays.iter().filter(|r| r.active).enumerate() {
                curves.push(SpectrumCurve {
                    spectrum: relay.spectrum.clone(),
                    color: relay_colors[idx % relay_colors.len()],
                    fill_alpha: 0.08,
                    line_alpha: 0.5,
                    line_width: 0.8,
                });
            }
            curves
        };
        let resonance_peaks = if t.show_resonance {
            let mut v = t.resonance_cache_own.clone();
            v.extend(t.resonance_cache_relay.iter().map(|(bin, score, _)| (*bin, *score)));
            v
        } else {
            Vec::new()
        };
        let masking = if t.show_masking && (!t.relays.is_empty() || mode == 2) {
            t.masking_cache.clone()
        } else {
            Vec::new()
        };

        SpectrumView::new(cx, SpectrumView {
            curves,
            config: SpectrumConfig::default(),
            resonance_peaks,
            masking,
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0));
    });

    // Analyzer row: resonance/masking text panels (tick-driven) + sensitivity
    // knob (built once, outside the tick Binding, so drag state survives).
    HStack::new(cx, move |cx| {
        Binding::new(cx, telemetry, move |cx| {
            let t = telemetry.get();
            HStack::new(cx, move |cx| {
                VStack::new(cx, move |cx| {
                    Label::new(cx, "RESONANCE").font_size(10.0).color(rgb(1.0, 0.55, 0.15));
                    Label::new(cx, t.resonance_text.clone()).font_size(10.0).color(col(0.8, 0.8, 0.8, 1.0));
                })
                .width(Stretch(1.0));
                let show = t.show_resonance;
                Button::new(cx, move |cx| Label::new(cx, if show { "ON" } else { "OFF" }))
                    .on_press(move |_cx| telemetry.update(|t| t.show_resonance = !t.show_resonance))
                    .background_color(if show { rgb(1.0, 0.45, 0.1) } else { col(0.15, 0.15, 0.15, 1.0) });
            })
            .width(Stretch(1.0))
            .height(Pixels(88.0))
            .padding(Pixels(6.0))
            .background_color(rgb(0.1, 0.1, 0.1));

            HStack::new(cx, move |cx| {
                VStack::new(cx, move |cx| {
                    Label::new(cx, "MASKING").font_size(10.0).color(rgb(0.95, 0.22, 0.18));
                    Label::new(cx, t.masking_text.clone()).font_size(10.0).color(col(0.8, 0.8, 0.8, 1.0));
                })
                .width(Stretch(1.0));
                let show = t.show_masking;
                if mode == 0 {
                    Label::new(cx, "OFF").color(col(0.35, 0.35, 0.35, 1.0));
                } else {
                    Button::new(cx, move |cx| Label::new(cx, if show { "ON" } else { "OFF" }))
                        .on_press(move |_cx| telemetry.update(|t| t.show_masking = !t.show_masking))
                        .background_color(if show { rgb(0.95, 0.22, 0.18) } else { col(0.15, 0.15, 0.15, 1.0) });
                }
            })
            .width(Stretch(1.0))
            .height(Pixels(88.0))
            .padding(Pixels(6.0))
            .background_color(rgb(0.1, 0.1, 0.1));
        });

        VStack::new(cx, move |cx| {
            let lens_knob = lens.clone();
            KnobView::new(
                cx,
                lens.get_plain(LucentParamsParamId::Sensitivity),
                0.5,
                0.0,
                100.0,
                false,
                move |_cx, g| match g {
                    Gesture::Start => lens_knob.begin_edit(LucentParamsParamId::Sensitivity),
                    Gesture::Change(v) => {
                        let norm = (v / 100.0).clamp(0.0, 1.0);
                        lens_knob.set(LucentParamsParamId::Sensitivity, norm as f64);
                        sensitivity_display.set(v);
                    }
                    Gesture::End => lens_knob.end_edit(LucentParamsParamId::Sensitivity),
                },
            )
            .width(Pixels(48.0))
            .height(Pixels(48.0));

            Label::new(cx, Memo::new(move |_| format_knob_value(sensitivity_display.get(), 100.0)))
                .font_size(10.0)
                .color(rgb(1.0, 0.65, 0.3));
            Label::new(cx, "SENSITIVITY").font_size(10.0).color(col(0.75, 0.75, 0.75, 1.0));
        })
        .width(Pixels(70.0))
        .height(Pixels(88.0))
        .alignment(Alignment::Center)
        .padding(Pixels(6.0))
        .background_color(rgb(0.1, 0.1, 0.1));
    })
    .width(Stretch(1.0))
    .height(Pixels(88.0))
    .horizontal_gap(Pixels(6.0));
}

// ─── SNAP Helpers (framework-independent, unchanged from the iced version) ──

fn snap_filename(vault_path: &str) -> String {
    let dir = std::path::Path::new(vault_path);
    let mut max_n = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let s = e.file_name().to_string_lossy().into_owned();
            if let Some(inner) = s.strip_prefix("SNAPSHOT-").and_then(|r| r.strip_suffix(".md")) {
                if let Ok(n) = inner.parse::<u32>() {
                    max_n = max_n.max(n);
                }
            }
        }
    }
    format!("SNAPSHOT-{:03}.md", max_n + 1)
}

fn snap_markdown(
    stereo: &[f32],
    mono: &[f32],
    delta: &[f32],
    relays: &[RelayData],
    corr: f32,
    pl: f32,
    pr: f32,
    sr: f32,
) -> String {
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
    let relay_section = if relays.is_empty() {
        String::new()
    } else {
        let tracks = relays
            .iter()
            .map(|r| format!("### {}\n| Hz | dB |\n|----|-----|\n{}\n", r.name, tbl(&r.spectrum)))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n## Relay Tracks\n\n{tracks}")
    };
    format!(
        "---\nplugin: lucent\ntype: snapshot\n---\n\n# Lucent Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n\
        {relay_section}",
        pl = pl, pr = pr, co = corr, st = tbl(stereo), mn = tbl(mono), dt = tbl(delta),
    )
}
