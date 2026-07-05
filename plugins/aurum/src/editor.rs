//! Vizia port of Aurum's Iced editor — 60+ params, 3 tabs, goniometer, meters.
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use vizia::prelude::*;
use vizia::vg;
use shared_analysis::SharedState;
use truce_vizia::ParamLens;
use shared_ui::{GoniometerView, StereoMeterView, Gesture, KnobView};
use crate::{AurumParams, AurumParamsParamId as P};

const VERSION: &str = env!("CARGO_PKG_VERSION");
fn rgb(r: f32, g: f32, b: f32) -> Color { Color::rgba((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255) }
fn col(r: f32, g: f32, b: f32, a: f32) -> Color { Color::rgba((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, (a * 255.0) as u8) }

#[derive(Clone)] struct Tel { peak_l: f32, peak_r: f32, peak_hl: f32, peak_hr: f32, peak_h: f32, phase_c: f32, bal: f32, in_peak: f32, gr: f32, snap: u32 }

struct Ticker { shared: Arc<SharedState>, tel: Signal<Tel>, snap: Signal<u32>, lt: RefCell<Instant> }
impl Ticker { fn new(cx: &mut Context, s: Arc<SharedState>, tel: Signal<Tel>, snap: Signal<u32>) -> Handle<'_, Self> { Self { shared: s, tel, snap, lt: RefCell::new(Instant::now()) }.build(cx, |_| {}) } }
impl View for Ticker {
    fn element(&self) -> Option<&'static str> { Some("ticker") }
    fn draw(&self, cx: &mut DrawContext, _: &vg::Canvas) {
        let due = { let mut l = self.lt.borrow_mut(); if Instant::now().duration_since(*l) >= Duration::from_millis(33) { *l = Instant::now(); true } else { false } };
        if due {
            let mut s = self.snap.get(); if s > 0 { s -= 1; self.snap.set(s); }
            self.tel.update(|t| *t = Tel { peak_l: self.shared.output_peak_l.load(Ordering::Relaxed), peak_r: self.shared.output_peak_r.load(Ordering::Relaxed), peak_hl: self.shared.peak_hold_l.load(Ordering::Relaxed), peak_hr: self.shared.peak_hold_r.load(Ordering::Relaxed), peak_h: self.shared.peak_hold.load(Ordering::Relaxed), phase_c: self.shared.phase_correlation.load(Ordering::Relaxed), bal: self.shared.balance.load(Ordering::Relaxed), in_peak: self.shared.input_peak.load(Ordering::Relaxed), gr: self.shared.gain_reduction.load(Ordering::Relaxed), snap: s, });
        }
        cx.needs_redraw();
    }
}

fn kn<'a>(cx: &'a mut Context, lbl: &'static str, norm: f32, def: f32, min: f32, max: f32, bip: bool, on: impl Fn(&mut EventContext, Gesture) + 'static) -> Handle<'a, impl View> { VStack::new(cx, move |cx| { Label::new(cx, lbl).font_size(8.0).color(col(0.55, 0.55, 0.55, 1.0)); KnobView::new(cx, norm, def, min, max, bip, on).width(Pixels(30.0)).height(Pixels(30.0)); }).width(Auto).vertical_gap(Pixels(2.0)).alignment(Alignment::Center) }
fn tog<'a>(cx: &'a mut Context, lbl: &'static str, act: bool, on: impl Fn(&mut EventContext) + 'static + Send + Sync) -> Handle<'a, impl View> { let s = Signal::new(act); Button::new(cx, move |cx| Label::new(cx, lbl).font_size(9.0)).on_press(move |cx| { s.update(|v| *v = !*v); on(cx); }).width(Pixels(52.0)).height(Pixels(22.0)).background_color(Memo::new(move |_| if s.get() { col(0.25, 0.12, 0.05, 1.0) } else { col(0.14, 0.14, 0.14, 1.0) })) }
fn cyc<'a>(cx: &'a mut Context, lbls: &'static [&'static str], cur: usize, on: impl Fn(&mut EventContext) + 'static + Send + Sync) -> Handle<'a, impl View> { Button::new(cx, move |cx| Label::new(cx, lbls[cur % lbls.len()]).font_size(9.0)).on_press(on).width(Pixels(64.0)).height(Pixels(22.0)).background_color(col(0.2, 0.12, 0.05, 1.0)) }
fn strip(cx: &mut Context, t: &'static str) { Label::new(cx, t).font_size(10.0).color(Color::rgb(255, 140, 26)); }
fn norm(v: f32, min: f32, max: f32) -> f32 { ((v - min) / (max - min)).clamp(0.0, 1.0) }
fn bip(v: f32, min: f32, max: f32) -> f32 { let m = (min + max) * 0.5; ((v - m) / (max - m) * 0.5 + 0.5).clamp(0.0, 1.0) }
fn set(l: &ParamLens<AurumParams>, id: P, n: f32) { l.automate(id, n as f64); }
fn tgl(l: &ParamLens<AurumParams>, id: P) { set(l, id, if l.get_plain(id) != 0.0 { 0.0 } else { 1.0 }); }

pub fn build(cx: &mut Context, lens: ParamLens<AurumParams>, params: Arc<AurumParams>, shared: Arc<SharedState>) {
    let cfg = shared_analysis::load_config("Aurum"); let vp = Signal::new(cfg.vault_path.clone()); let show = Signal::new(false); let vpi = Signal::new(cfg.vault_path.unwrap_or_default()); let tab = Signal::new(0usize);
    let tel = Signal::new(Tel { peak_l: -90.0, peak_r: -90.0, peak_hl: -90.0, peak_hr: -90.0, peak_h: -90.0, phase_c: 1.0, bal: 0.0, in_peak: -90.0, gr: 0.0, snap: 0 }); let snap_sig = Signal::new(0u32);
    Ticker::new(cx, shared.clone(), tel, snap_sig).width(Pixels(1.0)).height(Pixels(1.0));
    let lh = lens.clone(); let ph = params.clone();
    let _lb = lens.clone(); let _pb = params.clone(); let sb = shared.clone();
    let lr = lens.clone(); let pr = params.clone(); let sr = shared.clone();
    let lf = lens.clone(); let pf = params.clone(); let sf = shared.clone();
    let lt = lens.clone(); let pt = params.clone();
    VStack::new(cx, move |cx| {
        // HEADER
        HStack::new(cx, move |cx| {
            HStack::new(cx, |cx| { Label::new(cx, "LX").font_size(18.0).color(rgb(1.0, 0.45, 0.1)); Label::new(cx, "AUDIOLABS").font_size(18.0).color(Color::white()); Label::new(cx, format!("Aurum {VERSION}")).font_size(9.0).color(col(0.55, 0.55, 0.55, 1.0)); }).width(Auto).alignment(Alignment::Center);
            Element::new(cx).width(Stretch(1.0));
            for (i, lb) in ["SHAPE", "COLOR", "LIMIT"].iter().enumerate() { let ts = tab; Button::new(cx, move |cx| Label::new(cx, *lb).font_size(12.0)).on_press(move |_cx| ts.set(i)).width(Pixels(70.0)).height(Pixels(28.0)).background_color(Memo::new(move |_| if tab.get() == i { col(0.25, 0.15, 0.05, 1.0) } else { col(0.12, 0.12, 0.12, 1.0) })); }
            Element::new(cx).width(Stretch(1.0));
            let lc = lh.clone(); tog(cx, "SIDE", ph.side_active.value(), move |_cx| tgl(&lc, P::SideActive));
            let lc = lh.clone(); tog(cx, "MONO", ph.mono_active.value(), move |_cx| tgl(&lc, P::MonoActive));
            let lc = lh.clone(); tog(cx, "Δ", ph.delta_active.value(), move |_cx| tgl(&lc, P::DeltaActive));
            let lc = lh.clone(); tog(cx, "BYPASS", ph.bypass_active.value(), move |_cx| tgl(&lc, P::BypassActive));
        }).width(Stretch(1.0)).height(Pixels(44.0)).padding(Pixels(8.0)).alignment(Alignment::Center).background_color(rgb(0.08, 0.08, 0.08)).horizontal_gap(Pixels(4.0));
        // BODY
        HStack::new(cx, move |cx| {
            VStack::new(cx, move |cx| {
                Label::new(cx, "LX AUDIOLABS").font_size(14.0).color(Color::white());
                let sn = snap_sig;
                Button::new(cx, move |cx| { Label::new(cx, Memo::new(move |_| if sn.get() > 0 { "ANALYZING..." } else { "SNAP" })).font_size(12.0) })
                    .on_press(move |_cx| { sn.set(72); sb.snap_active.store(true, Ordering::Relaxed); }).width(Stretch(1.0)).height(Pixels(34.0))
                    .background_color(Memo::new(move |_| if sn.get() > 0 { col(0.55, 0.38, 0.05, 1.0) } else { col(0.18, 0.18, 0.18, 1.0) }));
                Button::new(cx, |cx| Label::new(cx, "VAULT SETUP").font_size(12.0)).on_press(move |_cx| show.update(|v| *v = !*v)).width(Stretch(1.0)).height(Pixels(34.0));
            }).width(Pixels(180.0)).height(Stretch(1.0)).padding(Pixels(10.0)).vertical_gap(Pixels(10.0)).background_color(rgb(0.09, 0.09, 0.09));
            Binding::new(cx, show, move |cx| {
                if show.get() { vault_form(cx, vpi, vp, show); }
                else {
                    let lt2 = lt.clone(); let pt2 = pt.clone();
                    VStack::new(cx, move |cx| { Binding::new(cx, tab, move |cx| { let tp = tab.get(); match tp { 0 => shape(cx, lt2.clone(), pt2.clone(), tel), 1 => color(cx, lt2.clone(), pt2.clone(), tel), _ => limit(cx, lt2.clone(), pt2.clone(), tel) } }); }).width(Stretch(1.0)).height(Stretch(1.0)).background_color(rgb(0.08, 0.08, 0.08));
                }
            });
            VStack::new(cx, move |cx| {
                let lc = lr.clone(); kn(cx, "OUT GAIN", bip(pr.output_gain.raw_target() as f32, -12.0, 12.0), 0.5, -12.0, 12.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::OutputGain, ((v + 12.0) / 24.0).clamp(0.0, 1.0)); } });
                Binding::new(cx, tel, move |cx| { let t = tel.get(); StereoMeterView::new(cx, t.peak_l, t.peak_r, t.peak_hl, t.peak_hr, t.bal).width(Pixels(180.0)).height(Pixels(220.0)); Label::new(cx, Memo::new(move |_| if tel.get().peak_h <= -90.0 { "--".to_string() } else { format!("{:.1} dB", tel.get().peak_h) })).font_size(9.0).color(col(0.6, 0.6, 0.6, 1.0)); });
                let sg = sr.clone(); Binding::new(cx, tel, move |cx| { let t = tel.get(); GoniometerView::new(cx, sg.scope_samples.clone(), sg.scope_write_pos.load(Ordering::Acquire), t.phase_c).width(Stretch(1.0)).height(Pixels(155.0)); });
                let sr2 = sr.clone(); Button::new(cx, |cx| Label::new(cx, "RESET PEAK").font_size(9.0)).on_press(move |_cx| sr2.reset_peak.store(true, Ordering::Relaxed)).width(Stretch(1.0)).height(Pixels(24.0));
            }).width(Pixels(200.0)).height(Stretch(1.0)).padding(Pixels(6.0)).vertical_gap(Pixels(4.0)).alignment(Alignment::Center).background_color(rgb(0.07, 0.07, 0.07));
        }).width(Stretch(1.0)).height(Stretch(1.0));
        // FOOTER — lf/pf pre-cloned inside HStack for AT + STEREO sections
        HStack::new(cx, move |cx| {
            let lfa = lf.clone(); let lfs = lf.clone();
            let pfa = pf.clone(); let pfs = pf.clone();
            Element::new(cx).width(Stretch(1.0));
            VStack::new(cx, move |cx| {
                let lc = lfa.clone(); tog(cx, "AT", pfa.at_active.value(), move |_cx| tgl(&lc, P::AtActive));
                let ld = lfa.clone(); kn(cx, "AMOUNT", pfa.at_amount.raw_target() as f32 / 100.0, 0.5, 0.0, 100.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&ld, P::AtAmount, (v / 100.0).clamp(0.0, 1.0)); } });
            }).width(Auto).vertical_gap(Pixels(2.0)).alignment(Alignment::Center);
            Element::new(cx).width(Stretch(1.0));
            VStack::new(cx, move |cx| {
                Label::new(cx, "STEREO/ROUTING").font_size(10.0).color(col(0.6, 0.6, 0.6, 1.0));
                HStack::new(cx, move |cx| {
                    let lc = lfs.clone(); kn(cx, "WIDTH", pfs.stereo_width.raw_target() as f32 / 2.0, 0.5, 0.0, 2.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::StereoWidth, (v / 2.0).clamp(0.0, 1.0)); } });
                    let ld = lfs.clone(); kn(cx, "M.FLOOR", pfs.mono_floor.raw_target() as f32 / 300.0, 0.0, 0.0, 300.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&ld, P::MonoFloor, (v / 300.0).clamp(0.0, 1.0)); } });
                    let le = lfs.clone(); let se = sf.clone();
                    Button::new(cx, |cx| Label::new(cx, "RESET").font_size(11.0)).on_press(move |_cx| { se.reset_peak.store(true, Ordering::Relaxed); set(&le, P::ClipCeiling, 0.8167); set(&le, P::OutputGain, 0.5); set(&le, P::StereoWidth, 0.5); }).width(Pixels(60.0)).height(Pixels(26.0));
                }).horizontal_gap(Pixels(8.0)).alignment(Alignment::Center);
            }).vertical_gap(Pixels(2.0)).alignment(Alignment::Center).width(Auto);
        }).width(Stretch(1.0)).height(Pixels(60.0)).padding(Pixels(8.0)).alignment(Alignment::Center).background_color(rgb(0.1, 0.1, 0.1));
    }).width(Pixels(1100.0)).height(Pixels(660.0)).background_color(rgb(0.09, 0.09, 0.09));
}

fn vault_form(cx: &mut Context, vpi: Signal<String>, vp: Signal<Option<String>>, show: Signal<bool>) { VStack::new(cx, move |cx| { Element::new(cx).height(Stretch(1.0)); HStack::new(cx, move |cx| { Element::new(cx).width(Stretch(1.0)); VStack::new(cx, move |cx| { Label::new(cx, "Vault Path").font_size(14.0).color(Color::white()); Textbox::new(cx, vpi).width(Pixels(350.0)); HStack::new(cx, move |cx| { Button::new(cx, |cx| Label::new(cx, "SAVE").font_size(11.0)).on_press(move |_cx| { let p = vpi.get().trim().to_string(); let n = if p.is_empty() { None } else { Some(p) }; vp.set(n.clone()); let mut c = shared_analysis::load_config("Aurum"); c.vault_path = n; let _ = shared_analysis::save_config("Aurum", &c); show.set(false); }).width(Pixels(60.0)).height(Pixels(26.0)); Button::new(cx, |cx| Label::new(cx, "CANCEL").font_size(11.0)).on_press(move |_cx| show.set(false)).width(Pixels(60.0)).height(Pixels(26.0)); }).width(Auto).horizontal_gap(Pixels(8.0)); }).width(Auto).vertical_gap(Pixels(8.0)); Element::new(cx).width(Stretch(1.0)); }).width(Stretch(1.0)).height(Stretch(1.0)).alignment(Alignment::Center); Element::new(cx).height(Stretch(1.0)); }).width(Stretch(1.0)).height(Stretch(1.0)).background_color(rgb(0.08, 0.08, 0.08)); }

fn shape(cx: &mut Context, l: ParamLens<AurumParams>, p: Arc<AurumParams>, tel: Signal<Tel>) { VStack::new(cx, move |cx| {
    HStack::new(cx, move |cx| { strip(cx, "INPUT"); Element::new(cx).width(Pixels(12.0)); Binding::new(cx, tel, move |cx| { let t = tel.get(); Label::new(cx, if t.in_peak <= -90.0 { "--".to_string() } else { format!("{:.1} dB", t.in_peak) }).font_size(14.0).color(Color::rgb(255, 140, 26)); }); }).width(Stretch(1.0)).padding(Pixels(8.0));
    let ls = l.clone(); let ps = p.clone();
    HStack::new(cx, move |cx| { strip(cx, "CLIPPER"); Element::new(cx).width(Pixels(12.0));
        let lc = ls.clone(); kn(cx, "CEIL dBTP", norm(ps.clip_ceiling.raw_target() as f32, -6.0, -0.1), 0.847, -6.0, -0.1, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::ClipCeiling, ((v + 6.0) / 5.9).clamp(0.0, 1.0)); } });
        let lc = ls.clone(); kn(cx, "SOFT %", ps.clip_softness.raw_target() as f32 / 100.0, 0.5, 0.0, 100.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::ClipSoftness, (v / 100.0).clamp(0.0, 1.0)); } });
        let lc = ls.clone(); tog(cx, "M/S", ps.clip_ms_mode.value(), move |_cx| tgl(&lc, P::ClipMsMode));
    }).width(Stretch(1.0)).padding(Pixels(8.0)).background_color(col(0.11, 0.11, 0.11, 1.0)).horizontal_gap(Pixels(4.0)).alignment(Alignment::Center);
    let lm = l.clone(); let pm = p.clone();
    HStack::new(cx, move |cx| { strip(cx, "M/S EQ"); Element::new(cx).width(Pixels(12.0));
        let ld = lm.clone(); let pd = pm.clone();
        VStack::new(cx, move |cx| { Label::new(cx, "MID").font_size(9.0).color(col(0.6, 0.6, 0.6, 1.0)); HStack::new(cx, move |cx| {
            let lc = lm.clone(); kn(cx, "LO SH", bip(pm.eq_m_lo_shelf.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqMLoShelf, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
            let lc = lm.clone(); kn(cx, "LO-MI", bip(pm.eq_m_lo_mid.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqMLoMid, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
            let lc = lm.clone(); kn(cx, "HI-MI", bip(pm.eq_m_hi_mid.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqMHiMid, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
            let lc = lm.clone(); kn(cx, "HI SH", bip(pm.eq_m_hi_shelf.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqMHiShelf, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
        }).horizontal_gap(Pixels(4.0)); }).vertical_gap(Pixels(2.0)).alignment(Alignment::Center);
        Element::new(cx).width(Pixels(16.0));
        VStack::new(cx, move |cx| { Label::new(cx, "SIDE").font_size(9.0).color(col(0.6, 0.6, 0.6, 1.0)); HStack::new(cx, move |cx| {
            let lc = ld.clone(); kn(cx, "LO SH", bip(pd.eq_s_lo_shelf.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqSLoShelf, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
            let lc = ld.clone(); kn(cx, "LO-MI", bip(pd.eq_s_lo_mid.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqSLoMid, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
            let lc = ld.clone(); kn(cx, "HI-MI", bip(pd.eq_s_hi_mid.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqSHiMid, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
            let lc = ld.clone(); kn(cx, "HI SH", bip(pd.eq_s_hi_shelf.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::EqSHiShelf, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
        }).horizontal_gap(Pixels(4.0)); }).vertical_gap(Pixels(2.0)).alignment(Alignment::Center);
    }).width(Stretch(1.0)).padding(Pixels(8.0)).background_color(col(0.1, 0.1, 0.1, 1.0)).horizontal_gap(Pixels(4.0)).alignment(Alignment::Center);
}).width(Stretch(1.0)).vertical_gap(Pixels(2.0)); }

fn color(cx: &mut Context, l: ParamLens<AurumParams>, p: Arc<AurumParams>, _tel: Signal<Tel>) { VStack::new(cx, move |cx| {
    let lb2 = l.clone(); let pb2 = p.clone();
    HStack::new(cx, move |cx| { strip(cx, "2-BAND COMP"); Element::new(cx).width(Pixels(12.0));
        let lc = lb2.clone(); kn(cx, "SPLIT Hz", norm(pb2.comp_split.raw_target() as f32, 80.0, 500.0), 0.286, 80.0, 500.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::CompSplit, ((v - 80.0) / 420.0).clamp(0.0, 1.0)); } });
        let lc = lb2.clone(); tog(cx, "LINK", pb2.comp_link.value(), move |_cx| tgl(&lc, P::CompLink));
        let lc = lb2.clone(); kn(cx, "THR LO", norm(pb2.comp_thresh_lo.raw_target() as f32, -30.0, 0.0), 0.4, -30.0, 0.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::CompThreshLo, ((v + 30.0) / 30.0).clamp(0.0, 1.0)); } });
        let lc = lb2.clone(); kn(cx, "THR HI", norm(pb2.comp_thresh_hi.raw_target() as f32, -30.0, 0.0), 0.4, -30.0, 0.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::CompThreshHi, ((v + 30.0) / 30.0).clamp(0.0, 1.0)); } });
        let lc = lb2.clone(); kn(cx, "RATIO", norm(pb2.comp_ratio.raw_target() as f32, 1.2, 3.0), 0.167, 1.2, 3.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::CompRatio, ((v - 1.2) / 1.8).clamp(0.0, 1.0)); } });
        let lc = lb2.clone(); kn(cx, "ATK ms", norm(pb2.comp_attack.raw_target() as f32, 10.0, 100.0), 0.222, 10.0, 100.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::CompAttack, ((v - 10.0) / 90.0).clamp(0.0, 1.0)); } });
        let lc = lb2.clone(); kn(cx, "REL ms", norm(pb2.comp_release.raw_target() as f32, 50.0, 500.0), 0.222, 50.0, 500.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::CompRelease, ((v - 50.0) / 450.0).clamp(0.0, 1.0)); } });
        let lc = lb2.clone(); kn(cx, "MIX %", pb2.comp_mix.raw_target() as f32 / 100.0, 0.5, 0.0, 100.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::CompMix, (v / 100.0).clamp(0.0, 1.0)); } });
    }).width(Stretch(1.0)).padding(Pixels(8.0)).background_color(col(0.11, 0.11, 0.11, 1.0)).horizontal_gap(Pixels(4.0)).alignment(Alignment::Center);
    let ls2 = l.clone(); let ps2 = p.clone();
    HStack::new(cx, move |cx| { strip(cx, "SWEETENING"); Element::new(cx).width(Pixels(12.0));
        let lc = ls2.clone(); kn(cx, "HPF Hz", norm(ps2.sweet_hpf.raw_target() as f32, 10.0, 60.0), 0.28, 10.0, 60.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SweetHpf, ((v - 10.0) / 50.0).clamp(0.0, 1.0)); } });
        let lc = ls2.clone(); kn(cx, "LPF Hz", norm(ps2.sweet_lpf.raw_target() as f32, 18000.0, 40000.0), 0.773, 18000.0, 40000.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SweetLpf, ((v - 18000.0) / 22000.0).clamp(0.0, 1.0)); } });
        let lc = ls2.clone(); kn(cx, "LO SH", bip(ps2.sweet_lo_shelf.raw_target() as f32, -4.0, 4.0), 0.5, -4.0, 4.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SweetLoShelf, ((v + 4.0) / 8.0).clamp(0.0, 1.0)); } });
        let lc = ls2.clone(); kn(cx, "HI SH", bip(ps2.sweet_hi_shelf.raw_target() as f32, -4.0, 4.0), 0.5, -4.0, 4.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SweetHiShelf, ((v + 4.0) / 8.0).clamp(0.0, 1.0)); } });
    }).width(Stretch(1.0)).padding(Pixels(8.0)).background_color(col(0.1, 0.1, 0.1, 1.0)).horizontal_gap(Pixels(4.0)).alignment(Alignment::Center);
    let lt2 = l.clone(); let pt2 = p.clone();
    HStack::new(cx, move |cx| { strip(cx, "SATURATOR"); Element::new(cx).width(Pixels(12.0));
        let lc = lt2.clone(); tog(cx, "M/S", pt2.sat_ms_mode.value(), move |_cx| tgl(&lc, P::SatMsMode));
        let lc = lt2.clone(); kn(cx, "DRV ST", pt2.sat_drive_stereo.raw_target() as f32 / 12.0, 0.0, 0.0, 12.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SatDriveStereo, (v / 12.0).clamp(0.0, 1.0)); } });
        let lc = lt2.clone(); kn(cx, "DRV MI", pt2.sat_drive_mid.raw_target() as f32 / 12.0, 0.0, 0.0, 12.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SatDriveMid, (v / 12.0).clamp(0.0, 1.0)); } });
        let lc = lt2.clone(); kn(cx, "DRV SI", pt2.sat_drive_side.raw_target() as f32 / 12.0, 0.0, 0.0, 12.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SatDriveSide, (v / 12.0).clamp(0.0, 1.0)); } });
        let lc = lt2.clone(); kn(cx, "MIX %", pt2.sat_mix.raw_target() as f32 / 60.0, 0.333, 0.0, 60.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::SatMix, (v / 60.0).clamp(0.0, 1.0)); } });
        let ld = lt2.clone(); let pd = pt2.clone(); let h = pd.sat_harmonics.value_i32() as usize % 3; cyc(cx, &["EVEN", "ODD", "MIXED"], h, move |_cx| { let n = (pd.sat_harmonics.value_i32() + 1) % 3; set(&ld, P::SatHarmonics, n as f32 / 2.0); });
    }).width(Stretch(1.0)).padding(Pixels(8.0)).background_color(col(0.11, 0.11, 0.11, 1.0)).horizontal_gap(Pixels(4.0)).alignment(Alignment::Center);
}).width(Stretch(1.0)).vertical_gap(Pixels(2.0)); }

fn limit(cx: &mut Context, l: ParamLens<AurumParams>, p: Arc<AurumParams>, tel: Signal<Tel>) { VStack::new(cx, move |cx| {
    let li = l.clone(); let pi = p.clone();
    VStack::new(cx, move |cx| {
        let li1 = li.clone(); let pi1 = pi.clone();
        HStack::new(cx, move |cx| { strip(cx, "M/S MB LIMITER"); Element::new(cx).width(Pixels(8.0));
            let lc = li1.clone(); kn(cx, "XOVER Hz", norm(pi1.mb_crossover.raw_target() as f32, 20.0, 500.0), 0.479, 20.0, 500.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::MbCrossover, ((v - 20.0) / 480.0).clamp(0.0, 1.0)); } });
            let lc = li1.clone(); kn(cx, "G.THR dB", norm(pi1.mb_global_thresh.raw_target() as f32, -18.0, 0.0), 1.0, -18.0, 0.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::MbGlobalThresh, ((v + 18.0) / 18.0).clamp(0.0, 1.0)); } });
            let lc = li1.clone(); kn(cx, "G.GAIN dB", bip(pi1.mb_global_gain.raw_target() as f32, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::MbGlobalGain, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
            let lc = li1.clone(); tog(cx, "LINK", pi1.mb_fader_link.value(), move |_cx| tgl(&lc, P::MbFaderLink));
            let lc = li1.clone(); cyc(cx, &["MODERN", "CLASSIC"], if pi1.mb_mode.value() { 0 } else { 1 }, move |_cx| tgl(&lc, P::MbMode));
        }).width(Stretch(1.0)).horizontal_gap(Pixels(4.0)).alignment(Alignment::Center);
        Element::new(cx).height(Pixels(4.0));
        let li2 = li.clone(); let pi2 = pi.clone();
        HStack::new(cx, move |cx| {
            bcol(cx, li2.clone(), "MID-LO", pi2.mb_thresh_mid_lo.raw_target() as f32, P::MbThreshMidLo, pi2.mb_attack_mid_lo.raw_target() as f32, P::MbAttackMidLo, pi2.mb_release_mid_lo.raw_target() as f32, P::MbReleaseMidLo, pi2.mb_gain_mid_lo.raw_target() as f32, P::MbGainMidLo);
            Element::new(cx).width(Pixels(16.0));
            bcol(cx, li2.clone(), "MID-HI", pi2.mb_thresh_mid_hi.raw_target() as f32, P::MbThreshMidHi, pi2.mb_attack_mid_hi.raw_target() as f32, P::MbAttackMidHi, pi2.mb_release_mid_hi.raw_target() as f32, P::MbReleaseMidHi, pi2.mb_gain_mid_hi.raw_target() as f32, P::MbGainMidHi);
            Element::new(cx).width(Pixels(16.0));
            bcol(cx, li2.clone(), "SIDE", pi2.mb_thresh_side.raw_target() as f32, P::MbThreshSide, pi2.mb_attack_side.raw_target() as f32, P::MbAttackSide, pi2.mb_release_side.raw_target() as f32, P::MbReleaseSide, pi2.mb_gain_side.raw_target() as f32, P::MbGainSide);
        }).horizontal_gap(Pixels(4.0));
    }).width(Stretch(1.0)).padding(Pixels(8.0)).background_color(col(0.11, 0.11, 0.11, 1.0)).vertical_gap(Pixels(4.0));
    let lt = l.clone(); let pt = p.clone();
    HStack::new(cx, move |cx| { strip(cx, "TP LIMITER"); Element::new(cx).width(Pixels(12.0));
        let lc = lt.clone(); kn(cx, "CEIL dBTP", norm(pt.lim_ceiling.raw_target() as f32, -6.0, -0.1), 0.847, -6.0, -0.1, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::LimCeiling, ((v + 6.0) / 5.9).clamp(0.0, 1.0)); } });
        let lc = lt.clone(); kn(cx, "REL ms", norm(pt.lim_release.raw_target() as f32, 10.0, 500.0), 0.184, 10.0, 500.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, P::LimRelease, ((v - 10.0) / 490.0).clamp(0.0, 1.0)); } });
        Element::new(cx).width(Pixels(20.0)); Binding::new(cx, tel, move |cx| { let t = tel.get(); let gr = if t.gr >= -0.01 { "0.0 dB".to_string() } else { format!("{:.1} dB", t.gr) }; Label::new(cx, format!("GR {gr} | L {:.1} R {:.1} dB", t.peak_l, t.peak_r)).font_size(11.0).color(Color::rgb(255, 140, 26)); });
    }).width(Stretch(1.0)).padding(Pixels(8.0)).background_color(col(0.1, 0.1, 0.1, 1.0)).horizontal_gap(Pixels(4.0)).alignment(Alignment::Center);
}).width(Stretch(1.0)).vertical_gap(Pixels(2.0)); }

fn bcol(cx: &mut Context, l: ParamLens<AurumParams>, label: &'static str, thr: f32, thr_id: P, atk: f32, atk_id: P, rel: f32, rel_id: P, gain: f32, gain_id: P) { VStack::new(cx, move |cx| { Label::new(cx, label).font_size(9.0).color(col(0.6, 0.6, 0.6, 1.0));
    let lc = l.clone(); kn(cx, "THR dB", norm(thr, -18.0, 0.0), 0.167, -18.0, 0.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, thr_id, ((v + 18.0) / 18.0).clamp(0.0, 1.0)); } });
    let lc = l.clone(); kn(cx, "ATK ms", norm(atk, 0.1, 50.0), 0.098, 0.1, 50.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, atk_id, ((v - 0.1) / 49.9).clamp(0.0, 1.0)); } });
    let lc = l.clone(); kn(cx, "REL ms", norm(rel, 10.0, 500.0), 0.184, 10.0, 500.0, false, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, rel_id, ((v - 10.0) / 490.0).clamp(0.0, 1.0)); } });
    let lc = l.clone(); kn(cx, "GAIN dB", bip(gain, -6.0, 6.0), 0.5, -6.0, 6.0, true, move |_cx, g| { if let Gesture::Change(v) = g { set(&lc, gain_id, ((v + 6.0) / 12.0).clamp(0.0, 1.0)); } });
}).vertical_gap(Pixels(2.0)).alignment(Alignment::Center).width(Auto); }
