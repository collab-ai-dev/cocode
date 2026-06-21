//! `coco-query`'s implementation of [`coco_hooks::HookLlmHandle`].
//!
//! Bridges the `Prompt` and `Agent` hook handler types to the parent
//! session's model runtime registry. Hooks-crate sits at L4; inference at L2 —
//! the trait lives in `coco-hooks` and is implemented here so the
//! L4 → L2 dependency arrow is reversed.
//!
//! # Status
//!
//! - **Prompt path**: full implementation. Builds a single-turn
//!   `QueryParams`, calls the registry runtime, parses the assistant
//!   text as `{ok: bool, reason?: string}` JSON. Recursion-safe:
//!   bypasses the `QueryEngine` turn loop entirely so
//!   `UserPromptSubmit` hooks don't fire from within a hook
//!   evaluation.
//!
//! - **Agent path**: full hook verdict path via a late-bound runner
//!   installed by `coco-cli::session_runtime`. The concrete runner
//!   builds a scoped child `QueryEngine` with `max_turns = 50`, a
//!   `StructuredOutputTool`, and a Stop enforcement function hook so
//!   the child must produce `{ok, reason?}`. `{ok:false}` maps to a
//!   blocking hook result (feedback prefixed `Agent hook condition was
//!   not met: `); max-turn/no-output still maps to `Cancelled`. The
//!   runner uses a verifier sandbox: `ALL_AGENT_DISALLOWED_TOOLS` are
//!   withheld, a dedicated verifier system prompt replaces the main
//!   one, thinking is disabled, and the default timeout is 60s. The
//!   explicit `Read(/transcriptPath)` session grant is not separately
//!   threaded; the transcript path reaches the child via the Stop hook
//!   input JSON in the processed prompt.
//!
//! # Model selection
//!
//! The per-hook `hook.model` field can override with either a literal
//! model id or an alias. Coco-rs routes through `ModelRole::HookAgent`
//! — bare model strings are deliberately rejected per the project rule
//! "never bare model string; route via `ModelRole`" (see root `CLAUDE.md`).
//!
//! - **Default runtime** — [`QueryHookLlm::for_session`] snapshots
//!   `ModelRole::HookAgent` from the shared
//!   [`coco_inference::ModelRuntimeRegistry`] at session bootstrap. Users
//!   who set `models.hook_agent` in settings.json get that model for
//!   every hook evaluation. Unconfigured roles inherit Main's spec
//!   via the cache's spec-equality shortcut (no redundant client
//!   built, detector baseline preserved).
//!
//! - **Per-call override** — the `model` parameter on
//!   [`HookLlmHandle::evaluate_prompt`] / `evaluate_agent` is parsed
//!   as a [`ModelRole`] (`"main"` / `"fast"` / `"explore"` / `"review"` /
//!   `"hook_agent"` / `"memory"` / `"subagent"` / `"plan"`, case-
//!   insensitive). Recognised roles route through the shared cache.
//!   Unrecognised strings fall through to the default client with a
//!   warn log so user misconfigurations are visible — and tell the
//!   user to either set `models.hook_agent` and omit `model`, or
//!   use a role name.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use coco_hooks::HookEvaluationResult;
use coco_hooks::HookLlmHandle;
use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::UserContentPart;
use coco_types::ModelRole;
use serde::Deserialize;

/// System prompt prepended to every Prompt hook evaluation.
///
/// The schema constraint is enforced by JSON parse rather than a
/// provider-level `output_format` (which is not yet wired in
/// `coco-inference`).
const HOOK_PROMPT_SYSTEM: &str = "You are evaluating a hook in Claude Code.

Your response must be a JSON object matching one of the following schemas:
1. If the condition is met, return: {\"ok\": true}
2. If the condition is not met, return: {\"ok\": false, \"reason\": \"Reason for why it is not met\"}";

/// JSON shape the hook prompt is expected to produce.
#[derive(Debug, Clone, Deserialize)]
struct HookResponse {
    ok: bool,
    #[serde(default)]
    reason: Option<String>,
}

/// `coco-query`'s `HookLlmHandle` implementation. Single struct for
/// both Prompt and Agent paths — they share `model_runtimes` and the
/// `Cancelled`/`NonBlockingError` mapping logic.
/// Manual `Debug` surfaces only the default model id.
pub struct QueryHookLlm {
    model_runtimes: Arc<ModelRuntimeRegistry>,
    default_model_id: String,
    agent_runner: tokio::sync::RwLock<Option<HookAgentRunnerRef>>,
}

impl std::fmt::Debug for QueryHookLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryHookLlm")
            .field("default_model_id", &self.default_model_id)
            .finish()
    }
}

impl QueryHookLlm {
    /// Build a session-wired hook handler. Pre-resolves
    /// `ModelRole::HookAgent` against the shared cache as the default
    /// runtime and stores the registry so per-call `model` overrides
    /// reach the user-configured role runtimes.
    ///
    /// When `HookAgent` is unconfigured the fallback chain in
    /// `runtime.rs:resolve_model_roles` populates it with Main's spec;
    /// the cache's spec-equality shortcut reuses the Main `Arc` so the
    /// common case stays zero-extra-allocation.
    pub async fn for_session(model_runtimes: Arc<ModelRuntimeRegistry>) -> Self {
        let default_model_id = model_runtimes
            .snapshot_for_role(ModelRole::HookAgent)
            .or_else(|e| {
                tracing::warn!(
                    error = %e,
                    "HookAgent role unresolved at hook-handle bootstrap; falling back to Main role"
                );
                model_runtimes.snapshot_for_role(ModelRole::Main)
            })
            .map(|snapshot| snapshot.model_id)
            .unwrap_or_else(|_| "unknown".to_string());
        Self {
            model_runtimes,
            default_model_id,
            agent_runner: tokio::sync::RwLock::new(None),
        }
    }

    /// Late-bind the real Agent hook runner. SessionRuntime installs
    /// this after it has an `Arc<Self>` so the runner can build scoped
    /// child engines without creating an ownership cycle during
    /// bootstrap.
    pub async fn install_agent_runner(&self, runner: HookAgentRunnerRef) {
        *self.agent_runner.write().await = Some(runner);
    }

    /// Pick the runtime source for a single hook invocation.
    ///
    /// Precedence (adapted to coco-rs's `ModelRole` indirection):
    /// 1. `model = Some(m)` and `m` parses as a `ModelRole` → resolve
    ///    that role via the shared cache (`Err` falls back to
    ///    `default_client` with a warn).
    /// 2. `model = Some(m)` and `m` is not a recognised role → warn
    ///    and use `default_client`. The warn message tells the user
    ///    that `hook.model` accepts role names, not bare model ids.
    /// 3. `model = None` → `default_client` (= HookAgent role).
    fn pick_source(&self, model: Option<&str>) -> ModelRuntimeSource {
        let Some(m) = model else {
            return ModelRuntimeSource::Role(ModelRole::HookAgent);
        };
        match ModelRole::from_str(m) {
            Ok(role) => ModelRuntimeSource::Role(role),
            Err(_) => {
                tracing::warn!(
                    requested_model = m,
                    "hook `model` is not a recognised ModelRole (expected one of \
                     main/fast/plan/explore/review/hook_agent/memory/subagent); \
                     set `models.hook_agent` in settings.json and omit `model`, \
                     or pass a role name. Falling back to HookAgent default."
                );
                ModelRuntimeSource::Role(ModelRole::HookAgent)
            }
        }
    }
}

/// Request passed from [`QueryHookLlm`] to the runtime-specific Agent
/// hook runner.
#[derive(Debug, Clone)]
pub struct HookAgentRunRequest {
    pub prompt: String,
    pub model_source: ModelRuntimeSource,
    pub model_id: String,
    pub timeout: Duration,
}

#[async_trait]
pub trait HookAgentRunner: Send + Sync + std::fmt::Debug {
    async fn run(&self, request: HookAgentRunRequest) -> HookEvaluationResult;
}

pub type HookAgentRunnerRef = Arc<dyn HookAgentRunner>;

#[async_trait]
impl HookLlmHandle for QueryHookLlm {
    async fn evaluate_prompt(
        &self,
        prompt: &str,
        model: Option<&str>,
        timeout: Duration,
    ) -> HookEvaluationResult {
        let source = self.pick_source(model);

        let prompt = build_prompt(prompt);

        let result = async {
            loop {
                let params = QueryParams {
                    prompt: prompt.clone(),
                    max_tokens: Some(1024),
                    thinking_level: None,
                    fast_mode: false,
                    tools: None,
                    tool_choice: None,
                    context_management: None,
                    query_source: Some("hook_prompt".into()),
                    agent_id: None,
                    time_since_last_assistant_ms: None,
                    cache: None,
                    agentic: false,
                    stop_sequences: None,
                    response_format: None,
                    cancel: None,
                    wire_tap: None,
                };
                match self
                    .model_runtimes
                    .query_once(source.clone(), &params)
                    .await
                {
                    ModelRuntimeQueryOutcome::Success { result, .. } => return Ok(result),
                    ModelRuntimeQueryOutcome::Retry { .. } => continue,
                    ModelRuntimeQueryOutcome::Failed { error, .. } => {
                        return Err(format!("hook prompt API error: {error}"));
                    }
                }
            }
        };
        let result = tokio::time::timeout(timeout, result).await;

        match result {
            // Timeout maps to `cancelled` — silent, no UI error.
            Err(_elapsed) => HookEvaluationResult::Cancelled,
            Ok(Err(error)) => HookEvaluationResult::NonBlockingError { error },
            Ok(Ok(query_result)) => {
                // Hook evaluation that silently `Cancelled`s on a
                // truncated / content-filtered verdict would leave the
                // user wondering why their hook didn't fire. Warn
                // before parsing so the missing decision is traceable.
                let stop = query_result.stop_reason.as_ref();
                if stop.is_some_and(coco_messages::FinishReason::is_abnormal) {
                    tracing::warn!(
                        stop_reason = ?stop,
                        tokens_out = query_result.usage.output_tokens.total,
                        "hook prompt unexpected stop_reason — \
                         decision may default to Cancelled"
                    );
                }
                parse_hook_response(&query_result.content)
            }
        }
    }

    async fn evaluate_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        timeout: Duration,
    ) -> HookEvaluationResult {
        let source = self.pick_source(model);
        let model_id = self
            .model_runtimes
            .snapshot_for_source(source.clone())
            .map(|snapshot| snapshot.model_id)
            .unwrap_or_else(|e| {
                tracing::warn!(
                    error = %e,
                    "Agent hook model source unresolved; falling back to default HookAgent model id"
                );
                self.default_model_id.clone()
            });

        let Some(runner) = self.agent_runner.read().await.clone() else {
            tracing::warn!("Agent hook evaluation has no runner installed; returning Cancelled");
            return HookEvaluationResult::Cancelled;
        };

        runner
            .run(HookAgentRunRequest {
                prompt: prompt.to_string(),
                model_source: source,
                model_id,
                timeout,
            })
            .await
    }
}

/// Build the message prompt for an LLM hook evaluation.
///
/// Two-message shape: `System` carries the JSON-output instruction;
/// `User` carries the user's hook prompt with `$ARGUMENTS` already
/// substituted upstream by `run_hook_via_handle_or_fallback`.
fn build_prompt(user_prompt: &str) -> Vec<LlmMessage> {
    vec![
        LlmMessage::System {
            content: vec![UserContentPart::text(HOOK_PROMPT_SYSTEM)],
            provider_options: None,
        },
        LlmMessage::User {
            content: vec![UserContentPart::text(user_prompt)],
            provider_options: None,
        },
    ]
}

/// Parse the assistant's text response as `{ok, reason}` JSON.
///
/// Failure modes:
/// - No text part in the response → NonBlockingError
/// - Text is not valid JSON or doesn't match `HookResponse` → NonBlockingError
/// - `ok: false` → Blocking with the supplied reason
/// - `ok: true` → Ok
fn parse_hook_response(content: &[AssistantContentPart]) -> HookEvaluationResult {
    // Multi-text-part assistant messages are now possible (streaming
    // path preserves per-part `provider_metadata`). The naive `join("")`
    // still works for hook LLM responses because hooks emit a single
    // JSON object as text; multi-text would corrupt the parse but the
    // existing test
    // (`test_parse_hook_response_concatenates_multiple_text_parts`)
    // verifies that the parser tolerates the multi-text shape and
    // returns a parse-failure outcome rather than crashing.
    let text = content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();

    if text.is_empty() {
        return HookEvaluationResult::NonBlockingError {
            error: "hook prompt returned empty assistant text".into(),
        };
    }

    let parsed = match serde_json::from_str::<HookResponse>(&text) {
        Ok(p) => p,
        Err(e) => {
            return HookEvaluationResult::NonBlockingError {
                error: format!("schema validation failed: {e} — raw response: {text}"),
            };
        }
    };

    if parsed.ok {
        HookEvaluationResult::Ok
    } else {
        HookEvaluationResult::Blocking {
            reason: parsed
                .reason
                .unwrap_or_else(|| "Prompt hook condition not met".into()),
        }
    }
}

#[cfg(test)]
#[path = "hook_llm.test.rs"]
mod tests;
