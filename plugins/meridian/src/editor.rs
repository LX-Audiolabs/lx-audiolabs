// Meridian editor — Iced UI truce port.
//
// Layout (990×660):
//   Header : brand + monitor strip (MONO/DELTA/BYPASS)
//   Left   : preset panel (SNAP, Vault, preset list)
//   Center : Top strip (HPF/LPF/Warmth/Exciter/Tilt) + SpectrumCanvas +
//            5-band EQ (Gain/Freq/Slope per band) + Compressor strip
//   Right  : Stereo Width, Pan, Output Gain, Auto Loud, Output Meter,
//            Goniometer, Correlation/Balance, GR Meter

use truce_iced::iced;
use truce_iced::iced::widget::{button, canvas, column, container, row, Space, Text};
use truce_iced::iced::widget::canvas::{Geometry, Path, Stroke};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Padding, Point, Rectangle, Subscription};
use truce_iced::iced::mouse::Cursor;
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;
use truce::prelude::{FloatParam, IntParam, BoolParam};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::path::PathBuf;

use shared_analysis::SharedState;
use shared_ui::{
    bold_font, header_brand, monitor_strip, vault_setup_box,
    ai_preset_panel, auto_loud_button, output_level_block, output_tools_strip,
    knob_gesture, knob_gesture_log, knob_gesture_bipolar, hslider_gesture,
    GoniometerCanvas, SpectrumCanvas, SpectrumCurve, SpectrumConfig, EqOverlay, Gesture,
};
use shared_dsp::Biquad;
use shared_analysis::SPECTRUM_BINS;

use crate::MeridianParams;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum MeridianMsg {
    Tick,
    HpfGesture(Gesture),
    LpfGesture(Gesture),
    CutSlopeChanged(i32),
    BassGainGesture(Gesture),
    BassSlopeChanged(i32),
    LoMidGainGesture(Gesture),
    LoMidSlopeChanged(i32),
    MidGainGesture(Gesture),
    MidSlopeChanged(i32),
    HighGainGesture(Gesture),
    HighSlopeChanged(i32),
    ExciteGainGesture(Gesture),
    ExciteSlopeChanged(i32),
    EqFreq1Gesture(Gesture),
    EqFreq2Gesture(Gesture),
    EqFreq3Gesture(Gesture),
    EqFreq4Gesture(Gesture),
    EqFreq5Gesture(Gesture),
    TiltGainGesture(Gesture),
    WarmthDriveGesture(Gesture),
    WarmthMixGesture(Gesture),
    ExciteAmountGesture(Gesture),
    ExciteBlendGesture(Gesture),
    ExciteFreqGesture(Gesture),
    CompThresholdGesture(Gesture),
    CompMixGesture(Gesture),
    CompAttackGesture(Gesture),
    CompReleaseGesture(Gesture),
    CompCharacterGesture(Gesture),
    CompMakeupGesture(Gesture),
    InflateEffectGesture(Gesture),
    InflateCurveGesture(Gesture),
    InflateBandSplitToggled,
    InflateClipToggled,
    StereoWidthGesture(Gesture),
    PanGesture(Gesture),
    OutputGainGesture(Gesture),
    MonoToggled,
    DeltaToggled,
    BypassToggled,
    AutoLoudTriggered,
    ResetAll,
    SelectPreset(usize),
    PresetNameChanged(String),
    SavePreset,
    SetupToggled,
    VaultPathChanged(String),
    SaveVaultPath,
    ResetPeak,
    SnapPressed,
}

// ─── MeridianProfile ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct MeridianProfile {
    pub name: String,
    pub hpf_freq: f32, pub lpf_freq: f32, pub cut_slope: i32,
    pub bass_gain: f32, pub bass_slope: i32,
    pub lo_mid_gain: f32, pub lo_mid_slope: i32,
    pub mid_gain: f32, pub mid_slope: i32,
    pub high_gain: f32, pub high_slope: i32,
    pub excite_gain: f32, pub excite_slope: i32,
    pub eq_freq_1: f32, pub eq_freq_2: f32, pub eq_freq_3: f32, pub eq_freq_4: f32, pub eq_freq_5: f32,
    pub tilt_gain: f32, pub warmth_drive: f32, pub warmth_mix: f32,
    pub excite_amount: f32, pub excite_blend: f32, pub excite_freq: f32,
    pub comp_threshold: f32, pub comp_mix: f32, pub comp_attack: f32, pub comp_release: f32,
    pub comp_character: f32, pub comp_makeup: f32,
    pub inflate_effect: f32, pub inflate_curve: f32, pub inflate_band_split: bool, pub inflate_clip: bool,
    pub stereo_width: f32, pub pan: f32, pub output_gain: f32,
}

impl Default for MeridianProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            hpf_freq: 2.0, lpf_freq: 35000.0, cut_slope: 0,
            bass_gain: 0.0, bass_slope: 1,
            lo_mid_gain: 0.0, lo_mid_slope: 1,
            mid_gain: 0.0, mid_slope: 1,
            high_gain: 0.0, high_slope: 1,
            excite_gain: 0.0, excite_slope: 1,
            eq_freq_1: 80.0, eq_freq_2: 300.0, eq_freq_3: 1000.0, eq_freq_4: 4000.0, eq_freq_5: 12000.0,
            tilt_gain: 0.0, warmth_drive: 0.0, warmth_mix: 0.0,
            excite_amount: 0.0, excite_blend: 0.0, excite_freq: 8000.0,
            comp_threshold: 0.0, comp_mix: 40.0, comp_attack: 15.0, comp_release: 120.0,
            comp_character: 2.0, comp_makeup: 0.0,
            inflate_effect: 0.0, inflate_curve: 0.0, inflate_band_split: false, inflate_clip: false,
            stereo_width: 100.0, pan: 0.0, output_gain: 0.0,
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

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

fn short_freq(freq: f32) -> String {
    if freq < 1000.0 { format!("{:.0} Hz", freq) }
    else { format!("{:.1} kHz", freq / 1000.0) }
}

fn slope_char(s: i32) -> &'static str { match s { 0 => "A", 1 => "B", _ => "C" } }

// ─── Compressor Envelope Canvas ──────────────────────────────────────────────

/// Mini envelope visualization for compressor gain reduction over time.
pub struct CompressorEnvelopeCanvas {
    pub history: Vec<f32>,
    pub current: f32,
    pub peak_hold: f32,
}

impl<M> canvas::Program<M> for CompressorEnvelopeCanvas {
    type State = ();
    fn draw(&self, _state: &Self::State, renderer: &truce_iced::iced::Renderer,
            _theme: &truce_iced::iced::Theme, bounds: Rectangle, _cursor: Cursor) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width; let h = bounds.height;
        let max_gr = 24.0f32; let margin = 2.0;
        frame.fill(&Path::rectangle(Point::ORIGIN, bounds.size()), Color::from_rgb(0.08, 0.08, 0.08));
        let n = if self.history.is_empty() { 1 } else { self.history.len() + 1 };
        let x_step = (w - margin * 2.0) / (n - 1).max(1) as f32;
        let mut points: Vec<Point> = Vec::with_capacity(n);
        for (i, &val) in self.history.iter().enumerate() {
            let x = margin + i as f32 * x_step;
            let y = h - margin - ((val / max_gr).clamp(0.0, 1.0)) * (h - margin * 2.0);
            points.push(Point::new(x, y));
        }
        {
            let x = margin + self.history.len() as f32 * x_step;
            let y = h - margin - ((self.current / max_gr).clamp(0.0, 1.0)) * (h - margin * 2.0);
            points.push(Point::new(x, y));
        }
        if points.len() >= 2 {
            let mut fb = canvas::path::Builder::new();
            fb.move_to(Point::new(margin, margin));
            for p in &points { fb.line_to(*p); }
            let lx = points.last().map(|p| p.x).unwrap_or(w - margin);
            fb.line_to(Point::new(lx, h - margin));
            fb.line_to(Point::new(margin, h - margin)); fb.close();
            frame.fill(&fb.build(), Color::from_rgba(1.0, 0.35, 0.15, 0.18));
            let mut lb = canvas::path::Builder::new();
            lb.move_to(points[0]);
            for p in &points[1..] { lb.line_to(*p); }
            frame.stroke(&lb.build(), Stroke::default().with_color(Color::from_rgb(1.0, 0.4, 0.2)).with_width(1.2));
        }
        frame.stroke(&Path::line(Point::new(margin, margin), Point::new(w - margin, margin)),
            Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.1)).with_width(0.5));
        let y6 = margin + (h - margin * 2.0) * (6.0 / max_gr);
        frame.stroke(&Path::line(Point::new(margin, y6), Point::new(w - margin, y6)),
            Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.06)).with_width(0.5));
        if self.peak_hold > 0.1 {
            let py = h - margin - ((self.peak_hold / max_gr).clamp(0.0, 1.0)) * (h - margin * 2.0);
            let mut x = margin;
            while x < w - margin {
                let ex = (x + 4.0).min(w - margin);
                frame.stroke(&Path::line(Point::new(x, py), Point::new(ex, py)),
                    Stroke::default().with_color(Color::from_rgba(1.0, 0.65, 0.15, 0.55)).with_width(1.0));
                x += 7.0;
            }
        }
        vec![frame.into_geometry()]
    }
}

// ─── Editor ──────────────────────────────────────────────────────────────────

pub struct MeridianEditor {
    params: Arc<MeridianParams>,
    shared_state: Arc<SharedState>,
    vault_path: Option<String>,
    show_setup: bool,
    vault_path_input: String,
    preset_name_input: String,
    presets: Vec<(String, Option<PathBuf>, MeridianProfile)>,
    selected_preset_index: Option<usize>,
    preset_refresh_counter: u32,
    band_levels: [f32; 5],
    phase_correlation: f32,
    balance: f32,
    output_peak: f32, peak_hold: f32,
    peak_l: f32, peak_r: f32,
    peak_hold_l: f32, peak_hold_r: f32,
    gain_reduction: f32,
    gr_peak_hold: f32, gr_peak_hold_ticks: u32,
    gr_history: Vec<f32>,
    auto_loud_measuring: bool,
    snap_active: bool, snap_blink_counter: u32,
    preset_warning: Option<String>, preset_warning_ticks: u32,
    slope_sel: [i32; 5],
    cut_slope_sel: i32,
}

impl IcedPlugin<MeridianParams> for MeridianEditor {
    type Message = MeridianMsg;

    fn new(params: Arc<MeridianParams>) -> Self {
        let config = shared_analysis::load_config("Meridian");
        let presets: Vec<(String, Option<PathBuf>, MeridianProfile)> = list_meridian_presets(config.vault_path.as_deref())
            .into_iter().map(|(n, p, prof)| (n, Some(p), prof)).collect();
        Self {
            shared_state: params.shared.clone(),
            params,
            vault_path: config.vault_path.clone(),
            show_setup: false,
            vault_path_input: config.vault_path.unwrap_or_default(),
            preset_name_input: String::new(),
            presets,
            selected_preset_index: None,
            preset_refresh_counter: 0,
            band_levels: [-90.0; 5],
            phase_correlation: 1.0, balance: 0.0,
            output_peak: -90.0, peak_hold: -90.0,
            peak_l: -90.0, peak_r: -90.0,
            peak_hold_l: -90.0, peak_hold_r: -90.0,
            gain_reduction: 0.0, gr_peak_hold: 0.0, gr_peak_hold_ticks: 0,
            gr_history: vec![0.0; 90],
            auto_loud_measuring: false,
            snap_active: false, snap_blink_counter: 0,
            preset_warning: None, preset_warning_ticks: 0,
            slope_sel: [1i32; 5],
            cut_slope_sel: 0,
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
        message: Message<MeridianMsg>,
        _cache: &ParamCache<MeridianParams>,
        ctx: &PluginContext<MeridianParams>,
    ) -> iced::Task<Message<MeridianMsg>> {
        match &message {
            Message::Plugin(msg) => self.handle_msg(msg, ctx),
            _ => {}
        }
        iced::Task::none()
    }

    fn view(&self, _cache: &ParamCache<MeridianParams>) -> Element<'_, Message<MeridianMsg>> {
        let pm = |m: MeridianMsg| Message::Plugin(m);

        // ── Sidebar ──
        let sel_name = self.selected_preset_index.and_then(|i| self.presets.get(i)).map(|(n, _, _)| n.as_str());
        let no_vault = self.vault_path.as_ref().is_none_or(|v| v.is_empty());
        let panel_warning: Option<&str> = if let Some(w) = &self.preset_warning { Some(w) }
            else if no_vault { Some("⚠ Set Vault-path first") } else { None };
        let factory: Vec<(&str, Message<MeridianMsg>)> = self.presets.iter().enumerate()
            .filter(|(_, (_, p, _))| p.is_none())
            .map(|(i, (n, _, _))| (n.as_str(), pm(MeridianMsg::SelectPreset(i)))).collect();
        let user: Vec<(&str, Message<MeridianMsg>)> = self.presets.iter().enumerate()
            .filter(|(_, (_, p, _))| p.is_some())
            .map(|(i, (n, _, _))| (n.as_str(), pm(MeridianMsg::SelectPreset(i)))).collect();
        let snap_blink = self.snap_active && (self.snap_blink_counter / 8).is_multiple_of(2);
        let snap_label = if self.snap_active { "ANALYZE..." } else if no_vault { "SET VAULT" } else { "SNAP" };

        let sidebar = ai_preset_panel(
            "VAULT PRESETS", sel_name, &self.preset_name_input,
            move |s| pm(MeridianMsg::PresetNameChanged(s)),
            pm(MeridianMsg::SavePreset), pm(MeridianMsg::SnapPressed),
            snap_label, snap_blink, pm(MeridianMsg::SetupToggled),
            panel_warning, factory.into_iter(), user.into_iter(),
        );

        // ── Center ──
        let center: Element<'_, Message<MeridianMsg>> = if self.show_setup {
            let sb = vault_setup_box("Meridian", &self.vault_path_input,
                move |s| pm(MeridianMsg::VaultPathChanged(s)),
                pm(MeridianMsg::SaveVaultPath), pm(MeridianMsg::SetupToggled));
            container(sb).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).into()
        } else {
            let strip_label = |t: &'static str| Text::new(t).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15));

            // Cut-slope toggle
            let cut_slope_is_b = self.cut_slope_sel >= 1;
            let cut_slope_toggle = column![
                Text::new("SLOPE").size(9).font(bold_font()).color(Color::from_rgb(0.55, 0.55, 0.55)),
                button(Text::new(if cut_slope_is_b { "B" } else { "A" }).size(12).font(bold_font()))
                    .on_press(pm(MeridianMsg::CutSlopeChanged(if cut_slope_is_b { 0 } else { 1 })))
                    .padding(5)
                    .style(move |_t, st| {
                        let bg = if cut_slope_is_b { Color::from_rgb(1.0, 0.45, 0.1) }
                            else if st == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) }
                            else { Color::from_rgb(0.15, 0.15, 0.15) };
                        button::Style { background: Some(bg.into()), text_color: Color::WHITE,
                            border: Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() }
                    }),
                Text::new(if cut_slope_is_b { "24 dB" } else { "12 dB" }).size(9).font(bold_font()).color(Color::from_rgb(0.7, 0.7, 0.7)),
            ].spacing(3).align_x(Alignment::Center);

            // Top strip
            let top_strip = container(row![
                column![
                    strip_label("FILTER"),
                    row![
                        knob_gesture_log("LOW CUT", self.params.hpf_freq.raw_target() as f32, 2.0, 2000.0, 2.0, move |g| pm(MeridianMsg::HpfGesture(g))),
                        cut_slope_toggle,
                        knob_gesture_log("HIGH CUT", self.params.lpf_freq.raw_target() as f32, 200.0, 35000.0, 35000.0, move |g| pm(MeridianMsg::LpfGesture(g))),
                    ].spacing(15).align_y(Alignment::Center),
                ].spacing(4).align_x(Alignment::Center),
                vsep(),
                column![
                    strip_label("WARMTH"),
                    row![
                        knob_gesture("DRIVE", self.params.warmth_drive.raw_target() as f32, 0.0, 12.0, 0.0, move |g| pm(MeridianMsg::WarmthDriveGesture(g))),
                        knob_gesture("W/MIX", self.params.warmth_mix.raw_target() as f32, 0.0, 100.0, 0.0, move |g| pm(MeridianMsg::WarmthMixGesture(g))),
                    ].spacing(15),
                ].spacing(4).align_x(Alignment::Center),
                vsep(),
                column![
                    strip_label("EXCITER"),
                    row![
                        knob_gesture("AMT", self.params.excite_amount.raw_target() as f32, 0.0, 30.0, 0.0, move |g| pm(MeridianMsg::ExciteAmountGesture(g))),
                        knob_gesture("BLEND", self.params.excite_blend.raw_target() as f32, 0.0, 100.0, 0.0, move |g| pm(MeridianMsg::ExciteBlendGesture(g))),
                        knob_gesture("FREQ", self.params.excite_freq.raw_target() as f32, 6000.0, 12000.0, 8000.0, move |g| pm(MeridianMsg::ExciteFreqGesture(g))),
                    ].spacing(15),
                ].spacing(4).align_x(Alignment::Center),
                vsep(),
                column![
                    strip_label("TILT EQ"),
                    knob_gesture_bipolar("SLOPE", self.params.tilt_gain.raw_target() as f32, -1.5, 1.5, 0.0, move |g| pm(MeridianMsg::TiltGainGesture(g))),
                ].spacing(4).align_x(Alignment::Center),
            ].spacing(15).align_y(Alignment::Center))
            .width(Length::Fill).height(Length::Fixed(100.0)).padding(Padding { top: 5.0, right: 10.0, bottom: 5.0, left: 10.0 });

            // Spectrum canvas with EQ curve overlay
            let spectrum_snapshot: Vec<f32> = self.shared_state.spectrum_avg
                .lock().map(|g| g.clone()).unwrap_or_else(|_| vec![-90.0; SPECTRUM_BINS]);
            let sr = self.shared_state.sample_rate.load(Ordering::Acquire);
            let eq_overlay = compute_eq_curve(&self.params, self.slope_sel, self.cut_slope_sel, sr);
            let spectrum_canvas = canvas(SpectrumCanvas {
                curves: vec![SpectrumCurve {
                    spectrum: spectrum_snapshot,
                    color: Color::from_rgb(0.1, 0.9, 0.7),
                    fill_alpha: 0.18,
                    line_alpha: 0.85,
                    line_width: 1.6,
                }],
                config: SpectrumConfig { sample_rate: sr, ..Default::default() },
                eq_overlay,
                resonance_peaks: Vec::new(),
                masking: Vec::new(),
            }).width(Length::Fill).height(Length::Fill);

            // 5 EQ Bands
            let freq_params: [&FloatParam; 5] = [&self.params.eq_freq_1, &self.params.eq_freq_2, &self.params.eq_freq_3, &self.params.eq_freq_4, &self.params.eq_freq_5];
            let freq_ranges: [(f32, f32, f32); 5] = [
                (40.0, 200.0, 80.0), (150.0, 800.0, 300.0), (500.0, 3000.0, 1000.0),
                (2000.0, 10000.0, 4000.0), (6000.0, 20000.0, 12000.0),
            ];
            let band_names = ["LO SHELF", "LO-MID", "MID", "HI-MID", "HI SHELF"];
            let band_is_shelf = [true, false, false, false, true];

            let mut eq_row = row![].spacing(10);
            for idx in 0..5 {
                let gain = [&self.params.bass_gain, &self.params.lo_mid_gain, &self.params.mid_gain, &self.params.high_gain, &self.params.excite_gain][idx].raw_target() as f32;
                let freq = freq_params[idx].raw_target() as f32;
                let (fmin, fmax, fdef) = freq_ranges[idx];
                let label = band_names[idx];
                let is_shelf = band_is_shelf[idx];
                let slope = self.slope_sel[idx];

                let gain_msg: fn(Gesture) -> MeridianMsg = match idx {
                    0 => |g| MeridianMsg::BassGainGesture(g), 1 => |g| MeridianMsg::LoMidGainGesture(g),
                    2 => |g| MeridianMsg::MidGainGesture(g), 3 => |g| MeridianMsg::HighGainGesture(g),
                    _ => |g| MeridianMsg::ExciteGainGesture(g),
                };
                let slope_msg: fn(i32) -> MeridianMsg = match idx {
                    0 => |s| MeridianMsg::BassSlopeChanged(s), 1 => |s| MeridianMsg::LoMidSlopeChanged(s),
                    2 => |s| MeridianMsg::MidSlopeChanged(s), 3 => |s| MeridianMsg::HighSlopeChanged(s),
                    _ => |s| MeridianMsg::ExciteSlopeChanged(s),
                };
                let freq_msg: fn(Gesture) -> MeridianMsg = match idx {
                    0 => |g| MeridianMsg::EqFreq1Gesture(g), 1 => |g| MeridianMsg::EqFreq2Gesture(g),
                    2 => |g| MeridianMsg::EqFreq3Gesture(g), 3 => |g| MeridianMsg::EqFreq4Gesture(g),
                    _ => |g| MeridianMsg::EqFreq5Gesture(g),
                };

                let mut slope_btns = row![].spacing(4);
                for s in 0..3 {
                    let is_sel = slope == s;
                    slope_btns = slope_btns.push(
                        button(Text::new(slope_char(s)).size(11).font(bold_font()))
                            .on_press(pm(slope_msg(s))).padding(4)
                            .style(move |_t, st| {
                                let bg = if is_sel { Color::from_rgb(1.0, 0.45, 0.1) }
                                    else if st == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) }
                                    else { Color::from_rgb(0.15, 0.15, 0.15) };
                                button::Style { background: Some(bg.into()), text_color: Color::WHITE,
                                    border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default() }
                            })
                    );
                }

                let band_col = column![
                    Text::new(label).size(11).font(bold_font()).color(Color::from_rgb(0.85, 0.85, 0.85)),
                    hslider_gesture(-12.0, 12.0, gain, 0.0, move |g| pm(gain_msg(g))),
                    Text::new(format!("{:.1} dB", gain)).size(11).font(bold_font()).color(Color::from_rgb(0.8, 0.8, 0.8)),
                    hslider_gesture(fmin, fmax, freq, fdef, move |g| pm(freq_msg(g))),
                    Text::new(short_freq(freq)).size(10).font(bold_font()).color(Color::from_rgb(0.7, 0.85, 1.0)),
                    Text::new(if is_shelf { "Shelf Slope" } else { "Filter Q" }).size(10).font(bold_font()).color(Color::from_rgb(0.55, 0.55, 0.55)),
                    slope_btns,
                ].spacing(4).align_x(Alignment::Center).width(Length::FillPortion(1));
                eq_row = eq_row.push(band_col);
            }

            // Band labels (original style: Sub/Bass/Mid/Presence/Air)
            let band_labels = ["Sub", "Bass", "Mid", "Presence", "Air"];
            let hz_labels: Vec<Element<'_, Message<MeridianMsg>>> = band_labels.iter().map(|&l|
                container(Text::new(l).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)))
                    .center_x(Length::Fill).width(Length::FillPortion(1)).into()
            ).collect();

            container(column![
                top_strip,
                container(spectrum_canvas).height(Length::Fill).width(Length::Fill),
                row(hz_labels).height(Length::Fixed(15.0)).width(Length::Fill),
                container(eq_row).width(Length::Fill).padding(10),
            ].spacing(5)).width(Length::Fill).height(Length::Fill).into()
        };

        // ── Right Sidebar: only OUTPUT GAIN + AUTO LOUD + Output Meter + Goniometer ──
        let scope_write_pos = self.shared_state.scope_write_pos.load(Ordering::Acquire);
        let gonio = canvas(GoniometerCanvas {
            samples: self.shared_state.scope_samples.clone(),
            write_pos: scope_write_pos,
            correlation: self.phase_correlation,
        }).width(Length::Fill).height(Length::Fixed(139.0));

        let right_title = container(Text::new("OUTPUT LEVEL").size(12).font(bold_font()).color(Color::from_rgb(0.75, 0.75, 0.75)))
            .width(Length::Fill).padding(Padding { top: 2.0, right: 0.0, bottom: 4.0, left: 0.0 });

        let sidebar_controls = row![
            container(knob_gesture_bipolar("OUT GAIN", self.params.output_gain.raw_target() as f32, -12.0, 12.0, 0.0,
                move |g| pm(MeridianMsg::OutputGainGesture(g)))).width(Length::Fixed(60.0)),
            container(auto_loud_button(self.auto_loud_measuring, false, false, pm(MeridianMsg::AutoLoudTriggered)))
                .width(Length::Fill).center_x(Length::Fill),
        ].spacing(4).align_y(Alignment::Center);

        let out_block = output_level_block(self.peak_l, self.peak_r, self.peak_hold_l, self.peak_hold_r,
            self.peak_hold, pm(MeridianMsg::ResetPeak), self.balance, Length::Fill);

        let right = container(column![
            right_title, sidebar_controls, out_block,
            Text::new("GONIOMETER").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
            container(gonio).width(Length::Fill).height(Length::Fixed(139.0)),
        ].spacing(6))
        .width(Length::Fixed(155.0)).height(Length::Fill).padding(8)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()),
            border: Border { color: Color::from_rgb(0.18, 0.18, 0.18), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── Footer (110px): Compressor + Stereo/Routing + Reset All ──
        let comp_label = Text::new("COMPRESSOR").size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15));
        let gr_env = canvas(CompressorEnvelopeCanvas {
            history: self.gr_history.clone(),
            current: self.gain_reduction,
            peak_hold: self.gr_peak_hold,
        }).width(Length::Fixed(110.0)).height(Length::Fixed(60.0));

        let compressor_section = column![
            comp_label,
            row![
                knob_gesture("THRESH", self.params.comp_threshold.raw_target() as f32, -30.0, 0.0, 0.0, move |g| pm(MeridianMsg::CompThresholdGesture(g))),
                knob_gesture("MIX", self.params.comp_mix.raw_target() as f32, 0.0, 100.0, 0.0, move |g| pm(MeridianMsg::CompMixGesture(g))),
                knob_gesture("ATTACK", self.params.comp_attack.raw_target() as f32, 5.0, 50.0, 15.0, move |g| pm(MeridianMsg::CompAttackGesture(g))),
                knob_gesture("RELEASE", self.params.comp_release.raw_target() as f32, 50.0, 300.0, 120.0, move |g| pm(MeridianMsg::CompReleaseGesture(g))),
                knob_gesture("RATIO", self.params.comp_character.raw_target() as f32, 1.5, 4.0, 2.0, move |g| pm(MeridianMsg::CompCharacterGesture(g))),
                knob_gesture("MAKEUP", self.params.comp_makeup.raw_target() as f32, 0.0, 12.0, 0.0, move |g| pm(MeridianMsg::CompMakeupGesture(g))),
                row![
                    gr_env,
                    column![
                        Text::new(format!("PK: {:.1}", self.gr_peak_hold)).size(12).font(bold_font()).color(Color::from_rgb(1.0, 0.6, 0.2)),
                        Text::new(format!("GR: {:.1}", self.gain_reduction)).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.3, 0.3)),
                    ].spacing(1).align_x(Alignment::Center),
                ].spacing(6).align_y(Alignment::Center),
            ].spacing(14).align_y(Alignment::Center),
        ].spacing(4).align_x(Alignment::Center);

        let inflate_band_split_on = self.params.inflate_band_split.value();
        let inflate_clip_on = self.params.inflate_clip.value();
        let toggle_style = move |active: bool| {
            move |_t: &iced::Theme, st: button::Status| {
                let bg = if active { Color::from_rgb(1.0, 0.45, 0.1) }
                    else if st == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) }
                    else { Color::from_rgb(0.15, 0.15, 0.15) };
                button::Style { background: Some(bg.into()), text_color: Color::WHITE,
                    border: Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() }
            }
        };
        let inflate_toggles = column![
            button(Text::new("SPLIT").size(9).font(bold_font()))
                .on_press(pm(MeridianMsg::InflateBandSplitToggled))
                .padding(4)
                .width(Length::Fixed(48.0))
                .style(toggle_style(inflate_band_split_on)),
            button(Text::new("CLIP").size(9).font(bold_font()))
                .on_press(pm(MeridianMsg::InflateClipToggled))
                .padding(4)
                .width(Length::Fixed(48.0))
                .style(toggle_style(inflate_clip_on)),
        ].spacing(4);

        let inflate_section = column![
            Text::new("INFLATE").size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)),
            row![
                knob_gesture("EFFECT", self.params.inflate_effect.raw_target() as f32, 0.0, 100.0, 0.0, move |g| pm(MeridianMsg::InflateEffectGesture(g))),
                knob_gesture_bipolar("CURVE", self.params.inflate_curve.raw_target() as f32, -50.0, 50.0, 0.0, move |g| pm(MeridianMsg::InflateCurveGesture(g))),
                inflate_toggles,
            ].spacing(10).align_y(Alignment::Center),
        ].spacing(4).align_x(Alignment::Center);

        let stereo_section = column![
            Text::new("STEREO / ROUTING").size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)),
            row![
                knob_gesture_bipolar("PAN", self.params.pan.raw_target() as f32, -1.0, 1.0, 0.0, move |g| pm(MeridianMsg::PanGesture(g))),
                knob_gesture_bipolar("WIDTH", self.params.stereo_width.raw_target() as f32, 0.0, 200.0, 100.0, move |g| pm(MeridianMsg::StereoWidthGesture(g))),
            ].spacing(12).align_y(Alignment::Center),
        ].spacing(4).align_x(Alignment::Center);

        let tools = output_tools_strip(pm(MeridianMsg::ResetAll));

        let footer = container(row![
            container(compressor_section).padding(5),
            vsep(), container(inflate_section).padding(5),
            Space::new().width(Length::Fill),
            vsep(), container(stereo_section).padding(5),
            vsep(), container(tools).padding(5),
        ].align_y(Alignment::Center).spacing(15))
        .width(Length::Fill).height(Length::Fixed(110.0)).padding(8)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── Header ──
        let strip = monitor_strip(
            self.params.mono_active.value(), self.params.delta_active.value(), self.params.bypass_active.value(),
            pm(MeridianMsg::MonoToggled), pm(MeridianMsg::DeltaToggled), pm(MeridianMsg::BypassToggled));
        let header = container(row![
            container(header_brand("MERIDIAN", VERSION)).width(Length::Fill), strip
        ].align_y(Alignment::Center).spacing(10))
        .width(Length::Fill).height(Length::Fixed(50.0)).padding(10)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── Assembly ──
        let main_body = row![
            sidebar, container(center).width(Length::Fill).height(Length::Fill), right
        ].height(Length::Fill).width(Length::Fill);

        let body = column![header, main_body, footer].width(Length::Fill).height(Length::Fill);

        container(body).width(Length::Fill).height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Color::from_rgb(0.06, 0.06, 0.06).into()),
                text_color: Some(Color::WHITE), ..Default::default()
            }).into()
    }
}



// ─── Message Handler ─────────────────────────────────────────────────────────

impl MeridianEditor {
    fn handle_msg(&mut self, msg: &MeridianMsg, ctx: &PluginContext<MeridianParams>) {
        match msg {
            MeridianMsg::Tick => self.do_tick(ctx),
            MeridianMsg::HpfGesture(g) => self.do_gesture(|p| &p.hpf_freq, g, ctx),
            MeridianMsg::LpfGesture(g) => self.do_gesture(|p| &p.lpf_freq, g, ctx),
            MeridianMsg::CutSlopeChanged(v) => { self.cut_slope_sel = *v; self.do_int(|p| &p.cut_slope, *v, ctx); }
            MeridianMsg::BassGainGesture(g) => self.do_gesture(|p| &p.bass_gain, g, ctx),
            MeridianMsg::BassSlopeChanged(v) => { self.slope_sel[0] = *v; self.do_int(|p| &p.bass_slope, *v, ctx); }
            MeridianMsg::LoMidGainGesture(g) => self.do_gesture(|p| &p.lo_mid_gain, g, ctx),
            MeridianMsg::LoMidSlopeChanged(v) => { self.slope_sel[1] = *v; self.do_int(|p| &p.lo_mid_slope, *v, ctx); }
            MeridianMsg::MidGainGesture(g) => self.do_gesture(|p| &p.mid_gain, g, ctx),
            MeridianMsg::MidSlopeChanged(v) => { self.slope_sel[2] = *v; self.do_int(|p| &p.mid_slope, *v, ctx); }
            MeridianMsg::HighGainGesture(g) => self.do_gesture(|p| &p.high_gain, g, ctx),
            MeridianMsg::HighSlopeChanged(v) => { self.slope_sel[3] = *v; self.do_int(|p| &p.high_slope, *v, ctx); }
            MeridianMsg::ExciteGainGesture(g) => self.do_gesture(|p| &p.excite_gain, g, ctx),
            MeridianMsg::ExciteSlopeChanged(v) => { self.slope_sel[4] = *v; self.do_int(|p| &p.excite_slope, *v, ctx); }
            MeridianMsg::EqFreq1Gesture(g) => self.do_gesture(|p| &p.eq_freq_1, g, ctx),
            MeridianMsg::EqFreq2Gesture(g) => self.do_gesture(|p| &p.eq_freq_2, g, ctx),
            MeridianMsg::EqFreq3Gesture(g) => self.do_gesture(|p| &p.eq_freq_3, g, ctx),
            MeridianMsg::EqFreq4Gesture(g) => self.do_gesture(|p| &p.eq_freq_4, g, ctx),
            MeridianMsg::EqFreq5Gesture(g) => self.do_gesture(|p| &p.eq_freq_5, g, ctx),
            MeridianMsg::TiltGainGesture(g) => self.do_gesture(|p| &p.tilt_gain, g, ctx),
            MeridianMsg::WarmthDriveGesture(g) => self.do_gesture(|p| &p.warmth_drive, g, ctx),
            MeridianMsg::WarmthMixGesture(g) => self.do_gesture(|p| &p.warmth_mix, g, ctx),
            MeridianMsg::ExciteAmountGesture(g) => self.do_gesture(|p| &p.excite_amount, g, ctx),
            MeridianMsg::ExciteBlendGesture(g) => self.do_gesture(|p| &p.excite_blend, g, ctx),
            MeridianMsg::ExciteFreqGesture(g) => self.do_gesture(|p| &p.excite_freq, g, ctx),
            MeridianMsg::CompThresholdGesture(g) => self.do_gesture(|p| &p.comp_threshold, g, ctx),
            MeridianMsg::CompMixGesture(g) => self.do_gesture(|p| &p.comp_mix, g, ctx),
            MeridianMsg::CompAttackGesture(g) => self.do_gesture(|p| &p.comp_attack, g, ctx),
            MeridianMsg::CompReleaseGesture(g) => self.do_gesture(|p| &p.comp_release, g, ctx),
            MeridianMsg::CompCharacterGesture(g) => self.do_gesture(|p| &p.comp_character, g, ctx),
            MeridianMsg::CompMakeupGesture(g) => self.do_gesture(|p| &p.comp_makeup, g, ctx),
            MeridianMsg::InflateEffectGesture(g) => self.do_gesture(|p| &p.inflate_effect, g, ctx),
            MeridianMsg::InflateCurveGesture(g) => self.do_gesture(|p| &p.inflate_curve, g, ctx),
            MeridianMsg::InflateBandSplitToggled => self.do_toggle(|p| &p.inflate_band_split, ctx),
            MeridianMsg::InflateClipToggled => self.do_toggle(|p| &p.inflate_clip, ctx),
            MeridianMsg::StereoWidthGesture(g) => self.do_gesture(|p| &p.stereo_width, g, ctx),
            MeridianMsg::PanGesture(g) => self.do_gesture(|p| &p.pan, g, ctx),
            MeridianMsg::OutputGainGesture(g) => self.do_gesture(|p| &p.output_gain, g, ctx),
            MeridianMsg::MonoToggled => self.do_toggle(|p| &p.mono_active, ctx),
            MeridianMsg::DeltaToggled => self.do_toggle(|p| &p.delta_active, ctx),
            MeridianMsg::BypassToggled => self.do_toggle(|p| &p.bypass_active, ctx),
            MeridianMsg::AutoLoudTriggered => { self.shared_state.auto_loud_trigger.store(true, Ordering::Release); }
            MeridianMsg::ResetAll => self.do_reset_all(ctx),
            MeridianMsg::SelectPreset(idx) => {
                if *idx < self.presets.len() {
                    self.selected_preset_index = Some(*idx);
                    let prof = &self.presets[*idx].2;
                    apply_profile(ctx, &self.params, prof);
                    self.slope_sel = [prof.bass_slope, prof.lo_mid_slope, prof.mid_slope, prof.high_slope, prof.excite_slope];
                    self.cut_slope_sel = prof.cut_slope;
                }
            }
            MeridianMsg::PresetNameChanged(s) => self.preset_name_input = s.clone(),
            MeridianMsg::SavePreset => self.do_save_preset(),
            MeridianMsg::SetupToggled => {
                self.show_setup = !self.show_setup;
                if self.show_setup { self.vault_path_input = self.vault_path.clone().unwrap_or_default(); }
            }
            MeridianMsg::VaultPathChanged(s) => self.vault_path_input = s.clone(),
            MeridianMsg::SaveVaultPath => self.do_save_vault_path(),
            MeridianMsg::ResetPeak => {
                self.shared_state.reset_peak.store(true, Ordering::Release);
                self.shared_state.peak_hold.store(-100.0, Ordering::Release);
                self.shared_state.peak_hold_l.store(-100.0, Ordering::Release);
                self.shared_state.peak_hold_r.store(-100.0, Ordering::Release);
            }
            MeridianMsg::SnapPressed => {
                if self.vault_path.as_ref().is_none_or(|v| v.is_empty()) { self.show_setup = true; return; }
                if self.shared_state.snap_active.load(Ordering::Acquire) { return; }
                self.shared_state.snap_active.store(true, Ordering::Release);
                self.shared_state.snap_phase.store(1, Ordering::Release);
            }
        }
    }

    fn do_tick(&mut self, _ctx: &PluginContext<MeridianParams>) {
        for b in 0..5 { self.band_levels[b] = self.shared_state.band_levels[b].load(Ordering::Acquire); }
        self.phase_correlation = self.shared_state.phase_correlation.load(Ordering::Acquire);
        self.balance = self.shared_state.balance.load(Ordering::Acquire);
        self.output_peak = self.shared_state.output_peak.load(Ordering::Acquire);
        self.peak_hold = self.shared_state.peak_hold.load(Ordering::Acquire);
        self.peak_l = self.shared_state.output_peak_l.load(Ordering::Acquire);
        self.peak_r = self.shared_state.output_peak_r.load(Ordering::Acquire);
        self.peak_hold_l = self.shared_state.peak_hold_l.load(Ordering::Acquire);
        self.peak_hold_r = self.shared_state.peak_hold_r.load(Ordering::Acquire);
        self.gain_reduction = self.shared_state.gain_reduction.load(Ordering::Acquire);
        self.gr_history.push(self.gain_reduction);
        if self.gr_history.len() > 90 { self.gr_history.remove(0); }
        if self.gain_reduction > self.gr_peak_hold {
            self.gr_peak_hold = self.gain_reduction; self.gr_peak_hold_ticks = 90;
        } else if self.gr_peak_hold_ticks > 0 { self.gr_peak_hold_ticks -= 1; }
        else { self.gr_peak_hold = (self.gr_peak_hold - 0.15).max(self.gain_reduction).max(0.0); }
        self.slope_sel = [
            self.params.bass_slope.value() as i32, self.params.lo_mid_slope.value() as i32,
            self.params.mid_slope.value() as i32, self.params.high_slope.value() as i32, self.params.excite_slope.value() as i32,
        ];
        self.cut_slope_sel = self.params.cut_slope.value() as i32;
        self.auto_loud_measuring = self.shared_state.auto_loud_measuring.load(Ordering::Acquire);
        let snap_now = self.shared_state.snap_active.load(Ordering::Acquire);
        let was_snap = self.snap_active;
        self.snap_active = snap_now;
        if self.snap_active { self.snap_blink_counter = self.snap_blink_counter.wrapping_add(1); }
        else if was_snap {
            self.snap_blink_counter = 0;
            if let Some(ref vp) = self.vault_path {
                if !vp.is_empty() {
                    let stereo = self.shared_state.snap_stereo_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let mono = self.shared_state.snap_mono_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let delta = self.shared_state.snap_delta_snap.lock().ok().map(|v| v.clone()).unwrap_or_else(|| vec![-90.0; 1024]);
                    let sr = self.shared_state.sample_rate.load(Ordering::Acquire);
                    let md = snap_markdown(&stereo, &mono, &delta, self.band_levels, self.phase_correlation, self.peak_l, self.peak_r, sr);
                    let fname = snap_filename(vp);
                    let _ = std::fs::write(std::path::Path::new(vp).join(&fname), &md);
                }
            }
        }
        let measuring = self.shared_state.auto_loud_measuring.load(Ordering::Acquire);
        if !measuring {
            let offset = self.shared_state.auto_loud_gain_offset.load(Ordering::Acquire);
            if offset.abs() > 0.05 {
                let current = self.params.output_gain.raw_target() as f32;
                let new_val = (current + offset).clamp(-12.0, 12.0);
                self.params.output_gain.set_value(new_val as f64);
                self.shared_state.auto_loud_gain_offset.store(0.0, Ordering::Release);
            }
        }
        self.preset_refresh_counter += 1;
        if self.preset_refresh_counter >= 150 {
            self.preset_refresh_counter = 0;
            let refreshed = list_meridian_presets(self.vault_path.as_deref());
            self.presets.retain(|(_, path, _)| path.is_none());
            self.presets.extend(refreshed.into_iter().map(|(n, p, profile)| (n, Some(p), profile)));
        }
        if self.preset_warning.is_some() {
            self.preset_warning_ticks += 1;
            if self.preset_warning_ticks >= 200 { self.preset_warning = None; self.preset_warning_ticks = 0; }
        }
    }

    fn do_gesture(&mut self, param: fn(&MeridianParams) -> &FloatParam, g: &Gesture, _ctx: &PluginContext<MeridianParams>) {
        let p = param(&self.params);
        match g {
            Gesture::Start => {}
            Gesture::Change(v) => p.set_value(*v as f64),
            Gesture::End => {}
        }
    }

    fn do_int(&mut self, param: fn(&MeridianParams) -> &IntParam, val: i32, _ctx: &PluginContext<MeridianParams>) {
        param(&self.params).set_value(val as i64);
    }

    fn do_toggle(&mut self, param: fn(&MeridianParams) -> &BoolParam, _ctx: &PluginContext<MeridianParams>) {
        let p = param(&self.params);
        p.set_value(!p.value());
    }

    fn do_reset_all(&mut self, _ctx: &PluginContext<MeridianParams>) {
        let p = &self.params;
        p.hpf_freq.set_value(2.0); p.lpf_freq.set_value(35000.0);
        p.cut_slope.set_value(0i64); self.cut_slope_sel = 0;
        p.bass_gain.set_value(0.0); p.bass_slope.set_value(1i64);
        p.lo_mid_gain.set_value(0.0); p.lo_mid_slope.set_value(1i64);
        p.mid_gain.set_value(0.0); p.mid_slope.set_value(1i64);
        p.high_gain.set_value(0.0); p.high_slope.set_value(1i64);
        p.excite_gain.set_value(0.0); p.excite_slope.set_value(1i64);
        p.eq_freq_1.set_value(80.0); p.eq_freq_2.set_value(300.0); p.eq_freq_3.set_value(1000.0);
        p.eq_freq_4.set_value(4000.0); p.eq_freq_5.set_value(12000.0);
        p.comp_threshold.set_value(0.0); p.comp_mix.set_value(40.0); p.comp_attack.set_value(15.0);
        p.comp_release.set_value(120.0); p.comp_character.set_value(2.0); p.comp_makeup.set_value(0.0);
        p.inflate_effect.set_value(0.0); p.inflate_curve.set_value(0.0);
        p.inflate_band_split.set_value(false); p.inflate_clip.set_value(false);
        p.tilt_gain.set_value(0.0); p.warmth_drive.set_value(0.0); p.warmth_mix.set_value(0.0);
        p.excite_amount.set_value(0.0); p.excite_blend.set_value(0.0); p.excite_freq.set_value(8000.0);
        p.stereo_width.set_value(100.0); p.pan.set_value(0.0); p.output_gain.set_value(0.0);
        self.shared_state.reset_analysis.store(true, Ordering::Release);
    }

    fn do_save_preset(&mut self) {
        let name = if self.preset_name_input.trim().is_empty() {
            format!("User Preset {}", self.presets.len() + 1)
        } else { self.preset_name_input.trim().to_string() };
        let p = MeridianProfile {
            name: name.clone(),
            hpf_freq: self.params.hpf_freq.raw_target() as f32,
            lpf_freq: self.params.lpf_freq.raw_target() as f32,
            cut_slope: self.params.cut_slope.value() as i32,
            bass_gain: self.params.bass_gain.raw_target() as f32,
            bass_slope: self.params.bass_slope.value() as i32,
            lo_mid_gain: self.params.lo_mid_gain.raw_target() as f32,
            lo_mid_slope: self.params.lo_mid_slope.value() as i32,
            mid_gain: self.params.mid_gain.raw_target() as f32,
            mid_slope: self.params.mid_slope.value() as i32,
            high_gain: self.params.high_gain.raw_target() as f32,
            high_slope: self.params.high_slope.value() as i32,
            excite_gain: self.params.excite_gain.raw_target() as f32,
            excite_slope: self.params.excite_slope.value() as i32,
            eq_freq_1: self.params.eq_freq_1.raw_target() as f32,
            eq_freq_2: self.params.eq_freq_2.raw_target() as f32,
            eq_freq_3: self.params.eq_freq_3.raw_target() as f32,
            eq_freq_4: self.params.eq_freq_4.raw_target() as f32,
            eq_freq_5: self.params.eq_freq_5.raw_target() as f32,
            tilt_gain: self.params.tilt_gain.raw_target() as f32,
            warmth_drive: self.params.warmth_drive.raw_target() as f32,
            warmth_mix: self.params.warmth_mix.raw_target() as f32,
            excite_amount: self.params.excite_amount.raw_target() as f32,
            excite_blend: self.params.excite_blend.raw_target() as f32,
            excite_freq: self.params.excite_freq.raw_target() as f32,
            comp_threshold: self.params.comp_threshold.raw_target() as f32,
            comp_mix: self.params.comp_mix.raw_target() as f32,
            comp_attack: self.params.comp_attack.raw_target() as f32,
            comp_release: self.params.comp_release.raw_target() as f32,
            comp_character: self.params.comp_character.raw_target() as f32,
            comp_makeup: self.params.comp_makeup.raw_target() as f32,
            inflate_effect: self.params.inflate_effect.raw_target() as f32,
            inflate_curve: self.params.inflate_curve.raw_target() as f32,
            inflate_band_split: self.params.inflate_band_split.value(),
            inflate_clip: self.params.inflate_clip.value(),
            stereo_width: self.params.stereo_width.raw_target() as f32,
            pan: self.params.pan.raw_target() as f32,
            output_gain: self.params.output_gain.raw_target() as f32,
        };
        let preset_dir = if let Some(ref vp) = self.vault_path {
            if !vp.is_empty() { PathBuf::from(vp) }
            else { shared_analysis::get_plugin_dir("Meridian").join("presets") }
        } else { shared_analysis::get_plugin_dir("Meridian").join("presets") };
        let _ = std::fs::create_dir_all(&preset_dir);
        let safe_name = p.name.replace(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_', "");
        let file_path = preset_dir.join(format!("{}.md", safe_name));
        let md = export_meridian_markdown(&p);
        if std::fs::write(&file_path, md).is_ok() {
            if let Some(existing) = self.presets.iter().position(|pr| pr.1.as_ref() == Some(&file_path)) {
                self.presets[existing] = (p.name.clone(), Some(file_path), p);
                self.selected_preset_index = Some(existing);
            } else {
                self.presets.push((p.name.clone(), Some(file_path), p));
                self.selected_preset_index = Some(self.presets.len() - 1);
            }
        }
        self.preset_name_input.clear();
    }

    fn do_save_vault_path(&mut self) {
        let new_path = if self.vault_path_input.trim().is_empty() { None }
            else { Some(self.vault_path_input.trim().to_string()) };
        let config = shared_analysis::PluginConfig { vault_path: new_path.clone(), ..Default::default() };
        if shared_analysis::save_config("Meridian", &config).is_ok() {
            self.vault_path = new_path; self.show_setup = false;
            let local = list_meridian_presets(self.vault_path.as_deref());
            self.presets.clear();
            for (n, p, prof) in local { self.presets.push((n, Some(p), prof)); }
            self.selected_preset_index = if self.presets.is_empty() { None } else { Some(0) };
        }
    }
}

// ─── EQ Curve ───────────────────────────────────────────────────────────────

/// Compute the EQ transfer function (amber overlay curve) from current Biquad filter
/// parameters. Returns `None` if sample rate is invalid.
fn compute_eq_curve(params: &MeridianParams, slope_sel: [i32; 5], cut_slope_sel: i32, sr: f32) -> Option<EqOverlay> {
    if sr < 1.0 { return None; }
    const N: usize = 256;
    let slope_val = |s: i32| -> f32 { match s { 0 => 0.5, 1 => 1.0, _ => 2.0 } };
    let q_val     = |s: i32| -> f32 { match s { 0 => 0.4, 1 => 0.7, _ => 1.5 } };

    let mut hpf = Biquad::new(); let mut lpf = Biquad::new();
    let mut hpf2 = Biquad::new(); let mut lpf2 = Biquad::new();
    let mut bass = Biquad::new(); let mut lo_mid = Biquad::new();
    let mut mid = Biquad::new(); let mut high = Biquad::new(); let mut excite = Biquad::new();
    let mut tilt_lo = Biquad::new(); let mut tilt_hi = Biquad::new();

    let hpf_f = params.hpf_freq.raw_target() as f32;
    let lpf_f = params.lpf_freq.raw_target() as f32;
    if cut_slope_sel >= 1 {
        const Q1: f32 = 0.541_196_1; const Q2: f32 = 1.306_563;
        hpf.set_butterworth_hp_q(hpf_f, Q1, sr); hpf2.set_butterworth_hp_q(hpf_f, Q2, sr);
        lpf.set_butterworth_lp_q(lpf_f, Q1, sr); lpf2.set_butterworth_lp_q(lpf_f, Q2, sr);
    } else {
        hpf.set_butterworth_hp(hpf_f, sr); lpf.set_butterworth_lp(lpf_f, sr);
        hpf2.set_identity(); lpf2.set_identity();
    }

    bass.set_low_shelf(params.eq_freq_1.raw_target() as f32, params.bass_gain.raw_target() as f32, slope_val(slope_sel[0]), sr);
    lo_mid.set_peaking_eq(params.eq_freq_2.raw_target() as f32, params.lo_mid_gain.raw_target() as f32, q_val(slope_sel[1]), sr);
    mid.set_peaking_eq(params.eq_freq_3.raw_target() as f32, params.mid_gain.raw_target() as f32, q_val(slope_sel[2]), sr);
    high.set_peaking_eq(params.eq_freq_4.raw_target() as f32, params.high_gain.raw_target() as f32, q_val(slope_sel[3]), sr);
    excite.set_high_shelf(params.eq_freq_5.raw_target() as f32, params.excite_gain.raw_target() as f32, slope_val(slope_sel[4]), sr);
    let tilt_db = params.tilt_gain.raw_target() as f32;
    tilt_lo.set_low_shelf(1000.0, tilt_db, 1.0, sr);
    tilt_hi.set_high_shelf(1000.0, -tilt_db, 1.0, sr);

    let points: Vec<(f32, f32)> = (0..N).map(|i| {
        let t = i as f32 / (N - 1) as f32;
        let freq = 20.0f32 * 1000.0f32.powf(t);
        let db = hpf.magnitude_db(freq, sr) + hpf2.magnitude_db(freq, sr)
            + lpf.magnitude_db(freq, sr) + lpf2.magnitude_db(freq, sr)
            + bass.magnitude_db(freq, sr) + lo_mid.magnitude_db(freq, sr)
            + mid.magnitude_db(freq, sr) + high.magnitude_db(freq, sr)
            + excite.magnitude_db(freq, sr) + tilt_lo.magnitude_db(freq, sr)
            + tilt_hi.magnitude_db(freq, sr);
        (t, db.clamp(-24.0, 24.0))
    }).collect();

    Some(EqOverlay {
        points,
        min_db: -24.0, max_db: 24.0,
        line_color: Color::from_rgba(1.0, 0.55, 0.05, 0.9),
        fill_alpha: 0.15,
        grid_db: vec![-24.0, -18.0, -12.0, -6.0, 0.0, 6.0, 12.0, 18.0, 24.0],
    })
}

// ─── Preset helpers ──────────────────────────────────────────────────────────

fn apply_profile(_ctx: &PluginContext<MeridianParams>, params: &MeridianParams, profile: &MeridianProfile) {
    params.hpf_freq.set_value(profile.hpf_freq as f64);
    params.lpf_freq.set_value(profile.lpf_freq as f64);
    params.cut_slope.set_value(profile.cut_slope as i64);
    params.bass_gain.set_value(profile.bass_gain as f64);
    params.bass_slope.set_value(profile.bass_slope as i64);
    params.lo_mid_gain.set_value(profile.lo_mid_gain as f64);
    params.lo_mid_slope.set_value(profile.lo_mid_slope as i64);
    params.mid_gain.set_value(profile.mid_gain as f64);
    params.mid_slope.set_value(profile.mid_slope as i64);
    params.high_gain.set_value(profile.high_gain as f64);
    params.high_slope.set_value(profile.high_slope as i64);
    params.excite_gain.set_value(profile.excite_gain as f64);
    params.excite_slope.set_value(profile.excite_slope as i64);
    params.eq_freq_1.set_value(profile.eq_freq_1 as f64);
    params.eq_freq_2.set_value(profile.eq_freq_2 as f64);
    params.eq_freq_3.set_value(profile.eq_freq_3 as f64);
    params.eq_freq_4.set_value(profile.eq_freq_4 as f64);
    params.eq_freq_5.set_value(profile.eq_freq_5 as f64);
    params.tilt_gain.set_value(profile.tilt_gain as f64);
    params.warmth_drive.set_value(profile.warmth_drive as f64);
    params.warmth_mix.set_value(profile.warmth_mix as f64);
    params.excite_amount.set_value(profile.excite_amount as f64);
    params.excite_blend.set_value(profile.excite_blend as f64);
    params.excite_freq.set_value(profile.excite_freq as f64);
    params.comp_threshold.set_value(profile.comp_threshold as f64);
    params.comp_mix.set_value(profile.comp_mix as f64);
    params.comp_attack.set_value(profile.comp_attack as f64);
    params.comp_release.set_value(profile.comp_release as f64);
    params.comp_character.set_value(profile.comp_character as f64);
    params.comp_makeup.set_value(profile.comp_makeup as f64);
    params.inflate_effect.set_value(profile.inflate_effect as f64);
    params.inflate_curve.set_value(profile.inflate_curve as f64);
    params.inflate_band_split.set_value(profile.inflate_band_split);
    params.inflate_clip.set_value(profile.inflate_clip);
    params.stereo_width.set_value(profile.stereo_width as f64);
    params.pan.set_value(profile.pan as f64);
    params.output_gain.set_value(profile.output_gain as f64);
}

fn export_meridian_markdown(p: &MeridianProfile) -> String {
    let mut s = String::new();
    s.push_str("---\nplugin: meridian\ntype: preset\n---\n\n");
    s.push_str("> Warning: Do NOT modify column names or table structure.\n\n");
    s.push_str("## Parameter\n\n| Parameter | Wert | Einheit |\n|---|---|---|\n");
    s.push_str(&format!("| HPF | {:.1} | Hz |\n", p.hpf_freq));
    s.push_str(&format!("| LPF | {:.1} | Hz |\n", p.lpf_freq));
    s.push_str(&format!("| Cut Slope | {} | |\n", if p.cut_slope >= 1 { "B" } else { "A" }));
    s.push_str(&format!("| Bass Gain | {:.1} | dB |\n", p.bass_gain));
    s.push_str(&format!("| Bass Slope | {} | |\n", slope_char(p.bass_slope)));
    s.push_str(&format!("| EQ Freq 1 | {:.0} | Hz |\n", p.eq_freq_1));
    s.push_str(&format!("| Lo-Mid Gain | {:.1} | dB |\n", p.lo_mid_gain));
    s.push_str(&format!("| Lo-Mid Slope | {} | |\n", slope_char(p.lo_mid_slope)));
    s.push_str(&format!("| EQ Freq 2 | {:.0} | Hz |\n", p.eq_freq_2));
    s.push_str(&format!("| Mid Gain | {:.1} | dB |\n", p.mid_gain));
    s.push_str(&format!("| Mid Slope | {} | |\n", slope_char(p.mid_slope)));
    s.push_str(&format!("| EQ Freq 3 | {:.0} | Hz |\n", p.eq_freq_3));
    s.push_str(&format!("| High Gain | {:.1} | dB |\n", p.high_gain));
    s.push_str(&format!("| High Slope | {} | |\n", slope_char(p.high_slope)));
    s.push_str(&format!("| EQ Freq 4 | {:.0} | Hz |\n", p.eq_freq_4));
    s.push_str(&format!("| Excite Gain | {:.1} | dB |\n", p.excite_gain));
    s.push_str(&format!("| Excite Slope | {} | |\n", slope_char(p.excite_slope)));
    s.push_str(&format!("| EQ Freq 5 | {:.0} | Hz |\n", p.eq_freq_5));
    s.push_str(&format!("| Comp Threshold | {:.1} | dB |\n", p.comp_threshold));
    s.push_str(&format!("| Comp Mix | {:.1} | % |\n", p.comp_mix));
    s.push_str(&format!("| Comp Attack | {:.1} | ms |\n", p.comp_attack));
    s.push_str(&format!("| Comp Release | {:.1} | ms |\n", p.comp_release));
    s.push_str(&format!("| Comp Character | {:.1} | |\n", p.comp_character));
    s.push_str(&format!("| Comp Makeup | {:.1} | dB |\n", p.comp_makeup));
    s.push_str(&format!("| Inflate Effect | {:.1} | % |\n", p.inflate_effect));
    s.push_str(&format!("| Inflate Curve | {:.1} | |\n", p.inflate_curve));
    s.push_str(&format!("| Inflate Band Split | {} | |\n", if p.inflate_band_split { "On" } else { "Off" }));
    s.push_str(&format!("| Inflate Clip | {} | |\n", if p.inflate_clip { "On" } else { "Off" }));
    s.push_str(&format!("| Warmth Drive | {:.1} | dB |\n", p.warmth_drive));
    s.push_str(&format!("| Warmth Mix | {:.1} | % |\n", p.warmth_mix));
    s.push_str(&format!("| Excite Amount | {:.1} | % |\n", p.excite_amount));
    s.push_str(&format!("| Excite Blend | {:.1} | % |\n", p.excite_blend));
    s.push_str(&format!("| Excite Freq | {:.0} | Hz |\n", p.excite_freq));
    s.push_str(&format!("| Tilt | {:.1} | dB |\n", p.tilt_gain));
    s.push_str(&format!("| Stereo Width | {:.1} | % |\n", p.stereo_width));
    s.push_str(&format!("| Pan | {:.2} | |\n", p.pan));
    s.push_str(&format!("| Output Gain | {:.1} | dB |\n", p.output_gain));
    s
}

fn parse_meridian_markdown(content: &str) -> Option<MeridianProfile> {
    match shared_analysis::preset_plugin_name(content).as_deref() {
        Some("meridian") => {}
        _ => return None,
    }
    let mut p = MeridianProfile::default();
    let mut has_hpf = false; let mut has_lpf = false;
    let mut has_bass = false; let mut has_mid = false; let mut has_output = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') {
            let parts: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
            if parts.len() >= 4 {
                match parts[1].to_lowercase().as_str() {
                    "hpf" => { if let Ok(v) = parts[2].parse() { p.hpf_freq = v; has_hpf = true; } }
                    "lpf" => { if let Ok(v) = parts[2].parse() { p.lpf_freq = v; has_lpf = true; } }
                    "cut slope" => { p.cut_slope = if parts[2] == "B" { 1 } else { 0 }; }
                    "bass gain" => { if let Ok(v) = parts[2].parse() { p.bass_gain = v; has_bass = true; } }
                    "bass slope" => { p.bass_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 1" => { if let Ok(v) = parts[2].parse() { p.eq_freq_1 = v; } }
                    "lo-mid gain" => { if let Ok(v) = parts[2].parse() { p.lo_mid_gain = v; } }
                    "lo-mid slope" => { p.lo_mid_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 2" => { if let Ok(v) = parts[2].parse() { p.eq_freq_2 = v; } }
                    "mid gain" => { if let Ok(v) = parts[2].parse() { p.mid_gain = v; has_mid = true; } }
                    "mid slope" => { p.mid_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 3" => { if let Ok(v) = parts[2].parse() { p.eq_freq_3 = v; } }
                    "high gain" => { if let Ok(v) = parts[2].parse() { p.high_gain = v; } }
                    "high slope" => { p.high_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 4" => { if let Ok(v) = parts[2].parse() { p.eq_freq_4 = v; } }
                    "excite gain" => { if let Ok(v) = parts[2].parse() { p.excite_gain = v; } }
                    "excite slope" => { p.excite_slope = match parts[2] { "A" => 0, "B" => 1, "C" => 2, _ => 1 }; }
                    "eq freq 5" => { if let Ok(v) = parts[2].parse() { p.eq_freq_5 = v; } }
                    "comp threshold" => { if let Ok(v) = parts[2].parse() { p.comp_threshold = v; } }
                    "comp mix" => { if let Ok(v) = parts[2].parse() { p.comp_mix = v; } }
                    "comp attack" => { if let Ok(v) = parts[2].parse() { p.comp_attack = v; } }
                    "comp release" => { if let Ok(v) = parts[2].parse() { p.comp_release = v; } }
                    "comp character" => { if let Ok(v) = parts[2].parse() { p.comp_character = v; } }
                    "comp makeup" => { if let Ok(v) = parts[2].parse() { p.comp_makeup = v; } }
                    "inflate effect" => { if let Ok(v) = parts[2].parse() { p.inflate_effect = v; } }
                    "inflate curve" => { if let Ok(v) = parts[2].parse() { p.inflate_curve = v; } }
                    "inflate band split" => { p.inflate_band_split = parts[2] == "On"; }
                    "inflate clip" => { p.inflate_clip = parts[2] == "On"; }
                    "warmth drive" => { if let Ok(v) = parts[2].parse() { p.warmth_drive = v; } }
                    "warmth mix" => { if let Ok(v) = parts[2].parse() { p.warmth_mix = v; } }
                    "excite amount" => { if let Ok(v) = parts[2].parse() { p.excite_amount = v; } }
                    "excite blend" => { if let Ok(v) = parts[2].parse() { p.excite_blend = v; } }
                    "excite freq" => { if let Ok(v) = parts[2].parse() { p.excite_freq = v; } }
                    "tilt" => { if let Ok(v) = parts[2].parse() { p.tilt_gain = v; } }
                    "stereo width" => { if let Ok(v) = parts[2].parse() { p.stereo_width = v; } }
                    "pan" => { if let Ok(v) = parts[2].parse() { p.pan = v; } }
                    "output gain" => { if let Ok(v) = parts[2].parse() { p.output_gain = v; has_output = true; } }
                    _ => {}
                }
            }
        }
    }
    if has_hpf && has_lpf && has_bass && has_mid && has_output { Some(p) } else { None }
}

fn list_meridian_presets(vault_path: Option<&str>) -> Vec<(String, PathBuf, MeridianProfile)> {
    let mut presets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let local_dir = shared_analysis::get_plugin_dir("Meridian").join("presets");
    let _ = std::fs::create_dir_all(&local_dir);
    let mut scan = |dir: &std::path::Path| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                if path.is_file() && path.extension().is_some_and(|e| e == "md")
                    && !stem.starts_with("SNAPSHOT-") && seen.insert(path.clone()) {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        match shared_analysis::preset_plugin_name(&content).as_deref() {
                            Some("meridian") => {}
                            _ => continue,
                        }
                        if let Some(mut prof) = parse_meridian_markdown(&content) {
                            prof.name = stem.clone();
                            presets.push((stem, path, prof));
                        }
                    }
                }
            }
        }
    };
    scan(&local_dir);
    if let Some(vp) = vault_path { if !vp.is_empty() { scan(std::path::Path::new(vp)); } }
    presets
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
        "---\nplugin: meridian\ntype: snapshot\n---\n\n# Meridian Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n\n\
        ## 5-Band\n| Band | Pegel |\n|------|-------|\n\
        | Sub | {b0:.1} dB |\n| Bass | {b1:.1} dB |\n| Mid | {b2:.1} dB |\n| Presence | {b3:.1} dB |\n| Air | {b4:.1} dB |\n",
        pl=pl, pr=pr, co=corr, st=tbl(stereo), mn=tbl(mono), dt=tbl(delta),
        b0=band_levels[0], b1=band_levels[1], b2=band_levels[2], b3=band_levels[3], b4=band_levels[4],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Old-format preset without any Inflate rows must still load, falling back
    /// to MeridianProfile::default() (effect=0, curve=0, split=off, clip=off).
    #[test]
    fn old_preset_without_inflate_rows_loads_with_defaults() {
        let old_md = "---\nplugin: meridian\ntype: preset\n---\n\n\
            | Parameter | Wert | Einheit |\n|---|---|---|\n\
            | HPF | 2.0 | Hz |\n| LPF | 35000.0 | Hz |\n\
            | Bass Gain | 1.5 | dB |\n| Mid Gain | -2.0 | dB |\n\
            | Output Gain | 0.0 | dB |\n";
        let profile = parse_meridian_markdown(old_md).expect("old preset must still parse");
        assert_eq!(profile.inflate_effect, 0.0);
        assert_eq!(profile.inflate_curve, 0.0);
        assert!(!profile.inflate_band_split);
        assert!(!profile.inflate_clip);
        // Re-exporting an old preset must extend it with the new rows.
        let extended = export_meridian_markdown(&profile);
        assert!(extended.contains("| Inflate Effect | 0.0 | % |"));
        assert!(extended.contains("| Inflate Clip | Off | |"));
    }

    /// Round-trip: export then parse must preserve Inflate values exactly.
    #[test]
    fn inflate_roundtrips_through_markdown() {
        let mut p = MeridianProfile::default();
        p.inflate_effect = 42.0;
        p.inflate_curve = -25.0;
        p.inflate_band_split = true;
        p.inflate_clip = false;
        let md = export_meridian_markdown(&p);
        let parsed = parse_meridian_markdown(&md).expect("exported preset must parse");
        assert_eq!(parsed.inflate_effect, 42.0);
        assert_eq!(parsed.inflate_curve, -25.0);
        assert!(parsed.inflate_band_split);
        assert!(!parsed.inflate_clip);
    }
}
