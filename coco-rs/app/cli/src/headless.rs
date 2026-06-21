//! Headless (`coco -p "<prompt>"`) entry point exposed as a library
//! function so live tests, embeddings, and the binary all drive the
//! same code path.
//!
//! `run_chat` returns a structured [`RunChatOutcome`] instead of
//! printing to stdout. The binary's `main()` thin-wraps this and
//! formats stdout from the outcome.
//!
//! Helpers shared by `run_chat` and the SDK runner (`MockModel`,
//! `resolve_main_model`, `cli_runtime_overrides`,
//! `build_runtime_config_for_cli`, `build_system_prompt[_for_model]`,
//! `resolve_startup_permission_state`) live here as well, so a test
//! can drive any of them in isolation.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use anyhow::Result;
use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use coco_messages::CostTracker;
use coco_query::ContinueReason;
use coco_tool_runtime::ToolRegistry;
use coco_types::TokenUsage;
use tokio_util::sync::CancellationToken;

use crate::Cli;

/// Fallback base instructions used when a resolved `ModelInfo`
/// declares no `base_instructions` (e.g. Claude built-ins and any
/// user-added non-builtin model in `~/.coco/providers.json` /
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
///
/// Validates numeric flags up-front so a non-positive value can't
/// silently propagate down to the budget tracker (where `<=0` would
/// trigger immediate "budget exhausted" and short-circuit every LLM
/// call to an empty response).
pub fn cli_runtime_overrides(cli: &Cli) -> Result<coco_config::RuntimeOverrides> {
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
pub fn build_runtime_config_for_cli(cli: &Cli, cwd: &Path) -> Result<coco_config::RuntimeConfig> {
    let mut builder = coco_config::RuntimeConfigBuilder::from_process(cwd)
        .with_overrides(cli_runtime_overrides(cli)?)
        .with_setting_sources(cli.setting_sources.clone());
    if let Some(path) = cli.settings.as_deref() {
        builder = builder.with_flag_settings(path);
    }
    Ok(builder.build()?)
}

/// Build a `RuntimeConfig` with a live `RuntimeReloader` so settings.json edits
/// hot-reload (sandbox, …) on the SDK / headless paths too — not just the TUI.
/// Falls back to a one-shot static build when the reloader can't spawn (e.g.
/// outside a Tokio runtime). Callers must keep the returned reloader alive for
/// the session and attach `sandbox_reload::spawn_sandbox_reload` after
/// `SessionRuntime::build`.
pub fn build_runtime_config_with_reloader(
    cli: &Cli,
    cwd: &Path,
) -> Result<(
    Option<coco_config_reload::RuntimeReloader>,
    coco_config::RuntimeConfig,
)> {
    let reload_opts = coco_config_reload::ReloadOptions::new(cwd.to_path_buf())
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
            Ok((None, build_runtime_config_for_cli(cli, cwd)?))
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
///
/// Headless and SDK paths share this helper so a future addition (e.g.,
/// project-tree ancestor walk) lands in one place. `plugin_sources` are the
/// plugin-contributed output-style directories (see
/// [`crate::session_bootstrap::plugin_output_style_sources`]).
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
        // Built-in styles set keep_coding_instructions: Some(true);
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
    let identity = base_instructions.unwrap_or(DEFAULT_SYSTEM_PROMPT_IDENTITY);
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
/// the prompt. Shared by headless, SDK, and TUI bootstraps.
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
    // TS `getUsingYourToolsSection`'s `taskToolName = [TaskCreate, TodoWrite]
    // .find(enabled)`; `replace` is a no-op for prompts without the bullet.
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
    cli: &Cli,
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

/// Reject requesting bypass when the host is not a sandbox.
/// Parse `--json-schema` (if set) and register the synthetic
/// `StructuredOutput` tool against `registry` + a matching Stop
/// function hook on `hook_registry`.
///
/// Only the non-interactive bootstrap (headless print mode / SDK
/// NDJSON) calls this; TUI must not, by design — the tool is excluded
/// from `register_all_tools` and only installed through this helper.
///
/// Returns `Ok(true)` when the flag was set and both the tool and
/// Stop hook were registered. Returns `Ok(false)` when the flag was
/// absent (caller proceeds without structured output). Returns
/// `Err(_)` when:
///   - `--json-schema` is not valid JSON
///   - the parsed value fails JSON-Schema meta-validation
///   - the Stop function hook fails to register (programmer error —
///     duplicate id, unsupported event)
pub fn inject_structured_output_tool_if_requested(
    cli: &Cli,
    registry: &ToolRegistry,
    hook_registry: &coco_hooks::HookRegistry,
) -> Result<bool> {
    let Some(raw) = cli.json_schema.as_deref() else {
        return Ok(false);
    };
    let schema: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("--json-schema is not valid JSON: {e}"))?;
    coco_tools::register_structured_output_tool(registry, schema)
        .map_err(|e| anyhow::anyhow!("--json-schema rejected: {e}"))?;

    // Block the model from ending its turn until it has pushed at least
    // one valid `StructuredOutput` attachment into history. Uses the
    // typed AttachmentKind directly instead of a fragile
    // `hasSuccessfulToolCall(name)` scan.
    hook_registry
        .register_function_hook(
            format!("structured-output-enforcement-{}", uuid::Uuid::new_v4()),
            coco_types::HookEventType::Stop,
            None,
            std::time::Duration::from_millis(5_000),
            std::sync::Arc::new(
                coco_query::structured_output_enforcement::StructuredOutputEnforcement,
            ),
            format!(
                "You MUST call the {} tool to complete this request. Call this tool now.",
                coco_types::ToolName::StructuredOutput.as_str()
            ),
        )
        .map_err(|e| anyhow::anyhow!("failed to register StructuredOutput Stop hook: {e}"))?;

    tracing::info!(
        target: "coco_cli::headless",
        "registered StructuredOutput tool + Stop enforcement hook from --json-schema"
    );
    Ok(true)
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
///
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
    /// `Some(api)` when a real provider was wired; `None` for mock fallback.
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
    /// continue the conversation in-process.
    pub final_messages: Vec<std::sync::Arc<coco_messages::Message>>,
    /// Working directory the engine actually used. Reflects the
    /// effective resolution: `--cwd <flag>` then `RunChatOptions::cwd`
    /// then `std::env::current_dir()`. Useful for tests asserting the
    /// flag-precedence rule.
    pub effective_cwd: PathBuf,
    /// Additional directories declared via `--add-dir` (resolved to
    /// absolute paths). Threaded onto every tool's permission context
    /// so file-system tools may read from them. Empty = no extras.
    pub additional_dirs: Vec<PathBuf>,
    /// Tool filter built from `--allowed-tools` / `--disallowed-tools`.
    /// `None` ⇒ both flags were empty (engine uses `unrestricted()`).
    pub tool_filter_summary: Option<ToolFilterSummary>,
}

/// Lightweight surface of [`coco_types::ToolFilter`] for tests — the
/// underlying type uses `HashSet<ToolId>` whose iteration is
/// non-deterministic, so we project to sorted vectors.
#[derive(Debug, Clone, Default)]
pub struct ToolFilterSummary {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
}

/// Options for [`run_chat_with_options`]. All fields default to the
/// same behavior as `run_chat`.
#[derive(Default)]
pub struct RunChatOptions {
    /// Override the working directory for this run. When `None`, the
    /// process-global `std::env::current_dir()` is used. Pass an
    /// explicit path to keep parallel tests / embeddings isolated.
    pub cwd: Option<PathBuf>,
    /// Cancellation token threaded into the engine. When the token is
    /// cancelled mid-run, the engine returns a `cancelled = true`
    /// outcome. `None` = a fresh token is created internally.
    pub cancel: Option<CancellationToken>,
    /// Pre-built message history to seed the conversation. Empty =
    /// start a fresh conversation (the default `run_chat` behavior).
    /// Non-empty = continue from the prior turns; the engine drives
    /// `run_with_messages(prior + user_prompt)` instead of `run`.
    pub prior_messages: Vec<std::sync::Arc<coco_messages::Message>>,
    /// Override the engine's session id. Used by `--resume` /
    /// `--continue` / `--fork-session` so the resumed run writes
    /// transcript entries under the source (or fork) session id
    /// instead of a fresh per-process uuid. `None` keeps the
    /// engine's default empty-session-id behavior.
    pub session_id_override: Option<String>,
    /// Stored coordinator/normal mode of the resumed session, used to
    /// reconcile coordinator mode (TS `matchSessionMode`). `None` = no
    /// resume / no stored mode.
    pub stored_mode: Option<String>,
}

/// Drive one headless agent run with default options. See
/// [`run_chat_with_options`] for cwd / cancellation / session-continuation.
pub async fn run_chat(cli: &Cli, prompt: Option<&str>) -> Result<RunChatOutcome> {
    run_chat_with_options(cli, prompt, RunChatOptions::default()).await
}

/// Drive one headless agent run with explicit options.
///
/// Equivalent to `coco -p "<prompt>"` with the same flag plumbing the
/// binary uses, plus three test-friendly knobs:
///
/// - `opts.cwd` — override `std::env::current_dir()` so parallel
///   embeddings / tests stay isolated.
/// - `opts.cancel` — thread an external [`CancellationToken`] for
///   mid-run cancellation.
/// - `opts.prior_messages` — seed the conversation with a previous
///   `RunChatOutcome.final_messages`, simulating `--continue` /
///   `--resume` in-process.
///
/// Honors these `Cli` flags end-to-end:
/// `--models.main`, `--fallback-model`, `--permission-mode`,
/// `--dangerously-skip-permissions` / `--allow-…`, `--max-turns`,
/// `--max-tokens`, `--settings`, `--system-prompt`,
/// `--append-system-prompt`, `--append-system-prompt-file`,
/// `--cwd`, `--add-dir`, `--allowed-tools`, `--disallowed-tools`.
pub async fn run_chat_with_options(
    cli: &Cli,
    prompt: Option<&str>,
    opts: RunChatOptions,
) -> Result<RunChatOutcome> {
    let prompt = prompt.unwrap_or("Hello!");
    // Cwd precedence: explicit user `--cwd` flag > `RunChatOptions::cwd`
    // (test/embedder injection) > `std::env::current_dir()`.
    let cwd: PathBuf = if let Some(flag) = cli.cwd.as_deref() {
        std::path::Path::new(flag).to_path_buf()
    } else if let Some(p) = opts.cwd {
        p
    } else {
        std::env::current_dir()?
    };
    tracing::info!(
        target: "coco_cli::headless",
        cwd = %cwd.display(),
        prompt_len = prompt.len(),
        has_prior_messages = !opts.prior_messages.is_empty(),
        "headless run starting"
    );

    let (sandbox_reloader, runtime_config) = build_runtime_config_with_reloader(cli, &cwd)?;
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

    // Load the plugin set once and reuse for output styles + command/skill
    // registration. Resolve the active output style here — fed into the system
    // prompt builder + threaded onto `SessionBootstrap` for the per-turn
    // reminder generator. Plugin-contributed styles are folded in alongside
    // user / project / managed dirs.
    let plugins = crate::session_bootstrap::load_session_plugins(&cwd);
    // Startup marketplace maintenance (seed/reconcile/delist) on the headless
    // surface too; background + non-fatal, mirroring the TUI.
    crate::session_bootstrap::spawn_marketplace_startup(coco_config::global_config::config_home());
    let plugin_style_sources = crate::session_bootstrap::plugin_output_style_sources(&plugins);
    let output_style_manager =
        build_output_style_manager(&runtime_config, &cwd, &plugin_style_sources);
    let active_output_style = output_style_manager.active().cloned();

    let main_model = resolve_main_model(&runtime_config);
    let provider_api = main_model.provider_api;
    let model_id = main_model.model_id.clone();
    // Resolve the session id up front so the registry's header-template vars
    // (`${SESSION_ID}`) and the `SessionRuntime` share one id. For
    // resume/continue/fork the override is already `Some`; a fresh run mints
    // one here and threads it into the build opts below.
    let session_id = opts
        .session_id_override
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let model_runtimes = Arc::new(coco_inference::ModelRuntimeRegistry::new(
        Arc::new(runtime_config.clone()),
        Some(crate::provider_login::shared_resolver()),
        Arc::new(coco_inference::HeaderVars {
            session_id: session_id.clone(),
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
        target: "coco_cli::headless",
        provider = main_model.provider,
        model_id = %model_id,
        real_provider = provider_api.is_some(),
        fallback_count = installed_fallback_count,
        fallback_policy_set = fallback_policy.is_some(),
        "model client resolved"
    );

    let registry = ToolRegistry::new();
    coco_tools::register_all_tools(&registry);

    let tool_count = registry.len();
    let tools = Arc::new(registry);
    let cancel = opts.cancel.unwrap_or_default();

    let startup = resolve_startup_permission_state(cli, &settings.merged)?;
    let permission_mode = startup.mode;
    let bypass_permissions_available = startup.bypass_available;
    tracing::info!(
        target: "coco_cli::headless",
        permission_mode = ?permission_mode,
        bypass_available = bypass_permissions_available,
        permission_notification = startup.notification.is_some(),
        tool_count,
        sandbox_mode = ?runtime_config.sandbox.mode,
        "permissions + tools ready"
    );

    let system_prompt = compose_system_prompt(
        cli,
        &cwd,
        &runtime_config,
        &main_model.provider,
        &model_id,
        active_output_style.as_ref(),
    )?;

    // Build the one canonical SessionRuntime — same shape as TUI/SDK — so the
    // leader engine and every subagent share ONE config, ONE session id, and
    // ONE `wire_engine` install list (agent + task handles, memory_runtime,
    // file_read_state, transcript/usage). Print mode forks subagents from a
    // single context, not a second session container.
    let config_home = coco_config::global_config::config_home();
    let (command_registry, skill_manager) =
        crate::session_bootstrap::build_session_command_registry(
            cli,
            &runtime_config,
            &cwd,
            &plugins,
        );
    let runtime = crate::session_runtime::SessionRuntime::build(
        crate::session_runtime::SessionRuntimeBuildOpts {
            cli,
            runtime_config: Arc::new(runtime_config.clone()),
            cwd: cwd.clone(),
            model_id: model_id.clone(),
            system_prompt,
            bypass_permissions_available,
            permission_mode,
            model_runtimes: Some(model_runtimes),
            tools: tools.clone(),
            session_manager: Arc::new(coco_session::SessionManager::with_backend(
                runtime_config.settings.merged.session.backend,
                config_home.clone(),
            )),
            fast_model_spec: None,
            permission_bridge: None,
            command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(command_registry))),
            skill_manager,
            agent_search_paths: crate::paths::standard_agent_search_paths(&config_home, &cwd),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            // Resume / continue / fork: key every runtime subsystem off the
            // resumed id, else task dirs + agent transcripts orphan. Resolved
            // above (override or freshly minted) and shared with the registry's
            // header-template vars.
            session_id_override: Some(session_id.clone()),
            // Headless / print: file-history checkpointing defaults OFF.
            is_non_interactive: true,
        },
    )
    .await?;

    // Sandbox hot-reload: re-flow settings.json `sandbox.*` edits into the live
    // SandboxState on the headless/print path too (TS `sandbox-adapter` covers
    // REPL and print/SDK alike). The task exits when the reloader drops at the
    // end of this function. Held in `_sandbox_reload` for the session lifetime.
    let _sandbox_reload = match (sandbox_reloader.as_ref(), runtime.sandbox_state()) {
        (Some(reloader), Some(state)) => Some(crate::sandbox_reload::spawn_sandbox_reload(
            state,
            &reloader.publisher(),
            cwd.clone(),
        )),
        _ => None,
    };

    // `StructuredOutput` tool + Stop hook. The tool registers into the shared
    // `tools` Arc; the Stop hook MUST target the runtime's hook registry —
    // the one its engines dispatch from.
    inject_structured_output_tool_if_requested(cli, tools.as_ref(), &runtime.hook_registry())?;

    // Agent/task spawning infra (TaskRuntime + agent team + worktree manager +
    // fork dispatcher), unconditional like TUI/SDK. Best-effort: a transient
    // task-dir / worktree-discovery failure must NOT kill a print run that
    // never spawns anything — degrade to NoOp handles instead.
    if let Err(e) = crate::session_bootstrap::install_session_late_binds(
        runtime.clone(),
        &cwd,
        None,
        None,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, "agent/task infrastructure unavailable in headless; spawns degrade");
    }
    // Unified MCP bootstrap: load config-file + plugin MCP servers. Headless is
    // single-turn, so await the connect batch — MCP tools must be registered
    // before the first (only) turn.
    crate::session_bootstrap::bootstrap_session_mcp(
        &runtime, &cwd, None, /*await_connect*/ true,
    )
    .await;

    // Leader-side teammate inbox consumption: drives `ShutdownApproved`
    // → teardown so a headless leader doesn't leak stale team membership /
    // orphaned tasks. No human UI ⇒ no permission bridge. Covers long-running
    // headless (stream-json input); a single-shot `-p` leader exits before the
    // 1 s poll fires — that bounded end-of-run drain is a documented follow-up.
    crate::leader_inbox_poller::install_leader(runtime.clone(), None).await;

    let session_id = runtime.current_session_id().await;

    // Resume hydration: seed transcript dedup + tool-result replacement onto
    // the runtime so `wire_engine` installs them on the engine and the resumed
    // prior messages are not re-written to the JSONL. Session usage for the
    // resumed id is loaded by `SessionRuntime::build` and flushed by the
    // engine's per-turn finalize — no manual flush needed.
    if opts.session_id_override.is_some() {
        runtime
            .seed_transcript_dedup(opts.prior_messages.iter().filter_map(|m| m.uuid().copied()))
            .await;
        let prior: Vec<coco_messages::Message> =
            opts.prior_messages.iter().map(|m| (**m).clone()).collect();
        runtime
            .seed_tool_result_replacement_state(&prior, &session_id, None)
            .await;
    }

    // Bootstrap the per-source permission rule maps; see
    // `crate::permission_rule_loader` for the conversion path. Headless runs
    // honor the same settings.json deny/allow/ask rules as the TUI.
    let (allow_rules, deny_rules, ask_rules) =
        crate::permission_rule_loader::typed_permission_rules(&runtime_config.settings);
    let permission_rule_source_roots =
        crate::permission_rule_loader::permission_rule_source_roots(&runtime_config.settings, &cwd);

    // Per-turn engine config: start from the runtime's base (memory-augmented
    // system prompt, sandbox_state, model / cache / compact / features …) then
    // layer the CLI-specific overrides the runtime base doesn't carry,
    // mirroring the SDK runner. Built through the runtime so `wire_engine`
    // installs the full handle/subsystem set on the leader.
    let mut config = runtime.current_engine_config().await;
    // `coco -p` is a one-shot run with no interactive prompt.
    // `is_non_interactive` drives the session-level side effects (self-fork
    // suppression, "sdk" label, prompt assembly) — TS `getIsNonInteractiveSession()`.
    config.is_non_interactive = true;
    // `avoid_permission_prompts` is the separate permission concept: with no
    // UI to prompt, the auto-mode classifier's `require_interactive_or_deny`
    // and the permission controller's no-bridge fallback DENY rather than
    // silently auto-allow. Kept distinct so a future consumer-backed
    // print/SDK mode could stay non-interactive while still routing `Ask`
    // to a `canUseTool` callback.
    config.avoid_permission_prompts = true;
    config.session_id = session_id.clone();
    config.permission_mode = permission_mode;
    config.bypass_permissions_available = bypass_permissions_available;
    config.permission_rule_source_roots = permission_rule_source_roots.clone();
    // Seed --add-dir + settings additionalDirectories into the session
    // working-dir allowlist. Lives ONLY on the live base now.
    let session_additional_dirs = crate::permission_rule_loader::seed_session_additional_dirs(
        cli,
        &runtime_config.settings,
        &cwd,
    );
    // `--print`: honor `--max-turns` then `loop.max_turns`; unbounded when
    // neither is set.
    config.max_turns = cli.max_turns.or(runtime_config.loop_config.max_turns);
    config.total_token_budget = cli
        .max_tokens
        .or_else(|| runtime_config.loop_config.total_token_budget.map(i64::from));
    config.cwd_override = Some(cwd.clone());
    config.tool_filter = build_tool_filter(cli);
    config.plans_directory = settings.merged.plans_directory.clone();

    tracing::info!(
        target: "coco_cli::headless",
        max_turns = ?config.max_turns,
        total_token_budget = ?config.total_token_budget,
        context_window = config.context_window,
        streaming_tools = config.streaming_tool_execution,
        plan_mode = ?config.plan_mode_settings,
        "engine config built"
    );

    // Seed the live permission base from the headless-loaded rule maps (the
    // runtime's bootstrap seed used the un-overridden base). The engine built
    // below shares this `app_state` (app_state_override = None). The rules +
    // dirs live ONLY on the live base now — the config no longer carries them.
    runtime.app_state.write().await.permissions = crate::session_runtime::live_permissions(
        permission_mode,
        allow_rules,
        deny_rules,
        ask_rules,
        session_additional_dirs,
        permission_rule_source_roots,
    );

    let engine = runtime.build_engine_from_config(config, cancel, None).await;

    // Resolve `@`-mentions in the prompt to file-content system-reminder
    // messages. Both branches below share one expansion pipeline so
    // headless behaves like TUI / SDK.
    let inputs = crate::at_mention_turn::resolve_turn_inputs_text_only(
        prompt,
        &cwd,
        &runtime.file_read_state,
    )
    .await;
    let new_turn_messages = crate::at_mention_turn::build_messages_for_turn(&inputs);
    let messages: Vec<std::sync::Arc<coco_messages::Message>> = if opts.prior_messages.is_empty() {
        new_turn_messages
            .into_iter()
            .map(std::sync::Arc::new)
            .collect()
    } else {
        let mut combined = opts.prior_messages;
        combined.extend(new_turn_messages.into_iter().map(std::sync::Arc::new));
        combined
    };
    if !inputs.mentioned_paths.is_empty() {
        engine
            .note_mentioned_paths(inputs.mentioned_paths.clone())
            .await;
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    // Drain events to /dev/null — callers wanting events should drop
    // down to `coco_query::QueryEngine::run_with_events` directly.
    let drainer = tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
    let result = engine
        .run_with_messages(messages, event_tx, coco_types::TurnId::generate())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    drainer.abort();

    // Wait for any in-flight auto-memory extraction + session-memory
    // fork to complete before we return so partial writes aren't dropped
    // on process exit. Drains extraction (60 s) and session memory
    // (15 s) so a half-written `summary.md` doesn't survive into the
    // next `--resume`.
    if let Some(memory_runtime) = engine.memory_runtime() {
        let _ = memory_runtime
            .extract
            .drain(coco_memory::service::extract::DEFAULT_DRAIN_TIMEOUT)
            .await;
        let _ = memory_runtime
            .session_memory
            .wait_for_extraction(coco_memory::service::session::DEFAULT_WAIT_TIMEOUT)
            .await;
    }

    // Persist coordinator mode at end-of-run so a later `--resume` re-derives
    // the role.
    {
        let session_id = runtime.current_session_id().await;
        crate::coordinator_mode_resume::persist_session_mode(
            &runtime.session_manager,
            &session_id,
            &runtime.runtime_config.features,
        );
    }

    let additional_dirs = resolve_additional_dirs(cli, &cwd);
    let tool_filter_summary = summarize_tool_filter(cli);

    Ok(RunChatOutcome {
        effective_cwd: cwd.clone(),
        additional_dirs,
        tool_filter_summary,
        response_text: result.response_text,
        turns: result.turns,
        total_usage: result.total_usage,
        cost_tracker: result.cost_tracker,
        model_id,
        provider_api,
        permission_mode,
        bypass_permissions_available,
        permission_notification: startup.notification,
        duration_ms: result.duration_ms,
        duration_api_ms: result.duration_api_ms,
        budget_exhausted: result.budget_exhausted,
        cancelled: result.cancelled,
        last_continue_reason: result.last_continue_reason,
        installed_fallback_count,
        final_messages: result.final_messages,
    })
}

/// Compose the session's system prompt, honoring `--system-prompt`
/// (full override), `--append-system-prompt` (text appended after the
/// default), and `--append-system-prompt-file` (file contents appended).
fn compose_system_prompt(
    cli: &Cli,
    cwd: &Path,
    runtime_config: &coco_config::RuntimeConfig,
    provider: &str,
    model_id: &str,
    output_style: Option<&coco_output_styles::OutputStyleConfig>,
) -> Result<String> {
    // 1. Base layer: `--system-prompt` wholly replaces the default
    //    identity + CLAUDE.md discovery. Otherwise build the default.
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
    //    fast if the file's missing rather than silently dropping).
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

/// Translate `--allowed-tools` / `--disallowed-tools` into a
/// [`coco_types::ToolFilter`]. Empty inputs ⇒ `unrestricted()`.
fn build_tool_filter(cli: &Cli) -> coco_types::ToolFilter {
    if cli.allowed_tools.is_empty() && cli.disallowed_tools.is_empty() {
        return coco_types::ToolFilter::unrestricted();
    }
    coco_types::ToolFilter::new(cli.allowed_tools.clone(), cli.disallowed_tools.clone())
}

/// Lightweight summary of the resolved tool filter for [`RunChatOutcome`].
/// Returns `None` when both `--allowed-tools` and `--disallowed-tools`
/// are empty (caller can equate that with `unrestricted`).
fn summarize_tool_filter(cli: &Cli) -> Option<ToolFilterSummary> {
    if cli.allowed_tools.is_empty() && cli.disallowed_tools.is_empty() {
        return None;
    }
    let mut allowed = cli.allowed_tools.clone();
    let mut disallowed = cli.disallowed_tools.clone();
    allowed.sort();
    disallowed.sort();
    Some(ToolFilterSummary {
        allowed,
        disallowed,
    })
}

/// Resolve `--add-dir` flag values to absolute paths anchored at `cwd`.
/// Used internally by `compose_system_prompt` to anchor `--add-dir`
/// paths for fence checks; callers that need the rendered display form
/// for the env block should use [`resolve_additional_dirs_display`].
pub(crate) fn resolve_additional_dirs(cli: &Cli, cwd: &Path) -> Vec<PathBuf> {
    cli.add_dir
        .iter()
        .map(|raw| {
            let p = Path::new(raw);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(p)
            }
        })
        .collect()
}

/// Public sibling of [`resolve_additional_dirs`] returning the display
/// form (`String`) that flows into `coco_context::build_system_prompt`'s
/// `additional_working_directories` slot. Single source of truth for the
/// `--add-dir` → env-block transformation; previously duplicated in
/// `session_bootstrap.rs` and `headless::compose_system_prompt`.
pub fn resolve_additional_dirs_display(cli: &Cli, cwd: &Path) -> Vec<String> {
    resolve_additional_dirs(cli, cwd)
        .iter()
        .map(|p| p.display().to_string())
        .collect()
}
