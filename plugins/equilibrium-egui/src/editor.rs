//! Egui prototype UI for Equilibrium (framework-compat evaluation against
//! the shipping truce-vizia/shared-ui port in `plugins/equilibrium`).
//!
//! Deliberately NOT feature-complete - this exists to gauge how egui feels
//! against a real DSP/param surface, not to ship. Left out entirely:
//! Target Profiles sidebar (presets/vault), LISTEN/APPLY/RESET ANALYSIS,
//! SNAP. Everything else (5-band Gain/Width/Pan/Solo, output gain, mono
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

use std::sync::{atomic::Ordering, Arc};

use egui::{Color32, FontId, RichText, Sense, Stroke};
use truce_core::editor::{PluginContext, PluginContextReadF32};
use truce_egui::theme::{HEADER_BG, HEADER_TEXT};
use truce_egui::widgets::param_knob;

use shared_analysis::SharedState;
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

pub fn build(ui: &mut egui::Ui, state: &PluginContext<EquilibriumParams>) {
    // Meters/telemetry live in atomics written by process() every block -
    // keep the editor repainting so they animate instead of updating only
    // on user interaction.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));

    let params = state.params().clone();
    let shared = params.shared.clone();

    egui::Panel::top("header")
        .exact_size(50.0)
        .frame(egui::Frame::NONE.fill(HEADER_BG))
        .show_inside(ui, |ui| {
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
                    lx_toggle(ui, state, K::BypassActive, "BYPASS");
                    lx_toggle(ui, state, K::DeltaActive, "DELTA");
                    lx_toggle(ui, state, K::MonoActive, "MONO");
                });
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::central_panel(ui.style()).inner_margin(10.0))
        .show_inside(ui, |ui| {
            spectrum_view(ui, &shared);

            // ponytail: fixed-width columns can still run wider than
            // WINDOW_W - a horizontal ScrollArea is the safety net so
            // nothing is ever unreachable, instead of hand-tuning exact
            // widget widths for a prototype.
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(16.0, 0.0);
                    for b in 0..5 {
                        ui.vertical(|ui| {
                            ui.set_width(150.0);
                            ui.label(RichText::new(format!("{} ({})", BAND_NAMES[b], BAND_HZ[b])).size(11.0).color(AMBER));

                            ui.label(RichText::new("Gain").size(10.0).color(Color32::from_gray(190)));
                            let gain = lx_hslider(ui, state, GAIN_IDS[b], -12.0, 12.0, 0.0);
                            ui.label(RichText::new(format!("{gain:.1} dB")).size(10.0).color(Color32::from_gray(200)));

                            ui.label(RichText::new("Width").size(10.0).color(Color32::from_gray(190)));
                            let width = lx_hslider(ui, state, WIDTH_IDS[b], 0.0, 150.0, 100.0);
                            ui.label(RichText::new(format!("{width:.0}%")).size(10.0).color(Color32::from_gray(200)));

                            ui.label(RichText::new("Pan").size(10.0).color(Color32::from_gray(190)));
                            let pan = lx_hslider(ui, state, PAN_IDS[b], -1.0, 1.0, 0.0);
                            ui.label(RichText::new(format_pan(pan)).size(10.0).color(Color32::from_gray(200)));

                            ui.add_space(4.0);
                            lx_toggle(ui, state, SOLO_IDS[b], "SOLO");
                        });
                    }

                    ui.separator();

                    ui.vertical(|ui| {
                        ui.set_width(130.0);
                        ui.label(RichText::new("OUTPUT").size(11.0).color(AMBER));
                        param_knob(ui, state, K::OutputGain, "Out Gain");
                        ui.add_space(6.0);
                        peak_meters(ui, &shared);

                        ui.add_space(10.0);
                        ui.label(RichText::new("MONO FLOOR").size(11.0).color(AMBER));
                        param_knob(ui, state, K::MonoFloor, "Floor");

                        ui.add_space(10.0);
                        lx_toggle(ui, state, K::PreMasterActive, "PRE-MASTER");
                        let pre_target = lx_hslider(ui, state, K::PreMasterTargetDb, -6.0, -3.0, -3.0);
                        ui.label(RichText::new(format!("Target: {pre_target:.1} dB")).size(10.0).color(Color32::from_gray(200)));

                        ui.add_space(10.0);
                        let measuring = shared.auto_loud_measuring.load(Ordering::Acquire);
                        if lx_button(ui, if measuring { "MEASURING..." } else { "AUTO LOUD" }, false).clicked() {
                            shared.auto_loud_trigger.store(true, Ordering::Release);
                        }

                        ui.add_space(10.0);
                        ui.label(RichText::new("GONIOMETER").size(10.0).color(Color32::from_gray(150)));
                        goniometer_view(ui, &shared);
                    });
                });
            });

            ui.add_space(10.0);
            if lx_button(ui, "RESET", true).clicked() {
                reset_all(state);
            }
        });
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
fn lx_button(ui: &mut egui::Ui, label: &str, danger: bool) -> egui::Response {
    let text_color = if danger { DANGER_TEXT } else { Color32::WHITE };
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::monospace(12.0), text_color);
    let size = galley.size() + egui::vec2(20.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let idle = if danger { DANGER_BG } else { IDLE_BG };
        let bg = if response.hovered() { idle.gamma_multiply(1.4) } else { idle };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, bg);
        painter.galley(rect.center() - galley.size() * 0.5, galley, text_color);
    }
    response
}

// ─── Spectrum / meters / goniometer - custom egui::Painter views ───────────

/// Custom-painted 5-band spectrum: current band level vs. tilted target
/// line, read straight off `SharedState` atomics every frame. Ports
/// `plugins/equilibrium/src/vizia_canvas.rs`'s `EqSpectrumView`, but via
/// `egui::Painter` instead of vizia's `vg::Canvas`.
fn spectrum_view(ui: &mut egui::Ui, shared: &Arc<SharedState>) {
    let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 180.0), egui::Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 2.0, Color32::from_rgb(20, 20, 20));

    let col_w = rect.width() / 5.0;
    let db_to_y = |db: f32| {
        let t = ((db + 24.0) / 30.0).clamp(0.0, 1.0); // -24..+6 dB mapped to bottom..top
        rect.bottom() - t * rect.height()
    };

    for b in 0..5 {
        let x0 = rect.left() + col_w * b as f32;
        let x_mid = x0 + col_w * 0.5;

        let level = shared.band_levels[b].load(Ordering::Acquire);
        let target = shared.target_levels[b].load(Ordering::Acquire) + TILT[b];

        let bar_top = db_to_y(level);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0 + 6.0, bar_top), egui::pos2(x0 + col_w - 6.0, rect.bottom())),
            1.0,
            AMBER.gamma_multiply(0.7),
        );

        let ty = db_to_y(target);
        painter.line_segment([egui::pos2(x0 + 4.0, ty), egui::pos2(x0 + col_w - 4.0, ty)], Stroke::new(1.5, Color32::from_rgb(255, 210, 120)));

        painter.text(egui::pos2(x_mid, rect.top() + 4.0), egui::Align2::CENTER_TOP, format!("{level:.1} dB"), FontId::monospace(10.0), Color32::WHITE);
    }
}

/// L/R peak bars, ported from `shared_ui::StereoMeterView` the same way as
/// the spectrum above - direct `SharedState` atomics, `egui::Painter`.
fn peak_meters(ui: &mut egui::Ui, shared: &Arc<SharedState>) {
    let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 60.0), egui::Sense::hover());
    let rect = resp.rect;
    let bar_w = (rect.width() - 6.0) / 2.0;

    let db_to_t = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);

    for (i, (peak, hold)) in [
        (shared.output_peak_l.load(Ordering::Acquire), shared.peak_hold_l.load(Ordering::Acquire)),
        (shared.output_peak_r.load(Ordering::Acquire), shared.peak_hold_r.load(Ordering::Acquire)),
    ]
    .into_iter()
    .enumerate()
    {
        let x0 = rect.left() + i as f32 * (bar_w + 6.0);
        let full = egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x0 + bar_w, rect.bottom()));
        painter.rect_filled(full, 1.0, Color32::from_rgb(15, 15, 15));

        let t = db_to_t(peak);
        let y = rect.bottom() - t * rect.height();
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x0 + bar_w, rect.bottom())), 1.0, AMBER);

        let hy = rect.bottom() - db_to_t(hold) * rect.height();
        painter.line_segment([egui::pos2(x0, hy), egui::pos2(x0 + bar_w, hy)], Stroke::new(1.0, Color32::WHITE));
    }
}

/// Lissajous vectorscope, ported from `shared_ui::canvas::GoniometerView`.
/// Same 3-alpha-band fade-trail (oldest/middle/newest third of the ring
/// buffer) and grid/correlation-dot layout as the Vizia original.
///
/// ponytail: draws each sample as an individual `circle_filled` (up to
/// 1024/frame - capped lower than Vizia's 2048 since Skia's batched
/// `draw_points` primitive has no direct egui equivalent; per-shape circles
/// are the straightforward port). Raise the cap or switch to a `Mesh` of
/// unindexed triangles if this turns out too slow on real hardware.
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
            let draw_count = n.min(1024);
            let wp = shared.scope_write_pos.load(Ordering::Acquire) % n;
            let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
            let third = draw_count / 3;

            for group in 0..3u8 {
                let alpha = match group {
                    0 => 30,
                    1 => 75,
                    _ => 184,
                };
                let dot_color = Color32::from_rgba_unmultiplied(26, 230, 128, alpha);
                let start = group as usize * third;
                let end = if group == 2 { draw_count } else { (group as usize + 1) * third };

                for k in start..end {
                    let age = draw_count - 1 - k;
                    let idx = (wp + n - age - 1) % n;
                    let [l, r] = samples[idx];
                    let m = (l + r) * inv_sqrt2;
                    let s = (l - r) * inv_sqrt2;
                    let sx = cx - s * scale;
                    let sy = cy - m * scale;
                    if sx >= rect.left() && sx <= rect.right() && sy >= rect.top() && sy <= rect.bottom() {
                        painter.circle_filled(egui::pos2(sx, sy), 0.9, dot_color);
                    }
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

fn reset_all(state: &PluginContext<EquilibriumParams>) {
    for (id, val) in [
        (K::LowGain, 0.0f64), (K::BassGain, 0.0), (K::MidGain, 0.0), (K::HighMidGain, 0.0), (K::HighGain, 0.0),
        (K::LowWidth, 100.0), (K::BassWidth, 100.0), (K::MidWidth, 100.0), (K::HighMidWidth, 100.0), (K::HighWidth, 100.0),
        (K::LowPan, 0.0), (K::BassPan, 0.0), (K::MidPan, 0.0), (K::HighMidPan, 0.0), (K::HighPan, 0.0),
        (K::OutputGain, 0.0), (K::MonoFloor, 0.0), (K::PreMasterTargetDb, -3.0),
    ] {
        state.automate(id, param_norm(id, val));
    }
    for id in [K::MonoActive, K::DeltaActive, K::BypassActive, K::PreMasterActive, K::ListenActive].into_iter().chain(SOLO_IDS) {
        state.automate(id, 0.0);
    }
}
