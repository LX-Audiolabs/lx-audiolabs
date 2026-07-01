// Aurum editor — Meridian/Equilibrium layout pattern (truce port).
// Layout: Left sidebar (Presets/SNAP/Vault) | Main (tabs + horizontal strips) | Right bar (Goniometer/Output) | Footer (AT/Stereo/Gain/Reset)

use truce_iced::iced::widget::{button, canvas, column, container, row, Space, Text};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Subscription};
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use shared_analysis::SharedState;
use shared_ui::{
    bold_font, header_brand, toggle_button, knob_gesture, knob_gesture_bipolar,
    output_tools_strip, output_level_block, at_block, vault_setup_box,
    GoniometerCanvas,
    Gesture,
};

use crate::{AurumParams, AurumParamsParamId};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const AMBER: Color = Color { r: 1.0, g: 0.55, b: 0.1, a: 1.0 };

#[derive(Debug, Clone)]
pub enum AurumMsg {
    Tick, SelectTab(usize), ResetPeak, ResetAll,
    SideToggled, MonoToggled, DeltaToggled, BypassToggled,
    AtToggled, AtAmountGesture(Gesture), OutputGainGesture(Gesture),
    StereoWidthGesture(Gesture), MonoFloorGesture(Gesture),
    ClipCeilGesture(Gesture), ClipSoftGesture(Gesture), ClipMsToggled,
    EqMLoShG(Gesture), EqMLoMiG(Gesture), EqMHiMiG(Gesture), EqMHiShG(Gesture),
    EqSLoShG(Gesture), EqSLoMiG(Gesture), EqSHiMiG(Gesture), EqSHiShG(Gesture),
    CompSplitG(Gesture), CompLinkToggled, CompThrLoG(Gesture), CompThrHiG(Gesture),
    CompRatioG(Gesture), CompAtkG(Gesture), CompRelG(Gesture), CompMixG(Gesture),
    SweetHpfG(Gesture), SweetLpfG(Gesture), SweetLoG(Gesture), SweetHiG(Gesture),
    SatMsToggled, SatDrvStG(Gesture), SatDrvMiG(Gesture), SatDrvSiG(Gesture),
    SatMixG(Gesture), SatHarmCycled,
    MbXoverG(Gesture), MbThrLoG(Gesture), MbThrHiG(Gesture), MbThrSiG(Gesture),
    MbGainLoG(Gesture), MbGainHiG(Gesture), MbGainSiG(Gesture),
    MbAtkLoG(Gesture), MbAtkHiG(Gesture), MbAtkSiG(Gesture),
    MbRelLoG(Gesture), MbRelHiG(Gesture), MbRelSiG(Gesture),
    MbLinkToggled, MbGainGG(Gesture), MbThrOffG(Gesture), MbModeToggled,
    LimCeilG(Gesture), LimRelG(Gesture),
    SnapPressed,
    SetupToggled, VaultPathChanged(String), SaveVaultPath,
}

pub struct AurumEditor {
    params: Arc<AurumParams>,
    shared_state: Arc<SharedState>,
    selected_tab: usize,
    output_peak: f32, peak_hold: f32, peak_l: f32, peak_r: f32,
    peak_hold_l: f32, peak_hold_r: f32, phase_correlation: f32, balance: f32,
    vault_path: Option<String>, show_setup: bool, vault_path_input: String,
    snap_blink: u32,
}

impl AurumEditor {
    fn gesture(&self, id: AurumParamsParamId, g: Gesture, ctx: &PluginContext<AurumParams>) {
        match g { Gesture::Start => ctx.begin_edit(id), Gesture::Change(v) => { ctx.set_param(id, v as f64); } Gesture::End => ctx.end_edit(id) }
    }

    fn strip_label(text: &str) -> Element<'_, Message<AurumMsg>> {
        Text::new(text).size(10).font(bold_font()).color(Color::from_rgb(1.0, 0.55, 0.15)).into()
    }

    fn knob<'a>(label: &'a str, val: f32, min: f32, max: f32, def: f32, msg: impl Fn(Gesture) -> AurumMsg + 'a) -> Element<'a, Message<AurumMsg>> {
        let rng = max - min;
        knob_gesture(label, val, min, max, def, move |g| {
            let ng = match g { Gesture::Change(v) => Gesture::Change(if rng > 0.0 { (v - min) / rng } else { 0.0 }), other => other };
            Message::Plugin(msg(ng))
        })
    }

    fn tab_btn<'a>(label: &'a str, idx: usize, selected: usize) -> Element<'a, Message<AurumMsg>> {
        let active = idx == selected;
        button(Text::new(label).size(12).font(bold_font()))
            .on_press(Message::Plugin(AurumMsg::SelectTab(idx))).padding([6, 16])
            .style(move |_t, _s| {
                let bg = if active { Color::from_rgb(0.25, 0.15, 0.05) } else { Color::from_rgb(0.12, 0.12, 0.12) };
                button::Style { background: Some(bg.into()), text_color: if active { AMBER } else { Color::from_rgb(0.6, 0.6, 0.6) },
                    border: Border { color: if active { AMBER } else { Color::from_rgb(0.2, 0.2, 0.2) }, width: 2.0, radius: 3.0.into() }, ..Default::default() }
            }).into()
    }
}

impl IcedPlugin<AurumParams> for AurumEditor {
    type Message = AurumMsg;

    fn new(params: Arc<AurumParams>) -> Self {
        let shared = params.shared.clone();
        let cfg = shared_analysis::load_config("Aurum");
        #[cfg(test)]
        let selected_tab = params.test_initial_tab.load(Ordering::Relaxed);
        #[cfg(not(test))]
        let selected_tab = 0;
        Self { params, shared_state: shared, selected_tab,
            output_peak: -90.0, peak_hold: -90.0, peak_l: -90.0, peak_r: -90.0,
            peak_hold_l: -90.0, peak_hold_r: -90.0, phase_correlation: 1.0, balance: 0.0,
            vault_path: cfg.vault_path.clone(), show_setup: false,
            vault_path_input: cfg.vault_path.unwrap_or_default(),
            snap_blink: 0 }
    }

    fn subscription(&self) -> Subscription<Message<AurumMsg>> {
        truce_iced::iced::event::listen_raw(|event, _status, _window| {
            use truce_iced::iced::{Event, window::Event as WinEvent};
            match event { Event::Window(WinEvent::RedrawRequested(_)) => Some(Message::Plugin(AurumMsg::Tick)), _ => None }
        })
    }

    fn needs_redraw(&self) -> bool { true }

    fn update(&mut self, message: Message<AurumMsg>, _p: &ParamCache<AurumParams>, ctx: &PluginContext<AurumParams>) -> truce_iced::iced::Task<Message<AurumMsg>> {
        let Message::Plugin(msg) = message else { return truce_iced::iced::Task::none(); };
        let p = &self.params;
        let toggle = |id: AurumParamsParamId, v: bool| { ctx.begin_edit(id); ctx.set_param(id, if v { 1.0 } else { 0.0 }); ctx.end_edit(id); };
        match msg {
            AurumMsg::Tick => {
                self.output_peak = self.shared_state.output_peak.load(Ordering::Relaxed);
                self.peak_hold = self.shared_state.peak_hold.load(Ordering::Relaxed);
                self.peak_l = self.shared_state.output_peak_l.load(Ordering::Relaxed);
                self.peak_r = self.shared_state.output_peak_r.load(Ordering::Relaxed);
                self.peak_hold_l = self.shared_state.peak_hold_l.load(Ordering::Relaxed);
                self.peak_hold_r = self.shared_state.peak_hold_r.load(Ordering::Relaxed);
                self.phase_correlation = self.shared_state.phase_correlation.load(Ordering::Relaxed);
                self.balance = self.shared_state.balance.load(Ordering::Relaxed);
                if self.snap_blink > 0 { self.snap_blink -= 1; }
            }
            AurumMsg::SelectTab(t) => self.selected_tab = t,
            AurumMsg::ResetPeak => { self.shared_state.reset_peak.store(true, Ordering::Relaxed); }
            AurumMsg::ResetAll => {
                self.shared_state.reset_peak.store(true, Ordering::Relaxed);
                ctx.begin_edit(AurumParamsParamId::ClipCeiling); ctx.set_param(AurumParamsParamId::ClipCeiling, 0.8167); ctx.end_edit(AurumParamsParamId::ClipCeiling);
                ctx.begin_edit(AurumParamsParamId::OutputGain); ctx.set_param(AurumParamsParamId::OutputGain, 0.5); ctx.end_edit(AurumParamsParamId::OutputGain);
                ctx.begin_edit(AurumParamsParamId::StereoWidth); ctx.set_param(AurumParamsParamId::StereoWidth, 0.5); ctx.end_edit(AurumParamsParamId::StereoWidth);
            }
            AurumMsg::SideToggled => toggle(AurumParamsParamId::SideActive, !p.side_active.value()),
            AurumMsg::MonoToggled => toggle(AurumParamsParamId::MonoActive, !p.mono_active.value()),
            AurumMsg::DeltaToggled => toggle(AurumParamsParamId::DeltaActive, !p.delta_active.value()),
            AurumMsg::BypassToggled => toggle(AurumParamsParamId::BypassActive, !p.bypass_active.value()),
            AurumMsg::AtToggled => toggle(AurumParamsParamId::AtActive, !p.at_active.value()),
            AurumMsg::AtAmountGesture(g) => self.gesture(AurumParamsParamId::AtAmount, g, ctx),
            AurumMsg::OutputGainGesture(g) => self.gesture(AurumParamsParamId::OutputGain, g, ctx),
            AurumMsg::StereoWidthGesture(g) => self.gesture(AurumParamsParamId::StereoWidth, g, ctx),
            AurumMsg::MonoFloorGesture(g) => self.gesture(AurumParamsParamId::MonoFloor, g, ctx),
            AurumMsg::ClipCeilGesture(g) => self.gesture(AurumParamsParamId::ClipCeiling, g, ctx),
            AurumMsg::ClipSoftGesture(g) => self.gesture(AurumParamsParamId::ClipSoftness, g, ctx),
            AurumMsg::ClipMsToggled => toggle(AurumParamsParamId::ClipMsMode, !p.clip_ms_mode.value()),
            AurumMsg::EqMLoShG(g) => self.gesture(AurumParamsParamId::EqMLoShelf, g, ctx),
            AurumMsg::EqMLoMiG(g) => self.gesture(AurumParamsParamId::EqMLoMid, g, ctx),
            AurumMsg::EqMHiMiG(g) => self.gesture(AurumParamsParamId::EqMHiMid, g, ctx),
            AurumMsg::EqMHiShG(g) => self.gesture(AurumParamsParamId::EqMHiShelf, g, ctx),
            AurumMsg::EqSLoShG(g) => self.gesture(AurumParamsParamId::EqSLoShelf, g, ctx),
            AurumMsg::EqSLoMiG(g) => self.gesture(AurumParamsParamId::EqSLoMid, g, ctx),
            AurumMsg::EqSHiMiG(g) => self.gesture(AurumParamsParamId::EqSHiMid, g, ctx),
            AurumMsg::EqSHiShG(g) => self.gesture(AurumParamsParamId::EqSHiShelf, g, ctx),
            AurumMsg::CompSplitG(g) => self.gesture(AurumParamsParamId::CompSplit, g, ctx),
            AurumMsg::CompLinkToggled => toggle(AurumParamsParamId::CompLink, !p.comp_link.value()),
            AurumMsg::CompThrLoG(g) => self.gesture(AurumParamsParamId::CompThreshLo, g, ctx),
            AurumMsg::CompThrHiG(g) => self.gesture(AurumParamsParamId::CompThreshHi, g, ctx),
            AurumMsg::CompRatioG(g) => self.gesture(AurumParamsParamId::CompRatio, g, ctx),
            AurumMsg::CompAtkG(g) => self.gesture(AurumParamsParamId::CompAttack, g, ctx),
            AurumMsg::CompRelG(g) => self.gesture(AurumParamsParamId::CompRelease, g, ctx),
            AurumMsg::CompMixG(g) => self.gesture(AurumParamsParamId::CompMix, g, ctx),
            AurumMsg::SweetHpfG(g) => self.gesture(AurumParamsParamId::SweetHpf, g, ctx),
            AurumMsg::SweetLpfG(g) => self.gesture(AurumParamsParamId::SweetLpf, g, ctx),
            AurumMsg::SweetLoG(g) => self.gesture(AurumParamsParamId::SweetLoShelf, g, ctx),
            AurumMsg::SweetHiG(g) => self.gesture(AurumParamsParamId::SweetHiShelf, g, ctx),
            AurumMsg::SatMsToggled => toggle(AurumParamsParamId::SatMsMode, !p.sat_ms_mode.value()),
            AurumMsg::SatDrvStG(g) => self.gesture(AurumParamsParamId::SatDriveStereo, g, ctx),
            AurumMsg::SatDrvMiG(g) => self.gesture(AurumParamsParamId::SatDriveMid, g, ctx),
            AurumMsg::SatDrvSiG(g) => self.gesture(AurumParamsParamId::SatDriveSide, g, ctx),
            AurumMsg::SatMixG(g) => self.gesture(AurumParamsParamId::SatMix, g, ctx),
            AurumMsg::SatHarmCycled => {
                let n = (p.sat_harmonics.value_i32() + 1) % 3;
                ctx.begin_edit(AurumParamsParamId::SatHarmonics); ctx.set_param(AurumParamsParamId::SatHarmonics, n as f64 / 2.0); ctx.end_edit(AurumParamsParamId::SatHarmonics);
            }
            AurumMsg::MbXoverG(g) => self.gesture(AurumParamsParamId::MbCrossover, g, ctx),
            AurumMsg::MbThrLoG(g) => self.gesture(AurumParamsParamId::MbThreshMidLo, g, ctx),
            AurumMsg::MbThrHiG(g) => self.gesture(AurumParamsParamId::MbThreshMidHi, g, ctx),
            AurumMsg::MbThrSiG(g) => self.gesture(AurumParamsParamId::MbThreshSide, g, ctx),
            AurumMsg::MbGainLoG(g) => self.gesture(AurumParamsParamId::MbGainMidLo, g, ctx),
            AurumMsg::MbGainHiG(g) => self.gesture(AurumParamsParamId::MbGainMidHi, g, ctx),
            AurumMsg::MbGainSiG(g) => self.gesture(AurumParamsParamId::MbGainSide, g, ctx),
            AurumMsg::MbAtkLoG(g) => self.gesture(AurumParamsParamId::MbAttackMidLo, g, ctx),
            AurumMsg::MbAtkHiG(g) => self.gesture(AurumParamsParamId::MbAttackMidHi, g, ctx),
            AurumMsg::MbAtkSiG(g) => self.gesture(AurumParamsParamId::MbAttackSide, g, ctx),
            AurumMsg::MbRelLoG(g) => self.gesture(AurumParamsParamId::MbReleaseMidLo, g, ctx),
            AurumMsg::MbRelHiG(g) => self.gesture(AurumParamsParamId::MbReleaseMidHi, g, ctx),
            AurumMsg::MbRelSiG(g) => self.gesture(AurumParamsParamId::MbReleaseSide, g, ctx),
            AurumMsg::MbLinkToggled => toggle(AurumParamsParamId::MbFaderLink, !p.mb_fader_link.value()),
            AurumMsg::MbGainGG(g) => self.gesture(AurumParamsParamId::MbGlobalGain, g, ctx),
            AurumMsg::MbThrOffG(g) => self.gesture(AurumParamsParamId::MbGlobalThresh, g, ctx),
            AurumMsg::MbModeToggled => toggle(AurumParamsParamId::MbMode, !p.mb_mode.value()),
            AurumMsg::LimCeilG(g) => self.gesture(AurumParamsParamId::LimCeiling, g, ctx),
            AurumMsg::LimRelG(g) => self.gesture(AurumParamsParamId::LimRelease, g, ctx),
            AurumMsg::SnapPressed => { self.snap_blink = 72; self.shared_state.snap_active.store(true, Ordering::Relaxed); }
            AurumMsg::SetupToggled => { self.show_setup = !self.show_setup; if self.show_setup { self.vault_path_input = self.vault_path.clone().unwrap_or_default(); } }
            AurumMsg::VaultPathChanged(p) => self.vault_path_input = p,
            AurumMsg::SaveVaultPath => {
                let np = if self.vault_path_input.trim().is_empty() { None } else { Some(self.vault_path_input.trim().to_string()) };
                self.vault_path = np;
                let mut cfg = shared_analysis::load_config("Aurum"); cfg.vault_path = self.vault_path.clone();
                let _ = shared_analysis::save_config("Aurum", &cfg); self.show_setup = false;
            }
        }
        truce_iced::iced::Task::none()
    }

    fn view<'a>(&'a self, _params: &'a ParamCache<AurumParams>) -> Element<'a, Message<AurumMsg>> {
        let p = &self.params;

        // HEADER
        let header = container(row![
            container(header_brand("Aurum", VERSION)).width(Length::Shrink),
            Space::new().width(Length::Fill),
            row![Self::tab_btn("SHAPE", 0, self.selected_tab), Self::tab_btn("COLOR", 1, self.selected_tab), Self::tab_btn("LIMIT", 2, self.selected_tab)].spacing(4),
            Space::new().width(Length::Fill),
            row![
                toggle_button("SIDE", p.side_active.value(), Message::Plugin(AurumMsg::SideToggled)),
                toggle_button("MONO", p.mono_active.value(), Message::Plugin(AurumMsg::MonoToggled)),
                toggle_button("Δ", p.delta_active.value(), Message::Plugin(AurumMsg::DeltaToggled)),
                toggle_button("BYPASS", p.bypass_active.value(), Message::Plugin(AurumMsg::BypassToggled)),
            ].spacing(6),
        ].align_y(Alignment::Center).spacing(8).padding(8))
        .width(Length::Fill).height(Length::Fixed(50.0))
        .style(|_t| container::Style { background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()), border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() }, ..Default::default() });

        // LEFT SIDEBAR
        let snap_label = if self.snap_blink > 0 { "ANALYZING..." } else { "SNAP" };
        let sidebar = container(column![
            Text::new("LX AUDIOLABS").font(bold_font()).size(14).color(Color::WHITE),
            button(Text::new(snap_label).font(bold_font()).size(12).width(Length::Fill).align_x(Alignment::Center))
                .on_press(Message::Plugin(AurumMsg::SnapPressed)).width(Length::Fill).padding([7, 1])
                .style(move |_t, _s| {
                    let bg = if self.snap_blink > 0 { Color::from_rgb(0.55, 0.38, 0.05) } else { Color::from_rgb(0.18, 0.18, 0.18) };
                    button::Style { background: Some(bg.into()), text_color: if self.snap_blink > 0 { Color::from_rgb(1.0, 0.85, 0.3) } else { AMBER },
                        border: Border { color: Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: 3.0.into() }, ..Default::default() }
                }),
            button(Text::new("VAULT SETUP").font(bold_font()).size(12).width(Length::Fill).align_x(Alignment::Center))
                .on_press(Message::Plugin(AurumMsg::SetupToggled)).width(Length::Fill).padding([7, 1])
                .style(|_t, s| {
                    let bg = if s == button::Status::Hovered { Color::from_rgb(0.25, 0.25, 0.25) } else { Color::from_rgb(0.18, 0.18, 0.18) };
                    button::Style { background: Some(bg.into()), text_color: Color::WHITE, border: Border { color: Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: 3.0.into() }, ..Default::default() }
                }),
        ].spacing(10))
        .width(Length::Fixed(180.0)).height(Length::Fill).padding(10)
        .style(|_t| container::Style { background: Some(Color::from_rgb(0.09, 0.09, 0.09).into()), border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() }, ..Default::default() });

        // MAIN
        let main = if self.show_setup {
            container(vault_setup_box("Aurum", &self.vault_path_input, |s| Message::Plugin(AurumMsg::VaultPathChanged(s)), Message::Plugin(AurumMsg::SaveVaultPath), Message::Plugin(AurumMsg::SetupToggled)))
                .width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill)
        } else {
              container(match self.selected_tab { 0 => self.shape_tab(), 1 => self.color_tab(), _ => self.limit_tab() }).width(Length::Fill).height(Length::Fill)
        };

        // RIGHT BAR: OUT GAIN → Peak Meters (fill) → Goniometer
        let scope_pos = self.shared_state.scope_write_pos.load(Ordering::Acquire);
        let right_bar = container(column![
            knob_gesture_bipolar("OUT GAIN", p.output_gain.raw_target() as f32, -12.0, 12.0, 0.0, |g| Message::Plugin(AurumMsg::OutputGainGesture(g))),
            output_level_block(self.peak_l, self.peak_r, self.peak_hold_l, self.peak_hold_r, self.peak_hold, Message::Plugin(AurumMsg::ResetPeak), self.balance, Length::Fill),
            Space::new().height(Length::Fixed(4.0)),
            canvas(GoniometerCanvas { samples: self.shared_state.scope_samples.clone(), write_pos: scope_pos, correlation: self.phase_correlation })
                .width(Length::Fill).height(Length::Fixed(180.0)),
        ].spacing(4).padding(6).align_x(Alignment::Center))
        .width(Length::Fixed(200.0)).height(Length::Fill)
        .style(|_t| container::Style { background: Some(Color::from_rgb(0.07, 0.07, 0.07).into()), border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() }, ..Default::default() });

        // FOOTER: Space | AT center | Space | STEREO/ROUTING + RESET right
        let footer = container(row![
            Space::new().width(Length::Fill),
            at_block(p.at_active.value(), p.at_amount.raw_target() as f32, Message::Plugin(AurumMsg::AtToggled), |v| Message::Plugin(AurumMsg::AtAmountGesture(Gesture::Change(v)))),
            Space::new().width(Length::Fill),
            column![
                Text::new("STEREO/ROUTING").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
                row![
                    knob_gesture_bipolar("WIDTH", p.stereo_width.raw_target() as f32, 0.0, 2.0, 1.0, |g| Message::Plugin(AurumMsg::StereoWidthGesture(g))),
                    Self::knob("M.FLOOR", p.mono_floor.raw_target() as f32, 0.0, 300.0, 0.0, AurumMsg::MonoFloorGesture),
                    output_tools_strip(Message::Plugin(AurumMsg::ResetAll)),
                ].spacing(8).align_y(Alignment::Center),
            ].spacing(2).align_x(Alignment::Center),
        ].spacing(8).align_y(Alignment::Center).padding([4, 12]))
        .width(Length::Fill).height(Length::Fixed(70.0))
        .style(|_t| container::Style { background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()), border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() }, ..Default::default() });

        let body = container(row![sidebar, main, right_bar].spacing(0))
            .width(Length::Fill).height(Length::Fill)
            .style(|_t| container::Style { background: Some(Color::from_rgb(0.09, 0.09, 0.09).into()), ..Default::default() });

        column![header, body, footer].into()
    }
}

// ─── Tab Builders ──────────────────────────────────────────────────────────

impl AurumEditor {
    fn shape_tab(&self) -> Element<'_, Message<AurumMsg>> {
        let p = &self.params;
        let in_db = self.shared_state.input_peak.load(Ordering::Relaxed);
        let inp = if in_db <= -90.0 { "--".to_string() } else { format!("{in_db:.1} dB") };

        container(column![
            container(row![Self::strip_label("INPUT"), Space::new().width(Length::Fixed(12.0)), Text::new(inp).size(14).font(bold_font()).color(AMBER)].align_y(Alignment::Center).spacing(4).padding([4, 12])).width(Length::Fill),
            container(row![Self::strip_label("CLIPPER"), Space::new().width(Length::Fixed(12.0)),
                Self::knob("CEIL dBTP", p.clip_ceiling.raw_target() as f32, -6.0, -0.1, -1.0, AurumMsg::ClipCeilGesture), Space::new().width(Length::Fixed(6.0)),
                Self::knob("SOFT %", p.clip_softness.raw_target() as f32, 0.0, 100.0, 50.0, AurumMsg::ClipSoftGesture), Space::new().width(Length::Fixed(6.0)),
                toggle_button("M/S", p.clip_ms_mode.value(), Message::Plugin(AurumMsg::ClipMsToggled)),
            ].align_y(Alignment::Center).spacing(4).padding([4, 12])).width(Length::Fill).style(|_t| container::Style { background: Some(Color::from_rgb(0.11, 0.11, 0.11).into()), ..Default::default() }),
            container(row![Self::strip_label("M/S EQ"), Space::new().width(Length::Fixed(12.0)),
                column![Text::new("MID").size(9).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
                    row![Self::knob("LO SH", p.eq_m_lo_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMLoShG), Self::knob("LO-MI", p.eq_m_lo_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMLoMiG), Self::knob("HI-MI", p.eq_m_hi_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMHiMiG), Self::knob("HI SH", p.eq_m_hi_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMHiShG)].spacing(4),
                ].spacing(2).align_x(Alignment::Center),
                Space::new().width(Length::Fixed(16.0)),
                column![Text::new("SIDE").size(9).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
                    row![Self::knob("LO SH", p.eq_s_lo_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSLoShG), Self::knob("LO-MI", p.eq_s_lo_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSLoMiG), Self::knob("HI-MI", p.eq_s_hi_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSHiMiG), Self::knob("HI SH", p.eq_s_hi_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSHiShG)].spacing(4),
                ].spacing(2).align_x(Alignment::Center),
            ].align_y(Alignment::Center).spacing(4).padding([4, 12])).width(Length::Fill).style(|_t| container::Style { background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()), ..Default::default() }),
        ].spacing(2)).width(Length::Fill).into()
    }

    fn color_tab(&self) -> Element<'_, Message<AurumMsg>> {
        let p = &self.params;
        container(column![
            container(row![Self::strip_label("2-BAND COMP"), Space::new().width(Length::Fixed(12.0)),
                Self::knob("SPLIT Hz", p.comp_split.raw_target() as f32, 80.0, 500.0, 200.0, AurumMsg::CompSplitG), Space::new().width(Length::Fixed(4.0)),
                toggle_button("LINK", p.comp_link.value(), Message::Plugin(AurumMsg::CompLinkToggled)), Space::new().width(Length::Fixed(4.0)),
                Self::knob("THR LO", p.comp_thresh_lo.raw_target() as f32, -30.0, 0.0, -12.0, AurumMsg::CompThrLoG), Self::knob("THR HI", p.comp_thresh_hi.raw_target() as f32, -30.0, 0.0, -12.0, AurumMsg::CompThrHiG),
                Self::knob("RATIO", p.comp_ratio.raw_target() as f32, 1.2, 3.0, 1.5, AurumMsg::CompRatioG), Self::knob("ATK ms", p.comp_attack.raw_target() as f32, 10.0, 100.0, 30.0, AurumMsg::CompAtkG),
                Self::knob("REL ms", p.comp_release.raw_target() as f32, 50.0, 500.0, 150.0, AurumMsg::CompRelG), Self::knob("MIX %", p.comp_mix.raw_target() as f32, 0.0, 100.0, 50.0, AurumMsg::CompMixG),
            ].align_y(Alignment::Center).spacing(4).padding([4, 12])).width(Length::Fill).style(|_t| container::Style { background: Some(Color::from_rgb(0.11, 0.11, 0.11).into()), ..Default::default() }),
            container(row![Self::strip_label("SWEETENING"), Space::new().width(Length::Fixed(12.0)),
                Self::knob("HPF Hz", p.sweet_hpf.raw_target() as f32, 10.0, 60.0, 24.0, AurumMsg::SweetHpfG), Self::knob("LPF Hz", p.sweet_lpf.raw_target() as f32, 18000.0, 40000.0, 35000.0, AurumMsg::SweetLpfG),
                Self::knob("LO SH", p.sweet_lo_shelf.raw_target() as f32, -4.0, 4.0, 0.0, AurumMsg::SweetLoG), Self::knob("HI SH", p.sweet_hi_shelf.raw_target() as f32, -4.0, 4.0, 0.0, AurumMsg::SweetHiG),
            ].align_y(Alignment::Center).spacing(4).padding([4, 12])).width(Length::Fill).style(|_t| container::Style { background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()), ..Default::default() }),
            container(row![Self::strip_label("SATURATOR"), Space::new().width(Length::Fixed(12.0)),
                toggle_button("M/S", p.sat_ms_mode.value(), Message::Plugin(AurumMsg::SatMsToggled)), Space::new().width(Length::Fixed(4.0)),
                Self::knob("DRV ST", p.sat_drive_stereo.raw_target() as f32, 0.0, 12.0, 0.0, AurumMsg::SatDrvStG), Self::knob("DRV MI", p.sat_drive_mid.raw_target() as f32, 0.0, 12.0, 0.0, AurumMsg::SatDrvMiG),
                Self::knob("DRV SI", p.sat_drive_side.raw_target() as f32, 0.0, 12.0, 0.0, AurumMsg::SatDrvSiG), Self::knob("MIX %", p.sat_mix.raw_target() as f32, 0.0, 60.0, 20.0, AurumMsg::SatMixG),
                { let h = ["EVEN","ODD","MIXED"][p.sat_harmonics.value_i32() as usize % 3]; toggle_button(h, true, Message::Plugin(AurumMsg::SatHarmCycled)) },
            ].align_y(Alignment::Center).spacing(4).padding([4, 12])).width(Length::Fill).style(|_t| container::Style { background: Some(Color::from_rgb(0.11, 0.11, 0.11).into()), ..Default::default() }),
        ].spacing(2)).width(Length::Fill).into()
    }

    fn limit_tab(&self) -> Element<'_, Message<AurumMsg>> {
        let p = &self.params;
        container(column![
            container(column![
                row![Self::strip_label("M/S MB LIMITER"), Space::new().width(Length::Fixed(8.0)),
                    Self::knob("XOVER Hz", p.mb_crossover.raw_target() as f32, 20.0, 500.0, 250.0, AurumMsg::MbXoverG),
                    Self::knob("G.THR dB", p.mb_global_thresh.raw_target() as f32, -18.0, 0.0, 0.0, AurumMsg::MbThrOffG),
                    Self::knob("G.GAIN dB", p.mb_global_gain.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::MbGainGG),
                    toggle_button("LINK", p.mb_fader_link.value(), Message::Plugin(AurumMsg::MbLinkToggled)),
                    { let m = if p.mb_mode.value() { "MODERN" } else { "CLASSIC" }; toggle_button(m, true, Message::Plugin(AurumMsg::MbModeToggled)) },
                ].spacing(4).align_y(Alignment::Center),
                Space::new().height(Length::Fixed(4.0)),
                row![band_col("MID-LO", p.mb_thresh_mid_lo.raw_target() as f32, AurumMsg::MbThrLoG, p.mb_attack_mid_lo.raw_target() as f32, AurumMsg::MbAtkLoG, p.mb_release_mid_lo.raw_target() as f32, AurumMsg::MbRelLoG, p.mb_gain_mid_lo.raw_target() as f32, AurumMsg::MbGainLoG),
                    Space::new().width(Length::Fixed(16.0)),
                    band_col("MID-HI", p.mb_thresh_mid_hi.raw_target() as f32, AurumMsg::MbThrHiG, p.mb_attack_mid_hi.raw_target() as f32, AurumMsg::MbAtkHiG, p.mb_release_mid_hi.raw_target() as f32, AurumMsg::MbRelHiG, p.mb_gain_mid_hi.raw_target() as f32, AurumMsg::MbGainHiG),
                    Space::new().width(Length::Fixed(16.0)),
                    band_col("SIDE", p.mb_thresh_side.raw_target() as f32, AurumMsg::MbThrSiG, p.mb_attack_side.raw_target() as f32, AurumMsg::MbAtkSiG, p.mb_release_side.raw_target() as f32, AurumMsg::MbRelSiG, p.mb_gain_side.raw_target() as f32, AurumMsg::MbGainSiG),
                ].spacing(4),
            ].spacing(4).padding([4, 12])).width(Length::Fill).style(|_t| container::Style { background: Some(Color::from_rgb(0.11, 0.11, 0.11).into()), ..Default::default() }),
            container(row![Self::strip_label("TP LIMITER"), Space::new().width(Length::Fixed(12.0)),
                Self::knob("CEIL dBTP", p.lim_ceiling.raw_target() as f32, -6.0, -0.1, -1.0, AurumMsg::LimCeilG),
                Self::knob("REL ms", p.lim_release.raw_target() as f32, 10.0, 500.0, 100.0, AurumMsg::LimRelG),
                Space::new().width(Length::Fixed(20.0)),
                {
                    let gr = self.shared_state.gain_reduction.load(Ordering::Relaxed);
                    let gr_s = if gr >= -0.01 { "0.0 dB".to_string() } else { format!("{gr:.1} dB") };
                    column![Self::strip_label("OUTPUT"), Text::new(format!("GR {gr_s} | L {:.1} R {:.1} dB", self.peak_l, self.peak_r)).size(11).font(bold_font()).color(AMBER)].spacing(2)
                },
            ].align_y(Alignment::Center).spacing(4).padding([4, 12])).width(Length::Fill).style(|_t| container::Style { background: Some(Color::from_rgb(0.1, 0.1, 0.1).into()), ..Default::default() }),
        ].spacing(2)).width(Length::Fill).into()
    }
}

fn band_col(label: &str, thr: f32, thr_msg: impl Fn(Gesture) -> AurumMsg + 'static, atk: f32, atk_msg: impl Fn(Gesture) -> AurumMsg + 'static, rel: f32, rel_msg: impl Fn(Gesture) -> AurumMsg + 'static, gain: f32, gain_msg: impl Fn(Gesture) -> AurumMsg + 'static) -> Element<'_, Message<AurumMsg>> {
    column![Text::new(label).size(9).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
        AurumEditor::knob("THR dB", thr, -18.0, 0.0, -3.0, thr_msg), AurumEditor::knob("ATK ms", atk, 0.1, 50.0, 5.0, atk_msg),
        AurumEditor::knob("REL ms", rel, 10.0, 500.0, 100.0, rel_msg), AurumEditor::knob("GAIN dB", gain, -6.0, 6.0, 0.0, gain_msg),
    ].spacing(2).align_x(Alignment::Center).into()
}
