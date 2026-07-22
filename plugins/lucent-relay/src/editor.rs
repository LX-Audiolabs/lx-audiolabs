//! Vizia port of the Iced editor. Lucent-Relay has no custom canvas,
//! spectrum, or knobs — pure form widgets: name Textbox, target PickList,
//! connection status Label. Uses same Ticker pattern as Lucent's editor.rs
//! (NOT cx.add_timer — known vizia_core 0.4.0 infinite-loop bug).
//!
//! ponytail: Editor reads/writes params.name / params.target directly via
//! Arc<LucentRelayParams>. No RelayHandle registry needed.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use vizia::prelude::*;
use vizia::vg;

use crate::{editor_publish_heartbeat, sync_live, LucentRelayParams};
use lx_analysis::relay_hub;
use lx_ui::{report_ticker, ticker_profile_enabled};

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

// ─── Telemetry snapshot (updated by Ticker) ────────────────────────────────

#[derive(Clone, PartialEq)]
struct RelayTelemetry {
    lucent_list: Vec<String>,
    connected: bool,
    last_connected_ms: Option<u64>,
    now_ms: u64,
}

// ─── Ticker — polls relay_hub every ~100ms ─────────────────────────────────

struct Ticker {
    params: Arc<LucentRelayParams>,
    telemetry: Signal<RelayTelemetry>,
    last_tick: RefCell<Instant>,
    selected_target: Signal<String>,
    target_options: Signal<Vec<String>>,
    selected_index: Signal<usize>,
}

impl Ticker {
    fn new(
        cx: &mut Context,
        params: Arc<LucentRelayParams>,
        telemetry: Signal<RelayTelemetry>,
        selected_target: Signal<String>,
        target_options: Signal<Vec<String>>,
        selected_index: Signal<usize>,
    ) -> Handle<'_, Self> {
        Self {
            params,
            telemetry,
            last_tick: RefCell::new(Instant::now()),
            selected_target,
            target_options,
            selected_index,
        }
        .build(cx, |_| {})
    }
}

const TICK_INTERVAL: Duration = Duration::from_millis(100);

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
        let profile = ticker_profile_enabled();
        let t0_total = if profile { Some(Instant::now()) } else { None };
        let t0_tick = if profile && due { Some(Instant::now()) } else { None };
        let mut telemetry_changed = false;
        if due {
            editor_publish_heartbeat(&self.params);
            let now_ms = lx_analysis::shm::now_ms();
            let lucent_list = relay_hub()
                .map(|hub| hub.read_consumers(now_ms))
                .unwrap_or_default();
            let current_target = self.params.target.read().map(|s| s.clone()).unwrap_or_default();
            let connected = relay_hub()
                .map(|hub| {
                    if current_target.is_empty() {
                        !hub.read_consumers(now_ms).is_empty()
                    } else {
                        hub.consumer_exists(&current_target, now_ms)
                    }
                })
                .unwrap_or(false);

            // Keep selected target if still valid. If nothing is selected and
            // exactly one consumer exists, auto-target it so the user doesn't
            // have to open the dropdown first.
            if current_target.is_empty() && lucent_list.len() == 1 {
                let auto = lucent_list[0].clone();
                self.selected_target.set(auto.clone());
                if let Ok(mut t) = self.params.target.write() {
                    *t = auto;
                }
                sync_live(&self.params);
                telemetry_changed = true;
            } else if !current_target.is_empty() && !lucent_list.contains(&current_target) {
                self.selected_target.set(String::new());
                if let Ok(mut t) = self.params.target.write() {
                    t.clear();
                }
                sync_live(&self.params);
                telemetry_changed = true;
            }

            // Build options list: (broadcast) + discovered consumers
            let mut opts = vec!["(broadcast)".to_string()];
            opts.extend(lucent_list.clone());
            if opts != self.target_options.get() {
                self.target_options.set(opts);
                telemetry_changed = true;
            }

            // Map selected_target to index
            let tgt = self.selected_target.get();
            let idx = if tgt.is_empty() {
                0 // (broadcast)
            } else {
                lucent_list
                    .iter()
                    .position(|l| *l == tgt)
                    .map(|i| i + 1)
                    .unwrap_or(0)
            };
            // ponytail: only set if different, else ComboBox fires spurious on_select
            if self.selected_index.get() != idx {
                self.selected_index.set(idx);
                telemetry_changed = true;
            }

            let prev = self.telemetry.get();
            let mut next = prev.clone();
            next.lucent_list = lucent_list;
            next.now_ms = now_ms;
            next.connected = connected;
            if connected {
                next.last_connected_ms = Some(now_ms);
            }
            if next != prev {
                self.telemetry.set(next);
                telemetry_changed = true;
            }
        }
        let tick_us = t0_tick.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);
        // Keep the render loop alive so the layer-cached views repaint their
        // dynamic overlays every frame. The telemetry Signal is still only set
        // when values actually change.
        let _ = telemetry_changed;
        cx.needs_redraw();
        let total_us = t0_total.map(|t| t.elapsed().as_micros() as u64).unwrap_or(0);
        if profile {
            report_ticker(tick_us, total_us);
        }
    }
}

// ─── UI ────────────────────────────────────────────────────────────────────

pub fn build(cx: &mut Context, params: Arc<LucentRelayParams>) {
    let initial_name = params.name.read().map(|s| s.clone()).unwrap_or_default();
    let initial_target = params.target.read().map(|s| s.clone()).unwrap_or_default();

    let name_signal = Signal::new(initial_name);
    let selected_target = Signal::new(initial_target.clone());
    let telemetry = Signal::new(RelayTelemetry {
        lucent_list: Vec::new(),
        connected: false,
        last_connected_ms: None,
        now_ms: 0,
    });
    // ComboBox needs a Signal, not Memo — Ticker updates this each cycle.
    let target_options = Signal::new(vec!["(broadcast)".to_string()]);
    let selected_index = Signal::new(0usize);

    // Ticker: polls relay_hub every ~100ms, updates telemetry + options + selected_index
    Ticker::new(
        cx,
        params.clone(),
        telemetry,
        selected_target,
        target_options,
        selected_index,
    )
    .width(Pixels(1.0))
    .height(Pixels(1.0));

    VStack::new(cx, move |cx| {
        // ── HEADER ──────────────────────────────────────────────────────
        HStack::new(cx, |cx| {
            Label::new(cx, "LX")
                .font_size(16.0)
                .color(rgb(1.0, 0.45, 0.1));
            Label::new(cx, "AUDIOLABS")
                .font_size(16.0)
                .color(Color::white());
            Element::new(cx).width(Stretch(1.0));
            Label::new(cx, format!("Lucent-Relay {VERSION}"))
                .font_size(10.0)
                .color(col(0.55, 0.55, 0.55, 1.0));
        })
        .width(Stretch(1.0))
        .height(Pixels(36.0))
        .padding(Pixels(8.0))
        .alignment(Alignment::Center)
        .background_color(rgb(0.08, 0.08, 0.10));

        // Common styling for the small form widgets.
        const FORM_H: f32 = 22.0;
        const FORM_BG: Color = Color::rgb(230, 230, 230);
        const FORM_BORDER: Color = Color::rgb(140, 140, 140);
        const FORM_TEXT: Color = Color::rgb(40, 40, 40);
        const FORM_ARROW: Color = Color::rgb(100, 100, 100);

        // ── NAME INPUT ──────────────────────────────────────────────────
        let params_for_name = params.clone();
        HStack::new(cx, move |cx| {
            Label::new(cx, "Name")
                .font_size(11.0)
                .color(col(0.55, 0.55, 0.55, 1.0));
            Textbox::new(cx, name_signal)
                .on_edit(move |_cx, text| {
                    name_signal.set(text.clone());
                    if let Ok(mut n) = params_for_name.name.write() {
                        *n = text;
                    }
                    sync_live(&params_for_name);
                })
                .width(Stretch(1.0))
                .height(Pixels(FORM_H))
                .padding(Pixels(4.0))
                .font_size(11.0)
                .background_color(FORM_BG)
                .border_color(FORM_BORDER)
                .border_width(Pixels(1.0))
                .corner_radius(Pixels(2.0));
        })
        .width(Stretch(1.0))
        .height(Pixels(32.0))
        .padding(Pixels(8.0))
        .alignment(Alignment::Center)
        .horizontal_gap(Pixels(8.0));

        // ── TARGET DROPDOWN ─────────────────────────────────────────────
        let params_for_target = params.clone();
        let target_opts_trigger = target_options;
        let target_opts_popup = target_options;
        let selected_idx_trigger = selected_index;
        let selected_idx_popup = selected_index;
        let selected_tgt_popup = selected_target;
        HStack::new(cx, move |cx| {
            Label::new(cx, "Target")
                .font_size(11.0)
                .color(col(0.55, 0.55, 0.55, 1.0));

            Dropdown::new(
                cx,
                // Trigger: styled box showing current target + down arrow
                move |cx| {
                    let trigger_text = Memo::new(move |_| {
                        let opts = target_opts_trigger.get();
                        let idx = selected_idx_trigger.get();
                        opts.get(idx)
                            .cloned()
                            .unwrap_or_else(|| "(broadcast)".to_string())
                    });
                    HStack::new(cx, move |cx| {
                        Label::new(cx, trigger_text)
                            .font_size(11.0)
                            .color(FORM_TEXT)
                            .hoverable(false);
                        Element::new(cx).width(Stretch(1.0)).hoverable(false);
                        Label::new(cx, "▼")
                            .font_size(8.0)
                            .color(FORM_ARROW)
                            .hoverable(false);
                    })
                    .width(Stretch(1.0))
                    .height(Pixels(FORM_H))
                    .padding(Pixels(4.0))
                    .background_color(FORM_BG)
                    .border_color(FORM_BORDER)
                    .border_width(Pixels(1.0))
                    .corner_radius(Pixels(2.0))
                    .alignment(Alignment::Center)
                    .on_press(|cx| cx.emit(PopupEvent::Switch));
                },
                // Popup: scrollable list, max ~2 visible rows
                move |cx| {
                    let opts = target_opts_popup.get();
                    let sel_idx = selected_idx_popup;
                    let sel_tgt = selected_tgt_popup;
                    let params = params_for_target.clone();
                    ScrollView::new(cx, move |cx| {
                        VStack::new(cx, move |cx| {
                            for (idx, name) in opts.iter().enumerate() {
                                let name_clone = name.clone();
                                let name_for_label = name_clone.clone();
                                let sel_idx_c = sel_idx;
                                let sel_tgt_c = sel_tgt;
                                let params_c = params.clone();
                                HStack::new(cx, move |cx| {
                                    Label::new(cx, name_for_label)
                                        .font_size(11.0)
                                        .color(Color::black())
                                        .hoverable(false);
                                })
                                .width(Stretch(1.0))
                                .height(Pixels(20.0))
                                .padding(Pixels(4.0))
                                .background_color(Color::white())
                                .alignment(Alignment::Center)
                                .on_press(move |cx| {
                                    sel_idx_c.set(idx);
                                    let val = if idx == 0 {
                                        String::new()
                                    } else {
                                        name_clone.clone()
                                    };
                                    sel_tgt_c.set(val.clone());
                                    if let Ok(mut t) = params_c.target.write() {
                                        *t = val;
                                    }
                                    sync_live(&params_c);
                                    cx.emit(PopupEvent::Close);
                                });
                            }
                        })
                        .width(Pixels(190.0))
                        .height(Auto);
                    })
                    .width(Pixels(190.0))
                    .height(Auto)
                    .max_height(Pixels(56.0))
                    .background_color(Color::white());
                },
            )
            .width(Pixels(190.0))
            .height(Pixels(FORM_H))
            .placement(Placement::BottomStart)
            .should_reposition(false);
        })
        .width(Stretch(1.0))
        .height(Pixels(32.0))
        .padding(Pixels(8.0))
        .alignment(Alignment::Center)
        .horizontal_gap(Pixels(8.0));

        // ── CONNECTION STATUS ───────────────────────────────────────────
        HStack::new(cx, move |cx| {
            Label::new(
                cx,
                Memo::new(move |_| {
                    let t = telemetry.get();
                    if t.connected {
                        String::from("● Connected")
                    } else {
                        match t.last_connected_ms {
                            Some(last) => {
                                let elapsed = t.now_ms.saturating_sub(last);
                                if elapsed < 1000 {
                                    format!("● No Lucent ({elapsed} ms ago)")
                                } else {
                                    format!(
                                        "● No Lucent (last seen {:.1} s ago)",
                                        elapsed as f32 / 1000.0
                                    )
                                }
                            }
                            None => String::from("● No Lucent"),
                        }
                    }
                }),
            )
            .font_size(11.0)
            .color(Memo::new(move |_| {
                if telemetry.get().connected {
                    rgb(0.2, 0.9, 0.3)
                } else {
                    rgb(0.9, 0.2, 0.2)
                }
            }));
        })
        .width(Stretch(1.0))
        .padding(Pixels(8.0));
    })
    .width(Pixels(260.0))
    .height(Pixels(160.0))
    .vertical_gap(Pixels(4.0))
    .background_color(rgb(0.09, 0.09, 0.09));
}
