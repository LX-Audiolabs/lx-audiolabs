// Equilibrium editor — Iced UI truce port.
//
// Layout (990×660):
//   Header : brand + monitor strip (MONO/DELTA/BYPASS)
//   Left   : preset panel (SNAP, Vault, preset list)
//   Center : SpectrumCanvas + 5 band columns (Gain/Width/Pan/Solo)
//   Right  : output gain, pre-master, auto-loud, output meter, goniometer
//   Footer : LISTEN/APPLY/RESET buttons, Mono Floor knob, RESET ALL

use truce_iced::iced;
use truce_iced::iced::widget::{button, canvas, column, container, row, Space, Text};
use truce_iced::iced::widget::canvas::{Geometry, Path, Stroke};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Padding, Point, Rectangle, Size, Subscription};
use truce_iced::iced::mouse::Cursor;
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;
use truce::prelude::{FloatParam, BoolParam};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::path::PathBuf;

use shared_analysis::SharedState;
use shared_ui::{
    bold_font, header_brand, monitor_strip, vault_setup_box,
    ai_preset_panel, output_tools_strip, auto_loud_button, output_level_block,
    knob_gesture_bipolar, knob_gesture_suffixed, hslider_gesture,
    GoniometerCanvas, Gesture,
};

use crate::EquilibriumParams;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum EquilibriumMsg {
    Tick,
    GainGesture(usize, Gesture),
    WidthGesture(usize, Gesture),
    PanGesture(usize, Gesture),
    MonoFloorGesture(Gesture),
    OutputGainGesture(Gesture),
    SoloToggled(usize),
    MonoToggled,
    DeltaToggled,
    BypassToggled,
    ListenToggled,
    ApplyAnalysisAsTarget,
    ResetAnalysis,
    AutoLoudTriggered,
    ResetAll,
    SelectPreset(usize),
    PresetNameChanged(String),
    SavePreset,
    SetupToggled,
    VaultPathChanged(String),
    SaveVaultPath,
    ResetPeak,
    PreMasterToggled,
    PreMasterGesture(Gesture),
    SnapPressed,
}

// ─── Editor ──────────────────────────────────────────────────────────────────

pub struct EquilibriumEditor {
    params: Arc<EquilibriumParams>,
    shared_state: Arc<SharedState>,

    // Preset state
    vault_path: Option<String>,
    show_setup: bool,
    vault_path_input: String,
    preset_name_input: String,
    presets: Vec<(String, Option<PathBuf>, EqPreset)>,
    selected_preset_index: Option<usize>,
    preset_refresh_counter: u32,

    // Cached meter values
    band_levels: [f32; 5],
    target_levels: [f32; 5],
    target_tolerances: [f32; 5],
    listen_levels: [f32; 5],
    listen_tolerances: [f32; 5],
    listen_level_min: [f32; 5],
    listen_level_max: [f32; 5],
    listen_samples: f32,
    phase_correlation: f32,
    output_peak: f32,
    peak_hold: f32,
    peak_l: f32,
    peak_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    balance: f32,
    auto_loud_measuring: bool,
    snap_active: bool,
    snap_blink_counter: u32,
}

#[derive(Debug, Clone)]
pub struct EqPreset {
    pub name: String,
    pub bands: [f32; 5],
    pub tolerances: [f32; 5],
    pub pans: [f32; 5],
    pub widths: [f32; 5],
    pub mono_floor_hz: f32,
    pub output_gain: f32,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn format_pan(pan: f32) -> String {
    if pan.abs() < 0.01 { "C".into() }
    else if pan < 0.0 { format!("L {:.0}%", -pan * 100.0) }
    else { format!("R {:.0}%", pan * 100.0) }
}

fn vsep<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new())
        .width(Length::Fixed(1.0))
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.08).into()),
            ..Default::default()
        })
        .into()
}

fn load_presets(vault_path: Option<&str>) -> Vec<(String, Option<PathBuf>, EqPreset)> {
    let mut presets = vec![
        ("Pink Noise".to_string(), None, EqPreset {
            name: "Pink Noise".into(),
            bands: [3.0, 0.0, -3.0, -6.0, -9.0],
            tolerances: shared_analysis::DEFAULT_TOLERANCES,
            pans: [0.0; 5], widths: [100.0; 5],
            mono_floor_hz: 0.0, output_gain: 0.0,
        }),
    ];
    let custom = shared_analysis::list_custom_presets("Equilibrium", vault_path);
    for (name, path, profile) in custom {
        presets.push((
            name,
            Some(path),
            EqPreset {
                name: profile.name.clone(),
                bands: profile.bands,
                tolerances: profile.tolerances,
                pans: profile.pans,
                widths: profile.widths,
                mono_floor_hz: profile.mono_floor_hz,
                output_gain: 0.0,
            },
        ));
    }
    presets
}

// ─── SpectrumCanvas ──────────────────────────────────────────────────────────

pub struct EqSpectrumCanvas {
    pub band_levels: [f32; 5],
    pub target_levels: [f32; 5],
    pub target_tolerances: [f32; 5],
    pub listen_levels: [f32; 5],
    pub listen_tolerances: [f32; 5],
    pub listen_level_min: [f32; 5],
    pub listen_level_max: [f32; 5],
    pub listen_samples: f32,
}

impl<M> canvas::Program<M> for EqSpectrumCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &truce_iced::iced::Renderer,
        _theme: &truce_iced::iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let height = bounds.height;
        let col_width = width / 5.0;

        frame.fill(&Path::rectangle(Point::ORIGIN, bounds.size()), Color::from_rgb(0.08, 0.08, 0.08));

        // Pink noise tilt: +3 dB/octave compensation so pink noise appears flat.
        const TILT: [f32; 5] = [-3.0, 0.0, 3.0, 6.0, 9.0];

        // Silence detection uses RAW band levels (before clamping/tilt)
        let raw_band_avg: f32 = (0..5).map(|b| self.band_levels[b]).sum::<f32>() / 5.0;
        let is_silent = raw_band_avg <= -70.0;

        // Compute normalized averages for relative display
        let mut listen_sum = 0.0;
        let mut band_sum = 0.0;
        for (b, &tilt) in TILT.iter().enumerate() {
            listen_sum += self.listen_levels[b].max(-50.0) + tilt;
            band_sum  += self.band_levels[b].max(-50.0) + tilt;
        }
        let listen_avg = listen_sum / 5.0;
        let band_avg   = band_sum / 5.0;

        let min_db = -30.0f32;
        let max_db = 12.0f32;
        let db_range = max_db - min_db;

        let db_to_y = |db: f32| {
            let norm = ((db - min_db) / db_range).clamp(0.0, 1.0);
            height - (norm * height)
        };

        // Horizontal dB grid lines
        for &db in &[-30.0f32, -24.0, -18.0, -12.0, -6.0, 0.0, 6.0, 12.0] {
            let y = db_to_y(db);
            let is_major = db == -30.0 || db == -18.0 || db == -6.0 || db == 6.0;
            let alpha = if is_major { 0.20 } else { 0.10 };
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(width, y)),
                Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, alpha)).with_width(1.0),
            );
        }

        // Separators
        for i in 1..5 {
            let x = i as f32 * col_width;
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, height)),
                Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.05)).with_width(1.0),
            );
        }

        for b in 0..5 {
            let col_x = b as f32 * col_width;

            // Peak meter amber bar — relative to spectral average
            let bar_alpha = if self.listen_samples > 0.0 { 0.12 } else { 0.55 };
            if !is_silent {
                let peak_db_t = self.band_levels[b].max(-50.0) + TILT[b];
                let norm_band_db = peak_db_t - band_avg;
                let bar_top_y = db_to_y(norm_band_db);
                let bar_h = (height - bar_top_y).max(0.0);
                frame.fill(
                    &Path::rectangle(Point::new(col_x + 5.0, bar_top_y), Size::new(col_width - 10.0, bar_h)),
                    Color::from_rgba(1.0, 0.45, 0.1, bar_alpha),
                );
            }

            // Target Corridor & Line — hidden during Listen/Analyze
            if self.listen_samples <= 100.0 {
                let target_db = self.target_levels[b].max(-30.0) + TILT[b];
                let target_sum: f32 = (0..5).map(|i| self.target_levels[i].max(-30.0) + TILT[i]).sum();
                let target_avg = target_sum / 5.0;
                let norm_target_db = target_db - target_avg;
                let tolerance = self.target_tolerances[b];

                let target_y = db_to_y(norm_target_db);
                let upper_y = db_to_y(norm_target_db + tolerance);
                let lower_y = db_to_y(norm_target_db - tolerance);
                let corridor_h = (lower_y - upper_y).max(2.0);

                // Target Corridor Shaded Area
                frame.fill(
                    &Path::rectangle(Point::new(col_x + 1.0, upper_y), Size::new(col_width - 2.0, corridor_h)),
                    Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                );

                // Target Line
                frame.stroke(
                    &Path::line(Point::new(col_x, target_y), Point::new(col_x + col_width, target_y)),
                    Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.55)).with_width(1.0),
                );
            }

            // Listen range (if active)
            if self.listen_samples > 100.0 {
                let listen_db = self.listen_levels[b].max(-50.0) + TILT[b];
                let norm_listen_db = listen_db - listen_avg;
                let listen_y = db_to_y(norm_listen_db);

                // Red Min/Max Box — exact peak range from analysis
                let min_db_l = self.listen_level_min[b].max(-50.0) + TILT[b];
                let max_db_l = self.listen_level_max[b].max(-50.0) + TILT[b];
                let norm_min = min_db_l - listen_avg;
                let norm_max = max_db_l - listen_avg;
                let upper_y = db_to_y(norm_max);
                let lower_y = db_to_y(norm_min);
                let tolerance_h = (lower_y - upper_y).max(2.0);

                frame.fill(
                    &Path::rectangle(Point::new(col_x + 1.0, upper_y), Size::new(col_width - 2.0, tolerance_h)),
                    Color::from_rgba(1.0, 0.3, 0.3, 0.12),
                );

                // Listen Tolerances Corridor
                let listen_tolerance = self.listen_tolerances[b];
                let l_upper_y = db_to_y(norm_listen_db + listen_tolerance);
                let l_lower_y = db_to_y(norm_listen_db - listen_tolerance);
                let l_corridor_h = (l_lower_y - l_upper_y).max(2.0);
                frame.fill(
                    &Path::rectangle(Point::new(col_x + 1.0, l_upper_y), Size::new(col_width - 2.0, l_corridor_h)),
                    Color::from_rgba(0.5, 0.5, 1.0, 0.10),
                );

                // Analyzed Level Line (Crimson)
                frame.stroke(
                    &Path::line(Point::new(col_x, listen_y), Point::new(col_x + col_width, listen_y)),
                    Stroke::default().with_color(Color::from_rgba(1.0, 0.3, 0.3, 0.7)).with_width(1.5),
                );
            }
        }

        vec![frame.into_geometry()]
    }
}

// ─── IcedPlugin ──────────────────────────────────────────────────────────────

impl IcedPlugin<EquilibriumParams> for EquilibriumEditor {
    type Message = EquilibriumMsg;

    fn new(params: Arc<EquilibriumParams>) -> Self {
        let config = shared_analysis::load_config("Equilibrium");
        let presets = load_presets(config.vault_path.as_deref());
        let selected_idx = Some(0usize.min(presets.len().saturating_sub(1)));

        let mut target_levels = [0.0f32; 5];
        let mut target_tolerances = shared_analysis::DEFAULT_TOLERANCES;
        if let Some(idx) = selected_idx {
            let p = &presets[idx].2;
            target_levels = p.bands;
            target_tolerances = p.tolerances;
            for b in 0..5 {
                params.shared.target_levels[b].store(p.bands[b], Ordering::Release);
                params.shared.target_tolerances[b].store(p.tolerances[b], Ordering::Release);
            }
            params.shared.selected_preset_index.store(idx, Ordering::Release);
        }

        Self {
            shared_state: params.shared.clone(),
            params,
            vault_path: config.vault_path.clone(),
            show_setup: false,
            vault_path_input: config.vault_path.unwrap_or_default(),
            preset_name_input: String::new(),
            presets,
            selected_preset_index: selected_idx,
            preset_refresh_counter: 0,
            band_levels: [-90.0; 5],
            target_levels,
            target_tolerances,
            listen_levels: [-90.0; 5],
            listen_tolerances: [0.0; 5],
            listen_level_min: [-90.0; 5],
            listen_level_max: [-90.0; 5],
            listen_samples: 0.0,
            phase_correlation: 1.0,
            output_peak: -90.0, peak_hold: -90.0,
            peak_l: -90.0, peak_r: -90.0,
            peak_hold_l: -90.0, peak_hold_r: -90.0,
            balance: 0.0,
            auto_loud_measuring: false,
            snap_active: false,
            snap_blink_counter: 0,
        }
    }

    fn subscription(&self) -> Subscription<Message<EquilibriumMsg>> {
        truce_iced::iced::event::listen_raw(|event, _status, _window| {
            use truce_iced::iced::{Event, window::Event as WinEvent};
            match event {
                Event::Window(WinEvent::RedrawRequested(_)) => Some(Message::Plugin(EquilibriumMsg::Tick)),
                _ => None,
            }
        })
    }

    fn needs_redraw(&self) -> bool { true }

    fn update(
        &mut self,
        message: Message<EquilibriumMsg>,
        _cache: &ParamCache<EquilibriumParams>,
        ctx: &PluginContext<EquilibriumParams>,
    ) -> iced::Task<Message<EquilibriumMsg>> {
        match &message {
            Message::Plugin(msg) => self.handle_msg(msg, ctx),
            _ => {}
        }
        iced::Task::none()
    }

    fn view(&self, _cache: &ParamCache<EquilibriumParams>) -> Element<'_, Message<EquilibriumMsg>> {
        let pm = |m: EquilibriumMsg| Message::Plugin(m);

        // ── Sidebar ──
        let sel_name = self.selected_preset_index.and_then(|i| self.presets.get(i)).map(|(n, _, _)| n.as_str());
        let no_vault = self.vault_path.as_ref().is_none_or(|v| v.is_empty());
        let warning: Option<&str> = if no_vault { Some("Set Vault-path first") } else { None };
        let factory: Vec<(&str, Message<EquilibriumMsg>)> = self.presets.iter().enumerate()
            .filter(|(_, (_, p, _))| p.is_none())
            .map(|(i, (n, _, _))| (n.as_str(), pm(EquilibriumMsg::SelectPreset(i)))).collect();
        let user: Vec<(&str, Message<EquilibriumMsg>)> = self.presets.iter().enumerate()
            .filter(|(_, (_, p, _))| p.is_some())
            .map(|(i, (n, _, _))| (n.as_str(), pm(EquilibriumMsg::SelectPreset(i)))).collect();
        let snap_blink = self.snap_active && (self.snap_blink_counter / 8).is_multiple_of(2);
        let snap_label = if self.snap_active { "ANALYZE..." } else if no_vault { "SET VAULT" } else { "SNAP" };

        let sidebar = ai_preset_panel(
            "TARGET PROFILES", sel_name, &self.preset_name_input,
            move |s| pm(EquilibriumMsg::PresetNameChanged(s)),
            pm(EquilibriumMsg::SavePreset), pm(EquilibriumMsg::SnapPressed),
            snap_label, snap_blink, pm(EquilibriumMsg::SetupToggled),
            warning, factory.into_iter(), user.into_iter(),
        );

        // ── Middle ──
        let middle: Element<'_, Message<EquilibriumMsg>> = if self.show_setup {
            let sb = vault_setup_box("Equilibrium", &self.vault_path_input,
                move |s| pm(EquilibriumMsg::VaultPathChanged(s)),
                pm(EquilibriumMsg::SaveVaultPath), pm(EquilibriumMsg::SetupToggled));
            container(sb).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).into()
        } else {
            let band_names = ["Sub", "Bass", "Mid", "Pres", "Air"];
            let band_hz = ["0-80Hz", "80-300Hz", "300Hz-2kHz", "2-6kHz", ">6kHz"];
            let mut sliders_row = row![].spacing(10);
            for b in 0..5 {
                let gain = [&self.params.low_gain, &self.params.bass_gain, &self.params.mid_gain, &self.params.high_mid_gain, &self.params.high_gain][b].raw_target() as f32;
                let width = [&self.params.low_width, &self.params.bass_width, &self.params.mid_width, &self.params.high_mid_width, &self.params.high_width][b].raw_target() as f32;
                let pan = [&self.params.low_pan, &self.params.bass_pan, &self.params.mid_pan, &self.params.high_mid_pan, &self.params.high_pan][b].raw_target() as f32;
                let is_solo = [&self.params.solo_low, &self.params.solo_bass, &self.params.solo_mid, &self.params.solo_high_mid, &self.params.solo_high][b].value();
                let band_col = column![
                    Text::new("Gain").size(12).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
                    hslider_gesture(-12.0, 12.0, gain, 0.0, move |g| pm(EquilibriumMsg::GainGesture(b, g))),
                    Text::new(format!("{:.1} dB", gain)).size(11).font(bold_font()).color(Color::from_rgb(0.8, 0.8, 0.8)),
                    Text::new("Width").size(12).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
                    hslider_gesture(0.0, 150.0, width, 100.0, move |g| pm(EquilibriumMsg::WidthGesture(b, g))),
                    Text::new(format!("{:.0}%", width)).size(11).font(bold_font()).color(Color::from_rgb(0.8, 0.8, 0.8)),
                    Text::new("Pan").size(12).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
                    hslider_gesture(-1.0, 1.0, pan, 0.0, move |g| pm(EquilibriumMsg::PanGesture(b, g))),
                    Text::new(format_pan(pan)).size(11).font(bold_font()).color(Color::from_rgb(0.8, 0.8, 0.8)),
                    button(Text::new(if is_solo { "SOLO ON" } else { "SOLO" }).size(12).font(bold_font()))
                        .on_press(pm(EquilibriumMsg::SoloToggled(b)))
                        .style(move |_, s| button::Style {
                            background: Some(if is_solo { Color::from_rgb(1.0, 0.45, 0.1) } else if s == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.15, 0.15, 0.15) }.into()),
                            text_color: Color::WHITE, border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default()
                        }),
                ].spacing(4).align_x(Alignment::Center).width(Length::FillPortion(1));
                sliders_row = sliders_row.push(band_col);
            }

            let spectrum = canvas(EqSpectrumCanvas {
                band_levels: self.band_levels, target_levels: self.target_levels,
                target_tolerances: self.target_tolerances, listen_levels: self.listen_levels,
                listen_tolerances: self.listen_tolerances, listen_level_min: self.listen_level_min,
                listen_level_max: self.listen_level_max, listen_samples: self.listen_samples,
            }).width(Length::Fill).height(Length::Fill);

            let labels: Vec<Element<'_, Message<EquilibriumMsg>>> = (0..5).map(|i|
                container(Text::new(format!("{} ({})", band_names[i], band_hz[i])).size(11).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)))
                    .center_x(Length::Fill).width(Length::FillPortion(1)).into()
            ).collect();

            container(column![
                container(spectrum).height(Length::Fill).width(Length::Fill),
                row(labels).height(Length::Fixed(20.0)).width(Length::Fill),
                container(sliders_row).height(Length::Shrink).width(Length::Fill).padding(10),
            ].spacing(10)).width(Length::Fill).height(Length::Fill).into()
        };

        // ── Right Sidebar ──
        let scope_write_pos = self.shared_state.scope_write_pos.load(Ordering::Acquire);
        let gonio = canvas(GoniometerCanvas {
            samples: self.shared_state.scope_samples.clone(),
            write_pos: scope_write_pos,
            correlation: self.phase_correlation,
        }).width(Length::Fill).height(Length::Fixed(139.0));

        let pre_active = self.params.pre_master_active.value();
        let pre_target = self.params.pre_master_target_db.raw_target() as f32;
        let pre_btn = button(Text::new("PRE-MASTER").size(10).font(bold_font()))
            .on_press(pm(EquilibriumMsg::PreMasterToggled)).padding([3, 6])
            .style(move |_, s| button::Style {
                background: Some((if pre_active { Color::from_rgb(1.0, 0.45, 0.1) } else if s == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.15, 0.15, 0.15) }).into()),
                text_color: Color::WHITE, border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default()
            });

        let pre_section = column![
            pre_btn,
            Text::new(format!("Target: {:.1} dB", pre_target)).size(10).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)),
            container(hslider_gesture(-6.0, -3.0, pre_target, -6.0, move |g| pm(EquilibriumMsg::PreMasterGesture(g)))).width(Length::Fill),
        ].spacing(4);

        let controls = row![
            container(knob_gesture_bipolar("OUT GAIN", self.params.output_gain.raw_target() as f32, -12.0, 12.0, 0.0,
                move |g| pm(EquilibriumMsg::OutputGainGesture(g)))).width(Length::Fixed(60.0)),
            column![pre_section, auto_loud_button(self.auto_loud_measuring,
                self.shared_state.auto_loud_gain_offset.load(Ordering::Acquire).abs() > 0.05,
                self.params.pre_master_active.value(), pm(EquilibriumMsg::AutoLoudTriggered))],
        ].spacing(4).align_y(Alignment::Center);

        let right_title = container(Text::new("OUTPUT LEVEL").size(12).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)))
            .width(Length::Fill).padding(Padding { top: 2.0, right: 0.0, bottom: 4.0, left: 0.0 });

        let out_block = output_level_block(self.peak_l, self.peak_r, self.peak_hold_l, self.peak_hold_r,
            self.peak_hold, pm(EquilibriumMsg::ResetPeak), self.balance, Length::Fill);

        let right = container(column![
            right_title, controls, out_block,
            Text::new("GONIOMETER").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
            container(gonio).width(Length::Fill).height(Length::Fixed(139.0)),
        ].spacing(6))
        .width(Length::Fixed(155.0)).height(Length::Fill).padding(8)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()),
            border: Border { color: Color::from_rgb(0.18, 0.18, 0.18), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── Header ──
        let strip = monitor_strip(
            self.params.mono_active.value(), self.params.delta_active.value(), self.params.bypass_active.value(),
            pm(EquilibriumMsg::MonoToggled), pm(EquilibriumMsg::DeltaToggled), pm(EquilibriumMsg::BypassToggled));
        let header = container(row![container(header_brand("EQUILIBRIUM", VERSION)).width(Length::Fill), strip]
            .align_y(Alignment::Center).spacing(10))
            .width(Length::Fill).height(Length::Fixed(50.0)).padding(10)
            .style(|_| container::Style {
                background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
                border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
                ..Default::default()
            });

        // ── Footer ──
        let is_listen = self.params.listen_active.value();
        let listen_btn = button(Text::new(if is_listen { "LISTEN ON" } else { "LISTEN" }).size(13).font(bold_font()))
            .on_press(pm(EquilibriumMsg::ListenToggled)).padding(8)
            .style(move |_, s| button::Style {
                background: Some((if is_listen { Color::from_rgb(1.0, 0.45, 0.1) } else if s == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.15, 0.15, 0.15) }).into()),
                text_color: Color::WHITE, border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default()
            });

        let apply_btn = button(Text::new("APPLY ANALYSIS").size(12))
            .on_press_maybe(if is_listen { Some(pm(EquilibriumMsg::ApplyAnalysisAsTarget)) } else { None }).padding(8)
            .style(move |_, s| {
                let bg = if is_listen { if s == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.15, 0.15, 0.15) } } else { Color::from_rgb(0.08, 0.08, 0.08) };
                let tc = if is_listen { Color::WHITE } else { Color::from_rgb(0.3, 0.3, 0.3) };
                button::Style { background: Some(bg.into()), text_color: tc, border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default() }
            });

        let ra_btn = button(Text::new("RESET ANALYSIS").size(12))
            .on_press_maybe(if is_listen { Some(pm(EquilibriumMsg::ResetAnalysis)) } else { None }).padding(8)
            .style(move |_, s| {
                let bg = if is_listen { if s == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.15, 0.15, 0.15) } } else { Color::from_rgb(0.08, 0.08, 0.08) };
                let tc = if is_listen { Color::WHITE } else { Color::from_rgb(0.3, 0.3, 0.3) };
                button::Style { background: Some(bg.into()), text_color: tc, border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default() }
            });

        let mf_knob = knob_gesture_suffixed("MONO FLOOR", " Hz", self.params.mono_floor.raw_target() as f32, 0.0, 300.0, 0.0,
            move |g| pm(EquilibriumMsg::MonoFloorGesture(g)));
        let reset_btn = output_tools_strip(pm(EquilibriumMsg::ResetAll));

        let analyse_section = column![
            Text::new("ANALYZE").size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)),
            row![listen_btn, apply_btn, ra_btn].spacing(12).align_y(Alignment::Center),
        ].spacing(4).align_x(Alignment::Center);

        let mf_section = column![
            Text::new("MONO FLOOR").size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)),
            row![mf_knob].spacing(12).align_y(Alignment::Center),
        ].spacing(4).align_x(Alignment::Center);

        let footer = container(row![
            container(analyse_section).padding(5), vsep(), Space::new().width(Length::Fill),
            vsep(), container(mf_section).padding(5), vsep(), container(reset_btn).padding(5),
        ].align_y(Alignment::Center).spacing(15))
        .width(Length::Fill).height(Length::Fixed(110.0)).padding(8)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── Assembly ──
        let body = column![header, row![sidebar, container(middle).width(Length::Fill).height(Length::Fill), right].height(Length::Fill).width(Length::Fill), footer]
            .width(Length::Fill).height(Length::Fill);

        container(body).width(Length::Fill).height(Length::Fill)
            .style(|_| container::Style { background: Some(Color::from_rgb(0.06, 0.06, 0.06).into()), text_color: Some(Color::WHITE), ..Default::default() }).into()
    }
}

// ─── Message Handler ─────────────────────────────────────────────────────────

impl EquilibriumEditor {
    fn handle_msg(&mut self, msg: &EquilibriumMsg, ctx: &PluginContext<EquilibriumParams>) {
        match msg {
            EquilibriumMsg::Tick => self.do_tick(ctx),
            EquilibriumMsg::GainGesture(b, g) => self.do_gesture(|p| [&p.low_gain, &p.bass_gain, &p.mid_gain, &p.high_mid_gain, &p.high_gain][*b], g, ctx),
            EquilibriumMsg::WidthGesture(b, g) => self.do_gesture(|p| [&p.low_width, &p.bass_width, &p.mid_width, &p.high_mid_width, &p.high_width][*b], g, ctx),
            EquilibriumMsg::PanGesture(b, g) => self.do_gesture(|p| [&p.low_pan, &p.bass_pan, &p.mid_pan, &p.high_mid_pan, &p.high_pan][*b], g, ctx),
            EquilibriumMsg::MonoFloorGesture(g) => self.do_gesture(|p| &p.mono_floor, g, ctx),
            EquilibriumMsg::OutputGainGesture(g) => self.do_gesture(|p| &p.output_gain, g, ctx),
            EquilibriumMsg::SoloToggled(b) => self.do_toggle(|p| [&p.solo_low, &p.solo_bass, &p.solo_mid, &p.solo_high_mid, &p.solo_high][*b], ctx),
            EquilibriumMsg::MonoToggled => self.do_toggle(|p| &p.mono_active, ctx),
            EquilibriumMsg::DeltaToggled => self.do_toggle(|p| &p.delta_active, ctx),
            EquilibriumMsg::BypassToggled => self.do_toggle(|p| &p.bypass_active, ctx),
            EquilibriumMsg::ListenToggled => self.do_toggle(|p| &p.listen_active, ctx),
            EquilibriumMsg::ApplyAnalysisAsTarget => {
                if self.listen_samples > 100.0 {
                    for b in 0..5 {
                        self.target_levels[b] = self.listen_levels[b];
                        self.target_tolerances[b] = self.listen_tolerances[b];
                        self.shared_state.target_levels[b].store(self.listen_levels[b], Ordering::Release);
                        self.shared_state.target_tolerances[b].store(self.listen_tolerances[b], Ordering::Release);
                    }
                }
            }
            EquilibriumMsg::ResetAnalysis => {
                self.shared_state.reset_analysis.store(true, Ordering::Release);
                self.listen_levels = [-90.0; 5];
                self.listen_level_min = [-90.0; 5];
                self.listen_level_max = [-90.0; 5];
                self.shared_state.listen_samples.store(0.0, Ordering::Release);
                for b in 0..5 {
                    self.shared_state.listen_levels[b].store(-90.0, Ordering::Release);
                    self.shared_state.listen_tolerances[b].store(0.0, Ordering::Release);
                }
            }
            EquilibriumMsg::AutoLoudTriggered => {
                if !self.params.pre_master_active.value() {
                    self.shared_state.auto_loud_trigger.store(true, Ordering::Release);
                }
            }
            EquilibriumMsg::ResetAll => {
                let p = &self.params;
                for (param, val) in [
                    (&p.low_gain, 0.0f64), (&p.bass_gain, 0.0), (&p.mid_gain, 0.0), (&p.high_mid_gain, 0.0), (&p.high_gain, 0.0),
                    (&p.low_width, 100.0), (&p.bass_width, 100.0), (&p.mid_width, 100.0), (&p.high_mid_width, 100.0), (&p.high_width, 100.0),
                    (&p.low_pan, 0.0), (&p.bass_pan, 0.0), (&p.mid_pan, 0.0), (&p.high_mid_pan, 0.0), (&p.high_pan, 0.0),
                    (&p.output_gain, 0.0), (&p.mono_floor, 0.0),
                ] { param.set_value(val); }
                self.shared_state.auto_loud_gain_offset.store(0.0, Ordering::Release);
                self.shared_state.reset_analysis.store(true, Ordering::Release);
            }
            EquilibriumMsg::SelectPreset(idx) => {
                let i = *idx;
                if i < self.presets.len() {
                    self.selected_preset_index = Some(i);
                    self.shared_state.selected_preset_index.store(i, Ordering::Release);
                    let prof = &self.presets[i].2;
                    for b in 0..5 {
                        self.shared_state.target_levels[b].store(prof.bands[b], Ordering::Release);
                        self.shared_state.target_tolerances[b].store(prof.tolerances[b], Ordering::Release);
                    }
                    // Bands are the Target Profile (analysis reference line), not a
                    // gain correction — only stereo settings apply directly to params.
                    self.params.low_width.set_value(prof.widths[0] as f64);
                    self.params.bass_width.set_value(prof.widths[1] as f64);
                    self.params.mid_width.set_value(prof.widths[2] as f64);
                    self.params.high_mid_width.set_value(prof.widths[3] as f64);
                    self.params.high_width.set_value(prof.widths[4] as f64);

                    self.params.low_pan.set_value(prof.pans[0] as f64);
                    self.params.bass_pan.set_value(prof.pans[1] as f64);
                    self.params.mid_pan.set_value(prof.pans[2] as f64);
                    self.params.high_mid_pan.set_value(prof.pans[3] as f64);
                    self.params.high_pan.set_value(prof.pans[4] as f64);

                    self.params.mono_floor.set_value(prof.mono_floor_hz as f64);
                }
            }
            EquilibriumMsg::PresetNameChanged(name) => self.preset_name_input = name.clone(),
            EquilibriumMsg::SavePreset => self.do_save_preset(),
            EquilibriumMsg::SetupToggled => self.show_setup = !self.show_setup,
            EquilibriumMsg::VaultPathChanged(p) => self.vault_path_input = p.clone(),
            EquilibriumMsg::SaveVaultPath => {
                let vp = self.vault_path_input.trim().to_string();
                if !vp.is_empty() {
                    self.vault_path = Some(vp.clone());
                    let mut cfg = shared_analysis::load_config("Equilibrium");
                    cfg.vault_path = Some(vp.clone());
                    let _ = shared_analysis::save_config("Equilibrium", &cfg);
                    self.presets = load_presets(Some(&vp));
                    self.selected_preset_index = Some(0);
                    self.show_setup = false;
                }
            }
            EquilibriumMsg::ResetPeak => {
                self.shared_state.reset_peak.store(true, Ordering::Release);
            }
            EquilibriumMsg::PreMasterToggled => {
                let nv = !self.params.pre_master_active.value();
                self.params.pre_master_active.set_value(nv);
                if nv {
                    self.shared_state.auto_loud_measuring.store(false, Ordering::Release);
                    self.shared_state.auto_loud_gain_offset.store(0.0, Ordering::Release);
                }
            }
            EquilibriumMsg::PreMasterGesture(g) => self.do_gesture(|p| &p.pre_master_target_db, g, ctx),
            EquilibriumMsg::SnapPressed => {
                if self.vault_path.as_ref().is_none_or(|v| v.is_empty()) {
                    self.show_setup = true;
                } else {
                    self.shared_state.snap_active.store(true, Ordering::Release);
                    self.shared_state.snap_phase.store(1, Ordering::Release);
                }
            }
        }
    }

    fn do_tick(&mut self, _ctx: &PluginContext<EquilibriumParams>) {
        for b in 0..5 {
            self.band_levels[b] = self.shared_state.band_levels[b].load(Ordering::Acquire);
            self.target_levels[b] = self.shared_state.target_levels[b].load(Ordering::Acquire);
            self.target_tolerances[b] = self.shared_state.target_tolerances[b].load(Ordering::Acquire);
            self.listen_levels[b] = self.shared_state.listen_levels[b].load(Ordering::Acquire);
            self.listen_tolerances[b] = self.shared_state.listen_tolerances[b].load(Ordering::Acquire);
            self.listen_level_min[b] = self.shared_state.listen_level_min[b].load(Ordering::Acquire);
            self.listen_level_max[b] = self.shared_state.listen_level_max[b].load(Ordering::Acquire);
        }
        self.listen_samples = self.shared_state.listen_samples.load(Ordering::Acquire);
        self.phase_correlation = self.shared_state.phase_correlation.load(Ordering::Acquire);
        self.output_peak = self.shared_state.output_peak.load(Ordering::Acquire);
        self.peak_hold = self.shared_state.peak_hold.load(Ordering::Acquire);
        self.peak_l = self.shared_state.output_peak_l.load(Ordering::Acquire);
        self.peak_r = self.shared_state.output_peak_r.load(Ordering::Acquire);
        self.peak_hold_l = self.shared_state.peak_hold_l.load(Ordering::Acquire);
        self.peak_hold_r = self.shared_state.peak_hold_r.load(Ordering::Acquire);
        self.balance = self.shared_state.balance.load(Ordering::Acquire);

        self.preset_refresh_counter = self.preset_refresh_counter.wrapping_add(1);
        if self.preset_refresh_counter % 60 == 0 {
            self.presets = load_presets(self.vault_path.as_deref());
        }

        let measuring = self.shared_state.auto_loud_measuring.load(Ordering::Acquire);
        self.auto_loud_measuring = measuring;
        if !measuring {
            let offset = self.shared_state.auto_loud_gain_offset.load(Ordering::Acquire);
            if offset.abs() > 0.05 {
                let cur = self.params.output_gain.raw_target() as f32;
                self.params.output_gain.set_value((cur + offset).clamp(-12.0, 12.0) as f64);
                self.shared_state.auto_loud_gain_offset.store(0.0, Ordering::Release);
            }
        }

        let snap_now = self.shared_state.snap_active.load(Ordering::Acquire);
        let was = self.snap_active;
        self.snap_active = snap_now;
        if self.snap_active { self.snap_blink_counter = self.snap_blink_counter.wrapping_add(1); }
        else if was {
            self.snap_blink_counter = 0;
            if let Some(ref vp) = self.vault_path {
                if !vp.is_empty() {
                    let stereo = self.shared_state.snap_stereo_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let mono = self.shared_state.snap_mono_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let delta = self.shared_state.snap_delta_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let sr = self.shared_state.sample_rate.load(Ordering::Acquire);
                    let md = snap_markdown(&self.params, &stereo, &mono, &delta, self.band_levels, self.phase_correlation, self.peak_l, self.peak_r, sr);
                    let fname = snap_filename(vp);
                    let _ = std::fs::write(std::path::Path::new(vp).join(&fname), &md);
                }
            }
        }
    }

    fn do_gesture(&self, f: impl Fn(&EquilibriumParams) -> &FloatParam, g: &Gesture, _ctx: &PluginContext<EquilibriumParams>) {
        let p = f(&self.params);
        match g {
            Gesture::Start => {}
            Gesture::Change(v) => p.set_value(*v as f64),
            Gesture::End => {}
        }
    }

    fn do_toggle(&self, f: impl Fn(&EquilibriumParams) -> &BoolParam, _ctx: &PluginContext<EquilibriumParams>) {
        let p = f(&self.params);
        p.set_value(!p.value());
    }

    fn do_save_preset(&mut self) {
        // Bands/tolerances are the Target Profile (analysis reference line,
        // set via ApplyAnalysisAsTarget or an already-selected preset) —
        // not the current gain knob positions.
        let bands = self.target_levels;
        let tolerances = self.target_tolerances;

        let name = if self.preset_name_input.trim().is_empty() {
            format!("User Preset {}", self.presets.len() + 1)
        } else { self.preset_name_input.trim().to_string() };

        let dir = match &self.vault_path {
            Some(vp) if !vp.is_empty() => std::path::PathBuf::from(vp),
            _ => shared_analysis::get_plugin_dir("Equilibrium").join("presets"),
        };
        let _ = std::fs::create_dir_all(&dir);
        let safe = name.replace(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_', "");
        let fp = dir.join(format!("{}.md", safe));

        let prof = shared_analysis::Profile {
            name: name.clone(), bands, tolerances,
            pans: [self.params.low_pan.raw_target() as f32, self.params.bass_pan.raw_target() as f32,
                self.params.mid_pan.raw_target() as f32, self.params.high_mid_pan.raw_target() as f32,
                self.params.high_pan.raw_target() as f32],
            widths: [self.params.low_width.raw_target() as f32, self.params.bass_width.raw_target() as f32,
                self.params.mid_width.raw_target() as f32, self.params.high_mid_width.raw_target() as f32,
                self.params.high_width.raw_target() as f32],
            mono_floor_hz: self.params.mono_floor.raw_target() as f32,
            ..shared_analysis::Profile::default()
        };
        let md = shared_analysis::export_preset_to_markdown(&prof);
        if std::fs::write(&fp, &md).is_ok() {
            self.presets = load_presets(self.vault_path.as_deref());
            self.preset_name_input.clear();
        }
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

fn snap_markdown(_p: &EquilibriumParams, stereo: &[f32], mono: &[f32], delta: &[f32],
    band_levels: [f32; 5], corr: f32, pl: f32, pr: f32, sr: f32) -> String
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
        "---\nplugin: equilibrium\ntype: snapshot\n---\n\n# Equilibrium Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n\n\
        ## 5-Band\n| Band | Pegel |\n|------|-------|\n\
        | Low | {b0:.1} dB |\n| Bass | {b1:.1} dB |\n| Mid | {b2:.1} dB |\n| Hi-Mid | {b3:.1} dB |\n| High | {b4:.1} dB |\n",
        pl=pl, pr=pr, co=corr, st=tbl(stereo), mn=tbl(mono), dt=tbl(delta),
        b0=band_levels[0], b1=band_levels[1], b2=band_levels[2], b3=band_levels[3], b4=band_levels[4],
    )
}
