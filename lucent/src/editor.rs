use nice_plug_iced::iced::widget::{button, column, container, row, canvas, text_input, Text, Space};
use nice_plug_iced::iced::{Color, Element, Length, Alignment, Background, Border, Padding, widget::canvas::Cache};
use nice_plug::editor::Editor;
use nice_plug_iced::NiceGuiContext;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use shared_analysis::{SharedState, SPECTRUM_BINS};
use crate::resonance_hub;
use shared_ui::{
    bold_font, header_brand, output_level_block, output_tools_strip,
    toggle_button, vault_setup_box,
    LxEditorApp, create_lx_editor, GoniometerCanvas,
    SpectrumCanvas, SpectrumCurve, SpectrumConfig,
};
use crate::ui::LucentUiState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub enum Message {
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
    nice_ctx: NiceGuiContext,
    ui_state: LucentUiState,
    /// Cached resonance peaks from DSP for LISTEN visualization
    resonance_cache: Vec<(usize, f32)>,
    /// Cached masking collision map (dB per bin) from DSP for the red overlay
    masking_cache: Vec<f32>,
    show_resonance: bool,
    show_masking: bool,
    /// UI-driven blink countdown for the SNAP button (ticks). Set on press so the
    /// amber SNAP pulses visibly while it analyses + saves (DSP itself is ~3 frames).
    snap_blink: u32,
    // Vault path config
    vault_path: Option<String>,
    show_setup: bool,
    vault_path_input: String,
    // Cached meter values
    output_peak: f32,
    peak_hold: f32,
    peak_l: f32,
    peak_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    phase_correlation: f32,
    balance: f32,
    // Canvas caches — must clear on Tick to force redraw
    fft_cache: Cache,
    output_meter_cache: Cache,
}

impl LxEditorApp for LucentEditor {
    type Message = Message;
    type Params = Arc<crate::LucentParams>;
    type State = Arc<SharedState>;

    fn boot(
        nice_ctx: NiceGuiContext,
        params: Self::Params,
        shared_state: Self::State,
    ) -> (Self, nice_plug_iced::iced::Task<Message>) {
        let config = shared_analysis::load_config("Lucent");
        let mut ui_state = LucentUiState::new();
        // Restore the persisted instance name into the editor's text buffer.
        if let Ok(name) = params.name.read() {
            if !name.is_empty() {
                ui_state.name = name.clone();
            }
        }
        let editor = Self {
            params,
            shared_state,
            nice_ctx,
            ui_state,
            resonance_cache: Vec::new(),
            masking_cache: Vec::new(),
            show_resonance: false,
            show_masking: false,
            snap_blink: 0,
            vault_path: config.vault_path.clone(),
            show_setup: false,
            vault_path_input: config.vault_path.clone().unwrap_or_default(),
            output_peak: -90.0,
            peak_hold: -90.0,
            peak_l: -90.0,
            peak_r: -90.0,
            peak_hold_l: -90.0,
            peak_hold_r: -90.0,
            phase_correlation: 1.0,
            balance: 0.0,
            fft_cache: Cache::new(),
            output_meter_cache: Cache::new(),
        };
        (editor, nice_plug_iced::iced::Task::none())
    }

    fn update(&mut self, msg: Message) -> nice_plug_iced::iced::Task<Message> {
        match msg {
            Message::Tick => {
                if let Ok(spectrum) = self.shared_state.spectrum_avg.lock() {
                    self.ui_state.own_spectrum = spectrum.to_vec();
                }
                // Read live resonance peaks from DSP for LISTEN visualization
                if let Ok(peaks) = resonance_hub().lock() {
                    self.resonance_cache = peaks.clone();
                }
                // Read masking collision map from DSP for the red overlay
                if let Ok(mm) = self.shared_state.masking_map.lock() {
                    self.masking_cache = mm.to_vec();
                }
                // Pull live relay feeds from the shared-memory hub (cross-plugin IPC).
                // This is what makes relay curves + the relay toggle row appear.
                // Mode 0 (Standalone): skip — only own spectrum, no relay interaction.
                // Mode 1 (Hybrid) / 2 (Relay): read relays for masking + display.
                let mode = self.params.analyze_mode.value();
                if mode != 0 {
                    let now_ms = shared_analysis::shm::now_ms();
                    // Match what the audio thread + liveness thread advertise: the
                    // effective name (fallback "Lucent N" when unnamed) derived from
                    // params.name — NOT ui_state.name, whose default differs and caused
                    // the relay's target ("Lucent 1") to miss the filter ("Lucent").
                    let slot = self.shared_state.shm_slot.load(Ordering::Acquire);
                    let raw = self.params.name.try_read()
                        .map(|n| n.clone())
                        .unwrap_or_else(|_| self.ui_state.name.clone());
                    let my_name = if slot >= 0 {
                        shared_analysis::shm::lucent_display_name(&raw, slot as u8)
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
                // Update peak meters
                self.output_peak = self.shared_state.output_peak.load(Ordering::Relaxed);
                self.peak_hold = self.shared_state.peak_hold.load(Ordering::Relaxed);
                self.peak_l = self.shared_state.output_peak_l.load(Ordering::Relaxed);
                self.peak_r = self.shared_state.output_peak_r.load(Ordering::Relaxed);
                self.peak_hold_l = self.shared_state.peak_hold_l.load(Ordering::Relaxed);
                self.peak_hold_r = self.shared_state.peak_hold_r.load(Ordering::Relaxed);
                self.phase_correlation = self.shared_state.phase_correlation.load(Ordering::Relaxed);
                self.balance = self.shared_state.balance.load(Ordering::Relaxed);

                // SNAP blink countdown (UI feedback while analysing + saving)
                if self.snap_blink > 0 {
                    self.snap_blink -= 1;
                }

                // CRITICAL: Clear canvas caches to force redraw on each Tick
                // Without this, UI only updates when parameters change
                self.fft_cache.clear();
                self.output_meter_cache.clear();
            }
            Message::RelayToggled(idx) => {
                self.ui_state.toggle_relay(idx);
            }
            Message::SnapPressed => {
                self.shared_state.snap_active.store(true, Ordering::Relaxed);
                self.snap_blink = 72; // ~1.2 s of visible pulse at 60 fps
            }
            Message::CycleMode => {
                // Cycle: 1 (Hybrid) → 0 (Standalone) → 2 (Relay) → 1 ...
                let next = match self.params.analyze_mode.value() {
                    1 => 0,
                    0 => 2,
                    _ => 1,
                };
                let setter = self.nice_ctx.param_setter();
                setter.begin_set_parameter(&self.params.analyze_mode);
                setter.set_parameter(&self.params.analyze_mode, next);
                setter.end_set_parameter(&self.params.analyze_mode);
            }
            Message::LucentNameChanged(name) => {
                // Persist through the param lock so the name survives reload and
                // the audio thread can publish it to the relay hub.
                if let Ok(mut n) = self.params.name.write() {
                    *n = name.clone();
                }
                self.ui_state.name = name;
            }
            Message::ResetPeak => {
                self.shared_state.reset_peak.store(true, Ordering::Relaxed);
                self.shared_state.peak_hold.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_l.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_r.store(-100.0, Ordering::Relaxed);
            }
            Message::SetupToggled => {
                self.show_setup = !self.show_setup;
                if self.show_setup {
                    self.vault_path_input = self.vault_path.clone().unwrap_or_default();
                }
            }
            Message::VaultPathChanged(path) => {
                self.vault_path_input = path;
            }
            Message::SaveVaultPath => {
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
            Message::ResonanceToggled => {
                self.show_resonance = !self.show_resonance;
            }
            Message::MaskingToggled => {
                self.show_masking = !self.show_masking;
            }
            Message::ResetAll => {
                // Clear peak holds.
                self.shared_state.reset_peak.store(true, Ordering::Relaxed);
                self.shared_state.peak_hold.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_l.store(-100.0, Ordering::Relaxed);
                self.shared_state.peak_hold_r.store(-100.0, Ordering::Relaxed);
            }
        }
        nice_plug_iced::iced::Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        // ── HEADER (50px): brand + Name input (relay-target name) + Δ/◉ ──
        let header = container(
            row![
                container(header_brand("Lucent", VERSION)).width(Length::Shrink),
                Space::new().width(Length::Fill),
                // Name field — fixed position, won't jump when mode button changes
                container(
                    text_input("Name", &self.ui_state.name)
                        .on_input(Message::LucentNameChanged)
                        .padding(4)
                        .size(11)
                ).width(Length::Fixed(130.0)).center_y(Length::Fill),
                Space::new().width(Length::Fill),
                // Mode cycle button + RESET — fixed width so STANDALONE fits
                {
                    let mode = self.params.analyze_mode.value();
                    let label = match mode { 0 => "STANDALONE", 2 => "RELAY", _ => "HYBRID" };
                    container(
                        row![
                            button(Text::new(label).size(11).font(bold_font()).width(Length::Fill).align_x(Alignment::Center))
                                .on_press(Message::CycleMode)
                                .padding([5, 8])
                                .width(Length::Fixed(110.0))
                                .style(|_theme, status| {
                                    let bg = if status == button::Status::Hovered { Color::from_rgb(0.25, 0.15, 0.05) } else { Color::from_rgb(0.15, 0.1, 0.05) };
                                    button::Style { background: Some(Background::Color(bg)), text_color: Color::from_rgb(1.0, 0.55, 0.1), border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default() }
                                }),
                            output_tools_strip(Message::ResetAll),
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
            background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── LEFT SIDEBAR: SNAP + VAULT SETUP (180px, no presets — Lucent is pure analyzer) ──
        let snap_blink = self.snap_blink > 0;
        let snap_label = if snap_blink { "ANALYZING..." } else { "SNAP" };

        let snap_btn_style = move |_theme: &nice_plug_iced::iced::Theme, status: button::Status| {
            let (bg, border_col) = if snap_blink {
                (Color::from_rgb(0.55, 0.38, 0.05), Color::from_rgb(0.8, 0.55, 0.1))
            } else if status == button::Status::Hovered {
                (Color::from_rgb(0.25, 0.25, 0.25), Color::from_rgb(0.3, 0.3, 0.3))
            } else {
                (Color::from_rgb(0.18, 0.18, 0.18), Color::from_rgb(0.3, 0.3, 0.3))
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: if snap_blink { Color::from_rgb(1.0, 0.85, 0.3) } else { Color::from_rgb(1.0, 0.55, 0.1) },
                border: Border { color: border_col, width: 1.0, radius: 3.0.into() },
                ..Default::default()
            }
        };
        let vault_btn_style = |_theme: &nice_plug_iced::iced::Theme, status: button::Status| {
            let bg = if status == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.18, 0.18, 0.18) };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border: Border { color: Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: 3.0.into() },
                ..Default::default()
            }
        };
        let btn_h = Length::Fixed(34.0);
        let btn_pad = Padding { top: 7.0, bottom: 1.0, left: 0.0, right: 0.0 };

        let sidebar: Element<Message> = container(
            column![
                Text::new("LX AUDIOLABS").font(bold_font()).size(14).color(Color::WHITE),
                button(Text::new(snap_label).font(bold_font()).size(12).width(Length::Fill).align_x(Alignment::Center))
                    .on_press(Message::SnapPressed).style(snap_btn_style).width(Length::Fill).height(btn_h).padding(btn_pad),
                button(Text::new("VAULT SETUP").font(bold_font()).size(12).width(Length::Fill).align_x(Alignment::Center))
                    .on_press(Message::SetupToggled).style(vault_btn_style).width(Length::Fill).height(btn_h).padding(btn_pad),
            ]
            .spacing(10)
        )
        .width(Length::Fixed(180.0))
        .height(Length::Fill)
        .padding(10)
        .style(|_theme| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.09, 0.09, 0.09))),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        })
        .into();

        // ── RIGHT SIDEBAR (155px): OUT knob + AUTO LOUD, OUTPUT block, GONIOMETER ──
        let right_title = container(Text::new("OUTPUT LEVEL").size(12).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)))
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
            self.peak_hold, Message::ResetPeak,
            self.balance,
            Length::Fill,
        );

        let right_sidebar = container(
            column![
                right_title,
                output_block,
                Text::new("GONIOMETER").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
                container(goniometer).width(Length::Fill).height(Length::Fixed(139.0)),
            ]
            .spacing(6)
        )
        .width(Length::Fixed(155.0))
        .height(Length::Fill)
        .padding(8)
        .style(|_theme| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.1, 0.1, 0.1))),
            border: Border { color: Color::from_rgb(0.18, 0.18, 0.18), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── CENTER: RELAYS (top) · FFT (fill) · ANALYZER (bottom) ── or SETUP screen ──
        let center: Element<Message> = if self.show_setup {
            let setup_box = vault_setup_box(
                "Lucent",
                &self.vault_path_input,
                Message::VaultPathChanged,
                Message::SaveVaultPath,
                Message::SetupToggled,
            );

            container(setup_box)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_theme| container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
                    ..Default::default()
                })
                .into()
        } else {
            // Analyzer panels row: RESONANCE (with toggle) | MASKING (with toggle)
            let resonance_text = self.resonance_summary();
            let masking_text = self.masking_summary();
            let show_res = self.show_resonance;
            let show_mask = self.show_masking;

            let analyzer_row = container(
                row![
                    // RESONANCE panel
                    container(
                        row![
                            column![
                                Text::new("RESONANCE").size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)),
                                Text::new(resonance_text).size(10).color(Color::from_rgb(0.8, 0.8, 0.8)),
                            ].spacing(2).width(Length::Fill),
                            toggle_button(
                                if show_res { "ON" } else { "OFF" },
                                show_res,
                                Message::ResonanceToggled,
                            ),
                        ].spacing(4).align_y(Alignment::Center)
                    )
                    .width(Length::FillPortion(1))
                    .height(Length::Fixed(80.0))
                    .padding(6)
                    .style(|_theme| panel_bg()),

                    // MASKING panel
                    container(
                        row![
                            column![
                                Text::new("MASKING").size(10).font(bold_font()).color(Color::from_rgb(0.95, 0.22, 0.18)),
                                Text::new(masking_text).size(10).color(Color::from_rgb(0.8, 0.8, 0.8)),
                            ].spacing(2).width(Length::Fill),
                            toggle_button(
                                if show_mask { "ON" } else { "OFF" },
                                show_mask,
                                Message::MaskingToggled,
                            ),
                        ].spacing(4).align_y(Alignment::Center)
                    )
                    .width(Length::FillPortion(1))
                    .height(Length::Fixed(80.0))
                    .padding(6)
                    .style(|_theme| panel_bg()),
                ]
                .spacing(6)
            )
            .width(Length::Fill);

            container(
                column![
                    self.render_main_panel(),
                    analyzer_row,
                ]
                .spacing(6)
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(6)
            .style(|_theme| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
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

        // ── ASSEMBLE: header / main_body (no footer) ──
        container(
            column![
                header,
                main_body,
            ]
            .width(Length::Fill)
            .height(Length::Fill)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.06, 0.06, 0.06))),
            ..Default::default()
        })
        .into()
    }

    fn tick() -> Message {
        Message::Tick
    }
}

impl LucentEditor {
    fn render_main_panel(&self) -> Element<'_, Message> {
        let mode = self.params.analyze_mode.value();
        let curves = if self.ui_state.relays.is_empty() || mode == 0 {
            vec![
                SpectrumCurve {
                    spectrum: self.ui_state.own_spectrum.clone(),
                    color: Color::from_rgb(0.1, 0.9, 0.7),
                    fill_alpha: 0.18,
                    line_alpha: 0.85,
                    line_width: 1.2,
                }
            ]
        } else {
            let mut curves_vec = vec![];

            curves_vec.push(SpectrumCurve {
                spectrum: self.ui_state.own_spectrum.clone(),
                color: Color::from_rgb(0.1, 0.9, 0.7),
                fill_alpha: 0.12,
                line_alpha: 0.6,
                line_width: 1.0,
            });

            let relay_colors = [
                Color::from_rgb(1.0, 0.6, 0.2),
                Color::from_rgb(0.8, 0.3, 0.3),
                Color::from_rgb(0.3, 0.8, 0.5),
                Color::from_rgb(0.4, 0.6, 1.0),
                Color::from_rgb(0.9, 0.7, 0.3),
                Color::from_rgb(0.7, 0.4, 0.8),
            ];

            for (idx, relay) in self.ui_state.active_relays().iter().enumerate() {
                let color = relay_colors[idx % relay_colors.len()];
                curves_vec.push(SpectrumCurve {
                    spectrum: relay.spectrum.clone(),
                    color,
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
        let resonance_peaks = if self.show_resonance {
            self.resonance_cache.clone()
        } else {
            Vec::new()
        };

        let masking = if self.show_masking && !self.ui_state.relays.is_empty() {
            self.masking_cache.clone()
        } else if self.show_masking && mode == 2 {
            // Relay mode: masking may have been computed even if UI relays
            // haven't synced yet — show whatever the audio thread produced.
            self.masking_cache.clone()
        } else {
            Vec::new()
        };

        let fft_canvas = canvas(SpectrumCanvas {
            curves,
            config,
            eq_overlay: None,
            resonance_peaks,
            masking,
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
            // Placeholder: no relays connected yet (mirrors Meridian's band-section bar).
            relay_row = relay_row.push(
                Text::new("— send a relay from another LX plugin —")
                    .size(10)
                    .font(bold_font())
                    .color(Color::from_rgb(0.4, 0.4, 0.4)),
            );
        } else {
            for (idx, relay) in self.ui_state.relays.iter().take(MAX_VISIBLE_RELAYS).enumerate() {
                relay_row = relay_row.push(toggle_button(
                    relay.name.as_str(),
                    relay.active,
                    Message::RelayToggled(idx),
                ));
            }
        }

        let relay_bar = if mode == 0 {
            // Standalone: no relay row at all.
            container(row![]).width(Length::Fill).height(Length::Fixed(0.0))
        } else {
            container(relay_row)
            .width(Length::Fill)
            .height(Length::Fixed(48.0))
            .padding(Padding::new(8.0))
            .style(|_theme| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.09, 0.09, 0.09))),
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

    /// Format top 3 resonance peaks as text lines.
    fn resonance_summary(&self) -> String {
        let peaks = &self.resonance_cache;
        if peaks.is_empty() {
            return "No resonances detected".to_string();
        }
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

    /// Format top 3 masking conflicts as text lines.
    fn masking_summary(&self) -> String {
        let mode = self.params.analyze_mode.value();
        if mode == 0 {
            return "Standalone — no masking".to_string();
        }
        if self.masking_cache.is_empty() || self.ui_state.relays.is_empty() {
            return "No masking detected".to_string();
        }
        let sr = self.shared_state.sample_rate.load(Ordering::Relaxed).max(1.0);
        let fft_size = (SPECTRUM_BINS * 2) as f32;
        // Find top 3 bins with highest masking dB
        let mut peaks: Vec<(usize, f32)> = self.masking_cache.iter()
            .enumerate()
            .filter(|(_, &db)| db > -70.0)
            .map(|(i, &db)| (i, db))
            .collect();
        peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if peaks.is_empty() {
            return "No masking detected".to_string();
        }
        peaks.iter().take(3)
            .map(|(bin, db)| {
                let freq = *bin as f32 * sr / fft_size;
                format!("{:.0} Hz  {:.1} dB", freq, db)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Shared panel background style for analyzer cards.
fn panel_bg() -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb(0.1, 0.1, 0.1))),
        border: Border { color: Color::from_rgb(0.18, 0.18, 0.18), width: 1.0, radius: 3.0.into() },
        ..Default::default()
    }
}

pub fn create(
    params: Arc<crate::LucentParams>,
    shared_state: Arc<SharedState>,
) -> Option<Box<dyn Editor>> {
    create_lx_editor::<LucentEditor>(
        params.editor_state.clone(),
        params,
        shared_state,
        true,
    )
}
