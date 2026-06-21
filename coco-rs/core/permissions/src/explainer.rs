//! Permission explainer — LLM-based risk assessment for tool actions.
//!
//! Generates human-readable explanations of what a tool action does,
//! why it's being run, and what could go wrong. Uses forced tool-use
//! for guaranteed structured output.
//!
//! **Architecture**: This module defines prompts, tool schemas, and response
//! parsing — but does NOT call the LLM directly. The caller injects an
//! async `query_fn` callback (same pattern as [`crate::classifier`]).
//! This keeps `coco-permissions` free of any `coco-inference` dependency.

use coco_messages::Message;
use coco_types::PermissionExplanation;
use coco_types::RiskLevel;
use coco_types::SideQueryRequest;
use coco_types::SideQueryResponse;
use coco_types::SideQueryToolDef;
use serde::Deserialize;
use tracing::debug;

// ── System prompt ──

const SYSTEM_PROMPT: &str = "Analyze shell commands and explain what they do, why you're running them, and potential risks.";

// ── Tool schema for forced structured output ──

/// Returns the canonical tool definition for the explainer.
///
/// Uses `SideQueryToolDef` from coco-types so the same type works
/// with both callback and SideQuery trait callers.
pub fn explainer_tool_def() -> SideQueryToolDef {
    SideQueryToolDef {
        name: "explain_command".to_string(),
        description: "Provide an explanation of a shell command".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "What this command does (1-2 sentences)"
                },
                "reasoning": {
                    "type": "string",
                    "description": "Why YOU are running this command. Start with \"I\" - e.g. \"I need to check the file contents\""
                },
                "risk": {
                    "type": "string",
                    "description": "What could go wrong, under 15 words"
                },
                "riskLevel": {
                    "type": "string",
                    "enum": ["LOW", "MEDIUM", "HIGH"],
                    "description": "LOW (safe dev workflows), MEDIUM (recoverable changes), HIGH (dangerous/irreversible)"
                }
            },
            "required": ["explanation", "reasoning", "risk", "riskLevel"]
        }),
    }
}

/// Backward-compatible alias.
pub fn explainer_tool_schema() -> SideQueryToolDef {
    explainer_tool_def()
}

// ── Query / Response types (use shared types from coco-types) ──

/// Everything the caller needs to make the LLM request.
///
/// Built from [`ExplainerParams`] via [`build_explainer_query`].
/// Can be used with either the callback pattern or `SideQuery` trait.
pub type ExplainerQuery = SideQueryRequest;

/// Raw response from the LLM — wraps `SideQueryResponse`.
pub type ExplainerResponse = SideQueryResponse;

// ── Input parameters ──

/// Parameters for generating a permission explanation.
#[derive(Debug)]
pub struct ExplainerParams<'a> {
    pub tool_name: &'a str,
    pub tool_input: &'a serde_json::Value,
    pub tool_description: Option<&'a str>,
    pub messages: Option<&'a [Message]>,
}

// ── Core logic ──

/// Build the explainer query from parameters.
///
/// This produces a `SideQueryRequest` that the caller sends to the LLM.
/// Works with both the callback pattern and `SideQuery` trait.
pub fn build_explainer_query(params: &ExplainerParams<'_>) -> ExplainerQuery {
    let formatted_input = format_tool_input(params.tool_input);
    let conversation_context = params
        .messages
        .map(|msgs| extract_conversation_context(msgs, 1000))
        .unwrap_or_default();

    let mut user_prompt = format!("Tool: {}\n", params.tool_name);
    if let Some(desc) = params.tool_description {
        user_prompt.push_str(&format!("Description: {desc}\n"));
    }
    user_prompt.push_str(&format!("Input:\n{formatted_input}\n"));
    if !conversation_context.is_empty() {
        user_prompt.push_str(&format!(
            "\nRecent conversation context:\n{conversation_context}\n"
        ));
    }
    user_prompt.push_str("\nExplain this command in context.");

    SideQueryRequest::with_forced_tool(
        SYSTEM_PROMPT,
        &user_prompt,
        explainer_tool_def(),
        "permission_explainer",
    )
}

/// Generate a permission explanation using an injected LLM query function.
///
/// The `query_fn` callback receives a [`SideQueryRequest`] and must return
/// a [`SideQueryResponse`]. The caller is responsible for the actual LLM API call.
///
/// Returns `None` on error (parse failure, LLM error, cancellation).
pub async fn generate_permission_explanation<F, Fut>(
    params: ExplainerParams<'_>,
    query_fn: F,
) -> Option<PermissionExplanation>
where
    F: FnOnce(SideQueryRequest) -> Fut,
    Fut: std::future::Future<Output = Result<SideQueryResponse, String>>,
{
    let query = build_explainer_query(&params);

    debug!(tool = %params.tool_name, "requesting permission explanation");

    let response = match query_fn(query).await {
        Ok(resp) => resp,
        Err(err) => {
            debug!(tool = %params.tool_name, "explainer error: {err}");
            return None;
        }
    };

    // Extract tool_use block input from SideQueryResponse
    let tool_json = match response.first_tool_input() {
        Some(json) => json,
        None => {
            debug!(tool = %params.tool_name, "no tool_use block in explainer response");
            return None;
        }
    };

    parse_explainer_response(tool_json, params.tool_name)
}

// ── Response parsing ──

/// Internal deserialization target matching the tool schema.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawExplanation {
    risk_level: String,
    explanation: String,
    reasoning: String,
    risk: String,
}

fn parse_risk_level(s: &str) -> Option<RiskLevel> {
    match s {
        "LOW" => Some(RiskLevel::Low),
        "MEDIUM" => Some(RiskLevel::Medium),
        "HIGH" => Some(RiskLevel::High),
        _ => None,
    }
}

fn parse_explainer_response(
    json: &serde_json::Value,
    tool_name: &str,
) -> Option<PermissionExplanation> {
    let raw: RawExplanation = match serde_json::from_value(json.clone()) {
        Ok(r) => r,
        Err(e) => {
            debug!(tool = %tool_name, "failed to parse explainer response: {e}");
            return None;
        }
    };

    let risk_level = match parse_risk_level(&raw.risk_level) {
        Some(rl) => rl,
        None => {
            debug!(tool = %tool_name, "invalid risk level: {}", raw.risk_level);
            return None;
        }
    };

    Some(PermissionExplanation {
        risk_level,
        explanation: raw.explanation,
        reasoning: raw.reasoning,
        risk: raw.risk,
    })
}

// ── Helpers ──

fn format_tool_input(input: &serde_json::Value) -> String {
    match serde_json::to_string_pretty(input) {
        Ok(s) => s,
        Err(_) => input.to_string(),
    }
}

/// Extract recent conversation context for the explainer.
///
/// Returns a summary of recent assistant messages to provide context
/// for "why" this command is being run.
fn extract_conversation_context(messages: &[Message], max_chars: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut total_chars = 0;

    // Walk backwards through messages, collect assistant text
    for msg in messages.iter().rev() {
        if let Message::Assistant(a) = msg {
            let text = extract_assistant_text(&a.message);
            if !text.is_empty() && total_chars < max_chars {
                let remaining = max_chars - total_chars;
                let truncated = if text.len() > remaining {
                    format!("{}...", &text[..remaining])
                } else {
                    text
                };
                total_chars += truncated.len();
                parts.push(truncated);
            }
            // Only look at last 3 assistant messages
            if parts.len() >= 3 {
                break;
            }
        }
    }

    parts.reverse();
    parts.join("\n\n")
}

fn extract_assistant_text(msg: &coco_messages::LlmMessage) -> String {
    match msg {
        coco_messages::LlmMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                coco_messages::AssistantContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "explainer.test.rs"]
mod tests;
