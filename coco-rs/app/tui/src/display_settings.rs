//! TUI display preferences derived from `settings.json`.

use coco_config::SettingSource;
use coco_config::SettingsWithSource;
use coco_config::settings::NativeReplayCacheSettings;
use coco_config::settings::SYNTAX_HIGHLIGHTING_DISABLED_KEY;
use coco_config::settings::TuiPerformanceSettings;
use coco_tui_ui::display::SyntaxHighlighting;
use std::time::Duration;

use crate::transcript::render::HistoryReplayCachePolicy;

/// Whether a display preference can be edited from the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplaySettingEditability {
    #[default]
    Editable,
    OverriddenBy(SettingSource),
}

impl DisplaySettingEditability {
    pub fn is_editable(self) -> bool {
        matches!(self, Self::Editable)
    }

    pub fn overriding_source(self) -> Option<SettingSource> {
        match self {
            Self::Editable => None,
            Self::OverriddenBy(source) => Some(source),
        }
    }
}

/// Display-only preferences consumed by TUI renderers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplaySettings {
    pub syntax_highlighting: SyntaxHighlighting,
    pub syntax_highlighting_editability: DisplaySettingEditability,
    pub show_thinking: bool,
    pub copy_full_response: bool,
    pub status_line: Option<coco_config::StatusLineSettings>,
    pub native_replay_cache: HistoryReplayCachePolicy,
    pub performance: TuiPerformanceConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiPerformanceConfig {
    pub enabled: bool,
    pub sample_every_n_frames: u64,
    pub slow_frame_ms: u64,
    pub slow_stage_us: u64,
}

impl Default for TuiPerformanceConfig {
    fn default() -> Self {
        performance_config(TuiPerformanceSettings::default())
    }
}

impl DisplaySettings {
    pub fn from_settings(settings: &coco_config::Settings) -> Self {
        Self {
            syntax_highlighting: SyntaxHighlighting::from_disabled(
                settings.syntax_highlighting_disabled,
            ),
            syntax_highlighting_editability: DisplaySettingEditability::Editable,
            show_thinking: settings.show_thinking,
            copy_full_response: settings.copy_full_response,
            status_line: settings.status_line.clone(),
            native_replay_cache: replay_cache_policy(settings.tui.native_replay_cache),
            performance: performance_config(settings.tui.performance),
        }
    }

    pub fn from_settings_with_sources(settings: &SettingsWithSource) -> Self {
        Self {
            syntax_highlighting: SyntaxHighlighting::from_disabled(
                settings.merged.syntax_highlighting_disabled,
            ),
            syntax_highlighting_editability: syntax_highlighting_editability(settings),
            show_thinking: settings.merged.show_thinking,
            copy_full_response: settings.merged.copy_full_response,
            status_line: settings.merged.status_line.clone(),
            native_replay_cache: replay_cache_policy(settings.merged.tui.native_replay_cache),
            performance: performance_config(settings.merged.tui.performance),
        }
    }

    pub fn from_runtime_config(config: &coco_config::RuntimeConfig) -> Self {
        Self::from_settings_with_sources(&config.settings)
    }

    pub fn with_syntax_highlighting(self, syntax_highlighting: SyntaxHighlighting) -> Self {
        Self {
            syntax_highlighting,
            ..self
        }
    }

    pub fn with_copy_full_response(self, copy_full_response: bool) -> Self {
        Self {
            copy_full_response,
            ..self
        }
    }
}

fn replay_cache_policy(settings: NativeReplayCacheSettings) -> HistoryReplayCachePolicy {
    HistoryReplayCachePolicy {
        enabled: settings.enabled,
        max_entries: settings.max_entries,
        max_estimated_bytes: kib_to_bytes(settings.max_estimated_kb),
        min_cells: settings.min_cells,
        min_content_bytes: kib_to_bytes(settings.min_content_kb),
        admit_min_render_elapsed: Duration::from_micros(settings.admit_min_render_us),
        admit_min_result_bytes: kib_to_bytes(settings.admit_min_result_kb),
    }
}

fn performance_config(settings: TuiPerformanceSettings) -> TuiPerformanceConfig {
    TuiPerformanceConfig {
        enabled: settings.enabled,
        sample_every_n_frames: settings.sample_every_n_frames,
        slow_frame_ms: settings.slow_frame_ms,
        slow_stage_us: settings.slow_stage_us,
    }
}

fn kib_to_bytes(kib: usize) -> usize {
    kib.saturating_mul(1024)
}

fn syntax_highlighting_editability(settings: &SettingsWithSource) -> DisplaySettingEditability {
    settings
        .per_source
        .iter()
        .filter_map(|(source, value)| {
            if *source > SettingSource::User
                && value_contains_dotted_key(value, SYNTAX_HIGHLIGHTING_DISABLED_KEY)
            {
                Some(*source)
            } else {
                None
            }
        })
        .max()
        .map(DisplaySettingEditability::OverriddenBy)
        .unwrap_or_default()
}

fn value_contains_dotted_key(value: &serde_json::Value, key: &str) -> bool {
    let mut current = value;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        let Some(next) = current.get(part) else {
            return false;
        };
        if parts.peek().is_none() {
            return true;
        }
        current = next;
    }
    false
}

#[cfg(test)]
#[path = "display_settings.test.rs"]
mod tests;
