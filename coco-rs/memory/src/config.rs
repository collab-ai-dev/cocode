//! Thin runtime adapter over `coco_config::MemoryConfig`.
//!
//! The settings + env resolution layer lives in `coco-config`. This
//! struct is field-for-field identical and exists only so memory-crate
//! consumers can take an owned `MemoryConfig` without depending on the
//! config crate just for the type alias. It re-borrows from the shared
//! source of truth — never grow new fields here.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConfig {
    pub directory: Option<PathBuf>,
    /// Memory base directory override (replaces `<config_home>` in the
    /// default per-project layout). See `coco_config::MemoryConfig`.
    pub memory_base_override: Option<PathBuf>,
    pub skip_index: bool,
    pub kairos_mode: bool,

    pub extraction_enabled: bool,
    pub extraction_throttle: i32,
    pub extraction_max_turns: i32,

    pub team_memory_enabled: bool,

    pub dream_enabled: bool,
    pub dream_min_hours: i32,
    pub dream_min_sessions: i32,

    pub session_memory_enabled: bool,
    pub session_memory_init_tokens: i64,
    pub session_memory_update_tokens: i64,
    pub session_memory_tool_calls: i32,
    pub session_memory_per_section_tokens: i64,
    pub session_memory_total_tokens: i64,

    pub searching_past_context_enabled: bool,

    /// Free-form policy text injected verbatim into the auto-memory
    /// system-prompt block. `None` ⇒ no extra section. Source of
    /// truth: `coco_config::MemoryConfig::extra_guidelines`.
    pub extra_guidelines: Option<String>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        coco_config::MemoryConfig::default().into()
    }
}

impl From<coco_config::MemoryConfig> for MemoryConfig {
    fn from(c: coco_config::MemoryConfig) -> Self {
        Self {
            directory: c.directory,
            memory_base_override: c.memory_base_override,
            skip_index: c.skip_index,
            kairos_mode: c.kairos_mode,
            extraction_enabled: c.extraction_enabled,
            extraction_throttle: c.extraction_throttle,
            extraction_max_turns: c.extraction_max_turns,
            team_memory_enabled: c.team_memory_enabled,
            dream_enabled: c.dream_enabled,
            dream_min_hours: c.dream_min_hours,
            dream_min_sessions: c.dream_min_sessions,
            session_memory_enabled: c.session_memory_enabled,
            session_memory_init_tokens: c.session_memory_init_tokens,
            session_memory_update_tokens: c.session_memory_update_tokens,
            session_memory_tool_calls: c.session_memory_tool_calls,
            session_memory_per_section_tokens: c.session_memory_per_section_tokens,
            session_memory_total_tokens: c.session_memory_total_tokens,
            searching_past_context_enabled: c.searching_past_context_enabled,
            extra_guidelines: c.extra_guidelines,
        }
    }
}
