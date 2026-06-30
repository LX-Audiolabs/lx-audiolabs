use std::sync::{Arc, OnceLock};

use truce_iced::iced::widget::{column, container, pick_list, row, text, text_input, Space};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Subscription};
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;

use shared_analysis::relay_hub;
use shared_ui::{bold_font, header_brand};
use crate::{LucentRelayParams, RelayHandle};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Static slot so `RelayUi::new()` can reach the handle without changing
/// the `IcedPlugin::new(params)` trait signature. Set once in the plugin
/// constructor, read once in the editor constructor.
static RELAY_HANDLE: OnceLock<RelayHandle> = OnceLock::new();

pub fn set_relay_handle(h: RelayHandle) {
    let _ = RELAY_HANDLE.set(h);
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
}

impl IcedPlugin<LucentRelayParams> for RelayUi {
    type Message = RelayMsg;

    fn new(_params: Arc<LucentRelayParams>) -> Self {
        let handle = RELAY_HANDLE.get().cloned().unwrap_or_default();
        let name_buf = handle.name();
        let target = handle.target();
        Self {
            handle,
            name_buf,
            lucent_list: Vec::new(),
            selected_target: target,
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
                self.lucent_list = relay_hub()
                    .map(|hub| hub.read_lucents(now))
                    .unwrap_or_default();
                // Keep selected target if still valid, else clear.
                let t = self.handle.target();
                if !t.is_empty() {
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

        // ── STATUS ──────────────────────────────────────────────────────────
        let status_text = if self.lucent_list.is_empty() {
            "No Lucent instances found — open a Lucent plugin first"
        } else {
            "FFT relay active — spectrum sent to selected Lucent"
        };
        let status = container(
            text(status_text)
                .size(10)
                .color(Color::from_rgb(0.40, 0.40, 0.50)),
        )
        .padding([10.0, 12.0])
        .width(Length::Fill);

        column![header, name_row, target_row, status]
            .spacing(4)
            .into()
    }
}
