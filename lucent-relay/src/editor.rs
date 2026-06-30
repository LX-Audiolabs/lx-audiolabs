use std::sync::Arc;
use truce_iced::iced::widget::{Column, container, text};
use truce_iced::iced::{Color, Element, Length};
use truce_iced::{IcedPlugin, Message, ParamCache};
use crate::LucentRelayParams;

pub struct RelayUi;

#[derive(Debug, Clone)]
pub enum RelayMsg {}

impl IcedPlugin<LucentRelayParams> for RelayUi {
    type Message = RelayMsg;

    fn new(_params: Arc<LucentRelayParams>) -> Self {
        RelayUi
    }

    fn view<'a>(&'a self, _params: &'a ParamCache<LucentRelayParams>) -> Element<'a, Message<RelayMsg>> {
        Column::new()
            .push(
                container(
                    text("LX AUDIOLABS — LUCENT RELAY")
                        .size(9)
                        .color(Color::from_rgb(0.53, 0.53, 0.60)),
                )
                .padding([6.0, 10.0])
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgb(0.08, 0.08, 0.10).into()),
                    ..Default::default()
                }),
            )
            .push(
                container(
                    text("FFT relay active — connect Lucent to receive spectrum")
                        .size(10)
                        .color(Color::from_rgb(0.40, 0.40, 0.50)),
                )
                .padding([20.0, 12.0])
                .width(Length::Fill),
            )
            .into()
    }
}
