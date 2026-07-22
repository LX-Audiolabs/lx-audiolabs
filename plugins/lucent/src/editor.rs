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
//! snap blink) lives in one `Signal<Telemetry>` updated every ~33ms by
//! the `Ticker` View below (replaces the old `Message::Tick` /
//! `RedrawRequested` subscription - NOT `cx.add_timer`/`cx.start_timer`,
//! see `Ticker`'s doc comment for why), and only the passive display
//! regions (right sidebar,
//! center spectrum+relay-bar+analyzer text, SNAP button) are wrapped in
//! `Binding`s keyed to it. The Name/Vault-path `Textbox`es and the
//! Name/Vault `Textbox`es stay outside tick-driven `Binding`s so typing
//! survives across ticks. Mode toggles and the Sensitivity knob bind to
//! each param's `ParamLens::value_signal` for host automation via
//! `truce-vizia`'s `refresh_params` idle poll.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vizia::prelude::*;
use vizia::vg;

use lx_analysis::{SPECTRUM_BINS, SharedState, relay_hub};
use truce_vizia::ParamLens;

use crate::ui::{LucentUiState, RelayData};
use crate::{
    LucentParams, LucentParamsParamId, editor_ensure_consumer, read_masking, read_resonance,
};
use lx_ui::{
    Gesture, GoniometerView, KnobView, SpectrumConfig, SpectrumCurve, SpectrumView,
    StereoMeterView, fmt_db, format_knob_value, rgb as vg_rgb,
};

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

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Max relay toggle buttons in the relay bar (matches planned group-track count).
const MAX_RELAY_BAR: usize = 8;

// ─── Telemetry (tick-frequency display state) ───────────────────────────────

#[derive(Clone, PartialEq)]
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
    /// Max-hold for UI text (cleared every `DISPLAY_HOLD_MS`).
    resonance_acc_own: HashMap<usize, f32>,
    resonance_acc_relay: HashMap<usize, (f32, Vec<String>)>,
    masking_acc: HashMap<usize, (f32, Vec<String>)>,
    /// Session max-hold for SNAP export — survives display refresh so the
    /// written file matches resonances/masking seen while scanning, not one
    /// detector frame at write time. Cleared after each SNAP write.
    snap_res_own: HashMap<usize, f32>,
    snap_res_relay: HashMap<usize, (f32, Vec<String>)>,
    snap_mask: HashMap<usize, (f32, Vec<String>)>,
    display_window_start: Instant,
    /// `Arc<Mutex<_>>` so SAVE / SNAP button closures (Send+Sync) can update
    /// the same path the ticker uses when writing SNAPSHOT-*.md.
    vault_path: Arc<Mutex<Option<String>>>,
    /// Button arms this; tick writes SNAPSHOT from `snap_*` buffers.
    snap_request: Arc<AtomicBool>,
}

const DISPLAY_HOLD_MS: u128 = 500;

fn max_hold_score(map: &mut HashMap<usize, f32>, bin: usize, score: f32) {
    map.entry(bin)
        .and_modify(|s| {
            if score > *s {
                *s = score;
            }
        })
        .or_insert(score);
}

fn max_hold_named(
    map: &mut HashMap<usize, (f32, Vec<String>)>,
    bin: usize,
    score: f32,
    names: &[String],
) {
    map.entry(bin)
        .and_modify(|(s, n)| {
            if score > *s {
                *s = score;
                *n = names.to_vec();
            }
        })
        .or_insert((score, names.to_vec()));
}

#[allow(clippy::too_many_arguments)]
fn tick(
    shared: &SharedState,
    params: &LucentParams,
    lens: &ParamLens<LucentParams>,
    instance_key: usize,
    accum: &Rc<RefCell<TickAccum>>,
    telemetry: Signal<Telemetry>,
) -> bool {
    let mode = lens.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
    let mut acc = accum.borrow_mut();

    // Relay discovery is independent of analyze mode — always heartbeat while
    // the editor is open (STANDALONE still must advertise as a SHM consumer).
    editor_ensure_consumer(params, shared);

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
        max_hold_score(&mut acc.resonance_acc_own, bin, score);
        max_hold_score(&mut acc.snap_res_own, bin, score);
    }
    for (bin, score, names) in &lists.relay {
        max_hold_named(&mut acc.resonance_acc_relay, *bin, *score, names);
        max_hold_named(&mut acc.snap_res_relay, *bin, *score, names);
    }
    for (bin, db, names) in &masking_top {
        max_hold_named(&mut acc.masking_acc, *bin, *db, names);
        max_hold_named(&mut acc.snap_mask, *bin, *db, names);
    }

    let sample_rate = shared.sample_rate.load(Ordering::Relaxed).max(1.0);
    let refresh_text = acc.display_window_start.elapsed().as_millis() >= DISPLAY_HOLD_MS;
    let new_texts = if refresh_text {
        let rt = format_resonance_text(
            &acc.resonance_acc_own,
            &acc.resonance_acc_relay,
            sample_rate,
        );
        let mt = format_masking_text(
            mode,
            &acc.masking_acc,
            acc.ui.relays.is_empty(),
            sample_rate,
        );
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
        // Apply user toggles from the UI back into the accumulator before
        // smoothing. `sync_relays` preserves active state by name, so a button
        // press survives the next tick instead of being overwritten from the
        // previous accumulator state.
        for t in telemetry.get().relays.iter() {
            if let Some(r) = acc.ui.relays.iter_mut().find(|r| r.slot == t.slot) {
                r.active = t.active;
            }
        }

        let now_ms = lx_analysis::shm::now_ms();
        let slot = shared.shm_slot.load(Ordering::Acquire);
        let raw = params
            .name
            .try_read()
            .map(|n| n.clone())
            .unwrap_or_default();
        let my_name = if slot >= 0 {
            lx_analysis::shm::display_name(&raw, slot as u8)
        } else {
            raw
        };
        let feeds = relay_hub()
            .map(|hub| hub.read_active(&my_name, now_ms))
            .unwrap_or_default();
        acc.ui.sync_relays(feeds);
        shared.relay_active_mask.store(
            acc.ui.relay_active_mask(),
            Ordering::Release,
        );
    } else {
        acc.ui.clear_relays();
        shared.relay_active_mask.store(0, Ordering::Release);
    }

    let peak_l = shared.output_peak_l.load(Ordering::Relaxed);
    let peak_r = shared.output_peak_r.load(Ordering::Relaxed);
    let peak_hold_l = shared.peak_hold_l.load(Ordering::Relaxed);
    let peak_hold_r = shared.peak_hold_r.load(Ordering::Relaxed);
    let peak_hold = shared.peak_hold.load(Ordering::Relaxed);
    let phase_correlation = shared.phase_correlation.load(Ordering::Relaxed);
    let balance = shared.balance.load(Ordering::Relaxed);
    // Sync host param state → telemetry (for Resonance/Masking Bitwig page toggles)
    let resonance_active = lens.get(LucentParamsParamId::ResonanceActive) > 0.5;
    let masking_active = lens.get(LucentParamsParamId::MaskingActive) > 0.5;

    let prev = telemetry.get();
    let mut next = Telemetry {
        show_resonance: resonance_active,
        show_masking: masking_active,
        own_spectrum: acc.ui.own_spectrum.clone(),
        relays: acc.ui.relays.clone(),
        resonance_cache_own: lists.own,
        resonance_cache_relay: lists.relay,
        masking_cache: masking_cache.unwrap_or_else(|| prev.masking_cache.clone()),
        resonance_text: new_texts
            .as_ref()
            .map(|(rt, _)| rt.clone())
            .unwrap_or_else(|| prev.resonance_text.clone()),
        masking_text: new_texts
            .as_ref()
            .map(|(_, mt)| mt.clone())
            .unwrap_or_else(|| prev.masking_text.clone()),
        snap_blink: prev.snap_blink,
        peak_l,
        peak_r,
        peak_hold_l,
        peak_hold_r,
        peak_hold,
        phase_correlation,
        balance,
    };

    // SNAP: write session max-hold of resonance/masking only (no stereo/mono/delta FFT).
    if acc.snap_request.swap(false, Ordering::AcqRel) {
        next.snap_blink = 72;
        let vault = acc
            .vault_path
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .filter(|p| !p.is_empty());
        if let Some(vp) = vault {
            let sr = shared.sample_rate.load(Ordering::Relaxed);
            let sensitivity_pct = lens.get_plain(LucentParamsParamId::Sensitivity);
            let mut res_own: Vec<(usize, f32)> = acc
                .snap_res_own
                .iter()
                .map(|(&bin, &score)| (bin, score))
                .collect();
            res_own.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut res_relay: Vec<(usize, f32, Vec<String>)> = acc
                .snap_res_relay
                .iter()
                .map(|(&bin, (score, names))| (bin, *score, names.clone()))
                .collect();
            res_relay.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut mask: Vec<(usize, f32, Vec<String>)> = acc
                .snap_mask
                .iter()
                .map(|(&bin, (db, names))| (bin, *db, names.clone()))
                .collect();
            mask.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let instance_name = params
                .name
                .try_read()
                .map(|n| n.clone())
                .unwrap_or_default();
            let md = snap_markdown(
                &instance_name,
                &res_own,
                &res_relay,
                &mask,
                sr,
                sensitivity_pct,
                mode,
            );
            let fname = snap_filename(&vp);
            let _ = std::fs::write(std::path::Path::new(&vp).join(&fname), &md);
            acc.snap_res_own.clear();
            acc.snap_res_relay.clear();
            acc.snap_mask.clear();
        }
    }
    if next.snap_blink > 0 {
        next.snap_blink -= 1;
    }

    if next != prev {
        telemetry.set(next);
        true
    } else {
        false
    }
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

    let mut lines = Vec::new();
    for (bin, score) in own.iter().take(3) {
        let freq = *bin as f32 * sample_rate / fft_size;
        lines.push(format!("Own: {:.0} Hz {:.1}", freq, score));
    }
    for (bin, score, contributors) in relay.iter().take(3) {
        let freq = *bin as f32 * sample_rate / fft_size;
        if contributors.is_empty() {
            lines.push(format!("Group: {:.0} Hz {:.1}", freq, score));
        } else {
            lines.push(format!(
                "Group: {:.0} Hz {:.1} ({})",
                freq,
                score,
                contributors.join(", ")
            ));
        }
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
    let mut peaks: Vec<(usize, f32, Vec<String>)> = acc
        .iter()
        .map(|(&bin, (db, names))| (bin, *db, names.clone()))
        .collect();
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

// ─── Ticker (drives `tick()` without vizia_core's buggy timer API) ──────────

/// Zero-visual-footprint View whose only job is calling `tick()` roughly
/// every 33ms, throttled internally via `last_tick`, and keeping itself
/// redrawing forever via `cx.needs_redraw()`. See the comment at the call
/// site in `build()` for why this replaces `cx.add_timer`/`cx.start_timer`.
struct Ticker {
    shared: Arc<SharedState>,
    params: Arc<LucentParams>,
    lens: ParamLens<LucentParams>,
    instance_key: usize,
    accum: Rc<RefCell<TickAccum>>,
    telemetry: Signal<Telemetry>,
    last_tick: RefCell<Instant>,
}

impl Ticker {
    fn new(
        cx: &mut Context,
        shared: Arc<SharedState>,
        params: Arc<LucentParams>,
        lens: ParamLens<LucentParams>,
        instance_key: usize,
        accum: Rc<RefCell<TickAccum>>,
        telemetry: Signal<Telemetry>,
    ) -> Handle<'_, Self> {
        Self {
            shared,
            params,
            lens,
            instance_key,
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
        let profile = lx_ui::ticker_profile_enabled();
        let t0_total = if profile { Some(Instant::now()) } else { None };
        let t0_tick = if profile && due { Some(Instant::now()) } else { None };
        let mut telemetry_changed = false;
        if due {
            telemetry_changed = tick(
                &self.shared,
                &self.params,
                &self.lens,
                self.instance_key,
                &self.accum,
                self.telemetry,
            );
        }
        let tick_us = t0_tick.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);
        // Keep the render loop alive so the layer-cached views repaint their
        // dynamic overlays every frame. The telemetry Signal is still only set
        // when values actually change.
        let _ = telemetry_changed;
        cx.needs_redraw();
        let total_us = t0_total.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);
        if profile {
            lx_ui::report_ticker(tick_us, total_us);
        }
    }
}

// ─── UI ──────────────────────────────────────────────────────────────────────

pub fn build(
    cx: &mut Context,
    lens: ParamLens<LucentParams>,
    shared: Arc<SharedState>,
    params: Arc<LucentParams>,
) {
    lx_ui::load_theme(cx);

    let instance_key = Arc::as_ptr(&params) as usize;
    let config = lx_analysis::load_config("Lucent");

    let mut initial_name = "Lucent".to_string();
    if let Ok(name) = params.name.read()
        && !name.is_empty()
    {
        initial_name = name.clone();
    }

    let telemetry = Signal::new(Telemetry {
        own_spectrum: Vec::new(),
        relays: Vec::new(),
        resonance_cache_own: Vec::new(),
        resonance_cache_relay: Vec::new(),
        masking_cache: Vec::new(),
        resonance_text: "No resonances detected".to_string(),
        masking_text: "No masking detected".to_string(),
        show_resonance: true,
        show_masking: true,
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
    let vault_path = Arc::new(Mutex::new(config.vault_path));
    let snap_request = Arc::new(AtomicBool::new(false));
    let accum = Rc::new(RefCell::new(TickAccum {
        ui: LucentUiState::new(),
        resonance_acc_own: HashMap::new(),
        resonance_acc_relay: HashMap::new(),
        masking_acc: HashMap::new(),
        snap_res_own: HashMap::new(),
        snap_res_relay: HashMap::new(),
        snap_mask: HashMap::new(),
        display_window_start: Instant::now(),
        vault_path: vault_path.clone(),
        snap_request: snap_request.clone(),
    }));

    // Not using cx.add_timer()/cx.start_timer() here: vizia_core 0.4.0's
    // Context::modify_timer has a real infinite-loop bug (peeks the
    // running_timers BinaryHeap without popping on an id mismatch, spins
    // forever if the target timer isn't at the heap's top - see CLAP-vault
    // features/2026-07-04-truce-2.0-upgrade-plan.md HANDOFF, found via
    // WinDbg). truce-vizia's own editor.rs starts a second, internal
    // meter-refresh timer right after this setup closure returns, so any
    // timer we start here collides with theirs in that heap and hangs the
    // whole host on open. Ticker below drives the same 33ms cadence via
    // draw()-triggered needs_redraw() instead - the pattern our earlier
    // prototypes (prototypes/lucent-vizia, prototypes/truce-vizia-spike)
    // already used, no add_timer/start_timer call at all.
    Ticker::new(
        cx,
        shared.clone(),
        params.clone(),
        lens.clone(),
        instance_key,
        accum.clone(),
        telemetry,
    )
    .width(Pixels(1.0))
    .height(Pixels(1.0));

    // ── HEADER ──────────────────────────────────────────────────────────────
    let shared_header = shared.clone();
    let lens_header = lens.clone();
    HStack::new(cx, move |cx| {
        let shared = shared_header;
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
                Label::new(cx, "LUCENT")
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

        let shared_for_name = shared.clone();
        Textbox::new(cx, name_signal)
            .on_edit(move |_cx, text| {
                if let Ok(mut n) = params.name.write() {
                    *n = text.clone();
                }
                if let Ok(mut bg) = params.name_bg.write() {
                    *bg = text.clone();
                }
                name_signal.set(text.clone());
                let slot = shared_for_name.shm_slot.load(Ordering::Acquire);
                if slot >= 0 {
                    let my_name = lx_analysis::shm::display_name(&text, slot as u8);
                    if let Some(hub) = relay_hub() {
                        hub.write_consumer_name(
                            slot as u8,
                            &my_name,
                            lx_analysis::shm::now_ms(),
                        );
                    }
                }
            })
            .width(Pixels(170.0));

        Element::new(cx).width(Stretch(1.0));

        let shared_for_reset = shared.clone();
        HStack::new(cx, move |cx| {
            let mode_signal = lens.value_signal(LucentParamsParamId::AnalyzeMode);
            let lens_for_mode = lens.clone();
            Binding::new(cx, mode_signal, move |cx| {
                let mode = lens_for_mode.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
                let label = match mode {
                    0 => "STANDALONE",
                    2 => "RELAY",
                    _ => "HYBRID",
                };
                let lens_press = lens_for_mode.clone();
                lx_ui::toggle_button(cx, label, true, move |_cx| {
                    let current = lens_press.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
                    let next: i64 = match current {
                        1 => 0,
                        0 => 2,
                        _ => 1,
                    };
                    lens_press.automate(LucentParamsParamId::AnalyzeMode, next as f64 / 2.0);
                    lens_press
                        .value_signal(LucentParamsParamId::AnalyzeMode)
                        .set(next as f32 / 2.0);
                })
                .width(Pixels(110.0));
            });

            lx_ui::danger_button(cx, "RESET", move |_cx| {
                shared_for_reset.reset_peak.store(true, Ordering::Relaxed);
                shared_for_reset.peak_hold.store(-100.0, Ordering::Relaxed);
                shared_for_reset
                    .peak_hold_l
                    .store(-100.0, Ordering::Relaxed);
                shared_for_reset
                    .peak_hold_r
                    .store(-100.0, Ordering::Relaxed);
            })
            .height(Pixels(lx_ui::BUTTON_HEIGHT));
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
        let vault_for_snap = vault_path.clone();
        let snap_req_for_btn = snap_request.clone();
        let setup_for_snap = setup_visible;
        VStack::new(cx, move |cx| {
            // Built once, not inside a `Binding` on `telemetry` - `tick()`
            // calls `telemetry.update(...)` unconditionally every ~33ms
            // (Vizia signals have no equality check, see
            // vizia_reactive::state::State::update_value_local), so a
            // `Binding` here would tear down and rebuild this Button every
            // tick. A real click's MouseDown/MouseUp lands on two different
            // rebuilt entity instances more often than not, and
            // `WindowEvent::Press` only fires when both match the same
            // `cx.current` - the click gets silently dropped. `Memo`
            // instead subscribes to `telemetry` and updates the label/color
            // properties on this same, persistent entity in place.
            Button::new(cx, move |cx| {
                Label::new(
                    cx,
                    Memo::new(move |_| {
                        if telemetry.get().snap_blink > 0 {
                            "ANALYZING..."
                        } else {
                            "SNAP"
                        }
                    }),
                )
                .font_size(12.0)
                .color(Memo::new(move |_| {
                    if telemetry.get().snap_blink > 0 {
                        rgb(1.0, 0.85, 0.3)
                    } else {
                        rgb(1.0, 0.55, 0.1)
                    }
                }))
            })
            .on_press(move |_cx| {
                let no_vault = vault_for_snap
                    .lock()
                    .ok()
                    .map(|g| g.as_ref().is_none_or(|v| v.is_empty()))
                    .unwrap_or(true);
                if no_vault {
                    setup_for_snap.set(true);
                } else {
                    // Session max-hold of resonance/masking — no SnapFFT phases.
                    snap_req_for_btn.store(true, Ordering::Release);
                }
            })
            .width(Stretch(1.0))
            .height(Pixels(34.0))
            .background_color(Memo::new(move |_| {
                if telemetry.get().snap_blink > 0 {
                    col(0.55, 0.38, 0.05, 1.0)
                } else {
                    col(0.18, 0.18, 0.18, 1.0)
                }
            }));

            Button::new(cx, |cx| Label::new(cx, "VAULT SETUP").font_size(12.0))
                .on_press(move |_cx| {
                    let now = !setup_visible.get();
                    setup_visible.set(now);
                })
                .width(Stretch(1.0))
                .height(Pixels(34.0))
                .class("lx-btn");
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
                    build_setup_form(cx, vault_path_input, setup_visible, vault_path.clone());
                } else {
                    // `build_main_panel` reads AnalyzeMode once at build time
                    // (masking availability, spectrum source) - rebuild it
                    // whenever the mode switches, or those go stale until the
                    // editor is reopened.
                    let mode_signal = lens.value_signal(LucentParamsParamId::AnalyzeMode);
                    let lens = lens.clone();
                    Binding::new(cx, mode_signal, move |cx| {
                        build_main_panel(cx, telemetry, lens.clone());
                    });
                }
            });
        })
        .width(Stretch(1.0))
        .height(Stretch(1.0))
        .background_color(rgb(0.08, 0.08, 0.08));

        // ── RIGHT SIDEBAR ─────────────────────────────────────────────────
        let shared_for_gonio = shared.clone();
        VStack::new(cx, move |cx| {
            Label::new(cx, "OUTPUT LEVEL")
                .font_size(12.0)
                .color(col(0.75, 0.75, 0.75, 1.0));

            Binding::new(cx, telemetry, move |cx| {
                let t = telemetry.get();
                VStack::new(cx, move |cx| {
                    StereoMeterView::new(
                        cx,
                        t.peak_l,
                        t.peak_r,
                        t.peak_hold_l,
                        t.peak_hold_r,
                        t.balance,
                    )
                    .width(Stretch(1.0))
                    .height(Pixels(255.0));

                    HStack::new(cx, move |cx| {
                        Label::new(cx, fmt_db(t.peak_hold_l))
                            .font_size(11.0)
                            .color(rgb(1.0, 0.45, 0.1));
                        Element::new(cx).width(Stretch(1.0));
                        Label::new(cx, "dB")
                            .font_size(10.0)
                            .color(col(0.8, 0.8, 0.8, 1.0));
                        Element::new(cx).width(Stretch(1.0));
                        Label::new(cx, fmt_db(t.peak_hold_r))
                            .font_size(11.0)
                            .color(rgb(1.0, 0.45, 0.1));
                    })
                    .width(Stretch(1.0))
                    .alignment(Alignment::Center);
                })
                .height(Auto)
                .vertical_gap(Pixels(4.0));
            });

            // Spacer pushes the goniometer block to the bottom of the sidebar.
            Element::new(cx).height(Stretch(1.0));

            Label::new(cx, "GONIOMETER")
                .font_size(10.0)
                .color(col(0.6, 0.6, 0.6, 1.0));

            Binding::new(cx, telemetry, move |cx| {
                let t = telemetry.get();
                GoniometerView::new(
                    cx,
                    shared_for_gonio.scope_samples.clone(),
                    shared_for_gonio.scope_write_pos.load(Ordering::Acquire),
                    t.phase_correlation,
                )
                .width(Stretch(1.0))
                .height(Pixels(155.0));
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

fn build_setup_form(
    cx: &mut Context,
    vault_path_input: Signal<String>,
    setup_visible: Signal<bool>,
    vault_path: Arc<Mutex<Option<String>>>,
) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "LX AUDIOLABS - SETUP")
            .font_size(18.0)
            .color(Color::white());
        Label::new(cx, "Configure your Vault path for Lucent:")
            .font_size(12.0)
            .color(Color::white());
        Textbox::new(cx, vault_path_input)
            .placeholder("Enter Vault absolute path...")
            .on_edit(move |_cx, text| vault_path_input.set(text))
            .width(Stretch(1.0));
        HStack::new(cx, move |cx| {
            Button::new(cx, |cx| Label::new(cx, "SAVE"))
                .on_press(move |_cx| {
                    let path = vault_path_input.get();
                    let new_path = if path.trim().is_empty() {
                        None
                    } else {
                        Some(path.trim().to_string())
                    };
                    if let Ok(mut g) = vault_path.lock() {
                        *g = new_path.clone();
                    }
                    let cfg = lx_analysis::PluginConfig {
                        vault_path: new_path,
                        ..Default::default()
                    };
                    let _ = lx_analysis::save_config("Lucent", &cfg);
                    setup_visible.set(false);
                })
                .class("lx-btn");
            Button::new(cx, |cx| Label::new(cx, "CANCEL"))
                .on_press(move |_cx| setup_visible.set(false))
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

fn build_main_panel(
    cx: &mut Context,
    telemetry: Signal<Telemetry>,
    lens: ParamLens<LucentParams>,
) {
    let mode = lens.get_plain(LucentParamsParamId::AnalyzeMode) as i64;

    // Relay bar — built once outside the tick-driven telemetry Binding.
    // Each button slot subscribes to its own Memo that only changes when the
    // relay name or active toggle changes, so MouseDown/MouseUp land on the
    // same entity and clicks are not dropped.
    if mode != 0 {
        HStack::new(cx, move |cx| {
            Label::new(cx, "RELAYS")
                .font_size(10.0)
                .color(rgb(1.0, 0.55, 0.15));

            let placeholder_visible = Memo::new(move |_| telemetry.get().relays.is_empty());
            Binding::new(cx, placeholder_visible, move |cx| {
                if placeholder_visible.get() {
                    Label::new(cx, "— send a relay from another LX plugin —")
                        .font_size(10.0)
                        .color(col(0.4, 0.4, 0.4, 1.0));
                }
            });

            for idx in 0..MAX_RELAY_BAR {
                let slot_memo = Memo::new(move |_| {
                    telemetry.get().relays.get(idx).map(|r| r.name.clone())
                });
                let active_memo = Memo::new(move |_| {
                    telemetry.get().relays.get(idx).map(|r| r.active).unwrap_or(false)
                });
                Binding::new(cx, slot_memo, move |cx| {
                    if let Some(name) = slot_memo.get() {
                        Button::new(cx, move |cx| Label::new(cx, name.clone()).font_size(9.0))
                            .on_press(move |_cx| {
                                telemetry.update(|t| {
                                    if let Some(r) = t.relays.get_mut(idx) {
                                        r.active = !r.active;
                                    }
                                });
                            })
                            .height(Pixels(lx_ui::BUTTON_HEIGHT_SMALL))
                            .class("lx-btn")
                            .toggle_class("active", active_memo);
                    }
                });
            }
        })
        .width(Stretch(1.0))
        .height(Pixels(40.0))
        .padding(Pixels(8.0))
        .horizontal_gap(Pixels(6.0))
        .alignment(Alignment::Left)
        .background_color(rgb(0.06, 0.06, 0.06));
    }

    // Spectrum canvas — passive display, safe to rebuild every tick.
    Binding::new(cx, telemetry, move |cx| {
        let t = telemetry.get();

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
                vg_rgb(1.0, 0.6, 0.2),
                vg_rgb(0.8, 0.3, 0.3),
                vg_rgb(0.3, 0.8, 0.5),
                vg_rgb(0.4, 0.6, 1.0),
                vg_rgb(0.9, 0.7, 0.3),
                vg_rgb(0.7, 0.4, 0.8),
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
            v.extend(
                t.resonance_cache_relay
                    .iter()
                    .map(|(bin, score, _)| (*bin, *score)),
            );
            v
        } else {
            Vec::new()
        };
        let masking = if t.show_masking && (!t.relays.is_empty() || mode == 2) {
            t.masking_cache.clone()
        } else {
            Vec::new()
        };

        SpectrumView::new(
            cx,
            SpectrumView {
                curves,
                config: SpectrumConfig::default(),
                resonance_peaks,
                masking,
                eq_curve: None,
                hovered_freq: std::cell::Cell::new(None),
            },
        )
        .width(Stretch(1.0))
        .height(Stretch(1.0));
    });

    // Analyzer row: resonance/masking text panels + sensitivity knob.
    // Built once, not inside a `Binding` on `telemetry` - `tick()` calls
    // `telemetry.update(...)` unconditionally every ~33ms (no equality
    // check in vizia_reactive::state::State::update_value_local), so a
    // `Binding` here tore down and rebuilt the ON/OFF buttons every tick.
    // A real click's MouseDown/MouseUp then landed on two different
    // rebuilt entity instances more often than not, and
    // `WindowEvent::Press` only fires when both match the same
    // `cx.current` - the click was silently dropped. `Memo` instead
    // subscribes to `telemetry` and updates text/color on this same,
    // persistent Button entity in place.
    HStack::new(cx, move |cx| {
        let lens_resonance = lens.clone();
        let lens_masking = lens.clone();
        let lens_sensitivity = lens;
        HStack::new(cx, move |cx| {
            VStack::new(cx, move |cx| {
                Label::new(cx, "RESONANCE")
                    .font_size(10.0)
                    .color(rgb(1.0, 0.55, 0.15));
                Label::new(
                    cx,
                    Memo::new(move |_| telemetry.get().resonance_text.clone()),
                )
                .font_size(10.0)
                .color(col(0.8, 0.8, 0.8, 1.0));
            })
            .width(Stretch(1.0));
            {
                let sig = lens_resonance.value_signal(LucentParamsParamId::ResonanceActive);
                let lens_r = lens_resonance.clone();
                Binding::new(cx, sig, move |cx| {
                    let active = lens_r.get(LucentParamsParamId::ResonanceActive) > 0.5;
                    let lens_r = lens_r.clone();
                    lx_ui::toggle_button_small(
                        cx,
                        if active { "ON" } else { "OFF" },
                        active,
                        move |_cx| {
                            let now = lens_r.get(LucentParamsParamId::ResonanceActive) <= 0.5;
                            let norm = if now { 1.0 } else { 0.0 };
                            lens_r.automate(LucentParamsParamId::ResonanceActive, norm);
                            sig.set(norm as f32);
                        },
                    );
                });
            }
        })
        .width(Stretch(1.0))
        .height(Pixels(88.0))
        .padding(Pixels(6.0))
        .background_color(rgb(0.1, 0.1, 0.1));

        HStack::new(cx, move |cx| {
            VStack::new(cx, move |cx| {
                Label::new(cx, "MASKING")
                    .font_size(10.0)
                    .color(rgb(0.95, 0.22, 0.18));
                Label::new(cx, Memo::new(move |_| telemetry.get().masking_text.clone()))
                    .font_size(10.0)
                    .color(col(0.8, 0.8, 0.8, 1.0));
            })
            .width(Stretch(1.0));
            if mode == 0 {
                Label::new(cx, "OFF").color(col(0.35, 0.35, 0.35, 1.0));
            } else {
                let sig = lens_masking.value_signal(LucentParamsParamId::MaskingActive);
                let lens_m = lens_masking.clone();
                Binding::new(cx, sig, move |cx| {
                    let active = lens_m.get(LucentParamsParamId::MaskingActive) > 0.5;
                    let lens_m = lens_m.clone();
                    lx_ui::toggle_button_small_danger(
                        cx,
                        if active { "ON" } else { "OFF" },
                        active,
                        move |_cx| {
                            let now = lens_m.get(LucentParamsParamId::MaskingActive) <= 0.5;
                            let norm = if now { 1.0 } else { 0.0 };
                            lens_m.automate(LucentParamsParamId::MaskingActive, norm);
                            sig.set(norm as f32);
                        },
                    );
                });
            }
        })
        .width(Stretch(1.0))
        .height(Pixels(88.0))
        .padding(Pixels(6.0))
        .background_color(rgb(0.1, 0.1, 0.1));

        Binding::new(
            cx,
            lens_sensitivity.value_signal(LucentParamsParamId::Sensitivity),
            {
                let lens_sensitivity = lens_sensitivity.clone();
                move |cx| {
                    let sens = lens_sensitivity.get_plain(LucentParamsParamId::Sensitivity);
                    let sens_display = Signal::new(sens);
                    let lens_knob = lens_sensitivity.clone();
                    VStack::new(cx, |cx| {
                        KnobView::new(
                            cx,
                            (sens / 100.0).clamp(0.0, 1.0),
                            0.5,
                            0.0,
                            100.0,
                            false,
                            move |_cx, g| match g {
                                Gesture::Start => {
                                    lens_knob.begin_edit(LucentParamsParamId::Sensitivity)
                                }
                                Gesture::Change(v) => {
                                    let norm = (v / 100.0).clamp(0.0, 1.0);
                                    lens_knob.set(LucentParamsParamId::Sensitivity, norm as f64);
                                    sens_display.set(v);
                                }
                                Gesture::End => {
                                    lens_knob.end_edit(LucentParamsParamId::Sensitivity)
                                }
                            },
                        )
                        .width(Pixels(40.0))
                        .height(Pixels(40.0));

                        Label::new(
                            cx,
                            Memo::new(move |_| format_knob_value(sens_display.get(), 100.0)),
                        )
                        .font_size(10.0)
                        .color(rgb(1.0, 0.65, 0.3));
                        Label::new(cx, "SENSITIVITY")
                            .font_size(10.0)
                            .color(col(0.75, 0.75, 0.75, 1.0));
                    })
                    .width(Pixels(70.0))
                    .height(Pixels(88.0))
                    .alignment(Alignment::Center)
                    .padding(Pixels(6.0))
                    .background_color(rgb(0.1, 0.1, 0.1));
                }
            },
        );
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
    instance_name: &str,
    res_own: &[(usize, f32)],
    res_relay: &[(usize, f32, Vec<String>)],
    masking: &[(usize, f32, Vec<String>)],
    sr: f32,
    sensitivity_pct: f32,
    mode: i64,
) -> String {
    let fft_sz = 2048.0;
    let bin_hz = sr / fft_sz;
    let mode_name = match mode {
        0 => "standalone",
        2 => "relay",
        _ => "hybrid",
    };
    // Name field value — distinguishes multiple Lucent instances. Empty → fallback.
    let name = if instance_name.trim().is_empty() {
        "Lucent"
    } else {
        instance_name.trim()
    };
    // YAML-safe quote if name has special chars
    let name_yaml = if name.chars().any(|c| matches!(c, ':' | '#' | '"' | '\'' | '\n'))
        || name.contains(": ")
    {
        format!("\"{}\"", name.replace('"', "\\\""))
    } else {
        name.to_string()
    };

    // Session max-hold lists (Sensitivity-gated) — full set, not UI top-N.
    let res_rows = {
        let mut rows = Vec::new();
        for &(bin, score) in res_own {
            let hz = bin as f32 * bin_hz;
            rows.push(format!("| Own ({name}) | {hz:.0} | {score:.2} | |"));
        }
        for (bin, score, names) in res_relay {
            let hz = *bin as f32 * bin_hz;
            rows.push(format!(
                "| Group | {hz:.0} | {score:.2} | {} |",
                names.join(", ")
            ));
        }
        if rows.is_empty() {
            "_No resonances detected at current sensitivity._".to_string()
        } else {
            format!(
                "| Source | Hz | Score | Contributors |\n|--------|-----|-------|--------------|\n{}",
                rows.join("\n")
            )
        }
    };
    let mask_rows = if mode == 0 {
        "_Standalone — no masking._".to_string()
    } else if masking.is_empty() {
        "_No masking areas detected at current sensitivity._".to_string()
    } else {
        let rows = masking
            .iter()
            .map(|(bin, db, names)| {
                let hz = *bin as f32 * bin_hz;
                format!("| {hz:.0} | {db:.1} | {} |", names.join(" / "))
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("| Hz | Amount (dB) | Maskers |\n|----|-------------|---------|\n{rows}")
    };

    format!(
        "---\n\
         plugin: lucent\n\
         name: {name_yaml}\n\
         type: snapshot\n\
         sample_rate: {sr:.0}\n\
         analyze_mode: {mode_name}\n\
         sensitivity_pct: {sensitivity_pct:.0}\n\
         ---\n\n\
         # Lucent Snapshot — {name}\n\n\
         > Resonance score = detector strength (Sensitivity-gated). \
         Masking dB = collision level after ERB smooth + persistence gate. \
         Lists are **session max-hold** over analysis until SNAP (not one frame, not UI top-N). \
         `name` = instance label from the Name field (multi-Lucent).\n\n\
         ## Resonance\n{res_rows}\n\n\
         ## Masking\n{mask_rows}\n"
    )
}
