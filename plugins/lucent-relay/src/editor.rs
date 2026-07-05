//! Vizia port of the Iced editor. Lucent-Relay has no custom canvas,
//! spectrum, or knobs — pure form widgets: name Textbox, target PickList,
//! connection status Label. Uses same Ticker pattern as Lucent's editor.rs
//! (NOT cx.add_timer — known vizia_core 0.4.0 infinite-loop bug).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use vizia::prelude::*;
use vizia::vg;

use shared_analysis::relay_hub;
use crate::{LucentRelayParams, RelayHandle};

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

// ─── Registry (same keyed-by-Arc-ptr pattern as Iced version) ──────────────

static RELAY_HANDLES: OnceLock<Mutex<HashMap<usize, RelayHandle>>> = OnceLock::new();

fn params_key(params: &Arc<LucentRelayParams>) -> usize {
    Arc::as_ptr(params) as usize
}

pub fn set_relay_handle(key: usize, h: RelayHandle) {
    let map = RELAY_HANDLES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut m) = map.lock() {
        m.insert(key, h);
    }
}

pub fn remove_relay_handle(key: usize) {
    if let Some(map) = RELAY_HANDLES.get()
        && let Ok(mut m) = map.lock() {
            m.remove(&key);
        }
}

fn take_relay_handle(key: usize) -> Option<RelayHandle> {
    RELAY_HANDLES.get()?.lock().ok()?.get(&key).cloned()
}

// ─── Telemetry snapshot (updated by Ticker) ────────────────────────────────

#[derive(Clone)]
struct RelayTelemetry {
    lucent_list: Vec<String>,
    connected: bool,
    last_connected_ms: Option<u64>,
    now_ms: u64,
}

// ─── Ticker — polls relay_hub every ~500ms ─────────────────────────────────

struct Ticker {
    handle: RelayHandle,
    telemetry: Signal<RelayTelemetry>,
    last_tick: RefCell<Instant>,
    selected_target: Signal<String>,
    target_options: Signal<Vec<String>>,
    selected_index: Signal<usize>,
}

impl Ticker {
    fn new(
        cx: &mut Context,
        handle: RelayHandle,
        telemetry: Signal<RelayTelemetry>,
        selected_target: Signal<String>,
        target_options: Signal<Vec<String>>,
        selected_index: Signal<usize>,
    ) -> Handle<'_, Self> {
        Self { handle, telemetry, last_tick: RefCell::new(Instant::now()), selected_target, target_options, selected_index }
            .build(cx, |_| {})
    }
}

const TICK_INTERVAL: Duration = Duration::from_millis(500);

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
            let now_ms = shared_analysis::shm::now_ms();
            let lucent_list = relay_hub()
                .map(|hub| hub.read_consumers(now_ms))
                .unwrap_or_default();
            let connected = relay_hub()
                .map(|hub| {
                    let t = self.handle.target();
                    if t.is_empty() {
                        !hub.read_consumers(now_ms).is_empty()
                    } else {
                        hub.consumer_exists(&t, now_ms)
                    }
                })
                .unwrap_or(false);

            // Build options list: (broadcast) + discovered consumers
            let mut opts = vec!["(broadcast)".to_string()];
            opts.extend(lucent_list.clone());
            self.target_options.set(opts);

            // Map selected_target to index
            let tgt = self.selected_target.get();
            let idx = if tgt.is_empty() {
                0 // (broadcast)
            } else {
                lucent_list.iter().position(|l| *l == tgt).map(|i| i + 1).unwrap_or(0)
            };
            // ponytail: only set if different, else ComboBox fires spurious on_select
            if self.selected_index.get() != idx {
                self.selected_index.set(idx);
            }

            // Keep selected target if still valid, else clear.
            let target = self.handle.target();
            if !target.is_empty() && !lucent_list.contains(&target) {
                self.selected_target.set(String::new());
            }

            let mut t = self.telemetry.get();
            t.lucent_list = lucent_list;
            t.now_ms = now_ms;
            t.connected = connected;
            if connected {
                t.last_connected_ms = Some(now_ms);
            }
            self.telemetry.update(|tt| *tt = t);
        }
        cx.needs_redraw();
    }
}

// ─── UI ────────────────────────────────────────────────────────────────────

pub fn build(cx: &mut Context, params: Arc<LucentRelayParams>) {
    let handle = take_relay_handle(params_key(&params)).unwrap_or_default();
    let initial_name = handle.name();
    let initial_target = handle.target();

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
    let selected_index = Signal::new(
        if initial_target.is_empty() { 0usize } else { 0 }
    );

    // Ticker: polls relay_hub every ~500ms, updates telemetry + options + selected_index
    Ticker::new(cx, handle.clone(), telemetry, selected_target, target_options, selected_index)
        .width(Pixels(1.0))
        .height(Pixels(1.0));

    VStack::new(cx, move |cx| {
        // ── HEADER ──────────────────────────────────────────────────────
        HStack::new(cx, |cx| {
            Label::new(cx, "LX").font_size(16.0).color(rgb(1.0, 0.45, 0.1));
            Label::new(cx, "AUDIOLABS").font_size(16.0).color(Color::white());
            Element::new(cx).width(Stretch(1.0));
            Label::new(cx, format!("Lucent-Relay {VERSION}")).font_size(10.0).color(col(0.55, 0.55, 0.55, 1.0));
        })
        .width(Stretch(1.0))
        .height(Pixels(36.0))
        .padding(Pixels(8.0))
        .alignment(Alignment::Center)
        .background_color(rgb(0.08, 0.08, 0.10));

        // ── NAME INPUT ──────────────────────────────────────────────────
        let handle_for_name1 = handle.clone();
        HStack::new(cx, move |cx| {
            Label::new(cx, "Name").font_size(11.0).color(col(0.55, 0.55, 0.55, 1.0));
            let handle_for_name = handle_for_name1;
            Textbox::new(cx, name_signal)
                .on_edit(move |_cx, text| {
                    name_signal.set(text.clone());
                    if let Ok(mut g) = handle_for_name.0.lock() {
                        g.name = text;
                    }
                })
                .width(Stretch(1.0))
                .height(Pixels(20.0))
                .font_size(11.0);
        })
        .width(Stretch(1.0))
        .height(Pixels(32.0))
        .padding(Pixels(8.0))
        .alignment(Alignment::Center)
        .horizontal_gap(Pixels(8.0));

        // ── TARGET DROPDOWN (ComboBox) ──────────────────────────────────
        let handle_for_target = handle.clone();
        HStack::new(cx, move |cx| {
            Label::new(cx, "Target").font_size(11.0).color(col(0.55, 0.55, 0.55, 1.0));

            let handle_for_target = handle_for_target;
            let target_opts = target_options;
            ComboBox::new(cx, target_opts, selected_index)
                .on_select(move |_cx, index| {
                    let opts = target_opts.get();
                    let val = if index == 0 || index >= opts.len() {
                        String::new()
                    } else {
                        opts[index].clone()
                    };
                    selected_target.set(val.clone());
                    if let Ok(mut g) = handle_for_target.0.lock() {
                        g.target = val;
                    }
                })
                .width(Stretch(1.0))
                .height(Pixels(20.0))
                .font_size(11.0);
        })
        .width(Stretch(1.0))
        .height(Pixels(32.0))
        .padding(Pixels(8.0))
        .alignment(Alignment::Center)
        .horizontal_gap(Pixels(8.0));

        // ── CONNECTION STATUS ───────────────────────────────────────────
        HStack::new(cx, move |cx| {
            Label::new(cx, Memo::new(move |_| {
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
                                format!("● No Lucent (last seen {:.1} s ago)", elapsed as f32 / 1000.0)
                            }
                        }
                        None => String::from("● No Lucent"),
                    }
                }
            }))
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

