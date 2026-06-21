//! `generateToolUseSummary` side-fork (`ModelRole::Fast`).
//!
//! Spawns a fire-and-forget call against `ModelRole::Fast` after each
//! tool batch to produce a ≤30-char "git-commit-subject"-style label.
//! The label drives the mobile-app one-line progress row that summarizes
//! what the tool batch accomplished.
//!
//! # Multi-LLM mapping
//!
//! `ModelRole::Fast` is resolved via the shared `ModelRuntimeRegistry`.
//! For Anthropic users this maps to Haiku; for OpenAI to `gpt-4o-mini`
//! (or whatever the user configured); for Google to Gemini Flash.
//! **Never hardcode `"Haiku"` or any provider-specific model id here** —
//! the resolver is the only entry point.
//!
//! # Never-throws contract
//!
//! Tool-use summary is non-critical UX polish. Every error path
//! returns `None` and logs at `tracing::debug` — the parent turn
//! must never observe a failure.
//!
//! # Gating
//!
//! The caller must enforce:
//! 1. `Feature::ToolUseSummary` enabled
//! 2. `agent_id.is_none()` — subagents don't surface in the mobile UI,
//!    so the Fast-tier call would burn tokens for nothing
//! 3. tool batch non-empty
//!
//! If any gate fails, do not call `generate_tool_use_summary`.
//!
//! # Max-tokens policy
//!
//! This side-fork does **not** set `QueryParams.max_tokens`. The TS
//! port hard-coded `64` to match Haiku's behavior, but on reasoning
//! Fast models (DeepSeek V4, Gemini Flash Thinking, …) a 64-token cap
//! is exhausted by reasoning before any visible text is emitted —
//! the model returns `stop_reason=length` with empty text and the
//! tokens are wasted. Falling back to the model-level `max_output_tokens`
//! from `ModelInfo` lets reasoning models budget their own thinking,
//! and lets `coco-config` cap costs per-role.

use std::time::Duration;

use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::UserContentPart;
use coco_types::ModelRole;
use coco_types::ToolUseSummaryParams;

/// System prompt for the tool-use-summary side-fork.
const TOOL_USE_SUMMARY_SYSTEM_PROMPT: &str = "Write a short summary label describing what these tool calls accomplished. It appears as a single-line row in a mobile app and truncates around 30 characters, so think git-commit-subject, not sentence.

Keep the verb in past tense and the most distinctive noun. Drop articles, connectors, and long location context first.

Examples:
- Searched in auth/
- Fixed NPE in UserService
- Created signup endpoint
- Read config.json
- Ran failing tests";

/// `lastAssistantText` is truncated to 200 chars before being included
/// in the user prompt.
const LAST_ASSISTANT_TEXT_MAX: usize = 200;

/// Per-field truncation cap for serialized tool input/output.
const TOOL_FIELD_TRUNCATE: usize = 300;

/// 10 second hard cap on the side-fork. The summary is non-critical,
/// so a slow Fast-tier model gets cancelled rather than dragging the
/// next iteration. Uses a concrete timeout to avoid orphaning tasks
/// when the parent cancels.
const TOOL_USE_SUMMARY_TIMEOUT: Duration = Duration::from_secs(10);

/// One tool's worth of summary input.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    /// Tool name as it appears in the model's tool_use block.
    pub name: String,
    /// Tool input — serialized to JSON for the prompt.
    pub input: serde_json::Value,
    /// Tool output — serialized to JSON for the prompt. `Null` when
    /// the tool errored or returned no content.
    pub output: serde_json::Value,
}

/// Inputs to a single tool-use-summary generation.
#[derive(Debug, Clone)]
pub struct ToolUseSummaryInput {
    /// One entry per tool in the batch, in execution order.
    pub tools: Vec<ToolInfo>,
    /// `preceding_tool_use_ids` — the tool_use_id strings from the
    /// model's last assistant turn. Flows through unchanged to
    /// `ToolUseSummaryParams.preceding_tool_use_ids` so SDK consumers
    /// can correlate the summary with its batch.
    pub preceding_tool_use_ids: Vec<String>,
    /// Last text-typed block from the most recent assistant message.
    /// Provides "user intent" context to the summarizer. Truncated to
    /// [`LAST_ASSISTANT_TEXT_MAX`] before being injected into the user
    /// prompt.
    pub last_assistant_text: Option<String>,
}

impl ToolUseSummaryInput {
    /// True iff there is anything for the summarizer to do.
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }
}

/// Build a [`ToolUseSummaryInput`] from a [`MessageHistory`] snapshot.
///
/// Walks the history in reverse to find the most recent assistant
/// message. Extracts that message's text parts (for
/// `last_assistant_text`) and tool-call parts (for `tools` +
/// `preceding_tool_use_ids`). Then walks forward from the assistant to
/// gather matching tool-result content (output).
///
/// Returns `None` if no assistant message exists or it has no tool
/// calls — in either case there's nothing to summarize.
pub fn build_input_from_history<M: std::borrow::Borrow<coco_messages::Message>>(
    messages: &[M],
) -> Option<ToolUseSummaryInput> {
    use coco_llm_types::LlmMessage;
    use coco_llm_types::ToolContentPart;
    use coco_messages::Message;
    use std::collections::HashMap;

    let last_assistant_idx = messages
        .iter()
        .rposition(|m| matches!(m.borrow(), Message::Assistant(_)))?;
    let last_assistant = match messages[last_assistant_idx].borrow() {
        Message::Assistant(a) => a,
        _ => return None,
    };

    let assistant_content = match &last_assistant.message {
        LlmMessage::Assistant { content, .. } => content,
        _ => return None,
    };

    let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
    let mut last_text: Option<String> = None;
    for part in assistant_content {
        match part {
            AssistantContentPart::Text(t) => {
                // Loop overwrites so the final assignment wins —
                // the last text block is the "user intent" snippet.
                last_text = Some(t.text.clone());
            }
            AssistantContentPart::ToolCall(tc) => {
                tool_calls.push((
                    tc.tool_call_id.clone(),
                    tc.tool_name.clone(),
                    tc.input.clone(),
                ));
            }
            _ => {}
        }
    }

    if tool_calls.is_empty() {
        return None;
    }

    let mut outputs: HashMap<String, serde_json::Value> = HashMap::new();
    for msg in &messages[last_assistant_idx + 1..] {
        let Message::ToolResult(tr) = msg.borrow() else {
            continue;
        };
        let LlmMessage::Tool { content, .. } = &tr.message else {
            continue;
        };
        let output = content
            .iter()
            .find_map(|p| match p {
                ToolContentPart::ToolResult(rp) if rp.tool_call_id == tr.tool_use_id => {
                    Some(serde_json::to_value(&rp.output).unwrap_or(serde_json::Value::Null))
                }
                _ => None,
            })
            .unwrap_or(serde_json::Value::Null);
        outputs.insert(tr.tool_use_id.clone(), output);
    }

    let tools: Vec<ToolInfo> = tool_calls
        .iter()
        .map(|(id, name, input)| ToolInfo {
            name: name.clone(),
            input: input.clone(),
            output: outputs.get(id).cloned().unwrap_or(serde_json::Value::Null),
        })
        .collect();

    let preceding_tool_use_ids: Vec<String> = tool_calls.into_iter().map(|(id, _, _)| id).collect();

    Some(ToolUseSummaryInput {
        tools,
        preceding_tool_use_ids,
        last_assistant_text: last_text,
    })
}

/// Generate a single tool-use-summary message via `ModelRole::Fast`.
///
/// Resolves the Fast-role runtime through the shared
/// [`ModelRuntimeRegistry`]. When `Fast` is unconfigured the resolver
/// returns `Err`; we log at `tracing::debug` and return `None`. **Do
/// not fall back to `Main`** — silently spending Main-tier tokens on
/// non-critical UX polish would surprise users.
///
/// Never throws. Every failure path (timeout, transport error, empty
/// model response, JSON parse error) collapses to `Ok(None)` so the
/// parent turn never observes the side-fork.
pub async fn generate_tool_use_summary(
    input: ToolUseSummaryInput,
    model_runtimes: std::sync::Arc<ModelRuntimeRegistry>,
) -> Option<ToolUseSummaryParams> {
    if !input.has_tools() {
        return None;
    }

    let source = ModelRuntimeSource::Role(ModelRole::Fast);
    if let Err(e) = model_runtimes.snapshot_for_source(source.clone()) {
        tracing::debug!(
            error = %e,
            "ModelRole::Fast runtime unresolved; skipping tool_use_summary generation"
        );
        return None;
    }

    let prompt = build_prompt(&input);

    let result = tokio::time::timeout(TOOL_USE_SUMMARY_TIMEOUT, async {
        loop {
            let params = QueryParams {
                prompt: prompt.clone(),
                // Intentionally `None`: defer to the Fast model's own
                // `max_output_tokens` (resolved from `ModelInfo` via
                // `coco-config`). See module docs "Max-tokens policy".
                max_tokens: None,
                thinking_level: None,
                fast_mode: false,
                tools: None,
                tool_choice: None,
                context_management: None,
                query_source: Some("tool_use_summary_generation".into()),
                agent_id: None,
                time_since_last_assistant_ms: None,
                cache: None,
                agentic: false,
                stop_sequences: None,
                response_format: None,
                cancel: None,
                wire_tap: None,
            };
            match model_runtimes.query_once(source.clone(), &params).await {
                ModelRuntimeQueryOutcome::Success { result, .. } => return Ok(result),
                ModelRuntimeQueryOutcome::Retry { .. } => continue,
                ModelRuntimeQueryOutcome::Failed { error, .. } => return Err(error),
            }
        }
    })
    .await;

    let query_result = match result {
        Err(_elapsed) => {
            tracing::debug!(
                timeout_ms = TOOL_USE_SUMMARY_TIMEOUT.as_millis() as u64,
                "tool_use_summary generation timed out"
            );
            return None;
        }
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "tool_use_summary generation API error");
            return None;
        }
        Ok(Ok(r)) => r,
    };

    let summary = extract_assistant_text(&query_result.content);
    let stop = query_result.stop_reason.as_ref();
    let stop_abnormal = stop.is_some_and(coco_messages::FinishReason::is_abnormal);
    let summary_empty = summary.is_empty();

    // Any not-as-expected outcome surfaces as a single `warn` so the
    // failure mode is obvious from the log without diffing two
    // separate lines from the inference layer. Expected =
    // non-empty text AND a normal stop_reason. Common abnormal cause
    // on reasoning Fast models: `stop_reason=length` with empty text
    // because the per-call token budget was consumed by reasoning.
    if summary_empty || stop_abnormal {
        tracing::warn!(
            stop_reason = ?stop,
            tokens_out = query_result.usage.output_tokens.total,
            summary_chars = summary.len(),
            empty = summary_empty,
            "tool_use_summary unexpected outcome; check Fast role \
             (reasoning models exhaust the per-call budget before emitting text)"
        );
    }

    if summary_empty {
        return None;
    }

    Some(ToolUseSummaryParams {
        summary,
        preceding_tool_use_ids: input.preceding_tool_use_ids,
    })
}

/// Build the System + User prompt pair sent to `ModelRole::Fast`.
fn build_prompt(input: &ToolUseSummaryInput) -> Vec<LlmMessage> {
    let mut user = String::new();

    if let Some(text) = input.last_assistant_text.as_deref() {
        let prefix = if text.chars().count() > LAST_ASSISTANT_TEXT_MAX {
            // Char-boundary safe truncation; chars() won't panic on
            // multi-byte UTF-8.
            text.chars()
                .take(LAST_ASSISTANT_TEXT_MAX)
                .collect::<String>()
        } else {
            text.to_string()
        };
        user.push_str("User's intent (from assistant's last message): ");
        user.push_str(&prefix);
        user.push_str("\n\n");
    }

    user.push_str("Tools completed:\n\n");
    for (i, tool) in input.tools.iter().enumerate() {
        if i > 0 {
            user.push_str("\n\n");
        }
        user.push_str("Tool: ");
        user.push_str(&tool.name);
        user.push_str("\nInput: ");
        user.push_str(&truncate_json(&tool.input, TOOL_FIELD_TRUNCATE));
        user.push_str("\nOutput: ");
        user.push_str(&truncate_json(&tool.output, TOOL_FIELD_TRUNCATE));
    }
    user.push_str("\n\nLabel:");

    vec![
        LlmMessage::System {
            content: vec![UserContentPart::text(TOOL_USE_SUMMARY_SYSTEM_PROMPT)],
            provider_options: None,
        },
        LlmMessage::User {
            content: vec![UserContentPart::text(&user)],
            provider_options: None,
        },
    ]
}

/// Truncate a `serde_json::Value`'s string representation to
/// `max_len` chars — caller-side guard so the model never sees
/// pathologically large tool outputs.
fn truncate_json(value: &serde_json::Value, max_len: usize) -> String {
    let s = match serde_json::to_string(value) {
        Ok(s) => s,
        Err(_) => return "[unable to serialize]".to_string(),
    };
    if s.chars().count() <= max_len {
        s
    } else {
        let mut truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        truncated.push_str("...");
        truncated
    }
}

/// Concatenate every text part of an assistant response into a single
/// string. Multi-text responses are rare but the streaming path can
/// emit several text parts when `provider_metadata` differs across
/// chunks. We `trim()` the result so trailing whitespace doesn't
/// reach the UI label.
fn extract_assistant_text(content: &[AssistantContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string()
}

#[cfg(test)]
#[path = "tool_use_summary.test.rs"]
mod tests;
