use nice_plug_iced::iced::widget::{column, container, pick_list, text_input, Text};
use nice_plug_iced::iced::{Alignment, Background, Color, Element, Length};
use nice_plug_iced::NiceGuiContext;
use nice_plug::editor::Editor;
use std::sync::Arc;

use shared_analysis::SharedState;
use shared_ui::{bold_font, header_brand};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    NameChanged(String),
    TargetSelected(String),
}

pub struct EditorModel {
    params: Arc<crate::LucentRelayParams>,
    #[allow(dead_code)]
    shared_state: Arc<SharedState>,
    #[allow(dead_code)]
    nice_ctx: NiceGuiContext,
    /// Local text buffer for the name field (mirrors `params.name`).
    name_input: String,
    /// Selected target Lucent (mirrors `params.target`). Empty = none chosen.
    target: String,
    /// Live Lucent instance names, refreshed each Tick from the shared hub.
    lucent_names: Vec<String>,
}

fn boot(
    nice_ctx: NiceGuiContext,
    params: Arc<crate::LucentRelayParams>,
    shared_state: Arc<SharedState>,
) -> (EditorModel, nice_plug_iced::iced::Task<Message>) {
    let name_input = params.name.read().map(|n| n.clone()).unwrap_or_default();
    let target = params.target.read().map(|t| t.clone()).unwrap_or_default();
    let model = EditorModel {
        params,
        shared_state,
        nice_ctx,
        name_input,
        target,
        lucent_names: Vec::new(),
    };
    (model, nice_plug_iced::iced::Task::none())
}

fn update(model: &mut EditorModel, message: Message) -> nice_plug_iced::iced::Task<Message> {
    match message {
        Message::Tick => {
            let now = shared_analysis::shm::now_ms();
            let names = shared_analysis::relay_hub().map(|hub| hub.read_lucents(now)).unwrap_or_default();
            if names != model.lucent_names { model.lucent_names = names; }
        }
        Message::NameChanged(value) => {
            model.name_input = value;
            if let Ok(mut name) = model.params.name.write() { *name = model.name_input.clone(); }
        }
        Message::TargetSelected(value) => {
            model.target = value;
            if let Ok(mut target) = model.params.target.write() { *target = model.target.clone(); }
        }
    }
    nice_plug_iced::iced::Task::none()
}

fn view(model: &EditorModel) -> Element<'_, Message> {
    let gr = Color::from_rgb(0.5, 0.5, 0.5);
    let gn = Color::from_rgb(0.15, 0.85, 0.35);
    let am = Color::from_rgb(1.0, 0.55, 0.15);

    let name_field = text_input("Name", &model.name_input)
        .on_input(Message::NameChanged)
        .size(12).padding(4)
        .width(Length::Fixed(140.0));

    let selected = if model.target.is_empty() { None } else { Some(model.target.clone()) };
    let target_dropdown = pick_list(model.lucent_names.clone(), selected, Message::TargetSelected)
        .placeholder("Select Lucent...")
        .text_size(11).padding(4)
        .width(Length::Fixed(150.0));

    // Connection status: resolved target name or "No Lucent found"
    let (status_text, status_color) = if model.lucent_names.is_empty() {
        ("No Lucent found".to_string(), am)
    } else if !model.target.is_empty() && model.lucent_names.contains(&model.target) {
        (format!("→ {}", model.target), gn)
    } else if model.lucent_names.len() == 1 {
        (format!("→ {} (auto)", model.lucent_names[0]), gn)
    } else {
        ("Select a Lucent".to_string(), gr)
    };

    container(
        column![
            header_brand("Lucent Relay", VERSION),
            name_field,
            target_dropdown,
            Text::new(status_text).size(11).font(bold_font()).color(status_color),
        ]
        .spacing(8)
        .padding(10)
        .align_x(Alignment::Center)
    )
    .width(Length::Fill).height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.07, 0.07, 0.07))),
        text_color: Some(Color::WHITE),
        ..Default::default()
    })
    .into()
}

pub fn create(
    params: Arc<crate::LucentRelayParams>,
    shared_state: Arc<SharedState>,
) -> Option<Box<dyn Editor>> {
    let window_state = params.editor_state.clone();
    let notifier = nice_plug_iced::iced::PollSubNotifier::default();
    let settings = nice_plug_iced::EditorSettings {
        ignore_non_modifier_keys: false,
        always_redraw: true,  // FIX: Bitwig 6.1 Beta + Win10 canvas blank (revert to false if Win11 issues)
    };

    let params_clone = params.clone();
    let shared_state_clone = shared_state.clone();

    nice_plug_iced::create_iced_editor(
        window_state,
        (),
        notifier,
        settings,
        move |editor_state, nice_ctx| {
            let params = params_clone.clone();
            let shared_state = shared_state_clone.clone();

            nice_plug_iced::application(
                editor_state,
                nice_ctx.clone(),
                move |_, ctx| boot(ctx, params.clone(), shared_state.clone()),
                update,
                view,
            )
            .subscription(move |_state| {
                nice_plug_iced::iced::window::frames().map(|_| Message::Tick)
            })
            .run()
        },
    )
}
