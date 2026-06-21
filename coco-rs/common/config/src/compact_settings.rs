//! Compaction settings layer.
//!
//! Bridges `~/.coco/settings.json` and `COCO_COMPACT_*` env vars into a
//! single `CompactConfig` that `coco_compact` consumes through plain
//! struct references — the crate itself reads no env at runtime.
//!
//! Layering: defaults → `Settings.compact` → env overrides → resolved
//! `CompactConfig`. All env vars are `COCO_*`-prefixed (root
//! `CLAUDE.md` → "Code Hygiene" rule); TS-style `CLAUDE_CODE_*` /
//! unprefixed names are intentionally NOT honored.

use serde::Deserialize;
use serde::Serialize;

use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;

/// Default percentage of context window to trigger HISTORY_SNIP staging.
const DEFAULT_HISTORY_SNIP_AUTO_PCT: f64 = 0.7;
/// Default percentage of context window to stage a context-collapse range.
const DEFAULT_STAGED_COMPACT_STAGE_PCT: f64 = 0.6;
/// Default percentage of context window to commit staged collapse ranges.
const DEFAULT_STAGED_COMPACT_COMMIT_PCT: f64 = 0.85;
/// Default per-message aggregate char cap for Tool Result Budget Level 2.
const DEFAULT_TOOL_RESULT_BUDGET_PER_MESSAGE_CHARS: i64 = 200_000;
/// Default number of recently read files restored after full compaction.
const DEFAULT_POST_COMPACT_MAX_FILES_TO_RESTORE: i32 = 5;

// ── PartialCompactSettings (settings.json shape) ─────────────────────

/// Top-level compact section in `settings.json`. Every field is optional
/// so absent JSON keys preserve defaults; absent sub-sections preserve
/// nested defaults too.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialCompactSettings {
    pub auto: PartialAutoCompactSettings,
    pub micro: PartialMicroCompactSettings,
    pub api_native: PartialApiNativeSettings,
    pub post_compact: PartialPostCompactSettings,
    pub session_memory: PartialSessionMemorySettings,
    pub experimental: PartialExperimentalSettings,
    pub tool_result_budget: PartialToolResultBudgetSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialAutoCompactSettings {
    pub enabled: Option<bool>,
    pub context_window_override: Option<i64>,
    pub pct_override: Option<f64>,
    pub blocking_limit_override: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialMicroCompactSettings {
    pub enabled: Option<bool>,
    pub keep_recent: Option<i32>,
    pub time_based: PartialTimeBasedMcSettings,
    /// Opt-in: count-based clearing of old tool results when the
    /// auto-compact threshold fires or `/compact` runs. TS external builds
    /// don't run this (the `microcompactMessages` call is a no-op outside
    /// `feature('CACHED_MICROCOMPACT')`); default `false` matches that.
    pub count_based_enabled: Option<bool>,
    /// Opt-in: per-turn cleanup of `[file unchanged]` placeholders.
    /// No TS equivalent; default `false`.
    pub clear_file_unchanged_stubs_enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialTimeBasedMcSettings {
    pub enabled: Option<bool>,
    pub gap_threshold_minutes: Option<i32>,
    pub keep_recent: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialApiNativeSettings {
    pub clear_tool_results: Option<bool>,
    pub clear_tool_uses: Option<bool>,
    pub max_input_tokens: Option<i64>,
    pub target_input_tokens: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialPostCompactSettings {
    pub max_files_to_restore: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialSessionMemorySettings {
    pub enabled: Option<bool>,
    pub min_tokens: Option<i64>,
    pub min_text_block_messages: Option<i32>,
    pub max_tokens: Option<i64>,
    pub max_summary_chars: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialExperimentalSettings {
    pub history_snip: PartialHistorySnipSettings,
    pub staged_compact: PartialStagedCompactSettings,
    pub display_collapses: PartialDisplayCollapseSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialHistorySnipSettings {
    pub enabled: Option<bool>,
    pub auto_pct: Option<f64>,
    pub model_invocable: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialStagedCompactSettings {
    pub enabled: Option<bool>,
    pub stage_at_pct: Option<f64>,
    pub commit_at_pct: Option<f64>,
    pub persist_to_transcript: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialDisplayCollapseSettings {
    pub read_search: Option<bool>,
    pub hook_summaries: Option<bool>,
    pub background_bash: Option<bool>,
    pub teammate_shutdowns: Option<bool>,
}

/// Tool Result Budget settings.
///
/// Level 2 enable + per-message char cap. Level 1 (per-tool
/// `<persisted-output>` persistence) is driven by each tool's
/// `max_result_size_bound()` declaration rather than this config. See
/// [`docs/coco-rs/tool-result-budget-plan.md`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialToolResultBudgetSettings {
    pub enabled: Option<bool>,
    pub per_message_chars: Option<i64>,
    pub persist_records: Option<bool>,
}

// ── Resolved CompactConfig (consumed at runtime) ─────────────────────

/// Resolved compaction configuration. Every field has a deterministic
/// default; settings/env overlays patch on top.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CompactConfig {
    pub auto: AutoCompactConfig,
    pub micro: MicroCompactConfig,
    pub api_native: ApiNativeConfig,
    pub post_compact: PostCompactConfig,
    pub session_memory: SessionMemoryConfig,
    pub experimental: ExperimentalConfig,
    pub tool_result_budget: ToolResultBudgetConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoCompactConfig {
    /// User toggle (was TS `globalConfig.autoCompactEnabled`).
    pub enabled: bool,
    /// Hard kill: env `COCO_COMPACT_DISABLE` — both auto and manual.
    pub disabled_by_env: bool,
    /// Soft kill: env `COCO_COMPACT_DISABLE_AUTO` — manual still works.
    pub auto_disabled_by_env: bool,
    /// Optional context-window cap. Env: `COCO_COMPACT_AUTO_WINDOW`.
    pub context_window_override: Option<i64>,
    /// Optional 0-100 percentage override. Env:
    /// `COCO_COMPACT_AUTO_PCT_OVERRIDE`.
    pub pct_override: Option<f64>,
    /// Optional blocking-limit override. Env:
    /// `COCO_COMPACT_BLOCKING_LIMIT`.
    pub blocking_limit_override: Option<i64>,
}

impl Default for AutoCompactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disabled_by_env: false,
            auto_disabled_by_env: false,
            context_window_override: None,
            pct_override: None,
            blocking_limit_override: None,
        }
    }
}

impl AutoCompactConfig {
    /// Single-source predicate: auto-compact runs only when both the user
    /// flag is on and neither kill switch fires.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.enabled && !self.disabled_by_env && !self.auto_disabled_by_env
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicroCompactConfig {
    pub enabled: bool,
    /// How many recent compactable tool_use_ids to retain.
    pub keep_recent: i32,
    pub time_based: TimeBasedMcConfig,
    /// Opt-in: count-based tool-result clearing on autocompact threshold
    /// and `/compact`. Default off — TS external runs no count-based MC
    /// (its `microcompactMessages` is a no-op outside `feature('CACHED_MICROCOMPACT')`).
    pub count_based_enabled: bool,
    /// Opt-in: per-turn `[file unchanged]` stub rewrite. No TS equivalent.
    /// Default off.
    pub clear_file_unchanged_stubs_enabled: bool,
}

impl Default for MicroCompactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_recent: 5,
            time_based: TimeBasedMcConfig::default(),
            count_based_enabled: false,
            clear_file_unchanged_stubs_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeBasedMcConfig {
    pub enabled: bool,
    /// Minutes of inactivity before triggering (TS default 60, matches cache TTL).
    pub gap_threshold_minutes: i32,
    /// Number of recent compactable tool_use_ids to keep when triggered.
    pub keep_recent: i32,
}

impl Default for TimeBasedMcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gap_threshold_minutes: 60,
            keep_recent: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostCompactConfig {
    /// Number of recently read files to restore after full compact.
    /// Env: `COCO_COMPACT_POST_COMPACT_MAX_FILES_TO_RESTORE`.
    pub max_files_to_restore: i32,
}

impl Default for PostCompactConfig {
    fn default() -> Self {
        Self {
            max_files_to_restore: DEFAULT_POST_COMPACT_MAX_FILES_TO_RESTORE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiNativeConfig {
    /// Env: `COCO_COMPACT_API_CLEAR_TOOL_RESULTS`.
    pub clear_tool_results: bool,
    /// Env: `COCO_COMPACT_API_CLEAR_TOOL_USES`.
    pub clear_tool_uses: bool,
    /// Env: `COCO_COMPACT_API_MAX_INPUT_TOKENS`. Server-side trigger
    /// threshold (input tokens).
    pub max_input_tokens: i64,
    /// Env: `COCO_COMPACT_API_TARGET_INPUT_TOKENS`. Keep target after
    /// clearing (input tokens).
    pub target_input_tokens: i64,
}

impl Default for ApiNativeConfig {
    fn default() -> Self {
        Self {
            clear_tool_results: false,
            clear_tool_uses: false,
            max_input_tokens: 180_000,
            target_input_tokens: 40_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMemoryConfig {
    pub enabled: bool,
    pub min_tokens: i64,
    pub min_text_block_messages: i32,
    pub max_tokens: i64,
    pub max_summary_chars: i64,
}

impl Default for SessionMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_tokens: 10_000,
            min_text_block_messages: 5,
            max_tokens: 40_000,
            max_summary_chars: 100_000,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExperimentalConfig {
    pub history_snip: HistorySnipConfig,
    pub staged_compact: StagedCompactConfig,
    pub display_collapses: DisplayCollapseConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistorySnipConfig {
    pub enabled: bool,
    pub auto_pct: f64,
    pub model_invocable: bool,
}

impl Default for HistorySnipConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_pct: DEFAULT_HISTORY_SNIP_AUTO_PCT,
            model_invocable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagedCompactConfig {
    pub enabled: bool,
    pub stage_at_pct: f64,
    pub commit_at_pct: f64,
    pub persist_to_transcript: bool,
}

impl Default for StagedCompactConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stage_at_pct: DEFAULT_STAGED_COMPACT_STAGE_PCT,
            commit_at_pct: DEFAULT_STAGED_COMPACT_COMMIT_PCT,
            persist_to_transcript: false,
        }
    }
}

/// Display-only message folding. Default-on because zero risk; users can
/// disable individual reducers if they want raw transcript scrollback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayCollapseConfig {
    pub read_search: bool,
    pub hook_summaries: bool,
    pub background_bash: bool,
    pub teammate_shutdowns: bool,
}

impl Default for DisplayCollapseConfig {
    fn default() -> Self {
        Self {
            read_search: true,
            hook_summaries: true,
            background_bash: true,
            teammate_shutdowns: true,
        }
    }
}

/// Tool Result Budget config.
///
/// Mapping to TS feature gates:
///
/// | Field | TS gate / GrowthBook | Default |
/// |---|---|---|
/// | `enabled` | `tengu_hawthorn_steeple` (Level 2 enable) | `false` |
/// | `per_message_chars` | `tengu_hawthorn_window` (per-message override) | `200_000` |
/// | `persist_records` | — (transcript record write toggle for fork agents) | `true` |
///
/// Per-tool persistence threshold overrides (TS `tengu_satin_quoll`) belong on
/// `Tool::max_result_size_bound()`; they are intentionally not surfaced as
/// compact config.
///
/// **Status**: config is live for the query-level aggregate budget. Level 1
/// helpers live in `coco-tool-runtime::tool_result_storage` and are called by
/// `coco-query`'s tool outcome builder when a tool opts in via
/// `max_result_size_bound()`. Remaining gaps are tracked in
/// `docs/coco-rs/tool-result-budget-plan.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultBudgetConfig {
    /// Enable Level 2 per-message budget. TS `tengu_hawthorn_steeple`.
    pub enabled: bool,
    /// Per-API-message aggregate char cap. TS
    /// `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS` (200_000); overridable via
    /// TS `tengu_hawthorn_window`.
    pub per_message_chars: i64,
    /// Whether `ContentReplacementRecord`s persist to the session
    /// transcript. Off for ephemeral fork agents that share a parent
    /// transcript.
    pub persist_records: bool,
}

impl Default for ToolResultBudgetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            per_message_chars: DEFAULT_TOOL_RESULT_BUDGET_PER_MESSAGE_CHARS,
            persist_records: true,
        }
    }
}

// ── Resolution: Settings + EnvSnapshot → CompactConfig ───────────────

impl CompactConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        let part = &settings.compact;

        // Auto.
        if let Some(v) = part.auto.enabled {
            config.auto.enabled = v;
        }
        if let Some(v) = part.auto.context_window_override.filter(|v| *v > 0) {
            config.auto.context_window_override = Some(v);
        }
        if let Some(v) = part.auto.pct_override.filter(|p| *p > 0.0 && *p <= 100.0) {
            config.auto.pct_override = Some(v);
        }
        if let Some(v) = part.auto.blocking_limit_override.filter(|v| *v > 0) {
            config.auto.blocking_limit_override = Some(v);
        }
        config.auto.disabled_by_env = env.is_truthy(EnvKey::CocoCompactDisable);
        config.auto.auto_disabled_by_env = env.is_truthy(EnvKey::CocoCompactDisableAuto);
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactAutoWindow)
            .filter(|v| *v > 0)
        {
            config.auto.context_window_override = Some(v);
        }
        if let Some(v) = env
            .get(EnvKey::CocoCompactAutoPctOverride)
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|p| *p > 0.0 && *p <= 100.0)
        {
            config.auto.pct_override = Some(v);
        }
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactBlockingLimit)
            .filter(|v| *v > 0)
        {
            config.auto.blocking_limit_override = Some(v);
        }

        // Micro.
        if let Some(v) = part.micro.enabled {
            config.micro.enabled = v;
        }
        if let Some(v) = part.micro.keep_recent {
            config.micro.keep_recent = v.max(0);
        }
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactMicroKeepRecent)
            .and_then(|v| i32::try_from(v).ok())
        {
            config.micro.keep_recent = v.max(0);
        }
        if let Some(v) = part.micro.time_based.enabled {
            config.micro.time_based.enabled = v;
        }
        if let Some(v) = part.micro.time_based.gap_threshold_minutes {
            config.micro.time_based.gap_threshold_minutes = v.max(1);
        }
        if let Some(v) = part.micro.time_based.keep_recent {
            config.micro.time_based.keep_recent = v.max(0);
        }
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactMicroTimeBasedKeepRecent)
            .and_then(|v| i32::try_from(v).ok())
        {
            config.micro.time_based.keep_recent = v.max(0);
        }
        if let Some(v) = part.micro.count_based_enabled {
            config.micro.count_based_enabled = v;
        }
        if let Some(v) = part.micro.clear_file_unchanged_stubs_enabled {
            config.micro.clear_file_unchanged_stubs_enabled = v;
        }

        // Post-compact restore.
        if let Some(v) = part.post_compact.max_files_to_restore {
            config.post_compact.max_files_to_restore = v.max(0);
        }
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactPostCompactMaxFilesToRestore)
            .and_then(|v| i32::try_from(v).ok())
        {
            config.post_compact.max_files_to_restore = v.max(0);
        }

        // API-native.
        if let Some(v) = part.api_native.clear_tool_results {
            config.api_native.clear_tool_results = v;
        }
        if let Some(v) = part.api_native.clear_tool_uses {
            config.api_native.clear_tool_uses = v;
        }
        if let Some(v) = part.api_native.max_input_tokens {
            config.api_native.max_input_tokens = v.max(0);
        }
        if let Some(v) = part.api_native.target_input_tokens {
            config.api_native.target_input_tokens = v.max(0);
        }
        if env.is_truthy(EnvKey::CocoCompactApiClearToolResults) {
            config.api_native.clear_tool_results = true;
        }
        if env.is_truthy(EnvKey::CocoCompactApiClearToolUses) {
            config.api_native.clear_tool_uses = true;
        }
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactApiMaxInputTokens)
            .filter(|v| *v > 0)
        {
            config.api_native.max_input_tokens = v;
        }
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactApiTargetInputTokens)
            .filter(|v| *v > 0)
        {
            config.api_native.target_input_tokens = v;
        }

        // Session memory.
        if let Some(v) = part.session_memory.enabled {
            config.session_memory.enabled = v;
        }
        if let Some(v) = part.session_memory.min_tokens {
            config.session_memory.min_tokens = v.max(0);
        }
        if let Some(v) = part.session_memory.min_text_block_messages {
            config.session_memory.min_text_block_messages = v.max(0);
        }
        if let Some(v) = part.session_memory.max_tokens {
            config.session_memory.max_tokens = v.max(config.session_memory.min_tokens);
        }
        if let Some(v) = part.session_memory.max_summary_chars {
            config.session_memory.max_summary_chars = v.max(0);
        }
        if env.is_truthy(EnvKey::CocoCompactSessionMemoryEnable) {
            config.session_memory.enabled = true;
        }
        if env.is_truthy(EnvKey::CocoCompactSessionMemoryDisable) {
            config.session_memory.enabled = false;
        }

        // Experimental — history snip.
        let snip = &part.experimental.history_snip;
        if let Some(v) = snip.enabled {
            config.experimental.history_snip.enabled = v;
        }
        if let Some(v) = snip.auto_pct.filter(|p| *p > 0.0 && *p <= 1.0) {
            config.experimental.history_snip.auto_pct = v;
        }
        if let Some(v) = snip.model_invocable {
            config.experimental.history_snip.model_invocable = v;
        }

        // Experimental — staged compact.
        let staged = &part.experimental.staged_compact;
        if let Some(v) = staged.enabled {
            config.experimental.staged_compact.enabled = v;
        }
        if let Some(v) = staged.stage_at_pct.filter(|p| *p > 0.0 && *p <= 1.0) {
            config.experimental.staged_compact.stage_at_pct = v;
        }
        if let Some(v) = staged.commit_at_pct.filter(|p| *p > 0.0 && *p <= 1.0) {
            config.experimental.staged_compact.commit_at_pct = v;
        }
        if let Some(v) = staged.persist_to_transcript {
            config.experimental.staged_compact.persist_to_transcript = v;
        }

        // Experimental — display collapses.
        let dc = &part.experimental.display_collapses;
        if let Some(v) = dc.read_search {
            config.experimental.display_collapses.read_search = v;
        }
        if let Some(v) = dc.hook_summaries {
            config.experimental.display_collapses.hook_summaries = v;
        }
        if let Some(v) = dc.background_bash {
            config.experimental.display_collapses.background_bash = v;
        }
        if let Some(v) = dc.teammate_shutdowns {
            config.experimental.display_collapses.teammate_shutdowns = v;
        }

        // Tool Result Budget.
        let trb = &part.tool_result_budget;
        if let Some(v) = trb.enabled {
            config.tool_result_budget.enabled = v;
        }
        if let Some(v) = trb.per_message_chars.filter(|v| *v > 0) {
            config.tool_result_budget.per_message_chars = v;
        }
        if let Some(v) = trb.persist_records {
            config.tool_result_budget.persist_records = v;
        }
        if env.is_truthy(EnvKey::CocoCompactToolResultBudgetEnable) {
            config.tool_result_budget.enabled = true;
        }
        if let Some(v) = env
            .get_i64(EnvKey::CocoCompactToolResultBudgetPerMessageChars)
            .filter(|v| *v > 0)
        {
            config.tool_result_budget.per_message_chars = v;
        }

        config.finalize();
        config
    }

    fn finalize(&mut self) {
        // Cross-field invariants for staged compact.
        let exp = &mut self.experimental.staged_compact;
        if exp.commit_at_pct < exp.stage_at_pct {
            exp.commit_at_pct = exp.stage_at_pct;
        }
    }
}

#[cfg(test)]
#[path = "compact_settings.test.rs"]
mod tests;
