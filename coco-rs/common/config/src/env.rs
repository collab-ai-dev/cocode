use std::collections::HashMap;
use std::env::VarError;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fmt;

use strum::IntoEnumIterator;

/// Known environment variables owned or interpreted by coco.
///
/// Keep dynamic provider keys as strings; this enum is for stable env keys
/// that are part of coco's runtime/config surface.
///
/// `strum::EnumIter` is derived so `EnvKey::iter()` always stays in sync
/// with the enum definition — no hand-maintained parallel array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum EnvKey {
    AnthropicApiKey,
    AnthropicAuthToken,
    AnthropicBaseUrl,
    AnthropicFoundryResource,
    AnthropicVertexProjectId,
    CocoAgentColor,
    CocoAgentId,
    CocoAgentName,
    /// Test/diagnostic override for the OpenAI OAuth token endpoint used by
    /// the provider-auth subscription flow (wiremock seam). Unset in normal
    /// use. Mirrors codex `CODEX_REFRESH_TOKEN_URL_OVERRIDE`.
    CocoAuthOpenaiTokenUrl,
    /// Test/diagnostic override for the Google (Gemini) OAuth token endpoint
    /// used by the provider-auth subscription flow (wiremock seam).
    CocoAuthGeminiTokenUrl,
    /// Test/diagnostic override for the xAI Grok OAuth token endpoint used by
    /// the provider-auth subscription flow (wiremock seam).
    CocoAuthXaiTokenUrl,
    /// Test/diagnostic override for the xAI Grok device authorization endpoint.
    CocoAuthXaiDeviceUrl,
    /// Test/diagnostic override for the OpenAI OAuth revocation endpoint
    /// (logout). Wiremock seam; unset in normal use.
    CocoAuthOpenaiRevokeUrl,
    /// Test/diagnostic override for the Google (Gemini) OAuth revocation
    /// endpoint (logout). Wiremock seam; unset in normal use.
    CocoAuthGeminiRevokeUrl,
    /// User override for the provider-auth credential-storage backend
    /// (`auto`|`file`|`keyring`|`ephemeral`). Highest-priority source in
    /// [`crate::resolve_credential_store_mode`]; unset ⇒ the build-provenance
    /// default applies.
    CocoAuthCredentialStore,
    /// Entrypoint label written to the concurrent-sessions PID registry
    /// (`<config_home>/sessions/{pid}.json`). Identifies *how* the
    /// session was started ("sdk-py", "tmux-bg", "cli-interactive", …)
    /// so `coco ps` can attribute live sessions. Optional; missing means
    /// the field is omitted from the registry record.
    CocoEntrypoint,
    /// WebSocket endpoint for Event Hub connector egress.
    /// Accepted schemes are `ws://` and `wss://`.
    CocoEventHubUrl,
    /// Unix-domain socket path for the SDK AppServer NDJSON listener.
    /// Unix-only; ignored by non-SDK entrypoints and unsupported on Windows.
    CocoServerUnixSocketPath,
    /// TCP bind address for the SDK AppServer WebSocket listener.
    /// Opt-in only; ignored by non-SDK entrypoints.
    CocoServerWebSocketBind,
    /// Windows named-pipe path for the SDK AppServer NDJSON listener.
    /// Opt-in only; ignored by non-SDK entrypoints and unsupported on Unix.
    CocoServerNamedPipe,
    /// Disable the autonomous skill-learning loop (review + curator).
    CocoSkillLearnDisable,
    /// Override the review-fork throttle (eligible turns between forks).
    CocoSkillLearnReviewThrottle,
    /// Disable only the periodic skill curator.
    CocoSkillLearnCuratorDisable,
    /// Maximum live AppServer session slots for multi-session SDK mode.
    CocoServerMaxSessions,
    /// Maximum AppServer surfaces that one connection may attach.
    CocoServerMaxSurfacesPerConnection,
    /// Maximum passive AppServer surfaces attached to one session.
    CocoServerMaxPassiveSurfacesPerSession,
    /// Per-session AppServer event retention ring size.
    CocoServerEventRetentionPerSession,
    /// Per-connection AppServer outbound queue capacity in frames.
    CocoServerOutboundQueueFrames,
    /// Active-turn drain timeout during AppServer close cascade, in seconds.
    /// Non-positive values are ignored by config resolution.
    CocoServerTurnDrainTimeoutSecs,
    /// Process shutdown drain timeout for AppServer sessions, in seconds.
    /// Non-positive values are ignored by config resolution.
    CocoServerShutdownTimeoutSecs,
    /// Idle TTL before an unattached cached `ProjectServices` entry is
    /// evicted, in seconds. Non-positive values are ignored.
    CocoServerProjectServicesIdleTtlSecs,
    /// Optional auto-archive timeout for a session with zero surfaces and no
    /// active/queued turn, in seconds. Unset or non-positive = off.
    CocoServerIdleSessionTimeoutSecs,
    /// SessionKind override for the concurrent-sessions PID registry.
    /// Accepted values: `bg`, `daemon`, `daemon-worker`. Anything else
    /// (or unset) means the session registers as `interactive`.
    CocoSessionKind,
    CocoBashAutoBackgroundOnTimeout,
    /// Truthy ⇒ snap the bash cwd back to the original working directory after
    /// every command, regardless of whether the cwd is inside the allowed
    /// working set.
    CocoBashMaintainProjectWorkingDir,
    CocoBubblewrap,
    CocoConfigDir,
    /// LLM wire-traffic dump verbosity: `off` (default) / `error` / `all`.
    /// Overrides `diagnostics.wire_dump` in settings.json. Dumps land under
    /// `<session_dir>/wire/`.
    CocoDiagnosticsWireDump,
    /// Max bytes persisted per request/response body before truncation
    /// (default 1 MiB). Overrides `diagnostics.wire_dump_max_body_bytes`.
    CocoDiagnosticsWireMaxBytes,
    /// Truthy ⇒ suppress the git-status block in the system prompt; defined-falsy
    /// ⇒ force it on (overriding the `include_git_instructions` setting).
    CocoDisableGitInstructions,
    CocoDisableFastMode,
    /// Truthy => disable the memory-pressure idle background shell reaper.
    CocoDisableMemoryPressureShellReaper,
    /// Truthy ⇒ skip loading managed/policy-level skills from the platform
    /// managed skills directory.
    CocoDisablePolicySkills,
    CocoDisableShellSnapshot,
    CocoFileReadIgnorePatterns,
    CocoFoundryResource,
    CocoGlobTimeoutSeconds,
    /// Grep content-mode default per-file match cap (§2.3). Overrides
    /// `tool.search.grep_per_file_limit`. 0 = unlimited.
    CocoGrepPerFileLimit,
    /// Glob result cap before truncation. Overrides `tool.search.glob_max_results`.
    CocoGlobMaxResults,
    /// Glob directory-grouping thresholds (§2.4). Override
    /// `tool.search.glob_group_min_{paths,dirs}`.
    CocoGlobGroupMinPaths,
    CocoGlobGroupMinDirs,
    CocoLang,
    /// Tracing-filter directive (full `EnvFilter` syntax, e.g.
    /// `coco=debug,coco_inference::stream=trace,info`). Read by
    /// `coco_otel::subscriber` at startup. Lower priority than
    /// `--log-level`, higher priority than `RUST_LOG`.
    CocoLog,
    /// Explicit log file path. Overrides the default rotating path
    /// (`<config_home>/logs/coco.log`).
    CocoLogFile,
    /// Log format: `pretty | compact | json`. Defaults to `pretty` for
    /// TTY output and `json` for file output.
    CocoLogFormat,
    /// Tri-state override for "verbose layout" (file:line + thread
    /// name) on each log event. Truthy → force on, falsy → force off,
    /// unset → follow the auto rule (enabled when the resolved filter
    /// is the bare level `debug` or `trace`).
    CocoLogLocation,
    /// When truthy, force a stderr fmt layer in addition to the file
    /// sink. SDK / TUI normally write to file only — this opts in to
    /// also seeing logs on stderr (must not be set in SDK mode unless
    /// the caller can tolerate logs on stderr alongside stdout NDJSON).
    CocoLogStderr,
    /// Timezone for log timestamps: `local | utc`. Lower priority than
    /// `--log-timezone`. Defaults to `local`.
    CocoLogTimezone,
    /// Claude Code compatibility toggle: truthy values allow prompt bodies in
    /// OTEL logs. `OTEL_LOG_ASSISTANT_RESPONSES` inherits this when unset.
    OtelLogUserPrompts,
    /// Claude Code compatibility tri-state toggle for assistant response body
    /// logging. Unset/unrecognized inherits `OTEL_LOG_USER_PROMPTS`; falsy
    /// explicitly redacts responses even when prompt logging is enabled.
    OtelLogAssistantResponses,
    /// `LspConfig::max_file_size_bytes` override. Wins over settings.
    /// Files exceeding this size are rejected at the tool layer before
    /// reaching the LSP server (rust-analyzer / pyright OOM-guard).
    CocoLspMaxFileSizeBytes,
    CocoMaxContextTokens,
    /// Hard cap on consecutive `StructuredOutput` retries before the
    /// engine surfaces `error_max_structured_output_retries` and ends
    /// the turn (default `5`).
    CocoMaxStructuredOutputRetries,
    CocoMaxToolUseConcurrency,
    /// Full-path override for the auto-memory directory. When set, replaces
    /// the computed `<config_home>/projects/<sanitized-canonical-git-root>/memory/`
    /// path. Used by deployments where the per-session cwd contains a
    /// process-name suffix and would otherwise produce a different project
    /// key per session.
    CocoMemoryPathOverride,
    /// Force-disable turn-end memory extraction. Wins over settings.
    CocoMemoryExtractionDisable,
    /// Force-disable auto-dream consolidation. Wins over settings.
    CocoMemoryDreamDisable,
    /// Force-disable session-memory per-session insights. Wins over settings.
    CocoMemorySessionMemoryDisable,
    /// Force-enable KAIROS daily-log mode (assistant-mode append-only logs).
    CocoMemoryKairos,
    /// Override the team-memory sync endpoint base URL. Defaults to the
    /// Anthropic API base.
    CocoTeamMemorySyncUrl,
    /// JSON array of mounted memory stores. Each entry is either a bare
    /// absolute-path string or an object
    /// `{ path, mode:"rw"|"ro", scope:"user"|"team", mount?, promptIndex?,
    /// promptIndexMaxBytes? }`. Parsed into `MemoryConfig::memory_stores`.
    /// A non-empty list enables team recall outright (mounted ⇒ enabled).
    CocoMemoryStores,
    /// Full auto-memory system-prompt body override. When set, memory
    /// prompt rendering returns `# auto memory\n{value}` and skips the
    /// crate-bundled taxonomy / index blocks.
    CocoCoworkMemoryGuidelines,
    /// Free-form policy / guidance text appended verbatim to the
    /// standard auto-memory system-prompt section.
    CocoCoworkMemoryExtraGuidelines,
    CocoMcpToolTimeoutMs,
    /// Claude Code compatibility knob for remote MCP tool-call silence.
    /// `COCO_MCP_TOOL_IDLE_TIMEOUT_MS` is the native coco-rs spelling; this
    /// variant preserves the upstream env surface.
    ClaudeCodeMcpToolIdleTimeout,
    CocoMcpToolIdleTimeoutMs,
    CocoModelMain,
    CocoParentSessionId,
    /// Truthy ⇒ emergency killswitch that forcibly disables
    /// `BypassPermissions` mode regardless of CLI flags / settings
    /// (local operator override). Pairs with the policy-scope
    /// `bypassPermissionsKillswitch` managed setting.
    CocoPermissionsDisableBypass,
    CocoPlanModeRequired,
    CocoRemote,
    /// Override for the memory base directory (the parent of `projects/`).
    /// When set, replaces `<config_home>` as the root of the
    /// `<base>/projects/<sanitized>/memory/` resolution chain. Used by
    /// swarm leaders that mount persistent memory from a network volume
    /// separate from the session's config home.
    CocoRemoteMemoryDir,
    CocoSandboxAllowNetwork,
    CocoSandboxExcludedCommands,
    /// Truthy values force a hard error at startup if sandbox can't initialise.
    CocoSandboxFailIfUnavailable,
    CocoSandboxMode,
    CocoSessionEndHooksTimeoutMs,
    /// Voice STT backend override: `remote` | `local` (legacy `openai` /
    /// `whisper` aliases accepted). Invalid values are ignored with a warning
    /// (settings.json wins the enum; env is a coarse override). Consumed by
    /// `VoiceConfig::resolve`.
    CocoVoiceBackend,
    /// Voice dictation language (BCP-47 / ISO-639-1 or `auto`).
    CocoVoiceLanguage,
    /// Remote STT model id override (e.g. `gpt-4o-mini-transcribe`).
    CocoVoiceModel,
    CocoShell,
    /// Override the rtk binary path used by the Bash output compressor
    /// (`Feature::OutputRewrite`). Ranked below the settings `rtk.binary_path` value —
    /// env wins so a one-off run can point at a different binary. `None` ⇒
    /// probe `$PATH` for `rtk` then `rr-rtk`.
    CocoRtkPath,
    /// Prefix string injected before every hook command. Consumed by
    /// `coco_hooks::execute_hook` for Command-type hooks; NOT wired
    /// into `ShellConfig` / `ShellExecutor` (bash-tool uses its own
    /// settings.json path).
    CocoShellPrefix,
    /// Truthy ⇒ use the persistent autonomous `/loop` preamble.
    CocoLoopPersistent,
    CocoSimple,
    /// Truthy ⇒ emit startup phase timings (one `debug!` per phase with a
    /// `duration_ms` field). Read by `coco_agent_host::startup_profile`.
    CocoStartupProfile,
    CocoTaskListId,
    CocoTeamName,
    CocoTeammateCommand,
    /// Override the base directory for agent-team files + mailboxes
    /// (default `config home/teams`). Read by
    /// `coco_coordinator::team_file::teams_base_dir`; lets tests isolate the
    /// teams/mailbox tree (and a future swarm-leader relocate it, like
    /// `CocoRemoteMemoryDir` does for the memory base).
    CocoTeamsDir,
    /// Tri-state override for the TUI's kitty keyboard-enhancement push
    /// (truthy ⇒ never push, falsy ⇒ push even where auto-detection would
    /// skip it, unset ⇒ auto: disabled only for VS Code terminals under
    /// WSL). Read by `coco_tui::keyboard_modes`.
    CocoTuiKeyboardEnhancementDisable,
    CocoVerifyPlan,
    /// Opt non-interactive sessions INTO file-history checkpointing.
    /// Non-interactive sessions default OFF; interactive defaults ON.
    CocoFileCheckpointingNoninteractiveEnable,
    /// Disable file-history checkpointing for every session, overriding the
    /// settings/interactive default.
    CocoFileCheckpointingDisable,
    /// Soft kill auto-compact only. Manual `/compact` keeps working.
    CocoCompactDisableAuto,
    /// Hard kill all compaction (auto + manual).
    CocoCompactDisable,
    /// Force-enable session-memory compact (overrides
    /// `Settings.compact.session_memory.enabled`).
    CocoCompactSessionMemoryEnable,
    /// Force-disable session-memory compact (wins over enable).
    CocoCompactSessionMemoryDisable,
    /// Auto-compact context-window cap.
    CocoCompactAutoWindow,
    /// Auto-compact threshold percentage override (1-100).
    CocoCompactAutoPctOverride,
    /// Manual-compact blocking limit.
    CocoCompactBlockingLimit,
    /// API-native context_management trigger threshold (input tokens).
    CocoCompactApiMaxInputTokens,
    /// API-native context_management keep-target after clearing (input tokens).
    CocoCompactApiTargetInputTokens,
    /// Enable Anthropic `clear_tool_uses_20250919` for tool-result content.
    CocoCompactApiClearToolResults,
    /// Enable Anthropic `clear_tool_uses_20250919` for entire tool_use blocks.
    CocoCompactApiClearToolUses,
    /// Override microcompact keep-recent count for compactable tool results.
    CocoCompactMicroKeepRecent,
    /// Override time-based microcompact keep-recent count.
    CocoCompactMicroTimeBasedKeepRecent,
    /// Override the number of recently read files restored after full compact.
    CocoCompactPostCompactMaxFilesToRestore,
    /// Enable Tool Result Budget Level 2 (per-message aggregate cap).
    /// Default off. See `docs/internal/tool-result-budget-plan.md`.
    CocoCompactToolResultBudgetEnable,
    /// Per-message byte cap for Tool Result Budget Level 2. Overrides the
    /// window-scaled default with a fixed cap.
    CocoCompactToolResultBudgetPerMessageBytes,
    /// 1h-TTL allowlist for prompt-cache (comma-separated `query_source`
    /// patterns, exact match or `prefix*` glob).
    /// See `docs/internal/prompt-cache-design.md` §16a.
    CocoPromptCacheAllowlist,
    /// Enable coordinator mode (system-prompt swap + worker pool +
    /// `<task-notification>` XML routing). Requires `Feature::AgentTeams`.
    CocoCoordinatorMode,
    /// Enable fork-subagent path: omitting `subagent_type` on AgentTool
    /// triggers an implicit fork that inherits the parent's full
    /// conversation context for prompt-cache sharing. Mutually exclusive
    /// with coordinator mode.
    CocoForkSubagent,
    /// Disable the post-turn promptSuggestion service. When set truthy, the
    /// engine skips spawning the side-channel fork that computes "what should
    /// I ask next" placeholders.
    CocoPromptSuggestionDisable,
    /// `--bare` mode: skip ALL post-turn forks (promptSuggestion,
    /// extractMemories, autoDream). Used by SDK / scripted `-p`
    /// invocations that don't want background work after each turn.
    CocoBareMode,
    /// Disable AgentTool background-task registration. When set truthy,
    /// `run_in_background: true` and `AgentDefinition.background = true` are
    /// both ignored — every spawn runs synchronously. Useful for sandbox / CI
    /// environments that want deterministic blocking behavior.
    CocoBackgroundTasksDisable,
    /// Override `api.retry.max_retries`. Applies after settings.json and is
    /// clamped by `ApiRetryConfig::finalize`.
    CocoApiMaxRetries,
    /// Claude Code compatibility alias for `CocoApiMaxRetries`.
    ClaudeCodeMaxRetries,
    /// Claude Code compatibility opt-in for long unattended retry backoffs.
    ClaudeCodeRetryWatchdog,
    /// Disable the startup auto-install of the official plugin marketplace
    /// (`anthropics/claude-plugins-official`). When set truthy, coco does not
    /// fetch/register the official marketplace on launch.
    CocoPluginsDisableOfficialMarketplace,
    /// Read-only plugin seed directories (PATH-delimited, precedence order).
    /// Customers bake a populated plugins dir into a container image and point
    /// this at it; seed marketplaces/plugin caches are used in place without
    /// re-cloning.
    CocoPluginSeedDir,
    /// Enable auto-detach of long-running foreground AgentTool spawns.
    /// When set to a positive integer (milliseconds), foreground sub-agents
    /// that haven't completed by this deadline fire `signal_detach` so the
    /// parent's awaiter unblocks with `AsyncLaunched` and the engine keeps
    /// running in the background. Setting truthy (`1` / `true` / `on`)
    /// without a number uses the default `120_000` (2 minutes).
    CocoAutoBackgroundTasks,
    /// Enable periodic AgentSummary timers for TUI users. Default off.
    /// SDK clients opt-in via the `agentProgressSummaries: true` control
    /// message; TUI users use this env var instead.
    ///
    /// Coordinator mode auto-enables periodic summaries regardless of this flag.
    CocoAgentSummaryEnable,
    /// Inject the AgentTool agent listing into a `<system-reminder>`
    /// attachment instead of inline in the tool description. Off by default
    /// (keeps the listing inline in the tool description).
    CocoAgentListInMessages,
    /// Terminal-multiplexer detection (third-party env vars, not
    /// COCO-prefixed). Surfaced through `EnvKey` so pane backends
    /// don't reach for `std::env::var` directly. The env names are
    /// fixed by the host tools (tmux, iTerm2, etc.) — coco-rs only
    /// reads them.
    Tmux,
    TmuxPane,
    TermProgram,
    ItermSessionId,
    /// DeepSeek API key (vendor name — exempt from `COCO_` prefix).
    /// Shared by both `deepseek-openai` and `deepseek-anthropic`
    /// builtin providers.
    DeepseekApiKey,
}

impl EnvKey {
    /// Iterate over every known env key. Backed by `strum::EnumIter`, so
    /// adding a variant automatically shows up here.
    pub fn all() -> impl Iterator<Item = Self> {
        Self::iter()
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicApiKey => "ANTHROPIC_API_KEY",
            Self::AnthropicAuthToken => "ANTHROPIC_AUTH_TOKEN",
            Self::AnthropicBaseUrl => "ANTHROPIC_BASE_URL",
            Self::AnthropicFoundryResource => "ANTHROPIC_FOUNDRY_RESOURCE",
            Self::AnthropicVertexProjectId => "ANTHROPIC_VERTEX_PROJECT_ID",
            Self::CocoAgentColor => "COCO_AGENT_COLOR",
            Self::CocoAgentId => "COCO_AGENT_ID",
            Self::CocoAgentName => "COCO_AGENT_NAME",
            Self::CocoAuthOpenaiTokenUrl => "COCO_AUTH_OPENAI_TOKEN_URL",
            Self::CocoAuthGeminiTokenUrl => "COCO_AUTH_GEMINI_TOKEN_URL",
            Self::CocoAuthXaiTokenUrl => "COCO_AUTH_XAI_TOKEN_URL",
            Self::CocoAuthXaiDeviceUrl => "COCO_AUTH_XAI_DEVICE_URL",
            Self::CocoAuthOpenaiRevokeUrl => "COCO_AUTH_OPENAI_REVOKE_URL",
            Self::CocoAuthGeminiRevokeUrl => "COCO_AUTH_GEMINI_REVOKE_URL",
            Self::CocoAuthCredentialStore => "COCO_AUTH_CREDENTIAL_STORE",
            Self::CocoEntrypoint => "COCO_ENTRYPOINT",
            Self::CocoEventHubUrl => "COCO_EVENT_HUB_URL",
            Self::CocoServerUnixSocketPath => "COCO_SERVER_UNIX_SOCKET_PATH",
            Self::CocoServerWebSocketBind => "COCO_SERVER_WEBSOCKET_BIND",
            Self::CocoServerNamedPipe => "COCO_SERVER_NAMED_PIPE",
            Self::CocoSkillLearnDisable => "COCO_SKILL_LEARN_DISABLE",
            Self::CocoSkillLearnReviewThrottle => "COCO_SKILL_LEARN_REVIEW_THROTTLE",
            Self::CocoSkillLearnCuratorDisable => "COCO_SKILL_LEARN_CURATOR_DISABLE",
            Self::CocoServerMaxSessions => "COCO_SERVER_MAX_SESSIONS",
            Self::CocoServerMaxSurfacesPerConnection => "COCO_SERVER_MAX_SURFACES_PER_CONNECTION",
            Self::CocoServerMaxPassiveSurfacesPerSession => {
                "COCO_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION"
            }
            Self::CocoServerEventRetentionPerSession => "COCO_SERVER_EVENT_RETENTION_PER_SESSION",
            Self::CocoServerOutboundQueueFrames => "COCO_SERVER_OUTBOUND_QUEUE_FRAMES",
            Self::CocoServerTurnDrainTimeoutSecs => "COCO_SERVER_TURN_DRAIN_TIMEOUT_SECS",
            Self::CocoServerShutdownTimeoutSecs => "COCO_SERVER_SHUTDOWN_TIMEOUT_SECS",
            Self::CocoServerProjectServicesIdleTtlSecs => {
                "COCO_SERVER_PROJECT_SERVICES_IDLE_TTL_SECS"
            }
            Self::CocoServerIdleSessionTimeoutSecs => "COCO_SERVER_IDLE_SESSION_TIMEOUT_SECS",
            Self::CocoSessionKind => "COCO_SESSION_KIND",
            Self::CocoBashAutoBackgroundOnTimeout => "COCO_BASH_AUTO_BACKGROUND_ON_TIMEOUT",
            Self::CocoBashMaintainProjectWorkingDir => "COCO_BASH_MAINTAIN_PROJECT_WORKING_DIR",
            Self::CocoBubblewrap => "COCO_BUBBLEWRAP",
            Self::CocoConfigDir => "COCO_CONFIG_DIR",
            Self::CocoDiagnosticsWireDump => "COCO_DIAGNOSTICS_WIRE_DUMP",
            Self::CocoDiagnosticsWireMaxBytes => "COCO_DIAGNOSTICS_WIRE_MAX_BYTES",
            Self::CocoDisableGitInstructions => "COCO_DISABLE_GIT_INSTRUCTIONS",
            Self::CocoDisableFastMode => "COCO_DISABLE_FAST_MODE",
            Self::CocoDisableMemoryPressureShellReaper => {
                "COCO_DISABLE_MEMORY_PRESSURE_SHELL_REAPER"
            }
            Self::CocoDisablePolicySkills => "COCO_DISABLE_POLICY_SKILLS",
            Self::CocoDisableShellSnapshot => "COCO_DISABLE_SHELL_SNAPSHOT",
            Self::CocoFileReadIgnorePatterns => "COCO_FILE_READ_IGNORE_PATTERNS",
            Self::CocoFoundryResource => "COCO_FOUNDRY_RESOURCE",
            Self::CocoGlobTimeoutSeconds => "COCO_GLOB_TIMEOUT_SECONDS",
            Self::CocoGrepPerFileLimit => "COCO_GREP_PER_FILE_LIMIT",
            Self::CocoGlobMaxResults => "COCO_GLOB_MAX_RESULTS",
            Self::CocoGlobGroupMinPaths => "COCO_GLOB_GROUP_MIN_PATHS",
            Self::CocoGlobGroupMinDirs => "COCO_GLOB_GROUP_MIN_DIRS",
            Self::CocoLang => "COCO_LANG",
            Self::CocoLog => "COCO_LOG",
            Self::CocoLogFile => "COCO_LOG_FILE",
            Self::CocoLogFormat => "COCO_LOG_FORMAT",
            Self::CocoLogLocation => "COCO_LOG_LOCATION",
            Self::CocoLogStderr => "COCO_LOG_STDERR",
            Self::CocoLogTimezone => "COCO_LOG_TIMEZONE",
            Self::OtelLogUserPrompts => "OTEL_LOG_USER_PROMPTS",
            Self::OtelLogAssistantResponses => "OTEL_LOG_ASSISTANT_RESPONSES",
            Self::CocoLspMaxFileSizeBytes => "COCO_LSP_MAX_FILE_SIZE_BYTES",
            Self::CocoMaxContextTokens => "COCO_MAX_CONTEXT_TOKENS",
            Self::CocoMaxStructuredOutputRetries => "COCO_MAX_STRUCTURED_OUTPUT_RETRIES",
            Self::CocoMaxToolUseConcurrency => "COCO_MAX_TOOL_USE_CONCURRENCY",
            Self::CocoMemoryPathOverride => "COCO_MEMORY_PATH_OVERRIDE",
            Self::CocoMemoryExtractionDisable => "COCO_MEMORY_EXTRACTION_DISABLE",
            Self::CocoMemoryDreamDisable => "COCO_MEMORY_DREAM_DISABLE",
            Self::CocoMemorySessionMemoryDisable => "COCO_MEMORY_SESSION_MEMORY_DISABLE",
            Self::CocoMemoryKairos => "COCO_MEMORY_KAIROS",
            Self::CocoTeamMemorySyncUrl => "COCO_TEAM_MEMORY_SYNC_URL",
            Self::CocoMemoryStores => "COCO_MEMORY_STORES",
            Self::CocoCoworkMemoryGuidelines => "COCO_COWORK_MEMORY_GUIDELINES",
            Self::CocoCoworkMemoryExtraGuidelines => "COCO_COWORK_MEMORY_EXTRA_GUIDELINES",
            Self::CocoMcpToolTimeoutMs => "COCO_MCP_TOOL_TIMEOUT_MS",
            Self::ClaudeCodeMcpToolIdleTimeout => "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT",
            Self::CocoMcpToolIdleTimeoutMs => "COCO_MCP_TOOL_IDLE_TIMEOUT_MS",
            Self::CocoModelMain => "COCO_MODEL_MAIN",
            Self::CocoParentSessionId => "COCO_PARENT_SESSION_ID",
            Self::CocoPermissionsDisableBypass => "COCO_PERMISSIONS_DISABLE_BYPASS",
            Self::CocoPlanModeRequired => "COCO_PLAN_MODE_REQUIRED",
            Self::CocoRemote => "COCO_REMOTE",
            Self::CocoRemoteMemoryDir => "COCO_REMOTE_MEMORY_DIR",
            Self::CocoSandboxAllowNetwork => "COCO_SANDBOX_ALLOW_NETWORK",
            Self::CocoSandboxExcludedCommands => "COCO_SANDBOX_EXCLUDED_COMMANDS",
            Self::CocoSandboxFailIfUnavailable => "COCO_SANDBOX_FAIL_IF_UNAVAILABLE",
            Self::CocoSandboxMode => "COCO_SANDBOX_MODE",
            Self::CocoSessionEndHooksTimeoutMs => "COCO_SESSIONEND_HOOKS_TIMEOUT_MS",
            Self::CocoShell => "COCO_SHELL",
            Self::CocoRtkPath => "COCO_RTK_PATH",
            Self::CocoShellPrefix => "COCO_SHELL_PREFIX",
            Self::CocoVoiceBackend => "COCO_VOICE_BACKEND",
            Self::CocoVoiceLanguage => "COCO_VOICE_LANGUAGE",
            Self::CocoVoiceModel => "COCO_VOICE_MODEL",
            Self::CocoLoopPersistent => "COCO_LOOP_PERSISTENT",
            Self::CocoSimple => "COCO_SIMPLE",
            Self::CocoStartupProfile => "COCO_STARTUP_PROFILE",
            Self::CocoTaskListId => "COCO_TASK_LIST_ID",
            Self::CocoTeamName => "COCO_TEAM_NAME",
            Self::CocoTeammateCommand => "COCO_TEAMMATE_COMMAND",
            Self::CocoTeamsDir => "COCO_TEAMS_DIR",
            Self::CocoTuiKeyboardEnhancementDisable => "COCO_TUI_KEYBOARD_ENHANCEMENT_DISABLE",
            Self::CocoVerifyPlan => "COCO_VERIFY_PLAN",
            Self::CocoFileCheckpointingNoninteractiveEnable => {
                "COCO_FILE_CHECKPOINTING_NONINTERACTIVE_ENABLE"
            }
            Self::CocoFileCheckpointingDisable => "COCO_FILE_CHECKPOINTING_DISABLE",
            Self::CocoCompactDisableAuto => "COCO_COMPACT_DISABLE_AUTO",
            Self::CocoCompactDisable => "COCO_COMPACT_DISABLE",
            Self::CocoCompactSessionMemoryEnable => "COCO_COMPACT_SESSION_MEMORY_ENABLE",
            Self::CocoCompactSessionMemoryDisable => "COCO_COMPACT_SESSION_MEMORY_DISABLE",
            Self::CocoCompactAutoWindow => "COCO_COMPACT_AUTO_WINDOW",
            Self::CocoCompactAutoPctOverride => "COCO_COMPACT_AUTO_PCT_OVERRIDE",
            Self::CocoCompactBlockingLimit => "COCO_COMPACT_BLOCKING_LIMIT",
            Self::CocoCompactApiMaxInputTokens => "COCO_COMPACT_API_MAX_INPUT_TOKENS",
            Self::CocoCompactApiTargetInputTokens => "COCO_COMPACT_API_TARGET_INPUT_TOKENS",
            Self::CocoCompactApiClearToolResults => "COCO_COMPACT_API_CLEAR_TOOL_RESULTS",
            Self::CocoCompactApiClearToolUses => "COCO_COMPACT_API_CLEAR_TOOL_USES",
            Self::CocoCompactMicroKeepRecent => "COCO_COMPACT_MICRO_KEEP_RECENT",
            Self::CocoCompactMicroTimeBasedKeepRecent => {
                "COCO_COMPACT_MICRO_TIME_BASED_KEEP_RECENT"
            }
            Self::CocoCompactPostCompactMaxFilesToRestore => {
                "COCO_COMPACT_POST_COMPACT_MAX_FILES_TO_RESTORE"
            }
            Self::CocoCompactToolResultBudgetEnable => "COCO_COMPACT_TOOL_RESULT_BUDGET_ENABLE",
            Self::CocoPromptCacheAllowlist => "COCO_PROMPT_CACHE_ALLOWLIST",
            Self::CocoCompactToolResultBudgetPerMessageBytes => {
                "COCO_COMPACT_TOOL_RESULT_BUDGET_PER_MESSAGE_BYTES"
            }
            Self::CocoCoordinatorMode => "COCO_COORDINATOR_MODE",
            Self::CocoForkSubagent => "COCO_FORK_SUBAGENT",
            Self::CocoPromptSuggestionDisable => "COCO_PROMPT_SUGGESTION_DISABLE",
            Self::CocoBareMode => "COCO_BARE_MODE",
            Self::CocoBackgroundTasksDisable => "COCO_BACKGROUND_TASKS_DISABLE",
            Self::CocoApiMaxRetries => "COCO_API_MAX_RETRIES",
            Self::ClaudeCodeMaxRetries => "CLAUDE_CODE_MAX_RETRIES",
            Self::ClaudeCodeRetryWatchdog => "CLAUDE_CODE_RETRY_WATCHDOG",
            Self::CocoPluginsDisableOfficialMarketplace => {
                "COCO_PLUGINS_DISABLE_OFFICIAL_MARKETPLACE"
            }
            Self::CocoPluginSeedDir => "COCO_PLUGIN_SEED_DIR",
            Self::CocoAutoBackgroundTasks => "COCO_AUTO_BACKGROUND_TASKS",
            Self::CocoAgentSummaryEnable => "COCO_AGENT_SUMMARY_ENABLE",
            Self::CocoAgentListInMessages => "COCO_AGENT_LIST_IN_MESSAGES",
            Self::Tmux => "TMUX",
            Self::TmuxPane => "TMUX_PANE",
            Self::TermProgram => "TERM_PROGRAM",
            Self::ItermSessionId => "ITERM_SESSION_ID",
            Self::DeepseekApiKey => "DEEPSEEK_API_KEY",
        }
    }
}

impl AsRef<OsStr> for EnvKey {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(self.as_str())
    }
}

impl fmt::Display for EnvKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Wrapper around `std::env::var` that accepts `EnvKey` directly.
pub fn var<K: AsRef<OsStr>>(key: K) -> Result<String, VarError> {
    std::env::var(key)
}

/// Wrapper around `std::env::var_os` that accepts `EnvKey` directly.
pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
    std::env::var_os(key)
}

/// Normalize a raw env value against the truthy set ("1"/"true"/"yes"/"on").
fn is_truthy_value(raw: &str) -> bool {
    matches!(raw.to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Normalize a raw env value against the falsy set ("0"/"false"/"no"/"off").
fn is_falsy_value(raw: &str) -> bool {
    matches!(raw.to_lowercase().as_str(), "0" | "false" | "no" | "off")
}

/// Parse a raw env value into Some(true)/Some(false) or None if neither set.
fn parse_truthy(raw: &str) -> Option<bool> {
    if is_truthy_value(raw) {
        Some(true)
    } else if is_falsy_value(raw) {
        Some(false)
    } else {
        None
    }
}

/// Returns true if the environment variable is set to a truthy value
/// ("1", "true", "yes", "on").
pub fn is_env_truthy<K: AsRef<OsStr>>(key: K) -> bool {
    var(key).ok().is_some_and(|v| is_truthy_value(&v))
}

/// Returns true if the environment variable is set to a falsy value
/// ("0", "false", "no", "off").
pub fn is_env_falsy<K: AsRef<OsStr>>(key: K) -> bool {
    var(key).ok().is_some_and(|v| is_falsy_value(&v))
}

/// Tri-state truthy lookup. `Some(true)`/`Some(false)` for recognised
/// truthy/falsy values, `None` when the var is unset or unrecognised —
/// lets callers fall through to a default without conflating "unset"
/// with "explicitly false".
pub fn env_truthy_opt<K: AsRef<OsStr>>(key: K) -> Option<bool> {
    var(key).ok().and_then(|v| parse_truthy(&v))
}

/// Resolve assistant response body logging.
///
/// `OTEL_LOG_ASSISTANT_RESPONSES` is a tri-state override: recognised truthy
/// and falsy values win; unset or unrecognised values inherit prompt logging.
pub fn log_assistant_responses_enabled(log_user_prompts: bool) -> bool {
    resolve_log_assistant_responses(None, log_user_prompts)
}

/// Resolve assistant response body logging with an optional settings value.
///
/// Priority: `OTEL_LOG_ASSISTANT_RESPONSES` > settings
/// `log.assistant_responses` > `OTEL_LOG_USER_PROMPTS`.
pub fn resolve_log_assistant_responses(
    settings_assistant_responses: Option<bool>,
    log_user_prompts: bool,
) -> bool {
    env_truthy_opt(EnvKey::OtelLogAssistantResponses)
        .or(settings_assistant_responses)
        .unwrap_or(log_user_prompts)
}

/// Get an environment variable as an optional string.
pub fn env_opt<K: AsRef<OsStr>>(key: K) -> Option<String> {
    var(key).ok().filter(|v| !v.is_empty())
}

/// Get an environment variable as an optional i32.
pub fn env_opt_i32<K: AsRef<OsStr>>(key: K) -> Option<i32> {
    env_opt(key).and_then(|v| v.parse().ok())
}

/// Get an environment variable as an optional i64.
pub fn env_opt_i64<K: AsRef<OsStr>>(key: K) -> Option<i64> {
    env_opt(key).and_then(|v| v.parse().ok())
}

/// Get an environment variable as an optional u32.
pub fn env_opt_u32<K: AsRef<OsStr>>(key: K) -> Option<u32> {
    env_opt(key).and_then(|v| v.parse().ok())
}

/// Startup snapshot of stable coco-owned environment variables.
#[derive(Debug, Clone, Default)]
pub struct EnvSnapshot {
    values: HashMap<EnvKey, String>,
    /// Dynamic `COCO_FEATURE_<key>=1/0` overrides. The key here is the
    /// lowercase feature key (e.g. `auto_memory`), not the env-var name.
    feature_overrides: std::collections::BTreeMap<String, bool>,
}

const COCO_FEATURE_PREFIX: &str = "COCO_FEATURE_";

impl EnvSnapshot {
    /// Capture known env vars from the current process.
    pub fn from_current_process() -> Self {
        let values = EnvKey::all()
            .filter_map(|key| env_opt(key).map(|value| (key, value)))
            .collect();
        let feature_overrides = std::env::vars()
            .filter_map(|(k, v)| {
                let stripped = k.strip_prefix(COCO_FEATURE_PREFIX)?;
                let bool_val = parse_truthy(&v)?;
                Some((stripped.to_lowercase(), bool_val))
            })
            .collect();
        Self {
            values,
            feature_overrides,
        }
    }

    /// Build a snapshot from explicit pairs. Intended for tests and callers
    /// that already captured their environment.
    pub fn from_pairs<I, S>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (EnvKey, S)>,
        S: Into<String>,
    {
        Self {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
            feature_overrides: std::collections::BTreeMap::new(),
        }
    }

    /// Build a snapshot from explicit pairs plus feature overrides. For tests.
    pub fn from_pairs_with_features<I, S, F>(pairs: I, features: F) -> Self
    where
        I: IntoIterator<Item = (EnvKey, S)>,
        S: Into<String>,
        F: IntoIterator<Item = (String, bool)>,
    {
        Self {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
            feature_overrides: features.into_iter().collect(),
        }
    }

    /// Access the captured `COCO_FEATURE_*` overrides keyed by lowercase
    /// feature key.
    pub fn feature_overrides(&self) -> &std::collections::BTreeMap<String, bool> {
        &self.feature_overrides
    }

    pub fn get(&self, key: EnvKey) -> Option<&str> {
        self.values.get(&key).map(String::as_str)
    }

    pub fn get_string(&self, key: EnvKey) -> Option<String> {
        self.get(key).map(str::to_string)
    }

    pub fn get_i32(&self, key: EnvKey) -> Option<i32> {
        self.get(key).and_then(|value| value.parse().ok())
    }

    pub fn get_i64(&self, key: EnvKey) -> Option<i64> {
        self.get(key).and_then(|value| value.parse().ok())
    }

    pub fn is_truthy(&self, key: EnvKey) -> bool {
        self.get(key).is_some_and(is_truthy_value)
    }

    pub fn is_falsy(&self, key: EnvKey) -> bool {
        self.get(key).is_some_and(is_falsy_value)
    }
}

/// Env-only config. No Settings file equivalent.
///
/// Only holds env vars that have **no** corresponding typed section on
/// `RuntimeConfig`. Anything that also flows into a section (tool, shell,
/// memory, sandbox, mcp, …) is intentionally omitted to avoid two
/// consumers resolving the same knob to different values.
///
/// Bedrock / Vertex / Foundry routing env vars were removed — those
/// providers aren't shipped in coco-rs today. Re-add alongside the
/// provider crate when they land.
#[derive(Debug, Clone, Default)]
pub struct EnvOnlyConfig {
    /// Single-knob `COCO_MODEL_MAIN` Main override (kept env-only — it is
    /// the user's "swap the whole thing" escape hatch and must work
    /// before settings.json is parsed). Per-role models go through
    /// `settings.models.*` exclusively.
    pub model_override: Option<String>,

    /// `COCO_SIMPLE=1` — skip stored OAuth tokens and `api_key_helper`;
    /// resolve auth from env vars only. Consumed by
    /// `coco_inference::auth::resolve_auth` via `AuthResolveOptions`.
    /// Auth-only flag — never gate features off this.
    pub force_env_auth: bool,
}

impl EnvOnlyConfig {
    /// Read all env vars once at startup.
    pub fn from_env() -> Self {
        Self::from_snapshot(&EnvSnapshot::from_current_process())
    }

    pub fn from_snapshot(env: &EnvSnapshot) -> Self {
        Self {
            model_override: env.get_string(EnvKey::CocoModelMain),
            force_env_auth: env.is_truthy(EnvKey::CocoSimple),
        }
    }
}

#[cfg(test)]
#[path = "env.test.rs"]
mod tests;
