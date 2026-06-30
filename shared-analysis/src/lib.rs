use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex};
use atomic_float::AtomicF32;

pub mod dev_log;
pub mod snap_fft;
pub use snap_fft::{SnapFFT, SnapMode};

// Re-export shm-hub transparently so existing callers keep working
pub use shm_hub as shm;
pub use shm_hub::{relay_hub, RelayHub, SPECTRUM_BINS, EQ_BANDS, MAX_NAME_LEN, STALE_MS, MAX_SLOTS, MAX_CONSUMERS, now_ms, display_name};

// Re-export vault/preset/config types so existing callers don't need to change imports
pub use shared_vault::{
    DEFAULT_TOLERANCES,
    Profile,
    export_preset_to_markdown,
    parse_preset_from_markdown,
    preset_plugin_name,
    PluginConfig,
    get_plugin_dir,
    load_config,
    save_config,
    list_custom_presets,
};

pub const SCOPE_BUFFER_LEN: usize = 4096;

/// Compute display-ready spectrum bins from raw FFT output.
/// Applies 4.5 dB/octave tilt compensation so pink noise appears flat.
/// `fft_output` = complex FFT bins (RealFft half-spectrum).
/// `frame` = output slice of length SPECTRUM_BINS, filled with dB values.
#[inline]
pub fn compute_spectrum_bins(fft_output: &[realfft::num_complex::Complex<f32>], frame: &mut [f32], fft_size: usize, sample_rate: f32) {
    let inv_norm = 2.0 / fft_size as f32;
    for (k, slot) in frame.iter_mut().enumerate() {
        let mag = fft_output[k].norm() * inv_norm;
        let db = if mag > 1e-9 { 20.0 * mag.log10() } else { -90.0 };
        let freq = k as f32 * sample_rate / fft_size as f32;
        let tilt = if freq > 20.0 { 4.5 * (freq / 1000.0).log2() } else { 0.0 };
        *slot = (db + tilt).clamp(-90.0, 12.0);
    }
}

/// Shared real-time analyzer values for the GUI.
///
/// ## Plugin ownership (ponytail: split into per-plugin state structs before
/// multi-plugin migration — current monolith works but gets painful fast)
///
/// ── Equilibrium ──
///   band_levels, target_levels, target_tolerances, listen_*,
///   selected_preset_index
///
/// ── Meridian ──
///   gain_reduction, EQ-curve fields (via params), reset_analysis,
///   snap_*, sample_rate, auto_loud_*
///
/// ── Aether ──
///   input_peak
///
/// ── All ──
///   phase_correlation, output_peak[_l,_r], peak_hold[_l,_r],
///   reset_peak, balance, spectrum_bins, spectrum_avg,
///   scope_samples, scope_write_pos
///
/// ── Lucent ──
///   masking_map, shm_slot, resonance (via resonance_hub)
pub struct SharedState {
    pub band_levels: [Arc<AtomicF32>; 5],
    pub target_levels: [Arc<AtomicF32>; 5],
    pub target_tolerances: [Arc<AtomicF32>; 5],
    pub listen_levels: [Arc<AtomicF32>; 5],
    pub listen_tolerances: [Arc<AtomicF32>; 5],
    pub listen_level_min: [Arc<AtomicF32>; 5],
    pub listen_level_max: [Arc<AtomicF32>; 5],
    pub listen_samples: Arc<AtomicF32>,
    pub phase_correlation: Arc<AtomicF32>,
    pub output_peak: Arc<AtomicF32>,
    pub peak_hold: Arc<AtomicF32>,
    /// Input peak (max |L|,|R| per block, dBFS) — for Aether's input reader. Fast
    /// value here; the editor latches the peak-hold (like Meridian's GR display).
    pub input_peak: Arc<AtomicF32>,
    pub output_peak_l: Arc<AtomicF32>,
    pub output_peak_r: Arc<AtomicF32>,
    pub peak_hold_l: Arc<AtomicF32>,
    pub peak_hold_r: Arc<AtomicF32>,
    pub reset_peak: Arc<AtomicBool>,
    pub reset_analysis: Arc<AtomicBool>,
    pub gain_reduction: Arc<AtomicF32>,
    pub balance: Arc<AtomicF32>,
    /// UI sets true to start AUTO LOUD measurement
    pub auto_loud_trigger: Arc<AtomicBool>,
    /// Audio thread sets true while measuring, false when done
    pub auto_loud_measuring: Arc<AtomicBool>,
    /// Audio thread writes computed gain offset (dB) after measurement
    pub auto_loud_gain_offset: Arc<AtomicF32>,
    /// FFT magnitude spectrum — Sum (L+R)*0.5, SPECTRUM_BINS bins, dB with tilt
    pub spectrum_bins: Arc<Mutex<Vec<f32>>>,
    /// Exponential moving average of spectrum_bins (α=1/50, ~50-frame average)
    pub spectrum_avg: Arc<Mutex<Vec<f32>>>,
    /// Ring buffer of [L, R] pairs for the goniometer/vectorscope display
    pub scope_samples: Arc<Mutex<Vec<[f32; 2]>>>,
    /// Write position in scope_samples ring buffer
    pub scope_write_pos: Arc<AtomicUsize>,
    /// Last selected preset index — survives editor close/reopen
    pub selected_preset_index: Arc<AtomicUsize>,
    /// True while SNAP export is running — GUI shows "ANALYZE..."
    pub snap_active: Arc<AtomicBool>,
    /// Sample rate set by audio thread — used by GUI for frequency labels in snapshots
    pub sample_rate: Arc<AtomicF32>,
    /// SNAP measurement phase: 0=idle, 1=stereo, 2=mono, 3=delta
    pub snap_phase: Arc<AtomicU8>,
    /// Spectrum snapshots captured at end of each SNAP phase
    pub snap_stereo_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_mono_snap: Arc<Mutex<Vec<f32>>>,
    pub snap_delta_snap: Arc<Mutex<Vec<f32>>>,
    /// Masking collision map (dB per bin) — where own signal overlaps competing relay
    /// energy. Lucent only; -90 dB means no collision at that bin.
    pub masking_map: Arc<Mutex<Vec<f32>>>,
    /// Shared-memory registry slot claimed by the audio thread (-1 = none yet).
    /// Published here so the editor can refresh the SHM heartbeat from its GUI
    /// tick — keeps Lucent/Relay discoverable even when transport is stopped
    /// (process() doesn't run, so an audio-only heartbeat would go stale).
    pub shm_slot: Arc<AtomicI32>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            band_levels: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            target_levels: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            target_tolerances: [
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[0])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[1])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[2])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[3])),
                Arc::new(AtomicF32::new(DEFAULT_TOLERANCES[4])),
            ],
            listen_levels: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            listen_tolerances: [
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
                Arc::new(AtomicF32::new(0.0)),
            ],
            listen_level_min: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            listen_level_max: [
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
                Arc::new(AtomicF32::new(-90.0)),
            ],
            listen_samples: Arc::new(AtomicF32::new(0.0)),
            phase_correlation: Arc::new(AtomicF32::new(1.0)),
            output_peak: Arc::new(AtomicF32::new(-90.0)),
            peak_hold: Arc::new(AtomicF32::new(-90.0)),
            input_peak: Arc::new(AtomicF32::new(-90.0)),
            output_peak_l: Arc::new(AtomicF32::new(-90.0)),
            output_peak_r: Arc::new(AtomicF32::new(-90.0)),
            peak_hold_l: Arc::new(AtomicF32::new(-90.0)),
            peak_hold_r: Arc::new(AtomicF32::new(-90.0)),
            reset_peak: Arc::new(AtomicBool::new(false)),
            reset_analysis: Arc::new(AtomicBool::new(false)),
            gain_reduction: Arc::new(AtomicF32::new(0.0)),
            balance: Arc::new(AtomicF32::new(0.0)),
            auto_loud_trigger: Arc::new(AtomicBool::new(false)),
            auto_loud_measuring: Arc::new(AtomicBool::new(false)),
            auto_loud_gain_offset: Arc::new(AtomicF32::new(0.0)),
            spectrum_bins: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            spectrum_avg: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            scope_samples: Arc::new(Mutex::new(vec![[0.0, 0.0]; SCOPE_BUFFER_LEN])),
            scope_write_pos: Arc::new(AtomicUsize::new(0)),
            selected_preset_index: Arc::new(AtomicUsize::new(0)),
            snap_active: Arc::new(AtomicBool::new(false)),
            sample_rate: Arc::new(AtomicF32::new(44100.0)),
            snap_phase: Arc::new(AtomicU8::new(0)),
            snap_stereo_snap: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            snap_mono_snap: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            snap_delta_snap: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            masking_map: Arc::new(Mutex::new(vec![-90.0; SPECTRUM_BINS])),
            shm_slot: Arc::new(AtomicI32::new(-1)),
        }
    }
}
