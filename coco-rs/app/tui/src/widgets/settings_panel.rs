//! State for the searchable settings browser.
//!
//! Registry metadata lives in [`crate::settings_registry`]; this type owns only
//! view state. Rendering reads live values from [`crate::state::UiState`], and
//! persistence stays in the update layer.

use crate::settings_registry::SettingMeta;
use crate::settings_registry::matches;
use crate::settings_registry::settings;

#[derive(Debug, Clone, Default)]
pub struct SettingsPanelState {
    /// Index into [`Self::filtered_settings`], never the global registry.
    pub selected: i32,
    pub filter: String,
}

impl SettingsPanelState {
    pub(crate) fn filtered_settings(&self) -> Vec<&'static SettingMeta> {
        settings()
            .iter()
            .filter(|meta| matches(meta, &self.filter))
            .collect()
    }

    pub(crate) fn selected_setting(&self) -> Option<&'static SettingMeta> {
        self.filtered_settings()
            .get(self.selected.max(0) as usize)
            .copied()
    }

    pub(crate) fn insert_filter(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    pub(crate) fn filter_backspace(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }
}

#[cfg(test)]
#[path = "settings_panel.test.rs"]
mod tests;
