use std::path::PathBuf;

pub struct CommonEditorState<P> {
    pub vault_path: Option<String>,
    pub vault_path_input: String,
    pub show_setup: bool,
    pub preset_name_input: String,
    pub presets: Vec<(String, Option<PathBuf>, P)>,
    pub selected_preset_index: Option<usize>,
    pub preset_warning: Option<String>,
    preset_warning_ticks: u32,
    preset_refresh_counter: u32,
}

impl<P> CommonEditorState<P> {
    pub fn new(
        vault_path: Option<String>,
        presets: Vec<(String, Option<PathBuf>, P)>,
        selected: Option<usize>,
    ) -> Self {
        Self {
            vault_path,
            vault_path_input: String::new(),
            show_setup: false,
            preset_name_input: String::new(),
            presets,
            selected_preset_index: selected,
            preset_warning: None,
            preset_warning_ticks: 0,
            preset_refresh_counter: 0,
        }
    }

    /// Returns true when a periodic preset refresh should run (every ~150 ticks).
    /// Also ages out the preset_warning display.
    pub fn tick_preset_state(&mut self) -> bool {
        if self.preset_warning.is_some() {
            self.preset_warning_ticks += 1;
            if self.preset_warning_ticks >= 200 {
                self.preset_warning = None;
                self.preset_warning_ticks = 0;
            }
        }
        self.preset_refresh_counter += 1;
        if self.preset_refresh_counter >= 150 {
            self.preset_refresh_counter = 0;
            true
        } else {
            false
        }
    }

    pub fn handle_setup_toggled(&mut self) {
        self.show_setup = !self.show_setup;
        if self.show_setup {
            self.vault_path_input = self.vault_path.clone().unwrap_or_default();
        }
    }

    pub fn handle_vault_path_changed(&mut self, path: String) {
        self.vault_path_input = path;
    }

    pub fn handle_preset_name_changed(&mut self, name: String) {
        self.preset_name_input = name;
    }

    /// Saves vault path to config and updates internal state.
    /// Returns Some(new_vault_path) on success, None on failure.
    pub fn save_vault_path(&mut self, plugin_name: &str) -> Option<Option<String>> {
        let new_path = if self.vault_path_input.trim().is_empty() {
            None
        } else {
            Some(self.vault_path_input.trim().to_string())
        };
        let config = shared_analysis::PluginConfig {
            vault_path: new_path.clone(),
            ..Default::default()
        };
        if shared_analysis::save_config(plugin_name, &config).is_ok() {
            self.vault_path = new_path.clone();
            self.show_setup = false;
            Some(new_path)
        } else {
            None
        }
    }
}
