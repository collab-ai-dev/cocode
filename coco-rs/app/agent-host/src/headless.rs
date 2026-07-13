//! Headless (`coco -p "<prompt>"`) entry point exposed as a library
//! function so live tests, embeddings, and the binary all drive the
//! same code path.
//!
//! `run_chat` returns a structured [`RunChatOutcome`] instead of
//! printing to stdout. The binary's `main()` thin-wraps this and
//! formats stdout from the outcome.
//!
//! Helpers shared by `run_chat` and noninteractive AppServer runners (`MockModel`,
//! `resolve_main_model`, `cli_runtime_overrides`,
//! `build_runtime_config_for_cli`, `build_system_prompt[_for_model]`,
//! `resolve_startup_permission_state`) live here as well, so a test
//! can drive any of them in isolation.

#[path = "headless_support.rs"]
mod support;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use coco_inference::{
    AISdkError, LanguageModel, LanguageModelCallOptions, LanguageModelGenerateResult,
    LanguageModelStreamResult,
};
use coco_llm_types::{AssistantContentPart, FinishReason, StopReason, TextPart, Usage};
use coco_messages::CostTracker;
use coco_query::ContinueReason;
use coco_tool_runtime::ToolRegistry;
use coco_types::TokenUsage;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentHostOptions,
    shutdown::{ShutdownCoordinator, ShutdownDrainOutcome},
};
use coco_app_runtime::ProcessRuntime;
pub(crate) use support::resolve_additional_dirs;
pub use support::resolve_additional_dirs_display;
use support::{
    append_headless_goal_status, append_headless_slash_text, build_tool_filter,
    headless_local_goal_text_outcome, headless_text_outcome, parse_headless_goal_slash,
    persist_headless_local_transcript_messages, summarize_tool_filter,
};

/// Fallback base instructions used when a resolved `ModelInfo`
/// declares no `base_instructions` (e.g. Claude built-ins and any
/// user-added non-builtin model in `config home/providers.json` /
/// `models.json` that doesn't set `base_instructions[_file]`). Routed
/// through `coco_config::DEFAULT_BASE_INSTRUCTIONS` so the on-disk
/// `instructions/default_prompt.md` is the single source of truth.
pub const DEFAULT_SYSTEM_PROMPT_IDENTITY: &str = coco_config::DEFAULT_BASE_INSTRUCTIONS;

// ─── Mock model (no-credentials fallback) ────────────────────────────

/// Built-in mock model for development/testing.
pub struct MockModel {
    call_count: AtomicI32,
}

impl MockModel {
    pub fn new() -> Self {
        Self {
            call_count: AtomicI32::new(0),
        }
    }
}

impl Default for MockModel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LanguageModel for MockModel {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "mock-model"
    }
    async fn do_generate(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> std::result::Result<LanguageModelGenerateResult, AISdkError> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);
        let user_text: String = options
            .prompt
            .iter()
            .filter_map(|msg| match msg {
                coco_llm_types::LlmMessage::User { content, .. } => Some(
                    content
                        .iter()
                        .filter_map(|c| match c {
                            coco_llm_types::UserContentPart::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");

        let response = format!(
            "[mock model, call #{call}] Received: \"{user_text}\"\n\n\
             No model configured. Set models.main via settings.json or --models.main to use a real provider."
        );

        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: response,
                provider_metadata: None,
            })],
            usage: Usage::new(user_text.len() as u64 / 4, 50),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> std::result::Result<LanguageModelStreamResult, AISdkError> {
        // Compose `do_generate` output into a synthetic stream so the
        // QueryEngine streaming path (which always calls `query_stream`)
        // works against the mock.
        let result = self.do_generate(options, abort_signal).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

// ─── RuntimeConfig + model resolution ────────────────────────────────

/// Derive `RuntimeOverrides` from the parsed CLI flags.
/// Validates numeric flags up-front so a non-positive value can't
/// silently propagate down to the budget tracker (where `<=0` would
/// trigger immediate "budget exhausted" and short-circuit every LLM
/// call to an empty response).
pub fn cli_runtime_overrides(cli: &AgentHostOptions) -> Result<coco_config::RuntimeOverrides> {
    use coco_types::ProviderModelSelection;

    let mut overrides = coco_config::RuntimeOverrides::default();
    if let Some(raw) = cli.models_main.as_deref() {
        overrides.model_override = Some(
            ProviderModelSelection::from_slash_str(raw)
                .map_err(|e| anyhow::anyhow!("--models.main: {e}"))?,
        );
    }
    if let Some(mode) = cli.permission_mode.as_deref()
        && let Ok(pm) = serde_json::from_value::<coco_types::PermissionMode>(
            serde_json::Value::String(mode.to_string()),
        )
    {
        overrides.permission_mode_override = Some(pm);
    }
    overrides.fallback_model_overrides = cli
        .fallback_model
        .iter()
        .map(|raw| {
            ProviderModelSelection::from_slash_str(raw)
                .map_err(|e| anyhow::anyhow!("--fallback-model: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;
    overrides.event_hub_url_override = cli.event_hub_url.clone();
    if let Some(max_tokens) = cli.max_tokens
        && max_tokens <= 0
    {
        anyhow::bail!(
            "--max-tokens must be > 0 (got {max_tokens}); a non-positive value short-circuits \
             the budget tracker and produces empty responses"
        );
    }
    if let Some(max_turns) = cli.max_turns
        && max_turns < 1
    {
        anyhow::bail!(
            "--max-turns must be >= 1 (got {max_turns}); 0 or negative would prevent the \
             agent loop from executing any turn"
        );
    }
    Ok(overrides)
}

/// Build a `RuntimeConfig` honoring CLI-level overrides.
pub fn build_runtime_config_for_cli(
    cli: &AgentHostOptions,
    cwd: &Path,
) -> Result<coco_config::RuntimeConfig> {
    let roots = crate::paths::settings_roots_for_cwd(cwd);
    build_runtime_config_for_cli_with_roots(cli, roots.project_root(), roots.local_root())
}

/// Build a `RuntimeConfig` honoring CLI-level overrides with split
/// project/local settings roots.
pub fn build_runtime_config_for_cli_with_roots(
    cli: &AgentHostOptions,
    project_root: &Path,
    local_root: &Path,
) -> Result<coco_config::RuntimeConfig> {
    let mut builder = coco_config::RuntimeConfigBuilder::from_process(local_root)
        .with_overrides(cli_runtime_overrides(cli)?)
        .with_settings_roots(project_root, local_root)
        .with_setting_sources(cli.setting_sources.clone());
    if let Some(path) = cli.settings.as_deref() {
        builder = builder.with_flag_settings(path);
    }
    Ok(builder.build()?)
}

/// Build a `RuntimeConfig` with a live `RuntimeReloader` so settings.json edits
/// hot-reload (sandbox, …) on the AppServer / headless paths too — not just the TUI.
/// Falls back to a one-shot static build when the reloader can't spawn (e.g.
/// outside a Tokio runtime). Callers must keep the returned reloader alive for
/// the session and ask the session handle to install its sandbox reload
/// supervisor after `SessionRuntime::build`.
pub fn build_runtime_config_with_reloader(
    cli: &AgentHostOptions,
    cwd: &Path,
) -> Result<(
    Option<coco_config_reload::RuntimeReloader>,
    coco_config::RuntimeConfig,
)> {
    let roots = crate::paths::settings_roots_for_cwd(cwd);
    build_runtime_config_with_reloader_roots(cli, roots.project_root(), roots.local_root())
}

/// Build a `RuntimeConfig` with hot-reload and split project/local settings
/// roots.
pub fn build_runtime_config_with_reloader_roots(
    cli: &AgentHostOptions,
    project_root: &Path,
    local_root: &Path,
) -> Result<(
    Option<coco_config_reload::RuntimeReloader>,
    coco_config::RuntimeConfig,
)> {
    let reload_opts = coco_config_reload::ReloadOptions::new(local_root.to_path_buf())
        .with_settings_roots(project_root, local_root)
        .with_overrides(cli_runtime_overrides(cli)?)
        .with_setting_sources(cli.setting_sources.clone());
    let reload_opts = if let Some(path) = cli.settings.as_deref() {
        reload_opts.with_flag_settings(path)
    } else {
        reload_opts
    };
    match coco_config_reload::RuntimeReloader::spawn(reload_opts) {
        Ok(reloader) => {
            let snapshot = reloader.current();
            Ok((Some(reloader), Arc::unwrap_or_clone(snapshot)))
        }
        Err(e) => {
            tracing::warn!(error = %e, "config hot-reload disabled; using one-shot build");
            Ok((
                None,
                build_runtime_config_for_cli_with_roots(cli, project_root, local_root)?,
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedMainModel {
    pub provider: String,
    pub provider_api: Option<coco_types::ProviderApi>,
    pub model_id: String,
    pub supports_prompt_cache: bool,
}

pub fn resolve_main_model(runtime_config: &coco_config::RuntimeConfig) -> ResolvedMainModel {
    use coco_types::ModelRole;

    if let Some(main_spec) = runtime_config.model_roles.get(ModelRole::Main) {
        let supports_prompt_cache = matches!(main_spec.api, coco_types::ProviderApi::Anthropic)
            && runtime_config
                .model_registry
                .resolve(&main_spec.provider, &main_spec.model_id)
                .is_some_and(|model| {
                    model
                        .info
                        .capabilities
                        .as_ref()
                        .is_some_and(|caps| caps.contains(&coco_types::Capability::PromptCache))
                });
        return ResolvedMainModel {
            provider: main_spec.provider.clone(),
            provider_api: Some(main_spec.api),
            model_id: main_spec.model_id.clone(),
            supports_prompt_cache,
        };
    }

    let model = MockModel::new();
    ResolvedMainModel {
        provider: model.provider().to_string(),
        provider_api: None,
        model_id: model.model_id().to_string(),
        supports_prompt_cache: false,
    }
}

// ─── Output style manager ────────────────────────────────────────────

/// Build a [`coco_output_styles::OutputStyleManager`] from settings,
/// the standard on-disk dirs ([`crate::paths::user_output_style_dir`],
/// [`crate::paths::project_output_style_dirs`],
/// [`crate::paths::managed_output_style_dir`]), and the supplied
/// plugin sources.
/// Headless and AppServer paths share this helper so a future addition (e.g.,
/// project-tree ancestor walk) lands in one place. `plugin_sources` are the
/// plugin-contributed output-style directories (see
/// [`coco_app_runtime::ProjectServices::output_style_sources`]).
pub fn build_output_style_manager(
    runtime_config: &coco_config::RuntimeConfig,
    cwd: &Path,
    plugin_sources: &[coco_output_styles::PluginOutputStyleSource],
) -> coco_output_styles::OutputStyleManager {
    coco_output_styles::OutputStyleManager::builder()
        .settings_name(runtime_config.settings.merged.output_style.clone())
        .user_dir(Some(crate::paths::user_output_style_dir()))
        .project_dirs(crate::paths::project_output_style_dirs(cwd))
        .managed_dir(Some(crate::paths::managed_output_style_dir()))
        .plugins(plugin_sources.to_vec())
        .build()
}

// ─── System prompt assembly ──────────────────────────────────────────

/// Convert a resolved [`OutputStyleConfig`] into the borrowed view the
/// `coco-context` prompt builder accepts.
fn output_style_section(
    style: &coco_output_styles::OutputStyleConfig,
) -> coco_context::prompt::OutputStyleSection<'_> {
    coco_context::prompt::OutputStyleSection {
        name: &style.name,
        prompt: &style.prompt,
        // Built-in styles set keep_coding_instructions: Some (true);
        // unset custom/plugin styles default to false, matching the strict
        // `keepCodingInstructions === true` gate.
        keep_coding_instructions: style.keep_coding_instructions.unwrap_or(false),
    }
}

/// Build the system prompt with environment context and CLAUDE.md content.
pub fn build_system_prompt(
    cwd: &Path,
    model_id: &str,
    base_instructions: Option<&str>,
    output_style: Option<&coco_output_styles::OutputStyleConfig>,
    additional_working_directories: &[String],
    include_git_status: bool,
) -> String {
    let claude_files = coco_context::discover_memory_files(cwd);
    let env_info = coco_context::get_environment_info(cwd, model_id, include_git_status);
    let default_identity;
    let identity = if let Some(base_instructions) = base_instructions {
        base_instructions
    } else {
        default_identity = coco_config::default_base_instructions();
        &default_identity
    };
    let section = output_style.map(output_style_section);
    coco_context::build_system_prompt(
        identity,
        &claude_files,
        &env_info,
        None,
        None,
        None,
        section,
        additional_working_directories,
    )
    .full_text()
}

/// Resolve model-specific instructions from runtime config, then build
/// the prompt. Shared by headless, AppServer, and TUI bootstraps.
pub fn build_system_prompt_for_model(
    cwd: &Path,
    runtime_config: &coco_config::RuntimeConfig,
    provider: &str,
    model_id: &str,
    output_style: Option<&coco_output_styles::OutputStyleConfig>,
    additional_working_directories: &[String],
) -> String {
    let resolved = runtime_config.model_registry.resolve(provider, model_id);
    let base_instructions = resolved
        .as_ref()
        .and_then(|model| model.info.base_instructions.as_deref());
    // Point the "Break down and manage your work with the <X> tool" nudge at
    // whichever task tool is actually live. The two are mutually exclusive:
    // TaskV2 on → TaskCreate, off → TodoWrite (see `task_tools.rs::is_enabled`).
    // The default prompt names TaskCreate, so only V1 needs a rewrite. Mirrors
    // `getUsingYourToolsSection`'s `taskToolName = [TaskCreate, TodoWrite]
    // .find (enabled)`; `replace` is a no-op for prompts without the bullet.
    let base_instructions: Option<String> = base_instructions.map(|base| {
        if runtime_config.features.enabled(coco_types::Feature::TaskV2) {
            base.to_string()
        } else {
            base.replace(
                &format!(
                    "with the {} tool",
                    coco_types::ToolName::TaskCreate.as_str()
                ),
                &format!("with the {} tool", coco_types::ToolName::TodoWrite.as_str()),
            )
        }
    });
    // Suppress the git-status block under COCO_REMOTE or a disabled
    // `include_git_instructions` setting (COCO_DISABLE_GIT_INSTRUCTIONS
    // overrides the setting either way).
    let env = coco_config::EnvSnapshot::from_current_process();
    let include_git_status = !env.is_truthy(coco_config::EnvKey::CocoRemote)
        && coco_config::gitsettings::should_include_git_instructions(
            &runtime_config.settings.merged,
            &env,
        );
    build_system_prompt(
        cwd,
        model_id,
        base_instructions.as_deref(),
        output_style,
        additional_working_directories,
        include_git_status,
    )
}

/// Compose the session's system prompt, honoring `--system-prompt`
/// (full override), `--append-system-prompt` (text appended after the
/// default), and `--append-system-prompt-file` (file contents appended).
pub(crate) fn compose_system_prompt(
    cli: &AgentHostOptions,
    cwd: &Path,
    runtime_config: &coco_config::RuntimeConfig,
    provider: &str,
    model_id: &str,
    output_style: Option<&coco_output_styles::OutputStyleConfig>,
) -> Result<String> {
    // 1. Base layer: `--system-prompt` wholly replaces the default
    // identity + CLAUDE.md discovery. Otherwise build the default.
    let additional_dirs = resolve_additional_dirs_display(cli, cwd);
    let mut prompt = if let Some(custom) = cli.system_prompt.as_deref() {
        custom.to_string()
    } else {
        build_system_prompt_for_model(
            cwd,
            runtime_config,
            provider,
            model_id,
            output_style,
            &additional_dirs,
        )
    };
    // 2. Append from `--append-system-prompt` (verbatim).
    if let Some(append) = cli.append_system_prompt.as_deref() {
        if !prompt.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push_str(append);
    }
    // 3. Append from `--append-system-prompt-file` (read once, fail
    // fast if the file's missing rather than silently dropping).
    if let Some(path) = cli.append_system_prompt_file.as_deref() {
        let body = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("--append-system-prompt-file {path:?}: {e}"))?;
        if !prompt.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push_str(&body);
    }
    Ok(prompt)
}

// ─── Permission resolution ───────────────────────────────────────────

/// Resolved startup permission state.
pub struct StartupPermissionState {
    pub mode: coco_types::PermissionMode,
    pub bypass_available: bool,
    /// Whether the classifier-backed `Auto` mode can be cycled into / set.
    /// Default-on, gated only by the `auto_mode.disabled` settings opt-out.
    pub auto_available: bool,
    pub notification: Option<String>,
}

/// Resolve the session's initial `PermissionMode` and the bypass capability.
pub fn resolve_startup_permission_state(
    cli: &AgentHostOptions,
    settings: &coco_config::Settings,
) -> Result<StartupPermissionState> {
    use coco_types::PermissionMode;

    let policy_flag = Some(settings.permissions.disable_bypass_mode);

    let permission_mode_cli = cli.permission_mode.as_deref().and_then(|raw| {
        match serde_json::from_value::<PermissionMode>(serde_json::json!(raw)) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("warning: invalid --permission-mode {raw:?}: {e}; ignoring");
                None
            }
        }
    });

    let resolved = coco_permissions::resolve_initial_permission_mode(
        cli.dangerously_skip_permissions,
        permission_mode_cli,
        settings.permissions.default_mode,
        policy_flag,
    );
    let mode = resolved.mode;

    let bypass_available = coco_permissions::compute_bypass_capability(
        mode == PermissionMode::BypassPermissions,
        cli.allow_dangerously_skip_permissions,
        policy_flag,
    );

    let auto_available = coco_permissions::compute_auto_mode_capability(
        settings.auto_mode.as_ref().is_some_and(|c| c.disabled),
    );

    let requesting_bypass =
        mode == PermissionMode::BypassPermissions || cli.allow_dangerously_skip_permissions;
    enforce_dangerous_skip_safety(requesting_bypass)?;

    Ok(StartupPermissionState {
        mode,
        bypass_available,
        auto_available,
        notification: resolved.notification,
    })
}

fn enforce_dangerous_skip_safety(requesting_bypass: bool) -> Result<()> {
    if !requesting_bypass {
        return Ok(());
    }
    if is_running_as_root() && !is_sandboxed_env() {
        return Err(anyhow::anyhow!(
            "Bypass permissions refuses to run as root/sudo outside a \
             sandbox. Set IS_SANDBOX=1 (or run under bubblewrap) if you \
             know what you're doing."
        ));
    }
    Ok(())
}

/// True when the process runs with effective root privileges (euid 0) — actual
/// root or under `sudo`. Checks the *effective* uid so `sudo coco` is also
/// caught (the prior env-name heuristic — `SUDO_USER`/`USER == root` — was a
/// fragile, spoofable proxy for this). Non-Unix has no uid → false.
fn is_running_as_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` is an always-succeeds libc call — no preconditions,
        // no arguments, no memory effects.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn is_sandboxed_env() -> bool {
    let truthy = |var: &str| -> bool {
        std::env::var(var)
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    };
    truthy("IS_SANDBOX") || coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBubblewrap)
}

// ─── run_chat ────────────────────────────────────────────────────────

/// Outcome of a single headless `coco -p` invocation.
/// Mirrors the data the binary's `main()` would have printed, but
/// returns it structured so tests / embeddings can assert on individual
/// fields.
#[derive(Debug)]
pub struct RunChatOutcome {
    /// Final assistant response text (what the binary prints to stdout).
    pub response_text: String,
    /// Number of agent loop turns executed.
    pub turns: i32,
    /// Total token usage accumulated across the session.
    pub total_usage: TokenUsage,
    /// Per-model cost / token tracking.
    pub cost_tracker: CostTracker,
    /// Resolved model id (provider-side wire name).
    pub model_id: String,
    /// `Some (api)` when a real provider was wired; `None` for mock fallback.
    pub provider_api: Option<coco_types::ProviderApi>,
    /// Resolved permission mode after CLI + settings + killswitch merge.
    pub permission_mode: coco_types::PermissionMode,
    /// `true` when the session is allowed to transition to `BypassPermissions`.
    pub bypass_permissions_available: bool,
    /// Optional notification surfaced when permission resolution downgraded
    /// (e.g. killswitch forced Bypass → AcceptEdits). Caller should print
    /// to stderr.
    pub permission_notification: Option<String>,
    /// Total wall-clock duration in milliseconds.
    pub duration_ms: i64,
    /// API time in milliseconds.
    pub duration_api_ms: i64,
    /// Whether the run hit the budget limit.
    pub budget_exhausted: bool,
    /// Whether the run was cancelled.
    pub cancelled: bool,
    /// Last continue reason from the engine loop.
    pub last_continue_reason: Option<ContinueReason>,
    /// Number of fallback runtime slots installed on the engine.
    /// (from `--fallback-model` flags + `models.<role>.fallbacks`).
    pub installed_fallback_count: usize,
    /// Final message history at session end, including the user prompt,
    /// any tool calls + results, and the final assistant reply. Tests
    /// or embedding callers can feed this into the next [`run_chat_with_options`]
    /// call (`opts.prior_messages = previous.final_messages`) to
    /// continue the conversation through typed `session/start.initial_messages`.
    pub final_messages: Vec<std::sync::Arc<coco_messages::Message>>,
    /// Working directory the engine actually used. Reflects the
    /// effective resolution: `--cwd <flag>` then `RunChatOptions::cwd`.
    pub effective_cwd: PathBuf,
    /// Additional directories declared via `--add-dir` (resolved to
    /// absolute paths). Threaded onto every tool's permission context
    /// so file-system tools may read from them. Empty = no extras.
    pub additional_dirs: Vec<PathBuf>,
    /// Tool filter built from `--allowed-tools` / `--disallowed-tools`.
    /// `None` ⇒ both flags were empty (engine uses `unrestricted()`).
    pub tool_filter_summary: Option<ToolFilterSummary>,
    /// Result of the local AppServer shutdown drain after the print-mode turn.
    pub app_server_shutdown: ShutdownDrainOutcome,
    /// Result of the Event Hub connector shutdown flush after the print-mode turn.
    pub event_hub_shutdown: ShutdownDrainOutcome,
}

/// Lightweight surface of [`coco_types::ToolFilter`] for tests — the
/// underlying type uses `HashSet<ToolId>` whose iteration is
/// non-deterministic, so we project to sorted vectors.
#[derive(Debug, Clone, Default)]
pub struct ToolFilterSummary {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
}

/// Options for [`run_chat_with_options`].
#[derive(Default)]
pub struct RunChatOptions {
    /// Working directory for this run. Required unless the CLI carries
    /// `--cwd`; pass an explicit path to keep parallel tests / embeddings
    /// isolated.
    pub cwd: Option<PathBuf>,
    /// Cancellation token threaded into the engine. When the token is
    /// cancelled mid-run, the engine returns a `cancelled = true`
    /// outcome. `None` = a fresh token is created internally.
    pub cancel: Option<CancellationToken>,
    /// Pre-built message history to seed the conversation. Empty =
    /// start a fresh conversation.
    /// Non-empty fresh runs enter through `session/start.initial_messages`;
    /// production resume enters through `session/resume`.
    pub prior_messages: Vec<std::sync::Arc<coco_messages::Message>>,
    /// Override the engine's session id. Used by `--resume` /
    /// `--continue` / `--fork-session` so the resumed run writes
    /// transcript entries under the source (or fork) session id
    /// instead of a freshly generated `SessionId`. `None` lets
    /// print mode use `--session-id` or mint a fresh id.
    pub session_id_override: Option<coco_types::SessionId>,
    /// Production CLI resume target. When present, startup enters through the
    /// local AppServer `session/resume` lifecycle instead of constructing and
    /// hydrating a runtime directly.
    pub resume_target: Option<coco_types::SessionTarget>,
    /// Stored coordinator/normal mode of the resumed session, used to
    /// reconcile coordinator mode. `None` = no
    /// resume / no stored mode.
    pub stored_mode: Option<String>,
    /// Process-scoped owner for shared runtime managers. Production callers pass
    /// the startup-owned instance; tests/embedders may omit it and get a
    /// call-scoped compatibility runtime.
    pub process_runtime: Option<Arc<ProcessRuntime>>,
}

/// Drive one headless agent run with explicit options.
/// Equivalent to `coco -p "<prompt>"` with the same flag plumbing the
/// binary uses, plus three test-friendly knobs:
/// - `opts.cwd` — explicit cwd used when `--cwd` is not set.
/// - `opts.cancel` — thread an external [`CancellationToken`] for
/// mid-run cancellation.
/// - `opts.prior_messages` — seed fresh process-local runs through
/// `session/start.initial_messages`; production resume uses
/// `session/resume`.
/// Honors these `AgentHostOptions` flags end-to-end:
/// `--models.main`, `--fallback-model`, `--permission-mode`,
/// `--dangerously-skip-permissions` / `--allow-…`, `--max-turns`,
/// `--max-tokens`, `--settings`, `--system-prompt`,
/// `--append-system-prompt`, `--append-system-prompt-file`,
/// `--cwd`, `--add-dir`, `--allowed-tools`, `--disallowed-tools`.
pub async fn run_chat_with_options(
    cli: &AgentHostOptions,
    prompt: Option<&str>,
    opts: RunChatOptions,
) -> Result<RunChatOutcome> {
    let prompt = prompt.unwrap_or("Hello!");
    // Cwd precedence: explicit user `--cwd` flag > `RunChatOptions::cwd`
    // (startup/test/embedder injection).
    let cwd: PathBuf = if let Some(flag) = cli.cwd.as_deref() {
        std::path::Path::new(flag).to_path_buf()
    } else if let Some(p) = opts.cwd {
        p
    } else {
        anyhow::bail!("run_chat_with_options requires RunChatOptions::cwd when --cwd is not set")
    };
    let process_runtime = opts.process_runtime.unwrap_or_else(ProcessRuntime::global);
    // Resolve the session id before any local no-model-turn exits. A
    // print-mode local command should still leave a resumable transcript, and
    // `--session-id` is the automation-facing way to address that session.
    let session_id = if let Some(session_id) = opts.session_id_override.clone() {
        session_id
    } else if let Some(session_id_string) = cli.session_id.clone() {
        coco_types::SessionId::try_new(session_id_string.clone())
            .map_err(|e| anyhow::anyhow!("invalid session id '{session_id_string}': {e}"))?
    } else {
        coco_types::SessionId::generate()
    };
    if let Some(goal_args) = parse_headless_goal_slash(prompt) {
        match coco_commands::parse_goal_command_args(goal_args) {
            Err(text) => {
                return Ok(headless_local_goal_text_outcome(
                    cli,
                    &cwd,
                    &session_id,
                    goal_args,
                    text,
                    opts.prior_messages,
                )
                .await);
            }
            Ok(coco_commands::GoalCommandRequest::Status) => {
                let text =
                    crate::goal_command::format_latest_goal_history_status(&opts.prior_messages)
                        .unwrap_or_else(|| "No goal set. Usage: `/goal <condition>`".to_string());
                return Ok(headless_local_goal_text_outcome(
                    cli,
                    &cwd,
                    &session_id,
                    "",
                    text,
                    opts.prior_messages,
                )
                .await);
            }
            Ok(coco_commands::GoalCommandRequest::Clear) => {
                if crate::goal_command::find_restorable_goal_condition(&opts.prior_messages)
                    .is_none()
                {
                    return Ok(headless_local_goal_text_outcome(
                        cli,
                        &cwd,
                        &session_id,
                        "clear",
                        "No goal set".to_string(),
                        opts.prior_messages,
                    )
                    .await);
                }
            }
            Ok(coco_commands::GoalCommandRequest::Set { .. }) => {}
        }
    }
    tracing::info!(
        target: "coco_agent_host::headless",
        cwd = %cwd.display(),
        prompt_len = prompt.len(),
        has_prior_messages = !opts.prior_messages.is_empty(),
        "headless run starting"
    );

    let runtime_config = build_runtime_config_for_cli(cli, &cwd)?;
    crate::model_card_refresh::spawn_if_enabled(&runtime_config);
    // Reconcile coordinator mode to a resumed session. Flips the env flag
    // before the engine assembles its system prompt below.
    if let Some(warning) = crate::coordinator_mode_resume::reconcile_on_resume(
        opts.stored_mode.as_deref(),
        &runtime_config.features,
    ) {
        eprintln!("{warning}");
    }
    let settings = &runtime_config.settings;

    // Startup marketplace maintenance (seed/reconcile/delist) on the headless
    // surface too; background + non-fatal, mirroring the TUI.
    crate::session_bootstrap::spawn_marketplace_startup(coco_config::global_config::config_home());

    let main_model = resolve_main_model(&runtime_config);
    let provider_api = main_model.provider_api;
    let model_id = main_model.model_id.clone();
    // Use the early-resolved session id so header-template vars
    // (`${SESSION_ID}`), no-model-turn local persistence, and the
    // `SessionRuntime` share one id.
    let model_runtimes = Arc::new(coco_inference::ModelRuntimeRegistry::new(
        Arc::new(runtime_config.clone()),
        Some(crate::provider_login::shared_resolver()),
        Arc::new(coco_inference::HeaderVars {
            session_id: Some(session_id.clone()),
            cwd: cwd.display().to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }),
    )?);
    let installed_fallback_count = runtime_config
        .model_roles
        .fallbacks(coco_types::ModelRole::Main)
        .len();
    let fallback_policy = runtime_config
        .model_roles
        .policy(coco_types::ModelRole::Main);
    tracing::info!(
        target: "coco_agent_host::headless",
        provider = main_model.provider,
        model_id = %model_id,
        real_provider = provider_api.is_some(),
        fallback_count = installed_fallback_count,
        fallback_policy_set = fallback_policy.is_some(),
        "model client resolved"
    );

    let registry = ToolRegistry::new();
    coco_tools::register_all_tools(&registry);

    // The registry is built only for the startup tool-count metric; the
    // per-session fold builds each session's own registry now.
    let tool_count = registry.len();
    let cancel = opts.cancel.unwrap_or_default();

    let startup = resolve_startup_permission_state(cli, &settings.merged)?;
    let permission_mode = startup.mode;
    let bypass_permissions_available = startup.bypass_available;
    tracing::info!(
        target: "coco_agent_host::headless",
        permission_mode = ?permission_mode,
        bypass_available = bypass_permissions_available,
        permission_notification = startup.notification.is_some(),
        tool_count,
        sandbox_mode = ?runtime_config.sandbox.mode,
        "permissions + tools ready"
    );

    // Build the one canonical SessionRuntime — same shape as TUI/AppServer — so the
    // leader engine and every subagent share ONE config, ONE session id, and
    // ONE `wire_engine` install list (agent + task handles, memory_runtime,
    // file_read_state, transcript/usage). Print mode forks subagents from a
    // single context, not a second session container.
    let config_home = coco_config::global_config::config_home();
    let session_manager = Arc::new(coco_session::SessionManager::with_backend(
        runtime_config.settings.merged.session.backend,
        config_home.clone(),
    ));
    let runtime_factory = crate::session_runtime::SessionRuntimeFactory::from_host_config(
        crate::session_runtime::SessionRuntimeFactoryHostConfig {
            cli: Arc::new(cli.clone()),
            cwd: cwd.clone(),
            model_runtimes: Some(model_runtimes),
            session_manager: Arc::clone(&session_manager),
            fast_model_spec: None,
            permission_bridge: None,
            process_runtime: process_runtime.clone(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            // Headless / print: file-history checkpointing defaults OFF.
            is_non_interactive: true,
        },
    );
    let mut local_app_server_bridge =
        crate::app_server_host::AppServerLocalBridge::with_host_inputs_and_server_config(
            crate::app_server_host::HostInputs {
                startup_cwd: Some(cwd.clone()),
                session_manager: Some(Arc::clone(&session_manager)),
                bypass_permissions_available,
                runtime_replacement: Some(crate::app_server_host::RuntimeReplacementContext {
                    runtime_factory,
                    process_runtime: process_runtime.clone(),
                    cwd: cwd.clone(),
                    requires_structured_output: cli.json_schema.is_some(),
                    integration_options: crate::session_bootstrap::SessionIntegrationOptions {
                        lsp: crate::session_bootstrap::SessionLspIntegration::Disabled,
                        mcp_connect: crate::session_bootstrap::SessionMcpConnectMode::Await,
                        late_binds_failure:
                            crate::session_bootstrap::SessionLateBindFailure::WarnAndContinue,
                        ..Default::default()
                    },
                }),
                turn_runner: Some(Arc::new(crate::app_server_host::SessionTurnExecutor::new(
                    None, None,
                ))),
                ..Default::default()
            },
            &runtime_config.server,
        );
    let event_hub_connector =
        crate::event_hub::ProcessEventHub::spawn(&runtime_config, &cwd, Vec::new());
    let event_hub_membership_watcher = event_hub_connector.as_ref().map(|connector| {
        local_app_server_bridge.set_hub_connector_egress(connector.egress());
        crate::event_hub::spawn_app_server_membership_watcher(
            Arc::clone(local_app_server_bridge.app_server()),
            connector.updater(),
        )
    });
    let startup_binding = if let Some(target) = opts.resume_target.clone() {
        local_app_server_bridge
            .resume_interactive_session(coco_types::SessionResumeParams { target }, None)
            .await
            .map_err(|err| anyhow::anyhow!("headless session/resume failed: {err}"))?
    } else {
        local_app_server_bridge
            .start_interactive_session(
                coco_types::SessionStartParams {
                    session_id: Some(session_id.clone()),
                    cwd: Some(cwd.to_string_lossy().into_owned()),
                    model: Some(model_id.clone()),
                    permission_mode: Some(permission_mode),
                    initial_messages: opts
                        .prior_messages
                        .iter()
                        .map(|message| (**message).clone())
                        .collect(),
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(|err| anyhow::anyhow!("headless session/start failed: {err}"))?
    };
    let session_handle = startup_binding.session;
    let session = session_handle.clone();

    // Sandbox hot-reload: re-flow settings.json `sandbox.*` edits into the live
    // SandboxState on the headless/print path through the session-owned config
    // publisher.
    session.install_sandbox_reload_supervisor().await;

    // `StructuredOutput` tool + inline enforcement. Registers into the
    // session's own fold registry, not a discarded startup one.
    session
        .install_structured_output_tool_if_requested(cli.json_schema.as_deref())
        .await?;

    let interactive_target = local_app_server_bridge
        .interactive_session()
        .map(crate::local_client::LocalSessionClient::interactive_target)
        .ok_or_else(|| anyhow::anyhow!("interactive surface was not installed"))?;
    local_app_server_bridge
        .client()
        .keep_alive(local_app_server_bridge.handler())
        .await?;

    let session_id = session.session_id().clone();

    // Bootstrap the per-source permission rule maps; see
    // `crate::permission_rule_loader` for the conversion path. Headless runs
    // honor the same settings.json deny/allow/ask rules as the TUI.
    let (allow_rules, deny_rules, ask_rules) =
        crate::permission_rule_loader::typed_permission_rules(&runtime_config.settings);
    let permission_rule_source_roots =
        crate::permission_rule_loader::permission_rule_source_roots(&runtime_config.settings, &cwd);

    let turn_thinking_level = session.thinking_level().await;
    let permission_mode_availability = coco_types::PermissionModeAvailability::new(
        bypass_permissions_available,
        startup.auto_available,
    );
    // Seed --add-dir + settings additionalDirectories into the session
    // working-dir allowlist. Lives ONLY on the live base now.
    let session_additional_dirs = crate::permission_rule_loader::seed_session_additional_dirs(
        cli,
        &runtime_config.settings,
        &cwd,
    );
    // `--print`: honor `--max-turns` then `loop.max_turns`; unbounded when
    // neither is set.
    let max_turns = cli.max_turns.or(runtime_config.loop_config.max_turns);
    let total_token_budget = cli
        .max_tokens
        .or_else(|| runtime_config.loop_config.total_token_budget.map(i64::from));

    tracing::info!(
        target: "coco_agent_host::headless",
        max_turns = ?max_turns,
        total_token_budget = ?total_token_budget,
        "engine config built"
    );

    // Seed the live permission base from the headless-loaded rule maps (the
    // runtime's bootstrap seed used the un-overridden base). The engine built
    // below shares this `app_state` (app_state_override = None). The rules +
    // dirs live ONLY on the live base now — the config no longer carries them.
    session
        .set_live_permissions(crate::session_runtime::live_permissions(
            permission_mode,
            allow_rules,
            deny_rules,
            ask_rules,
            session_additional_dirs,
            permission_rule_source_roots.clone(),
        ))
        .await;
    session
        .apply_turn_runtime_config(crate::session_runtime::SessionTurnRuntimeConfig {
            is_non_interactive: true,
            avoid_permission_prompts: true,
            permission_mode,
            permission_mode_availability,
            permission_rule_source_roots: permission_rule_source_roots.clone(),
            max_turns,
            total_token_budget,
            cwd_override: Some(cwd.clone()),
            tool_filter: build_tool_filter(cli),
            plans_directory: settings.merged.plans_directory.clone(),
            plan_mode_custom_instructions: cli.plan_mode_instructions.clone(),
        })
        .await;

    let mut effective_prompt = prompt.to_string();
    let mut prefix_messages: Vec<std::sync::Arc<coco_messages::Message>> = Vec::new();
    let prior_messages = opts.prior_messages;

    if let Some(goal_args) = parse_headless_goal_slash(prompt) {
        match coco_commands::parse_goal_command_args(goal_args) {
            Err(text) => {
                append_headless_slash_text(&mut prefix_messages, "goal", goal_args, &text);
                persist_headless_local_transcript_messages(
                    cli,
                    &cwd,
                    &session_id,
                    &prior_messages,
                    &prefix_messages,
                )
                .await;
                let mut final_messages = prior_messages;
                final_messages.extend(prefix_messages);
                return Ok(headless_text_outcome(
                    cli,
                    &cwd,
                    text,
                    final_messages,
                    model_id,
                    provider_api,
                    permission_mode,
                    bypass_permissions_available,
                    startup.notification,
                    installed_fallback_count,
                ));
            }
            Ok(request) => {
                let args = crate::goal_command::goal_display_args(&request).to_string();
                // Headless is non-interactive; the trust gate is deliberately skipped.
                let outcome = crate::goal_command::resolve_goal_request_for_session_with_history(
                    &session,
                    request,
                    &prior_messages,
                    false,
                )
                .await;

                match outcome {
                    crate::goal_command::GoalOutcome::Text(text) => {
                        append_headless_slash_text(&mut prefix_messages, "goal", &args, &text);
                        persist_headless_local_transcript_messages(
                            cli,
                            &cwd,
                            &session_id,
                            &prior_messages,
                            &prefix_messages,
                        )
                        .await;
                        let mut final_messages = prior_messages;
                        final_messages.extend(prefix_messages);
                        return Ok(headless_text_outcome(
                            cli,
                            &cwd,
                            text,
                            final_messages,
                            model_id,
                            provider_api,
                            permission_mode,
                            bypass_permissions_available,
                            startup.notification,
                            installed_fallback_count,
                        ));
                    }
                    crate::goal_command::GoalOutcome::StatusThenText { status, text } => {
                        append_headless_goal_status(&mut prefix_messages, status);
                        crate::goal_command::persist_active_goal_snapshot(&session).await;
                        append_headless_slash_text(&mut prefix_messages, "goal", &args, &text);
                        persist_headless_local_transcript_messages(
                            cli,
                            &cwd,
                            &session_id,
                            &prior_messages,
                            &prefix_messages,
                        )
                        .await;
                        let mut final_messages = prior_messages;
                        final_messages.extend(prefix_messages);
                        return Ok(headless_text_outcome(
                            cli,
                            &cwd,
                            text,
                            final_messages,
                            model_id,
                            provider_api,
                            permission_mode,
                            bypass_permissions_available,
                            startup.notification,
                            installed_fallback_count,
                        ));
                    }
                    crate::goal_command::GoalOutcome::SetAndRun {
                        status,
                        text,
                        kickoff,
                    } => {
                        append_headless_goal_status(&mut prefix_messages, status);
                        crate::goal_command::persist_active_goal_snapshot(&session).await;
                        append_headless_slash_text(&mut prefix_messages, "goal", &args, &text);
                        effective_prompt = kickoff;
                    }
                }
            }
        }
    }

    if !prefix_messages.is_empty() {
        session
            .replace_history_with_arc_messages(
                prior_messages
                    .iter()
                    .chain(prefix_messages.iter())
                    .cloned()
                    .collect(),
            )
            .await;
    }

    // Interrupt the print-mode turn on caller cancellation OR an OS signal
    // (SIGINT/SIGTERM). Without the signal arm, `kill <pid>` during a print
    // turn hits the default terminate action instead of a graceful interrupt
    //.
    let cancel_monitor = {
        let cancel = cancel.clone();
        let client = local_app_server_bridge.connect_local_client();
        let handler = local_app_server_bridge.handler().clone();
        let target = interactive_target.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = crate::shutdown::os_interrupt_signal() => {}
            }
            let _ = client.turn_interrupt(&handler, target).await;
        })
    };

    let completion = local_app_server_bridge
        .start_turn_and_wait_for_end(
            session_id.clone(),
            coco_types::TurnStartParams {
                target: interactive_target,
                prompt: effective_prompt,
                history_override: Vec::new(),
                images: Vec::new(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: Some(permission_mode),
                thinking_level: turn_thinking_level,
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    cancel_monitor.abort();

    let session_result = completion.session_result;

    // Wait for scheduled turn-end extraction/session-memory work before
    // returning so partial writes aren't dropped on process exit. Auto-dream
    // remains fire-and-forget like TS.
    crate::shutdown::drain_session_memory(&session).await;

    // Persist coordinator mode at end-of-run so a later `--resume` re-derives
    // the role.
    crate::shutdown::persist_session_resume_mode(&session).await;

    let additional_dirs = resolve_additional_dirs(cli, &cwd);
    let tool_filter_summary = summarize_tool_filter(cli);
    let usage_snapshot = session.session_usage_snapshot().await;
    let cost_tracker = CostTracker::from_snapshot(usage_snapshot);
    let final_messages = session.history_messages().await;
    let response_text = session_result.result.clone().unwrap_or_else(|| {
        final_messages
            .iter()
            .rev()
            .find_map(|message| match message.as_ref() {
                coco_messages::Message::Assistant(assistant) => match &assistant.message {
                    coco_messages::LlmMessage::Assistant { content, .. } => {
                        content.iter().find_map(|part| match part {
                            coco_messages::AssistantContent::Text(text) => Some(text.text.clone()),
                            _ => None,
                        })
                    }
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or_default()
    });
    let budget_exhausted = matches!(
        completion.ended.outcome,
        coco_types::TurnOutcome::BudgetExhausted(_)
    );
    let cancelled = matches!(
        completion.ended.outcome,
        coco_types::TurnOutcome::Interrupted(_)
    );
    let shutdown_timeout = Duration::from_secs(runtime_config.server.shutdown_timeout_secs as u64);
    let shutdown = ShutdownCoordinator::new("headless", shutdown_timeout)
        .drain_app_server_and_event_hub(
            local_app_server_bridge.shutdown_registered_sessions(),
            event_hub_connector,
            event_hub_membership_watcher,
        )
        .await;

    Ok(RunChatOutcome {
        effective_cwd: cwd.clone(),
        additional_dirs,
        tool_filter_summary,
        app_server_shutdown: shutdown.app_server,
        event_hub_shutdown: shutdown.event_hub,
        response_text,
        turns: session_result.total_turns,
        total_usage: session_result.usage,
        cost_tracker,
        model_id,
        provider_api,
        permission_mode,
        bypass_permissions_available,
        permission_notification: startup.notification,
        duration_ms: session_result.duration_ms,
        duration_api_ms: session_result.duration_api_ms,
        budget_exhausted,
        cancelled,
        last_continue_reason: None,
        installed_fallback_count,
        final_messages,
    })
}

#[cfg(test)]
#[path = "headless.test.rs"]
mod tests;
