use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use truce_iced::iced::widget::{column, container, pick_list, row, text, text_input, Space};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Subscription};
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;

use shared_analysis::relay_hub;
use shared_ui::{bold_font, header_brand};
use crate::{LucentRelayParams, RelayHandle};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Registry so `RelayUi::new()` can reach the handle belonging to the same
/// plugin instance without changing the `IcedPlugin::new(params)` trait
/// signature. Keyed by `Arc::as_ptr(&params)` — that Arc is the same
/// allocation the plugin constructor and the editor constructor both see,
/// so the key is unique per instance regardless of how many Lucent-Relay
/// instances share the process (was: single `OnceLock`, so every instance
/// after the first read/wrote the first instance's handle).
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
    if let Some(map) = RELAY_HANDLES.get() {
        if let Ok(mut m) = map.lock() {
            m.remove(&key);
        }
    }
}

fn take_relay_handle(key: usize) -> Option<RelayHandle> {
    RELAY_HANDLES.get()?.lock().ok()?.get(&key).cloned()
}

#[derive(Debug, Clone)]
pub enum RelayMsg {
    Tick,
    NameChanged(String),
    TargetSelected(String),
}

pub struct RelayUi {
    handle: RelayHandle,
    name_buf: String,
    lucent_list: Vec<String>,
    selected_target: String,
    connected: bool,
    /// Wall-clock ms of the last Tick where `connected` was true. Used to show
    /// "ms since last seen" when disconnected — shm-hub only exposes a live/
    /// dead bool (`consumer_exists`), not the underlying heartbeat timestamp,
    /// so this is tracked client-side instead of adding a new shm-hub API.
    last_connected_ms: Option<u64>,
    now_ms: u64,
}

impl IcedPlugin<LucentRelayParams> for RelayUi {
    type Message = RelayMsg;

    fn new(params: Arc<LucentRelayParams>) -> Self {
        let handle = take_relay_handle(params_key(&params)).unwrap_or_default();
        let name_buf = handle.name();
        let target = handle.target();
        Self {
            handle,
            name_buf,
            lucent_list: Vec::new(),
            selected_target: target,
            connected: false,
            last_connected_ms: None,
            now_ms: 0,
        }
    }

    fn subscription(&self) -> Subscription<Message<RelayMsg>> {
        truce_iced::iced::event::listen_raw(|event, _status, _window| {
            use truce_iced::iced::{Event, window::Event as WinEvent};
            match event {
                Event::Window(WinEvent::RedrawRequested(_)) => {
                    Some(Message::Plugin(RelayMsg::Tick))
                }
                _ => None,
            }
        })
    }

    fn update(
        &mut self,
        message: Message<RelayMsg>,
        _params: &ParamCache<LucentRelayParams>,
        _ctx: &PluginContext<LucentRelayParams>,
    ) -> truce_iced::iced::Task<Message<RelayMsg>> {
        let Message::Plugin(msg) = message else { return truce_iced::iced::Task::none(); };

        match msg {
            RelayMsg::Tick => {
                let now = shared_analysis::shm::now_ms();
                self.now_ms = now;
                self.lucent_list = relay_hub()
                    .map(|hub| hub.read_consumers(now))
                    .unwrap_or_default();
                self.connected = relay_hub()
                    .map(|hub| {
                        let t = self.handle.target();
                        if t.is_empty() {
                            !hub.read_consumers(now).is_empty()
                        } else {
                            hub.consumer_exists(&t, now)
                        }
                    })
                    .unwrap_or(false);
                if self.connected {
                    self.last_connected_ms = Some(now);
                }
                // Keep selected target if still valid, else clear.
                let t = self.handle.target();
                if !t.is_empty() && self.lucent_list.iter().any(|l| *l == t) {
                    self.selected_target = t;
                }
            }
            RelayMsg::NameChanged(name) => {
                self.name_buf = name.clone();
                if let Ok(mut g) = self.handle.0.lock() {
                    g.name = name;
                }
            }
            RelayMsg::TargetSelected(target) => {
                self.selected_target = target.clone();
                if let Ok(mut g) = self.handle.0.lock() {
                    g.target = target;
                }
            }
        }

        truce_iced::iced::Task::none()
    }

    fn needs_redraw(&self) -> bool {
        true // always repaint — SHM slot/heartbeat can change without params or UI input
    }

    fn view<'a>(
        &'a self,
        _params: &'a ParamCache<LucentRelayParams>,
    ) -> Element<'a, Message<RelayMsg>> {
        // ── HEADER ──────────────────────────────────────────────────────────
        let header = container(
            row![
                header_brand("Lucent-Relay", VERSION),
                Space::new().width(Length::Fill),
            ]
            .align_y(Alignment::Center)
            .padding(8),
        )
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.10).into()),
            border: Border {
                color: Color::from_rgb(0.15, 0.15, 0.15),
                width: 1.0,
                ..Default::default()
            },
            ..Default::default()
        });

        // ── NAME INPUT ──────────────────────────────────────────────────────
        let name_row = row![
            text("Name").size(10).font(bold_font()).color(Color::from_rgb(0.55, 0.55, 0.55)),
            Space::new().width(Length::Fixed(8.0)),
            text_input("Relay name", &self.name_buf)
                .on_input(|s| Message::Plugin(RelayMsg::NameChanged(s)))
                .padding(4)
                .size(11)
                .width(Length::Fill),
        ]
        .align_y(Alignment::Center)
        .padding([4.0, 12.0]);

        // ── TARGET DROPDOWN ─────────────────────────────────────────────────
        let target_label = text("Target Lucent")
            .size(10)
            .font(bold_font())
            .color(Color::from_rgb(0.55, 0.55, 0.55));

        let target_options: Vec<String> = {
            let mut opts = vec![String::from("(broadcast)")];
            opts.extend(self.lucent_list.clone());
            opts
        };
        let current_target = if self.selected_target.is_empty() {
            String::from("(broadcast)")
        } else {
            self.selected_target.clone()
        };

        let target_dropdown = pick_list(
            target_options,
            Some(current_target),
            |selected| {
                let val = if selected == "(broadcast)" {
                    String::new()
                } else {
                    selected
                };
                Message::Plugin(RelayMsg::TargetSelected(val))
            },
        )
        .padding(4)
        .text_size(11);

        let target_row = row![
            target_label,
            Space::new().width(Length::Fixed(8.0)),
            target_dropdown,
        ]
        .align_y(Alignment::Center)
        .padding([4.0, 12.0]);

        // ── CONNECTION STATUS ──────────────────────────────────────────────
        let (conn_color, conn_text) = if self.connected {
            (Color::from_rgb(0.2, 0.9, 0.3), "● Connected".to_string())
        } else {
            match self.last_connected_ms {
                Some(last) => {
                    let elapsed = self.now_ms.saturating_sub(last);
                    let ago = if elapsed < 1000 {
                        format!("{elapsed} ms ago")
                    } else {
                        format!("{:.1} s ago", elapsed as f32 / 1000.0)
                    };
                    (Color::from_rgb(0.9, 0.2, 0.2), format!("● No Lucent (last seen {ago})"))
                }
                None => (Color::from_rgb(0.9, 0.2, 0.2), "● No Lucent".to_string()),
            }
        };

        let conn_indicator = row![
            text(conn_text).size(10).font(bold_font()).color(conn_color),
            Space::new().width(Length::Fill),
        ]
        .padding([2.0, 12.0])
        .align_y(Alignment::Center);

        column![header, name_row, target_row, conn_indicator]
            .spacing(4)
            .into()
    }
}
