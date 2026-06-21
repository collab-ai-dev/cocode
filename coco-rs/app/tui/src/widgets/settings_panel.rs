//! Settings panel state — backing struct for the in-TUI settings
//! modal. Rendering lives in [`crate::presentation::settings`]; this
//! module owns only the typed state + helper methods.
//!
//! State/render are split across crate modules so the same state can be
//! rendered into both the modal surface and the embedded surface
//! content path without duplicating the widget.

use crate::display_settings::DisplaySettings;
use crate::i18n::t;
use coco_tui_ui::display::SyntaxHighlighting;

/// Settings panel tab. Theme selection moved to the standalone `/theme` picker;
/// the first tab now hosts only display toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Display,
    OutputStyle,
    Permissions,
    About,
}

/// Settings panel state.
#[derive(Debug, Clone)]
pub struct SettingsPanelState {
    pub active_tab: SettingsTab,
    pub selected: i32,
    pub display_settings: DisplaySettings,
    pub output_styles: Vec<String>,
    pub permission_rules: Vec<PermissionRuleDisplay>,
}

/// A permission rule for display.
#[derive(Debug, Clone)]
pub struct PermissionRuleDisplay {
    pub tool: String,
    pub behavior: String,
    pub source: String,
}

impl SettingsPanelState {
    pub fn new(display_settings: DisplaySettings) -> Self {
        Self {
            active_tab: SettingsTab::Display,
            selected: 0,
            display_settings,
            output_styles: Vec::new(),
            permission_rules: Vec::new(),
        }
    }

    pub fn set_display_settings(&mut self, display_settings: DisplaySettings) {
        self.display_settings = display_settings;
    }

    pub fn is_syntax_highlighting_selected(&self) -> bool {
        self.selected == self.syntax_highlighting_index()
    }

    pub fn is_copy_full_response_selected(&self) -> bool {
        self.selected == self.copy_full_response_index()
    }

    /// The Display tab has two selectable rows: syntax highlighting + copy.
    pub fn display_item_count(&self) -> usize {
        2
    }

    fn syntax_highlighting_index(&self) -> i32 {
        0
    }

    fn copy_full_response_index(&self) -> i32 {
        1
    }

    pub fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            SettingsTab::Display => SettingsTab::OutputStyle,
            SettingsTab::OutputStyle => SettingsTab::Permissions,
            SettingsTab::Permissions => SettingsTab::About,
            SettingsTab::About => SettingsTab::Display,
        };
        self.selected = 0;
    }

    pub fn prev_tab(&mut self) {
        self.active_tab = match self.active_tab {
            SettingsTab::Display => SettingsTab::About,
            SettingsTab::OutputStyle => SettingsTab::Display,
            SettingsTab::Permissions => SettingsTab::OutputStyle,
            SettingsTab::About => SettingsTab::Permissions,
        };
        self.selected = 0;
    }
}

impl Default for SettingsPanelState {
    fn default() -> Self {
        Self::new(DisplaySettings::default())
    }
}

pub(crate) fn syntax_highlighting_status(syntax_highlighting: SyntaxHighlighting) -> String {
    match syntax_highlighting {
        SyntaxHighlighting::Enabled => t!("settings.enabled").to_string(),
        SyntaxHighlighting::Disabled => t!("settings.disabled").to_string(),
    }
}

pub(crate) fn syntax_highlighting_status_for_display(settings: DisplaySettings) -> String {
    let status = syntax_highlighting_status(settings.syntax_highlighting);
    if let Some(source) = settings.syntax_highlighting_editability.overriding_source() {
        format!(
            "{status} ({})",
            t!("settings.overridden_by", source = source.as_str())
        )
    } else {
        status
    }
}
