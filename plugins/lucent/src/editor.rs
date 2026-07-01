use truce_iced::iced::widget::{button, canvas, column, container, row, text_input, Space, Text};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Padding, Subscription};
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;
use std::sync::{Arc, atomic::Ordering};

use shared_analysis::{SharedState, SPECTRUM_BINS};
use crate::resonance_hub;
use crate::LucentParamsParamId;
use shared_ui::{
    bold_font, header_brand, output_level_block, output_tools_strip,
    toggle_button, vault_setup_box,
    GoniometerCanvas, SpectrumCanvas, SpectrumCurve, SpectrumConfig,
};
use crate::ui::LucentUiState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub enum LucentMsg {
    Tick,
    RelayToggled(usize),
    SnapPressed,
    CycleMode,
    LucentNameChanged(String),
    SetupToggled,
    VaultPathChanged(String),
    SaveVaultPath,
    ResonanceToggled,
    MaskingToggled,
    ResetPeak,
    ResetAll,
}

pub struct LucentEditor {
    params: Arc<crate::LucentParams>,
    shared_state: Arc<SharedState>,
    ui_state: LucentUiState,
    resonance_cache: Vec<(usize, f32)>,
    masking_cache: Vec<f32>,
    show_resonance: bool,
    show_masking: bool,
    snap_blink: u32,
    vault_path: Option<String>,
    show_setup: bool,
    vault_path_input: String,
    output_peak: f32,
    peak_hold: f32,
    peak_l: f32,
    peak_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    phase_correlation: f32,
    balance: f32,
}

impl IcedPlugin<crate::LucentParams> for LucentEditor {
    type Message = LucentMsg;

    fn new(params: Arc<crate::LucentParams>) -> Self {
        let config = shared_analysis::load_config("Lucent");
        let mut ui_state = LucentUiState::new();
        if let Ok(name) = params.name.read() {
            if !name.is_empty() { ui_state.name = name.clone(); }
        }
        let shared_state = params.shared.clone();
        Self {
            params,
            shared_state,
            ui_state,
            resonance_cache: Vec::new(),
            masking_cache: Vec::new(),
            show_resonance: false,
            show_masking: false,
            snap_blink: 0,
            vault_path: config.vault_path.clone(),
            show_setup: false,
            vault_path_input: config.vault_path.unwrap_or_default(),
            output_peak: -90.0,
            peak_hold: -90.0,
            peak_l: -90.0,
            peak_r: -90.0,
            peak_hold_l: -90.0,
            peak_hold_r: -90.0,
            phase_correlation: 1.0,
            balance: 0.0,
        }
    }

    fn subscription(&self) -> Subscription<Message<LucentMsg>> {
        truce_iced::iced::event::listen_raw(|event, _status, _window| {
            use truce_iced::iced::{Event, window::Event as WinEvent};
            match event {
                Event::Window(WinEvent::RedrawRequested(_)) => {
                    Some(Message::Plugin(LucentMsg::Tick))
                }
                _ => None,
            }
        })
    }

    /// Streaming editor: spectrum, masking, resonance, relay feeds, and
    /// meters update continuously from the audio thread via lock-free
    /// atomics and shared state. The idle gate would otherwise skip
    /// frames when no UI input or param change fires.
    fn needs_redraw(&self) -> bool {
        true
    }

    fn update(
        &mut self,
        message: Message<LucentMsg>,
        params: &ParamCache<crate::LucentParams>,
        ctx: &PluginContext<crate::LucentParams>,
    ) -> truce_iced::iced::Task<Message<LucentMsg>> {
        let Message::Plugin(msg) = message else { return truce_iced::iced::Task::none(); };

        match msg {
            LucentMsg::Tick => {
                if let Ok(spectrum) = self.shared_state.spectrum_avg.lock() {
                    self.ui_state.own_spectrum = spectrum.to_vec();
                }
                if let Ok(peaks) = resonance_hub().lock() {
                    self.resonance_cache = peaks.clone();
                }
                if let Ok(mm) = self.shared_state.masking_map.lock() {
                    self.masking_cache = mm.to_vec();
                }
                let mode = params.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
                if mode != 0 {
                    let now_ms = shared_analysis::shm::now_ms();
                    let slot = self.shared_state.shm_slot.load(Ordering::Acquire);
                    let raw = self.params.name.try_read()
                        .map(|n| n.clone())
                        .unwrap_or_else(|_| self.ui_state.name.clone());
                    let my_name = if slot >= 0 {
                        shared_analysis::shm::display_name(&raw, slot as u8)
                    } else {
                        raw
                    };
                    let feeds = shared_analysis::relay_hub()
                        .map(|hub| hub.read_active(&my_name, now_ms))
                        .unwrap_or_default();
                    self.ui_state.sync_relays(feeds);
                } else {
                    self.ui_state.clear_relays();
                }
                self.output_peak = self.shared_state.output_peak.load(Ordering::Relaxed);
                self.peak_hold = self.shared_state.peak_hold.load(Ordering::Relaxed);
                self.peak_l = self.shared_state.output_peak_l.load(Ordering::Relaxed);
                self.peak_r = self.shared_state.output_peak_r.load(Ordering::Relaxed);
                self.peak_hold_l = self.shared_state.peak_hold_l.load(Ordering::Relaxed);
                self.peak_hold_r = self.shared_state.peak_hold_r.load(Ordering::Relaxed);
                self.phase_correlation = self.shared_state.phase_correlation.load(Ordering::Relaxed);
                self.balance = self.shared_state.balance.load(Ordering::Relaxed);
                let snap_now = self.shared_state.snap_active.load(Ordering::Relaxed);
                if snap_now { self.snap_blink = 72; }
                else if self.snap_blink == 1 {
                    // SNAP just completed — export to vault
                    if let Some(ref vp) = self.vault_path {
                        if !vp.is_empty() {
                            let stereo = self.shared_state.snap_stereo_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                            let mono = self.shared_state.snap_mono_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                            let delta = self.shared_state.snap_delta_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                            let sr = self.shared_state.sample_rate.load(Ordering::Relaxed);
                            let band_levels = [-90.0f32; 5];
                            let md = snap_markdown(&stereo, &mono, &delta, band_levels, self.phase_correlation, self.peak_l, self.peak_r, sr);
                            let fname = snap_filename(vp);
                            let _ = std::fs::write(std::path::Path::new(vp).join(&fname), &md);
                        }
                    }
                }
                if self.snap_blink > 0 { self.snap_blink -= 1; }
            }
            LucentMsg::RelayToggled(idx) => {
                self.ui_state.toggle_relay(idx);
            }
            LucentMsg::SnapPressed => {
                self.shared_state.snap_active.store(true, Ordering::Relaxed);
                self.snap_blink = 72;
            }
            LucentMsg::CycleMode => {
                let current = params.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
                let next: i64 = match current { 1 => 0, 0 => 2, _ => 1 };
                ctx.begin_edit(LucentParamsParamId::AnalyzeMode);
                ctx.set_param(LucentParamsParamId::AnalyzeMode, next as f64 / 2.0);
                ctx.end_edit(LucentParamsParamId::AnalyzeMode);
            }
            LucentMsg::LucentNameChanged(name) => {
                if let Ok(mut n) = self.params.name.write() { *n = name.clone(); }
                self.ui_state.name = name;
            }
            LucentMsg::ResetPeak => {
                self.shared_state.reset_peak.store(true, Ordering::Relaxed);
                self.shared_state.peak_hold.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_l.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_r.store(-100.0, Ordering::Relaxed);
            }
            LucentMsg::SetupToggled => {
                self.show_setup = !self.show_setup;
                if self.show_setup {
                    self.vault_path_input = self.vault_path.clone().unwrap_or_default();
                }
            }
            LucentMsg::VaultPathChanged(path) => {
                self.vault_path_input = path;
            }
            LucentMsg::SaveVaultPath => {
                let new_path = if self.vault_path_input.trim().is_empty() {
                    None
                } else {
                    Some(self.vault_path_input.trim().to_string())
                };
                self.vault_path = new_path.clone();
                let config = shared_analysis::PluginConfig { vault_path: new_path, ..Default::default() };
                let _ = shared_analysis::save_config("Lucent", &config);
                self.show_setup = false;
            }
            LucentMsg::ResonanceToggled => {
                self.show_resonance = !self.show_resonance;
            }
            LucentMsg::MaskingToggled => {
                self.show_masking = !self.show_masking;
            }
            LucentMsg::ResetAll => {
                self.shared_state.reset_peak.store(true, Ordering::Relaxed);
                self.shared_state.peak_hold.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_l.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_r.store(-100.0, Ordering::Relaxed);
            }
        }

        truce_iced::iced::Task::none()
    }

    fn view<'a>(
        &'a self,
        params: &'a ParamCache<crate::LucentParams>,
    ) -> Element<'a, Message<LucentMsg>> {
        // ── HEADER ──────────────────────────────────────────────────────────────
        let header = container(
            row![
                container(header_brand("Lucent", VERSION)).width(Length::Shrink),
                Space::new().width(Length::Fill),
                container(
                    text_input("Name", &self.ui_state.name)
                        .on_input(|s| Message::Plugin(LucentMsg::LucentNameChanged(s)))
                        .padding(4)
                        .size(11)
                ).width(Length::Fixed(130.0)).center_y(Length::Fill),
                Space::new().width(Length::Fill),
                {
                    let mode = params.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
                    let label = match mode { 0 => "STANDALONE", 2 => "RELAY", _ => "HYBRID" };
                    container(
                        row![
                            button(
                                Text::new(label).size(11).font(bold_font())
                                    .width(Length::Fill).align_x(Alignment::Center)
                            )
                            .on_press(Message::Plugin(LucentMsg::CycleMode))
                            .padding([5, 8])
                            .width(Length::Fixed(110.0))
                            .style(|_theme, status| {
                                let bg = if status == button::Status::Hovered {
                                    Color::from_rgb(0.25, 0.15, 0.05)
                                } else {
                                    Color::from_rgb(0.15, 0.1, 0.05)
                                };
                                button::Style {
                                    background: Some(bg.into()),
                                    text_color: Color::from_rgb(1.0, 0.55, 0.1),
                                    border: Border { radius: 2.0.into(), ..Default::default() },
                                    ..Default::default()
                                }
                            }),
                            output_tools_strip(Message::Plugin(LucentMsg::ResetAll)),
                        ].spacing(6).align_y(Alignment::Center)
                    )
                },
            ]
            .align_y(Alignment::Center)
            .spacing(8)
            .padding(8)
        )
        .width(Length::Fill)
        .height(Length::Fixed(50.0))
        .style(|_theme| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── LEFT SIDEBAR ─────────────────────────────────────────────────────────
        let snap_blink = self.snap_blink > 0;
        let snap_label = if snap_blink { "ANALYZING..." } else { "SNAP" };

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
                text_color: if snap_blink { Color::from_rgb(1.0, 0.85, 0.3) } else { Color::from_rgb(1.0, 0.55, 0.1) },
                border: Border { color: border_col, width: 1.0, radius: 3.0.into() },
                ..Default::default()
            }
        };
        let vault_btn_style = |_theme: &truce_iced::iced::Theme, status: button::Status| {
            let bg = if status == button::Status::Hovered {
                Color::from_rgb(0.25, 0.25, 0.25)
            } else {
                Color::from_rgb(0.18, 0.18, 0.18)
            };
            button::Style {
                background: Some(bg.into()),
                text_color: Color::WHITE,
                border: Border { color: Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: 3.0.into() },
                ..Default::default()
            }
        };
        let btn_h = Length::Fixed(34.0);
        let btn_pad = Padding { top: 7.0, bottom: 1.0, left: 0.0, right: 0.0 };

        let sidebar: Element<Message<LucentMsg>> = container(
            column![
                Text::new("LX AUDIOLABS").font(bold_font()).size(14).color(Color::WHITE),
                button(
                    Text::new(snap_label).font(bold_font()).size(12)
                        .width(Length::Fill).align_x(Alignment::Center)
                )
                .on_press(Message::Plugin(LucentMsg::SnapPressed))
                .style(snap_btn_style).width(Length::Fill).height(btn_h).padding(btn_pad),
                button(
                    Text::new("VAULT SETUP").font(bold_font()).size(12)
                        .width(Length::Fill).align_x(Alignment::Center)
                )
                .on_press(Message::Plugin(LucentMsg::SetupToggled))
                .style(vault_btn_style).width(Length::Fill).height(btn_h).padding(btn_pad),
            ]
            .spacing(10)
        )
        .width(Length::Fixed(180.0))
        .height(Length::Fill)
        .padding(10)
        .style(|_theme| container::Style {
            background: Some(Color::from_rgb(0.09, 0.09, 0.09).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        })
        .into();

        // ── RIGHT SIDEBAR ─────────────────────────────────────────────────────────
        let right_title = container(
            Text::new("OUTPUT LEVEL").size(12).font(bold_font())
                .color(Color::from_rgb(0.75, 0.75, 0.75))
        )
        .width(Length::Fill)
        .padding(Padding { top: 2.0, right: 0.0, bottom: 4.0, left: 0.0 });

        let scope_write_pos = self.shared_state.scope_write_pos.load(Ordering::Acquire);
        let goniometer = canvas(GoniometerCanvas {
            samples: self.shared_state.scope_samples.clone(),
            write_pos: scope_write_pos,
            correlation: self.phase_correlation,
        })
        .width(Length::Fill)
        .height(Length::Fixed(139.0));

        let output_block = output_level_block(
            self.peak_l, self.peak_r, self.peak_hold_l, self.peak_hold_r,
            self.peak_hold, Message::Plugin(LucentMsg::ResetPeak),
            self.balance,
            Length::Fill,
        );

        let right_sidebar = container(
            column![
                right_title,
                output_block,
                Text::new("GONIOMETER").size(10).font(bold_font())
                    .color(Color::from_rgb(0.6, 0.6, 0.6)),
                container(goniometer).width(Length::Fill).height(Length::Fixed(139.0)),
            ]
            .spacing(6)
        )
        .width(Length::Fixed(155.0))
        .height(Length::Fill)
        .padding(8)
        .style(|_theme| container::Style {
            background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()),
            border: Border { color: Color::from_rgb(0.18, 0.18, 0.18), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── CENTER ────────────────────────────────────────────────────────────────
        let mode = params.get_plain(LucentParamsParamId::AnalyzeMode) as i64;
        let center: Element<Message<LucentMsg>> = if self.show_setup {
            let setup_box = vault_setup_box(
                "Lucent",
                &self.vault_path_input,
                |s| Message::Plugin(LucentMsg::VaultPathChanged(s)),
                Message::Plugin(LucentMsg::SaveVaultPath),
                Message::Plugin(LucentMsg::SetupToggled),
            );
            container(setup_box)
                .width(Length::Fill).height(Length::Fill)
                .center_x(Length::Fill).center_y(Length::Fill)
                .style(|_theme| container::Style {
                    background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
                    ..Default::default()
                })
                .into()
        } else {
            let resonance_text = self.resonance_summary();
            let masking_text = self.masking_summary(mode);
            let show_res = self.show_resonance;
            let show_mask = self.show_masking;

            let analyzer_row = container(
                row![
                    container(
                        row![
                            column![
                                Text::new("RESONANCE").size(10).font(bold_font())
                                    .color(Color::from_rgb(1.0, 0.55, 0.15)),
                                Text::new(resonance_text).size(10)
                                    .color(Color::from_rgb(0.8, 0.8, 0.8)),
                            ].spacing(2).width(Length::Fill),
                            toggle_button(
                                if show_res { "ON" } else { "OFF" },
                                show_res,
                                Message::Plugin(LucentMsg::ResonanceToggled),
                            ),
                        ].spacing(4).align_y(Alignment::Center)
                    )
                    .width(Length::FillPortion(1)).height(Length::Fixed(80.0))
                    .padding(6).style(|_theme| panel_bg()),

                    container(
                        row![
                            column![
                                Text::new("MASKING").size(10).font(bold_font())
                                    .color(Color::from_rgb(0.95, 0.22, 0.18)),
                                Text::new(masking_text).size(10)
                                    .color(Color::from_rgb(0.8, 0.8, 0.8)),
                            ].spacing(2).width(Length::Fill),
                            toggle_button(
                                if show_mask { "ON" } else { "OFF" },
                                show_mask,
                                Message::Plugin(LucentMsg::MaskingToggled),
                            ),
                        ].spacing(4).align_y(Alignment::Center)
                    )
                    .width(Length::FillPortion(1)).height(Length::Fixed(80.0))
                    .padding(6).style(|_theme| panel_bg()),
                ]
                .spacing(6)
            )
            .width(Length::Fill);

            container(
                column![
                    self.render_main_panel(mode),
                    analyzer_row,
                ]
                .spacing(6)
            )
            .width(Length::Fill).height(Length::Fill).padding(6)
            .style(|_theme| container::Style {
                background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
                ..Default::default()
            })
            .into()
        };

        let main_body = row![
            sidebar,
            container(center).width(Length::Fill).height(Length::Fill),
            right_sidebar,
        ]
        .height(Length::Fill);

        container(
            column![header, main_body]
                .width(Length::Fill).height(Length::Fill)
        )
        .width(Length::Fill).height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Color::from_rgb(0.06, 0.06, 0.06).into()),
            ..Default::default()
        })
        .into()
    }
}

impl LucentEditor {
    fn render_main_panel(&self, mode: i64) -> Element<'_, Message<LucentMsg>> {
        let curves = if self.ui_state.relays.is_empty() || mode == 0 {
            vec![SpectrumCurve {
                spectrum: self.ui_state.own_spectrum.clone(),
                color: Color::from_rgb(0.1, 0.9, 0.7),
                fill_alpha: 0.18,
                line_alpha: 0.85,
                line_width: 1.2,
            }]
        } else {
            let relay_colors = [
                Color::from_rgb(1.0, 0.6, 0.2),
                Color::from_rgb(0.8, 0.3, 0.3),
                Color::from_rgb(0.3, 0.8, 0.5),
                Color::from_rgb(0.4, 0.6, 1.0),
                Color::from_rgb(0.9, 0.7, 0.3),
                Color::from_rgb(0.7, 0.4, 0.8),
            ];
            let mut curves_vec = vec![SpectrumCurve {
                spectrum: self.ui_state.own_spectrum.clone(),
                color: Color::from_rgb(0.1, 0.9, 0.7),
                fill_alpha: 0.12,
                line_alpha: 0.6,
                line_width: 1.0,
            }];
            for (idx, relay) in self.ui_state.active_relays().iter().enumerate() {
                curves_vec.push(SpectrumCurve {
                    spectrum: relay.spectrum.clone(),
                    color: relay_colors[idx % relay_colors.len()],
                    fill_alpha: 0.08,
                    line_alpha: 0.5,
                    line_width: 0.8,
                });
            }
            curves_vec
        };

        let config = SpectrumConfig {
            sample_rate: self.shared_state.sample_rate.load(Ordering::Relaxed),
            ..Default::default()
        };
        let resonance_peaks = if self.show_resonance { self.resonance_cache.clone() } else { Vec::new() };
        let masking = if self.show_masking && (!self.ui_state.relays.is_empty() || mode == 2) {
            self.masking_cache.clone()
        } else {
            Vec::new()
        };

        let fft_canvas = canvas(SpectrumCanvas {
            curves, config, eq_overlay: None, resonance_peaks, masking,
        })
        .width(Length::Fill)
        .height(Length::Fill);

        const MAX_VISIBLE_RELAYS: usize = 6;
        let mut relay_row = row![
            Text::new("RELAYS").size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        if self.ui_state.relays.is_empty() {
            relay_row = relay_row.push(
                Text::new("— send a relay from another LX plugin —")
                    .size(10).font(bold_font()).color(Color::from_rgb(0.4, 0.4, 0.4)),
            );
        } else {
            for (idx, relay) in self.ui_state.relays.iter().take(MAX_VISIBLE_RELAYS).enumerate() {
                relay_row = relay_row.push(toggle_button(
                    relay.name.as_str(),
                    relay.active,
                    Message::Plugin(LucentMsg::RelayToggled(idx)),
                ));
            }
        }

        let relay_bar = if mode == 0 {
            container(row![]).width(Length::Fill).height(Length::Fixed(0.0))
        } else {
            container(relay_row)
                .width(Length::Fill).height(Length::Fixed(48.0))
                .padding(Padding::new(8.0))
                .style(|_theme| container::Style {
                    background: Some(Color::from_rgb(0.09, 0.09, 0.09).into()),
                    border: Border {
                        color: Color::from_rgb(0.15, 0.15, 0.15),
                        width: 1.0,
                        ..Default::default()
                    },
                    ..Default::default()
                })
        };

        column![
            relay_bar,
            container(fft_canvas).width(Length::Fill).height(Length::Fill),
        ]
        .spacing(6.0)
        .into()
    }

    fn resonance_summary(&self) -> String {
        let peaks = &self.resonance_cache;
        if peaks.is_empty() { return "No resonances detected".to_string(); }
        let sr = self.shared_state.sample_rate.load(Ordering::Relaxed).max(1.0);
        let fft_size = (SPECTRUM_BINS * 2) as f32;
        peaks.iter().take(3)
            .map(|(bin, score)| {
                let freq = *bin as f32 * sr / fft_size;
                format!("{:.0} Hz  {:.1}", freq, score)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn masking_summary(&self, mode: i64) -> String {
        if mode == 0 { return "Standalone — no masking".to_string(); }
        if self.masking_cache.is_empty() || self.ui_state.relays.is_empty() {
            return "No masking detected".to_string();
        }
        let sr = self.shared_state.sample_rate.load(Ordering::Relaxed).max(1.0);
        let fft_size = (SPECTRUM_BINS * 2) as f32;
        let mut peaks: Vec<(usize, f32)> = self.masking_cache.iter()
            .enumerate()
            .filter(|(_, db)| **db > -70.0)
            .map(|(i, &db)| (i, db))
            .collect();
        peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if peaks.is_empty() { return "No masking detected".to_string(); }
        peaks.iter().take(3)
            .map(|(bin, db)| {
                let freq = *bin as f32 * sr / fft_size;
                format!("{:.0} Hz  {:.1} dB", freq, db)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn panel_bg() -> container::Style {
    container::Style {
        background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()),
        border: Border { color: Color::from_rgb(0.18, 0.18, 0.18), width: 1.0, radius: 3.0.into() },
        ..Default::default()
    }
}

// ─── SNAP Helpers ────────────────────────────────────────────────────────────

fn snap_filename(vault_path: &str) -> String {
    let dir = std::path::Path::new(vault_path);
    let mut max_n = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let s = e.file_name().to_string_lossy().into_owned();
            if let Some(inner) = s.strip_prefix("SNAPSHOT-").and_then(|r| r.strip_suffix(".md")) {
                if let Ok(n) = inner.parse::<u32>() { max_n = max_n.max(n); }
            }
        }
    }
    format!("SNAPSHOT-{:03}.md", max_n + 1)
}

fn snap_markdown(stereo: &[f32], mono: &[f32], delta: &[f32],
    _band_levels: [f32; 5], corr: f32, pl: f32, pr: f32, sr: f32) -> String
{
    let fft_sz = 2048.0;
    let freqs: &[f32] = &[20.0, 40.0, 80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10000.0, 16000.0, 20000.0];
    let tbl = |s: &[f32]| {
        freqs.iter().map(|&f| {
            let bin = ((f * fft_sz / sr) as usize).min(s.len().saturating_sub(1));
            format!("| {} | {:.1} |", if f >= 1000.0 { format!("{:.0}k", f/1000.0) } else { format!("{:.0}", f) }, s[bin])
        }).collect::<Vec<_>>().join("\n")
    };
    format!(
        "---\nplugin: lucent\ntype: snapshot\n---\n\n# Lucent Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n",
        pl=pl, pr=pr, co=corr, st=tbl(stereo), mn=tbl(mono), dt=tbl(delta),
    )
}
