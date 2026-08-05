//! TUI display preferences derived from `settings.json`.

use coco_config::SettingSource;
use coco_config::SettingsWithSource;
use coco_config::settings::NativeReplayCacheSettings;
use coco_config::settings::ReflowMaxRows;
use coco_config::settings::SyntaxHighlightingLevel;
use coco_config::settings::TerminalTitleItem;
use coco_config::settings::TuiPerformanceSettings;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::motion::MotionMode;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::reflow_cap::MaxReflowRows;
use crate::reflow_cap::resolve_max_reflow_rows;
use crate::settings_registry::SettingId;
use crate::settings_registry::settings as registered_settings;
use crate::transcript::render::HistoryReplayCachePolicy;

/// Display-only preferences consumed by TUI renderers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplaySettings {
    pub syntax_highlighting: SyntaxHighlighting,
    pub show_thinking: bool,
    pub copy_full_response: bool,
    pub status_line: Option<coco_config::StatusLineSettings>,
    pub native_replay_cache: HistoryReplayCachePolicy,
    pub max_reflow_rows: MaxReflowRows,
    /// Whether time-varying UI may animate. Defaults to `Animated`.
    pub motion: MotionMode,
    /// Segments of the terminal window/tab title, in order. Empty leaves the
    /// terminal's own title untouched.
    pub terminal_title: Vec<TerminalTitleItem>,
    /// Whether the startup header shows a rotating usage tip.
    pub tips: bool,
    pub performance: TuiPerformanceConfig,
    overriding_sources: BTreeMap<SettingId, SettingSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiPerformanceConfig {
    pub frame_enabled: bool,
    pub frame_sample_every_n_frames: u64,
    pub frame_slow_threshold_ms: u64,
    pub frame_stage_slow_threshold_us: u64,
    pub memory_enabled: bool,
    pub memory_sample_interval_secs: u64,
    pub memory_delta_threshold_bytes: u64,
    pub heap_profile_enabled: bool,
}

impl Default for TuiPerformanceConfig {
    fn default() -> Self {
        performance_config(TuiPerformanceSettings::default())
    }
}

impl DisplaySettings {
    pub fn from_settings(settings: &coco_config::Settings) -> Self {
        Self {
            syntax_highlighting: syntax_highlighting_from_level(settings.syntax_highlighting),
            show_thinking: settings.show_thinking,
            copy_full_response: settings.copy_full_response,
            status_line: settings.status_line.clone(),
            native_replay_cache: replay_cache_policy(settings.tui.native_replay_cache),
            max_reflow_rows: max_reflow_rows(settings.tui.reflow_max_rows),
            motion: MotionMode::from_animations_enabled(settings.tui.animations),
            terminal_title: settings.tui.terminal_title.clone(),
            tips: settings.tui.tips,
            performance: performance_config(settings.tui.performance),
            overriding_sources: BTreeMap::new(),
        }
    }

    pub fn from_settings_with_sources(settings: &SettingsWithSource) -> Self {
        Self {
            syntax_highlighting: syntax_highlighting_from_level(
                settings.merged.syntax_highlighting,
            ),
            show_thinking: settings.merged.show_thinking,
            copy_full_response: settings.merged.copy_full_response,
            status_line: settings.merged.status_line.clone(),
            native_replay_cache: replay_cache_policy(settings.merged.tui.native_replay_cache),
            max_reflow_rows: max_reflow_rows(settings.merged.tui.reflow_max_rows),
            motion: MotionMode::from_animations_enabled(settings.merged.tui.animations),
            terminal_title: settings.merged.tui.terminal_title.clone(),
            tips: settings.merged.tui.tips,
            performance: performance_config(settings.merged.tui.performance),
            overriding_sources: overriding_sources(settings),
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

    pub(crate) fn overriding_source(&self, id: SettingId) -> Option<SettingSource> {
        self.overriding_sources.get(&id).copied()
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

fn max_reflow_rows(setting: ReflowMaxRows) -> MaxReflowRows {
    resolve_max_reflow_rows(setting, coco_tui_ui::terminal_detect::terminal_info().name)
}

fn performance_config(settings: TuiPerformanceSettings) -> TuiPerformanceConfig {
    TuiPerformanceConfig {
        frame_enabled: settings.frame_enabled,
        frame_sample_every_n_frames: settings.frame_sample_every_n_frames,
        frame_slow_threshold_ms: settings.frame_slow_threshold_ms,
        frame_stage_slow_threshold_us: settings.frame_stage_slow_threshold_us,
        memory_enabled: settings.memory_enabled,
        memory_sample_interval_secs: settings.memory_sample_interval_secs,
        memory_delta_threshold_bytes: mib_to_bytes(settings.memory_delta_threshold_mb),
        heap_profile_enabled: settings.heap_profile_enabled,
    }
}

fn kib_to_bytes(kib: usize) -> usize {
    kib.saturating_mul(1024)
}

fn mib_to_bytes(mib: u64) -> u64 {
    mib.saturating_mul(1024 * 1024)
}

/// Map the persisted config tier onto the render-time tier. The two enums are
/// deliberately separate: `coco-config` (Common layer) must not depend on the
/// TUI presentational crate, and `coco-tui-ui` must not depend on config.
pub(crate) fn syntax_highlighting_from_level(level: SyntaxHighlightingLevel) -> SyntaxHighlighting {
    match level {
        SyntaxHighlightingLevel::Off => SyntaxHighlighting::Off,
        SyntaxHighlightingLevel::Lite => SyntaxHighlighting::Lite,
        SyntaxHighlightingLevel::Full => SyntaxHighlighting::Full,
    }
}

/// Inverse of [`syntax_highlighting_from_level`], for persisting a runtime
/// toggle back into `settings.json`.
pub(crate) fn syntax_highlighting_to_level(syntax: SyntaxHighlighting) -> SyntaxHighlightingLevel {
    match syntax {
        SyntaxHighlighting::Off => SyntaxHighlightingLevel::Off,
        SyntaxHighlighting::Lite => SyntaxHighlightingLevel::Lite,
        SyntaxHighlighting::Full => SyntaxHighlightingLevel::Full,
    }
}

fn overriding_sources(settings: &SettingsWithSource) -> BTreeMap<SettingId, SettingSource> {
    registered_settings()
        .iter()
        .map(|meta| meta.id)
        .filter_map(|id| {
            settings
                .per_source
                .iter()
                .filter_map(|(source, value)| {
                    (*source > SettingSource::User
                        && id
                            .source_keys()
                            .iter()
                            .any(|key| value_contains_dotted_key(value, key)))
                    .then_some(*source)
                })
                .max()
                .map(|source| (id, source))
        })
        .collect()
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
