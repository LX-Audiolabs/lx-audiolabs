use nice_plug::prelude::*;
use std::sync::{Arc, RwLock};
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};

use shared_analysis::{relay_hub, SharedState, SPECTRUM_BINS};

mod editor;

pub struct LucentRelay {
    params: Arc<LucentRelayParams>,
    shared_state: Arc<SharedState>,
    /// Cached FFT plan + scratch buffers (allocated once, never on the audio thread).
    fft_fwd: Arc<dyn RealToComplex<f32>>,
    fft_input: Vec<f32>,
    fft_write_pos: usize,
    fft_hann: Vec<f32>,
    fft_windowed: Vec<f32>,
    fft_output: Vec<Complex<f32>>,
    fft_bins: Vec<f32>,
    /// Auto-assigned shared-memory slot, claimed on `initialize`, freed on `Drop`.
    claimed_slot: Option<u8>,
    /// Last name seen from `params.name`, refreshed via non-blocking `try_read`
    /// so the audio thread never blocks and never allocates per frame.
    cached_name: String,
    /// Fallback label ("Relay N") used while the name field is empty.
    fallback_label: String,
    /// Last target Lucent name read from `params.target` (cached, non-blocking).
    /// Empty = no explicit selection (auto-resolve to the single live Lucent).
    cached_target: String,
    /// Reusable scratch buffer for the auto-resolved target name, so the audio
    /// thread never allocates while resolving which Lucent to send to.
    target_buf: [u8; shared_analysis::shm::MAX_NAME_LEN],
    /// Liveness thread "alive" flag. Refreshes this relay's presence (no bins) in
    /// SHM every 100 ms while the plugin is *activated*, so Lucent shows it even
    /// with the transport stopped. Tied to activate/deactivate — disabling the
    /// plugin makes it vanish from Lucent. Audio (process) sends the real FFT.
    liveness: Option<Arc<std::sync::atomic::AtomicBool>>,
}

const FFT_SIZE: usize = 2048;

#[derive(Params)]
pub struct LucentRelayParams {
    #[persist = "lucent-relay-window-state"]
    pub editor_state: Arc<nice_plug_iced::WindowState>,

    /// User-typed display name shown in Lucent's overlay (persisted with the
    /// project). Empty → Lucent shows the "Relay N" fallback. The slot itself
    /// is assigned automatically, so no slot number to configure.
    #[persist = "relay-name"]
    pub name: RwLock<String>,

    /// Target Lucent instance name to send the FFT to (persisted).
    #[persist = "relay-target"]
    pub target: RwLock<String>,
}

impl Default for LucentRelay {
    fn default() -> Self {
        let hann: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let x = i as f32 / (FFT_SIZE - 1) as f32;
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * x).cos())
            })
            .collect();
        let mut planner = RealFftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(FFT_SIZE);
        let fft_output = fft_fwd.make_output_vec();
        Self {
            params: Arc::new(LucentRelayParams::default()),
            shared_state: Arc::new(SharedState::default()),
            fft_fwd,
            fft_input: vec![0.0; FFT_SIZE],
            fft_write_pos: 0,
            fft_hann: hann,
            fft_windowed: vec![0.0; FFT_SIZE],
            fft_output,
            fft_bins: vec![-90.0; SPECTRUM_BINS],
            claimed_slot: None,
            cached_name: String::new(),
            fallback_label: String::from("Relay"),
            cached_target: String::new(),
            target_buf: [0u8; shared_analysis::shm::MAX_NAME_LEN],
            liveness: None,
        }
    }
}

impl Default for LucentRelayParams {
    fn default() -> Self {
        Self {
            editor_state: nice_plug_iced::WindowState::from_logical_size(260, 200),
            name: RwLock::new(String::new()),
            target: RwLock::new(String::new()),
        }
    }
}

impl Plugin for LucentRelay {
    const NAME: &'static str = "Lucent Relay";
    const VENDOR: &'static str = "LX Audiolabs";
    const URL: &'static str = "https://github.com/lxndrbe";
    const EMAIL: &'static str = "ardvinnamoon@gmail.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

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
        // Claim a free overlay slot once. If re-initialized while still holding
        // one, keep it. The actual slot number is invisible to the user.
        if self.claimed_slot.is_none() {
            if let Some(hub) = relay_hub() {
                self.claimed_slot = hub.claim_slot(shared_analysis::shm::now_ms());
            }
        }
        if let Some(slot) = self.claimed_slot {
            self.fallback_label = format!("Relay {}", slot + 1);
        }
        // Publish the slot for the liveness thread.
        self.shared_state.shm_slot.store(
            self.claimed_slot.map(|s| s as i32).unwrap_or(-1),
            std::sync::atomic::Ordering::Release,
        );

        // (Re)spawn the liveness thread. It keeps this relay's presence fresh in SHM
        // every 100 ms while activated (no bins — audio owns the spectrum), mirroring
        // the routing rule: auto-pick the single live Lucent, else the chosen one,
        // else don't advertise. Stopped in deactivate()/Drop → disabling hides it.
        {
            use std::sync::atomic::Ordering;
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
                        let now = shared_analysis::shm::now_ms();
                        let names = hub.read_lucents(now);
                        let sel = params.target.try_read().map(|t| t.clone()).unwrap_or_default();
                        let resolved: Option<String> = if names.len() == 1 {
                            Some(names[0].clone())
                        } else if names.iter().any(|x| x == &sel) {
                            Some(sel)
                        } else {
                            None
                        };
                        if let Some(target) = resolved {
                            let raw = params.name.try_read().map(|n| n.clone()).unwrap_or_default();
                            let label = if raw.is_empty() {
                                format!("Relay {}", slot + 1)
                            } else {
                                raw
                            };
                            hub.touch(slot as u8, &label, &target, now);
                        }
                    }
                }
            });
        }
        true
    }

    fn deactivate(&mut self) {
        // Host disabled the plugin → stop advertising and free the slot so Lucent
        // drops this relay right away.
        if let Some(alive) = self.liveness.take() {
            alive.store(false, std::sync::atomic::Ordering::Release);
        }
        self.shared_state.shm_slot.store(-1, std::sync::atomic::Ordering::Release);
        if let Some(slot) = self.claimed_slot.take() {
            if let Some(hub) = relay_hub() {
                hub.release_slot(slot);
            }
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        #[cfg(target_arch = "x86_64")]
        #[allow(deprecated)]
        unsafe {
            let csr = std::arch::x86_64::_mm_getcsr();
            std::arch::x86_64::_mm_setcsr(csr | 0x8040);
        }

        let now_ms = shared_analysis::shm::now_ms();

        // No slot claimed yet — retry once per buffer.
        if self.claimed_slot.is_none() {
            if let Some(hub) = relay_hub() { self.claimed_slot = hub.claim_slot(now_ms); if let Some(s) = self.claimed_slot { self.fallback_label = format!("Relay {}", s + 1); } }
        }
        if let Ok(name) = self.params.name.try_read() { if *name != self.cached_name { self.cached_name = name.clone(); } }
        if let Ok(target) = self.params.target.try_read() { if *target != self.cached_target { self.cached_target = target.clone(); } }

        let channels = buffer.as_slice();
        if channels.len() < 2 { return ProcessStatus::Normal; }
        let num_samples = channels[0].len();

        // ── Per-sample loop: FFT ring buffer + pass-through ──
        #[allow(clippy::needless_range_loop)]
        for i in 0..num_samples {
            let in_l = channels[0][i];
            let in_r = channels[1][i];

            // FFT on mono input (pre-EQ, for Lucent's masking view)
            let mid = (in_l + in_r) * 0.5;
            self.fft_input[self.fft_write_pos] = mid;
            self.fft_write_pos += 1;

            if self.fft_write_pos >= FFT_SIZE {
                self.fft_write_pos = 0;
                for (d, (s, w)) in self.fft_windowed.iter_mut().zip(self.fft_input.iter().zip(self.fft_hann.iter())) { *d = s * w; }
                if self.fft_fwd.process(&mut self.fft_windowed, &mut self.fft_output).is_ok() {
                    for (b, c) in self.fft_bins.iter_mut().zip(self.fft_output.iter().take(SPECTRUM_BINS)) {
                        let mag = (c.re*c.re + c.im*c.im).sqrt() / FFT_SIZE as f32;
                        *b = if mag < 1e-10 { -90.0 } else { 20.0 * mag.log10() }.clamp(-90.0, 0.0);
                    }
                    if let Some(slot) = self.claimed_slot {
                        if let Some(hub) = relay_hub() {
                            let label: &str = if self.cached_name.is_empty() { &self.fallback_label } else { &self.cached_name };
                            let resolved: Option<&str> = if !self.cached_target.is_empty() && hub.lucent_exists(&self.cached_target, now_ms) { Some(self.cached_target.as_str()) } else if let Some(n) = hub.single_lucent_name(now_ms, &mut self.target_buf) { std::str::from_utf8(&self.target_buf[..n]).ok() } else { None };
                            if let Some(target) = resolved {
                                let dummy_energy = [-90.0f32; 5];
                                hub.write(slot, label, target, &self.fft_bins, &dummy_energy, now_ms);
                            }
                        }
                    }
                }
            }

            // Pass-through: audio unmodified
        }

        ProcessStatus::Normal
    }
}

impl Drop for LucentRelay {
    fn drop(&mut self) {
        // Stop the liveness thread, then hand the slot back so a new relay can
        // reuse it without waiting for the stale-heartbeat timeout.
        if let Some(alive) = self.liveness.take() {
            alive.store(false, std::sync::atomic::Ordering::Release);
        }
        if let Some(slot) = self.claimed_slot.take() {
            if let Some(hub) = relay_hub() {
                hub.release_slot(slot);
            }
        }
    }
}

impl ClapPlugin for LucentRelay {
    const CLAP_ID: &'static str = "be.lxndr.lucent-relay";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("FFT spectrum relay — sends track audio analysis to Lucent");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::Analyzer,
        ClapFeature::Utility,
    ];
}

nice_export_clap!(LucentRelay);
