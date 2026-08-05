//! Metadata for settings exposed by the TUI settings browser.
//!
//! The registry describes discoverability and edit routing only. Runtime values
//! stay in [`crate::display_settings::DisplaySettings`], and mutations stay in
//! the update layer. Keeping those concerns separate makes the list searchable
//! without turning setting keys into an untyped dispatch API.

use crate::i18n::t;

/// Stable identity for a setting shown in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum SettingId {
    Theme,
    SyntaxHighlighting,
    ShowThinking,
    CopyFullResponse,
    Animations,
    TerminalTitle,
    Tips,
    ReflowMaxRows,
    StatusLine,
    NativeReplayCache,
    FrameTelemetry,
    MemoryTelemetry,
}

impl SettingId {
    pub(crate) const ALL: [Self; 12] = [
        Self::Theme,
        Self::SyntaxHighlighting,
        Self::ShowThinking,
        Self::CopyFullResponse,
        Self::Animations,
        Self::TerminalTitle,
        Self::Tips,
        Self::ReflowMaxRows,
        Self::StatusLine,
        Self::NativeReplayCache,
        Self::FrameTelemetry,
        Self::MemoryTelemetry,
    ];

    /// Canonical `settings.json` key (or command-owned surface for Theme).
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::SyntaxHighlighting => "syntax_highlighting",
            Self::ShowThinking => "show_thinking",
            Self::CopyFullResponse => "copy_full_response",
            Self::Animations => "tui.animations",
            Self::TerminalTitle => "tui.terminal_title",
            Self::Tips => "tui.tips",
            Self::ReflowMaxRows => "tui.reflow_max_rows",
            Self::StatusLine => "statusLine",
            Self::NativeReplayCache => "tui.native_replay_cache.enabled",
            Self::FrameTelemetry => "tui.performance.frame_enabled",
            Self::MemoryTelemetry => "tui.performance.memory_enabled",
        }
    }

    /// Raw paths accepted while resolving per-source ownership. The status
    /// line keeps its documented snake_case alias, while `key()` remains the
    /// canonical on-disk spelling shown to users.
    pub(crate) const fn source_keys(self) -> &'static [&'static str] {
        match self {
            Self::Theme => &[],
            Self::SyntaxHighlighting => &["syntax_highlighting"],
            Self::ShowThinking => &["show_thinking"],
            Self::CopyFullResponse => &["copy_full_response"],
            Self::Animations => &["tui.animations"],
            Self::TerminalTitle => &["tui.terminal_title"],
            Self::Tips => &["tui.tips"],
            Self::ReflowMaxRows => &["tui.reflow_max_rows"],
            Self::StatusLine => &["statusLine", "status_line"],
            Self::NativeReplayCache => &["tui.native_replay_cache.enabled"],
            Self::FrameTelemetry => &["tui.performance.frame_enabled"],
            Self::MemoryTelemetry => &["tui.performance.memory_enabled"],
        }
    }

    pub(crate) fn label(self) -> String {
        match self {
            Self::Theme => t!("dialog.settings_label_theme").to_string(),
            Self::SyntaxHighlighting => t!("dialog.settings_label_syntax_highlighting").to_string(),
            Self::ShowThinking => t!("dialog.settings_label_show_thinking").to_string(),
            Self::CopyFullResponse => t!("dialog.settings_label_copy_full_response").to_string(),
            Self::Animations => t!("dialog.settings_label_animations").to_string(),
            Self::TerminalTitle => t!("dialog.settings_label_terminal_title").to_string(),
            Self::Tips => t!("dialog.settings_label_tips").to_string(),
            Self::ReflowMaxRows => t!("dialog.settings_label_reflow_max_rows").to_string(),
            Self::StatusLine => t!("dialog.settings_label_status_line").to_string(),
            Self::NativeReplayCache => t!("dialog.settings_label_native_replay_cache").to_string(),
            Self::FrameTelemetry => t!("dialog.settings_label_frame_telemetry").to_string(),
            Self::MemoryTelemetry => t!("dialog.settings_label_memory_telemetry").to_string(),
        }
    }

    pub(crate) fn description(self) -> String {
        match self {
            Self::Theme => t!("dialog.settings_desc_theme").to_string(),
            Self::SyntaxHighlighting => t!("dialog.settings_desc_syntax_highlighting").to_string(),
            Self::ShowThinking => t!("dialog.settings_desc_show_thinking").to_string(),
            Self::CopyFullResponse => t!("dialog.settings_desc_copy_full_response").to_string(),
            Self::Animations => t!("dialog.settings_desc_animations").to_string(),
            Self::TerminalTitle => t!("dialog.settings_desc_terminal_title").to_string(),
            Self::Tips => t!("dialog.settings_desc_tips").to_string(),
            Self::ReflowMaxRows => t!("dialog.settings_desc_reflow_max_rows").to_string(),
            Self::StatusLine => t!("dialog.settings_desc_status_line").to_string(),
            Self::NativeReplayCache => t!("dialog.settings_desc_native_replay_cache").to_string(),
            Self::FrameTelemetry => t!("dialog.settings_desc_frame_telemetry").to_string(),
            Self::MemoryTelemetry => t!("dialog.settings_desc_memory_telemetry").to_string(),
        }
    }
}

/// Browser grouping. The renderer keeps the list flat and prefixes each row
/// with this category so filtering never creates orphaned header rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingCategory {
    Appearance,
    Terminal,
    Performance,
}

impl SettingCategory {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Appearance => t!("dialog.settings_category_appearance").to_string(),
            Self::Terminal => t!("dialog.settings_category_terminal").to_string(),
            Self::Performance => t!("dialog.settings_category_performance").to_string(),
        }
    }
}

/// Value shape. Editors can be added per kind without embedding function
/// pointers or config-layer dependencies in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingKind {
    Boolean,
    Choice,
    Sequence,
    Command,
}

impl SettingKind {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Boolean => t!("dialog.settings_kind_boolean").to_string(),
            Self::Choice => t!("dialog.settings_kind_choice").to_string(),
            Self::Sequence => t!("dialog.settings_kind_sequence").to_string(),
            Self::Command => t!("dialog.settings_kind_command").to_string(),
        }
    }
}

/// Update-layer action available for a row today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingAction {
    OpenThemePicker,
    CycleSyntaxHighlighting,
    ToggleCopyFullResponse,
    ReadOnly,
}

/// One declarative registry row.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingMeta {
    pub(crate) id: SettingId,
    pub(crate) category: SettingCategory,
    pub(crate) kind: SettingKind,
    pub(crate) action: SettingAction,
    /// Lowercase, locale-neutral aliases used in addition to the localized
    /// label, description, category, and canonical key.
    pub(crate) keywords: &'static [&'static str],
}

const SETTINGS: [SettingMeta; SettingId::ALL.len()] = [
    SettingMeta {
        id: SettingId::Theme,
        category: SettingCategory::Appearance,
        kind: SettingKind::Choice,
        action: SettingAction::OpenThemePicker,
        keywords: &["theme", "color", "palette"],
    },
    SettingMeta {
        id: SettingId::SyntaxHighlighting,
        category: SettingCategory::Appearance,
        kind: SettingKind::Choice,
        action: SettingAction::CycleSyntaxHighlighting,
        keywords: &["syntax", "highlight", "code"],
    },
    SettingMeta {
        id: SettingId::ShowThinking,
        category: SettingCategory::Appearance,
        kind: SettingKind::Boolean,
        action: SettingAction::ReadOnly,
        keywords: &["thinking", "reasoning"],
    },
    SettingMeta {
        id: SettingId::CopyFullResponse,
        category: SettingCategory::Appearance,
        kind: SettingKind::Boolean,
        action: SettingAction::ToggleCopyFullResponse,
        keywords: &["copy", "response", "clipboard"],
    },
    SettingMeta {
        id: SettingId::Animations,
        category: SettingCategory::Terminal,
        kind: SettingKind::Boolean,
        action: SettingAction::ReadOnly,
        keywords: &["animation", "motion", "accessibility"],
    },
    SettingMeta {
        id: SettingId::TerminalTitle,
        category: SettingCategory::Terminal,
        kind: SettingKind::Sequence,
        action: SettingAction::ReadOnly,
        keywords: &["terminal", "title", "tab"],
    },
    SettingMeta {
        id: SettingId::Tips,
        category: SettingCategory::Terminal,
        kind: SettingKind::Boolean,
        action: SettingAction::ReadOnly,
        keywords: &["tips", "hint"],
    },
    SettingMeta {
        id: SettingId::ReflowMaxRows,
        category: SettingCategory::Terminal,
        kind: SettingKind::Choice,
        action: SettingAction::ReadOnly,
        keywords: &["reflow", "resize", "scrollback"],
    },
    SettingMeta {
        id: SettingId::StatusLine,
        category: SettingCategory::Terminal,
        kind: SettingKind::Command,
        action: SettingAction::ReadOnly,
        keywords: &["status", "line", "command"],
    },
    SettingMeta {
        id: SettingId::NativeReplayCache,
        category: SettingCategory::Performance,
        kind: SettingKind::Boolean,
        action: SettingAction::ReadOnly,
        keywords: &["cache", "replay", "history"],
    },
    SettingMeta {
        id: SettingId::FrameTelemetry,
        category: SettingCategory::Performance,
        kind: SettingKind::Boolean,
        action: SettingAction::ReadOnly,
        keywords: &["frame", "telemetry", "performance"],
    },
    SettingMeta {
        id: SettingId::MemoryTelemetry,
        category: SettingCategory::Performance,
        kind: SettingKind::Boolean,
        action: SettingAction::ReadOnly,
        keywords: &["memory", "telemetry", "performance"],
    },
];

pub(crate) fn settings() -> &'static [SettingMeta; SettingId::ALL.len()] {
    &SETTINGS
}

pub(crate) fn matches(meta: &SettingMeta, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    let haystack = format!(
        "{} {} {} {} {}",
        meta.category.label(),
        meta.id.label(),
        meta.id.description(),
        meta.id.key(),
        meta.keywords.join(" ")
    )
    .to_lowercase();
    query.split_whitespace().all(|term| haystack.contains(term))
}

#[cfg(test)]
#[path = "settings_registry.test.rs"]
mod tests;
