//! Plugin dialog + plugin-hint state (install hints, marketplace /
//! installed / errors tabs).
//!
//! Split from `surface_payloads.rs`; the dialog interceptor still lives in
//! `update/plugin_dialog.rs` until that surface migrates to `modal_pane/`.

/// Plugin-hint recommendation dialog state.
///
/// Surfaced when a CLI/SDK emits a `<claude-code-hint />` tag referencing a
/// plugin and the pre-store gate passed. Show-once per plugin. The user
/// picks install / dismiss / disable-all.
#[derive(Debug, Clone)]
pub struct PluginHintState {
    /// Fully-qualified plugin ID (`name@marketplace`).
    pub plugin_id: String,
    /// Human-readable plugin name.
    pub plugin_name: String,
    /// The marketplace that hosts the plugin.
    pub marketplace_name: String,
    /// Short description from the marketplace entry.
    pub plugin_description: Option<String>,
    /// First token of the command that emitted the hint.
    pub source_command: String,
    /// Selected option index: 0 = install, 1 = dismiss, 2 = disable-all.
    pub selected: i32,
}

impl PluginHintState {
    /// Number of selectable options.
    pub const OPTION_COUNT: i32 = 3;

    /// The response keyed by the current selection.
    pub fn selected_response(&self) -> PluginHintResponse {
        match self.selected {
            0 => PluginHintResponse::Install,
            2 => PluginHintResponse::Disable,
            _ => PluginHintResponse::Dismiss,
        }
    }
}

/// User decision on a plugin-hint dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginHintResponse {
    /// Install the recommended plugin.
    Install,
    /// Dismiss without installing.
    Dismiss,
    /// Dismiss and never show plugin-install hints again.
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDialogTab {
    Installed,
    Marketplaces,
    Errors,
}

impl PluginDialogTab {
    pub const ALL: [Self; 3] = [Self::Installed, Self::Marketplaces, Self::Errors];

    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Marketplaces => "Marketplaces",
            Self::Errors => "Errors",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Installed => Self::Marketplaces,
            Self::Marketplaces => Self::Errors,
            Self::Errors => Self::Installed,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Installed => Self::Errors,
            Self::Marketplaces => Self::Installed,
            Self::Errors => Self::Marketplaces,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginDialogState {
    pub installed: Vec<coco_types::PluginDialogInstalledRow>,
    pub marketplaces: Vec<coco_types::PluginDialogMarketplaceRow>,
    pub errors: Vec<coco_types::PluginDialogErrorRow>,
    pub selected_tab: PluginDialogTab,
    pub selected_idx: usize,
    pub filter_query: String,
    pub filter_focused: bool,
}

impl PluginDialogState {
    pub fn from_wire(payload: coco_types::PluginDialogPayload) -> Self {
        Self {
            installed: payload.installed,
            marketplaces: payload.marketplaces,
            errors: payload.errors,
            selected_tab: PluginDialogTab::Installed,
            selected_idx: 0,
            filter_query: String::new(),
            filter_focused: false,
        }
    }

    pub fn selected_len(&self) -> usize {
        match self.selected_tab {
            PluginDialogTab::Installed => self.filtered_installed_indices().len(),
            PluginDialogTab::Marketplaces => self.filtered_marketplace_indices().len(),
            PluginDialogTab::Errors => self.filtered_error_indices().len(),
        }
    }

    pub fn move_down(&mut self) {
        let len = self.selected_len();
        if len > 0 {
            self.selected_idx = (self.selected_idx + 1).min(len - 1);
        }
    }

    pub fn move_up(&mut self) {
        self.selected_idx = self.selected_idx.saturating_sub(1);
    }

    pub fn cycle_tab_next(&mut self) {
        self.selected_tab = self.selected_tab.next();
        self.selected_idx = 0;
    }

    pub fn cycle_tab_prev(&mut self) {
        self.selected_tab = self.selected_tab.prev();
        self.selected_idx = 0;
    }

    pub fn apply_filter_char(&mut self, c: char) {
        if c == '\n' || c == '\r' {
            return;
        }
        if c == '/' && self.filter_query.is_empty() {
            return;
        }
        self.filter_query.push(c.to_ascii_lowercase());
        self.selected_idx = 0;
    }

    pub fn backspace_filter(&mut self) -> bool {
        let changed = self.filter_query.pop().is_some();
        if changed {
            self.selected_idx = 0;
        }
        changed
    }

    pub fn clear_filter(&mut self) {
        self.filter_query.clear();
        self.filter_focused = false;
        self.selected_idx = 0;
    }

    pub fn filtered_installed_indices(&self) -> Vec<usize> {
        self.installed
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                self.matches_filter(&[&row.id, &row.name, row.description.as_deref().unwrap_or("")])
                    .then_some(idx)
            })
            .collect()
    }

    pub fn filtered_marketplace_indices(&self) -> Vec<usize> {
        self.marketplaces
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                self.matches_filter(&[&row.name, row.source.as_deref().unwrap_or("")])
                    .then_some(idx)
            })
            .collect()
    }

    pub fn filtered_error_indices(&self) -> Vec<usize> {
        self.errors
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                self.matches_filter(&[&row.plugin_id, &row.message])
                    .then_some(idx)
            })
            .collect()
    }

    pub fn focused_action(&self) -> Option<coco_types::PluginDialogAction> {
        match self.selected_tab {
            PluginDialogTab::Installed => {
                let idx = *self.filtered_installed_indices().get(self.selected_idx)?;
                self.installed.get(idx)?.actions.first().cloned()
            }
            PluginDialogTab::Marketplaces => {
                let idx = *self.filtered_marketplace_indices().get(self.selected_idx)?;
                self.marketplaces.get(idx)?.actions.first().cloned()
            }
            PluginDialogTab::Errors => None,
        }
    }

    fn matches_filter(&self, fields: &[&str]) -> bool {
        if self.filter_query.is_empty() {
            return true;
        }
        fields
            .iter()
            .any(|field| field.to_ascii_lowercase().contains(&self.filter_query))
    }
}
