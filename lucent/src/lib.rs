use nice_plug::prelude::*;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::sync::atomic::Ordering;
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};
use shared_analysis::{SPECTRUM_BINS, SharedState, relay_hub};

type ResonanceHub = Arc<Mutex<Vec<(usize, f32)>>>;

pub fn resonance_hub() -> &'static ResonanceHub {
    static HUB: OnceLock<ResonanceHub> = OnceLock::new();
    HUB.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

mod editor;
mod ui;

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct MaskingAnalyzer {
    /// Collision level (dB) per bin — where own signal fights competing relay energy.
    masking_map: Vec<f32>,
    /// Pre-allocated scratch to avoid per-frame allocation on the audio thread.
    scratch: Vec<f32>,
}

impl MaskingAnalyzer {
    fn new(_sample_rate: f32) -> Self {
        Self {
            masking_map: vec![-90.0; SPECTRUM_BINS],
            scratch: vec![-90.0; SPECTRUM_BINS],
        }
    }

    /// Collision map: per bin, the loudest overlap between any two active
    /// sources. When `own_spectrum` is `Some`, the own signal participates in
    /// the pairwise comparison (Hybrid mode — "does a relay mask my track?").
    /// When `None`, only relays are compared against each other (Relay mode —
    /// "does relay A mask relay B?"). Standalone mode skips this call entirely.
    /// Each pair whose signals both exceed the display floor contributes
    /// `min(a,b)` — the quieter signal defines the masking severity. The bin
    /// gets the max collision across all pairs. Light spreading follows so the
    /// overlay reads as bands, not isolated pixels.
    fn compute_masking(&mut self, own_spectrum: Option<&[f32]>, relay_spectra: &[Vec<f32>]) {
        const FLOOR: f32 = -70.0;
        let n = self.masking_map.len();

        // Gather all sources that are above floor at each bin.
        for j in 0..n {
            let mut active: [f32; 17] = [-90.0f32; 17]; // own + up to 16 relays
            let mut count = 0usize;

            if let Some(own_spec) = own_spectrum {
                let own = own_spec.get(j).copied().unwrap_or(-90.0);
                if own > FLOOR {
                    active[count] = own;
                    count += 1;
                }
            }
            for relay in relay_spectra {
                if let Some(&v) = relay.get(j) {
                    if v > FLOOR {
                        if count < active.len() {
                            active[count] = v;
                            count += 1;
                        }
                    }
                }
            }

            // Pairwise: loudest collision among any two active sources.
            let mut best = -90.0f32;
            for a in 0..count {
                for b in (a + 1)..count {
                    let collision = active[a].min(active[b]);
                    if collision > best {
                        best = collision;
                    }
                }
            }
            self.scratch[j] = best;
        }

        // Light spreading: 2-bin max dilation (masking bleeds to neighbours).
        for j in 0..n {
            let lo = j.saturating_sub(2);
            let hi = (j + 2).min(n - 1);
            let mut m = -90.0f32;
            for k in lo..=hi {
                if self.scratch[k] > m {
                    m = self.scratch[k];
                }
            }
            self.masking_map[j] = m;
        }
    }
}

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
            prominence_threshold: 6.0,
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

        for (k, prom) in peaks {
            self.last_prominence[*k] = *prom;
        }

        // Persistence counts how reliably a bin recurs (the "by frequency" weight),
        // capped so the score stays bounded. The displayed resonance_score then follows
        // a fast-attack / slow-release envelope — mirroring Meridian's slow low-band
        // spectrum EMA (α≈0.02–0.05): recurring resonances climb and linger ~1 s so real
        // problem frequencies stay visible, while transient one-off peaks fade quickly.
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

pub struct Lucent {
    params: Arc<LucentParams>,
    shared_state: Arc<SharedState>,

    /// Cached FFT plan + scratch buffers (allocated once, never on the audio thread).
    fft_fwd: Arc<dyn RealToComplex<f32>>,
    fft_input: Vec<f32>,
    fft_write_pos: usize,
    fft_hann: Vec<f32>,
    fft_windowed: Vec<f32>,
    fft_output: Vec<Complex<f32>>,
    peak_tracker: PeakTracker,
    masking_analyzer: MaskingAnalyzer,

    // Peak-hold (dB) state
    peak_hold_value: f32,
    peak_hold_l_value: f32,
    peak_hold_r_value: f32,

    /// Shared-hub slot advertising this instance's name to relays (claimed on
    /// init, freed on Drop). Last name published, cached to skip redundant writes.
    claimed_lucent_slot: Option<u8>,
    cached_name: String,
    /// Liveness thread "alive" flag. The thread advertises this instance's name to
    /// SHM every 100 ms while the plugin is *activated* — so a relay sees it even
    /// when transport is stopped. Tied to activate/deactivate: spawned in
    /// `initialize()`, stopped in `deactivate()` (host disables the plugin → it
    /// vanishes) and `Drop`. Audio (process) advertises too while playing.
    liveness: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Params)]
pub struct LucentParams {
    #[persist = "lucent-window-state"]
    pub editor_state: Arc<nice_plug_iced::WindowState>,

    /// Operating mode: 0=Standalone (own audio, no relays), 1=Hybrid (own+relays),
    /// 2=Relay (only relay feeds, no own FFT).
    #[id = "analyze_mode"]  pub analyze_mode: IntParam,

    /// This Lucent instance's display name (e.g. the group-track name). Persisted,
    /// published to the shared hub so Lucent-Relays can target it by name, and used
    /// to filter which relay feeds this instance accepts.
    #[persist = "lucent-name"]
    pub name: RwLock<String>,
}

impl Default for LucentParams {
    fn default() -> Self {
        Self {
            editor_state: nice_plug_iced::WindowState::from_logical_size(990, 550),
            analyze_mode: IntParam::new("Analyze Mode", 0, IntRange::Linear { min: 0, max: 2 }),
            name: RwLock::new(String::new()),
        }
    }
}

impl Default for Lucent {
    fn default() -> Self {
        let fft_size = SPECTRUM_BINS * 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft_fwd_plan = planner.plan_fft_forward(fft_size);
        let fft_output_buf = fft_fwd_plan.make_output_vec();
        Self {
            params: Arc::new(LucentParams::default()),
            shared_state: Arc::new(SharedState::default()),
            fft_fwd: fft_fwd_plan,
            fft_input: vec![0.0; fft_size],
            fft_write_pos: 0,
            fft_hann: (0..fft_size)
                .map(|i| {
                    let n = fft_size;
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos())
                })
                .collect(),
            fft_windowed: vec![0.0; fft_size],
            fft_output: fft_output_buf,
            peak_tracker: PeakTracker::new(),
            masking_analyzer: MaskingAnalyzer::new(44100.0),

            peak_hold_value: -100.0,
            peak_hold_l_value: -100.0,
            peak_hold_r_value: -100.0,

            claimed_lucent_slot: None,
            cached_name: String::new(),
            liveness: None,
        }
    }
}

impl Plugin for Lucent {
    const NAME: &'static str = "Lucent";
    const VENDOR: &'static str = "LX Audiolabs";
    const URL: &'static str = "https://github.com/lxndrbe/clap-development";
    const EMAIL: &'static str = "contact@lxaudiolabs.com";
    const VERSION: &'static str = VERSION;

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: std::num::NonZeroU32::new(2),
        main_output_channels: std::num::NonZeroU32::new(2),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames::const_default(),
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared_state.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        _buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        // Claim a slot in the Lucent-name registry so relays can target this
        // instance. Keep any slot already held across re-initialization.
        if self.claimed_lucent_slot.is_none() {
            if let Some(hub) = relay_hub() {
                self.claimed_lucent_slot = hub.claim_lucent_slot(shared_analysis::shm::now_ms());
            }
        }
        // Publish the slot so the editor knows its own effective name (fallback
        // "Lucent N" when unnamed) for filtering relay feeds.
        self.shared_state.shm_slot.store(
            self.claimed_lucent_slot.map(|s| s as i32).unwrap_or(-1),
            Ordering::Release,
        );

        // (Re)spawn the liveness thread. It advertises the name every 100 ms while
        // the plugin is activated — so the relay sees this Lucent even with the
        // transport stopped. Stopped in deactivate()/Drop, so disabling the plugin
        // makes it vanish (audio-only heartbeat would also vanish on transport stop).
        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.liveness = Some(alive.clone());
        let ss = self.shared_state.clone();
        let params = self.params.clone();
        std::thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let slot = ss.shm_slot.load(Ordering::Acquire);
                if slot < 0 {
                    continue;
                }
                if let Some(hub) = relay_hub() {
                    let raw = params.name.try_read().map(|n| n.clone()).unwrap_or_default();
                    let name = shared_analysis::shm::lucent_display_name(&raw, slot as u8);
                    hub.write_lucent_name(slot as u8, &name, shared_analysis::shm::now_ms());
                }
            }
        });
        true
    }

    fn deactivate(&mut self) {
        // Host disabled the plugin → stop advertising and free the slot so the
        // relay drops this Lucent right away.
        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        self.shared_state.shm_slot.store(-1, Ordering::Release);
        if let Some(slot) = self.claimed_lucent_slot.take() {
            if let Some(hub) = relay_hub() {
                hub.release_lucent_slot(slot);
            }
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let sample_rate = context.transport().sample_rate;
        self.shared_state.sample_rate.store(sample_rate, Ordering::Release);
        let fft_size = self.fft_input.len();
        let now_ms = shared_analysis::shm::now_ms();

        // Publish this instance's name + heartbeat to the Lucent registry so
        // relays can target it. Name read non-blocking (cached on contention);
        // retry the slot claim if none was free at init.
        if self.claimed_lucent_slot.is_none() {
            if let Some(hub) = relay_hub() {
                self.claimed_lucent_slot = hub.claim_lucent_slot(now_ms);
                self.shared_state.shm_slot.store(
                    self.claimed_lucent_slot.map(|s| s as i32).unwrap_or(-1),
                    Ordering::Release,
                );
            }
        }
        if let Ok(name) = self.params.name.try_read() {
            if *name != self.cached_name {
                self.cached_name = name.clone();
            }
        }
        if let Some(slot) = self.claimed_lucent_slot {
            if let Some(hub) = relay_hub() {
                let name = shared_analysis::shm::lucent_display_name(&self.cached_name, slot);
                hub.write_lucent_name(slot, &name, now_ms);
            }
        }

        // Reset peak holds on request
        if self.shared_state.reset_peak.swap(false, Ordering::Release) {
            self.peak_hold_value = -100.0;
            self.peak_hold_l_value = -100.0;
            self.peak_hold_r_value = -100.0;
        }

        let mode = self.params.analyze_mode.value();

        let channels = buffer.as_slice();
        let n = channels[0].len();

        let mut max_out_l = 0.0f32;
        let mut max_out_r = 0.0f32;
        let mut sum_power_out_l = 0.0f32;
        let mut sum_power_out_r = 0.0f32;
        let mut sum_lr = 0.0f32;
        let mut sum_l2 = 0.0f32;
        let mut sum_r2 = 0.0f32;

        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            let in_l = channels[0][i];
            let in_r = channels[1][i];
            let mono_in = (in_l + in_r) * 0.5;

            // Pure pass-through: Lucent is an analyzer, not a processor.
            // Audio flows through unmodified.

            max_out_l = max_out_l.max(in_l.abs());
            max_out_r = max_out_r.max(in_r.abs());
            sum_power_out_l += in_l * in_l;
            sum_power_out_r += in_r * in_r;
            sum_lr += in_l * in_r;
            sum_l2 += in_l * in_l;
            sum_r2 += in_r * in_r;

            // Scope ring buffer for goniometer (post-processing = same as input)
            let scope_len = shared_analysis::SCOPE_BUFFER_LEN;
            let scope_pos = self.shared_state.scope_write_pos.load(Ordering::Relaxed);
            if let Ok(mut scope) = self.shared_state.scope_samples.lock() {
                if scope_pos < scope.len() {
                    scope[scope_pos] = [in_l, in_r];
                }
            }
            self.shared_state.scope_write_pos.store((scope_pos + 1) % scope_len, Ordering::Relaxed);

            // FFT analysis on the INPUT mono
            self.fft_input[self.fft_write_pos] = mono_in;
            self.fft_write_pos += 1;

            if self.fft_write_pos >= fft_size {
                self.fft_write_pos = 0;

                // Window into pre-allocated scratch (no per-frame allocation).
                for (d, (s, w)) in self
                    .fft_windowed
                    .iter_mut()
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
                            // STANDALONE: own resonances + spectrum, no relays, no masking.
                            let peaks = self.peak_tracker.find_peaks(&frame);
                            self.peak_tracker.update(&peaks);
                            let last_resonances = self.peak_tracker.resonance_peaks();
                            if let Ok(mut peaks) = resonance_hub().lock() {
                                *peaks = last_resonances.clone();
                            }
                            if let Ok(mut mm) = self.shared_state.masking_map.try_lock() {
                                mm.iter_mut().for_each(|m| *m = -90.0);
                            }
                            if let Ok(mut bins) = self.shared_state.spectrum_bins.lock() {
                                bins.copy_from_slice(&frame);
                            }
                            if let Ok(mut avg) = self.shared_state.spectrum_avg.lock() {
                                for k in 0..n_bins {
                                    avg[k] = avg[k] * (49.0 / 50.0) + frame[k] * (1.0 / 50.0);
                                }
                            }
                        }
                        1 => {
                            // HYBRID: own resonances + spectrum + relay feeds + own-vs-relays masking.
                            let peaks = self.peak_tracker.find_peaks(&frame);
                            self.peak_tracker.update(&peaks);
                            let last_resonances = self.peak_tracker.resonance_peaks();
                            if let Ok(mut peaks) = resonance_hub().lock() {
                                *peaks = last_resonances.clone();
                            }
                            let my_name = self.claimed_lucent_slot
                                .map(|s| shared_analysis::shm::lucent_display_name(&self.cached_name, s))
                                .unwrap_or_else(|| self.cached_name.clone());
                            let relay_spectra: Vec<Vec<f32>> = relay_hub()
                                .map(|hub| {
                                    hub.read_active(&my_name, now_ms)
                                        .into_iter()
                                        .map(|(_, spec)| spec)
                                        .collect()
                                })
                                .unwrap_or_default();
                            self.masking_analyzer.compute_masking(Some(&frame), &relay_spectra);
                            if let Ok(mut mm) = self.shared_state.masking_map.try_lock() {
                                mm.copy_from_slice(&self.masking_analyzer.masking_map);
                            }
                            if let Ok(mut bins) = self.shared_state.spectrum_bins.lock() {
                                bins.copy_from_slice(&frame);
                            }
                            if let Ok(mut avg) = self.shared_state.spectrum_avg.lock() {
                                for k in 0..n_bins {
                                    avg[k] = avg[k] * (49.0 / 50.0) + frame[k] * (1.0 / 50.0);
                                }
                            }
                        }
                        _ => {
                            // RELAY-ONLY (2): suppress own spectrum + resonance, relay-vs-relay masking.
                            if let Ok(mut bins) = self.shared_state.spectrum_bins.lock() {
                                bins.iter_mut().for_each(|b| *b = -90.0);
                            }
                            if let Ok(mut avg) = self.shared_state.spectrum_avg.lock() {
                                avg.iter_mut().for_each(|b| *b = -90.0);
                            }
                            if let Ok(mut peaks) = resonance_hub().lock() {
                                peaks.clear();
                            }
                            let my_name = self.claimed_lucent_slot
                                .map(|s| shared_analysis::shm::lucent_display_name(&self.cached_name, s))
                                .unwrap_or_else(|| self.cached_name.clone());
                            let relay_spectra: Vec<Vec<f32>> = relay_hub()
                                .map(|hub| {
                                    hub.read_active(&my_name, now_ms)
                                        .into_iter()
                                        .map(|(_, spec)| spec)
                                        .collect()
                                })
                                .unwrap_or_default();
                            self.masking_analyzer.compute_masking(None, &relay_spectra);
                            if let Ok(mut mm) = self.shared_state.masking_map.try_lock() {
                                mm.copy_from_slice(&self.masking_analyzer.masking_map);
                            }
                        }
                    }
                }
            }
        }

        // --- Peak meters (dB) ---
        let peak_l_db = util::gain_to_db(max_out_l.max(1e-9));
        let peak_r_db = util::gain_to_db(max_out_r.max(1e-9));
        let peak_mono_db = peak_l_db.max(peak_r_db);
        self.shared_state.output_peak_l.store(peak_l_db, Ordering::Release);
        self.shared_state.output_peak_r.store(peak_r_db, Ordering::Release);
        self.shared_state.output_peak.store(peak_mono_db, Ordering::Release);
        if peak_l_db > self.peak_hold_l_value { self.peak_hold_l_value = peak_l_db; }
        if peak_r_db > self.peak_hold_r_value { self.peak_hold_r_value = peak_r_db; }
        if peak_mono_db > self.peak_hold_value { self.peak_hold_value = peak_mono_db; }
        self.shared_state.peak_hold_l.store(self.peak_hold_l_value, Ordering::Release);
        self.shared_state.peak_hold_r.store(self.peak_hold_r_value, Ordering::Release);
        self.shared_state.peak_hold.store(self.peak_hold_value, Ordering::Release);

        // --- Stereo balance + correlation (from output) ---
        if n > 0 {
            let sw = 1.0 / n as f32;
            let rms_l = (sum_power_out_l * sw).sqrt();
            let rms_r = (sum_power_out_r * sw).sqrt();
            let balance = if rms_l + rms_r > 1e-6 { (rms_l - rms_r) / (rms_l + rms_r) } else { 0.0 };
            self.shared_state.balance.store(balance, Ordering::Release);

            let corr = if sum_l2 > 1e-9 && sum_r2 > 1e-9 {
                sum_lr / (sum_l2.sqrt() * sum_r2.sqrt())
            } else {
                1.0
            };
            self.shared_state.phase_correlation.store(corr.clamp(-1.0, 1.0), Ordering::Release);
        }

        ProcessStatus::Normal
    }
}

impl Drop for Lucent {
    fn drop(&mut self) {
        // Stop the liveness thread, then free the slot so relays stop listing this.
        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        self.shared_state.shm_slot.store(-1, Ordering::Release);
        if let Some(slot) = self.claimed_lucent_slot.take() {
            if let Some(hub) = relay_hub() {
                hub.release_lucent_slot(slot);
            }
        }
    }
}

impl ClapPlugin for Lucent {
    const CLAP_ID: &'static str = "be.lxndr.lucent";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Frequency masking analyzer");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Utility,
    ];
}

nice_export_clap!(Lucent);
