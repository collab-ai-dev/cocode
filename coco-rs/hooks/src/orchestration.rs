//! Hook orchestration — parallel execution, result aggregation, and env injection.
//!
//!
//! This module provides the high-level orchestration layer that:
//! 1. Builds structured hook inputs (serialized as JSON for command stdin)
//! 2. Injects environment variables (tool name, session ID, CWD, etc.)
//! 3. Executes matching hooks in parallel with per-hook timeouts
//! 4. Parses hook stdout as JSON or plain text
//! 5. Aggregates results into a single `AggregatedHookResult`

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use coco_config::EnvKey;
use coco_config::env;
use coco_messages::AttachmentEmitter;
use coco_messages::AttachmentMessage;
use coco_messages::HookCancelledPayload;
use coco_messages::HookErrorDuringExecutionPayload;
use coco_messages::HookNonBlockingErrorPayload;
use coco_types::HookEventType;
use coco_types::HookOutcome;
use coco_types::PermissionBehavior;

use crate::HookExecutionResult;
use crate::HookHandler;
use crate::HookRegistry;
use crate::execute_hook;

/// Default timeout for tool-related hook execution (10 minutes).
const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Default timeout for Prompt (LLM) hooks (30 seconds), independent of
/// the generic tool-hook timeout.
///
const DEFAULT_PROMPT_HOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for Agent (LLM-judge) hooks (60 seconds), independent
/// of the generic tool-hook timeout.
///
const DEFAULT_AGENT_HOOK_TIMEOUT: Duration = Duration::from_secs(60);

/// Default timeout for SessionEnd hooks (1.5 seconds).
///
const SESSION_END_HOOK_TIMEOUT: Duration = Duration::from_millis(1500);

// ---------------------------------------------------------------------------
// Hook input types — re-exported from inputs module
// ---------------------------------------------------------------------------

pub use crate::inputs::BaseHookInput;
pub use crate::inputs::CompactTrigger;
pub use crate::inputs::ConfigChangeSource;
pub use crate::inputs::ElicitationAction;
pub use crate::inputs::ElicitationMode;
pub use crate::inputs::ExitReason;
pub use crate::inputs::FileChangeEvent;
pub use crate::inputs::HookInput;
pub use crate::inputs::InstructionsLoadReason;
pub use crate::inputs::MemoryType;
pub use crate::inputs::PostCompactInput;
pub use crate::inputs::PostToolUseFailureInput;
pub use crate::inputs::PostToolUseInput;
pub use crate::inputs::PreCompactInput;
pub use crate::inputs::PreToolUseInput;
pub use crate::inputs::SessionEndInput;
pub use crate::inputs::SessionStartInput;
pub use crate::inputs::SessionStartSource;
pub use crate::inputs::SetupTrigger;
pub use crate::inputs::StopFailureInput;
pub use crate::inputs::StopInput;
pub use crate::inputs::SubagentStartInput;
pub use crate::inputs::SubagentStopInput;
pub use crate::inputs::UserPromptSubmitInput;
pub use crate::inputs::base_from_ctx;

// ---------------------------------------------------------------------------
// Hook JSON output parsing (stdout)
// ---------------------------------------------------------------------------

/// Structured JSON output from a hook command's stdout.
///
/// `hookJSONOutputSchema` — the hook writes JSON to stdout for structured control.
/// Supports both flat fields (Rust-native) and nested `hookSpecificOutput`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookJsonOutput {
    /// If false, the agent should stop after this turn.
    #[serde(rename = "continue")]
    pub should_continue: Option<bool>,
    /// Suppress default output display.
    #[serde(alias = "suppressOutput")]
    pub suppress_output: Option<bool>,
    /// Reason string when stopping.
    #[serde(alias = "stopReason")]
    pub stop_reason: Option<String>,
    /// "approve" or "block" — used for permission-decision hooks.
    pub decision: Option<String>,
    /// Human-readable reason for the decision.
    pub reason: Option<String>,
    /// Message to inject into conversation as a system message.
    #[serde(alias = "systemMessage")]
    pub system_message: Option<String>,
    /// Permission decision override for PreToolUse hooks (flat format).
    #[serde(alias = "permissionDecision")]
    pub permission_decision: Option<String>,
    /// Additional context to inject (flat format).
    #[serde(alias = "additionalContext")]
    pub additional_context: Option<String>,
    /// Updated tool input (PreToolUse only, flat format).
    #[serde(alias = "updatedInput")]
    pub updated_input: Option<serde_json::Value>,
    /// Updated MCP tool output (PostToolUse only, flat format).
    #[serde(alias = "updatedMCPToolOutput")]
    pub updated_mcp_tool_output: Option<serde_json::Value>,
    /// Initial user message to inject (SessionStart hooks, flat format).
    #[serde(alias = "initialUserMessage")]
    pub initial_user_message: Option<String>,
    /// File watch paths to register (CwdChanged/FileChanged hooks).
    #[serde(default, alias = "watchPaths")]
    pub watch_paths: Vec<String>,
    /// Whether to retry the operation (PermissionDenied hooks).
    #[serde(default)]
    pub retry: bool,
    /// Human-readable status for progress display.
    #[serde(alias = "statusMessage")]
    pub status_message: Option<String>,
    /// When true, the hook runner should re-wake after async completion.
    #[serde(default, alias = "asyncRewake")]
    pub async_rewake: bool,
    /// Nested event-specific output.
    ///
    /// When present, fields from this object override flat-format fields.
    #[serde(default, alias = "hookSpecificOutput")]
    pub hook_specific_output: Option<HookSpecificOutput>,
}

// Event-specific hook output and the PermissionRequest sub-decision
// live in `coco-types` so the SDK boundary (`SdkHookOutput`) and the
// internal hook orchestrator parse the same typed shape — no
// translation layer, no string matching, no serde duplication.
//
// Re-exported here so existing `coco_hooks::orchestration::*` import
// paths keep working. `ElicitationAction` is re-exported from
// `crate::inputs` earlier in this file (it serves both hook INPUT
// and hook OUTPUT — one wire vocabulary).
pub use coco_types::HookDecision;
pub use coco_types::HookPermissionDecision;
pub use coco_types::HookSpecificOutput;
pub use coco_types::PermissionRequestDecision;

/// Parse hook stdout, attempting JSON first, falling back to plain text.
pub fn parse_hook_output(stdout: &str) -> ParsedHookOutput {
    let trimmed = stdout.trim();
    if !trimmed.starts_with('{') {
        return ParsedHookOutput::PlainText(stdout.to_string());
    }
    // Distinguish "not valid JSON at all" (→ plain text) from "valid JSON but
    // wrong shape" (→ validation error). `parseHookOutput` returns
    // `validationError` only for the latter; the result loop surfaces it as a
    // `hook_non_blocking_error` and does NOT inject the text as context.
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => match serde_json::from_value::<HookJsonOutput>(value) {
            Ok(json) => ParsedHookOutput::Json(Box::new(json)),
            Err(e) => {
                tracing::debug!("hook JSON failed schema validation: {e}");
                ParsedHookOutput::ValidationError(format!(
                    "{e}\n\nExpected schema:\n{HOOK_OUTPUT_SCHEMA_HINT}"
                ))
            }
        },
        Err(e) => {
            tracing::debug!("hook output starts with {{ but is not valid JSON: {e}");
            ParsedHookOutput::PlainText(stdout.to_string())
        }
    }
}

/// Schema hint appended to hook JSON validation errors.
const HOOK_OUTPUT_SCHEMA_HINT: &str = "\
{
  \"continue\": \"boolean (optional)\",
  \"suppressOutput\": \"boolean (optional)\",
  \"stopReason\": \"string (optional)\",
  \"decision\": \"\\\"approve\\\" | \\\"block\\\" (optional)\",
  \"reason\": \"string (optional)\",
  \"systemMessage\": \"string (optional)\",
  \"permissionDecision\": \"\\\"allow\\\" | \\\"deny\\\" | \\\"ask\\\" (optional)\",
  \"hookSpecificOutput\": \"object (optional, see docs for per-event fields)\"
}";

/// Result of parsing hook stdout.
#[derive(Debug, Clone)]
pub enum ParsedHookOutput {
    Json(Box<HookJsonOutput>),
    PlainText(String),
    /// Stdout was valid JSON but did not match the hook output schema. Surfaced
    /// as a `hook_non_blocking_error`; never injected as model context.
    ValidationError(String),
}

// ---------------------------------------------------------------------------
// Aggregated result
// ---------------------------------------------------------------------------

/// Blocking error from a hook.
///
/// Blocking error from a hook.
///
/// `source` carries the typed provenance — three real
/// providers (Command / Function / Llm) and consumers (telemetry, error
/// rendering, log filtering) need to tell them apart without parsing
/// the command string.
#[derive(Debug, Clone)]
pub struct HookBlockingError {
    pub blocking_error: String,
    pub source: HookBlockingSource,
}

impl HookBlockingError {
    /// Convenience: the shell-command string when this error came from
    /// a `Command` hook, otherwise an empty string. Preserves the
    /// pre-refactor accessor shape for log/telemetry consumers that
    /// printed `err.command` verbatim.
    pub fn command(&self) -> &str {
        match &self.source {
            HookBlockingSource::Command(cmd) => cmd.as_str(),
            _ => "",
        }
    }
}

/// What produced a [`HookBlockingError`]. Lets consumers branch on the
/// real provider rather than scraping a synthetic command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookBlockingSource {
    /// Settings-loaded `HookHandler::Command` — the literal shell
    /// command string that fired the hook.
    Command(String),
    /// Settings-loaded `HookHandler::Http` — carries the configured
    /// URL so consumers can distinguish HTTP webhook denials from
    /// shell-command denials without parsing a synthetic label.
    Http(String),
    /// In-memory [`crate::FunctionHook`] — carries the hook's id so
    /// log lines and telemetry can correlate to the registration
    /// site.
    Function { hook_id: String },
    /// LLM-driven `HookHandler::Prompt` / `HookHandler::Agent` hook.
    /// No command/id; the LLM's blocking decision lives in
    /// `blocking_error`.
    Llm,
    /// SDK-supplied [`crate::HookHandler::SdkCallback`] — carries the
    /// `callback_id` registered at `initialize` time so telemetry,
    /// log filtering, and error rendering can distinguish SDK denials
    /// from shell-hook denials.
    Sdk { callback_id: String },
}

/// Aggregated result from executing all matching hooks for a single event.
#[derive(Debug, Clone, Default)]
pub struct AggregatedHookResult {
    pub blocking_error: Option<HookBlockingError>,
    pub prevent_continuation: bool,
    pub stop_reason: Option<String>,
    pub permission_behavior: Option<PermissionBehavior>,
    pub hook_permission_decision_reason: Option<String>,
    pub additional_contexts: Vec<String>,
    pub updated_input: Option<serde_json::Value>,
    /// Updated MCP tool output (PostToolUse only).
    pub updated_mcp_tool_output: Option<serde_json::Value>,
    pub system_message: Option<String>,
    /// Whether to suppress default output display.
    pub suppress_output: bool,
    /// Initial user message injected by SessionStart hooks.
    pub initial_user_message: Option<String>,
    /// File watch paths to register (from CwdChanged/FileChanged hooks).
    pub watch_paths: Vec<String>,
    /// Whether to retry the operation (from PermissionDenied hooks).
    pub retry: bool,
    /// Human-readable status from the last hook that provided one.
    pub status_message: Option<String>,
    /// True if any hook requested async re-wake.
    pub async_rewake: bool,
    /// PermissionRequest hook decision.
    pub permission_request_result: Option<PermissionRequestDecision>,
    /// Elicitation hook response.
    pub elicitation_response: Option<ElicitationResponse>,
    /// ElicitationResult hook response.
    pub elicitation_result_response: Option<ElicitationResponse>,
}

/// SessionStart hook output captured for immediate insertion into a
/// rewritten conversation instead of the next-turn reminder buffer.
#[derive(Debug, Clone, Default)]
pub struct SessionStartHookExecution {
    pub aggregate: AggregatedHookResult,
    pub events: Vec<coco_system_reminder::HookEvent>,
}

/// Elicitation response from a hook.
///
/// `action` is the typed `ElicitationAction` (Accept / Decline / Cancel)
/// — the wire shape is fixed by `coco_types::ElicitationAction`'s lowercase serde.
#[derive(Debug, Clone)]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    pub content: Option<serde_json::Value>,
}

impl AggregatedHookResult {
    pub fn is_blocked(&self) -> bool {
        self.blocking_error.is_some()
    }
}

/// Result of executing a single hook (command, prompt, http, etc).
#[derive(Debug, Clone)]
pub struct SingleHookResult {
    pub command: String,
    pub succeeded: bool,
    pub output: String,
    pub blocked: bool,
    pub outcome: HookOutcome,
    /// Human-readable status for progress display.
    pub status_message: Option<String>,
    /// When true, the hook runner should re-wake after async completion.
    pub async_rewake: bool,
    /// Provenance for the [`HookBlockingError`] this result may
    /// produce. Required (no `Option`) so every construction site
    /// makes an explicit choice — SDK callbacks must carry
    /// `Sdk { callback_id }`, HTTP hooks carry `Http(url)`, etc.
    /// The previous `Option<...>` default-to-Command shape silently
    /// mis-tagged HTTP and SDK denials as shell-command sources.
    pub source: HookBlockingSource,
    /// SDK callback typed output, if this result came from an
    /// `SdkCallback` handler. When `Some`, aggregation applies the
    /// typed [`coco_types::SdkHookOutput`] directly via
    /// [`apply_sdk_hook_output`] — no JSON `parse_hook_output`
    /// fallback, no string-vs-typed round-trip. `None` for every
    /// other handler kind (Command/Http/Prompt/Agent).
    pub sdk_output: Option<coco_types::SdkHookOutput>,
}

impl SingleHookResult {
    /// Snapshot the source. Borrowed-clone helper for aggregation
    /// sites that consume the source into a `HookBlockingError`.
    fn blocking_source(&self) -> HookBlockingSource {
        self.source.clone()
    }
}

// ---------------------------------------------------------------------------
// Environment variable injection
// ---------------------------------------------------------------------------

/// Workspace-trust gate.
///
/// A global guard that blocks ALL hook execution in interactive mode when the
/// user has not yet accepted workspace trust for the current project.
/// Returns `true` to skip hooks.
///
/// Coco-rs does not yet ship a workspace-trust dialog (see
/// `crate-coco-hooks.md` Known Gaps), so the default is "trusted"
/// unless the caller explicitly opts out via
/// `OrchestrationContext.workspace_trust_accepted = Some(false)` or
/// the runtime sets `COCO_WORKSPACE_TRUST_ACCEPTED=0`. Once the
/// trust dialog ships, `OrchestrationContext` will carry the
/// dialog-resolved value and this fallback becomes inert.
pub fn should_skip_hook_due_to_trust(ctx: &OrchestrationContext) -> bool {
    if let Some(accepted) = ctx.workspace_trust_accepted {
        return !accepted;
    }
    matches!(
        std::env::var("COCO_WORKSPACE_TRUST_ACCEPTED").as_deref(),
        Ok("0")
    )
}

/// Plugin context for hook environment variables.
#[derive(Debug, Clone, Default)]
pub struct HookPluginContext {
    /// Root directory for the plugin or skill.
    pub plugin_root: Option<String>,
    /// Plugin identifier (for data directory).
    pub plugin_id: Option<String>,
    /// Plugin configuration options (exposed as CLAUDE_PLUGIN_OPTION_*).
    pub plugin_options: HashMap<String, String>,
    /// Root directory for the skill (uses CLAUDE_PLUGIN_ROOT for compat).
    pub skill_root: Option<String>,
}

/// Build the set of environment variables to inject into a hook command process.
pub fn build_hook_env(
    session_id: &str,
    cwd: &str,
    tool_name: Option<&str>,
    hook_event: HookEventType,
    project_dir: Option<&str>,
) -> HashMap<String, String> {
    build_hook_env_with_plugin(
        session_id,
        cwd,
        tool_name,
        hook_event,
        project_dir,
        None,
        None,
    )
}

/// Build hook environment with optional plugin context.
pub fn build_hook_env_with_plugin(
    session_id: &str,
    cwd: &str,
    tool_name: Option<&str>,
    hook_event: HookEventType,
    project_dir: Option<&str>,
    plugin_ctx: Option<&HookPluginContext>,
    hook_index: Option<usize>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let hook_event_str = hook_event.as_str();
    env.insert("HOOK_EVENT".to_string(), hook_event_str.to_string());
    env.insert("HOOK_SESSION_ID".to_string(), session_id.to_string());
    env.insert("HOOK_CWD".to_string(), cwd.to_string());
    if let Some(name) = tool_name {
        env.insert("HOOK_TOOL_NAME".to_string(), name.to_string());
    }
    if let Some(dir) = project_dir {
        env.insert("CLAUDE_PROJECT_DIR".to_string(), dir.to_string());
    }

    // Plugin/skill env vars.
    if let Some(ctx) = plugin_ctx {
        if let Some(root) = &ctx.plugin_root {
            env.insert("CLAUDE_PLUGIN_ROOT".to_string(), root.clone());
        }
        if let Some(id) = &ctx.plugin_id {
            // Plugin data dir convention: ~/.coco/plugins/<plugin_id>/data
            if let Ok(home) = std::env::var("HOME") {
                let data_dir = format!("{home}/.coco/plugins/{id}/data");
                env.insert("CLAUDE_PLUGIN_DATA".to_string(), data_dir);
            }
        }
        for (key, value) in &ctx.plugin_options {
            // Sanitize key to valid env var identifier.
            let env_key = key
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
                .to_ascii_uppercase();
            env.insert(format!("CLAUDE_PLUGIN_OPTION_{env_key}"), value.clone());
        }
        // Skill root uses CLAUDE_PLUGIN_ROOT for consistency.
        if let Some(root) = &ctx.skill_root {
            env.insert("CLAUDE_PLUGIN_ROOT".to_string(), root.clone());
        }
    }

    // CLAUDE_ENV_FILE — for SessionStart, Setup, CwdChanged, FileChanged.
    // Hooks write shell snippets to this path; the next bash command
    // sources them via `coco_shell::SessionEnvReader`. Location matches
    // Location: <coco_home>/session-env/<session_id>/{event}-hook-{idx}.sh
    // where `event` is lowercase ("setup", "sessionstart", …) so the
    // reader's regex picks them up.
    if let Some(idx) = hook_index
        && matches!(
            hook_event,
            HookEventType::SessionStart
                | HookEventType::Setup
                | HookEventType::CwdChanged
                | HookEventType::FileChanged
        )
    {
        let event_lower = hook_event_str.to_ascii_lowercase();
        let coco_home = coco_config::global_config::config_home();
        let dir = coco_home.join("session-env").join(session_id);
        // Best-effort dir creation — failures fall through and let the
        // hook's `> $CLAUDE_ENV_FILE` redirect surface the error.
        let _ = std::fs::create_dir_all(&dir);
        let env_file = dir
            .join(format!("{event_lower}-hook-{idx}.sh"))
            .to_string_lossy()
            .into_owned();
        env.insert("CLAUDE_ENV_FILE".to_string(), env_file);
    }

    env
}

// ---------------------------------------------------------------------------
// Core parallel execution engine
// ---------------------------------------------------------------------------

/// Execute matching hooks in parallel with per-hook timeouts.
///
/// Each hook runs in its own tokio task. A `CancellationToken` can abort all
/// outstanding hooks. Returns one `SingleHookResult` per matched hook.
#[allow(clippy::too_many_arguments)]
pub async fn execute_hooks_parallel(
    registry: &HookRegistry,
    event: HookEventType,
    tool_name: Option<&str>,
    hook_input_json: &str,
    env_vars: &HashMap<String, String>,
    cancel: &CancellationToken,
    default_timeout: Duration,
    event_tx: Option<&tokio::sync::mpsc::Sender<crate::HookExecutionEvent>>,
    attachment_emitter: &AttachmentEmitter,
) -> Vec<SingleHookResult> {
    execute_hooks_parallel_filtered(
        registry,
        event,
        tool_name,
        hook_input_json,
        env_vars,
        cancel,
        default_timeout,
        event_tx,
        attachment_emitter,
        /*allow_managed_hooks_only*/ false,
        /*http_url_allowlist*/ None,
        /*http_env_var_policy*/ None,
        /*async_registry*/ None,
        /*async_rewake_sink*/ None,
        /*llm_handle*/ None,
        /*workspace_trust_accepted*/ None,
    )
    .await
}

/// Execute hooks with managed-only + HTTP-policy filtering. Used by
/// every orchestration entry point. `http_url_allowlist` and
/// `http_env_var_policy` are typically pulled from
/// `OrchestrationContext`; tests/the public `execute_hooks_parallel`
/// shim default them to `None`. `async_registry` captures `is_async`
/// hook output for delivery via the reminder pipeline.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    name = "hook_event",
    fields(
        hook_event = ?event,
        tool_name = ?tool_name,
        managed_only = allow_managed_hooks_only,
    ),
)]
async fn execute_hooks_parallel_filtered(
    registry: &HookRegistry,
    event: HookEventType,
    tool_name: Option<&str>,
    hook_input_json: &str,
    env_vars: &HashMap<String, String>,
    cancel: &CancellationToken,
    default_timeout: Duration,
    event_tx: Option<&tokio::sync::mpsc::Sender<crate::HookExecutionEvent>>,
    attachment_emitter: &AttachmentEmitter,
    allow_managed_hooks_only: bool,
    http_url_allowlist: Option<&[String]>,
    http_env_var_policy: Option<&[String]>,
    async_registry: Option<&std::sync::Arc<crate::async_registry::AsyncHookRegistry>>,
    async_rewake_sink: Option<&std::sync::Arc<dyn crate::AsyncRewakeSink>>,
    llm_handle: Option<&std::sync::Arc<dyn crate::llm_handle::HookLlmHandle>>,
    workspace_trust_accepted: Option<bool>,
) -> Vec<SingleHookResult> {
    // Global trust gate — if the workspace is explicitly untrusted,
    // no hook fires.
    if matches!(workspace_trust_accepted, Some(false)) {
        tracing::debug!(
            event = ?event,
            "skipping hooks: workspace trust not accepted"
        );
        return Vec::new();
    }
    let matching = registry.find_matching(event, tool_name);
    if matching.is_empty() {
        return Vec::new();
    }

    // Filter hooks based on policies.
    let matching: Vec<_> = matching
        .into_iter()
        .filter(|h| {
            // HTTP hooks are not supported for SessionStart/Setup events.
            if matches!(event, HookEventType::SessionStart | HookEventType::Setup)
                && matches!(h.handler, HookHandler::Http { .. })
            {
                tracing::debug!("HTTP hooks not supported for {event:?}, skipping");
                return false;
            }
            // Policy `allowedHttpHookUrls` enforcement. `None` = no restriction;
            // `Some(empty)` = block all HTTP hooks; `Some(non-empty)` =
            // URL must match one pattern.
            if let HookHandler::Http { url, .. } = &h.handler
                && let Some(allow) = http_url_allowlist
                && !crate::ssrf::url_matches_allowlist(url, allow)
            {
                tracing::warn!(
                    %url,
                    "HTTP hook blocked: URL not in allowedHttpHookUrls policy"
                );
                return false;
            }
            // When allow_managed_hooks_only, skip non-managed hooks.
            // Managed hooks use Builtin scope (from policy config).
            // Session hooks are also allowed (programmatically added).
            if allow_managed_hooks_only
                && !matches!(
                    h.scope,
                    coco_types::HookScope::Builtin | coco_types::HookScope::Session
                )
            {
                tracing::debug!(
                    "skipping non-managed hook for {event:?} (scope={:?})",
                    h.scope
                );
                return false;
            }
            true
        })
        .collect();
    if matching.is_empty() {
        return Vec::new();
    }

    tracing::info!(hook_count = matching.len(), "hook_event firing");

    let (tx, mut rx) = mpsc::channel::<SingleHookResult>(matching.len());
    let sdk_hook_callback = registry.sdk_hook_callback();

    let policy_set: Option<HashSet<&str>> =
        http_env_var_policy.map(|p| p.iter().map(String::as_str).collect());

    for (idx, hook) in matching.iter().enumerate() {
        // Intersect per-hook `allowed_env_vars` with policy
        // `httpHookAllowedEnvVars` when both are set. When policy is `None`,
        // the per-hook list passes through untouched.
        let handler = match (&hook.handler, policy_set.as_ref()) {
            (
                HookHandler::Http {
                    url,
                    headers,
                    timeout_ms,
                    allowed_env_vars,
                },
                Some(policy),
            ) => {
                let intersected: Vec<String> = allowed_env_vars
                    .iter()
                    .filter(|v| policy.contains(v.as_str()))
                    .cloned()
                    .collect();
                HookHandler::Http {
                    url: url.clone(),
                    headers: headers.clone(),
                    timeout_ms: *timeout_ms,
                    allowed_env_vars: intersected,
                }
            }
            _ => hook.handler.clone(),
        };
        let cancel = cancel.clone();
        let env = env_vars.clone();
        let input_json = hook_input_json.to_string();
        let timeout = resolve_timeout(&handler, default_timeout);
        let command_label = handler_label(&handler);
        // Tag this spawn with its provenance so `apply_hook_specific_output`
        // and `aggregate_results_for_event` can build `HookBlockingError`
        // with the correct `HookBlockingSource` variant — SDK callbacks
        // carry `Sdk { callback_id }` instead of a synthetic `sdk:<id>`
        // command-label string.
        let handler_source = derive_handler_source(&handler);
        let hook_id = format!("hook-{idx}");
        let hook_event_str = format!("{event:?}");
        let event_tx = event_tx.cloned();
        let emitter = attachment_emitter.clone();
        let llm_handle_clone = llm_handle.cloned();
        let sdk_hook_callback = sdk_hook_callback.clone();
        let is_async = hook.is_async;
        let async_rewake = hook.async_rewake;
        // Only clone sender for sync hooks. Async hooks deliver
        // out-of-band through `async_registry`; their result is later
        // surfaced to the model via the reminder pipeline.
        let tx = if is_async || async_rewake {
            None
        } else {
            Some(tx.clone())
        };
        // Per-spawn handle to the async registry. Cloned outside the
        // spawn so the registry can be `Some(...)` and the spawn can
        // call `.register()` / completion methods.
        let async_reg_for_spawn = async_registry.cloned();
        let async_rewake_sink_for_spawn = async_rewake_sink.cloned();
        let async_hook_id = if is_async {
            format!("hook-{idx}-{}", uuid::Uuid::new_v4().simple())
        } else {
            String::new()
        };
        let async_event_label = format!("{event:?}");

        // Emit Started event before spawning the task.
        if let Some(etx) = &event_tx {
            let _ = etx
                .send(crate::HookExecutionEvent::Started {
                    hook_id: hook_id.clone(),
                    hook_name: command_label.clone(),
                    hook_event: hook_event_str.clone(),
                })
                .await;
        }

        tokio::spawn(async move {
            // Progress polling: emit Progress events every 1s while hook runs.
            let progress_handle = if let Some(etx) = event_tx.clone() {
                let hid = hook_id.clone();
                let hname = command_label.clone();
                Some(tokio::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(1));
                    interval.tick().await; // skip immediate first tick
                    loop {
                        interval.tick().await;
                        let send_result = etx
                            .send(crate::HookExecutionEvent::Progress {
                                hook_id: hid.clone(),
                                hook_name: hname.clone(),
                                stdout: String::new(),
                                stderr: String::new(),
                            })
                            .await;
                        if send_result.is_err() {
                            break;
                        }
                    }
                }))
            } else {
                None
            };

            let result = tokio::select! {
                _ = cancel.cancelled() => {
                    // `hook_cancelled`: API-hidden, UI surfaces cancel + command metadata.
                    let attachment = AttachmentMessage::silent_hook_cancelled(
                        HookCancelledPayload {
                            hook_name: command_label.clone(),
                            tool_use_id: String::new(),
                            hook_event: event,
                            command: Some(command_label.clone()),
                            duration_ms: None,
                        },
                    );
                    emitter.emit(attachment);
                    SingleHookResult {
                        command: command_label.clone(),
                        succeeded: false,
                        output: "Hook cancelled".to_string(),
                        blocked: false,
                        outcome: HookOutcome::Cancelled,
                        status_message: None,
                        async_rewake: false,
                        source: handler_source.clone(),
                        sdk_output: None,
                    }
                }
                res = tokio::time::timeout(
                    timeout,
                    run_hook_via_handle_or_fallback(HookExecutionRequest {
                        handler: &handler,
                        env_vars: &env,
                        stdin_input: Some(&input_json),
                        llm_handle: llm_handle_clone.as_ref(),
                        sdk_hook_callback: sdk_hook_callback.as_ref(),
                        event,
                        timeout,
                        async_options: Some(crate::AsyncCommandOptions {
                            registry: async_reg_for_spawn.clone(),
                            hook_id: async_hook_id.clone(),
                            hook_name: command_label.clone(),
                            hook_event: async_event_label.clone(),
                            timeout,
                            forced_async: is_async,
                            async_rewake,
                            rewake_sink: async_rewake_sink_for_spawn.clone(),
                        }),
                    }),
                ) => {
                    match res {
                        Ok(Ok(exec_result)) => process_execution_result(
                            exec_result,
                            &command_label,
                            handler_source.clone(),
                            event,
                            &emitter,
                        ),
                        Ok(Err(e)) => {
                            // `hook_error_during_execution` (API-hidden, UI-visible):
                            // the hook itself crashed.
                            let err_msg = format!("{e}");
                            let attachment = AttachmentMessage::silent_hook_error_during_execution(
                                HookErrorDuringExecutionPayload {
                                    content: err_msg.clone(),
                                    hook_name: command_label.clone(),
                                    tool_use_id: String::new(),
                                    hook_event: event,
                                },
                            );
                            emitter.emit(attachment);
                            SingleHookResult {
                                command: command_label.clone(),
                                succeeded: false,
                                output: err_msg,
                                blocked: false,
                                outcome: HookOutcome::NonBlockingError,
                                status_message: None,
                                async_rewake: false,
                                source: handler_source.clone(),
                                sdk_output: None,
                            }
                        }
                        Err(_elapsed) => {
                            // Timeout is a non-blocking error;
                            // emit `hook_non_blocking_error` for UI / audit.
                            let err_msg = format!("hook timed out after {timeout:?}");
                            let attachment = AttachmentMessage::silent_hook_non_blocking_error(
                                HookNonBlockingErrorPayload {
                                    error: err_msg.clone(),
                                    hook_name: command_label.clone(),
                                    tool_use_id: String::new(),
                                    hook_event: event,
                                },
                            );
                            emitter.emit(attachment);
                            SingleHookResult {
                                command: command_label.clone(),
                                succeeded: false,
                                output: err_msg,
                                blocked: false,
                                outcome: HookOutcome::NonBlockingError,
                                status_message: None,
                                async_rewake: false,
                                source: handler_source.clone(),
                                sdk_output: None,
                            }
                        }
                    }
                }
            };

            // Stop progress polling.
            if let Some(h) = progress_handle {
                h.abort();
            }

            // Emit Response event after completion.
            if let Some(etx) = &event_tx {
                let _ = etx
                    .send(crate::HookExecutionEvent::Response {
                        hook_id,
                        hook_name: result.command.clone(),
                        exit_code: None,
                        stdout: result.output.clone(),
                        stderr: String::new(),
                        outcome: result.outcome,
                    })
                    .await;
            }

            // Sync result delivery. Async hooks already returned via
            // the registry above, so `tx` is `None` for them.
            if let Some(tx) = tx {
                let _ = tx.send(result).await;
            }
        });
    }

    // Drop our copy of the sender so the channel closes when all tasks finish.
    drop(tx);

    let mut results = Vec::with_capacity(matching.len());
    while let Some(r) = rx.recv().await {
        results.push(r);
    }
    let blocked = results.iter().filter(|r| r.blocked).count();
    let errored = results.iter().filter(|r| !r.succeeded).count();
    tracing::info!(
        completed = results.len(),
        blocked,
        errored,
        "hook_event done"
    );
    results
}

/// Aggregate individual hook results into a single `AggregatedHookResult`.
///
/// Backwards-compatible shim that calls
/// [`aggregate_results_for_event`] without the
/// `hookSpecificOutput.hookEventName` cross-check. Prefer the
/// `_for_event` variant in new code so mismatched event names are
/// rejected.
pub fn aggregate_results(results: &[SingleHookResult]) -> AggregatedHookResult {
    aggregate_results_for_event(results, None)
}

/// Same as [`aggregate_results`] but enforces the
/// `hookSpecificOutput.hookEventName === expected` invariant. When a
/// hook firing for event `Some(expected)` emits a `hookSpecificOutput`
/// claiming a different event, the nested output is ignored and a
/// warning is logged. Flat-format fields (decision, additional_context,
/// etc.) are still honored — they aren't event-tagged.
pub fn aggregate_results_for_event(
    results: &[SingleHookResult],
    expected_event: Option<HookEventType>,
) -> AggregatedHookResult {
    let mut agg = AggregatedHookResult::default();

    for r in results {
        if r.status_message.is_some() {
            agg.status_message.clone_from(&r.status_message);
        }
        if r.async_rewake {
            agg.async_rewake = true;
        }

        // **Typed SDK path** — when the callback returned a typed
        // `SdkHookOutput`, apply it directly without parsing the
        // legacy shell-hook stdout JSON. Skips the round-trip
        // `Value → string → parse_hook_output` rescue entirely.
        if let Some(sdk_output) = &r.sdk_output {
            apply_sdk_hook_output(&mut agg, sdk_output, r, expected_event);
            continue;
        }

        if r.blocked {
            agg.blocking_error = Some(HookBlockingError {
                blocking_error: r.output.clone(),
                source: r.blocking_source(),
            });
        }

        // Parse stdout for JSON control signals.
        match parse_hook_output(&r.output) {
            ParsedHookOutput::Json(json) => {
                if json.should_continue == Some(false) {
                    agg.prevent_continuation = true;
                    if json.stop_reason.is_some() {
                        agg.stop_reason = json.stop_reason.clone();
                    }
                }

                match json.decision.as_deref() {
                    Some("approve") => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Allow,
                        ));
                    }
                    Some("block") => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Deny,
                        ));
                        agg.blocking_error = Some(HookBlockingError {
                            blocking_error: json
                                .reason
                                .clone()
                                .unwrap_or_else(|| "Blocked by hook".to_string()),
                            source: r.blocking_source(),
                        });
                    }
                    _ => {}
                }

                match json.permission_decision.as_deref() {
                    Some("allow") => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Allow,
                        ));
                    }
                    Some("deny") => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Deny,
                        ));
                    }
                    Some("ask") => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Ask,
                        ));
                    }
                    _ => {}
                }
                // Silent `hook_permission_decision` attachment is deferred —
                // emitting it requires `tool_use_id` + `hook_event`, which
                // `aggregate_results` doesn't currently receive. Thread them
                // through before re-enabling; shipping placeholder values
                // would poison audit data.

                if let Some(reason) = &json.reason
                    && agg.permission_behavior.is_some()
                {
                    agg.hook_permission_decision_reason = Some(reason.clone());
                }

                if let Some(ctx) = &json.additional_context {
                    agg.additional_contexts.push(ctx.clone());
                }

                if json.updated_input.is_some() {
                    agg.updated_input = json.updated_input.clone();
                }

                if json.updated_mcp_tool_output.is_some() {
                    agg.updated_mcp_tool_output = json.updated_mcp_tool_output.clone();
                }

                if json.system_message.is_some() {
                    agg.system_message = json.system_message.clone();
                    // `hook_system_message` silent attachment deferred —
                    // needs `event` + `hook_name` threaded into
                    // `aggregate_results` alongside `hook_permission_decision`.
                }

                if json.status_message.is_some() {
                    agg.status_message = json.status_message.clone();
                }

                if json.suppress_output == Some(true) {
                    agg.suppress_output = true;
                }

                if json.initial_user_message.is_some() && agg.initial_user_message.is_none() {
                    agg.initial_user_message = json.initial_user_message.clone();
                }

                agg.watch_paths.extend(json.watch_paths.iter().cloned());

                if json.retry {
                    agg.retry = true;
                }

                if json.async_rewake {
                    agg.async_rewake = true;
                }

                // Process hookSpecificOutput (nested output).
                // Fields from hookSpecificOutput override flat-format fields.
                if let Some(specific) = &json.hook_specific_output {
                    let claimed = specific.claimed_event();
                    let mismatch = expected_event.map(|exp| exp != claimed).unwrap_or(false);
                    if mismatch {
                        // Event name mismatch: log + skip instead of throwing so
                        // a misconfigured hook doesn't poison every
                        // result in the batch — the flat-format fields
                        // above are still applied.
                        tracing::warn!(
                            expected = ?expected_event,
                            claimed = ?claimed,
                            command = %r.command,
                            "hook returned hookSpecificOutput.hookEventName mismatch; ignoring nested fields"
                        );
                    } else {
                        apply_hook_specific_output(&mut agg, specific, r);
                    }
                }
            }
            ParsedHookOutput::PlainText(text) => {
                // Plain stdout becomes model context only on a clean exit
                // (hook_success). A failed hook's stderr is surfaced as a
                // `hook_non_blocking_error` attachment (emitted in
                // `process_execution_result`), never injected as success-context.
                let trimmed = text.trim();
                if r.succeeded && !trimmed.is_empty() {
                    agg.additional_contexts.push(trimmed.to_string());
                }
            }
            ParsedHookOutput::ValidationError(_) => {
                // Valid-JSON-but-wrong-shape output is surfaced as a
                // `hook_non_blocking_error` at execution time (see
                // `process_execution_result`) and must NOT be injected as model
                // context.
            }
        }
    }

    agg
}

/// Merge two permission behaviors: deny > ask > allow.
fn merge_permission(
    current: Option<PermissionBehavior>,
    new: PermissionBehavior,
) -> PermissionBehavior {
    match (current, new) {
        (_, PermissionBehavior::Deny) | (Some(PermissionBehavior::Deny), _) => {
            PermissionBehavior::Deny
        }
        (_, PermissionBehavior::Ask) | (Some(PermissionBehavior::Ask), _) => {
            PermissionBehavior::Ask
        }
        _ => PermissionBehavior::Allow,
    }
}

/// Apply a typed [`coco_types::SdkHookOutput`] to the aggregated
/// result. Used by [`aggregate_results_for_event`] when the spawn
/// loop populates `SingleHookResult.sdk_output` from an `SdkCallback`
/// handler — bypasses the legacy shell-hook stdout parser.
///
/// Consumes the typed `hookJSONOutputSchema` shape directly. The
/// top-level fields (`continue`, `suppressOutput`, `decision`, `reason`,
/// `systemMessage`) and the nested `hookSpecificOutput` union are applied
/// in one pass.
fn apply_sdk_hook_output(
    agg: &mut AggregatedHookResult,
    output: &coco_types::SdkHookOutput,
    result: &SingleHookResult,
    expected_event: Option<HookEventType>,
) {
    use coco_types::HookDecision;
    let source = result.blocking_source();

    // continue: false ⇒ stop the loop. Pair with stop_reason.
    if output.r#continue == Some(false) {
        agg.prevent_continuation = true;
        if output.stop_reason.is_some() {
            agg.stop_reason.clone_from(&output.stop_reason);
        }
    }

    if output.suppress_output == Some(true) {
        agg.suppress_output = true;
    }

    if let Some(msg) = &output.system_message {
        agg.system_message = Some(msg.clone());
    }

    // Top-level decision: 'approve' | 'block'.
    match output.decision {
        Some(HookDecision::Approve) => {
            agg.permission_behavior = Some(merge_permission(
                agg.permission_behavior,
                PermissionBehavior::Allow,
            ));
        }
        Some(HookDecision::Block) => {
            agg.permission_behavior = Some(merge_permission(
                agg.permission_behavior,
                PermissionBehavior::Deny,
            ));
            agg.blocking_error = Some(HookBlockingError {
                blocking_error: output
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Blocked by hook".to_string()),
                source,
            });
        }
        None => {}
    }

    if output.reason.is_some() && agg.permission_behavior.is_some() {
        agg.hook_permission_decision_reason
            .clone_from(&output.reason);
    }

    // hookSpecificOutput dispatch: when the hook fired for event X emits
    // `hookSpecificOutput.hookEventName = Y`, ignore the nested fields and
    // log (matches the legacy parser).
    if let Some(specific) = &output.hook_specific_output {
        let claimed = specific.claimed_event();
        let mismatch = expected_event.map(|exp| exp != claimed).unwrap_or(false);
        if mismatch {
            tracing::warn!(
                expected = ?expected_event,
                claimed = ?claimed,
                command = %result.command,
                "SDK hook returned hookSpecificOutput.hookEventName mismatch; ignoring nested fields"
            );
        } else {
            apply_hook_specific_output(agg, specific, result);
        }
    }
}

/// Apply event-specific output from `hookSpecificOutput` to the aggregated result.
///
/// Switches on `hookSpecificOutput.hookEventName`.
fn apply_hook_specific_output(
    agg: &mut AggregatedHookResult,
    specific: &HookSpecificOutput,
    result: &SingleHookResult,
) {
    let source = result.blocking_source();
    match specific {
        HookSpecificOutput::PreToolUse {
            permission_decision,
            permission_decision_reason,
            updated_input,
            additional_context,
        } => {
            if let Some(pd) = permission_decision {
                match pd {
                    HookPermissionDecision::Allow => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Allow,
                        ));
                    }
                    HookPermissionDecision::Deny => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Deny,
                        ));
                        agg.blocking_error = Some(HookBlockingError {
                            blocking_error: permission_decision_reason
                                .clone()
                                .unwrap_or_else(|| "Blocked by hook".to_string()),
                            source,
                        });
                    }
                    HookPermissionDecision::Ask => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Ask,
                        ));
                    }
                }
            }
            if let Some(reason) = permission_decision_reason {
                agg.hook_permission_decision_reason = Some(reason.clone());
            }
            if updated_input.is_some() {
                agg.updated_input.clone_from(updated_input);
            }
            if let Some(ctx) = additional_context {
                agg.additional_contexts.push(ctx.clone());
            }
        }
        HookSpecificOutput::PostToolUse {
            additional_context,
            updated_mcp_tool_output,
        } => {
            if let Some(ctx) = additional_context {
                agg.additional_contexts.push(ctx.clone());
            }
            if updated_mcp_tool_output.is_some() {
                agg.updated_mcp_tool_output
                    .clone_from(updated_mcp_tool_output);
            }
        }
        HookSpecificOutput::PostToolUseFailure { additional_context }
        | HookSpecificOutput::UserPromptSubmit { additional_context }
        | HookSpecificOutput::Setup { additional_context }
        | HookSpecificOutput::SubagentStart { additional_context }
        | HookSpecificOutput::Notification { additional_context } => {
            if let Some(ctx) = additional_context {
                agg.additional_contexts.push(ctx.clone());
            }
        }
        HookSpecificOutput::SessionStart {
            additional_context,
            initial_user_message,
            watch_paths,
        } => {
            if let Some(ctx) = additional_context {
                agg.additional_contexts.push(ctx.clone());
            }
            if initial_user_message.is_some() && agg.initial_user_message.is_none() {
                agg.initial_user_message.clone_from(initial_user_message);
            }
            if let Some(paths) = watch_paths {
                agg.watch_paths.extend(paths.iter().cloned());
            }
        }
        HookSpecificOutput::PermissionDenied { retry } => {
            if *retry == Some(true) {
                agg.retry = true;
            }
        }
        HookSpecificOutput::PermissionRequest { decision } => {
            if let Some(d) = decision {
                match d {
                    PermissionRequestDecision::Allow { updated_input } => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Allow,
                        ));
                        if updated_input.is_some() {
                            agg.updated_input.clone_from(updated_input);
                        }
                        agg.permission_request_result = Some(d.clone());
                    }
                    PermissionRequestDecision::Deny { .. } => {
                        agg.permission_behavior = Some(merge_permission(
                            agg.permission_behavior,
                            PermissionBehavior::Deny,
                        ));
                        agg.permission_request_result = Some(d.clone());
                    }
                }
            }
        }
        HookSpecificOutput::Elicitation { action, content } => {
            if let Some(act) = action {
                if matches!(act, ElicitationAction::Decline) {
                    agg.blocking_error = Some(HookBlockingError {
                        blocking_error: "Elicitation denied by hook".to_string(),
                        source: result.blocking_source(),
                    });
                }
                agg.elicitation_response = Some(ElicitationResponse {
                    action: *act,
                    content: content.clone(),
                });
            }
        }
        HookSpecificOutput::ElicitationResult { action, content } => {
            if let Some(act) = action {
                if matches!(act, ElicitationAction::Decline) {
                    agg.blocking_error = Some(HookBlockingError {
                        blocking_error: "Elicitation result blocked by hook".to_string(),
                        source: result.blocking_source(),
                    });
                }
                agg.elicitation_result_response = Some(ElicitationResponse {
                    action: *act,
                    content: content.clone(),
                });
            }
        }
        HookSpecificOutput::CwdChanged { watch_paths }
        | HookSpecificOutput::FileChanged { watch_paths } => {
            if let Some(paths) = watch_paths {
                agg.watch_paths.extend(paths.iter().cloned());
            }
        }
        HookSpecificOutput::WorktreeCreate { .. } => {
            // WorktreeCreate-specific output is informational only.
        }
    }
}

// ---------------------------------------------------------------------------
// Event-specific orchestration functions
// ---------------------------------------------------------------------------

/// Context passed to all orchestration functions.
#[derive(Debug, Clone)]
pub struct OrchestrationContext {
    pub session_id: String,
    pub cwd: PathBuf,
    pub project_dir: Option<PathBuf>,
    pub permission_mode: Option<String>,
    /// Path to the active session transcript, threaded into every
    /// hook input's `transcript_path` field.
    pub transcript_path: Option<String>,
    /// Subagent identifier — `Some` when the orchestration runs inside
    /// an `AgentTool` worker. Plumbed onto every fired hook's base input.
    pub agent_id: Option<String>,
    /// Subagent type (e.g. `"Explore"`, `"Review"`) — see
    /// `BaseHookInput::agent_type`.
    pub agent_type: Option<String>,
    pub cancel: CancellationToken,
    /// When true, all hooks are disabled and execute_event returns immediately.
    pub disable_all_hooks: bool,
    /// When true, only managed (policy-level) hooks are allowed.
    pub allow_managed_hooks_only: bool,
    /// Workspace-trust gate:
    /// `Some(true)` = trust accepted, hooks may run; `Some(false)` =
    /// not yet accepted, all hooks skipped; `None` = no dialog has run
    /// (defaults to "trusted" until the dialog ships).
    pub workspace_trust_accepted: Option<bool>,
    /// Sink for silent `AttachmentMessage`s produced by hook execution
    /// (cancel / error / timeout). Use [`AttachmentEmitter::noop`] in tests
    /// or when no session-scoped sink is available.
    pub attachment_emitter: AttachmentEmitter,
    /// Sink for sync hook events that should surface as per-turn
    /// reminders (`hook_success` / `hook_blocking_error` /
    /// `hook_additional_context` / `hook_stopped_continuation`).
    /// Wired by `SessionRuntime` for SessionStart and UserPromptSubmit;
    /// `None` for other events that aren't reminder-bearing.
    pub sync_event_sink: Option<crate::SyncHookEventBuffer>,
    /// Glob-style allowlist for HTTP hook URLs from policy settings
    /// (`allowedHttpHookUrls`). `None` = no restriction; `Some(empty)`
    /// = block every HTTP hook; `Some(non-empty)` = URL must match one
    /// pattern. Parity with `execHttpHook.ts:137-145`. Patterns support
    /// `*` as wildcard.
    pub http_url_allowlist: Option<Vec<String>>,
    /// Policy-level env-var allowlist
    /// (`policySettings.httpHookAllowedEnvVars`). When `Some`, the
    /// per-hook `allowed_env_vars` is intersected with this set before
    /// interpolation runs.
    pub http_env_var_policy: Option<Vec<String>>,
    /// Registry that captures stdout/stderr/exit-code of `is_async`
    /// hooks so the reminder pipeline can deliver them on later turns.
    /// `None` = legacy fire-and-forget behaviour.
    pub async_registry: Option<std::sync::Arc<crate::async_registry::AsyncHookRegistry>>,
    /// Sink for `asyncRewake` exit-code-2 notifications. These bypass the
    /// async registry and enqueue a task-notification directly.
    pub async_rewake_sink: Option<std::sync::Arc<dyn crate::AsyncRewakeSink>>,
    /// Callback the orchestration uses to drive `Prompt` / `Agent`
    /// hook handlers through the parent session's LLM. `None` falls
    /// back to a passthrough that returns the prompt text verbatim
    /// and logs a warning. Implementations live in `coco-query` and are
    /// installed by `coco-cli::session_runtime`.
    pub llm_handle: Option<std::sync::Arc<dyn crate::llm_handle::HookLlmHandle>>,
}

/// Generic event execution — builds env, runs hooks in parallel, aggregates.
///
/// Use this for events that don't need special return types. The event-specific
/// functions below are convenience wrappers that build the correct input and
/// pass the appropriate `match_value` for each event type.
pub async fn execute_event(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    event: HookEventType,
    match_value: Option<&str>,
    input: &crate::inputs::HookInput,
    timeout: Duration,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let json_input = serde_json::to_string(input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        match_value,
        event,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        event,
        match_value,
        &json_input,
        &env,
        &ctx.cancel,
        timeout,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(aggregate_results_for_event(&results, Some(event)))
}

/// Execute PreToolUse hooks and return the aggregated result.
pub async fn execute_pre_tool_use(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    tool_name: &str,
    tool_use_id: &str,
    tool_input: &serde_json::Value,
    event_tx: Option<&tokio::sync::mpsc::Sender<crate::HookExecutionEvent>>,
) -> crate::Result<AggregatedHookResult> {
    let input = PreToolUseInput {
        base: base_from_ctx(ctx),
        tool_name: tool_name.to_string(),
        tool_input: tool_input.clone(),
        tool_use_id: tool_use_id.to_string(),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        Some(tool_name),
        HookEventType::PreToolUse,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::PreToolUse,
        Some(tool_name),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        event_tx,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::PreToolUse),
    ))
}

/// Execute PostToolUse hooks and return the aggregated result.
pub async fn execute_post_tool_use(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    tool_name: &str,
    tool_use_id: &str,
    tool_input: &serde_json::Value,
    tool_response: &serde_json::Value,
    event_tx: Option<&tokio::sync::mpsc::Sender<crate::HookExecutionEvent>>,
) -> crate::Result<AggregatedHookResult> {
    let input = PostToolUseInput {
        base: base_from_ctx(ctx),
        tool_name: tool_name.to_string(),
        tool_input: tool_input.clone(),
        tool_response: tool_response.clone(),
        tool_use_id: tool_use_id.to_string(),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        Some(tool_name),
        HookEventType::PostToolUse,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::PostToolUse,
        Some(tool_name),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        event_tx,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::PostToolUse),
    ))
}

/// Execute PostToolUseFailure hooks and return the aggregated result.
///
/// Input: `{tool_name, tool_input, tool_use_id, error, is_interrupt?}`.
#[allow(clippy::too_many_arguments)]
pub async fn execute_post_tool_use_failure(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    tool_name: &str,
    tool_use_id: &str,
    tool_input: &serde_json::Value,
    error: &str,
    is_interrupt: Option<bool>,
    event_tx: Option<&tokio::sync::mpsc::Sender<crate::HookExecutionEvent>>,
) -> crate::Result<AggregatedHookResult> {
    let input = PostToolUseFailureInput {
        base: base_from_ctx(ctx),
        tool_name: tool_name.to_string(),
        tool_input: tool_input.clone(),
        tool_use_id: tool_use_id.to_string(),
        error: error.to_string(),
        is_interrupt,
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        Some(tool_name),
        HookEventType::PostToolUseFailure,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::PostToolUseFailure,
        Some(tool_name),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        event_tx,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::PostToolUseFailure),
    ))
}

/// Execute PreCompact hooks and return custom instructions / display messages.
///
/// Returns `{newCustomInstructions, userDisplayMessage}`.
pub async fn execute_pre_compact(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    trigger: CompactTrigger,
    custom_instructions: Option<&str>,
) -> crate::Result<PreCompactResult> {
    let input = PreCompactInput {
        base: base_from_ctx(ctx),
        trigger,
        custom_instructions: custom_instructions.map(String::from),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::PreCompact,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::PreCompact,
        None,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    let mut new_instructions: Vec<String> = Vec::new();
    let mut display_messages: Vec<String> = Vec::new();

    for r in &results {
        let trimmed = r.output.trim();
        if r.succeeded {
            if !trimmed.is_empty() {
                new_instructions.push(trimmed.to_string());
                display_messages.push(format!(
                    "PreCompact [{}] completed successfully: {trimmed}",
                    r.command
                ));
            } else {
                display_messages.push(format!("PreCompact [{}] completed successfully", r.command));
            }
        } else if !trimmed.is_empty() {
            display_messages.push(format!("PreCompact [{}] failed: {trimmed}", r.command));
        } else {
            display_messages.push(format!("PreCompact [{}] failed", r.command));
        }
    }

    Ok(PreCompactResult {
        new_custom_instructions: if new_instructions.is_empty() {
            None
        } else {
            Some(new_instructions.join("\n\n"))
        },
        user_display_message: if display_messages.is_empty() {
            None
        } else {
            Some(display_messages.join("\n"))
        },
    })
}

/// Result of PreCompact hook execution.
#[derive(Debug, Clone, Default)]
pub struct PreCompactResult {
    pub new_custom_instructions: Option<String>,
    pub user_display_message: Option<String>,
}

/// Execute PostCompact hooks and return display messages.
///
/// Returns `{userDisplayMessage}`.
pub async fn execute_post_compact(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    trigger: CompactTrigger,
    compact_summary: &str,
) -> crate::Result<PostCompactResult> {
    let input = PostCompactInput {
        base: base_from_ctx(ctx),
        trigger,
        compact_summary: compact_summary.to_string(),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::PostCompact,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::PostCompact,
        None,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    let mut display_messages: Vec<String> = Vec::new();
    for r in &results {
        let trimmed = r.output.trim();
        if r.succeeded {
            if !trimmed.is_empty() {
                display_messages.push(format!(
                    "PostCompact [{}] completed successfully: {trimmed}",
                    r.command
                ));
            } else {
                display_messages.push(format!(
                    "PostCompact [{}] completed successfully",
                    r.command
                ));
            }
        } else if !trimmed.is_empty() {
            display_messages.push(format!("PostCompact [{}] failed: {trimmed}", r.command));
        } else {
            display_messages.push(format!("PostCompact [{}] failed", r.command));
        }
    }

    Ok(PostCompactResult {
        user_display_message: if display_messages.is_empty() {
            None
        } else {
            Some(display_messages.join("\n"))
        },
    })
}

/// Result of PostCompact hook execution.
#[derive(Debug, Clone, Default)]
pub struct PostCompactResult {
    pub user_display_message: Option<String>,
}

/// Execute SessionStart hooks.
pub async fn execute_session_start(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    source: SessionStartSource,
    agent_type: Option<&str>,
    model: Option<&str>,
) -> crate::Result<AggregatedHookResult> {
    let (results, agg) =
        execute_session_start_raw(registry, ctx, source, agent_type, model).await?;
    push_sync_hook_events(ctx, HookEventType::SessionStart, &results, &agg).await;
    Ok(agg)
}

/// Execute SessionStart hooks and return the reminder events instead of
/// pushing them into the sync hook buffer.
pub async fn execute_session_start_collect_events(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    source: SessionStartSource,
    agent_type: Option<&str>,
    model: Option<&str>,
) -> crate::Result<SessionStartHookExecution> {
    let (results, aggregate) =
        execute_session_start_raw(registry, ctx, source, agent_type, model).await?;
    let events = build_sync_hook_events(HookEventType::SessionStart, &results, &aggregate);
    Ok(SessionStartHookExecution { aggregate, events })
}

async fn execute_session_start_raw(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    source: SessionStartSource,
    agent_type: Option<&str>,
    model: Option<&str>,
) -> crate::Result<(Vec<SingleHookResult>, AggregatedHookResult)> {
    let input = SessionStartInput {
        base: base_from_ctx(ctx),
        source,
        agent_type: agent_type.map(String::from),
        model: model.map(String::from),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::SessionStart,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::SessionStart,
        Some(source.as_wire_str()),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    let agg = aggregate_results_for_event(&results, Some(HookEventType::SessionStart));
    Ok((results, agg))
}

/// Execute UserPromptSubmit hooks before each turn's LLM call.
///
/// Returns the aggregated result so the caller can:
/// - emit a system warning on `blocking_error` (suppressing the turn),
/// - skip the turn on `prevent_continuation` (keeping the prompt),
/// - drop `additional_contexts` (already pushed onto the sync buffer
///   as `HookEvent::AdditionalContext` so the reminder pipeline emits
///   `hook_additional_context` for the next turn).
pub async fn execute_user_prompt_submit(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    prompt_text: &str,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = UserPromptSubmitInput {
        base: base_from_ctx(ctx),
        prompt: prompt_text.to_string(),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::UserPromptSubmit,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::UserPromptSubmit,
        /*matcher*/ None,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    let agg = aggregate_results_for_event(&results, Some(HookEventType::UserPromptSubmit));
    push_sync_hook_events(ctx, HookEventType::UserPromptSubmit, &results, &agg).await;
    Ok(agg)
}

/// Push the reminder-bearing slice of an aggregated hook result onto
/// `ctx.sync_event_sink`. No-op if no sink is wired.
///
/// Push reminder-bearing hook results onto `ctx.sync_event_sink`.
/// Render gates:
/// - `hook_success` only renders when `hookEvent` is `SessionStart` or
///   `UserPromptSubmit` AND content is non-empty.
/// - The other three reminder kinds always render given non-empty
///   data, so we push them unconditionally per result.
async fn push_sync_hook_events(
    ctx: &OrchestrationContext,
    event: HookEventType,
    results: &[SingleHookResult],
    agg: &AggregatedHookResult,
) {
    let Some(buf) = ctx.sync_event_sink.as_ref() else {
        return;
    };

    let events = build_sync_hook_events(event, results, agg);
    if !events.is_empty() {
        buf.extend(events).await;
    }
}

fn build_sync_hook_events(
    event: HookEventType,
    results: &[SingleHookResult],
    agg: &AggregatedHookResult,
) -> Vec<coco_system_reminder::HookEvent> {
    let kind = match event {
        HookEventType::SessionStart => coco_system_reminder::HookEventKind::SessionStart,
        HookEventType::UserPromptSubmit => coco_system_reminder::HookEventKind::UserPromptSubmit,
        _ => coco_system_reminder::HookEventKind::Other,
    };
    let event_label = match event {
        HookEventType::SessionStart | HookEventType::UserPromptSubmit => event.as_str(),
        _ => "",
    };

    let mut events: Vec<coco_system_reminder::HookEvent> = Vec::new();

    for r in results {
        let trimmed = r.output.trim();
        if r.succeeded
            && !trimmed.is_empty()
            && matches!(
                kind,
                coco_system_reminder::HookEventKind::SessionStart
                    | coco_system_reminder::HookEventKind::UserPromptSubmit
            )
        {
            events.push(coco_system_reminder::HookEvent::Success {
                hook_name: r.command.clone(),
                hook_event: kind,
                content: r.output.clone(),
            });
        }
        if r.blocked {
            let err = agg
                .blocking_error
                .as_ref()
                .map(|e| e.blocking_error.clone())
                .unwrap_or_else(|| r.output.clone());
            events.push(coco_system_reminder::HookEvent::BlockingError {
                hook_name: r.command.clone(),
                command: r.command.clone(),
                error: err,
            });
        }
    }

    if !agg.additional_contexts.is_empty() {
        events.push(coco_system_reminder::HookEvent::AdditionalContext {
            hook_name: event_label.to_string(),
            content: agg.additional_contexts.clone(),
        });
    }

    if agg.prevent_continuation {
        events.push(coco_system_reminder::HookEvent::StoppedContinuation {
            hook_name: event_label.to_string(),
            message: agg.stop_reason.clone().unwrap_or_default(),
        });
    }

    events
}

/// Execute SubagentStart hooks before a subagent begins running.
///
/// Aggregated `additional_contexts` are returned to the caller for
/// injection into the subagent's first user message as
/// `hook_additional_context` attachments.
pub async fn execute_subagent_start(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    agent_type: &str,
    agent_id: &str,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = SubagentStartInput {
        base: base_from_ctx(ctx),
        agent_type: agent_type.to_string(),
        agent_id: agent_id.to_string(),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::SubagentStart,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::SubagentStart,
        Some(agent_type),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::SubagentStart),
    ))
}

/// Execute SubagentStop hooks after a subagent finishes (success, failure,
/// or cancel).
///
/// Input: `{stop_hook_active, agent_id, agent_transcript_path, agent_type, last_assistant_message?}`.
/// `agent_transcript_path` is required on the wire — pass an empty
/// string when the subagent does not persist a transcript.
#[allow(clippy::too_many_arguments)]
pub async fn execute_subagent_stop(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    stop_hook_active: bool,
    agent_type: &str,
    agent_id: &str,
    agent_transcript_path: &str,
    last_assistant_message: Option<&str>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = SubagentStopInput {
        base: base_from_ctx(ctx),
        stop_hook_active,
        agent_type: agent_type.to_string(),
        agent_id: agent_id.to_string(),
        agent_transcript_path: agent_transcript_path.to_string(),
        last_assistant_message: last_assistant_message.map(String::from),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::SubagentStop,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::SubagentStop,
        Some(agent_type),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::SubagentStop),
    ))
}

/// Execute SessionEnd hooks with a tighter timeout.
pub async fn execute_session_end(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    reason: ExitReason,
) -> crate::Result<Vec<SingleHookResult>> {
    let timeout = session_end_timeout();
    let input = SessionEndInput {
        base: base_from_ctx(ctx),
        reason,
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::SessionEnd,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::SessionEnd,
        Some(reason.as_wire_str()),
        &json_input,
        &env,
        &ctx.cancel,
        timeout,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(results)
}

/// Execute StopFailure hooks.
///
/// Execute `Stop` hooks and return the aggregated result.
///
/// Stop hooks fire when a turn ends naturally (no tool calls, `end_turn` stop).
/// A blocking Stop hook's feedback is injected back into the conversation and
/// the loop continues.
///
/// `stop_hook_active` is `true` when this Stop firing is the loop's
/// reentrant call after a previous Stop hook blocked.
/// `last_assistant_message` carries the final assistant-text payload so
/// hooks can read it without parsing the transcript file.
pub async fn execute_stop(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    stop_hook_active: bool,
    last_assistant_message: Option<&str>,
    history: &[std::sync::Arc<coco_messages::Message>],
    event_tx: Option<&tokio::sync::mpsc::Sender<crate::HookExecutionEvent>>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = StopInput {
        base: base_from_ctx(ctx),
        stop_hook_active,
        last_assistant_message: last_assistant_message.map(String::from),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::Stop,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::Stop,
        None,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        event_tx,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    let mut agg = aggregate_results_for_event(&results, Some(HookEventType::Stop));
    apply_function_hook_results(
        &mut agg,
        evaluate_function_hooks(registry, HookEventType::Stop, None, history).await,
    );
    Ok(agg)
}

/// Merge function-hook results into the aggregate result of settings
/// hooks. Iteration is in **registration order** so a function hook
/// registered before any settings hook fires first; this matches
/// `Promise.all` resolution order where hooks share one stream.
///
/// `blocking_error` is **first-blocker-wins**: whichever side (settings
/// or function) wrote to the slot first keeps it. Settings hooks
/// always run before function hooks within `execute_stop`, so if both
/// block, the settings-hook message is the one rendered to the user.
/// That's an implementation gap noted in
/// [`coco-hooks/CLAUDE.md`](../../CLAUDE.md) "Function hooks";
/// settings/function priority becomes observable only when both block
/// the same event, which no in-tree use case does today.
fn apply_function_hook_results(
    agg: &mut AggregatedHookResult,
    function_results: Vec<FunctionHookEvalResult>,
) {
    for r in function_results {
        if !r.passed && agg.blocking_error.is_none() {
            agg.blocking_error = Some(HookBlockingError {
                blocking_error: r.error_message,
                source: HookBlockingSource::Function { hook_id: r.id },
            });
        }
    }
}

/// One function-hook predicate's outcome.
#[derive(Debug, Clone)]
struct FunctionHookEvalResult {
    id: String,
    passed: bool,
    error_message: String,
}

/// Evaluate every function hook registered for `event` matching
/// `matcher` against the supplied history.
///
/// Each predicate runs on its own [`tokio::task::spawn_blocking`]
/// thread (predicates are sync per [`crate::FunctionHookPredicate`]'s
/// contract) under [`tokio::time::timeout`] of the hook's configured
/// timeout. Predicates that panic or time out are treated as
/// `passed = false` so the safe default is "block Stop and re-prompt
/// the model".
///
/// All predicates fan out **in parallel** via
/// [`futures::future::join_all`]. The result `Vec` preserves registration
/// order so [`apply_function_hook_results`]' first-blocker-wins
/// reduction is deterministic across runs.
///
/// `history` is shared across spawned tasks via a single
/// [`std::sync::Arc`] — no per-hook `Vec` clone. For an N-hook +
/// M-message history this is O(N + M) instead of O(N·M).
async fn evaluate_function_hooks(
    registry: &HookRegistry,
    event: HookEventType,
    matcher: Option<&str>,
    history: &[std::sync::Arc<coco_messages::Message>],
) -> Vec<FunctionHookEvalResult> {
    let hooks = registry.find_matching_function_hooks(event, matcher);
    if hooks.is_empty() {
        return Vec::new();
    }
    // Single allocation; each spawned task gets an Arc::clone (one
    // atomic refcount bump) instead of cloning the Vec.
    let history: std::sync::Arc<Vec<std::sync::Arc<coco_messages::Message>>> =
        std::sync::Arc::new(history.to_vec());

    let futures = hooks.into_iter().map(|hook| {
        let history = history.clone();
        let predicate = hook.predicate.clone();
        let name_for_log = predicate.name().to_string();
        let timeout = hook.timeout;
        let id = hook.id.clone();
        let error_message = hook.error_message.clone();
        async move {
            let join = tokio::task::spawn_blocking(move || predicate.evaluate(&history));
            let passed = match tokio::time::timeout(timeout, join).await {
                Ok(Ok(b)) => b,
                Ok(Err(e)) => {
                    tracing::warn!(
                        hook_id = %id,
                        predicate = %name_for_log,
                        error = %e,
                        "function hook predicate panicked; treating as failed"
                    );
                    false
                }
                Err(_) => {
                    tracing::warn!(
                        hook_id = %id,
                        predicate = %name_for_log,
                        timeout_ms = timeout.as_millis() as u64,
                        "function hook predicate timed out; treating as failed"
                    );
                    false
                }
            };
            FunctionHookEvalResult {
                id,
                passed,
                error_message,
            }
        }
    });

    futures::future::join_all(futures).await
}

pub async fn execute_stop_failure(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    error: &str,
    error_details: Option<&str>,
    last_assistant_message: Option<&str>,
) -> crate::Result<Vec<SingleHookResult>> {
    let input = StopFailureInput {
        base: base_from_ctx(ctx),
        error: error.to_string(),
        error_details: error_details.map(String::from),
        last_assistant_message: last_assistant_message.map(String::from),
    };

    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::StopFailure,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );

    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::StopFailure,
        Some(error),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        /*event_tx*/ None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;

    Ok(results)
}

// ---------------------------------------------------------------------------
// Event-specific entry points
// ---------------------------------------------------------------------------
//
// Each helper builds the appropriate input, populates the env, runs
// `execute_hooks_parallel_filtered`, and aggregates with the matching
// `expected_event`. Trigger sites in coco-rs subsystems call these
// directly — none of them need anything beyond `OrchestrationContext`.

/// Execute Setup hooks (init / maintenance triggers).
pub async fn execute_setup(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    trigger: SetupTrigger,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::SetupInput {
        base: base_from_ctx(ctx),
        trigger,
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::Setup,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::Setup,
        Some(trigger.as_wire_str()),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::Setup),
    ))
}

/// Execute Notification hooks (e.g. permission_prompt, idle_prompt).
///
/// Input: `{message, title?, notification_type}`.
pub async fn execute_notification(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    notification_type: &str,
    message: &str,
    title: Option<&str>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::NotificationInput {
        base: base_from_ctx(ctx),
        notification_type: notification_type.to_string(),
        message: message.to_string(),
        title: title.map(String::from),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::Notification,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::Notification,
        Some(notification_type),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::Notification),
    ))
}

/// Execute PermissionRequest hooks. Output `hookSpecificOutput.decision`
/// drives the dialog's allow / deny outcome.
///
/// Input: `{tool_name, tool_input, permission_suggestions?}`
/// — `tool_use_id` is NOT included on this event.
pub async fn execute_permission_request(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    tool_name: &str,
    tool_input: &serde_json::Value,
    permission_suggestions: Option<&serde_json::Value>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::PermissionRequestInput {
        base: base_from_ctx(ctx),
        tool_name: tool_name.to_string(),
        tool_input: tool_input.clone(),
        permission_suggestions: permission_suggestions.cloned(),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        Some(tool_name),
        HookEventType::PermissionRequest,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::PermissionRequest,
        Some(tool_name),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::PermissionRequest),
    ))
}

/// Execute PermissionDenied hooks (after the auto-mode classifier rules
/// the call out). Output's `retry: true` lets the model retry.
///
/// Input: `{tool_name, tool_input, tool_use_id, reason}`.
pub async fn execute_permission_denied(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    tool_name: &str,
    tool_use_id: &str,
    tool_input: &serde_json::Value,
    reason: &str,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::PermissionDeniedInput {
        base: base_from_ctx(ctx),
        tool_name: tool_name.to_string(),
        tool_input: tool_input.clone(),
        tool_use_id: tool_use_id.to_string(),
        reason: reason.to_string(),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        Some(tool_name),
        HookEventType::PermissionDenied,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::PermissionDenied,
        Some(tool_name),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::PermissionDenied),
    ))
}

/// Execute Elicitation hooks (MCP elicitation gating).
///
/// Input: `{mcp_server_name, message, mode?, url?, elicitation_id?, requested_schema?}`.
#[allow(clippy::too_many_arguments)]
pub async fn execute_elicitation(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    mcp_server_name: &str,
    message: &str,
    mode: Option<ElicitationMode>,
    url: Option<&str>,
    elicitation_id: Option<&str>,
    requested_schema: Option<&serde_json::Value>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::ElicitationInput {
        base: base_from_ctx(ctx),
        mcp_server_name: mcp_server_name.to_string(),
        message: message.to_string(),
        mode,
        url: url.map(String::from),
        elicitation_id: elicitation_id.map(String::from),
        requested_schema: requested_schema.cloned(),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::Elicitation,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::Elicitation,
        Some(mcp_server_name),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::Elicitation),
    ))
}

/// Execute ElicitationResult hooks (after the user responds to MCP).
///
/// Input: `{mcp_server_name, elicitation_id?, mode?, action, content?}`.
pub async fn execute_elicitation_result(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    mcp_server_name: &str,
    elicitation_id: Option<&str>,
    mode: Option<ElicitationMode>,
    action: ElicitationAction,
    content: Option<&serde_json::Value>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::ElicitationResultInput {
        base: base_from_ctx(ctx),
        mcp_server_name: mcp_server_name.to_string(),
        elicitation_id: elicitation_id.map(String::from),
        mode,
        action,
        content: content.cloned(),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::ElicitationResult,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::ElicitationResult,
        Some(mcp_server_name),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::ElicitationResult),
    ))
}

/// Execute ConfigChange hooks (settings file mutated mid-session).
pub async fn execute_config_change(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    source: ConfigChangeSource,
    file_path: Option<&str>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::ConfigChangeInput {
        base: base_from_ctx(ctx),
        source,
        file_path: file_path.map(String::from),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::ConfigChange,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::ConfigChange,
        Some(source.as_wire_str()),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::ConfigChange),
    ))
}

/// Execute InstructionsLoaded hooks (CLAUDE.md / rule discovery).
///
/// Input: `{file_path, memory_type, load_reason, globs?, trigger_file_path?, parent_file_path?}`.
#[allow(clippy::too_many_arguments)]
pub async fn execute_instructions_loaded(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    file_path: &str,
    memory_type: MemoryType,
    load_reason: InstructionsLoadReason,
    globs: Option<Vec<String>>,
    trigger_file_path: Option<&str>,
    parent_file_path: Option<&str>,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::InstructionsLoadedInput {
        base: base_from_ctx(ctx),
        file_path: file_path.to_string(),
        memory_type,
        load_reason,
        globs,
        trigger_file_path: trigger_file_path.map(String::from),
        parent_file_path: parent_file_path.map(String::from),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::InstructionsLoaded,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::InstructionsLoaded,
        Some(load_reason.as_wire_str()),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::InstructionsLoaded),
    ))
}

/// Execute CwdChanged hooks (working directory swap).
pub async fn execute_cwd_changed(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    old_cwd: &str,
    new_cwd: &str,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::CwdChangedInput {
        base: base_from_ctx(ctx),
        old_cwd: old_cwd.to_string(),
        new_cwd: new_cwd.to_string(),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        new_cwd,
        None,
        HookEventType::CwdChanged,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::CwdChanged,
        None,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::CwdChanged),
    ))
}

/// Execute FileChanged hooks (a watched file changed).
///
/// Callers wire this from external file-watch infrastructure.
pub async fn execute_file_changed(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    file_path: &str,
    event: FileChangeEvent,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::FileChangedInput {
        base: base_from_ctx(ctx),
        file_path: file_path.to_string(),
        event,
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::FileChanged,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    // Match FileChanged hooks against the basename of `file_path`.
    let basename = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string);
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::FileChanged,
        basename.as_deref(),
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::FileChanged),
    ))
}

/// Execute WorktreeCreate hook. The hook's stdout (or
/// `hookSpecificOutput.worktreePath`) holds the absolute worktree path.
pub async fn execute_worktree_create(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    name: &str,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::WorktreeCreateInput {
        base: base_from_ctx(ctx),
        name: name.to_string(),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::WorktreeCreate,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::WorktreeCreate,
        None,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::WorktreeCreate),
    ))
}

/// Execute WorktreeRemove hook.
pub async fn execute_worktree_remove(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    worktree_path: &str,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let input = crate::inputs::WorktreeRemoveInput {
        base: base_from_ctx(ctx),
        worktree_path: worktree_path.to_string(),
    };
    let json_input = serde_json::to_string(&input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        HookEventType::WorktreeRemove,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        HookEventType::WorktreeRemove,
        None,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(
        &results,
        Some(HookEventType::WorktreeRemove),
    ))
}

/// Run one of the task-shaped events through the parallel executor.
/// `task_type` is the matcher field for TaskCreated / TaskCompleted;
/// TeammateIdle uses `None` to match-all.
async fn run_event_with_input<I: serde::Serialize>(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    event: HookEventType,
    matcher: Option<&str>,
    input: &I,
) -> crate::Result<AggregatedHookResult> {
    if ctx.disable_all_hooks {
        return Ok(AggregatedHookResult::default());
    }
    let json_input = serde_json::to_string(input)?;
    let env = build_hook_env(
        &ctx.session_id,
        &ctx.cwd.to_string_lossy(),
        None,
        event,
        ctx.project_dir.as_deref().and_then(|p| p.to_str()),
    );
    let results = execute_hooks_parallel_filtered(
        registry,
        event,
        matcher,
        &json_input,
        &env,
        &ctx.cancel,
        DEFAULT_HOOK_TIMEOUT,
        None,
        &ctx.attachment_emitter,
        ctx.allow_managed_hooks_only,
        ctx.http_url_allowlist.as_deref(),
        ctx.http_env_var_policy.as_deref(),
        ctx.async_registry.as_ref(),
        ctx.async_rewake_sink.as_ref(),
        ctx.llm_handle.as_ref(),
        ctx.workspace_trust_accepted,
    )
    .await;
    Ok(aggregate_results_for_event(&results, Some(event)))
}

/// Execute TaskCreated hooks.
///
/// Input: `{task_id, task_subject, task_description?, teammate_name?, team_name?}`.
pub async fn execute_task_created(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    task_id: &str,
    task_subject: &str,
    task_description: Option<&str>,
    teammate_name: Option<&str>,
    team_name: Option<&str>,
) -> crate::Result<AggregatedHookResult> {
    let input = crate::inputs::TaskCreatedInput {
        base: base_from_ctx(ctx),
        task_id: task_id.to_string(),
        task_subject: task_subject.to_string(),
        task_description: task_description.map(String::from),
        teammate_name: teammate_name.map(String::from),
        team_name: team_name.map(String::from),
    };
    run_event_with_input(registry, ctx, HookEventType::TaskCreated, None, &input).await
}

/// Execute TaskCompleted hooks.
pub async fn execute_task_completed(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    task_id: &str,
    task_subject: &str,
    task_description: Option<&str>,
    teammate_name: Option<&str>,
    team_name: Option<&str>,
) -> crate::Result<AggregatedHookResult> {
    let input = crate::inputs::TaskCompletedInput {
        base: base_from_ctx(ctx),
        task_id: task_id.to_string(),
        task_subject: task_subject.to_string(),
        task_description: task_description.map(String::from),
        teammate_name: teammate_name.map(String::from),
        team_name: team_name.map(String::from),
    };
    run_event_with_input(registry, ctx, HookEventType::TaskCompleted, None, &input).await
}

/// Execute TeammateIdle hooks.
///
/// Input: `{teammate_name, team_name}`.
pub async fn execute_teammate_idle(
    registry: &HookRegistry,
    ctx: &OrchestrationContext,
    teammate_name: &str,
    team_name: &str,
) -> crate::Result<AggregatedHookResult> {
    let input = crate::inputs::TeammateIdleInput {
        base: base_from_ctx(ctx),
        teammate_name: teammate_name.to_string(),
        team_name: team_name.to_string(),
    };
    run_event_with_input(registry, ctx, HookEventType::TeammateIdle, None, &input).await
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a blocking error from a PreToolUse hook.
pub fn format_pre_tool_blocking_message(hook_name: &str, error: &HookBlockingError) -> String {
    format!("{hook_name} hook error: {}", error.blocking_error)
}

/// Format a blocking error from a Stop hook.
pub fn format_stop_hook_message(error: &HookBlockingError) -> String {
    format!("Stop hook feedback:\n{}", error.blocking_error)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

// base_from_ctx() is now in crate::inputs and re-exported above.

/// Bridge between the spawn loop and `HookHandler::Prompt` /
/// `HookHandler::Agent` execution paths.
///
/// When `llm_handle` is `Some`, Prompt and Agent handlers route through
/// the LLM and yield a `CommandOutput` shaped as stdout JSON
/// `{"decision": "block", "reason": "..."}` so the existing
/// JSON-output code path in `aggregate_results` interprets the
/// blocking/success state without any new branches. When `llm_handle`
/// is `None`, falls back to the legacy passthrough in [`execute_hook`].
struct HookExecutionRequest<'a> {
    handler: &'a HookHandler,
    env_vars: &'a std::collections::HashMap<String, String>,
    stdin_input: Option<&'a str>,
    llm_handle: Option<&'a std::sync::Arc<dyn crate::llm_handle::HookLlmHandle>>,
    sdk_hook_callback: Option<&'a crate::SdkHookCallback>,
    event: HookEventType,
    timeout: Duration,
    async_options: Option<crate::AsyncCommandOptions>,
}

async fn run_hook_via_handle_or_fallback(
    request: HookExecutionRequest<'_>,
) -> crate::Result<HookExecutionResult> {
    use crate::llm_handle::HookEvaluationResult;

    let HookExecutionRequest {
        handler,
        env_vars,
        stdin_input,
        llm_handle,
        sdk_hook_callback,
        event,
        timeout,
        async_options,
    } = request;

    if let HookHandler::SdkCallback { callback_id, .. } = handler {
        let Some(callback) = sdk_hook_callback else {
            return Err(crate::HooksError::generic(format!(
                "SDK hook callback {callback_id:?} is not installed"
            )));
        };
        // The hook input is the already-serialized JSON the orchestrator
        // built — parse to a `Value` so the callback receives the same
        // shape it would have over the SDK wire.
        let input = match stdin_input {
            Some(raw) => serde_json::from_str(raw)?,
            None => serde_json::Value::Null,
        };
        // `tool_use_id` is on the typed input struct; for callback
        // dispatch we re-extract it from the serialized form so the
        // callback signature stays stable across all event types.
        let tool_use_id = input
            .get("tool_use_id")
            .or_else(|| input.get("toolUseID"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // Typed end-to-end: callback returns `SdkHookOutput` directly,
        // we wrap as `HookExecutionResult::SdkOutput`, aggregation
        // applies it via `apply_sdk_hook_output` — no JSON round-trip,
        // no `parse_hook_output` rescue.
        let output = callback(crate::SdkHookCallbackRequest {
            callback_id: callback_id.clone(),
            event,
            input,
            tool_use_id,
        })
        .await?;
        return Ok(HookExecutionResult::SdkOutput(output));
    }

    let Some(llm) = llm_handle else {
        return match async_options {
            Some(options) => {
                crate::execute_hook_with_async_options(handler, env_vars, stdin_input, options)
                    .await
            }
            None => execute_hook(handler, env_vars, stdin_input).await,
        };
    };

    let (prompt, model, is_agent) = match handler {
        HookHandler::Prompt { prompt, model, .. } => (prompt.clone(), model.clone(), false),
        HookHandler::Agent { prompt, model, .. } => (prompt.clone(), model.clone(), true),
        // Command / Http aren't LLM-driven — pass through.
        _ => {
            return match async_options {
                Some(options) => {
                    crate::execute_hook_with_async_options(handler, env_vars, stdin_input, options)
                        .await
                }
                None => execute_hook(handler, env_vars, stdin_input).await,
            };
        }
    };

    // Substitute `$ARGUMENTS` with the serialized hook input JSON
    // before the LLM call. Implementations that want richer substitution
    // can do it inside `evaluate_*`.
    let processed = match stdin_input {
        Some(args) => prompt.replace("$ARGUMENTS", args),
        None => prompt,
    };

    let outcome = if is_agent {
        llm.evaluate_agent(&processed, model.as_deref(), timeout)
            .await
    } else {
        llm.evaluate_prompt(&processed, model.as_deref(), timeout)
            .await
    };

    // Map evaluation outcomes back onto the JSON-output shape that
    // `aggregate_results` already understands. exit_code 2 = blocking,
    // 1 = non-blocking error, 0 = success.
    let (exit_code, stdout, stderr) = match outcome {
        HookEvaluationResult::Ok => (0, String::new(), String::new()),
        HookEvaluationResult::Blocking { reason } => {
            let body = serde_json::json!({
                "decision": "block",
                "reason": reason,
            })
            .to_string();
            (2, body, String::new())
        }
        HookEvaluationResult::Cancelled => (1, String::new(), "hook cancelled".to_string()),
        HookEvaluationResult::NonBlockingError { error } => (1, String::new(), error),
    };
    Ok(HookExecutionResult::CommandOutput {
        exit_code,
        stdout,
        stderr,
    })
}

/// Resolve timeout for a hook handler — uses the handler's explicit
/// timeout if set, otherwise a handler-type default.
///
/// Command / Http / SdkCallback fall back to the event-supplied
/// `default`. Prompt and Agent hooks are LLM-driven and use shorter
/// defaults (30s / 60s) independent of the generic 10-minute
/// tool-hook timeout, so an unconfigured judge can't hang for minutes.
fn resolve_timeout(handler: &HookHandler, default: Duration) -> Duration {
    let (explicit, handler_default) = match handler {
        HookHandler::Command { timeout_ms, .. } => (*timeout_ms, default),
        HookHandler::Http { timeout_ms, .. } => (*timeout_ms, default),
        HookHandler::Prompt { timeout_ms, .. } => (*timeout_ms, DEFAULT_PROMPT_HOOK_TIMEOUT),
        HookHandler::Agent { timeout_ms, .. } => (*timeout_ms, DEFAULT_AGENT_HOOK_TIMEOUT),
        HookHandler::SdkCallback { timeout_ms, .. } => (*timeout_ms, default),
    };
    explicit
        .and_then(|ms| u64::try_from(ms).ok())
        .map(Duration::from_millis)
        .unwrap_or(handler_default)
}

/// Derive the [`HookBlockingSource`] provenance for a handler. Every
/// handler maps to exactly one source variant — no implicit default,
/// so a new handler type fails compilation here instead of silently
/// defaulting to `Command`.
fn derive_handler_source(handler: &HookHandler) -> HookBlockingSource {
    match handler {
        HookHandler::Command { command, .. } => HookBlockingSource::Command(command.clone()),
        HookHandler::Http { url, .. } => HookBlockingSource::Http(url.clone()),
        HookHandler::Prompt { .. } | HookHandler::Agent { .. } => HookBlockingSource::Llm,
        HookHandler::SdkCallback { callback_id, .. } => HookBlockingSource::Sdk {
            callback_id: callback_id.clone(),
        },
    }
}

/// Human-readable label for a hook handler (used in result reporting).
fn handler_label(handler: &HookHandler) -> String {
    match handler {
        HookHandler::Command { command, .. } => command.clone(),
        HookHandler::Prompt { prompt, .. } => format!("prompt:{prompt}"),
        HookHandler::Http { url, .. } => url.clone(),
        HookHandler::Agent { prompt, .. } => format!("agent:{prompt}"),
        HookHandler::SdkCallback { callback_id, .. } => format!("sdk:{callback_id}"),
    }
}

/// Process a raw `HookExecutionResult` into a `SingleHookResult`.
fn process_execution_result(
    exec: HookExecutionResult,
    label: &str,
    source: HookBlockingSource,
    event: HookEventType,
    emitter: &AttachmentEmitter,
) -> SingleHookResult {
    match exec {
        HookExecutionResult::CommandOutput {
            exit_code,
            stdout,
            stderr,
        } => {
            // Exit code 2 is the "blocking error" convention.
            let parsed = parse_hook_output(&stdout);
            let stdout_has_json_control = matches!(parsed, ParsedHookOutput::Json(_));
            // Valid-JSON-but-wrong-shape stdout is a non-blocking validation
            // error: surface it for UI/audit. Aggregation suppresses the raw
            // text from model context.
            if let ParsedHookOutput::ValidationError(msg) = &parsed {
                emitter.emit(AttachmentMessage::silent_hook_non_blocking_error(
                    HookNonBlockingErrorPayload {
                        error: msg.clone(),
                        hook_name: label.to_string(),
                        tool_use_id: String::new(),
                        hook_event: event,
                    },
                ));
            }
            let blocked = exit_code == 2 && !stdout_has_json_control;
            // A non-zero, non-2 plain exit yields ONLY a
            // `hook_non_blocking_error` carrying the stderr — it never becomes
            // model context. Emit that attachment here (the aggregator already
            // suppresses the failed-hook stdout from `additional_contexts`).
            if exit_code != 0 && exit_code != 2 && !stdout_has_json_control {
                let trimmed = stderr.trim();
                let detail = if trimmed.is_empty() {
                    "No stderr output".to_string()
                } else {
                    trimmed.to_string()
                };
                emitter.emit(AttachmentMessage::silent_hook_non_blocking_error(
                    HookNonBlockingErrorPayload {
                        error: format!("Failed with non-blocking status code: {detail}"),
                        hook_name: label.to_string(),
                        tool_use_id: String::new(),
                        hook_event: event,
                    },
                ));
            }
            let output = if stdout_has_json_control || exit_code == 0 {
                stdout
            } else {
                stderr
            };
            SingleHookResult {
                command: label.to_string(),
                succeeded: exit_code == 0,
                output,
                blocked,
                outcome: match exit_code {
                    0 => HookOutcome::Success,
                    2 => HookOutcome::Blocking,
                    _ => HookOutcome::NonBlockingError,
                },
                status_message: None,
                async_rewake: false,
                source,
                sdk_output: None,
            }
        }
        HookExecutionResult::PromptText(text) => SingleHookResult {
            command: label.to_string(),
            succeeded: true,
            output: text,
            blocked: false,
            outcome: HookOutcome::Success,
            status_message: None,
            async_rewake: false,
            source,
            sdk_output: None,
        },
        HookExecutionResult::SdkOutput(out) => {
            // Compute `blocked` directly from the typed output:
            // top-level `decision: 'block'` or
            // `hookSpecificOutput.PreToolUse.permissionDecision: 'deny'`
            // both signal a blocking-error result. Elicitation
            // `action: decline` is also a block. Everything else is a
            // non-blocking success — even with `continue: false`
            // (that's `prevent_continuation`, not a blocking error).
            let blocked = is_sdk_output_blocking(&out);
            SingleHookResult {
                command: label.to_string(),
                succeeded: true,
                output: String::new(),
                blocked,
                outcome: if blocked {
                    HookOutcome::Blocking
                } else {
                    HookOutcome::Success
                },
                status_message: None,
                async_rewake: false,
                source,
                sdk_output: Some(out),
            }
        }
    }
}

/// Determine whether a typed `SdkHookOutput` should be treated as a
/// blocking result. Mirrors the rules `aggregate_results_for_event`
/// applies — used here to set `SingleHookResult.blocked` consistently
/// before aggregation sees it.
fn is_sdk_output_blocking(out: &coco_types::SdkHookOutput) -> bool {
    use coco_types::HookDecision;
    use coco_types::HookPermissionDecision;
    use coco_types::HookSpecificOutput;
    if out.decision == Some(HookDecision::Block) {
        return true;
    }
    if let Some(spec) = &out.hook_specific_output {
        match spec {
            HookSpecificOutput::PreToolUse {
                permission_decision: Some(HookPermissionDecision::Deny),
                ..
            } => return true,
            HookSpecificOutput::PermissionRequest {
                decision: Some(coco_types::PermissionRequestDecision::Deny { .. }),
            } => return true,
            HookSpecificOutput::Elicitation {
                action: Some(coco_types::ElicitationAction::Decline),
                ..
            } => return true,
            HookSpecificOutput::ElicitationResult {
                action: Some(coco_types::ElicitationAction::Decline),
                ..
            } => return true,
            _ => {}
        }
    }
    false
}

/// Get the SessionEnd hook timeout, optionally overridden via env var.
fn session_end_timeout() -> Duration {
    env::env_opt(EnvKey::CocoSessionEndHooksTimeoutMs)
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|&ms| ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(SESSION_END_HOOK_TIMEOUT)
}

#[cfg(test)]
#[path = "orchestration.test.rs"]
mod tests;
