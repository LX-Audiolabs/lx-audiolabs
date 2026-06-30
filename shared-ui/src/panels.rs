use truce_iced::iced::widget::{button, column, container, row, scrollable, Space, Text, text_input};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Padding};

use crate::widgets::bold_font;

#[allow(clippy::too_many_arguments)]
pub fn ai_preset_panel<'a, Message: Clone + 'a>(
    title: &'a str,
    selected_name: Option<&'a str>,
    current_name_input: &str,
    on_name_change: impl Fn(String) -> Message + 'a,
    on_save_vault: Message,
    on_copy_prompt: Message,
    snap_label: &'a str,
    snap_blink: bool,
    on_vault_setup: Message,
    warning: Option<&'a str>,
    factory_presets: impl Iterator<Item = (&'a str, Message)> + 'a,
    user_presets: impl Iterator<Item = (&'a str, Message)> + 'a,
) -> Element<'a, Message> {
    let title_text = Text::new(title)
        .font(bold_font())
        .size(truce_iced::iced::Pixels(14.0))
        .color(Color::WHITE);

    let name_input = text_input("Preset Name...", current_name_input)
        .on_input(on_name_change)
        .padding(6)
        .size(truce_iced::iced::Pixels(13.0))
        .style(|_theme, status| {
            let focused = matches!(status, truce_iced::iced::widget::text_input::Status::Focused { .. });
            truce_iced::iced::widget::text_input::Style {
                background: Color::WHITE.into(),
                border: Border { color: Color::from_rgb(if focused { 0.55 } else { 0.4 }, if focused { 0.55 } else { 0.4 }, if focused { 0.55 } else { 0.4 }), width: 1.0, radius: truce_iced::iced::border::Radius::from(3.0) },
                icon: Color::from_rgb(0.4, 0.4, 0.4),
                placeholder: Color::from_rgb(0.5, 0.5, 0.5),
                value: Color::BLACK,
                selection: Color::from_rgba(1.0, 0.55, 0.1, 0.4),
            }
        });

    let btn_style = |_theme: &truce_iced::iced::Theme, status: button::Status| {
        let bg = if status == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.18, 0.18, 0.18) };
        button::Style {
            background: Some(bg.into()),
            text_color: Color::WHITE,
            border: Border { color: Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: truce_iced::iced::border::Radius::from(3.0) },
            ..Default::default()
        }
    };
    // top-biased padding: button height is fixed (34px) and iced top-aligns the
    // label, so a symmetric pad leaves the text sitting too high. +3px top centers it.
    let btn_pad = Padding { top: 7.0, bottom: 1.0, left: 0.0, right: 0.0 };
    let btn_h = Length::Fixed(34.0);

    let snap_btn_style = move |_theme: &truce_iced::iced::Theme, status: button::Status| {
        let (bg, border_col) = if snap_blink {
            (Color::from_rgb(0.55, 0.38, 0.05), Color::from_rgb(0.8, 0.55, 0.1))
        } else if status == button::Status::Hovered {
            (Color::from_rgb(0.25, 0.25, 0.25), Color::from_rgb(0.3, 0.3, 0.3))
        } else {
            (Color::from_rgb(0.18, 0.18, 0.18), Color::from_rgb(0.3, 0.3, 0.3))
        };
        button::Style {
            background: Some(bg.into()),
            // SNAP is the special feature → text always amber (brighter while blinking).
            text_color: if snap_blink { Color::from_rgb(1.0, 0.85, 0.3) } else { Color::from_rgb(1.0, 0.55, 0.1) },
            border: Border { color: border_col, width: 1.0, radius: truce_iced::iced::border::Radius::from(3.0) },
            ..Default::default()
        }
    };
    // SNAP + Vault buttons share the size-12 bold style (cross-plugin convention).
    let btn_copy = button(Text::new(snap_label).font(bold_font()).size(truce_iced::iced::Pixels(12.0)).width(Length::Fill).align_x(Alignment::Center))
        .on_press(on_copy_prompt).style(snap_btn_style).width(Length::Fill).height(btn_h).padding(btn_pad);
    let btn_save = button(Text::new("SAVE").font(bold_font()).size(truce_iced::iced::Pixels(12.0)).width(Length::Fill).align_x(Alignment::Center))
        .on_press(on_save_vault).style(btn_style).width(Length::Fill).height(btn_h).padding(btn_pad);
    let btn_setup = button(Text::new("VAULT SETUP").font(bold_font()).size(truce_iced::iced::Pixels(12.0)).width(Length::Fill).align_x(Alignment::Center))
        .on_press(on_vault_setup).style(btn_style).width(Length::Fill).height(btn_h).padding(btn_pad);

    let mut controls = column![
        name_input,
        row![btn_copy, btn_save].spacing(4),
        btn_setup,
    ].spacing(6);

    if let Some(warn) = warning {
        let warn_el = container(
            Text::new(warn).size(truce_iced::iced::Pixels(9.0))
                .color(Color::from_rgb(1.0, 0.75, 0.2)),
        )
        .padding(Padding { top: 4.0, bottom: 4.0, left: 6.0, right: 6.0 })
        .width(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Color::from_rgb(0.18, 0.13, 0.04).into()),
            border: Border { color: Color::from_rgb(0.4, 0.3, 0.1), width: 1.0, radius: truce_iced::iced::border::Radius::from(2.0) },
            ..Default::default()
        });
        controls = controls.push(warn_el);
    }

    let factory_items: Vec<_> = factory_presets.collect();
    let user_items: Vec<_> = user_presets.collect();

    let mut list_col = column![].spacing(4);

    if !factory_items.is_empty() {
        let mut factory_col = column![
            Text::new("── Factory ──────────────────")
                .size(truce_iced::iced::Pixels(11.0))
                .font(bold_font())
                .color(Color::from_rgb(1.0, 0.55, 0.15))
        ].spacing(4);
        for (name, on_load) in factory_items {
            let is_sel = selected_name == Some(name);
            factory_col = factory_col.push(preset_list_item(name, is_sel, on_load));
        }
        list_col = list_col.push(factory_col);
        if !user_items.is_empty() {
            list_col = list_col.push(Space::new().height(6.0));
        }
    }

    if !user_items.is_empty() {
        let mut user_col = column![
            Text::new("── Vault Presets ────────────")
                .size(truce_iced::iced::Pixels(11.0))
                .font(bold_font())
                .color(Color::from_rgb(1.0, 0.55, 0.15))
        ].spacing(4);
        for (name, on_load) in user_items {
            let is_sel = selected_name == Some(name);
            user_col = user_col.push(preset_list_item(name, is_sel, on_load));
        }
        list_col = list_col.push(user_col);
    }

    let preset_list = scrollable(list_col).height(Length::Fill);

    container(
        column![
            title_text,
            controls,
            Space::new().height(10.0),
            preset_list
        ].spacing(10)
    )
    .width(Length::Fixed(180.0))
    .height(Length::Fill)
    .padding(10.0)
    .style(|_theme| container::Style {
        background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()),
        border: Border {
            color: Color::from_rgb(0.18, 0.18, 0.18),
            width: 1.0,
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

pub fn vault_setup_box<'a, Message: Clone + 'a>(
    plugin_name: &str,
    vault_path_input: &str,
    on_path_change: impl Fn(String) -> Message + 'a,
    on_save: Message,
    on_cancel: Message,
) -> Element<'a, Message> {
    fn btn_style(_theme: &truce_iced::iced::Theme, status: button::Status) -> button::Style {
        let bg = if status == button::Status::Hovered {
            Color::from_rgb(0.25, 0.25, 0.25)
        } else {
            Color::from_rgb(0.15, 0.15, 0.15)
        };
        button::Style { background: Some(bg.into()), text_color: Color::WHITE, ..Default::default() }
    }

    container(
        column![
            Text::new("LX AUDIOLABS - SETUP").size(18).font(bold_font()).color(Color::WHITE),
            Text::new(format!("Configure your Vault path for {}:", plugin_name)).size(12).color(Color::WHITE),
            text_input("Enter Vault absolute path...", vault_path_input)
                .on_input(on_path_change)
                .padding(8)
                .width(Length::Fill),
            row![
                button(Text::new("SAVE").size(12).font(bold_font()))
                    .on_press(on_save)
                    .padding(8)
                    .style(btn_style),
                button(Text::new("CANCEL").size(12))
                    .on_press(on_cancel)
                    .padding(8)
                    .style(btn_style),
            ]
            .spacing(10)
        ]
        .spacing(15)
        .max_width(600.0)
    )
    .padding(20)
    .style(|_theme| container::Style {
        background: Some(Color::from_rgb(0.15, 0.15, 0.15).into()),
        border: Border {
            color: Color::from_rgb(0.3, 0.3, 0.3),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    })
    .into()
}

pub fn preset_list_item<'a, Message: Clone + 'a>(
    name: &'a str,
    is_selected: bool,
    on_load: Message,
) -> Element<'a, Message> {
    let name_btn = button(Text::new(format!("> {}", name)).font(bold_font()).size(truce_iced::iced::Pixels(13.0)))
        .style(move |_theme, status| {
            let color = if is_selected {
                Color::from_rgb(1.0, 0.45, 0.1)
            } else if status == button::Status::Hovered {
                Color::WHITE
            } else {
                Color::from_rgb(0.9, 0.9, 0.9)
            };
            let bg = if is_selected {
                Color::from_rgb(0.18, 0.14, 0.08)
            } else {
                Color::TRANSPARENT
            };
            button::Style { background: Some(bg.into()), text_color: color, ..Default::default() }
        })
        .on_press(on_load)
        .width(Length::Fill);

    name_btn.into()
}
