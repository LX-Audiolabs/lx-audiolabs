// Meridian editor — Iced UI truce port (stub).
// Full UI rebuild pending.

use truce_iced::iced;
use truce_iced::iced::widget::{column, container, Text};
use truce_iced::iced::{Element, Length, Subscription};
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;
use std::sync::Arc;

use shared_analysis::SharedState;

use crate::MeridianParams;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub enum MeridianMsg {
    Tick,
}

pub struct MeridianEditor {
    _params: Arc<MeridianParams>,
    _shared_state: Arc<SharedState>,
}

impl IcedPlugin<MeridianParams> for MeridianEditor {
    type Message = MeridianMsg;

    fn new(params: Arc<MeridianParams>) -> Self {
        Self {
            _shared_state: params.shared.clone(),
            _params: params,
        }
    }

    fn subscription(&self) -> Subscription<Message<MeridianMsg>> {
        truce_iced::iced::event::listen_raw(|event, _status, _window| {
            use truce_iced::iced::{Event, window::Event as WinEvent};
            match event {
                Event::Window(WinEvent::RedrawRequested(_)) => Some(Message::Plugin(MeridianMsg::Tick)),
                _ => None,
            }
        })
    }

    fn needs_redraw(&self) -> bool { true }

    fn update(
        &mut self,
        _message: Message<MeridianMsg>,
        _cache: &ParamCache<MeridianParams>,
        _ctx: &PluginContext<MeridianParams>,
    ) -> iced::Task<Message<MeridianMsg>> {
        iced::Task::none()
    }

    fn view(&self, _cache: &ParamCache<MeridianParams>) -> Element<'_, Message<MeridianMsg>> {
        let content = column![
            Text::new(format!("MERIDIAN {}", VERSION)).size(24),
            Text::new("DSP fully migrated. Full UI rebuild pending.").size(14),
        ]
        .spacing(16)
        .padding(24);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
