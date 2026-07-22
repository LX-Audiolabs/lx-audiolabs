//! Single Relay data (from Lucent-Relay plugin)
#[derive(Clone, Debug, PartialEq)]
pub struct RelayData {
    /// SHM publisher slot — stable identity even when labels collide.
    pub slot: u8,
    pub name: String,
    pub spectrum: Vec<f32>, // FFT bins in dB
    pub active: bool,
}

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
    /// preserving the user's per-relay active toggle (matched by SHM slot).
    /// Applies EMA smoothing (α = 1/6) so relay spectra don't jump frame-to-frame.
    pub fn sync_relays(&mut self, feeds: Vec<(u8, String, Vec<f32>)>) {
        let alpha: f32 = 1.0 / 6.0; // ~100 ms smoothing at ~17 FFT frames/s
        let new_relays = feeds
            .into_iter()
            .map(|(slot, name, spectrum)| {
                let active = self
                    .relays
                    .iter()
                    .find(|r| r.slot == slot)
                    .map(|r| r.active)
                    .unwrap_or(true);
                let smoothed = if let Some(prev) = self.relays.iter().find(|r| r.slot == slot) {
                    if prev.spectrum.len() == spectrum.len() {
                        prev.spectrum
                            .iter()
                            .zip(spectrum.iter())
                            .map(|(&p, &s)| p * (1.0 - alpha) + s * alpha)
                            .collect()
                    } else {
                        spectrum
                    }
                } else {
                    spectrum
                };
                RelayData {
                    slot,
                    name,
                    spectrum: smoothed,
                    active,
                }
            })
            .collect();
        self.relays = new_relays;
    }

    /// Bitmask for `SharedState::relay_active_mask` (bit `i` = slot `i` on).
    pub fn relay_active_mask(&self) -> u32 {
        let mut mask = 0u32;
        for r in &self.relays {
            if r.active {
                mask |= 1u32 << r.slot;
            }
        }
        mask
    }
}

impl Default for LucentUiState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_relays_preserves_active_toggle_by_slot() {
        let mut ui = LucentUiState::new();
        ui.sync_relays(vec![(0, "Kick".into(), vec![-40.0; 4])]);
        ui.relays[0].active = false;

        ui.sync_relays(vec![(0, "Kick Renamed".into(), vec![-30.0; 4])]);
        assert!(!ui.relays[0].active);
        assert_eq!(ui.relays[0].name, "Kick Renamed");
    }

    #[test]
    fn relay_active_mask_reflects_toggles() {
        let mut ui = LucentUiState::new();
        ui.sync_relays(vec![
            (0, "A".into(), vec![]),
            (2, "B".into(), vec![]),
        ]);
        ui.relays[0].active = false;
        assert_eq!(ui.relay_active_mask(), 1u32 << 2);
    }
}