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
        let parts: Vec<String> = ctx
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
            .collect();
        if parts.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::HookSuccess,
            parts.join("\n\n"),
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
        if parts.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::HookBlockingError,
            parts.join("\n\n"),
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
        if parts.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::HookAdditionalContext,
            parts.join("\n\n"),
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
        if parts.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::HookStoppedContinuation,
            parts.join("\n\n"),
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
        for e in &ctx.hook_events {
            if let HookEvent::AsyncResponse {
                system_message,
                additional_context,
            } = e
            {
                if let Some(m) = system_message.as_ref().filter(|s| !s.is_empty()) {
                    messages.push(ReminderMessage {
                        role: MessageRole::User,
                        blocks: vec![ContentBlock::Text { text: m.clone() }],
                        is_meta: true,
                    });
                }
                if let Some(c) = additional_context.as_ref().filter(|s| !s.is_empty()) {
                    messages.push(ReminderMessage {
                        role: MessageRole::User,
                        blocks: vec![ContentBlock::Text { text: c.clone() }],
                        is_meta: true,
                    });
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

#[cfg(test)]
#[path = "hook_events.test.rs"]
mod tests;
