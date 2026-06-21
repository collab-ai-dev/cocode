//! CLI entry point via clap.

pub mod agent_handle_factory;
pub mod agent_transcript_persistence;
pub mod at_mention_turn;
pub mod bash_tool_handle;
pub mod command_queue_sink;
pub mod conversation_export;
pub mod coordinator_mode_resume;
pub mod cron_tick;
pub mod disk_task_output;
pub mod elicitation_hooks;
pub mod file_changed_watcher;
pub mod fork_dispatcher;
pub mod headless;
pub mod hook_agent_runner;
pub mod leader_inbox_poller;
pub mod leader_permission;
pub mod live_permission_mode;
pub mod lsp_handle_adapter;
pub mod mcp_handle_adapter;
pub mod model_card_refresh;
pub mod output;
pub mod paths;
pub mod permission_rule_loader;
pub mod plugin_watch;
pub mod provider_login;
pub mod resume_hint;
pub mod resume_resolver;
pub mod sandbox_approval_bridge_tui;
pub mod sandbox_reload;
pub mod sdk_server;
pub mod session_bootstrap;
pub mod session_rename;
pub mod session_runtime;
pub mod shell_tool_selection;
pub mod side_query_impl;
pub mod side_question;
pub mod skill_watch;
pub mod startup_profile;
pub mod task_runtime;
pub mod team_memory_sync;
pub mod team_task_list_router;
pub mod teammate_inbox_pump;
pub mod tracing_init;
pub mod tui_permission_bridge;

use clap::Parser;
use clap::Subcommand;

/// Multi-line `--version` text: semver + commit hash/date/subject + build time.
/// The `COCO_BUILD_*` components are emitted by build.rs; `concat!` over `env!`
/// keeps it a compile-time `&'static str` (clap needs a const version).
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\ncommit: ",
    env!("COCO_BUILD_GIT_HASH"),
    " (",
    env!("COCO_BUILD_GIT_DATE"),
    ") ",
    env!("COCO_BUILD_GIT_SUBJECT"),
    "\nbuilt:  ",
    env!("COCO_BUILD_TIME"),
);

/// The coco CLI.
#[derive(Clone, Parser)]
#[command(name = "coco", about = "AI coding agent", version = LONG_VERSION)]
pub struct Cli {
    /// Prompt to send (non-interactive mode).
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Main model to use.
    #[arg(long = "models.main")]
    pub models_main: Option<String>,

    /// Run without TUI (REPL mode).
    #[arg(long)]
    pub no_tui: bool,

    /// Output as NDJSON (SDK mode).
    #[arg(long)]
    pub json: bool,

    /// Settings file override.
    #[arg(long)]
    pub settings: Option<String>,

    /// Maximum tokens.
    #[arg(long)]
    pub max_tokens: Option<i64>,

    /// Maximum turns.
    #[arg(long)]
    pub max_turns: Option<i32>,

    /// Permission mode.
    #[arg(long)]
    pub permission_mode: Option<String>,

    /// Working directory override.
    #[arg(long, short = 'C')]
    pub cwd: Option<String>,

    /// Debug mode.
    #[arg(long)]
    pub debug: bool,

    /// Verbose mode.
    #[arg(long, short)]
    pub verbose: bool,

    /// Run the conversation in the background.
    #[arg(long, alias = "background")]
    pub bg: bool,

    /// Resume a specific session by ID (shorthand for `resume <id>`).
    #[arg(long, short = 'r')]
    pub resume: Option<String>,

    /// Thinking budget for extended thinking mode.
    #[arg(long)]
    pub thinking_budget: Option<i64>,

    /// System prompt override (appended to default).
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Append instructions from a file to the system prompt.
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    /// MCP config JSON (inline server definitions).
    #[arg(long)]
    pub mcp_config: Option<String>,

    /// Continue the most recent conversation.
    #[arg(long, short = 'c', alias = "continue")]
    pub continue_session: bool,

    /// Output format: text, json, stream-json.
    #[arg(long, default_value = "text")]
    pub output_format: String,

    /// Reasoning effort level: low, medium, high, max.
    #[arg(long)]
    pub effort: Option<coco_types::ReasoningEffort>,

    /// Allow specific tools (repeatable).
    #[arg(long, num_args = 1..)]
    pub allowed_tools: Vec<String>,

    /// Deny specific tools (repeatable).
    #[arg(long, num_args = 1..)]
    pub disallowed_tools: Vec<String>,

    /// Additional directories to allow access to (repeatable).
    #[arg(long, num_args = 1..)]
    pub add_dir: Vec<String>,

    /// Create a git worktree for isolated work.
    #[arg(long, short = 'w')]
    pub worktree: Option<Option<String>>,

    /// Set display name for the session.
    #[arg(long, short = 'n')]
    pub name: Option<String>,

    /// Bypass all permission checks (dangerous).
    ///
    /// Starts the session directly in `BypassPermissions` mode AND
    /// unlocks it as a reachable target for Shift+Tab / plan-mode exit.
    #[arg(long)]
    pub dangerously_skip_permissions: bool,

    /// Unlock `BypassPermissions` as an option without entering it at
    /// startup.
    ///
    /// The user still starts in the default (or `--permission-mode`) mode,
    /// but can later cycle into bypass via Shift+Tab or plan-mode exit.
    #[arg(long)]
    pub allow_dangerously_skip_permissions: bool,

    /// Print response and exit (non-interactive mode).
    #[arg(long, alias = "print")]
    pub non_interactive: bool,

    /// Automatic fallback model(s) on overload. Repeatable — each
    /// occurrence appends one more tier to the Main role's fallback
    /// chain. Accepted form: `provider/model_id`. The chain is
    /// walked in flag order on capacity-error streaks.
    ///
    /// Legacy single-flag usage (`--fallback-model anthropic/sonnet`)
    /// continues to work and produces a 1-tier chain.
    #[arg(long, value_name = "PROVIDER/MODEL_ID")]
    pub fallback_model: Vec<String>,

    /// Custom agent for the session.
    #[arg(long)]
    pub agent: Option<String>,

    /// Maximum spending limit in USD.
    #[arg(long)]
    pub max_budget_usd: Option<f64>,

    /// Run setup hooks and exit.
    #[arg(long)]
    pub init_only: bool,

    /// Disable session persistence.
    #[arg(long)]
    pub no_session_persistence: bool,

    /// Bare mode: skip session-start + per-turn background housekeeping
    /// (auto-dream, memory extraction, prompt suggestion, stale-dir sweeps).
    /// Flag form of `COCO_BARE_MODE=1`.
    #[arg(long)]
    pub bare: bool,

    // ── SDK/scripting flags ──
    /// Structured input format for non-interactive mode.
    ///
    /// Pairs with `--output-format` to drive scripted pipelines over stdio.
    #[arg(long)]
    pub input_format: Option<String>,

    /// Inline JSON Schema (NOT a file path) that validates the
    /// structured output of the run. Only honored in non-interactive
    /// sessions (`-p` print mode / SDK NDJSON); ignored in TUI.
    ///
    /// Example: `--json-schema '{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}'`
    #[arg(long)]
    pub json_schema: Option<String>,

    /// Replay user messages on resume (includes them in the transcript replay).
    #[arg(long)]
    pub replay_user_messages: bool,

    /// Emit hook lifecycle events in the stream-json output.
    ///
    /// Gates `HookStarted/Progress/Response` in the wire stream.
    #[arg(long)]
    pub include_hook_events: bool,

    /// Emit partial (incomplete) assistant messages in the stream-json output.
    ///
    /// Exposes in-flight streaming content for clients that render mid-turn.
    #[arg(long)]
    pub include_partial_messages: bool,

    /// Thinking mode: enabled, adaptive, or disabled.
    ///
    /// Orthogonal to `--thinking-budget` (which sets the token ceiling when enabled).
    #[arg(long)]
    pub thinking: Option<String>,

    /// Max tokens for extended thinking.
    ///
    /// Cap on reasoning tokens per turn.
    #[arg(long)]
    pub max_thinking_tokens: Option<i64>,

    /// File containing instructions to append to the system prompt.
    ///
    /// Reads the file and appends its contents to the default system prompt.
    #[arg(long)]
    pub append_system_prompt_file: Option<String>,

    /// Fail fast on invalid MCP config rather than best-effort loading.
    ///
    /// If set, any malformed server entry aborts startup.
    #[arg(long)]
    pub strict_mcp_config: bool,

    /// Comma-separated list of setting sources to load (user, project, local).
    ///
    /// Restricts which config layers participate.
    #[arg(long)]
    pub setting_sources: Option<String>,

    /// Fork a new session from the provided session ID.
    ///
    /// Copies history from `--resume <id>` into a fresh session rather than continuing it.
    #[arg(long)]
    pub fork_session: bool,

    /// Comma-separated list of provider beta headers to opt into.
    ///
    /// Example: `prompt-caching-2024-07-31`.
    #[arg(long)]
    pub betas: Option<String>,

    /// Explicit session ID to use for this run.
    ///
    /// For deterministic session IDs in automation. Distinct from `--resume`
    /// (continue existing) and `--fork-session` (copy existing).
    #[arg(long)]
    pub session_id: Option<String>,

    /// MCP tool name to delegate permission prompts to.
    ///
    /// Routes `Ask` decisions to the named tool instead of the built-in TUI / SDK bridge.
    #[arg(long)]
    pub permission_prompt_tool: Option<String>,

    // ── Tracing / log dev knobs ──
    /// Tracing-filter directive applied to all logs. Highest-priority
    /// override; takes precedence over `COCO_LOG` and `RUST_LOG`.
    ///
    /// Accepts either a bare level (`debug`) or a full `EnvFilter`
    /// directive (`coco=trace,coco_inference::stream=trace,info`). A
    /// bare level is expanded to `coco=<level>,<level>` so coco crates
    /// stay verbose without flooding third-party output.
    #[arg(long, value_name = "DIRECTIVE")]
    pub log_level: Option<String>,

    /// Log output format: `pretty | compact | json`. Default depends
    /// on mode (json for SDK, compact for TUI/headless).
    #[arg(long, value_name = "FORMAT")]
    pub log_format: Option<String>,

    /// Override the default rotating log file path
    /// (`<config_home>/logs/coco.log`).
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<String>,

    /// Force-enable a stderr fmt layer in addition to the file sink.
    /// Useful for `--print` debugging sessions where you want to see
    /// logs alongside the response.
    #[arg(long)]
    pub log_stderr: bool,

    /// Show source `file:line` + thread name on each log event.
    /// Tri-state: `--log-location` (or `=true`) forces on,
    /// `--log-location=false` forces off, omission falls back to the
    /// auto-rule — enabled when the resolved filter is the bare level
    /// `debug` or `trace`. Higher priority than `COCO_LOG_LOCATION`.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    pub log_location: Option<bool>,

    /// Timezone for log timestamps: `local | utc`. Defaults to `local`.
    /// Higher priority than `COCO_LOG_TIMEZONE`.
    #[arg(long, value_name = "TZ")]
    pub log_timezone: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// CLI subcommands.
#[derive(Clone, Subcommand)]
pub enum Commands {
    /// Start a new conversation.
    Chat {
        /// Initial prompt.
        prompt: Option<String>,
    },
    /// Manage configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Resume a previous session.
    Resume {
        /// Session ID or title.
        session_id: Option<String>,
    },
    /// List sessions.
    Sessions,
    /// Show status.
    Status,
    /// Run diagnostics.
    Doctor,
    /// Log in to a provider subscription via OAuth (e.g. `coco login openai`).
    Login {
        /// Provider to log into (e.g. `openai`). Defaults to `openai`.
        provider: Option<String>,
        /// Print the authorization URL instead of opening a browser
        /// (headless / SSH).
        #[arg(long, alias = "headless")]
        no_browser: bool,
    },
    /// Clear stored provider credentials (defaults to `openai`).
    Logout {
        /// Provider to log out of (e.g. `openai`). Defaults to `openai`.
        provider: Option<String>,
    },
    /// Initialize project (.claude/ directory).
    Init,
    /// Review code changes or a PR.
    Review {
        /// PR number or file to review.
        target: Option<String>,
    },
    /// Manage MCP server connections.
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Manage plugins.
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// List discovered agent definitions.
    ///
    /// Walks `~/.coco/agents/` and `.coco/agents/` for markdown frontmatter agent specs.
    Agents,
    /// Show auto-mode defaults.
    #[command(name = "auto-mode")]
    AutoMode {
        /// Subcommand: "defaults" to show default rules.
        subcmd: Option<String>,
    },

    /// Run a long-running background supervisor (daemon mode).
    Daemon,

    /// List running background sessions.
    Ps,

    /// Show logs from a background session.
    Logs {
        /// Session ID.
        session_id: String,
    },

    /// Attach to a running background session.
    Attach {
        /// Session ID.
        session_id: String,
    },

    /// Kill a running background session.
    Kill {
        /// Session ID.
        session_id: String,
    },

    /// Start remote control / bridge mode.
    #[command(alias = "rc", alias = "bridge")]
    RemoteControl,

    /// Sync with a remote session.
    Sync,

    /// Show release notes for the current version.
    #[command(name = "release-notes")]
    ReleaseNotes,

    /// Upgrade to the latest version.
    Upgrade,

    /// Show cost and usage information.
    Usage,

    /// Run in SDK mode — NDJSON over stdio with the JSON-RPC control
    /// protocol. Intended to be spawned as a subprocess by the
    /// Python/TypeScript SDK client.
    Sdk,
}

/// Config subcommand actions.
#[derive(Clone, Subcommand)]
pub enum ConfigAction {
    /// Get a configuration value.
    Get {
        /// Configuration key.
        key: String,
    },
    /// Set a configuration value.
    Set {
        /// Configuration key.
        key: String,
        /// New value.
        value: String,
    },
    /// List all configuration values.
    List,
    /// Reset to defaults.
    Reset,
}

/// MCP subcommand actions.
#[derive(Clone, Subcommand)]
pub enum McpAction {
    /// List connected servers.
    List,
    /// Add a server.
    Add {
        /// Server name.
        name: String,
        /// Configuration JSON.
        config: Option<String>,
    },
    /// Remove a server.
    Remove {
        /// Server name.
        name: String,
    },
}

/// Plugin subcommand actions.
#[derive(Clone, Subcommand)]
pub enum PluginAction {
    /// List installed plugins.
    List,
    /// Install a plugin from a local path or known marketplace.
    /// Mirrors the `/plugin install` slash command: pass a local
    /// directory for path install, or `<name>[@<marketplace>]` to
    /// install from a previously-registered marketplace. Plugin install
    /// always targets a pluginId — to add a *marketplace* from a git
    /// SSH/HTTPS URL, GitHub shorthand, or local path, use
    /// `/plugin marketplace add <source>`.
    Install {
        /// Local directory containing `PLUGIN.toml`, or plugin
        /// identifier of the form `<name>[@<marketplace>]`.
        name: String,
    },
    /// Uninstall a plugin by name.
    Uninstall {
        /// Plugin name.
        name: String,
    },
    /// Validate a plugin manifest at the given path.
    ///
    /// Checks PLUGIN.toml structure.
    Validate {
        /// Path to plugin directory (must contain `PLUGIN.toml`).
        path: String,
    },
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
