//! Application host for the `coco` surfaces.
//!
//! Owns session-runtime construction, AppServer/SDK request handling, and
//! protocol-neutral application behavior. The `coco-cli` package owns only
//! process startup, clap dispatch, terminal presentation, and surface wiring.

pub mod embedded_hub;
pub mod execution_plan;
pub mod startup_profile;
pub mod tracing_init;

use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;

pub const BUILD_PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_GIT_HASH: &str = env!("COCO_BUILD_GIT_HASH");
pub const BUILD_GIT_DATE: &str = env!("COCO_BUILD_GIT_DATE");
pub const BUILD_GIT_SUBJECT: &str = env!("COCO_BUILD_GIT_SUBJECT");
pub const BUILD_TIME: &str = env!("COCO_BUILD_TIME");

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

pub fn build_provenance() -> coco_utils_common::BuildProvenance {
    coco_utils_common::BuildProvenance::new(
        BUILD_PACKAGE_VERSION,
        BUILD_GIT_HASH,
        BUILD_GIT_DATE,
        BUILD_GIT_SUBJECT,
        BUILD_TIME,
    )
}

impl Cli {
    /// Convert parsed surface arguments into the clap-independent host input.
    pub fn agent_host_options(&self) -> coco_agent_host::AgentHostOptions {
        coco_agent_host::AgentHostOptions {
            prompt: self.prompt.clone(),
            models_main: self.models_main.clone(),
            settings: self.settings.clone(),
            event_hub_url: self.event_hub_url.clone(),
            max_tokens: self.max_tokens,
            max_turns: self.max_turns,
            permission_mode: self.permission_mode.clone(),
            cwd: self.cwd.clone(),
            resume: self.resume.clone(),
            system_prompt: self.system_prompt.clone(),
            append_system_prompt: self.append_system_prompt.clone(),
            continue_session: self.continue_session,
            allowed_tools: self.allowed_tools.clone(),
            disallowed_tools: self.disallowed_tools.clone(),
            fallback_model: self.fallback_model.clone(),
            add_dir: self.add_dir.clone(),
            dangerously_skip_permissions: self.dangerously_skip_permissions,
            allow_dangerously_skip_permissions: self.allow_dangerously_skip_permissions,
            no_session_persistence: self.no_session_persistence,
            json_schema: self.json_schema.clone(),
            include_hook_events: self.include_hook_events,
            append_system_prompt_file: self.append_system_prompt_file.clone(),
            plan_mode_instructions: self.plan_mode_instructions.clone(),
            setting_sources: self.setting_sources.clone(),
            fork_session: self.fork_session,
            session_id: self.session_id.clone(),
        }
    }
}

/// The cocode CLI.
#[derive(Clone, Parser)]
#[command(name = "cocode", about = "AI coding agent", version = LONG_VERSION)]
pub struct Cli {
    /// Prompt to send (non-interactive mode).
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Main model to use.
    #[arg(long = "models.main")]
    pub models_main: Option<String>,

    /// Settings file override.
    #[arg(long)]
    pub settings: Option<String>,

    /// Event Hub WebSocket endpoint for session event egress.
    #[arg(
        long = "event-hub-url",
        value_name = "WS_URL",
        conflicts_with = "serve_hub"
    )]
    pub event_hub_url: Option<String>,

    /// Start an embedded local Event Hub and send this process's events to it.
    #[arg(long = "serve-hub")]
    pub serve_hub: bool,

    /// Port for the embedded Event Hub.
    #[arg(long = "hub-port", default_value_t = 8731, requires = "serve_hub")]
    pub hub_port: u16,

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

    /// Resume a specific session by ID (shorthand for `resume <id>`).
    #[arg(long, short = 'r')]
    pub resume: Option<String>,

    /// System prompt override (appended to default).
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Append instructions from a file to the system prompt.
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    /// Continue the most recent conversation.
    #[arg(long, short = 'c', alias = "continue")]
    pub continue_session: bool,

    /// Allow specific tools (repeatable).
    #[arg(long, num_args = 1..)]
    pub allowed_tools: Vec<String>,

    /// Deny specific tools (repeatable).
    #[arg(long, num_args = 1..)]
    pub disallowed_tools: Vec<String>,

    /// Additional directories to allow access to (repeatable).
    #[arg(long, num_args = 1..)]
    pub add_dir: Vec<String>,

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

    /// Disable session persistence.
    #[arg(long)]
    pub no_session_persistence: bool,

    /// Bare mode: skip session-start + per-turn background housekeeping
    /// (auto-dream, memory extraction, prompt suggestion, stale-dir sweeps).
    /// Flag form of `COCO_BARE_MODE=1`.
    #[arg(long)]
    pub bare: bool,

    /// Inline JSON Schema (NOT a file path) that validates the
    /// structured output of the run. Only honored in non-interactive
    /// sessions (`-p` print mode / SDK NDJSON); ignored in TUI.
    ///
    /// Example: `--json-schema '{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}'`
    #[arg(long)]
    pub json_schema: Option<String>,

    /// Emit hook lifecycle events in the stream-json output.
    ///
    /// Gates `HookStarted/Progress/Response` in the wire stream.
    #[arg(long)]
    pub include_hook_events: bool,

    /// File containing instructions to append to the system prompt.
    ///
    /// Reads the file and appends its contents to the default system prompt.
    #[arg(long)]
    pub append_system_prompt_file: Option<String>,

    /// Custom workflow body for plan mode.
    ///
    /// Replaces the default plan-mode implementation phases while keeping the
    /// read-only preamble and ExitPlanMode footer.
    #[arg(long, hide = true)]
    pub plan_mode_instructions: Option<String>,

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

    /// Explicit session ID to use for this run.
    ///
    /// For deterministic session IDs in automation. Distinct from `--resume`
    /// (continue existing) and `--fork-session` (copy existing).
    #[arg(long)]
    pub session_id: Option<String>,

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
        /// Import an existing credential from another tool's auth file
        /// (e.g. `~/.codex/auth.json`) instead of running OAuth. The file is
        /// read once and never modified; symlinks are rejected.
        #[arg(long, value_name = "PATH")]
        import: Option<std::path::PathBuf>,
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
    /// Manage Mixture-of-Agents presets.
    Moa {
        #[command(subcommand)]
        action: MoaAction,
    },
    /// List discovered agent definitions.
    ///
    /// Walks `config home/agents/` and `project config dir/agents/` for markdown frontmatter agent specs.
    Agents,
    /// Show auto-mode defaults.
    #[command(name = "auto-mode")]
    AutoMode {
        /// Subcommand: "defaults" to show default rules.
        subcmd: Option<String>,
    },

    /// Run a local exec-server over WebSocket or stdio.
    #[command(name = "exec-server")]
    ExecServer {
        /// Listen URL: `ws://IP:PORT` or `stdio`.
        #[arg(long, default_value = coco_exec_server::DEFAULT_LISTEN_URL)]
        listen: String,
    },

    /// List running background sessions.
    Ps {
        /// Emit a JSON array (for scripting; no TTY required).
        #[arg(long)]
        json: bool,
        /// Include process-less terminal entries (completed / failed).
        #[arg(long)]
        all: bool,
    },

    /// Show release notes for the current version.
    #[command(name = "release-notes")]
    ReleaseNotes,

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

/// MoA preset management actions.
#[derive(Clone, Subcommand)]
pub enum MoaAction {
    /// List configured MoA presets.
    List,
    /// Create or replace a MoA preset.
    Configure {
        /// Preset name.
        name: String,
        /// Aggregator model as provider/model.
        #[arg(long, value_name = "provider/model")]
        aggregator: String,
        /// Reference model as provider/model. May be repeated.
        #[arg(long = "reference", value_name = "provider/model", required = true)]
        references: Vec<String>,
        /// Reference fanout policy.
        #[arg(long, value_enum, default_value = "per_iteration")]
        fanout: MoaFanoutArg,
        /// Maximum tokens for each reference response.
        #[arg(long = "reference-max-tokens", value_name = "N")]
        reference_max_tokens: Option<i64>,
        /// Temperature override for reference calls.
        #[arg(long = "reference-temperature", value_name = "FLOAT")]
        reference_temperature: Option<f32>,
        /// Temperature override for the aggregator call.
        #[arg(long = "aggregator-temperature", value_name = "FLOAT")]
        aggregator_temperature: Option<f32>,
        /// Make this preset the default for `/moa`.
        #[arg(long = "default")]
        make_default: bool,
        /// Enable this preset.
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        /// Disable this preset.
        #[arg(long, conflicts_with = "enable")]
        disable: bool,
    },
    /// Delete a MoA preset.
    Delete {
        /// Preset name.
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum MoaFanoutArg {
    #[default]
    PerIteration,
    UserTurn,
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
    /// Authenticate with an MCP server.
    Login {
        /// Server name.
        name: String,
        /// Print the authorization URL instead of opening a browser.
        #[arg(long, alias = "headless")]
        no_browser: bool,
    },
    /// Clear stored OAuth credentials for an MCP server.
    Logout {
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
