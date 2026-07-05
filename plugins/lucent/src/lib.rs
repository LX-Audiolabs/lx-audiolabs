use truce::prelude::*;
use truce_core::{custom_state::State as StateSerialize, state::StateLoadError, editor::Editor};
use truce_vizia::ViziaEditor;
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

/// Drops peaks that are almost certainly a musical overtone of a louder,
/// lower peak rather than an independent resonance — normal harmonic
/// spectral structure, not a problem. FFT bins are linear in Hz, so the
/// nth harmonic of a peak at bin `k0` falls near bin `n * k0` exactly; no
/// pitch/fundamental tracking needed (which would be unreliable on a full
/// mix bus anyway). Only suppresses when the candidate harmonic isn't
/// louder than the fundamental by more than a few dB — a peak riding well
/// above where a harmonic should sit is kept, since that's more likely a
/// real resonance than natural overtone rolloff.
fn suppress_harmonics(spectrum: &[f32], peaks: Vec<(usize, f32)>) -> Vec<(usize, f32)> {
    const MAX_HARMONIC: usize = 8;
    const BIN_TOLERANCE: usize = 2;
    const LOUDER_MARGIN_DB: f32 = 3.0;

    peaks.iter().copied().filter(|&(k, _)| {
        !peaks.iter().any(|&(k0, _)| {
            k0 < k
                && spectrum[k] <= spectrum[k0] + LOUDER_MARGIN_DB
                && (2..=MAX_HARMONIC).any(|n| (k0 * n).abs_diff(k) <= BIN_TOLERANCE)
        })
    }).collect()
}

mod editor;
mod ui;

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 550;

// ─── Sensitivity ────────────────────────────────────────────────

/// Derived from the `Sensitivity` knob (0.0 = strict/conservative, 1.0 =
/// sensitive). All six numbers below were the hand-tuned constants this
/// analyzer shipped with; they're now the sensitivity=0.5 midpoint of each
/// range so the knob's center detent reproduces the previous (already-tuned)
/// behavior exactly, and moving it scales — all at once — how loud, how
/// tonal, and how long a peak must be before it counts as a resonance or
/// masking collision. One knob, not two: Lucent only displays, it never
/// suggests or applies a cut, so there's no separate "how strong an action"
/// axis to control.
struct SensitivityThresholds {
    contrast_min_db: f32,
    flatness_max: f32,
    floor_db: f32,
    score_min: f32,
    persistence_min: u32,
    masking_floor_db: f32,
    /// Minimum Q (center freq / -3dB bandwidth) for a peak to count as
    /// narrowband. Rejects broad humps (formants, EQ buckets, room-mode
    /// clusters) that pass contrast+flatness but aren't a sharp resonance.
    min_q: f32,
}

fn sensitivity_thresholds(sensitivity: f32) -> SensitivityThresholds {
    let d = sensitivity.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| a + (b - a) * d;
    SensitivityThresholds {
        contrast_min_db: lerp(8.0, 3.0),
        flatness_max: lerp(0.5, 0.85),
        floor_db: lerp(-65.0, -85.0),
        score_min: lerp(4.0, 1.0),
        persistence_min: lerp(20.0, 4.0) as u32,
        masking_floor_db: lerp(-55.0, -85.0),
        min_q: lerp(6.0, 2.0),
    }
}

// ─── Masking analyzer ────────────────────────────────────────────────────────

struct MaskingAnalyzer {
    /// Persistence-gated collision level per bin — what everything outside
    /// this struct reads (FFT overlay bars, `top_peaks`).
    masking_map: Vec<f32>,
    /// This frame's ERB-smoothed collision level, before the persistence
    /// gate. Kept separate so a single loud-but-brief collision doesn't
    /// immediately count as "masking" (see `persistence`).
    raw: Vec<f32>,
    persistence: Vec<u32>,
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
            raw: vec![-90.0; SPECTRUM_BINS],
            persistence: vec![0u32; SPECTRUM_BINS],
            scratch: vec![-90.0; SPECTRUM_BINS],
            scratch_contributors: vec![Vec::new(); SPECTRUM_BINS],
            masking_contributors: vec![Vec::new(); SPECTRUM_BINS],
        }
    }

    /// `relay_named` pairs each Relay spectrum with its track name so a
    /// masking collision can be attributed to the two tracks that caused it.
    /// `persistence_min` is the Sensitivity knob's shared persistence gate
    /// (same field resonance uses) — a collision only counts once it holds
    /// for that many frames, not on a single-frame blip.
    fn compute_masking(
        &mut self,
        own_spectrum: Option<&[f32]>,
        relay_named: &[(String, Vec<f32>)],
        floor_db: f32,
        sample_rate: f32,
        persistence_min: u32,
    ) {
        let n = self.masking_map.len();

        for j in 0..n {
            let mut active: [(f32, &str); 17] = [(-90.0f32, ""); 17];
            let mut count = 0usize;

            if let Some(own_spec) = own_spectrum {
                let own = own_spec.get(j).copied().unwrap_or(-90.0);
                if own > floor_db {
                    active[count] = (own, "Own");
                    count += 1;
                }
            }
            for (name, relay) in relay_named {
                if let Some(&v) = relay.get(j)
                    && v > floor_db && count < active.len() {
                        active[count] = (v, name.as_str());
                        count += 1;
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

        // Smooth over the ERB (critical-band) width around each bin instead
        // of a fixed ±2 bins: FFT bins are linear in Hz but the ear's
        // critical bandwidth grows with frequency (~35Hz at 100Hz, ~1100Hz
        // at 10kHz per Glasberg & Moore), so a fixed bin window is roughly
        // right at low frequencies but far too narrow at high ones — it was
        // comparing frequencies as if they were in separate perceptual
        // bands when the ear would blend them together.
        let bin_hz = sample_rate / (n as f32 * 2.0);
        for j in 0..n {
            let freq = j as f32 * bin_hz;
            let erb_hz = 24.7 * (4.37 * freq / 1000.0 + 1.0);
            let half_window = ((erb_hz / 2.0 / bin_hz).round() as usize).clamp(2, 40);
            let lo = j.saturating_sub(half_window);
            let hi = (j + half_window).min(n - 1);
            let mut m = -90.0f32;
            let mut m_idx = j;
            for k in lo..=hi {
                if self.scratch[k] > m { m = self.scratch[k]; m_idx = k; }
            }
            self.raw[j] = m;
            self.masking_contributors[j] = self.scratch_contributors[m_idx].clone();
        }

        const PERSIST_CAP: u32 = 40;
        for j in 0..n {
            if self.raw[j] > floor_db {
                self.persistence[j] = (self.persistence[j] + 1).min(PERSIST_CAP);
            } else {
                self.persistence[j] = self.persistence[j].saturating_sub(1);
            }
            self.masking_map[j] = if self.persistence[j] > persistence_min { self.raw[j] } else { -90.0 };
        }
    }

    /// Top `n` masking-collision bins (frequency + dB + the two contributing
    /// track names), sorted by severity. Mirrors the selection logic that
    /// used to live in `editor.rs::masking_summary`, moved here so the
    /// contributor names travel with the peak instead of being dropped.
    fn top_peaks(&self, n: usize, floor_db: f32) -> Vec<(usize, f32, Vec<String>)> {
        let mut peaks: Vec<(usize, f32, Vec<String>)> = self.masking_map.iter().enumerate()
            .filter(|&(_, &db)| db > floor_db)
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
}

impl PeakTracker {
    fn new() -> Self {
        Self {
            persistence: vec![0u32; SPECTRUM_BINS],
            last_prominence: vec![0.0; SPECTRUM_BINS],
            resonance_score: vec![0.0; SPECTRUM_BINS],
        }
    }

    /// Local maximum + four gates, each rejecting a distinct false-positive
    /// mode the raw 2-neighbor prominence check let through:
    /// - floor: rejects peaks sitting in the noise floor (no real signal there)
    /// - contrast: prominence against a wide local baseline (±8 bins) instead
    ///   of just the immediate 2 neighbors, less sensitive to single-bin ripple
    /// - flatness: rejects broadband/noisy content (cymbals, hats) that has
    ///   lots of small local maxima but isn't a narrowband resonance — the
    ///   main fix for the high-frequency false-positive bias, since bright
    ///   material triggers many raw local maxima that a flat dB threshold
    ///   alone can't tell apart from an actual tonal peak.
    /// - Q (bandwidth): rejects broad humps (formants, EQ buckets, room-mode
    ///   clusters) — contrast+flatness alone can't tell a wide bump from a
    ///   sharp resonance, only the -3dB bandwidth can.
    fn find_peaks(&self, spectrum: &[f32], t: &SensitivityThresholds, sample_rate: f32) -> Vec<(usize, f32)> {
        const BASELINE_WINDOW: usize = 8;
        const FLATNESS_WINDOW: usize = 4;
        const MAX_BW_SEARCH: usize = 24;

        let n = spectrum.len();
        let bin_hz = sample_rate / (n as f32 * 2.0);
        let mut peaks = Vec::new();
        for k in 1..n.saturating_sub(1) {
            let left = spectrum[k - 1];
            let center = spectrum[k];
            let right = spectrum[k + 1];
            if !(center > left && center > right) {
                continue;
            }
            if center < t.floor_db {
                continue;
            }

            let lo = k.saturating_sub(BASELINE_WINDOW);
            let hi = (k + BASELINE_WINDOW).min(n - 1);
            let baseline = spectrum[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
            let contrast = center - baseline;
            if contrast < t.contrast_min_db {
                continue;
            }

            let flo = k.saturating_sub(FLATNESS_WINDOW);
            let fhi = (k + FLATNESS_WINDOW).min(n - 1);
            let window = &spectrum[flo..=fhi];
            let power_sum: f32 = window.iter().map(|&db| 10f32.powf(db / 10.0)).sum();
            let log_sum: f32 = window.iter().map(|&db| 10f32.powf(db / 10.0).max(1e-12).ln()).sum();
            let count = window.len() as f32;
            let arith_mean = power_sum / count;
            let geo_mean = (log_sum / count).exp();
            let flatness = if arith_mean > 1e-12 { geo_mean / arith_mean } else { 1.0 };
            if flatness > t.flatness_max {
                continue;
            }

            let bw_lo_bound = k.saturating_sub(MAX_BW_SEARCH);
            let mut lo_edge = k;
            while lo_edge > bw_lo_bound && spectrum[lo_edge - 1] > center - 3.0 {
                lo_edge -= 1;
            }
            let bw_hi_bound = (k + MAX_BW_SEARCH).min(n - 1);
            let mut hi_edge = k;
            while hi_edge < bw_hi_bound && spectrum[hi_edge + 1] > center - 3.0 {
                hi_edge += 1;
            }
            let bandwidth_hz = (hi_edge - lo_edge).max(1) as f32 * bin_hz;
            let q = (k as f32 * bin_hz) / bandwidth_hz;
            if q < t.min_q {
                continue;
            }

            peaks.push((k, contrast));
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

    fn resonance_peaks(&self, t: &SensitivityThresholds) -> Vec<(usize, f32)> {
        let mut resonant = Vec::new();
        for k in 1..SPECTRUM_BINS.saturating_sub(1) {
            if self.resonance_score[k] > t.score_min && self.persistence[k] > t.persistence_min {
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
    #[param(name = "Analyze Mode", default = 0, range = "discrete(0, 2)", group = "Lucent")]
    pub analyze_mode: IntParam,
    #[param(name = "Resonance", default = 1, group = "Lucent")]
    pub resonance_active: BoolParam,
    #[param(name = "Masking", default = 1, group = "Lucent")]
    pub masking_active: BoolParam,
    /// How deep the resonance/masking detectors dig: 0% = shallow (only
    /// strong, sustained findings), 100% = deep (surfaces weaker, shorter
    /// ones too). 50% reproduces the previously hand-tuned thresholds.
    #[param(name = "Sensitivity", default = 50.0, range = "linear(0.0, 100.0)", unit = "%", smooth = "linear(20)", group = "Lucent")]
    pub sensitivity: FloatParam,
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
    /// Envelope follower driving the goniometer's visual auto-gain — same
    /// pattern as Equilibrium/Meridian, so all three plugins' vectorscopes
    /// fill the same visual range regardless of the signal's actual level
    /// instead of Lucent's showing a tiny raw-amplitude dot cluster.
    scope_vis_envelope: f32,
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
            scope_vis_envelope: 1e-4,
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

        if self.claimed_lucent_slot.is_none()
            && let Some(hub) = relay_hub() {
                self.claimed_lucent_slot = hub.claim_consumer_slot(shared_analysis::shm::now_ms());
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
        if self.claimed_lucent_slot.is_none()
            && let Some(hub) = relay_hub() {
                self.claimed_lucent_slot = hub.claim_consumer_slot(now_ms);
                self.params.shared.shm_slot.store(
                    self.claimed_lucent_slot.map(|s| s as i32).unwrap_or(-1),
                    Ordering::Release,
                );
            }

        // Publish name heartbeat
        if let Ok(name) = self.params.name.try_read()
            && *name != self.cached_name { self.cached_name = name.clone(); }
        if let Some(slot) = self.claimed_lucent_slot
            && let Some(hub) = relay_hub() {
                let name = shared_analysis::shm::display_name(&self.cached_name, slot);
                hub.write_consumer_name(slot, &name, now_ms);
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
                    let sensitivity = sensitivity_thresholds(self.params.sensitivity.raw_target() as f32 / 100.0);

                    match mode {
                        0 => {
                            let peaks = self.peak_tracker.find_peaks(&frame, &sensitivity, sample_rate);
                            let peaks = suppress_harmonics(&frame, peaks);
                            self.peak_tracker.update(&peaks);
                            let own_resonances = self.peak_tracker.resonance_peaks(&sensitivity);
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
                            let peaks = self.peak_tracker.find_peaks(&frame, &sensitivity, sample_rate);
                            let peaks = suppress_harmonics(&frame, peaks);
                            self.peak_tracker.update(&peaks);
                            let own_resonances = self.peak_tracker.resonance_peaks(&sensitivity);

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
                            let relay_peaks = self.relay_peak_tracker.find_peaks(&relay_sum, &sensitivity, sample_rate);
                            let relay_peaks = suppress_harmonics(&relay_sum, relay_peaks);
                            self.relay_peak_tracker.update(&relay_peaks);
                            let relay_resonances = attribute_contributors(
                                &self.relay_peak_tracker.resonance_peaks(&sensitivity), &relay_named,
                            );

                            publish_resonance(self.instance_key, ResonanceLists { own: own_resonances, relay: relay_resonances });

                            self.masking_analyzer.compute_masking(Some(&frame), &relay_named, sensitivity.masking_floor_db, sample_rate, sensitivity.persistence_min);
                            publish_masking(self.instance_key, self.masking_analyzer.top_peaks(3, sensitivity.masking_floor_db));
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
                            let relay_peaks = self.relay_peak_tracker.find_peaks(&relay_sum, &sensitivity, sample_rate);
                            let relay_peaks = suppress_harmonics(&relay_sum, relay_peaks);
                            self.relay_peak_tracker.update(&relay_peaks);
                            let relay_resonances = attribute_contributors(
                                &self.relay_peak_tracker.resonance_peaks(&sensitivity), &relay_named,
                            );
                            publish_resonance(self.instance_key, ResonanceLists { own: Vec::new(), relay: relay_resonances });

                            self.masking_analyzer.compute_masking(None, &relay_named, sensitivity.masking_floor_db, sample_rate, sensitivity.persistence_min);
                            publish_masking(self.instance_key, self.masking_analyzer.top_peaks(3, sensitivity.masking_floor_db));
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

        // Goniometer scope buffer — visual auto-gain envelope, same pattern
        // as Equilibrium/Meridian (5ms attack / 300ms release envelope
        // scaling samples to ~90% of the display), so Lucent's vectorscope
        // fills the same visual range as theirs instead of showing a tiny
        // raw-amplitude dot cluster at typical (well below full-scale) mix levels.
        {
            let start_pos = self.params.shared.scope_write_pos.load(Ordering::Acquire);
            if let Ok(mut scope) = self.params.shared.scope_samples.try_lock() {
                let buf_len = scope_len;
                let in0 = buffer.input(0);
                let in1 = buffer.input(1);
                let block_peak = (0..n)
                    .map(|i| in0[i].abs().max(in1[i].abs()))
                    .fold(0.0f32, f32::max)
                    .max(1e-9);
                let att = 1.0 - (-(n as f32) / (0.005 * sample_rate)).exp();
                let rel = 1.0 - (-(n as f32) / (0.300 * sample_rate)).exp();
                if block_peak > self.scope_vis_envelope {
                    self.scope_vis_envelope += att * (block_peak - self.scope_vis_envelope);
                } else {
                    self.scope_vis_envelope += rel * (block_peak - self.scope_vis_envelope);
                }
                let vis_gain = if self.scope_vis_envelope > 1e-5 {
                    (0.9 / self.scope_vis_envelope).min(20.0)
                } else {
                    0.0
                };
                for i in 0..n {
                    let pos = (start_pos + i) % buf_len;
                    scope[pos] = [in0[i] * vis_gain, in1[i] * vis_gain];
                }
                self.params.shared.scope_write_pos.store((start_pos + n) % buf_len, Ordering::Release);
            }
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
        // Vizia pilot (CLAP-vault features/2026-07-04-truce-2.0-upgrade-plan.md).
        // `shared` is captured directly into the setup closure rather than
        // read through `ParamLens` - the goniometer/spectrum/meter data
        // lives in `LucentParams::shared` (atomics + mutexes written by
        // `process()`), not in the param store `ParamLens` binds to.
        let shared = self.params.shared.clone();
        let params = self.params.clone();
        ViziaEditor::<LucentParams>::new(
            self.params.clone(),
            (WINDOW_W, WINDOW_H),
            move |cx, lens| editor::build(cx, lens, shared.clone(), params.clone()),
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
        if let Some(slot) = self.claimed_lucent_slot.take()
            && let Some(hub) = relay_hub() {
                hub.release_consumer_slot(slot);
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
