// Aurum editor — Tab-based UI (truce port).
//
// Tabs: SHAPE (0), COLOR (1), LIMIT (2)
// Right bar: Goniometer + Output Peaks (always visible)

use truce_iced::iced::widget::{button, canvas, column, container, row, Space, Text};
use truce_iced::iced::{Alignment, Border, Color, Element, Length, Subscription};
use truce_iced::{IcedPlugin, Message, ParamCache};
use truce_core::editor::PluginContext;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use shared_analysis::SharedState;
use shared_ui::{
    bold_font, header_brand, toggle_button, knob_gesture, knob_gesture_bipolar,
    output_tools_strip, output_level_block, at_block, Gesture,
    GoniometerCanvas,
};

use crate::{AurumParams, AurumParamsParamId};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const AMBER: Color = Color { r: 1.0, g: 0.55, b: 0.1, a: 1.0 };

#[derive(Debug, Clone)]
pub enum AurumMsg {
    Tick,
    SelectTab(usize),
    ResetPeak,
    ResetAll,

    // Monitor
    SideToggled, MonoToggled, DeltaToggled, BypassToggled,
    AtToggled, AtAmountGesture(Gesture),
    OutputGainGesture(Gesture),
    StereoWidthGesture(Gesture),
    MonoFloorGesture(Gesture),

    // Clipper
    ClipCeilGesture(Gesture), ClipSoftGesture(Gesture), ClipMsToggled,

    // M/S EQ Mid
    EqMLoShGesture(Gesture), EqMLoMiGesture(Gesture), EqMHiMiGesture(Gesture), EqMHiShGesture(Gesture),
    // M/S EQ Side
    EqSLoShGesture(Gesture), EqSLoMiGesture(Gesture), EqSHiMiGesture(Gesture), EqSHiShGesture(Gesture),

    // 2-Band Comp
    CompSplitGesture(Gesture), CompLinkToggled,
    CompThrLoGesture(Gesture), CompThrHiGesture(Gesture),
    CompRatioGesture(Gesture), CompAtkGesture(Gesture), CompRelGesture(Gesture),
    CompMixGesture(Gesture),

    // Sweetening
    SweetHpfGesture(Gesture), SweetLpfGesture(Gesture),
    SweetLoGesture(Gesture), SweetHiGesture(Gesture),

    // Saturator
    SatMsToggled,
    SatDrvStGesture(Gesture), SatDrvMiGesture(Gesture), SatDrvSiGesture(Gesture),
    SatMixGesture(Gesture), SatHarmCycled,

    // MB Limiter
    MbXoverGesture(Gesture),
    MbThrLoGesture(Gesture), MbThrHiGesture(Gesture), MbThrSiGesture(Gesture),
    MbGainLoGesture(Gesture), MbGainHiGesture(Gesture), MbGainSiGesture(Gesture),
    MbAtkLoGesture(Gesture), MbAtkHiGesture(Gesture), MbAtkSiGesture(Gesture),
    MbRelLoGesture(Gesture), MbRelHiGesture(Gesture), MbRelSiGesture(Gesture),
    MbLinkToggled, MbGainGesture(Gesture), MbThrOffGesture(Gesture), MbModeToggled,

    // TP Limiter
    LimCeilGesture(Gesture), LimRelGesture(Gesture),
}

pub struct AurumEditor {
    params: Arc<AurumParams>,
    shared_state: Arc<SharedState>,
    selected_tab: usize,
    output_peak: f32, peak_hold: f32,
    peak_l: f32, peak_r: f32, peak_hold_l: f32, peak_hold_r: f32,
    phase_correlation: f32, balance: f32,
}

impl AurumEditor {
    fn gesture_f(&self, id: AurumParamsParamId, g: Gesture, ctx: &PluginContext<AurumParams>) {
        match g {
            Gesture::Start => ctx.begin_edit(id),
            Gesture::Change(v) => { ctx.set_param(id, v as f64); }
            Gesture::End => ctx.end_edit(id),
        }
    }

    fn tab_btn<'a>(label: &'a str, idx: usize, selected: usize) -> Element<'a, Message<AurumMsg>> {
        let active = idx == selected;
        button(Text::new(label).size(12).font(bold_font()))
            .on_press(Message::Plugin(AurumMsg::SelectTab(idx)))
            .padding([6, 16])
            .style(move |_t, _s| {
                let bg = if active { Color::from_rgb(0.25, 0.15, 0.05) } else { Color::from_rgb(0.12, 0.12, 0.12) };
                let border_col = if active { AMBER } else { Color::from_rgb(0.2, 0.2, 0.2) };
                button::Style {
                    background: Some(bg.into()), text_color: if active { AMBER } else { Color::from_rgb(0.6, 0.6, 0.6) },
                    border: Border { color: border_col, width: 2.0, radius: 3.0.into() }, ..Default::default()
                }
            }).into()
    }
}

impl IcedPlugin<AurumParams> for AurumEditor {
    type Message = AurumMsg;

    fn new(params: Arc<AurumParams>) -> Self {
        let shared = params.shared.clone();
        Self {
            params, shared_state: shared,
            selected_tab: 0,
            output_peak: -90.0, peak_hold: -90.0,
            peak_l: -90.0, peak_r: -90.0, peak_hold_l: -90.0, peak_hold_r: -90.0,
            phase_correlation: 1.0, balance: 0.0,
        }
    }

    fn subscription(&self) -> Subscription<Message<AurumMsg>> {
        truce_iced::iced::event::listen_raw(|event, _status, _window| {
            use truce_iced::iced::{Event, window::Event as WinEvent};
            match event { Event::Window(WinEvent::RedrawRequested(_)) => Some(Message::Plugin(AurumMsg::Tick)), _ => None }
        })
    }

    fn needs_redraw(&self) -> bool { true }

    fn update(&mut self, message: Message<AurumMsg>, _params: &ParamCache<AurumParams>, ctx: &PluginContext<AurumParams>) -> truce_iced::iced::Task<Message<AurumMsg>> {
        let Message::Plugin(msg) = message else { return truce_iced::iced::Task::none(); };

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
            }
            AurumMsg::SelectTab(t) => self.selected_tab = t,
            AurumMsg::ResetPeak => { self.shared_state.reset_peak.store(true, Ordering::Relaxed); }
            AurumMsg::ResetAll => {
                self.shared_state.reset_peak.store(true, Ordering::Relaxed);
                // Reset key params to defaults
                ctx.begin_edit(AurumParamsParamId::ClipCeiling); ctx.set_param(AurumParamsParamId::ClipCeiling, -1.0); ctx.end_edit(AurumParamsParamId::ClipCeiling);
                ctx.begin_edit(AurumParamsParamId::MbThreshMidLo); ctx.set_param(AurumParamsParamId::MbThreshMidLo, -3.0); ctx.end_edit(AurumParamsParamId::MbThreshMidLo);
                ctx.begin_edit(AurumParamsParamId::MbThreshMidHi); ctx.set_param(AurumParamsParamId::MbThreshMidHi, -3.0); ctx.end_edit(AurumParamsParamId::MbThreshMidHi);
                ctx.begin_edit(AurumParamsParamId::MbThreshSide); ctx.set_param(AurumParamsParamId::MbThreshSide, -6.0); ctx.end_edit(AurumParamsParamId::MbThreshSide);
                ctx.begin_edit(AurumParamsParamId::OutputGain); ctx.set_param(AurumParamsParamId::OutputGain, 0.0); ctx.end_edit(AurumParamsParamId::OutputGain);
                ctx.begin_edit(AurumParamsParamId::StereoWidth); ctx.set_param(AurumParamsParamId::StereoWidth, 1.0); ctx.end_edit(AurumParamsParamId::StereoWidth);
            }
            // Monitor
            AurumMsg::SideToggled => { ctx.begin_edit(AurumParamsParamId::SideActive); ctx.set_param(AurumParamsParamId::SideActive, if self.params.side_active.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::SideActive); }
            AurumMsg::MonoToggled => { ctx.begin_edit(AurumParamsParamId::MonoActive); ctx.set_param(AurumParamsParamId::MonoActive, if self.params.mono_active.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::MonoActive); }
            AurumMsg::DeltaToggled => { ctx.begin_edit(AurumParamsParamId::DeltaActive); ctx.set_param(AurumParamsParamId::DeltaActive, if self.params.delta_active.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::DeltaActive); }
            AurumMsg::BypassToggled => { ctx.begin_edit(AurumParamsParamId::BypassActive); ctx.set_param(AurumParamsParamId::BypassActive, if self.params.bypass_active.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::BypassActive); }
            AurumMsg::AtToggled => { ctx.begin_edit(AurumParamsParamId::AtActive); ctx.set_param(AurumParamsParamId::AtActive, if self.params.at_active.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::AtActive); }
            AurumMsg::AtAmountGesture(g) => self.gesture_f(AurumParamsParamId::AtAmount, g, ctx),
            AurumMsg::OutputGainGesture(g) => self.gesture_f(AurumParamsParamId::OutputGain, g, ctx),
            AurumMsg::StereoWidthGesture(g) => self.gesture_f(AurumParamsParamId::StereoWidth, g, ctx),
            AurumMsg::MonoFloorGesture(g) => self.gesture_f(AurumParamsParamId::MonoFloor, g, ctx),
            // Clipper
            AurumMsg::ClipCeilGesture(g) => self.gesture_f(AurumParamsParamId::ClipCeiling, g, ctx),
            AurumMsg::ClipSoftGesture(g) => self.gesture_f(AurumParamsParamId::ClipSoftness, g, ctx),
            AurumMsg::ClipMsToggled => { ctx.begin_edit(AurumParamsParamId::ClipMsMode); ctx.set_param(AurumParamsParamId::ClipMsMode, if self.params.clip_ms_mode.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::ClipMsMode); }
            // M/S EQ Mid
            AurumMsg::EqMLoShGesture(g) => self.gesture_f(AurumParamsParamId::EqMLoShelf, g, ctx),
            AurumMsg::EqMLoMiGesture(g) => self.gesture_f(AurumParamsParamId::EqMLoMid, g, ctx),
            AurumMsg::EqMHiMiGesture(g) => self.gesture_f(AurumParamsParamId::EqMHiMid, g, ctx),
            AurumMsg::EqMHiShGesture(g) => self.gesture_f(AurumParamsParamId::EqMHiShelf, g, ctx),
            // M/S EQ Side
            AurumMsg::EqSLoShGesture(g) => self.gesture_f(AurumParamsParamId::EqSLoShelf, g, ctx),
            AurumMsg::EqSLoMiGesture(g) => self.gesture_f(AurumParamsParamId::EqSLoMid, g, ctx),
            AurumMsg::EqSHiMiGesture(g) => self.gesture_f(AurumParamsParamId::EqSHiMid, g, ctx),
            AurumMsg::EqSHiShGesture(g) => self.gesture_f(AurumParamsParamId::EqSHiShelf, g, ctx),
            // Comp
            AurumMsg::CompSplitGesture(g) => self.gesture_f(AurumParamsParamId::CompSplit, g, ctx),
            AurumMsg::CompLinkToggled => { ctx.begin_edit(AurumParamsParamId::CompLink); ctx.set_param(AurumParamsParamId::CompLink, if self.params.comp_link.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::CompLink); }
            AurumMsg::CompThrLoGesture(g) => self.gesture_f(AurumParamsParamId::CompThreshLo, g, ctx),
            AurumMsg::CompThrHiGesture(g) => self.gesture_f(AurumParamsParamId::CompThreshHi, g, ctx),
            AurumMsg::CompRatioGesture(g) => self.gesture_f(AurumParamsParamId::CompRatio, g, ctx),
            AurumMsg::CompAtkGesture(g) => self.gesture_f(AurumParamsParamId::CompAttack, g, ctx),
            AurumMsg::CompRelGesture(g) => self.gesture_f(AurumParamsParamId::CompRelease, g, ctx),
            AurumMsg::CompMixGesture(g) => self.gesture_f(AurumParamsParamId::CompMix, g, ctx),
            // Sweetening
            AurumMsg::SweetHpfGesture(g) => self.gesture_f(AurumParamsParamId::SweetHpf, g, ctx),
            AurumMsg::SweetLpfGesture(g) => self.gesture_f(AurumParamsParamId::SweetLpf, g, ctx),
            AurumMsg::SweetLoGesture(g) => self.gesture_f(AurumParamsParamId::SweetLoShelf, g, ctx),
            AurumMsg::SweetHiGesture(g) => self.gesture_f(AurumParamsParamId::SweetHiShelf, g, ctx),
            // Saturator
            AurumMsg::SatMsToggled => { ctx.begin_edit(AurumParamsParamId::SatMsMode); ctx.set_param(AurumParamsParamId::SatMsMode, if self.params.sat_ms_mode.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::SatMsMode); }
            AurumMsg::SatDrvStGesture(g) => self.gesture_f(AurumParamsParamId::SatDriveStereo, g, ctx),
            AurumMsg::SatDrvMiGesture(g) => self.gesture_f(AurumParamsParamId::SatDriveMid, g, ctx),
            AurumMsg::SatDrvSiGesture(g) => self.gesture_f(AurumParamsParamId::SatDriveSide, g, ctx),
            AurumMsg::SatMixGesture(g) => self.gesture_f(AurumParamsParamId::SatMix, g, ctx),
            AurumMsg::SatHarmCycled => {
                let n = (self.params.sat_harmonics.value_i32() + 1) % 3;
                ctx.begin_edit(AurumParamsParamId::SatHarmonics); ctx.set_param(AurumParamsParamId::SatHarmonics, n as f64 / 2.0); ctx.end_edit(AurumParamsParamId::SatHarmonics);
            }
            // MB Limiter
            AurumMsg::MbXoverGesture(g) => self.gesture_f(AurumParamsParamId::MbCrossover, g, ctx),
            AurumMsg::MbThrLoGesture(g) => self.gesture_f(AurumParamsParamId::MbThreshMidLo, g, ctx),
            AurumMsg::MbThrHiGesture(g) => self.gesture_f(AurumParamsParamId::MbThreshMidHi, g, ctx),
            AurumMsg::MbThrSiGesture(g) => self.gesture_f(AurumParamsParamId::MbThreshSide, g, ctx),
            AurumMsg::MbGainLoGesture(g) => self.gesture_f(AurumParamsParamId::MbGainMidLo, g, ctx),
            AurumMsg::MbGainHiGesture(g) => self.gesture_f(AurumParamsParamId::MbGainMidHi, g, ctx),
            AurumMsg::MbGainSiGesture(g) => self.gesture_f(AurumParamsParamId::MbGainSide, g, ctx),
            AurumMsg::MbAtkLoGesture(g) => self.gesture_f(AurumParamsParamId::MbAttackMidLo, g, ctx),
            AurumMsg::MbAtkHiGesture(g) => self.gesture_f(AurumParamsParamId::MbAttackMidHi, g, ctx),
            AurumMsg::MbAtkSiGesture(g) => self.gesture_f(AurumParamsParamId::MbAttackSide, g, ctx),
            AurumMsg::MbRelLoGesture(g) => self.gesture_f(AurumParamsParamId::MbReleaseMidLo, g, ctx),
            AurumMsg::MbRelHiGesture(g) => self.gesture_f(AurumParamsParamId::MbReleaseMidHi, g, ctx),
            AurumMsg::MbRelSiGesture(g) => self.gesture_f(AurumParamsParamId::MbReleaseSide, g, ctx),
            AurumMsg::MbLinkToggled => { ctx.begin_edit(AurumParamsParamId::MbFaderLink); ctx.set_param(AurumParamsParamId::MbFaderLink, if self.params.mb_fader_link.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::MbFaderLink); }
            AurumMsg::MbGainGesture(g) => self.gesture_f(AurumParamsParamId::MbGlobalGain, g, ctx),
            AurumMsg::MbThrOffGesture(g) => self.gesture_f(AurumParamsParamId::MbGlobalThresh, g, ctx),
            AurumMsg::MbModeToggled => { ctx.begin_edit(AurumParamsParamId::MbMode); ctx.set_param(AurumParamsParamId::MbMode, if self.params.mb_mode.value() { 0.0 } else { 1.0 }); ctx.end_edit(AurumParamsParamId::MbMode); }
            // TP Lim
            AurumMsg::LimCeilGesture(g) => self.gesture_f(AurumParamsParamId::LimCeiling, g, ctx),
            AurumMsg::LimRelGesture(g) => self.gesture_f(AurumParamsParamId::LimRelease, g, ctx),
        }
        truce_iced::iced::Task::none()
    }

    fn view<'a>(&'a self, _params: &'a ParamCache<AurumParams>) -> Element<'a, Message<AurumMsg>> {
        let p = &self.params;

        // ── HEADER ──────────────────────────────────────────────────────────
        let header = container(row![
            container(header_brand("Aurum", VERSION)).width(Length::Shrink),
            Space::new().width(Length::Fill),
            row![
                Self::tab_btn("SHAPE", 0, self.selected_tab),
                Self::tab_btn("COLOR", 1, self.selected_tab),
                Self::tab_btn("LIMIT", 2, self.selected_tab),
            ].spacing(4),
            Space::new().width(Length::Fill),
            row![
                toggle_button("SIDE", p.side_active.value(), Message::Plugin(AurumMsg::SideToggled)),
                toggle_button("MONO", p.mono_active.value(), Message::Plugin(AurumMsg::MonoToggled)),
                toggle_button("Δ", p.delta_active.value(), Message::Plugin(AurumMsg::DeltaToggled)),
                toggle_button("BYPASS", p.bypass_active.value(), Message::Plugin(AurumMsg::BypassToggled)),
            ].spacing(6),
        ].align_y(Alignment::Center).spacing(8).padding(8))
        .width(Length::Fill).height(Length::Fixed(50.0))
        .style(|_t| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.08).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        });

        // ── TAB CONTENT ─────────────────────────────────────────────────────
        let tab_content: Element<Message<AurumMsg>> = match self.selected_tab {
            0 => self.shape_tab(),
            1 => self.color_tab(),
            _ => self.limit_tab(),
        };

        // ── RIGHT BAR ───────────────────────────────────────────────────────
        let right_bar = self.right_bar();

        // ── MAIN ────────────────────────────────────────────────────────────
        let body = container(row![
            tab_content,
            container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill)
                .style(|_t| container::Style { background: Some(Color::from_rgb(0.15, 0.15, 0.15).into()), ..Default::default() }),
            right_bar,
        ].spacing(0))
        .width(Length::Fill).height(Length::Fill)
        .style(|_t| container::Style { background: Some(Color::from_rgb(0.09, 0.09, 0.09).into()), ..Default::default() });

        column![header, body].into()
    }
}

// ─── Tab Builders ────────────────────────────────────────────────────────────

impl AurumEditor {
    fn section_label(text: &str) -> Element<'_, Message<AurumMsg>> {
        Text::new(text).size(10).font(bold_font()).color(Color::from_rgb(0.7, 0.7, 0.7)).into()
    }

    fn knob<'a>(label: &'a str, val: f32, min: f32, max: f32, def: f32, msg: impl Fn(Gesture) -> AurumMsg + 'a) -> Element<'a, Message<AurumMsg>> {
        knob_gesture(label, val, min, max, def, move |g| Message::Plugin(msg(g)))
    }

    fn shape_tab(&self) -> Element<'_, Message<AurumMsg>> {
        let p = &self.params;
        let in_db = self.shared_state.input_peak.load(Ordering::Relaxed);
        let inp = if in_db <= -90.0 { "--".to_string() } else { format!("{in_db:.1} dB") };

        container(column![
            // Input Monitor
            container(column![
                Self::section_label("INPUT"),
                Text::new(inp).size(18).font(bold_font()).color(AMBER),
            ].spacing(4).align_x(Alignment::Center)).width(Length::Fill),

            Space::new().height(Length::Fixed(8.0)),

            // Clipper + M/S EQ side by side
            row![
                // Clipper
                container(column![
                    Self::section_label("CLIPPER"),
                    Self::knob("Ceil dBTP", p.clip_ceiling.raw_target() as f32, -6.0, -0.1, -1.0, AurumMsg::ClipCeilGesture),
                    Self::knob("Soft %", p.clip_softness.raw_target() as f32, 0.0, 100.0, 50.0, AurumMsg::ClipSoftGesture),
                    toggle_button("M/S", p.clip_ms_mode.value(), Message::Plugin(AurumMsg::ClipMsToggled)),
                ].spacing(6).align_x(Alignment::Center)).width(Length::Fixed(120.0)),

                container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill)
                    .style(|_t| container::Style { background: Some(Color::from_rgb(0.15, 0.15, 0.15).into()), ..Default::default() }),

                // M/S EQ
                container(column![
                    Self::section_label("M/S EQ"),
                    row![
                        // Mid
                        column![
                            Text::new("MID").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
                            Self::knob("LoSh dB", p.eq_m_lo_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMLoShGesture),
                            Self::knob("LoMi dB", p.eq_m_lo_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMLoMiGesture),
                            Self::knob("HiMi dB", p.eq_m_hi_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMHiMiGesture),
                            Self::knob("HiSh dB", p.eq_m_hi_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqMHiShGesture),
                        ].spacing(4).align_x(Alignment::Center),
                        Space::new().width(Length::Fixed(12.0)),
                        // Side
                        column![
                            Text::new("SIDE").size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
                            Self::knob("LoSh dB", p.eq_s_lo_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSLoShGesture),
                            Self::knob("LoMi dB", p.eq_s_lo_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSLoMiGesture),
                            Self::knob("HiMi dB", p.eq_s_hi_mid.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSHiMiGesture),
                            Self::knob("HiSh dB", p.eq_s_hi_shelf.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::EqSHiShGesture),
                        ].spacing(4).align_x(Alignment::Center),
                    ].spacing(6),
                ].spacing(6).align_x(Alignment::Center)),
            ].spacing(6).align_y(Alignment::Start),
        ].spacing(8).padding(12))
        .width(Length::Fill).into()
    }

    fn color_tab(&self) -> Element<'_, Message<AurumMsg>> {
        let p = &self.params;

        container(column![
            row![
                // 2-Band Comp
                container(column![
                    Self::section_label("2-BAND COMP"),
                    Self::knob("Split Hz", p.comp_split.raw_target() as f32, 80.0, 500.0, 200.0, AurumMsg::CompSplitGesture),
                    toggle_button("LINK", p.comp_link.value(), Message::Plugin(AurumMsg::CompLinkToggled)),
                    Self::knob("ThrLo dB", p.comp_thresh_lo.raw_target() as f32, -30.0, 0.0, -12.0, AurumMsg::CompThrLoGesture),
                    Self::knob("ThrHi dB", p.comp_thresh_hi.raw_target() as f32, -30.0, 0.0, -12.0, AurumMsg::CompThrHiGesture),
                    Self::knob("Ratio :1", p.comp_ratio.raw_target() as f32, 1.2, 3.0, 1.5, AurumMsg::CompRatioGesture),
                    Self::knob("Atk ms", p.comp_attack.raw_target() as f32, 10.0, 100.0, 30.0, AurumMsg::CompAtkGesture),
                    Self::knob("Rel ms", p.comp_release.raw_target() as f32, 50.0, 500.0, 150.0, AurumMsg::CompRelGesture),
                    Self::knob("Mix %", p.comp_mix.raw_target() as f32, 0.0, 100.0, 50.0, AurumMsg::CompMixGesture),
                ].spacing(4).align_x(Alignment::Center)).width(Length::Fixed(140.0)),

                container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill)
                    .style(|_t| container::Style { background: Some(Color::from_rgb(0.15, 0.15, 0.15).into()), ..Default::default() }),

                // Sweetening EQ
                container(column![
                    Self::section_label("SWEETENING"),
                    Self::knob("HPF Hz", p.sweet_hpf.raw_target() as f32, 10.0, 60.0, 24.0, AurumMsg::SweetHpfGesture),
                    Self::knob("LPF Hz", p.sweet_lpf.raw_target() as f32, 18000.0, 40000.0, 35000.0, AurumMsg::SweetLpfGesture),
                    Self::knob("LoSh dB", p.sweet_lo_shelf.raw_target() as f32, -4.0, 4.0, 0.0, AurumMsg::SweetLoGesture),
                    Self::knob("HiSh dB", p.sweet_hi_shelf.raw_target() as f32, -4.0, 4.0, 0.0, AurumMsg::SweetHiGesture),
                ].spacing(4).align_x(Alignment::Center)).width(Length::Fixed(120.0)),

                container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill)
                    .style(|_t| container::Style { background: Some(Color::from_rgb(0.15, 0.15, 0.15).into()), ..Default::default() }),

                // Saturator
                container(column![
                    Self::section_label("SATURATOR"),
                    toggle_button("M/S", p.sat_ms_mode.value(), Message::Plugin(AurumMsg::SatMsToggled)),
                    Self::knob("DrvSt dB", p.sat_drive_stereo.raw_target() as f32, 0.0, 12.0, 0.0, AurumMsg::SatDrvStGesture),
                    Self::knob("DrvMi dB", p.sat_drive_mid.raw_target() as f32, 0.0, 12.0, 0.0, AurumMsg::SatDrvMiGesture),
                    Self::knob("DrvSi dB", p.sat_drive_side.raw_target() as f32, 0.0, 12.0, 0.0, AurumMsg::SatDrvSiGesture),
                    Self::knob("Mix %", p.sat_mix.raw_target() as f32, 0.0, 60.0, 20.0, AurumMsg::SatMixGesture),
                    {
                        let harm = ["EVEN", "ODD", "MIXED"][p.sat_harmonics.value_i32() as usize % 3];
                        toggle_button(harm, true, Message::Plugin(AurumMsg::SatHarmCycled))
                    },
                ].spacing(4).align_x(Alignment::Center)).width(Length::Fixed(120.0)),
            ].spacing(6).align_y(Alignment::Start),
        ].spacing(8).padding(12))
        .width(Length::Fill).into()
    }

    fn limit_tab(&self) -> Element<'_, Message<AurumMsg>> {
        let p = &self.params;

        container(column![
            // MB Limiter section
            Self::section_label("M/S MULTIBAND LIMITER"),
            row![
                Self::knob("Xover Hz", p.mb_crossover.raw_target() as f32, 20.0, 500.0, 250.0, AurumMsg::MbXoverGesture),
                Self::knob("G.Thr dB", p.mb_global_thresh.raw_target() as f32, -18.0, 0.0, 0.0, AurumMsg::MbThrOffGesture),
                Self::knob("G.Gain dB", p.mb_global_gain.raw_target() as f32, -6.0, 6.0, 0.0, AurumMsg::MbGainGesture),
                toggle_button("LINK", p.mb_fader_link.value(), Message::Plugin(AurumMsg::MbLinkToggled)),
                {
                    let mode = if p.mb_mode.value() { "MODERN" } else { "CLASSIC" };
                    toggle_button(mode, true, Message::Plugin(AurumMsg::MbModeToggled))
                },
            ].spacing(6).align_y(Alignment::Center),

            Space::new().height(Length::Fixed(12.0)),

            // MB Bands: Mid-Lo, Mid-Hi, Side
            row![
                band_column("MID-LO",
                    p.mb_thresh_mid_lo.raw_target() as f32, AurumMsg::MbThrLoGesture,
                    p.mb_attack_mid_lo.raw_target() as f32, AurumMsg::MbAtkLoGesture,
                    p.mb_release_mid_lo.raw_target() as f32, AurumMsg::MbRelLoGesture,
                    p.mb_gain_mid_lo.raw_target() as f32, AurumMsg::MbGainLoGesture,
                ),
                Space::new().width(Length::Fixed(8.0)),
                band_column("MID-HI",
                    p.mb_thresh_mid_hi.raw_target() as f32, AurumMsg::MbThrHiGesture,
                    p.mb_attack_mid_hi.raw_target() as f32, AurumMsg::MbAtkHiGesture,
                    p.mb_release_mid_hi.raw_target() as f32, AurumMsg::MbRelHiGesture,
                    p.mb_gain_mid_hi.raw_target() as f32, AurumMsg::MbGainHiGesture,
                ),
                Space::new().width(Length::Fixed(8.0)),
                band_column("SIDE",
                    p.mb_thresh_side.raw_target() as f32, AurumMsg::MbThrSiGesture,
                    p.mb_attack_side.raw_target() as f32, AurumMsg::MbAtkSiGesture,
                    p.mb_release_side.raw_target() as f32, AurumMsg::MbRelSiGesture,
                    p.mb_gain_side.raw_target() as f32, AurumMsg::MbGainSiGesture,
                ),
            ].spacing(8),

            Space::new().height(Length::Fixed(12.0)),

            // TP Limiter
            Self::section_label("TRUE PEAK LIMITER"),
            row![
                Self::knob("Ceil dBTP", p.lim_ceiling.raw_target() as f32, -6.0, -0.1, -1.0, AurumMsg::LimCeilGesture),
                Self::knob("Rel ms", p.lim_release.raw_target() as f32, 10.0, 500.0, 100.0, AurumMsg::LimRelGesture),
            ].spacing(6),

            Space::new().height(Length::Fixed(8.0)),

            // Output Monitor
            Self::section_label("OUTPUT"),
            {
                let gr = self.shared_state.gain_reduction.load(Ordering::Relaxed);
                let gr_str = if gr >= -0.01 { "0.0 dB".to_string() } else { format!("{gr:.1} dB") };
                Text::new(format!("GR: {gr_str} | L:{:.1} R:{:.1} dB", self.peak_l, self.peak_r))
                    .size(12).font(bold_font()).color(AMBER)
            },
        ].spacing(6).padding(12))
        .width(Length::Fill).into()
    }

    fn right_bar(&self) -> Element<'_, Message<AurumMsg>> {
        let p = &self.params;
        let scope_pos = self.shared_state.scope_write_pos.load(Ordering::Acquire);

        container(column![
            // Goniometer
            canvas(GoniometerCanvas {
                samples: self.shared_state.scope_samples.clone(),
                write_pos: scope_pos,
                correlation: self.phase_correlation,
            }).width(Length::Fixed(180.0)).height(Length::Fixed(180.0)),

            Space::new().height(Length::Fixed(8.0)),

            // Output Level
            output_level_block(
                self.peak_l, self.peak_r,
                self.peak_hold_l, self.peak_hold_r,
                self.peak_hold,
                Message::Plugin(AurumMsg::ResetPeak),
                self.balance,
                Length::Fixed(100.0),
            ),

            Space::new().height(Length::Fixed(8.0)),

            // Stereo Width + Mono Floor
            column![
                Self::section_label("STEREO"),
                knob_gesture_bipolar("WIDTH", p.stereo_width.raw_target() as f32, 0.0, 2.0, 1.0, |g| Message::Plugin(AurumMsg::StereoWidthGesture(g))),
                Self::knob("M.Floor Hz", p.mono_floor.raw_target() as f32, 0.0, 300.0, 0.0, AurumMsg::MonoFloorGesture),
            ].spacing(4).align_x(Alignment::Center),

            Space::new().height(Length::Fixed(8.0)),

            // Output Gain
            knob_gesture_bipolar("GAIN", p.output_gain.raw_target() as f32, -12.0, 12.0, 0.0, |g| Message::Plugin(AurumMsg::OutputGainGesture(g))),

            Space::new().height(Length::Fixed(8.0)),

            // AT Block
            at_block(
                p.at_active.value(),
                p.at_amount.raw_target() as f32,
                Message::Plugin(AurumMsg::AtToggled),
                |v| Message::Plugin(AurumMsg::AtAmountGesture(Gesture::Change(v))),
            ),

            Space::new().height(Length::Fixed(12.0)),

            output_tools_strip(Message::Plugin(AurumMsg::ResetAll)),
        ].spacing(2).padding(8).align_x(Alignment::Center))
        .width(Length::Fixed(200.0))
        .height(Length::Fill)
        .style(|_t| container::Style {
            background: Some(Color::from_rgb(0.07, 0.07, 0.07).into()),
            border: Border { color: Color::from_rgb(0.15, 0.15, 0.15), width: 1.0, ..Default::default() },
            ..Default::default()
        }).into()
    }
}

fn band_column(
    label: &str,
    thr: f32, thr_msg: impl Fn(Gesture) -> AurumMsg + 'static,
    atk: f32, atk_msg: impl Fn(Gesture) -> AurumMsg + 'static,
    rel: f32, rel_msg: impl Fn(Gesture) -> AurumMsg + 'static,
    gain: f32, gain_msg: impl Fn(Gesture) -> AurumMsg + 'static,
) -> Element<'_, Message<AurumMsg>> {
    column![
        Text::new(label).size(10).font(bold_font()).color(Color::from_rgb(0.6, 0.6, 0.6)),
        AurumEditor::knob("Thr dB", thr, -18.0, 0.0, -3.0, thr_msg),
        AurumEditor::knob("Atk ms", atk, 0.1, 50.0, 5.0, atk_msg),
        AurumEditor::knob("Rel ms", rel, 10.0, 500.0, 100.0, rel_msg),
        AurumEditor::knob("Gain dB", gain, -6.0, 6.0, 0.0, gain_msg),
    ].spacing(4).align_x(Alignment::Center).into()
}
