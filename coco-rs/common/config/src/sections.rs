use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use coco_types::ModelRole;
use coco_types::PermissionMode;
use coco_types::ProviderModelSelection;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;
use crate::settings::SettingsWithSource;
use crate::settings::source::SettingSource;

const DEFAULT_MAX_TOOL_CONCURRENCY: i32 = 10;
const DEFAULT_MAX_RESULT_SIZE: i32 = 400_000;
const DEFAULT_RESULT_PREVIEW_SIZE: i32 = 2_000;
const DEFAULT_BASH_TIMEOUT_MS: i64 = 120_000;
const DEFAULT_BASH_MAX_TIMEOUT_MS: i64 = 600_000;
/// Bash output RETAIN cap (memory bound). Output up to this many bytes is
/// captured and — when it exceeds the tool's 30K persistence threshold —
/// windowed inline + persisted whole to disk for recovery. Larger than the
/// old head-only 30K truncation because the offload seam keeps the complete
/// output; the inline window itself stays ~30K (`BashTool::inline_window_budget`).
const DEFAULT_BASH_MAX_OUTPUT_BYTES: i64 = 2_000_000;
/// Upper cap on the Bash retain budget — larger configured values are clamped
/// down at `finalize()` time.
pub(crate) const BASH_MAX_OUTPUT_BYTES_UPPER: i64 = 10_000_000;
const DEFAULT_GLOB_TIMEOUT_SECONDS: i32 = 10;
/// Grep content-mode default per-file match cap before a `+N more in <file>`
/// marker (0 = unlimited). Overridable per call via the Grep `per_file_limit`
/// input; this is the fallback default.
const DEFAULT_GREP_PER_FILE_LIMIT: i32 = 25;
/// Default cap on paths a single Glob call returns before truncation.
const DEFAULT_GLOB_MAX_RESULTS: i32 = 100;
/// Glob output is directory-grouped only when it returns at least this many
/// paths spanning at least [`DEFAULT_GLOB_GROUP_MIN_DIRS`] directories; below
/// either threshold, flat output is more compact.
const DEFAULT_GLOB_GROUP_MIN_PATHS: i32 = 25;
const DEFAULT_GLOB_GROUP_MIN_DIRS: i32 = 3;
// DEFAULT_MAX_RETRIES = 10, base delay 500ms.
const DEFAULT_MAX_RETRIES: i32 = 10;
const MAX_RETRIES_CAP: i32 = 15;
const DEFAULT_RETRY_BASE_DELAY_MS: i64 = 500;
// #134: `getRetryDelay` maxDelayMs default is 32000.
const DEFAULT_RETRY_MAX_DELAY_MS: i64 = 32_000;
const DEFAULT_RETRY_JITTER: f64 = 0.25;
/// 60-second HTTP fetch timeout
/// `FETCH_TIMEOUT_MS = 60_000`. Long enough for slow origins, short
/// enough that the model doesn't stall forever on a stuck fetch.
const DEFAULT_WEB_FETCH_TIMEOUT_SECS: i64 = 60;
const DEFAULT_SERVER_MAX_SESSIONS: i64 = 32;
const DEFAULT_SERVER_MAX_SURFACES_PER_CONNECTION: i64 = 8;
const DEFAULT_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION: i64 = 16;
const DEFAULT_SERVER_EVENT_RETENTION_PER_SESSION: i64 = 1024;
const DEFAULT_SERVER_OUTBOUND_QUEUE_FRAMES: i64 = 1024;
const DEFAULT_SERVER_TURN_DRAIN_TIMEOUT_SECS: i64 = 10;
const DEFAULT_SERVER_SHUTDOWN_TIMEOUT_SECS: i64 = 30;
const DEFAULT_SERVER_PROJECT_SERVICES_IDLE_TTL_SECS: i64 = 3600;
/// Retained full-text cap: the fetched page is truncated to this many bytes
/// before windowing + persisting, so the persisted artifact (and every footer
/// number derived from it) is bounded. Larger than the old 100K side-query
/// budget because the windowed path stores the whole retained copy for
/// recovery, not a lossy summary. The HTTP-level 10MB hard cap is separate.
const DEFAULT_WEB_FETCH_MAX_CONTENT_LENGTH: i64 = 2_000_000;
/// Inline byte budget for a windowed fetch: content at or below this is
/// returned verbatim (zero side-query, zero persistence); over it gets a
/// head+tail window + recoverable pointer.
const DEFAULT_WEB_FETCH_INLINE_BYTE_BUDGET: i64 = 15_000;
/// Larger verbatim window for preapproved documentation hosts serving clean
/// markdown — the flagship docs-reading case must not regress to a 15K window.
const DEFAULT_WEB_FETCH_PREAPPROVED_VERBATIM_BUDGET: i64 = 100_000;
static MAX_RETRIES_CLAMP_WARNED: AtomicBool = AtomicBool::new(false);
/// Default user agent — so robots.txt
/// rules targeting Claude-Code's fetcher apply identically to coco-rs.
const DEFAULT_WEB_FETCH_USER_AGENT: &str = "Claude-User (coco-rs; +https://support.anthropic.com/)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TeammateMode {
    Auto,
    Tmux,
    Iterm2,
    #[default]
    InProcess,
}

impl TeammateMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Tmux => "tmux",
            Self::Iterm2 => "iterm2",
            Self::InProcess => "in-process",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialServerSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_socket_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_bind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_pipe_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sessions: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_surfaces_per_connection: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_passive_surfaces_per_session: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_retention_per_session: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_queue_frames: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_drain_timeout_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutdown_timeout_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_services_idle_ttl_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_session_timeout_secs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub unix_socket_path: Option<String>,
    pub websocket_bind: Option<String>,
    pub named_pipe_name: Option<String>,
    pub max_sessions: i64,
    pub max_surfaces_per_connection: i64,
    pub max_passive_surfaces_per_session: i64,
    pub event_retention_per_session: i64,
    pub outbound_queue_frames: i64,
    pub turn_drain_timeout_secs: i64,
    pub shutdown_timeout_secs: i64,
    /// Evict a cached `ProjectServices` entry with zero attached sessions after
    /// this many seconds (multi-session plan §6.2 / §17).
    pub project_services_idle_ttl_secs: i64,
    /// Optional auto-archive of a session with zero surfaces AND no active or
    /// queued turn after this many seconds. `None` = off (the default);
    /// unattended background work is legitimate (plan §7.6 / §17).
    pub idle_session_timeout_secs: Option<i64>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            unix_socket_path: None,
            websocket_bind: None,
            named_pipe_name: None,
            max_sessions: DEFAULT_SERVER_MAX_SESSIONS,
            max_surfaces_per_connection: DEFAULT_SERVER_MAX_SURFACES_PER_CONNECTION,
            max_passive_surfaces_per_session: DEFAULT_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION,
            event_retention_per_session: DEFAULT_SERVER_EVENT_RETENTION_PER_SESSION,
            outbound_queue_frames: DEFAULT_SERVER_OUTBOUND_QUEUE_FRAMES,
            turn_drain_timeout_secs: DEFAULT_SERVER_TURN_DRAIN_TIMEOUT_SECS,
            shutdown_timeout_secs: DEFAULT_SERVER_SHUTDOWN_TIMEOUT_SECS,
            project_services_idle_ttl_secs: DEFAULT_SERVER_PROJECT_SERVICES_IDLE_TTL_SECS,
            idle_session_timeout_secs: None,
        }
    }
}

impl ServerConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        Self {
            unix_socket_path: env
                .get_string(EnvKey::CocoServerUnixSocketPath)
                .or_else(|| settings.server.unix_socket_path.clone())
                .filter(|path| !path.trim().is_empty()),
            websocket_bind: env
                .get_string(EnvKey::CocoServerWebSocketBind)
                .or_else(|| settings.server.websocket_bind.clone())
                .filter(|addr| !addr.trim().is_empty()),
            named_pipe_name: env
                .get_string(EnvKey::CocoServerNamedPipe)
                .or_else(|| settings.server.named_pipe_name.clone())
                .filter(|name| !name.trim().is_empty()),
            max_sessions: env
                .get_i64(EnvKey::CocoServerMaxSessions)
                .or(settings.server.max_sessions)
                .filter(|count| *count > 0)
                .unwrap_or(DEFAULT_SERVER_MAX_SESSIONS),
            max_surfaces_per_connection: env
                .get_i64(EnvKey::CocoServerMaxSurfacesPerConnection)
                .or(settings.server.max_surfaces_per_connection)
                .filter(|count| *count > 0)
                .unwrap_or(DEFAULT_SERVER_MAX_SURFACES_PER_CONNECTION),
            max_passive_surfaces_per_session: env
                .get_i64(EnvKey::CocoServerMaxPassiveSurfacesPerSession)
                .or(settings.server.max_passive_surfaces_per_session)
                .filter(|count| *count > 0)
                .unwrap_or(DEFAULT_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION),
            event_retention_per_session: env
                .get_i64(EnvKey::CocoServerEventRetentionPerSession)
                .or(settings.server.event_retention_per_session)
                .filter(|count| *count > 0)
                .unwrap_or(DEFAULT_SERVER_EVENT_RETENTION_PER_SESSION),
            outbound_queue_frames: env
                .get_i64(EnvKey::CocoServerOutboundQueueFrames)
                .or(settings.server.outbound_queue_frames)
                .filter(|count| *count > 0)
                .unwrap_or(DEFAULT_SERVER_OUTBOUND_QUEUE_FRAMES),
            turn_drain_timeout_secs: env
                .get_i64(EnvKey::CocoServerTurnDrainTimeoutSecs)
                .or(settings.server.turn_drain_timeout_secs)
                .filter(|secs| *secs > 0)
                .unwrap_or(DEFAULT_SERVER_TURN_DRAIN_TIMEOUT_SECS),
            shutdown_timeout_secs: env
                .get_i64(EnvKey::CocoServerShutdownTimeoutSecs)
                .or(settings.server.shutdown_timeout_secs)
                .filter(|secs| *secs > 0)
                .unwrap_or(DEFAULT_SERVER_SHUTDOWN_TIMEOUT_SECS),
            project_services_idle_ttl_secs: env
                .get_i64(EnvKey::CocoServerProjectServicesIdleTtlSecs)
                .or(settings.server.project_services_idle_ttl_secs)
                .filter(|secs| *secs > 0)
                .unwrap_or(DEFAULT_SERVER_PROJECT_SERVICES_IDLE_TTL_SECS),
            idle_session_timeout_secs: env
                .get_i64(EnvKey::CocoServerIdleSessionTimeoutSecs)
                .or(settings.server.idle_session_timeout_secs)
                .filter(|secs| *secs > 0),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialAgentTeamsSettings {
    pub teammate_mode: Option<TeammateMode>,
    pub default_model_role: Option<ModelRole>,
    pub agent_type_model_roles: Option<HashMap<String, ModelRole>>,
    pub default_model: Option<ProviderModelSelection>,
    pub show_spinner_tree: Option<bool>,
    pub max_agents: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTeamsConfig {
    pub teammate_mode: TeammateMode,
    pub default_model_role: ModelRole,
    pub agent_type_model_roles: HashMap<String, ModelRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<ProviderModelSelection>,
    pub show_spinner_tree: bool,
    pub max_agents: i32,
}

impl Default for AgentTeamsConfig {
    fn default() -> Self {
        Self {
            teammate_mode: TeammateMode::InProcess,
            default_model_role: ModelRole::Main,
            agent_type_model_roles: HashMap::new(),
            default_model: None,
            show_spinner_tree: true,
            max_agents: 8,
        }
    }
}

impl AgentTeamsConfig {
    pub fn resolve(settings: &Settings) -> crate::Result<Self> {
        let mut config = Self::default();
        let section = &settings.agent_teams;
        if let Some(mode) = section.teammate_mode {
            config.teammate_mode = mode;
        }
        if let Some(role) = section.default_model_role {
            config.default_model_role = role;
        }
        if let Some(roles) = &section.agent_type_model_roles {
            config.agent_type_model_roles = roles.clone();
        }
        if let Some(model) = &section.default_model {
            config.default_model = Some(model.clone());
        }
        if let Some(show) = section.show_spinner_tree {
            config.show_spinner_tree = show;
        }
        if let Some(max_agents) = section.max_agents {
            config.max_agents = max_agents.max(1);
        }
        Ok(config)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialToolSettings {
    pub max_tool_concurrency: Option<i32>,
    pub max_result_size: Option<i32>,
    pub result_preview_size: Option<i32>,
    pub enable_result_persistence: Option<bool>,
    pub glob_timeout_seconds: Option<i32>,
    pub file_read_ignore_patterns: Option<Vec<String>>,
    pub bash: Option<PartialBashSettings>,
    pub search: Option<PartialSearchSettings>,
}

/// settings.json `tool.search` section — Grep/Glob output-formatting knobs
/// (native rtk absorptions, design §2.3/§2.4). Independent of `Feature::OutputRewrite`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialSearchSettings {
    pub grep_per_file_limit: Option<i32>,
    pub glob_max_results: Option<i32>,
    pub glob_group_min_paths: Option<i32>,
    pub glob_group_min_dirs: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialBashSettings {
    pub default_timeout_ms: Option<i64>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub auto_background_on_timeout: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfig {
    pub max_tool_concurrency: i32,
    pub max_result_size: i32,
    pub result_preview_size: i32,
    pub enable_result_persistence: bool,
    pub glob_timeout_seconds: i32,
    pub file_read_ignore_patterns: Vec<String>,
    pub bash: BashConfig,
    /// Grep/Glob output-formatting knobs (§2.3/§2.4).
    pub search: SearchFormatConfig,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            max_tool_concurrency: DEFAULT_MAX_TOOL_CONCURRENCY,
            max_result_size: DEFAULT_MAX_RESULT_SIZE,
            result_preview_size: DEFAULT_RESULT_PREVIEW_SIZE,
            enable_result_persistence: true,
            glob_timeout_seconds: DEFAULT_GLOB_TIMEOUT_SECONDS,
            file_read_ignore_patterns: Vec::new(),
            bash: BashConfig::default(),
            search: SearchFormatConfig::default(),
        }
    }
}

impl ToolConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        let tool = &settings.tool;

        if let Some(v) = tool.max_tool_concurrency {
            config.max_tool_concurrency = v;
        }
        if let Some(v) = tool.max_result_size {
            config.max_result_size = v;
        }
        if let Some(v) = tool.result_preview_size {
            config.result_preview_size = v;
        }
        if let Some(v) = tool.enable_result_persistence {
            config.enable_result_persistence = v;
        }
        if let Some(v) = tool.glob_timeout_seconds {
            config.glob_timeout_seconds = v;
        }
        if let Some(patterns) = &tool.file_read_ignore_patterns {
            config.file_read_ignore_patterns.clone_from(patterns);
        }
        if let Some(bash) = &tool.bash {
            config.bash.apply_settings(bash);
        }
        if let Some(search) = &tool.search {
            config.search.apply_settings(search);
        }

        if let Some(v) = env.get_i32(EnvKey::CocoMaxToolUseConcurrency) {
            config.max_tool_concurrency = v;
        }
        if let Some(v) = env.get_i32(EnvKey::CocoGlobTimeoutSeconds) {
            config.glob_timeout_seconds = v;
        }
        if let Some(v) = env.get_i32(EnvKey::CocoGrepPerFileLimit) {
            config.search.grep_per_file_limit = v;
        }
        if let Some(v) = env.get_i32(EnvKey::CocoGlobMaxResults) {
            config.search.glob_max_results = v;
        }
        if let Some(v) = env.get_i32(EnvKey::CocoGlobGroupMinPaths) {
            config.search.glob_group_min_paths = v;
        }
        if let Some(v) = env.get_i32(EnvKey::CocoGlobGroupMinDirs) {
            config.search.glob_group_min_dirs = v;
        }
        if let Some(raw) = env.get(EnvKey::CocoFileReadIgnorePatterns) {
            config.file_read_ignore_patterns = raw
                .split([':', ','])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        if env.is_truthy(EnvKey::CocoBashAutoBackgroundOnTimeout) {
            config.bash.auto_background_on_timeout = true;
        } else if env.is_falsy(EnvKey::CocoBashAutoBackgroundOnTimeout) {
            config.bash.auto_background_on_timeout = false;
        }

        config.finalize();
        config
    }

    fn finalize(&mut self) {
        self.max_tool_concurrency = self.max_tool_concurrency.max(1);
        self.max_result_size = self.max_result_size.max(0);
        self.result_preview_size = self.result_preview_size.max(0);
        self.glob_timeout_seconds = self.glob_timeout_seconds.max(1);
        self.bash.finalize();
        self.search.finalize();
    }
}

/// Grep/Glob output-formatting policy (native rtk absorptions, §2.3/§2.4).
/// These are display thresholds, not physics — resolved from settings/env so
/// they can be tuned without a rebuild. Independent of `Feature::OutputRewrite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFormatConfig {
    /// Default per-file match-line cap in Grep content mode before a
    /// `+N more in <file>` marker (0 = unlimited). The Grep `per_file_limit`
    /// input overrides this per call.
    pub grep_per_file_limit: i32,
    /// Max paths a Glob call returns before truncation (`+N more files`).
    pub glob_max_results: i32,
    /// Group Glob output by directory only when it returns ≥ this many paths…
    pub glob_group_min_paths: i32,
    /// …spanning ≥ this many distinct directories. Below either, flat wins.
    pub glob_group_min_dirs: i32,
}

impl Default for SearchFormatConfig {
    fn default() -> Self {
        Self {
            grep_per_file_limit: DEFAULT_GREP_PER_FILE_LIMIT,
            glob_max_results: DEFAULT_GLOB_MAX_RESULTS,
            glob_group_min_paths: DEFAULT_GLOB_GROUP_MIN_PATHS,
            glob_group_min_dirs: DEFAULT_GLOB_GROUP_MIN_DIRS,
        }
    }
}

impl SearchFormatConfig {
    fn apply_settings(&mut self, settings: &PartialSearchSettings) {
        if let Some(v) = settings.grep_per_file_limit {
            self.grep_per_file_limit = v;
        }
        if let Some(v) = settings.glob_max_results {
            self.glob_max_results = v;
        }
        if let Some(v) = settings.glob_group_min_paths {
            self.glob_group_min_paths = v;
        }
        if let Some(v) = settings.glob_group_min_dirs {
            self.glob_group_min_dirs = v;
        }
    }

    fn finalize(&mut self) {
        // 0 is meaningful for the per-file cap (= unlimited); clamp negatives.
        self.grep_per_file_limit = self.grep_per_file_limit.max(0);
        // Result cap + grouping thresholds must be ≥ 1 to be meaningful.
        self.glob_max_results = self.glob_max_results.max(1);
        self.glob_group_min_paths = self.glob_group_min_paths.max(1);
        self.glob_group_min_dirs = self.glob_group_min_dirs.max(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashConfig {
    pub default_timeout_ms: i64,
    pub max_timeout_ms: i64,
    pub max_output_bytes: i64,
    pub auto_background_on_timeout: bool,
}

impl Default for BashConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: DEFAULT_BASH_TIMEOUT_MS,
            max_timeout_ms: DEFAULT_BASH_MAX_TIMEOUT_MS,
            max_output_bytes: DEFAULT_BASH_MAX_OUTPUT_BYTES,
            // `shouldAutoBackground` defaults ON: a foreground command that
            // hits its timeout is moved to the background rather than killed.
            // Set false to restore hard-kill-on-timeout.
            auto_background_on_timeout: true,
        }
    }
}

impl BashConfig {
    fn apply_settings(&mut self, settings: &PartialBashSettings) {
        if let Some(v) = settings.default_timeout_ms {
            self.default_timeout_ms = v;
        }
        if let Some(v) = settings.max_timeout_ms {
            self.max_timeout_ms = v;
        }
        if let Some(v) = settings.max_output_bytes {
            self.max_output_bytes = v;
        }
        if let Some(v) = settings.auto_background_on_timeout {
            self.auto_background_on_timeout = v;
        }
    }

    fn finalize(&mut self) {
        self.default_timeout_ms = self.default_timeout_ms.max(1);
        self.max_timeout_ms = self.max_timeout_ms.max(self.default_timeout_ms);
        self.max_output_bytes = self.max_output_bytes.clamp(0, BASH_MAX_OUTPUT_BYTES_UPPER);
    }
}

// Compaction settings live in `crate::compact_settings`
// (`CompactConfig` and its sub-structs). Per-invocation run-options for
// `compact_conversation` live in `coco_compact::CompactRunOptions`.
// The two are intentionally distinct types: the former is the global
// resolved-from-settings struct; the latter is the per-call parameter
// bag.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialApiSettings {
    pub retry: Option<PartialApiRetrySettings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialApiRetrySettings {
    pub max_retries: Option<i32>,
    pub base_delay_ms: Option<i64>,
    pub max_delay_ms: Option<i64>,
    pub jitter_factor: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiConfig {
    pub retry: ApiRetryConfig,
}

impl ApiConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        let mut max_retries_source = MaxRetriesSource::Default;
        if let Some(retry) = &settings.api.retry {
            config.retry.apply_settings(retry);
            if retry.max_retries.is_some() {
                max_retries_source = MaxRetriesSource::Settings;
            }
        }
        if let Some(v) = env.get_i32(EnvKey::ClaudeCodeMaxRetries)
            && v >= 0
        {
            config.retry.max_retries = v;
            max_retries_source = MaxRetriesSource::ClaudeCodeEnv;
        }
        if let Some(v) = env.get_i32(EnvKey::CocoApiMaxRetries) {
            config.retry.max_retries = v;
            max_retries_source = MaxRetriesSource::CocoEnv;
        }
        config.retry.finalize(max_retries_source);
        config
    }
}

#[derive(Debug, Clone, Copy)]
enum MaxRetriesSource {
    Default,
    Settings,
    ClaudeCodeEnv,
    CocoEnv,
}

impl MaxRetriesSource {
    const fn label(self) -> &'static str {
        match self {
            Self::Default => "default api retry max_retries",
            Self::Settings => "api.retry.max_retries",
            Self::ClaudeCodeEnv => "CLAUDE_CODE_MAX_RETRIES",
            Self::CocoEnv => "COCO_API_MAX_RETRIES",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiRetryConfig {
    pub max_retries: i32,
    pub base_delay_ms: i64,
    pub max_delay_ms: i64,
    pub jitter_factor: f64,
}

impl Default for ApiRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay_ms: DEFAULT_RETRY_BASE_DELAY_MS,
            max_delay_ms: DEFAULT_RETRY_MAX_DELAY_MS,
            jitter_factor: DEFAULT_RETRY_JITTER,
        }
    }
}

impl ApiRetryConfig {
    fn apply_settings(&mut self, settings: &PartialApiRetrySettings) {
        if let Some(v) = settings.max_retries {
            self.max_retries = v;
        }
        if let Some(v) = settings.base_delay_ms {
            self.base_delay_ms = v;
        }
        if let Some(v) = settings.max_delay_ms {
            self.max_delay_ms = v;
        }
        if let Some(v) = settings.jitter_factor {
            self.jitter_factor = v;
        }
    }

    fn finalize(&mut self, max_retries_source: MaxRetriesSource) {
        self.max_retries = self.max_retries.max(0);
        if self.max_retries > MAX_RETRIES_CAP {
            if !MAX_RETRIES_CLAMP_WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    source = max_retries_source.label(),
                    requested = self.max_retries,
                    cap = MAX_RETRIES_CAP,
                    "api retry max_retries clamped to retry cap"
                );
            }
            self.max_retries = MAX_RETRIES_CAP;
        }
        self.base_delay_ms = self.base_delay_ms.max(0);
        self.max_delay_ms = self.max_delay_ms.max(self.base_delay_ms);
        self.jitter_factor = self.jitter_factor.clamp(0.0, 1.0);
    }
}

// `ApiFallbackConfig` previously lived here. Removed — no consumer.
// Stream-fallback and overflow-recovery live inside `app/query::engine`.
// The escalated-max-tokens ceiling is now per-model on
// `ModelInfo.max_output_tokens_escalate`. Recovery cap stays in
// `app/query::config::MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialLoopSettings {
    pub max_turns: Option<i32>,
    /// Session-level token budget. On-disk wire name kept as
    /// `max_tokens` for settings.json compatibility; the field reads
    /// as the total session budget (input + output, accumulated),
    /// matching the renamed `QueryEngineConfig.total_token_budget`.
    #[serde(alias = "total_token_budget", rename = "max_tokens")]
    pub total_token_budget: Option<i32>,
    pub permission_mode: Option<PermissionMode>,
    pub enable_streaming_tools: Option<bool>,
    pub default_prompt_enabled: Option<bool>,
    pub dynamic_enabled: Option<bool>,
    pub persistent_preamble_enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopConfig {
    pub max_turns: Option<i32>,
    /// Session-level token budget. See [`PartialLoopSettings::total_token_budget`].
    #[serde(alias = "total_token_budget", rename = "max_tokens")]
    pub total_token_budget: Option<i32>,
    pub permission_mode: PermissionMode,
    pub enable_streaming_tools: bool,
    pub default_prompt_enabled: bool,
    pub dynamic_enabled: bool,
    pub persistent_preamble_enabled: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            // Unbounded by default — TS only caps turns when `--max-turns`
            // (print mode) or `loop.max_turns` (settings) is explicitly set.
            // The interactive REPL runs until the model stops on its own.
            max_turns: None,
            total_token_budget: None,
            permission_mode: PermissionMode::Default,
            enable_streaming_tools: true,
            default_prompt_enabled: false,
            dynamic_enabled: false,
            persistent_preamble_enabled: false,
        }
    }
}

impl LoopConfig {
    pub fn resolve(
        settings: &Settings,
        overrides: &crate::RuntimeOverrides,
        env: &EnvSnapshot,
    ) -> Self {
        let mut config = Self::default();
        let loop_settings = &settings.loop_config;

        if loop_settings.max_turns.is_some() {
            config.max_turns = loop_settings.max_turns;
        }
        if loop_settings.total_token_budget.is_some() {
            config.total_token_budget = loop_settings.total_token_budget;
        }
        if let Some(mode) = loop_settings.permission_mode {
            config.permission_mode = mode;
        }
        if let Some(v) = loop_settings.enable_streaming_tools {
            config.enable_streaming_tools = v;
        }
        if let Some(v) = loop_settings.default_prompt_enabled {
            config.default_prompt_enabled = v;
        }
        if let Some(v) = loop_settings.dynamic_enabled {
            config.dynamic_enabled = v;
        }
        if let Some(v) = loop_settings.persistent_preamble_enabled {
            config.persistent_preamble_enabled = v;
        }
        if env.is_truthy(EnvKey::CocoLoopPersistent) {
            config.persistent_preamble_enabled = true;
        } else if env.is_falsy(EnvKey::CocoLoopPersistent) {
            config.persistent_preamble_enabled = false;
        }
        if let Some(mode) = overrides.permission_mode_override {
            config.permission_mode = mode;
        }
        config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialShellSettings {
    pub tool: Option<ShellToolSelection>,
    pub default_shell: Option<String>,
    pub disable_snapshot: Option<bool>,
    pub maintain_project_working_dir: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellToolSelection {
    #[default]
    Auto,
    Bash,
    PowerShell,
    Disabled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellConfig {
    pub tool: ShellToolSelection,
    pub default_shell: Option<String>,
    pub disable_snapshot: bool,
    /// When true, snap the bash cwd back to the session's original cwd
    /// after every command — even if the cwd is inside the allowed
    /// working set. Driven by `COCO_BASH_MAINTAIN_PROJECT_WORKING_DIR`.
    pub maintain_project_working_dir: bool,
}

impl ShellConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self {
            tool: settings.shell.tool.unwrap_or_default(),
            default_shell: settings.shell.default_shell.clone(),
            disable_snapshot: settings.shell.disable_snapshot.unwrap_or(false),
            maintain_project_working_dir: settings
                .shell
                .maintain_project_working_dir
                .unwrap_or(false),
        };
        if let Some(shell) = env.get_string(EnvKey::CocoShell) {
            config.default_shell = Some(shell);
        }
        if env.is_truthy(EnvKey::CocoDisableShellSnapshot) {
            config.disable_snapshot = true;
        }
        if env.is_truthy(EnvKey::CocoBashMaintainProjectWorkingDir) {
            config.maintain_project_working_dir = true;
        }
        config
    }
}

/// Which output-rewriter engine backs Bash output compression — selects the
/// concrete `BashOutputRewriter` implementation built at bootstrap. rtk is the
/// only engine today; this enum is the extension point for a second backend
/// (per-engine settings nest under their own key, e.g. `output_rewrite.rtk`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputRewriteEngine {
    /// Rust Token Killer — the external `rtk` binary (and, in phase 2, its
    /// embedded filter core). Configured under `output_rewrite.rtk`.
    #[default]
    Rtk,
}

/// rtk tier-selection policy (design §3.5) — which rtk lifecycle point acts:
/// external is a **pre-spawn rewrite**, builtin is a **post-exec filter**. This
/// is an rtk-engine-internal knob (`output_rewrite.rtk.mode`), distinct from
/// [`OutputRewriteEngine`] which selects the backend. `BashOutputRewriter`
/// projects this to two capability predicates (`does_pre_spawn_rewrite` /
/// `does_post_exec_filter`) so `BashTool` arbitrates without seeing `RtkMode`.
///
/// **v0 scope:** the embedded core exposes only the declarative TOML long-tail;
/// the git / cargo / pytest family formatters are still upstream-coupled, so
/// under `BuiltinFirst` those commands fall through to raw output until either
/// the caller selects an `External*` mode (with a binary on PATH) or the
/// upstream `cmds` decouple lands. There is no degraded-family fallback to
/// arbitrate yet, so `BuiltinFirst` never spawns the binary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtkMode {
    /// Default. Embedded core filters post-exec; no pre-spawn rewrite. Zero
    /// install, works in sandboxes and background tasks.
    #[default]
    BuiltinFirst,
    /// Binary rewrite when available; embedded post-exec filter as the fallback
    /// when the rewrite does not fire.
    ExternalFirst,
    /// Only the embedded post-exec filter; never spawn the binary
    /// (deterministic CI / air-gapped).
    BuiltinOnly,
    /// Only the external binary rewrite; never run the embedded filter (parity
    /// debugging; keeps the `rtk gain` ledger fed).
    ExternalOnly,
}

/// Default kill-time for the `rtk rewrite` probe. A hung rewriter must never
/// delay the real command.
pub const RTK_DEFAULT_REWRITE_TIMEOUT_MS: i64 = 500;

/// settings.json `output_rewrite` section — the generic Bash output-compression
/// capability config. `engine` selects the backend; per-engine settings nest
/// under their own key. Every field optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialOutputRewriteSettings {
    pub engine: Option<OutputRewriteEngine>,
    pub rtk: PartialRtkSettings,
}

/// settings.json `output_rewrite.rtk` section — rtk-engine-specific knobs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialRtkSettings {
    pub mode: Option<RtkMode>,
    pub binary_path: Option<String>,
    pub exclude_commands: Vec<String>,
    pub rewrite_timeout_ms: Option<i64>,
}

/// Resolved Bash output-compression config. On/off is
/// [`coco_types::Feature::OutputRewrite`]; this carries the engine selection
/// plus each engine's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRewriteConfig {
    /// Which rewriter backend to build at bootstrap.
    pub engine: OutputRewriteEngine,
    /// rtk-engine settings (used when `engine == Rtk`).
    pub rtk: RtkConfig,
}

impl OutputRewriteConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        Self {
            engine: settings.output_rewrite.engine.unwrap_or_default(),
            rtk: RtkConfig::resolve(settings, env),
        }
    }
}

/// Resolved rtk-engine configuration. Filter tuning, tee mode, transparent
/// prefixes and analytics stay in rtk's own `~/.config/rtk/config.toml`, owned
/// by rtk — never mirrored here (design §5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtkConfig {
    /// rtk tier-selection policy (§3.5).
    pub mode: RtkMode,
    /// Explicit rtk binary; `None` → probe `$PATH` once per session.
    pub binary_path: Option<String>,
    /// coco-side skip list, matched on the first command word before the
    /// probe spawns. rtk's own `[hooks] exclude_commands` still applies
    /// inside the engine — union of vetoes (§3.4), NOT a mirror.
    pub exclude_commands: Vec<String>,
    /// Kill the `rtk rewrite` probe after this and fall back.
    pub rewrite_timeout_ms: i64,
}

impl Default for RtkConfig {
    fn default() -> Self {
        Self {
            mode: RtkMode::default(),
            binary_path: None,
            exclude_commands: Vec::new(),
            rewrite_timeout_ms: RTK_DEFAULT_REWRITE_TIMEOUT_MS,
        }
    }
}

impl RtkConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let rtk = &settings.output_rewrite.rtk;
        let mut config = Self {
            mode: rtk.mode.unwrap_or_default(),
            binary_path: rtk.binary_path.clone(),
            exclude_commands: rtk.exclude_commands.clone(),
            rewrite_timeout_ms: rtk
                .rewrite_timeout_ms
                .filter(|&ms| ms > 0)
                .unwrap_or(RTK_DEFAULT_REWRITE_TIMEOUT_MS),
        };
        // `COCO_RTK_PATH` overrides the settings binary path (env wins so a
        // one-off run can point at a different binary without editing files).
        if let Some(path) = env.get_string(EnvKey::CocoRtkPath) {
            config.binary_path = Some(path);
        }
        config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialMemorySettings {
    pub directory: Option<PathBuf>,
    pub skip_index: Option<bool>,
    pub kairos_mode: Option<bool>,

    // Extraction (turn-end forked agent — services/extractMemories)
    pub extraction_enabled: Option<bool>,
    pub extraction_throttle: Option<i32>,
    pub extraction_max_turns: Option<i32>,

    // Team memory
    pub team_memory_enabled: Option<bool>,

    // Auto-dream consolidation (services/autoDream)
    pub dream_enabled: Option<bool>,
    pub dream_min_hours: Option<i32>,
    pub dream_min_sessions: Option<i32>,

    // Session memory (services/SessionMemory) — distinct from compact's
    pub session_memory_enabled: Option<bool>,
    pub session_memory_init_tokens: Option<i64>,
    pub session_memory_update_tokens: Option<i64>,
    pub session_memory_tool_calls: Option<i32>,
    pub session_memory_per_section_tokens: Option<i64>,
    pub session_memory_total_tokens: Option<i64>,

    // Optional "Searching past context" prompt block (TS
    // `buildSearchingPastContextSection`, gated by `tengu_coral_fern`).
    pub searching_past_context_enabled: Option<bool>,

    /// Free-form policy text appended verbatim to the auto-memory
    /// system-prompt block. Surfaced through
    /// `coco_memory::MemoryRuntime::render_system_prompt_section` so
    /// Cowork-style deployments can push operator-controlled memory
    /// governance into context without modifying crate-bundled
    /// prompts.
    pub extra_guidelines: Option<String>,
}

/// Resolved auto-memory configuration.
/// Whether the subsystem is **active** is gated upstream by
/// `Feature::AutoMemory`; this struct only carries internal sub-toggles
/// and parameters. Sub-toggles for extraction, team memory, auto-dream,
/// and session memory all live here as flat fields with prefix naming
/// — there is no separate `*Config` per subsystem (matches the project
/// convention: one `Feature` gate, all sub-toggles flat in the owning
/// `*Config`).
/// Source of truth for `coco_memory::MemoryConfig` (thin adapter).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub directory: Option<PathBuf>,
    /// Memory **base** directory override — replaces the per-project
    /// `<config_home>/projects/<slug>/memory/` layout's `<config_home>`
    /// component, NOT the full memory directory. Project slug + the
    /// `projects/` / `memory/` segments are still appended.
    /// `directory` (full path override) takes precedence when both are set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_base_override: Option<PathBuf>,
    pub skip_index: bool,
    pub kairos_mode: bool,

    /// Extraction (turn-end forked agent).
    pub extraction_enabled: bool,
    pub extraction_throttle: i32,
    pub extraction_max_turns: i32,

    /// Team memory (memdir/team subdir).
    pub team_memory_enabled: bool,

    /// Auto-dream consolidation.
    pub dream_enabled: bool,
    pub dream_min_hours: i32,
    pub dream_min_sessions: i32,

    /// Session memory — distinct feature from
    /// `compact_settings::SessionMemoryConfig`.
    pub session_memory_enabled: bool,
    pub session_memory_init_tokens: i64,
    pub session_memory_update_tokens: i64,
    pub session_memory_tool_calls: i32,
    pub session_memory_per_section_tokens: i64,
    pub session_memory_total_tokens: i64,

    /// Inject the optional "Searching past context" guidance block in
    /// the auto-memory system-prompt section. Off by default, mirroring
    /// the `tengu_coral_fern` GrowthBook gate.
    pub searching_past_context_enabled: bool,

    /// Full auto-memory prompt body override. Env-only, mirroring
    /// Claude Code's `CLAUDE_COWORK_MEMORY_GUIDELINES`: when set, the
    /// rendered prompt is exactly `# auto memory\n{trimmed}` and the
    /// standard bundled memory prompt is skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidelines: Option<String>,

    /// Free-form policy text appended verbatim to the auto-memory
    /// system-prompt section (after the standard taxonomy /
    /// how-to-save blocks, before the optional searching-past-context
    /// block). `None` or empty after trim ⇒ no extra section.
    /// Resolution: `extra_guidelines` setting in `settings.memory`
    /// (string) → env override `COCO_COWORK_MEMORY_EXTRA_GUIDELINES`
    /// (env wins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_guidelines: Option<String>,

    /// Mounted memory stores parsed from `COCO_MEMORY_STORES`. Empty by
    /// default. A non-empty list enables team recall outright (mounted ⇒
    /// enabled). Env-only — not a settings field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_stores: Vec<crate::memory_stores::MemoryStore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDisabledReason {
    FeatureGate,
    BareMode,
    RemoteWithoutMemoryDir,
}

impl MemoryDisabledReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FeatureGate => "feature_gate",
            Self::BareMode => "bare_mode",
            Self::RemoteWithoutMemoryDir => "remote_without_memory_dir",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryActivation {
    pub active: bool,
    pub disabled_reason: Option<MemoryDisabledReason>,
}

impl MemoryActivation {
    pub fn active() -> Self {
        Self {
            active: true,
            disabled_reason: None,
        }
    }

    pub fn disabled(reason: MemoryDisabledReason) -> Self {
        Self {
            active: false,
            disabled_reason: Some(reason),
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            directory: None,
            memory_base_override: None,
            skip_index: false,
            kairos_mode: false,
            extraction_enabled: true,
            extraction_throttle: 1,
            extraction_max_turns: 5,
            team_memory_enabled: false,
            dream_enabled: true,
            dream_min_hours: 24,
            dream_min_sessions: 5,
            session_memory_enabled: true,
            session_memory_init_tokens: 10_000,
            session_memory_update_tokens: 5_000,
            session_memory_tool_calls: 3,
            session_memory_per_section_tokens: 2_000,
            session_memory_total_tokens: 12_000,
            searching_past_context_enabled: false,
            guidelines: None,
            extra_guidelines: None,
            memory_stores: Vec::new(),
        }
    }
}

impl MemoryConfig {
    fn resolve_sub_toggles(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        let s = &settings.memory;

        if let Some(v) = s.skip_index {
            config.skip_index = v;
        }
        if let Some(v) = s.kairos_mode {
            config.kairos_mode = v;
        }
        if let Some(v) = s.extraction_enabled {
            config.extraction_enabled = v;
        }
        if let Some(v) = s.extraction_throttle {
            config.extraction_throttle = v;
        }
        if let Some(v) = s.extraction_max_turns {
            config.extraction_max_turns = v;
        }
        if let Some(v) = s.team_memory_enabled {
            config.team_memory_enabled = v;
        }
        if let Some(v) = s.dream_enabled {
            config.dream_enabled = v;
        }
        if let Some(v) = s.dream_min_hours {
            config.dream_min_hours = v;
        }
        if let Some(v) = s.dream_min_sessions {
            config.dream_min_sessions = v;
        }
        if let Some(v) = s.session_memory_enabled {
            config.session_memory_enabled = v;
        }
        if let Some(v) = s.session_memory_init_tokens {
            config.session_memory_init_tokens = v;
        }
        if let Some(v) = s.session_memory_update_tokens {
            config.session_memory_update_tokens = v;
        }
        if let Some(v) = s.session_memory_tool_calls {
            config.session_memory_tool_calls = v;
        }
        if let Some(v) = s.session_memory_per_section_tokens {
            config.session_memory_per_section_tokens = v;
        }
        if let Some(v) = s.session_memory_total_tokens {
            config.session_memory_total_tokens = v;
        }
        if let Some(v) = s.searching_past_context_enabled {
            config.searching_past_context_enabled = v;
        }
        if let Some(v) = &s.extra_guidelines
            && !v.trim().is_empty()
        {
            config.extra_guidelines = Some(v.clone());
        }

        // Force-disable env overrides (truthy = disable). Settings can
        // already say "off"; these env vars only ever turn things off.
        if env.is_truthy(EnvKey::CocoMemoryExtractionDisable) {
            config.extraction_enabled = false;
        }
        if env.is_truthy(EnvKey::CocoMemoryDreamDisable) {
            config.dream_enabled = false;
        }
        if env.is_truthy(EnvKey::CocoMemorySessionMemoryDisable) {
            config.session_memory_enabled = false;
        }
        if env.is_truthy(EnvKey::CocoMemoryKairos) {
            config.kairos_mode = true;
        }
        if let Some(text) = env.get_string(EnvKey::CocoCoworkMemoryGuidelines)
            && !text.trim().is_empty()
        {
            config.guidelines = Some(text);
        }
        if let Some(text) = env.get_string(EnvKey::CocoCoworkMemoryExtraGuidelines)
            && !text.trim().is_empty()
        {
            config.extra_guidelines = Some(text);
        }

        // Clamps. Negative / zero values would break the gates.
        config.extraction_throttle = config.extraction_throttle.max(1);
        config.extraction_max_turns = config.extraction_max_turns.max(1);
        config.dream_min_hours = config.dream_min_hours.max(1);
        config.dream_min_sessions = config.dream_min_sessions.max(1);
        config.session_memory_init_tokens = config.session_memory_init_tokens.max(1);
        config.session_memory_update_tokens = config.session_memory_update_tokens.max(1);
        config.session_memory_tool_calls = config.session_memory_tool_calls.max(1);
        config.session_memory_per_section_tokens = config.session_memory_per_section_tokens.max(1);
        config.session_memory_total_tokens = config.session_memory_total_tokens.max(1);
        config
    }

    pub fn resolve_with_sources(settings: &SettingsWithSource, env: &EnvSnapshot) -> Self {
        match Self::try_resolve_with_sources(settings, env) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!(
                    target: "coco::config",
                    error = %err,
                    "falling back to memory config without mounted stores"
                );
                let mut config = Self::resolve_sub_toggles(&settings.merged, env);
                config.directory = highest_trusted_memory_directory(settings);
                if let Some(base) = env.get_string(EnvKey::CocoRemoteMemoryDir)
                    && let Ok(path) = validate_memory_dir_override(&base, TildeExpansion::Reject)
                {
                    config.memory_base_override = Some(path);
                }
                if let Some(dir) = env.get_string(EnvKey::CocoMemoryPathOverride)
                    && let Ok(path) = validate_memory_dir_override(&dir, TildeExpansion::Reject)
                {
                    config.directory = Some(path);
                }
                config
            }
        }
    }

    pub fn try_resolve_with_sources(
        settings: &SettingsWithSource,
        env: &EnvSnapshot,
    ) -> crate::Result<Self> {
        let mut config = Self::resolve_sub_toggles(&settings.merged, env);

        // Mounted memory stores (env-only). A non-empty list enables team
        // recall outright (mounted ⇒ enabled) — see `is_team_recall_enabled`.
        if let Some(raw) = env.get_string(EnvKey::CocoMemoryStores) {
            config.memory_stores = crate::memory_stores::try_parse_memory_stores(&raw)?;
        }

        // Path overrides — two distinct semantics:
        // • `COCO_MEMORY_PATH_OVERRIDE` (operator): **full path** to the
        // personal memory directory. The `<projects>/<slug>/memory/`
        // layout is bypassed entirely. TS:
        // `CLAUDE_COWORK_MEMORY_PATH_OVERRIDE`.
        // • `COCO_REMOTE_MEMORY_DIR` (swarm leader → teammate
        // propagation): **base dir** that replaces `<config_home>`
        // in the default layout — the per-project slug + `memory/`
        // are still appended. Same project on both leader and
        // teammate (same canonical git root → same slug) resolves
        // to the same final memory dir. TS:
        // `CLAUDE_CODE_REMOTE_MEMORY_DIR`.
        // `memory.directory` is a full-path override and must only come
        // from trusted/operator-controlled settings layers. Project and
        // plugin settings may tune sub-options, but they cannot redirect
        // the personal memory root. The two env vars MAY coexist; full
        // override wins as the final personal directory, but the remote
        // base still has to validate so remote activation can trust it.
        config.directory = highest_trusted_memory_directory(settings);

        if let Some(base) = env.get_string(EnvKey::CocoRemoteMemoryDir) {
            match validate_memory_dir_override(&base, TildeExpansion::Reject) {
                Ok(path) => config.memory_base_override = Some(path),
                Err(reason) => tracing::warn!(
                    target: "coco::config",
                    env = %EnvKey::CocoRemoteMemoryDir,
                    reason,
                    "ignoring invalid auto-memory base override"
                ),
            }
        }

        if let Some(dir) = env.get_string(EnvKey::CocoMemoryPathOverride) {
            match validate_memory_dir_override(&dir, TildeExpansion::Reject) {
                Ok(path) => config.directory = Some(path),
                Err(reason) => tracing::warn!(
                    target: "coco::config",
                    env = %EnvKey::CocoMemoryPathOverride,
                    reason,
                    "ignoring invalid auto-memory directory override"
                ),
            }
        }

        Ok(config)
    }

    /// Whether team-memory recall is enabled.
    ///
    /// The `isTeamMemoryEnabled` precedence inversion: a mounted store
    /// (non-empty `memory_stores`) enables team recall outright, BEFORE
    /// the `team_memory_enabled` toggle is consulted. coco has no rollout
    /// flag, so "mounted ⇒ enabled"; otherwise fall back to the existing
    /// `team_memory_enabled` config.
    pub fn is_team_recall_enabled(&self) -> bool {
        !self.memory_stores.is_empty() || self.team_memory_enabled
    }
}

fn highest_trusted_memory_directory(settings: &SettingsWithSource) -> Option<PathBuf> {
    for source in [
        SettingSource::Policy,
        SettingSource::Flag,
        SettingSource::Local,
        SettingSource::User,
    ] {
        let Some(value) = settings.per_source.get(&source) else {
            continue;
        };
        let Some(raw) = value
            .get("memory")
            .and_then(|m| m.get("directory"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        match validate_memory_dir_override(raw, TildeExpansion::ExpandSafe) {
            Ok(path) => return Some(path),
            Err(reason) => tracing::warn!(
                target: "coco::config",
                source = %source,
                reason,
                "ignoring invalid auto-memory directory setting"
            ),
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TildeExpansion {
    Reject,
    ExpandSafe,
}

fn validate_memory_dir_override(
    raw: &str,
    tilde_expansion: TildeExpansion,
) -> Result<PathBuf, &'static str> {
    if raw.is_empty() {
        return Err("empty");
    }
    if raw.contains('\0') {
        return Err("null_byte");
    }
    let expanded;
    let raw = if raw.starts_with('~') {
        match tilde_expansion {
            TildeExpansion::Reject => return Err("tilde"),
            TildeExpansion::ExpandSafe => {
                let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) else {
                    return Err("tilde");
                };
                let Some(home) = dirs::home_dir() else {
                    return Err("home_unavailable");
                };
                expanded = home.join(rest).to_string_lossy().into_owned();
                expanded.as_str()
            }
        }
    } else {
        raw
    };
    if raw.starts_with("\\\\") || raw.starts_with("//") {
        return Err("unc");
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && (bytes[0] as char).is_ascii_alphabetic() {
        let tail = raw[2..].trim_start_matches(['\\', '/']);
        if tail.is_empty() {
            return Err("drive_root");
        }
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err("not_absolute");
    }
    if path.to_string_lossy().len() < 3 {
        return Err("near_root");
    }
    Ok(path)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialMcpRuntimeSettings {
    pub tool_timeout_ms: Option<i32>,
    pub tool_idle_timeout_ms: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRuntimeConfig {
    pub tool_timeout_ms: Option<i32>,
    pub tool_idle_timeout_ms: Option<i32>,
}

impl McpRuntimeConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        if let Some(v) = settings.mcp_runtime.tool_timeout_ms {
            config.tool_timeout_ms = Some(v);
        }
        if let Some(v) = settings.mcp_runtime.tool_idle_timeout_ms {
            config.tool_idle_timeout_ms = Some(v);
        }
        if let Some(v) = env.get_i32(EnvKey::CocoMcpToolTimeoutMs) {
            config.tool_timeout_ms = Some(v);
        }
        if let Some(v) = env.get_i32(EnvKey::ClaudeCodeMcpToolIdleTimeout) {
            config.tool_idle_timeout_ms = Some(v);
        }
        if let Some(v) = env.get_i32(EnvKey::CocoMcpToolIdleTimeoutMs) {
            config.tool_idle_timeout_ms = Some(v);
        }
        if let Some(v) = config.tool_timeout_ms {
            config.tool_timeout_ms = Some(v.max(1));
        }
        if let Some(v) = config.tool_idle_timeout_ms {
            config.tool_idle_timeout_ms = Some(if v <= 0 { 0 } else { v.max(1_000) });
        }
        config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialWebFetchSettings {
    pub timeout_secs: Option<i64>,
    pub max_content_length: Option<i64>,
    pub user_agent: Option<String>,
    pub inline_byte_budget: Option<i64>,
    pub preapproved_verbatim_budget: Option<i64>,
    pub extraction: Option<WebFetchExtraction>,
}

/// How WebFetch turns a fetched page into a model-visible result.
///
/// Owned here (config layer) and consumed by `coco-tools`, mirroring the other
/// config enums — the tool reads the resolved value, it does not define policy.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebFetchExtraction {
    /// Default: [`Windowed`](Self::Windowed) for content that is already clean
    /// (markdown / plain text / JSON and other non-HTML passthrough);
    /// [`Llm`](Self::Llm) for `text/html`. Rationale: the head+tail window
    /// evidence was measured on clean-markdown backends; scraped HTML keeps
    /// nav/footer chrome that clusters in the head window, so the side-model
    /// extract stays the HTML default until a main-content pass exists.
    #[default]
    Auto,
    /// Deterministic head+tail window + persisted pointer for ALL content types.
    Windowed,
    /// Side-query LLM extraction for everything non-preapproved (v0 behavior).
    Llm,
}

/// 1 MiB default cap per persisted request/response body.
const DEFAULT_WIRE_DUMP_MAX_BODY_BYTES: i64 = 1024 * 1024;

/// Verbosity for raw LLM wire-traffic dumps written under the session
/// directory (`<session_dir>/wire/`). Off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireDumpLevel {
    /// No capture; zero overhead.
    #[default]
    Off,
    /// Capture every call, but persist a request/response triplet only
    /// when the call fails; successful calls write only an index line.
    Error,
    /// Persist every call's request and response.
    All,
}

impl WireDumpLevel {
    /// Canonical lowercase token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::All => "all",
        }
    }

    /// Parse a settings / env token. Tolerant of common synonyms so a
    /// `COCO_DIAGNOSTICS_WIRE_DUMP=1` still does something sensible.
    pub fn from_token(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "0" | "none" | "" => Some(Self::Off),
            "error" | "errors" | "error_only" => Some(Self::Error),
            "all" | "true" | "1" | "full" => Some(Self::All),
            _ => None,
        }
    }

    /// Whether capture is disabled.
    pub fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }
}

/// Diagnostics knobs (currently only the LLM wire-traffic dumper).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsConfig {
    /// Verbosity for the wire-traffic dumper.
    pub wire_dump: WireDumpLevel,
    /// Max bytes persisted per request/response body before truncation.
    pub wire_dump_max_body_bytes: i64,
    /// Redact known secret patterns before writing. Leave on except for
    /// self-host debugging.
    pub wire_dump_redact: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            wire_dump: WireDumpLevel::Off,
            wire_dump_max_body_bytes: DEFAULT_WIRE_DUMP_MAX_BODY_BYTES,
            wire_dump_redact: true,
        }
    }
}

impl DiagnosticsConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        if let Some(v) = &settings.diagnostics.wire_dump
            && let Some(level) = WireDumpLevel::from_token(v)
        {
            config.wire_dump = level;
        }
        if let Some(v) = settings.diagnostics.wire_dump_max_body_bytes {
            config.wire_dump_max_body_bytes = v;
        }
        if let Some(v) = settings.diagnostics.wire_dump_redact {
            config.wire_dump_redact = v;
        }
        // Env layer wins over settings.
        if let Some(s) = env.get(EnvKey::CocoDiagnosticsWireDump) {
            match WireDumpLevel::from_token(s) {
                Some(level) => config.wire_dump = level,
                None => tracing::warn!(
                    value = s,
                    "ignoring COCO_DIAGNOSTICS_WIRE_DUMP: expected off|error|all"
                ),
            }
        }
        if let Some(v) = env.get_i64(EnvKey::CocoDiagnosticsWireMaxBytes) {
            config.wire_dump_max_body_bytes = v;
        }
        config.wire_dump_max_body_bytes = config.wire_dump_max_body_bytes.max(0);
        config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialDiagnosticsSettings {
    pub wire_dump: Option<String>,
    pub wire_dump_max_body_bytes: Option<i64>,
    pub wire_dump_redact: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebFetchConfig {
    pub timeout_secs: i64,
    /// Retained full-text cap: the page is truncated to this before windowing
    /// + persisting (default 2_000_000). The persisted artifact never exceeds
    /// it, so all footer numbers describe the same bounded text.
    pub max_content_length: i64,
    pub user_agent: String,
    /// Verbatim inline budget for a windowed fetch (default 15_000).
    pub inline_byte_budget: i64,
    /// Larger verbatim budget for preapproved docs hosts (default 100_000).
    pub preapproved_verbatim_budget: i64,
    /// Extraction strategy dispatch.
    pub extraction: WebFetchExtraction,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            timeout_secs: DEFAULT_WEB_FETCH_TIMEOUT_SECS,
            max_content_length: DEFAULT_WEB_FETCH_MAX_CONTENT_LENGTH,
            user_agent: DEFAULT_WEB_FETCH_USER_AGENT.to_string(),
            inline_byte_budget: DEFAULT_WEB_FETCH_INLINE_BYTE_BUDGET,
            preapproved_verbatim_budget: DEFAULT_WEB_FETCH_PREAPPROVED_VERBATIM_BUDGET,
            extraction: WebFetchExtraction::Auto,
        }
    }
}

impl WebFetchConfig {
    pub fn resolve(settings: &Settings) -> Self {
        let mut config = Self::default();
        if let Some(v) = settings.web_fetch.timeout_secs {
            config.timeout_secs = v;
        }
        if let Some(v) = settings.web_fetch.max_content_length {
            config.max_content_length = v;
        }
        if let Some(v) = &settings.web_fetch.user_agent {
            config.user_agent.clone_from(v);
        }
        if let Some(v) = settings.web_fetch.inline_byte_budget.filter(|v| *v > 0) {
            config.inline_byte_budget = v;
        }
        if let Some(v) = settings
            .web_fetch
            .preapproved_verbatim_budget
            .filter(|v| *v > 0)
        {
            config.preapproved_verbatim_budget = v;
        }
        if let Some(v) = settings.web_fetch.extraction {
            config.extraction = v;
        }
        config.timeout_secs = config.timeout_secs.max(1);
        config.max_content_length = config.max_content_length.max(0);
        config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialWebSearchSettings {
    pub provider: Option<WebSearchProvider>,
    pub max_results: Option<i32>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProvider {
    #[default]
    DuckDuckGo,
    Tavily,
    OpenAi,
}

impl WebSearchProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Tavily => "tavily",
            Self::OpenAi => "openai",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchConfig {
    pub provider: WebSearchProvider,
    pub max_results: i32,
    pub api_key: Option<String>,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: WebSearchProvider::DuckDuckGo,
            max_results: 5,
            api_key: None,
        }
    }
}

impl WebSearchConfig {
    pub fn resolve(settings: &Settings) -> Self {
        let mut config = Self::default();
        if let Some(v) = settings.web_search.provider {
            config.provider = v;
        }
        if let Some(v) = settings.web_search.max_results {
            config.max_results = v;
        }
        if let Some(v) = &settings.web_search.api_key {
            config.api_key = Some(v.clone());
        }
        config.max_results = config.max_results.clamp(1, 20);
        config
    }
}

// `AttachmentConfig` previously lived here. Removed — no consumer,
// and the two fields (`disable_attachments`,
// `enable_token_usage_attachment`) weren't wired into
// `coco_context::attachment`. Re-add when the attachment pipeline
// grows explicit on/off gates.

/// 10 MB cap on the file the agent can dispatch LSP queries against.
/// (rust-analyzer chokes on huge generated bundles; pyright reads the
/// whole file into memory.)
const DEFAULT_LSP_MAX_FILE_SIZE_BYTES: i64 = 10_000_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialLspSettings {
    pub max_file_size_bytes: Option<i64>,
}

/// Resolved LSP tool-layer knobs. Today only the per-query file-size
/// gate; future fields (per-server timeout overrides, prewarm policy,
/// notification debounce) land here so the wire shape stays stable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspConfig {
    /// Maximum on-disk size of a file the `LspTool` will dispatch
    /// a query against. Files larger than this are rejected at the
    /// tool layer (`validate_lsp_file`) before reaching the LSP
    /// server. `0` disables the gate.
    pub max_file_size_bytes: i64,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            max_file_size_bytes: DEFAULT_LSP_MAX_FILE_SIZE_BYTES,
        }
    }
}

impl LspConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        if let Some(v) = settings.lsp.max_file_size_bytes {
            config.max_file_size_bytes = v;
        }
        if let Some(v) = env.get_i64(EnvKey::CocoLspMaxFileSizeBytes) {
            config.max_file_size_bytes = v;
        }
        config.max_file_size_bytes = config.max_file_size_bytes.max(0);
        config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialPathSettings {
    pub project_dir: Option<PathBuf>,
}

/// Resolved filesystem paths. Only `project_dir` ships today — the
/// unused `plugin_root` / `env_file` slots were removed (consumers
/// elsewhere read them from their own scopes rather than this struct).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathConfig {
    pub project_dir: Option<PathBuf>,
}

impl PathConfig {
    pub fn resolve(settings: &Settings) -> Self {
        Self {
            project_dir: settings.paths.project_dir.clone(),
        }
    }
}

// ─── Voice (speech-to-text dictation) ────────────────────────────────────
//
// `VoiceConfig` carries backend/language/model sub-parameters. On/off is owned
// by `Feature::Voice` (`/voice` persists `features.voice`) — there is
// deliberately NO `enabled` field here (mirrors `MemoryConfig`/`AutoMemory`).
//
// Remote routing is a `(remote.provider, remote.model)` pair: `provider` keys
// into the providers registry (reusing its base_url + credential resolution),
// so multiple OpenAI-wire STT hosts are providers.json entries, NOT bespoke
// credential fields here. STT is an auxiliary API surface like `EmbeddingConfig`
// — not a `ModelRole`. Local routing is a `LocalSttEngine` discriminant + a
// flat per-engine knob struct, so new on-device engines are additive.

/// Where captured audio is transcribed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceBackend {
    /// Remote OpenAI-wire transcription (`/v1/audio/transcriptions`). MVP
    /// default. The endpoint + credentials come from the provider named by
    /// [`RemoteVoiceConfig::provider`] in the providers registry, so any
    /// OpenAI-wire STT host (OpenAI, Groq, a self-hosted faster-whisper) is a
    /// providers.json entry away — no separate credential config lives here.
    /// `openai` is a deserialize alias so a `backend: "openai"` persisted by an
    /// earlier build still parses (and doesn't fail the whole settings load).
    #[default]
    #[serde(alias = "openai")]
    Remote,
    /// On-device transcription (engine chosen by [`LocalVoiceConfig::engine`]).
    /// Requires the matching cargo feature (e.g. `local-voice` for Whisper)
    /// compiled into the binary; the factory returns a typed error otherwise.
    #[serde(alias = "whisper")]
    Local,
}

impl VoiceBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Local => "local",
        }
    }

    /// Parse the coarse env-override token (settings.json deserializes straight
    /// into the enum via serde; this is only for `COCO_VOICE_BACKEND`).
    pub fn from_token(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "remote" | "openai" => Some(Self::Remote),
            "local" | "whisper" => Some(Self::Local),
            _ => None,
        }
    }
}

/// How a finished transcript lands in the prompt input (from jcode). Only
/// `Insert` is wired in Phase 1; the rest are reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMode {
    /// Insert at the cursor (Phase 1 default).
    #[default]
    Insert,
    /// Append to the end of the current input.
    Append,
    /// Replace the whole input buffer.
    Replace,
    /// Insert then immediately submit the message.
    Send,
}

/// Remote (OpenAI-wire) STT knobs — the sanctioned `(provider, model)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteVoiceConfig {
    /// Providers-registry key that fronts `/v1/audio/transcriptions` (supplies
    /// base_url + credentials). Must be an OpenAI-wire provider — `openai`, or
    /// a `providers.json` entry such as `groq`. Default `openai`.
    pub provider: String,
    /// Transcription model id served by `provider`. Default
    /// `gpt-4o-mini-transcribe` (cheapest/fastest); `gpt-4o-transcribe` or
    /// `whisper-1` also valid.
    pub model: String,
}

impl Default for RemoteVoiceConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4o-mini-transcribe".to_string(),
        }
    }
}

/// Which on-device STT engine `VoiceBackend::Local` dispatches to. Closed set;
/// each variant reads its own knob struct on [`LocalVoiceConfig`]. Only
/// `whisper` is implemented today (behind the `local-voice` cargo feature).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalSttEngine {
    /// whisper.cpp via `whisper-rs`.
    #[default]
    Whisper,
}

/// On-device backend selection + per-engine knobs. Flat per-engine sub-structs
/// (not a `#[serde(tag)]` enum) so switching `engine` preserves the other
/// engine's settings and a partial merge never hits a missing-tag case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalVoiceConfig {
    /// Active on-device engine.
    pub engine: LocalSttEngine,
    /// Whisper knobs (used when `engine == whisper`).
    pub whisper: LocalWhisperConfig,
}

/// On-device Whisper knobs (behind the `local-voice` feature).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalWhisperConfig {
    /// Whisper size/name token, e.g. `base.en`, `small`, `medium`. A name in
    /// the built-in model table downloads with a pinned checksum; any other
    /// name requires `model_url`.
    pub model: String,
    /// Where weights are cached. `None` ⇒ `<config_home>/models/whisper/`.
    pub cache_dir: Option<PathBuf>,
    /// Base URL to fetch ggml weights from (mirror override, e.g. an
    /// `hf-mirror.com` host). `None` ⇒ the built-in HuggingFace base. The file
    /// name is derived from `model`.
    pub download_base: Option<String>,
    /// Full URL override for this exact model's weights — highest priority,
    /// bypasses `download_base` and the built-in table. Required when `model`
    /// is not in the built-in table.
    pub model_url: Option<String>,
    /// Auto-download missing weights on first use. Only known-table models
    /// (pinned checksum) auto-download; a custom `model_url` always needs the
    /// explicit `/voice-config download`. `false` ⇒ error with a hint instead.
    pub auto_download: bool,
    /// Surface a download-progress indicator on first use.
    pub show_download_progress: bool,
}

impl Default for LocalWhisperConfig {
    fn default() -> Self {
        Self {
            model: "base.en".to_string(),
            cache_dir: None,
            download_base: None,
            model_url: None,
            auto_download: true,
            show_download_progress: true,
        }
    }
}

const DEFAULT_VOICE_LANGUAGE: &str = "auto";

/// Resolved voice-input configuration. On/off is `Feature::Voice`, not a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub backend: VoiceBackend,
    /// Dictation language: BCP-47 / ISO-639-1 (e.g. `en`, `es`, `zh`) or
    /// `auto`. Shared by both backends — passed to the remote API's `language`
    /// hint and to whisper's `set_language`, each of which uses it to improve
    /// accuracy/latency. Free-form and backend-defined — not enumerated.
    pub language: String,
    pub transcript_mode: TranscriptMode,
    pub remote: RemoteVoiceConfig,
    pub local: LocalVoiceConfig,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            backend: VoiceBackend::default(),
            language: DEFAULT_VOICE_LANGUAGE.to_string(),
            transcript_mode: TranscriptMode::default(),
            remote: RemoteVoiceConfig::default(),
            local: LocalVoiceConfig::default(),
        }
    }
}

impl VoiceConfig {
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = Self::default();
        if let Some(backend) = settings.voice.backend {
            config.backend = backend;
        }
        if let Some(language) = &settings.voice.language {
            config.language.clone_from(language);
        }
        if let Some(mode) = settings.voice.transcript_mode {
            config.transcript_mode = mode;
        }
        if let Some(remote) = &settings.voice.remote {
            config.remote = remote.clone();
        }
        if let Some(local) = &settings.voice.local {
            config.local = local.clone();
        }
        // Env is a coarse override layer (settings.json wins the typed enum).
        if let Some(raw) = env.get(EnvKey::CocoVoiceBackend) {
            match VoiceBackend::from_token(raw) {
                Some(backend) => config.backend = backend,
                None => tracing::warn!(
                    value = raw,
                    "ignoring COCO_VOICE_BACKEND: expected remote|local"
                ),
            }
        }
        if let Some(language) = env.get_string(EnvKey::CocoVoiceLanguage) {
            config.language = language;
        }
        if let Some(model) = env.get_string(EnvKey::CocoVoiceModel) {
            config.remote.model = model;
        }
        config
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialVoiceSettings {
    pub backend: Option<VoiceBackend>,
    pub language: Option<String>,
    pub transcript_mode: Option<TranscriptMode>,
    pub remote: Option<RemoteVoiceConfig>,
    pub local: Option<LocalVoiceConfig>,
}

#[cfg(test)]
#[path = "sections.test.rs"]
mod tests;
