/// Single Relay data (from Lucent-Relay plugin)
#[derive(Clone, Debug)]
pub struct RelayData {
    pub name: String,
    pub spectrum: Vec<f32>, // FFT bins in dB
    pub active: bool,
}

impl RelayData {
    #[allow(dead_code)]
    pub fn new(name: String) -> Self {
        Self {
            name,
            spectrum: vec![],
            active: true,
        }
    }

    #[allow(dead_code)]
    pub fn dummy(name: &str) -> Self {
        let mut spectrum = vec![0.0f32; 1024];
        // Create some synthetic peak patterns for visual testing
        for (i, s) in spectrum.iter_mut().enumerate() {
            let freq_factor = i as f32 / 1024.0;
            *s = -40.0 + 20.0 * (freq_factor * std::f32::consts::PI).sin();
        }
        Self {
            name: name.to_string(),
            spectrum,
            active: true,
        }
    }
}

/// Lucent UI State — manages own spectrum + relay feeds + plugin name
#[derive(Clone, Debug)]
pub struct LucentUiState {
    pub own_spectrum: Vec<f32>, // FFT bins in dB from this plugin's audio
    pub relays: Vec<RelayData>,
    pub name: String,
}

impl LucentUiState {
    pub fn new() -> Self {
        Self {
            own_spectrum: vec![],
            relays: vec![],
            name: "Lucent".to_string(),
        }
    }

    /// For testing: create with dummy relays
    #[allow(dead_code)]
    pub fn with_dummy_relays() -> Self {
        Self {
            own_spectrum: vec![-40.0f32; 1024],
            relays: vec![
                RelayData::dummy("Kick"),
                RelayData::dummy("Bass"),
                RelayData::dummy("Synth"),
            ],
            name: "Lucent".to_string(),
        }
    }

    pub fn toggle_relay(&mut self, index: usize) {
        if let Some(relay) = self.relays.get_mut(index) {
            relay.active = !relay.active;
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

    pub fn active_relays(&self) -> Vec<&RelayData> {
        self.relays.iter().filter(|r| r.active).collect()
    }

    #[allow(dead_code)]
    pub fn sum_active_spectra(&self) -> Vec<f32> {
        if self.relays.is_empty() {
            return self.own_spectrum.clone();
        }

        let active = self.active_relays();
        if active.is_empty() {
            return self.own_spectrum.clone();
        }

        let len = active.iter().map(|r| r.spectrum.len()).max().unwrap_or(1024);
        let mut sum = vec![f32::NEG_INFINITY; len];

        for relay in &active {
            for (i, &val) in relay.spectrum.iter().enumerate() {
                if i < sum.len() {
                    // Log-domain sum (max for energy)
                    sum[i] = sum[i].max(val);
                }
            }
        }

        sum
    }
}

impl Default for LucentUiState {
    fn default() -> Self {
        Self::new()
    }
}
