/// Single Relay data (from Lucent-Relay plugin)
#[derive(Clone, Debug)]
pub struct RelayData {
    pub name: String,
    pub spectrum: Vec<f32>, // FFT bins in dB
    pub active: bool,
}

// ponytail: dead test helpers removed — YAGNI

/// Lucent UI State — manages own spectrum + relay feeds + plugin name
#[derive(Clone, Debug)]
pub struct LucentUiState {
    pub own_spectrum: Vec<f32>, // FFT bins in dB from this plugin's audio
    pub relays: Vec<RelayData>,
}

impl LucentUiState {
    pub fn new() -> Self {
        Self {
            own_spectrum: vec![],
            relays: vec![],
        }
    }

    /// Clear all relay data (Standalone mode — no relay interaction).
    pub fn clear_relays(&mut self) {
        self.relays.clear();
    }

    /// Replace the relay list with live feeds from the shared-memory hub,
    /// preserving the user's per-relay active toggle (matched by name).
    /// Applies EMA smoothing (α = 1/6) so relay spectra don't jump frame-to-frame.
    pub fn sync_relays(&mut self, feeds: Vec<(String, Vec<f32>)>) {
        let alpha: f32 = 1.0 / 6.0; // ~100 ms smoothing at ~17 FFT frames/s
        let new_relays = feeds
            .into_iter()
            .map(|(name, spectrum)| {
                let active = self
                    .relays
                    .iter()
                    .find(|r| r.name == name)
                    .map(|r| r.active)
                    .unwrap_or(true);
                // EMA-smooth the spectrum with the previous frame for this relay
                let smoothed = if let Some(prev) = self.relays.iter().find(|r| r.name == name) {
                    if prev.spectrum.len() == spectrum.len() {
                        prev.spectrum.iter().zip(spectrum.iter())
                            .map(|(&p, &s)| p * (1.0 - alpha) + s * alpha)
                            .collect()
                    } else {
                        spectrum
                    }
                } else {
                    spectrum
                };
                RelayData { name, spectrum: smoothed, active }
            })
            .collect();
        self.relays = new_relays;
    }

}

impl Default for LucentUiState {
    fn default() -> Self {
        Self::new()
    }
}
