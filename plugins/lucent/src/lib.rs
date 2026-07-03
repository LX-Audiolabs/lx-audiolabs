use truce::prelude::*;
use truce_core::{custom_state::State as StateSerialize, state::StateLoadError, editor::Editor};
use truce_iced::IcedEditor;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::sync::atomic::Ordering;
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};

use shared_analysis::{SPECTRUM_BINS, SharedState, relay_hub, SnapFFT, SnapMode};

/// Resonance findings for one Lucent instance: `own` = peaks found in this
/// instance's own bus signal, `relay` = peaks found in the power-summed
/// spectrum of the Relay tracks it's listening to (group-level resonance
/// that can emerge from the sum even if no single track shows it).
#[derive(Default, Clone)]
pub struct ResonanceLists {
    pub own: Vec<(usize, f32)>,
    /// (bin, resonance score, contributor track names) — contributors are the
    /// Relay tracks whose own spectrum is above `CONTRIB_FLOOR_DB` at that bin,
    /// i.e. which tracks are actually feeding this group-level peak.
    pub relay: Vec<(usize, f32, Vec<String>)>,
}

/// Magnitude floor (dB) above which a Relay track counts as a contributor to
/// a group resonance peak. Same value as `MaskingAnalyzer`'s `FLOOR`, kept as
/// its own constant here since the two aren't the same computation.
const CONTRIB_FLOOR_DB: f32 = -70.0;

/// For each (bin, score) group-level peak, find which named Relay spectra
/// are actually above the floor at that bin.
fn attribute_contributors(
    peaks: &[(usize, f32)],
    relay_spectra: &[(String, Vec<f32>)],
) -> Vec<(usize, f32, Vec<String>)> {
    peaks.iter().map(|(bin, score)| {
        let contributors = relay_spectra.iter()
            .filter(|(_, spec)| spec.get(*bin).copied().unwrap_or(-90.0) > CONTRIB_FLOOR_DB)
            .map(|(name, _)| name.clone())
            .collect();
        (*bin, *score, contributors)
    }).collect()
}

/// Keyed by `Arc::as_ptr(&params)` — unique per plugin instance. A bare
/// `OnceLock<Vec<_>>` here would mean every Lucent instance in the process
/// overwrites the same global list (same failure mode as the Lucent-Relay
/// `RELAY_HANDLE` singleton bug).
type ResonanceRegistry = Arc<Mutex<HashMap<usize, ResonanceLists>>>;

fn resonance_registry() -> &'static ResonanceRegistry {
    static REG: OnceLock<ResonanceRegistry> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn publish_resonance(key: usize, lists: ResonanceLists) {
    if let Ok(mut m) = resonance_registry().lock() {
        m.insert(key, lists);
    }
}

pub fn read_resonance(key: usize) -> ResonanceLists {
    resonance_registry()
        .lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

pub fn remove_resonance(key: usize) {
    if let Ok(mut m) = resonance_registry().lock() {
        m.remove(&key);
    }
}

/// Same instance-keyed pattern as `ResonanceRegistry`, for the top masking
/// collisions (bin, dB, contributor track names) of each Lucent instance.
type MaskingRegistry = Arc<Mutex<HashMap<usize, Vec<(usize, f32, Vec<String>)>>>>;

fn masking_registry() -> &'static MaskingRegistry {
    static REG: OnceLock<MaskingRegistry> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn publish_masking(key: usize, peaks: Vec<(usize, f32, Vec<String>)>) {
    if let Ok(mut m) = masking_registry().lock() {
        m.insert(key, peaks);
    }
}

pub fn read_masking(key: usize) -> Vec<(usize, f32, Vec<String>)> {
    masking_registry()
        .lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

pub fn remove_masking(key: usize) {
    if let Ok(mut m) = masking_registry().lock() {
        m.remove(&key);
    }
}

/// Per-bin power-sum (linear domain) of multiple dB spectra, back to dB.
/// Models how the tracks actually combine on a bus — e.g. two -6dB signals
/// at the same frequency sum to ~-3dB, not -6dB (`min`/`max` would say -6dB
/// and miss the additive buildup).
fn power_sum_spectrum(spectra: &[Vec<f32>]) -> Vec<f32> {
    let mut out = vec![-90.0f32; SPECTRUM_BINS];
    if spectra.is_empty() {
        return out;
    }
    for j in 0..SPECTRUM_BINS {
        let sum_lin: f32 = spectra
            .iter()
            .map(|s| 10f32.powf(s.get(j).copied().unwrap_or(-90.0) / 10.0))
            .sum();
        out[j] = if sum_lin < 1e-9 { -90.0 } else { 10.0 * sum_lin.log10() };
    }
    out
}

mod editor;
mod ui;

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 550;

// ─── Masking analyzer ────────────────────────────────────────────────────────

struct MaskingAnalyzer {
    masking_map: Vec<f32>,
    scratch: Vec<f32>,
    /// Names of the two tracks whose pairwise collision produced `scratch[j]`
    /// / `masking_map[j]` (empty when no collision above floor at that bin).
    scratch_contributors: Vec<Vec<String>>,
    masking_contributors: Vec<Vec<String>>,
}

impl MaskingAnalyzer {
    fn new(_sample_rate: f32) -> Self {
        Self {
            masking_map: vec![-90.0; SPECTRUM_BINS],
            scratch: vec![-90.0; SPECTRUM_BINS],
            scratch_contributors: vec![Vec::new(); SPECTRUM_BINS],
            masking_contributors: vec![Vec::new(); SPECTRUM_BINS],
        }
    }

    /// `relay_named` pairs each Relay spectrum with its track name so a
    /// masking collision can be attributed to the two tracks that caused it.
    fn compute_masking(&mut self, own_spectrum: Option<&[f32]>, relay_named: &[(String, Vec<f32>)]) {
        const FLOOR: f32 = -70.0;
        let n = self.masking_map.len();

        for j in 0..n {
            let mut active: [(f32, &str); 17] = [(-90.0f32, ""); 17];
            let mut count = 0usize;

            if let Some(own_spec) = own_spectrum {
                let own = own_spec.get(j).copied().unwrap_or(-90.0);
                if own > FLOOR {
                    active[count] = (own, "Own");
                    count += 1;
                }
            }
            for (name, relay) in relay_named {
                if let Some(&v) = relay.get(j) {
                    if v > FLOOR && count < active.len() {
                        active[count] = (v, name.as_str());
                        count += 1;
                    }
                }
            }

            let mut best = -90.0f32;
            let mut best_pair = ("", "");
            for a in 0..count {
                for b in (a + 1)..count {
                    let collision = active[a].0.min(active[b].0);
                    if collision > best {
                        best = collision;
                        best_pair = (active[a].1, active[b].1);
                    }
                }
            }
            self.scratch[j] = best;
            self.scratch_contributors[j] = if best > -90.0 {
                vec![best_pair.0.to_string(), best_pair.1.to_string()]
            } else {
                Vec::new()
            };
        }

        for j in 0..n {
            let lo = j.saturating_sub(2);
            let hi = (j + 2).min(n - 1);
            let mut m = -90.0f32;
            let mut m_idx = j;
            for k in lo..=hi {
                if self.scratch[k] > m { m = self.scratch[k]; m_idx = k; }
            }
            self.masking_map[j] = m;
            self.masking_contributors[j] = self.scratch_contributors[m_idx].clone();
        }
    }

    /// Top `n` masking-collision bins (frequency + dB + the two contributing
    /// track names), sorted by severity. Mirrors the selection logic that
    /// used to live in `editor.rs::masking_summary`, moved here so the
    /// contributor names travel with the peak instead of being dropped.
    fn top_peaks(&self, n: usize) -> Vec<(usize, f32, Vec<String>)> {
        const FLOOR: f32 = -70.0;
        let mut peaks: Vec<(usize, f32, Vec<String>)> = self.masking_map.iter().enumerate()
            .filter(|&(_, &db)| db > FLOOR)
            .map(|(i, &db)| (i, db, self.masking_contributors[i].clone()))
            .collect();
        peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        peaks.truncate(n);
        peaks
    }
}

// ─── Peak tracker ─────────────────────────────────────────────────────────────

struct PeakTracker {
    persistence: Vec<u32>,
    last_prominence: Vec<f32>,
    resonance_score: Vec<f32>,
    prominence_threshold: f32,
}

impl PeakTracker {
    fn new() -> Self {
        Self {
            persistence: vec![0u32; SPECTRUM_BINS],
            last_prominence: vec![0.0; SPECTRUM_BINS],
            resonance_score: vec![0.0; SPECTRUM_BINS],
            prominence_threshold: 3.5,
        }
    }

    fn find_peaks(&self, spectrum: &[f32]) -> Vec<(usize, f32)> {
        let mut peaks = Vec::new();
        for k in 1..spectrum.len().saturating_sub(1) {
            let left = spectrum[k - 1];
            let center = spectrum[k];
            let right = spectrum[k + 1];
            if center > left && center > right {
                let prominence = center - ((left + right) / 2.0).max(-90.0);
                if prominence > self.prominence_threshold {
                    peaks.push((k, prominence));
                }
            }
        }
        peaks
    }

    fn update(&mut self, peaks: &[(usize, f32)]) {
        let peak_bins: Vec<usize> = peaks.iter().map(|(k, _)| *k).collect();
        for (k, prom) in peaks { self.last_prominence[*k] = *prom; }

        const PERSIST_CAP: u32 = 40;
        for k in 0..SPECTRUM_BINS {
            if peak_bins.contains(&k) {
                self.persistence[k] = (self.persistence[k] + 1).min(PERSIST_CAP);
            } else {
                self.persistence[k] = self.persistence[k].saturating_sub(1);
            }
            let target = self.last_prominence[k] * (self.persistence[k] as f32 / PERSIST_CAP as f32);
            let coef = if target > self.resonance_score[k] { 0.6 } else { 0.04 };
            self.resonance_score[k] = (self.resonance_score[k] * (1.0 - coef) + target * coef).max(0.0);
        }
    }

    fn resonance_peaks(&self) -> Vec<(usize, f32)> {
        let mut resonant = Vec::new();
        for k in 1..SPECTRUM_BINS.saturating_sub(1) {
            if self.resonance_score[k] > 2.0 && self.persistence[k] > 2 {
                resonant.push((k, self.resonance_score[k]));
            }
        }
        resonant.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        resonant.truncate(16);
        resonant
    }
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct LucentParams {
    #[param(name = "Analyze Mode", default = 0, range = "discrete(0, 2)")]
    pub analyze_mode: IntParam,
    #[skip]
    pub name: RwLock<String>,
    #[skip]
    pub shared: Arc<SharedState>,
}

// ─── Persistent state ────────────────────────────────────────────────────────

#[derive(State, Default, Clone)]
pub struct LucentState {
    pub name: String,
}

// ─── Plugin ───────────────────────────────────────────────────────────────────

pub struct Lucent {
    params: Arc<LucentParams>,
    fft_fwd: Arc<dyn RealToComplex<f32>>,
    fft_input: Vec<f32>,
    fft_write_pos: usize,
    fft_hann: Vec<f32>,
    fft_windowed: Vec<f32>,
    fft_output: Vec<Complex<f32>>,
    peak_tracker: PeakTracker,
    relay_peak_tracker: PeakTracker,
    masking_analyzer: MaskingAnalyzer,
    snap_fft: SnapFFT,
    sample_rate: f32,
    peak_hold_value: f32,
    peak_hold_l_value: f32,
    peak_hold_r_value: f32,
    claimed_lucent_slot: Option<u8>,
    cached_name: String,
    liveness: Option<Arc<std::sync::atomic::AtomicBool>>,
    instance_key: usize,
}

impl Lucent {
    pub fn new(params: Arc<LucentParams>) -> Self {
        let instance_key = Arc::as_ptr(&params) as usize;
        let fft_size = SPECTRUM_BINS * 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(fft_size);
        let fft_output = fft_fwd.make_output_vec();
        Self {
            params,
            fft_fwd,
            fft_input: vec![0.0; fft_size],
            fft_write_pos: 0,
            fft_hann: (0..fft_size)
                .map(|i| {
                    let n = fft_size;
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos())
                })
                .collect(),
            fft_windowed: vec![0.0; fft_size],
            fft_output,
            peak_tracker: PeakTracker::new(),
            relay_peak_tracker: PeakTracker::new(),
            masking_analyzer: MaskingAnalyzer::new(44100.0),
            snap_fft: SnapFFT::new(),
            sample_rate: 44100.0,
            peak_hold_value: -100.0,
            peak_hold_l_value: -100.0,
            peak_hold_r_value: -100.0,
            claimed_lucent_slot: None,
            cached_name: String::new(),
            liveness: None,
            instance_key,
        }
    }
}

#[inline]
fn gain_to_db(amp: f32) -> f32 {
    if amp < 1e-10 { -200.0 } else { 20.0 * amp.log10() }
}

impl PluginLogic for Lucent {
    fn reset(&mut self, sr: f64, _max: usize) {
        self.sample_rate = sr as f32;
        self.params.shared.sample_rate.store(sr as f32, Ordering::Release);

        if self.claimed_lucent_slot.is_none() {
            if let Some(hub) = relay_hub() {
                self.claimed_lucent_slot = hub.claim_consumer_slot(shared_analysis::shm::now_ms());
            }
        }
        self.params.shared.shm_slot.store(
            self.claimed_lucent_slot.map(|s| s as i32).unwrap_or(-1),
            Ordering::Release,
        );

        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.liveness = Some(alive.clone());
        let shared = self.params.shared.clone();
        let params = self.params.clone();
        std::thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let slot = shared.shm_slot.load(Ordering::Acquire);
                if slot < 0 { continue; }
                if let Some(hub) = relay_hub() {
                    let raw = params.name.try_read().map(|n| n.clone()).unwrap_or_default();
                    let name = shared_analysis::shm::display_name(&raw, slot as u8);
                    hub.write_consumer_name(slot as u8, &name, shared_analysis::shm::now_ms());
                }
            }
        });
    }

    fn process(
        &mut self,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        let fft_size = self.fft_input.len();
        let now_ms = shared_analysis::shm::now_ms();

        // Re-claim slot if lost
        if self.claimed_lucent_slot.is_none() {
            if let Some(hub) = relay_hub() {
                self.claimed_lucent_slot = hub.claim_consumer_slot(now_ms);
                self.params.shared.shm_slot.store(
                    self.claimed_lucent_slot.map(|s| s as i32).unwrap_or(-1),
                    Ordering::Release,
                );
            }
        }

        // Publish name heartbeat
        if let Ok(name) = self.params.name.try_read() {
            if *name != self.cached_name { self.cached_name = name.clone(); }
        }
        if let Some(slot) = self.claimed_lucent_slot {
            if let Some(hub) = relay_hub() {
                let name = shared_analysis::shm::display_name(&self.cached_name, slot);
                hub.write_consumer_name(slot, &name, now_ms);
            }
        }

        // Reset peak holds on request
        if self.params.shared.reset_peak.swap(false, Ordering::Release) {
            self.peak_hold_value = -100.0;
            self.peak_hold_l_value = -100.0;
            self.peak_hold_r_value = -100.0;
        }

        let mode = self.params.analyze_mode.value();
        let snap_phase = self.params.shared.snap_phase.load(Ordering::Acquire);

        // Pass-through: copy input to output
        for ch in 0..buffer.channels() {
            let (inp, out) = buffer.io(ch);
            out.copy_from_slice(inp);
        }

        // Analysis
        let n = buffer.num_samples();
        let sample_rate = self.sample_rate;
        let scope_len = shared_analysis::SCOPE_BUFFER_LEN;

        let mut max_out_l = 0.0f32;
        let mut max_out_r = 0.0f32;
        let mut sum_power_out_l = 0.0f32;
        let mut sum_power_out_r = 0.0f32;
        let mut sum_lr = 0.0f32;
        let mut sum_l2 = 0.0f32;
        let mut sum_r2 = 0.0f32;

        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            let in_l = buffer.input(0)[i];
            let in_r = buffer.input(1)[i];
            let mono_in = (in_l + in_r) * 0.5;

            // SNAP FFT (same pattern as Meridian/Equilibrium)
            if snap_phase > 0 {
                let sample = match snap_phase {
                    1 | 2 => mono_in,
                    3 => {
                        let in_mono = (in_l + in_r) * 0.5;
                        let out_mono = mono_in; // Lucent is pass-through, so out = in for SNAP
                        out_mono - in_mono // delta = 0 for pure analyzer
                    }
                    _ => 0.0,
                };
                if self.snap_fft.push_sample(sample) {
                    let frame = self.snap_fft.compute_fft(sample_rate);
                    let threshold = if snap_phase == 2 || snap_phase == 3 { 30 } else { 60 };
                    if self.snap_fft.accumulate_snap(&frame, snap_phase, threshold) {
                        let mode_snap = match snap_phase {
                            1 => SnapMode::Stereo, 2 => SnapMode::Mono, _ => SnapMode::Delta,
                        };
                        let snapshot = self.snap_fft.read_snapshot(mode_snap);
                        if let Ok(mut buf) = match mode_snap {
                            SnapMode::Stereo => self.params.shared.snap_stereo_snap.try_lock(),
                            SnapMode::Mono => self.params.shared.snap_mono_snap.try_lock(),
                            SnapMode::Delta => self.params.shared.snap_delta_snap.try_lock(),
                        } {
                            *buf = snapshot;
                        }
                        let next_phase = if snap_phase < 3 { snap_phase + 1 } else { 0 };
                        self.params.shared.snap_phase.store(next_phase, Ordering::Release);
                        if next_phase == 0 {
                            self.params.shared.snap_active.store(false, Ordering::Release);
                            self.snap_fft.reset_snapshots();
                        }
                    }
                }
            }

            max_out_l = max_out_l.max(in_l.abs());
            max_out_r = max_out_r.max(in_r.abs());
            sum_power_out_l += in_l * in_l;
            sum_power_out_r += in_r * in_r;
            sum_lr += in_l * in_r;
            sum_l2 += in_l * in_l;
            sum_r2 += in_r * in_r;

            let scope_pos = self.params.shared.scope_write_pos.load(Ordering::Relaxed);
            if let Ok(mut scope) = self.params.shared.scope_samples.try_lock() {
                if scope_pos < scope.len() {
                    scope[scope_pos] = [in_l, in_r];
                }
            }
            self.params.shared.scope_write_pos.store((scope_pos + 1) % scope_len, Ordering::Relaxed);

            self.fft_input[self.fft_write_pos] = mono_in;
            self.fft_write_pos += 1;

            if self.fft_write_pos >= fft_size {
                self.fft_write_pos = 0;

                for (d, (s, w)) in self.fft_windowed.iter_mut()
                    .zip(self.fft_input.iter().zip(self.fft_hann.iter()))
                {
                    *d = s * w;
                }

                if self.fft_fwd.process(&mut self.fft_windowed, &mut self.fft_output).is_ok() {
                    let n_bins = SPECTRUM_BINS;
                    let mut frame = [0.0f32; SPECTRUM_BINS];
                    shared_analysis::compute_spectrum_bins(&self.fft_output, &mut frame, fft_size, sample_rate);

                    match mode {
                        0 => {
                            let peaks = self.peak_tracker.find_peaks(&frame);
                            self.peak_tracker.update(&peaks);
                            let own_resonances = self.peak_tracker.resonance_peaks();
                            publish_resonance(self.instance_key, ResonanceLists { own: own_resonances, relay: Vec::new() });
                            publish_masking(self.instance_key, Vec::new());
                            if let Ok(mut mm) = self.params.shared.masking_map.try_lock() {
                                mm.iter_mut().for_each(|m| *m = -90.0);
                            }
                            if let Ok(mut bins) = self.params.shared.spectrum_bins.try_lock() {
                                bins.copy_from_slice(&frame);
                            }
                            if let Ok(mut avg) = self.params.shared.spectrum_avg.try_lock() {
                                // Energy-gating: only update EMA if signal above -80 dB
                                let frame_energy = frame.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                                let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                                let gate = energy_db > -80.0;
                                for k in 0..n_bins {
                                    let input = if gate { frame[k] } else { 0.0 };
                                    avg[k] = avg[k] * (49.0 / 50.0) + input * (1.0 / 50.0);
                                }
                            }
                        }
                        1 => {
                            let peaks = self.peak_tracker.find_peaks(&frame);
                            self.peak_tracker.update(&peaks);
                            let own_resonances = self.peak_tracker.resonance_peaks();

                            let my_name = self.claimed_lucent_slot
                                .map(|s| shared_analysis::shm::display_name(&self.cached_name, s))
                                .unwrap_or_else(|| self.cached_name.clone());
                            let relay_named: Vec<(String, Vec<f32>)> = relay_hub()
                                .map(|hub| hub.read_active(&my_name, now_ms))
                                .unwrap_or_default();
                            let relay_spectra: Vec<Vec<f32>> = relay_named.iter()
                                .map(|(_, spec)| spec.clone()).collect();

                            // Group-level resonance: power-sum of the Relay tracks can show
                            // a buildup that no single track (nor this bus's own signal) has.
                            let relay_sum = power_sum_spectrum(&relay_spectra);
                            let relay_peaks = self.relay_peak_tracker.find_peaks(&relay_sum);
                            self.relay_peak_tracker.update(&relay_peaks);
                            let relay_resonances = attribute_contributors(
                                &self.relay_peak_tracker.resonance_peaks(), &relay_named,
                            );

                            publish_resonance(self.instance_key, ResonanceLists { own: own_resonances, relay: relay_resonances });

                            self.masking_analyzer.compute_masking(Some(&frame), &relay_named);
                            publish_masking(self.instance_key, self.masking_analyzer.top_peaks(3));
                            if let Ok(mut mm) = self.params.shared.masking_map.try_lock() {
                                mm.copy_from_slice(&self.masking_analyzer.masking_map);
                            }
                            if let Ok(mut bins) = self.params.shared.spectrum_bins.try_lock() {
                                bins.copy_from_slice(&frame);
                            }
                            if let Ok(mut avg) = self.params.shared.spectrum_avg.try_lock() {
                                let frame_energy = frame.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                                let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                                let gate = energy_db > -80.0;
                                for k in 0..n_bins {
                                    let input = if gate { frame[k] } else { 0.0 };
                                    avg[k] = avg[k] * (49.0 / 50.0) + input * (1.0 / 50.0);
                                }
                            }
                        }
                        _ => {
                            if let Ok(mut bins) = self.params.shared.spectrum_bins.try_lock() {
                                bins.iter_mut().for_each(|b| *b = -90.0);
                            }
                            if let Ok(mut avg) = self.params.shared.spectrum_avg.try_lock() {
                                avg.iter_mut().for_each(|b| *b = -90.0);
                            }
                            let my_name = self.claimed_lucent_slot
                                .map(|s| shared_analysis::shm::display_name(&self.cached_name, s))
                                .unwrap_or_else(|| self.cached_name.clone());
                            let relay_named: Vec<(String, Vec<f32>)> = relay_hub()
                                .map(|hub| hub.read_active(&my_name, now_ms))
                                .unwrap_or_default();
                            let relay_spectra: Vec<Vec<f32>> = relay_named.iter()
                                .map(|(_, spec)| spec.clone()).collect();

                            // RELAY mode: no own signal, so resonance is purely the
                            // Relay tracks "untereinander und zusammen" — masking below
                            // covers "untereinander" (pairwise), this covers "zusammen".
                            let relay_sum = power_sum_spectrum(&relay_spectra);
                            let relay_peaks = self.relay_peak_tracker.find_peaks(&relay_sum);
                            self.relay_peak_tracker.update(&relay_peaks);
                            let relay_resonances = attribute_contributors(
                                &self.relay_peak_tracker.resonance_peaks(), &relay_named,
                            );
                            publish_resonance(self.instance_key, ResonanceLists { own: Vec::new(), relay: relay_resonances });

                            self.masking_analyzer.compute_masking(None, &relay_named);
                            publish_masking(self.instance_key, self.masking_analyzer.top_peaks(3));
                            if let Ok(mut mm) = self.params.shared.masking_map.try_lock() {
                                mm.copy_from_slice(&self.masking_analyzer.masking_map);
                            }
                        }
                    }
                }
            }
        }

        // Peak meters
        let peak_l_db = gain_to_db(max_out_l.max(1e-9));
        let peak_r_db = gain_to_db(max_out_r.max(1e-9));
        let peak_mono_db = peak_l_db.max(peak_r_db);
        self.params.shared.output_peak_l.store(peak_l_db, Ordering::Release);
        self.params.shared.output_peak_r.store(peak_r_db, Ordering::Release);
        self.params.shared.output_peak.store(peak_mono_db, Ordering::Release);
        if peak_l_db > self.peak_hold_l_value { self.peak_hold_l_value = peak_l_db; }
        if peak_r_db > self.peak_hold_r_value { self.peak_hold_r_value = peak_r_db; }
        if peak_mono_db > self.peak_hold_value { self.peak_hold_value = peak_mono_db; }
        self.params.shared.peak_hold_l.store(self.peak_hold_l_value, Ordering::Release);
        self.params.shared.peak_hold_r.store(self.peak_hold_r_value, Ordering::Release);
        self.params.shared.peak_hold.store(self.peak_hold_value, Ordering::Release);

        // Stereo balance + correlation
        if n > 0 {
            let sw = 1.0 / n as f32;
            let rms_l = (sum_power_out_l * sw).sqrt();
            let rms_r = (sum_power_out_r * sw).sqrt();
            let balance = if rms_l + rms_r > 1e-6 { (rms_l - rms_r) / (rms_l + rms_r) } else { 0.0 };
            self.params.shared.balance.store(balance, Ordering::Release);

            let corr = if sum_l2 > 1e-9 && sum_r2 > 1e-9 {
                sum_lr / (sum_l2.sqrt() * sum_r2.sqrt())
            } else {
                1.0
            };
            self.params.shared.phase_correlation.store(corr.clamp(-1.0, 1.0), Ordering::Release);
        }

        ProcessStatus::Normal
    }

    fn save_state(&self) -> Vec<u8> {
        let name = self.params.name.read().map(|n| n.clone()).unwrap_or_default();
        LucentState { name }.serialize()
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), StateLoadError> {
        match LucentState::deserialize(data) {
            Some(s) => {
                if let Ok(mut n) = self.params.name.write() { *n = s.name; }
                Ok(())
            }
            None => Err(StateLoadError::Malformed("LucentState")),
        }
    }

    fn state_changed(&mut self) {
        // Preset recall / undo / session load — sync cached name from restored params.
        if let Ok(n) = self.params.name.read() {
            self.cached_name = n.clone();
        }
    }

    fn editor(&self) -> Box<dyn Editor> {
        IcedEditor::<LucentParams, editor::LucentEditor>::new(
            self.params.clone(),
            (WINDOW_W, WINDOW_H),
        )
        .into_editor()
    }
}

impl Drop for Lucent {
    fn drop(&mut self) {
        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        self.params.shared.shm_slot.store(-1, Ordering::Release);
        if let Some(slot) = self.claimed_lucent_slot.take() {
            if let Some(hub) = relay_hub() {
                hub.release_consumer_slot(slot);
            }
        }
        remove_resonance(self.instance_key);
        remove_masking(self.instance_key);
    }
}

truce::plugin! {
    logic: Lucent,
    params: LucentParams,
}

#[cfg(test)]
mod tests {
    use crate::Plugin;
    use std::time::Duration;

    #[test]
    fn renders_pass_through() {
        use truce_test::{InputSource, assertions, driver};
        let result = driver!(Plugin)
            .duration(Duration::from_millis(50))
            .input(InputSource::Constant(0.5))
            .run();
        assertions::assert_no_nans(&result);
        assertions::assert_nonzero(&result);
    }

    #[test]
    fn state_round_trips() {
        truce_test::assert_state_round_trip::<Plugin>();
    }
}
