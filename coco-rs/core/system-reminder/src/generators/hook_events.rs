//! Hook-event generators (5 variants, one per `Attachment.type`).
//!
//! - `hook_success` — only SessionStart / UserPromptSubmit emit a message;
//!   empty content skips.
//! - `hook_blocking_error`.
//! - `hook_additional_context` — empty content skips; lines joined by `\n`.
//! - `hook_stopped_continuation`.
//! - `async_hook_response` — multi-message: systemMessage and/or
//!   additionalContext.
//!
//! Each generator reads `ctx.hook_events` and emits for matching
//! variants. Engine populates the vec by draining its async hook
//! registry at turn start.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::generator::HookEvent;
use crate::generator::HookEventKind;
use crate::types::AttachmentType;
use crate::types::ContentBlock;
use crate::types::MessageRole;
use crate::types::ReminderMessage;
use crate::types::ReminderOutput;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;
use coco_context::ContextualUserFragment;

const MAX_HOOK_REMINDER_BYTES: usize = 32_000;
const MAX_ASYNC_HOOK_MESSAGE_BYTES: usize = 16_000;
const HOOK_REMINDER_TRUNCATED: &str = "\n...[additional hook output truncated]...";

// ---------------------------------------------------------------------------
// HookSuccessGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct HookSuccessGenerator;

#[async_trait]
impl AttachmentGenerator for HookSuccessGenerator {
    fn name(&self) -> &str {
        "HookSuccessGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::HookSuccess
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.hook_success
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        // One message per qualifying event, joined with `\n\n` into a
        // single reminder to avoid proliferating attachments when several
        // hooks fire in one turn.
        let parts = ctx
            .hook_events
            .iter()
            .filter_map(|e| match e {
                HookEvent::Success {
                    hook_name,
                    hook_event,
                    content,
                } if matches!(
                    hook_event,
                    HookEventKind::SessionStart | HookEventKind::UserPromptSubmit
                ) && !content.is_empty() =>
                {
                    Some(format!("{hook_name} hook success: {content}"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some(content) = join_bounded_hook_parts(parts) else {
            return Ok(None);
        };
        Ok(Some(SystemReminder::new(
            AttachmentType::HookSuccess,
            content,
        )))
    }
}

// ---------------------------------------------------------------------------
// HookBlockingErrorGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct HookBlockingErrorGenerator;

#[async_trait]
impl AttachmentGenerator for HookBlockingErrorGenerator {
    fn name(&self) -> &str {
        "HookBlockingErrorGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::HookBlockingError
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.hook_blocking_error
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let parts: Vec<String> = ctx
            .hook_events
            .iter()
            .filter_map(|e| match e {
                HookEvent::BlockingError {
                    hook_name,
                    command,
                    error,
                } => Some(format!(
                    "{hook_name} hook blocking error from command: \"{command}\": {error}"
                )),
                _ => None,
            })
            .collect();
        let Some(content) = join_bounded_hook_parts(parts) else {
            return Ok(None);
        };
        Ok(Some(SystemReminder::new(
            AttachmentType::HookBlockingError,
            content,
        )))
    }
}

// ---------------------------------------------------------------------------
// HookAdditionalContextGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct HookAdditionalContextGenerator;

#[async_trait]
impl AttachmentGenerator for HookAdditionalContextGenerator {
    fn name(&self) -> &str {
        "HookAdditionalContextGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::HookAdditionalContext
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.hook_additional_context
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let parts: Vec<String> = ctx
            .hook_events
            .iter()
            .filter_map(|e| match e {
                HookEvent::AdditionalContext { hook_name, content } if !content.is_empty() => {
                    Some(format!(
                        "{hook_name} hook additional context: {}",
                        content.join("\n")
                    ))
                }
                _ => None,
            })
            .collect();
        let Some(content) = join_bounded_hook_parts(parts) else {
            return Ok(None);
        };
        Ok(Some(SystemReminder::new(
            AttachmentType::HookAdditionalContext,
            content,
        )))
    }
}

// ---------------------------------------------------------------------------
// HookStoppedContinuationGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct HookStoppedContinuationGenerator;

#[async_trait]
impl AttachmentGenerator for HookStoppedContinuationGenerator {
    fn name(&self) -> &str {
        "HookStoppedContinuationGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::HookStoppedContinuation
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.hook_stopped_continuation
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let parts: Vec<String> = ctx
            .hook_events
            .iter()
            .filter_map(|e| match e {
                HookEvent::StoppedContinuation { hook_name, message } => {
                    Some(format!("{hook_name} hook stopped continuation: {message}"))
                }
                _ => None,
            })
            .collect();
        let Some(content) = join_bounded_hook_parts(parts) else {
            return Ok(None);
        };
        Ok(Some(SystemReminder::new(
            AttachmentType::HookStoppedContinuation,
            content,
        )))
    }
}

// ---------------------------------------------------------------------------
// AsyncHookResponseGenerator
// ---------------------------------------------------------------------------

/// `async_hook_response` produces up to two separate user messages
/// inside one `<system-reminder>` wrapper. Uses
/// [`ReminderOutput::Messages`] to preserve the multi-message shape.
#[derive(Debug, Default)]
pub struct AsyncHookResponseGenerator;

#[async_trait]
impl AttachmentGenerator for AsyncHookResponseGenerator {
    fn name(&self) -> &str {
        "AsyncHookResponseGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::AsyncHookResponse
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.async_hook_response
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let mut messages: Vec<ReminderMessage> = Vec::new();
        let mut remaining = MAX_HOOK_REMINDER_BYTES;
        for e in &ctx.hook_events {
            if let HookEvent::AsyncResponse {
                system_message,
                additional_context,
            } = e
            {
                if let Some(m) = system_message.as_ref().filter(|s| !s.is_empty()) {
                    let text = render_hook_fragment(m, remaining.min(MAX_ASYNC_HOOK_MESSAGE_BYTES));
                    if text.is_empty() {
                        break;
                    }
                    remaining = remaining.saturating_sub(text.len());
                    messages.push(ReminderMessage {
                        role: MessageRole::User,
                        blocks: vec![ContentBlock::Text { text }],
                        is_meta: true,
                    });
                }
                if remaining > 0
                    && let Some(c) = additional_context.as_ref().filter(|s| !s.is_empty())
                {
                    let text = render_hook_fragment(c, remaining.min(MAX_ASYNC_HOOK_MESSAGE_BYTES));
                    if text.is_empty() {
                        break;
                    }
                    remaining = remaining.saturating_sub(text.len());
                    messages.push(ReminderMessage {
                        role: MessageRole::User,
                        blocks: vec![ContentBlock::Text { text }],
                        is_meta: true,
                    });
                }
                if remaining == 0 {
                    break;
                }
            }
        }
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder {
            attachment_type: AttachmentType::AsyncHookResponse,
            output: ReminderOutput::Messages(messages),
            is_meta: true,
            is_silent: false,
            metadata: None,
        }))
    }
}

fn join_bounded_hook_parts(parts: Vec<String>) -> Option<String> {
    let fragment_overhead = coco_context::BoundedExternalContextFragment::minimum_rendered_bytes(
        coco_context::ContextFragmentKind::Hook,
    );
    let content_budget = MAX_HOOK_REMINDER_BYTES.saturating_sub(fragment_overhead);
    let mut output = String::with_capacity(content_budget);
    for part in parts {
        let separator = if output.is_empty() { "" } else { "\n\n" };
        if output
            .len()
            .saturating_add(separator.len())
            .saturating_add(part.len())
            > content_budget
        {
            if output.is_empty() {
                output = bounded_hook_part(&part, content_budget);
                break;
            }
            append_complete_marker(&mut output, content_budget, HOOK_REMINDER_TRUNCATED);
            break;
        }
        output.push_str(separator);
        output.push_str(&part);
    }
    if output.is_empty() {
        None
    } else {
        let rendered = render_hook_fragment(&output, MAX_HOOK_REMINDER_BYTES);
        (!rendered.is_empty()).then_some(rendered)
    }
}

fn render_hook_fragment(text: &str, budget: usize) -> String {
    coco_context::BoundedExternalContextFragment::new(
        coco_context::ContextFragmentKind::Hook,
        text,
        budget,
    )
    .render()
}

fn bounded_hook_part(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    if budget <= HOOK_REMINDER_TRUNCATED.len() {
        return coco_utils_string::take_bytes_at_char_boundary(text, budget).to_string();
    }
    let mut output = String::with_capacity(budget);
    output.push_str(coco_utils_string::take_bytes_at_char_boundary(
        text,
        budget - HOOK_REMINDER_TRUNCATED.len(),
    ));
    output.push_str(HOOK_REMINDER_TRUNCATED);
    output
}

fn append_complete_marker(output: &mut String, budget: usize, marker: &str) {
    let content_budget = budget.saturating_sub(marker.len());
    output.truncate(output.floor_char_boundary(content_budget));
    output.push_str(marker);
}

#[cfg(test)]
#[path = "hook_events.test.rs"]
mod tests;
