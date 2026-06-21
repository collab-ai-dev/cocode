//! Shared `/btw` side-question fork logic for both the TUI runner and the
//! SDK runner. Mirrors TS `utils/sideQuestion.ts` (`runSideQuestion` +
//! `extractSideQuestionResponse`): wrap the question in a tool-less
//! "lightweight agent" system-reminder, run a one-shot fork that shares the
//! parent's prompt cache, then flatten the answer out of the per-block
//! assistant messages.

use std::sync::Arc;

use coco_query::forked_agent::{ForkDispatcherRef, ForkedAgentOptions, deny_all_handle};
use coco_types::CacheSafeParams;
use coco_types::ForkLabel;

/// Prepended (as a `<system-reminder>`) to every `/btw` question so the fork
/// model knows it is a separate, tool-less, one-off agent and does not
/// narrate being "interrupted" or promise follow-up actions. Text mirrors TS
/// `utils/sideQuestion.ts`. Wrapping the user message does not bust the parent
/// prompt cache — only the cached prefix (system prompt + parent history)
/// drives the cache key; this question is the uncached suffix.
const SIDE_QUESTION_SYSTEM_REMINDER: &str = "<system-reminder>This is a side question from the user. You must answer this question directly in a single response.

IMPORTANT CONTEXT:
- You are a separate, lightweight agent spawned to answer this one question
- The main agent is NOT interrupted - it continues working independently in the background
- You share the conversation context but are a completely separate instance
- Do NOT reference being interrupted or what you were \"previously doing\" - that framing is incorrect

CRITICAL CONSTRAINTS:
- You have NO tools available - you cannot read files, run commands, search, or take any actions
- This is a one-off response - there will be no follow-up turns
- You can ONLY provide information based on what you already know from the conversation context
- NEVER say things like \"Let me try...\", \"I'll now...\", \"Let me check...\", or promise to take any action
- If you don't know the answer, say so - do not offer to look it up or investigate

Simply answer the question with the information you have.</system-reminder>";

/// Run the side question as a one-shot, tool-less fork that shares the
/// parent's prompt cache, and return the answer text (or a degraded
/// explanation on tool-attempt / API error / empty response). The `cache`
/// and `dispatcher` come from the caller's surface — the TUI runtime's
/// `last_cache_safe_params` or the SDK's persistent engine. Mirrors TS
/// `runSideQuestion`; the parent conversation is never mutated.
pub async fn run_side_question_fork(
    cache: &CacheSafeParams,
    dispatcher: &ForkDispatcherRef,
    question: &str,
) -> String {
    let wrapped = format!("{SIDE_QUESTION_SYSTEM_REMINDER}\n\n{question}");
    let mut options = ForkedAgentOptions::for_label(ForkLabel::SideQuestion);
    options.can_use_tool = Some(deny_all_handle("side question: tools disabled"));
    match dispatcher.dispatch(cache, &options, &wrapped, None).await {
        Ok(result) => extract_side_question_answer(&result.messages),
        Err(e) => format!("(side-question failed: {e})"),
    }
}

/// Flatten the answer out of the fork's result messages. Mirrors TS
/// `extractSideQuestionResponse`: the provider yields one assistant message
/// per content block (a thinking block then a text block), so concatenate
/// the text of EVERY assistant message — not just the last. Falls back to a
/// tool-attempt note (model ignored the no-tools instruction), then an
/// api-error note, then a generic "no response".
pub fn extract_side_question_answer(messages: &[Arc<coco_messages::Message>]) -> String {
    let text = messages
        .iter()
        .filter_map(|m| match m.as_ref() {
            coco_messages::Message::Assistant(a) => Some(&a.message),
            _ => None,
        })
        .map(coco_messages::wrapping::extract_text_from_llm_message)
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let text = text.trim();
    if !text.is_empty() {
        return text.to_string();
    }
    if let Some(tool) = first_attempted_tool_name(messages) {
        return format!(
            "(The model tried to call {tool} instead of answering directly. \
             Try rephrasing or ask in the main conversation.)"
        );
    }
    if let Some(err) = first_api_error(messages) {
        return format!("(API error: {err})");
    }
    "(No response received.)".to_string()
}

/// First tool name the fork's assistant messages tried to call, if any —
/// used to explain why a tool-less side question produced no text.
fn first_attempted_tool_name(messages: &[Arc<coco_messages::Message>]) -> Option<String> {
    messages.iter().find_map(|m| {
        let coco_messages::Message::Assistant(a) = m.as_ref() else {
            return None;
        };
        let coco_llm_types::LlmMessage::Assistant { content, .. } = &a.message else {
            return None;
        };
        content.iter().find_map(|part| match part {
            coco_llm_types::AssistantContentPart::ToolCall(tc) => Some(tc.tool_name.clone()),
            _ => None,
        })
    })
}

/// First API-error text surfaced by the fork (retries exhausted), if any.
fn first_api_error(messages: &[Arc<coco_messages::Message>]) -> Option<String> {
    messages.iter().find_map(|m| match m.as_ref() {
        coco_messages::Message::System(coco_messages::SystemMessage::ApiError(e)) => {
            Some(e.error.clone())
        }
        _ => None,
    })
}

#[cfg(test)]
#[path = "side_question.test.rs"]
mod tests;
