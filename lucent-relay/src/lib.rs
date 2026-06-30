use truce::prelude::*;
use truce_core::custom_state::State as StateSerialize;
use truce_core::state::StateLoadError;
use truce_iced::IcedEditor;
use std::sync::{Arc, Mutex};
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};

use shared_analysis::{relay_hub, SharedState, SPECTRUM_BINS};

mod editor;

const FFT_SIZE: usize = 2048;
const WINDOW_W: u32 = 260;
const WINDOW_H: u32 = 160;

// ─── Parameters ──────────────────────────────────────────────────────────────
// Truce requires at least one Param field. Bypass is semantically correct
// for a pass-through analyzer.

#[derive(Params)]
pub struct LucentRelayParams {
    #[param(name = "Bypass", default = 0)]
    pub bypass: BoolParam,
}

// ─── Persistent state (strings → State, not Params in Truce) ─────────────────

#[derive(State, Default, Clone)]
pub struct RelayPersist {
    pub name:   String,
    pub target: String,
}

// ─── Shared handle: audio thread ↔ UI ────────────────────────────────────────

#[derive(Default, Clone)]
pub struct RelayHandle(pub Arc<Mutex<RelayPersist>>);

impl RelayHandle {
    pub fn name(&self)   -> String { self.0.lock().map(|s| s.name.clone()).unwrap_or_default() }
    pub fn target(&self) -> String { self.0.lock().map(|s| s.target.clone()).unwrap_or_default() }
}

// ─── Plugin ───────────────────────────────────────────────────────────────────

pub struct LucentRelay {
    params:        Arc<LucentRelayParams>,
    relay_handle:  RelayHandle,
    shm_state:     Arc<SharedState>,
    fft_fwd:       Arc<dyn RealToComplex<f32>>,
    fft_input:     Vec<f32>,
    fft_write_pos: usize,
    fft_hann:      Vec<f32>,
    fft_windowed:  Vec<f32>,
    fft_output:    Vec<Complex<f32>>,
    fft_bins:      Vec<f32>,
    claimed_slot:  Option<u8>,
    cached_name:   String,
    fallback_label: String,
    cached_target:  String,
    target_buf:    [u8; shared_analysis::shm::MAX_NAME_LEN],
    liveness:      Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl LucentRelay {
    pub fn new(params: Arc<LucentRelayParams>) -> Self {
        let hann: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let x = i as f32 / (FFT_SIZE - 1) as f32;
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * x).cos())
            })
            .collect();
        let mut planner = RealFftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(FFT_SIZE);
        let fft_output = fft_fwd.make_output_vec();
        let relay_handle = RelayHandle::default();
        editor::set_relay_handle(relay_handle.clone());
        Self {
            params,
            relay_handle,
            shm_state:      Arc::new(SharedState::default()),
            fft_fwd,
            fft_input:      vec![0.0; FFT_SIZE],
            fft_write_pos:  0,
            fft_hann:       hann,
            fft_windowed:   vec![0.0; FFT_SIZE],
            fft_output,
            fft_bins:       vec![-90.0; SPECTRUM_BINS],
            claimed_slot:   None,
            cached_name:    String::new(),
            fallback_label: String::from("Relay"),
            cached_target:  String::new(),
            target_buf:     [0u8; shared_analysis::shm::MAX_NAME_LEN],
            liveness:       None,
        }
    }

    fn claim_slot(&mut self) {
        if self.claimed_slot.is_none() {
            if let Some(hub) = relay_hub() {
                self.claimed_slot = hub.claim_slot(shared_analysis::shm::now_ms());
                if let Some(s) = self.claimed_slot {
                    self.fallback_label = format!("Relay {}", s + 1);
                }
            }
        }
        self.shm_state.shm_slot.store(
            self.claimed_slot.map(|s| s as i32).unwrap_or(-1),
            std::sync::atomic::Ordering::Release,
        );
    }

    fn spawn_liveness_thread(&mut self) {
        use std::sync::atomic::Ordering;
        if let Some(alive) = self.liveness.take() { alive.store(false, Ordering::Release); }
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.liveness = Some(alive.clone());
        let ss = self.shm_state.clone();
        let handle = self.relay_handle.clone();
        std::thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let slot = ss.shm_slot.load(Ordering::Acquire);
                if slot < 0 { continue; }
                if let Some(hub) = relay_hub() {
                    let now = shared_analysis::shm::now_ms();
                    let lucents = hub.read_lucents(now);
                    let sel = handle.target();
                    let resolved: Option<String> = if lucents.len() == 1 {
                        Some(lucents[0].clone())
                    } else if lucents.iter().any(|x| *x == sel) {
                        Some(sel)
                    } else {
                        None
                    };
                    if let Some(target) = resolved {
                        let raw = handle.name();
                        let label = if raw.is_empty() { format!("Relay {}", slot + 1) } else { raw };
                        hub.touch(slot as u8, &label, &target, now);
                    }
                }
            }
        });
    }

    fn publish_fft(&mut self, now_ms: u64) {
        let Some(slot) = self.claimed_slot else { return };
        let Some(hub) = relay_hub() else { return };
        let label: &str = if self.cached_name.is_empty() { &self.fallback_label } else { &self.cached_name };
        let resolved: Option<&str> = if !self.cached_target.is_empty()
            && hub.lucent_exists(&self.cached_target, now_ms)
        {
            Some(self.cached_target.as_str())
        } else if let Some(n) = hub.single_lucent_name(now_ms, &mut self.target_buf) {
            std::str::from_utf8(&self.target_buf[..n]).ok()
        } else {
            None
        };
        if let Some(target) = resolved {
            hub.write(slot, label, target, &self.fft_bins, &[-90.0f32; 5], now_ms);
        }
    }
}

impl PluginLogic for LucentRelay {
    fn reset(&mut self, _sr: f64, _max: usize) {
        self.claim_slot();
        self.spawn_liveness_thread();
    }

    fn process(
        &mut self,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            #[allow(deprecated)]
            let csr = std::arch::x86_64::_mm_getcsr();
            #[allow(deprecated)]
            std::arch::x86_64::_mm_setcsr(csr | 0x8040);
        }

        let now_ms = shared_analysis::shm::now_ms();
        if self.claimed_slot.is_none() { self.claim_slot(); }

        let n = self.relay_handle.name();
        if n != self.cached_name { self.cached_name = n; }
        let t = self.relay_handle.target();
        if t != self.cached_target { self.cached_target = t; }

        // Pass-through (copy input → output per channel)
        for ch in 0..buffer.channels() {
            let (inp, out) = buffer.io(ch);
            out.copy_from_slice(inp);
        }

        // FFT: read inputs via &self method to avoid double &mut borrow
        let n_samples = buffer.num_samples();
        for i in 0..n_samples {
            let l = buffer.input(0)[i];
            let r = if buffer.num_input_channels() > 1 { buffer.input(1)[i] } else { l };
            self.fft_input[self.fft_write_pos] = (l + r) * 0.5;
            self.fft_write_pos += 1;

            if self.fft_write_pos >= FFT_SIZE {
                self.fft_write_pos = 0;
                for (d, (s, w)) in self.fft_windowed.iter_mut()
                    .zip(self.fft_input.iter().zip(self.fft_hann.iter()))
                { *d = s * w; }
                if self.fft_fwd.process(&mut self.fft_windowed, &mut self.fft_output).is_ok() {
                    for (b, c) in self.fft_bins.iter_mut().zip(self.fft_output.iter().take(SPECTRUM_BINS)) {
                        let mag = (c.re * c.re + c.im * c.im).sqrt() / FFT_SIZE as f32;
                        *b = if mag < 1e-10 { -90.0 } else { 20.0 * mag.log10() }.clamp(-90.0, 0.0);
                    }
                    self.publish_fft(now_ms);
                }
            }
        }
        ProcessStatus::Normal
    }

    fn save_state(&self) -> Vec<u8> {
        self.relay_handle.0.lock()
            .map(|g| g.serialize())
            .unwrap_or_default()
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), StateLoadError> {
        match RelayPersist::deserialize(data) {
            Some(p) => {
                if let Ok(mut g) = self.relay_handle.0.lock() { *g = p; }
                Ok(())
            }
            None => Err(StateLoadError::Malformed("RelayPersist")),
        }
    }

    fn state_changed(&mut self) {
        // Preset recall / undo / session load — sync cached relay handle state.
        if let Ok(g) = self.relay_handle.0.lock() {
            self.cached_name = g.name.clone();
            self.cached_target = g.target.clone();
        }
    }

    fn editor(&self) -> Box<dyn Editor> {
        IcedEditor::<LucentRelayParams, editor::RelayUi>::new(
            self.params.clone(),
            (WINDOW_W, WINDOW_H),
        )
        .into_editor()
    }
}

impl Drop for LucentRelay {
    fn drop(&mut self) {
        if let Some(alive) = self.liveness.take() {
            alive.store(false, std::sync::atomic::Ordering::Release);
        }
        if let Some(slot) = self.claimed_slot.take() {
            if let Some(hub) = relay_hub() { hub.release_slot(slot); }
        }
    }
}

truce::plugin! {
    logic: LucentRelay,
    params: LucentRelayParams,
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
