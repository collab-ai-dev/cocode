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
//!   `StructuredOutputTool`, and `requires_structured_output` enabled so
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
//! model id or an alias. The runtime routes through `ModelRole::HookAgent`
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
use coco_hooks::HookLlmEvaluationContext;
use coco_hooks::HookLlmHandle;
use coco_inference::InferenceError;
use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSnapshot;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_inference::ResponseFormat;
use coco_inference::RetryConfig;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::UserContentPart;
use coco_types::ModelRole;
use coco_types::TokenUsage;
use serde_json::Value;
use serde_json::json;

const STOP_HOOK_TRANSCRIPT_MAX_BYTES: usize = 64 * 1024;
const STOP_HOOK_TRANSCRIPT_RETRY_BYTES: usize = 32 * 1024;

/// System prompt prepended to every Prompt hook evaluation.
const HOOK_PROMPT_SYSTEM: &str = "You are evaluating a hook in Coco.

Your response must be a JSON object matching one of the following schemas:
1. If the condition is met, return: {\"ok\": true}
2. If the condition is not met, return: {\"ok\": false, \"reason\": \"Reason for why it is not met\"}";

const STOP_HOOK_PROMPT_SYSTEM: &str = r#"You are evaluating a stop-condition hook in Coco. Read the conversation transcript carefully, then judge whether the user-provided condition is satisfied.

Your response must be a JSON object with one of these shapes:
- {"ok": true, "reason": "<quote evidence from the transcript that satisfies the condition>"}
- {"ok": false, "reason": "<quote what is missing or what blocks the condition>"}
- {"ok": false, "impossible": true, "reason": "<explain why the condition can never be satisfied>"}

Always include a "reason" field, quoting specific text from the transcript whenever possible. If the transcript does not contain clear evidence that the condition is satisfied, return {"ok": false, "reason": "insufficient evidence in transcript"}.

Only use {"ok": false, "impossible": true} when the condition is genuinely unachievable within this session: for example it is self-contradictory, depends on an unavailable resource or capability, or the assistant already tried, exhausted reasonable options, and stated it cannot be done. An assistant claim is evidence, not proof; independently confirm from the transcript when possible. Do not use impossible merely because the goal has not been reached yet, may take more work, or is slow. When in doubt, return {"ok": false} without "impossible"."#;

/// `coco-query`'s `HookLlmHandle` implementation. Single struct for
/// both Prompt and Agent paths — they share `model_runtimes` and the
/// `Cancelled`/`NonBlockingError` mapping logic.
/// Manual `Debug` surfaces only the default model id.
pub struct QueryHookLlm {
    model_runtimes: Arc<ModelRuntimeRegistry>,
    default_model_id: String,
    agent_runner: Arc<tokio::sync::RwLock<Option<HookAgentRunnerRef>>>,
    usage_recorder: Option<Arc<dyn HookUsageRecorder>>,
    usage_accounting: Option<crate::usage_accounting::UsageAccounting>,
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
            agent_runner: Arc::new(tokio::sync::RwLock::new(None)),
            usage_recorder: None,
            usage_accounting: None,
        }
    }

    pub fn with_usage_recorder(mut self, recorder: Arc<dyn HookUsageRecorder>) -> Self {
        self.usage_recorder = Some(recorder);
        self
    }

    pub fn scoped_with_usage_accounting(
        &self,
        accounting: crate::usage_accounting::UsageAccounting,
    ) -> Self {
        Self {
            model_runtimes: self.model_runtimes.clone(),
            default_model_id: self.default_model_id.clone(),
            agent_runner: self.agent_runner.clone(),
            usage_recorder: None,
            usage_accounting: Some(accounting),
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

#[async_trait]
pub trait HookUsageRecorder: Send + Sync + std::fmt::Debug {
    async fn record_hook_usage(
        &self,
        snapshot: &ModelRuntimeSnapshot,
        usage: TokenUsage,
        duration_ms: i64,
    );
}

/// Request passed from [`QueryHookLlm`] to the runtime-specific Agent
/// hook runner.
#[derive(Debug, Clone)]
pub struct HookAgentRunRequest {
    pub prompt: String,
    pub model_source: ModelRuntimeSource,
    pub model_id: String,
    pub timeout: Duration,
    pub usage_accounting: Option<crate::usage_accounting::UsageAccounting>,
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
        context: HookLlmEvaluationContext,
    ) -> HookEvaluationResult {
        let source = self.pick_source(model);
        let is_stop_event = is_stop_event(context.event);
        let user_prompt = prompt.to_string();

        let result = async {
            let event_tx = None;
            let moa_turn_id = coco_types::TurnId::generate();
            let mut prompt_too_long_retried = false;
            loop {
                let prompt = build_prompt(
                    &user_prompt,
                    &context,
                    if prompt_too_long_retried {
                        STOP_HOOK_TRANSCRIPT_RETRY_BYTES
                    } else {
                        STOP_HOOK_TRANSCRIPT_MAX_BYTES
                    },
                );
                let params = QueryParams {
                    prompt: prompt.clone(),
                    temperature: None,
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
                    response_format: Some(hook_response_format()),
                    fallback_min_context_window: None,
                    cancel: None,
                    wire_tap: None,
                };
                let usage_recorder = self
                    .usage_accounting
                    .as_ref()
                    .map(crate::moa::MoaReferenceUsageRecorder::Accounting)
                    .unwrap_or(crate::moa::MoaReferenceUsageRecorder::None);
                let params = crate::moa::maybe_attach_moa_guidance_for_query_once(
                    &self.model_runtimes,
                    &source,
                    &params,
                    &event_tx,
                    &moa_turn_id,
                    usage_recorder,
                )
                .await;
                let started = std::time::Instant::now();
                match self
                    .model_runtimes
                    .query_once(source.clone(), &params)
                    .await
                {
                    ModelRuntimeQueryOutcome::Success {
                        result, snapshot, ..
                    } => {
                        if let Some(accounting) = &self.usage_accounting {
                            accounting
                                .record_snapshot_usage(
                                    &snapshot,
                                    result.usage,
                                    started.elapsed().as_millis() as i64,
                                    coco_types::UsageSource::HookPrompt,
                                )
                                .await;
                        } else if let Some(recorder) = &self.usage_recorder {
                            recorder
                                .record_hook_usage(
                                    &snapshot,
                                    result.usage,
                                    started.elapsed().as_millis() as i64,
                                )
                                .await;
                        }
                        return Ok(result);
                    }
                    ModelRuntimeQueryOutcome::Retry { .. } => continue,
                    ModelRuntimeQueryOutcome::Failed { error, .. } => {
                        if is_stop_event && !prompt_too_long_retried && is_prompt_too_long(&error) {
                            prompt_too_long_retried = true;
                            continue;
                        }
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
                parse_hook_response(&query_result.content, context.event)
            }
        }
    }

    async fn evaluate_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        timeout: Duration,
        _context: HookLlmEvaluationContext,
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
                usage_accounting: self.usage_accounting.clone(),
            })
            .await
    }
}

/// Build the message prompt for an LLM hook evaluation.
///
/// Two-message shape: `System` carries the JSON-output instruction;
/// `User` carries the user's hook prompt with `$ARGUMENTS` already
/// substituted upstream by `run_hook_via_handle_or_fallback`.
fn build_prompt(
    user_prompt: &str,
    context: &HookLlmEvaluationContext,
    stop_transcript_max_bytes: usize,
) -> Vec<LlmMessage> {
    if is_stop_event(context.event) {
        let transcript = if context.transcript_history.is_empty() {
            "(no transcript evidence available)".to_string()
        } else {
            bounded_transcript_history(&context.transcript_history, stop_transcript_max_bytes)
        };
        let stop_prompt = format!(
            "Conversation transcript:\n{transcript}\n\nBased on the conversation transcript above, has the following stopping condition been satisfied? Answer based on transcript evidence only.\n\nCondition: {user_prompt}",
        );
        return vec![
            LlmMessage::System {
                content: vec![UserContentPart::text(STOP_HOOK_PROMPT_SYSTEM)],
                provider_options: None,
            },
            LlmMessage::User {
                content: vec![UserContentPart::text(stop_prompt)],
                provider_options: None,
            },
        ];
    }
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

fn is_stop_event(event: coco_types::HookEventType) -> bool {
    matches!(
        event,
        coco_types::HookEventType::Stop | coco_types::HookEventType::SubagentStop
    )
}

fn bounded_transcript_history(history: &[String], max_bytes: usize) -> String {
    if history.is_empty() {
        return "(no transcript evidence available)".to_string();
    }
    let mut kept_rev: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;
    for idx in (0..history.len()).rev() {
        let entry = &history[idx];
        let separator = usize::from(!kept_rev.is_empty());
        if used + separator + entry.len() <= max_bytes {
            kept_rev.push(entry.clone());
            used += separator + entry.len();
            continue;
        }
        let remaining = max_bytes.saturating_sub(used + separator);
        let suffix = coco_utils_string::take_last_bytes_at_char_boundary(entry, remaining);
        if !suffix.is_empty() {
            kept_rev.push(suffix.to_string());
        }
        omitted = idx;
        break;
    }
    kept_rev.reverse();
    let mut transcript = kept_rev.join("\n");
    if omitted > 0 {
        transcript = format!(
            "({omitted} older transcript entries omitted due to hook context limit)\n{transcript}"
        );
    }
    transcript
}

fn hook_response_format() -> ResponseFormat {
    ResponseFormat::json_with_schema(json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "reason": { "type": "string" },
            "impossible": { "type": "boolean" }
        },
        "required": ["ok", "reason"],
        "additionalProperties": false
    }))
    .with_name("hook_verdict")
}

fn is_prompt_too_long(error: &InferenceError) -> bool {
    matches!(error, InferenceError::ContextWindowExceeded { .. })
        || RetryConfig::is_prompt_too_long(error)
}

/// Parse the assistant's text response as `{ok, reason}` JSON.
///
/// Failure modes:
/// - No text part in the response → NonBlockingError
/// - Text is not valid JSON or doesn't match `HookResponse` → NonBlockingError
/// - `ok: false` → Blocking with the supplied reason
/// - `ok: true` → Ok
fn parse_hook_response(
    content: &[AssistantContentPart],
    event: coco_types::HookEventType,
) -> HookEvaluationResult {
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

    let parsed = match serde_json::from_str::<Value>(&text) {
        Ok(p) => p,
        Err(e) => {
            return HookEvaluationResult::NonBlockingError {
                error: format!("schema validation failed: {e} — raw response: {text}"),
            };
        }
    };

    parse_hook_response_value(parsed, event, &text)
}

fn parse_hook_response_value(
    value: Value,
    event: coco_types::HookEventType,
    raw: &str,
) -> HookEvaluationResult {
    let Some(obj) = value.as_object() else {
        return schema_error("expected JSON object", raw);
    };
    let is_stop = matches!(
        event,
        coco_types::HookEventType::Stop | coco_types::HookEventType::SubagentStop
    );
    let allowed: &[&str] = if is_stop {
        &["ok", "reason", "impossible"]
    } else {
        &["ok", "reason"]
    };
    if let Some(field) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return schema_error(format!("unknown field `{field}`"), raw);
    }
    if !is_stop && obj.contains_key("impossible") {
        return schema_error(
            "`impossible` is only valid for Stop/SubagentStop hooks",
            raw,
        );
    }
    let Some(ok) = obj.get("ok").and_then(Value::as_bool) else {
        return schema_error("field `ok` must be a boolean", raw);
    };
    let reason = obj
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    if is_stop {
        let Some(reason) = reason.filter(|r| !r.trim().is_empty()) else {
            return schema_error("field `reason` is required", raw);
        };
        if obj
            .get("impossible")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return HookEvaluationResult::Impossible { reason };
        }
        if ok {
            HookEvaluationResult::Success {
                reason: Some(reason),
            }
        } else {
            HookEvaluationResult::Blocking { reason }
        }
    } else if ok {
        HookEvaluationResult::Success { reason: None }
    } else {
        let Some(reason) = reason.filter(|r| !r.trim().is_empty()) else {
            return schema_error("field `reason` is required when `ok` is false", raw);
        };
        HookEvaluationResult::Blocking { reason }
    }
}

fn schema_error(message: impl Into<String>, raw: &str) -> HookEvaluationResult {
    HookEvaluationResult::NonBlockingError {
        error: format!(
            "schema validation failed: {} — raw response: {raw}",
            message.into()
        ),
    }
}

#[cfg(test)]
#[path = "hook_llm.test.rs"]
mod tests;
