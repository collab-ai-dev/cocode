//! Cross-crate `AttachmentMessage` sink for silent-event producers.
//!
//! The engine (`app/query`) owns `MessageHistory`; producer crates
//! (hooks, permissions, commands, core/tool-runtime, skills) push typed
//! `Message::Attachment` entries through an mpsc channel, drained at
//! the head of each outer-loop turn.
//!
//! Emitters are instance-scoped — the engine constructs one per session
//! and threads a clone through to each producer's context (e.g.
//! `OrchestrationContext::attachment_emitter`). Tests and standalone
//! callers use [`AttachmentEmitter::noop`].
//!
//! `emit()` is fire-and-forget: a closed channel or noop sink silently
//! drops — a missed silent event must never panic a producer. The
//! unbounded channel is intentional so a slow receiver can't back-pressure
//! a hook; producer cadence is bounded by tool execution.

use tokio::sync::mpsc::UnboundedSender;

use super::AttachmentMessage;

/// Cross-crate handle for producing silent `AttachmentMessage`s.
/// Clone is cheap (mpsc sender is `Arc`-backed).
#[derive(Debug, Clone)]
pub struct AttachmentEmitter {
    sender: Option<UnboundedSender<AttachmentMessage>>,
}

impl AttachmentEmitter {
    /// Drops every message. Used in tests and when no session sink is wired.
    pub const fn noop() -> Self {
        Self { sender: None }
    }

    /// Wrap a live sender.
    pub fn new(sender: UnboundedSender<AttachmentMessage>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// True when the emitter has a live channel.
    pub fn is_active(&self) -> bool {
        self.sender.as_ref().is_some_and(|s| !s.is_closed())
    }

    /// Fire-and-forget send. Returns whether the message was queued.
    pub fn emit(&self, msg: AttachmentMessage) -> bool {
        match &self.sender {
            Some(tx) => tx.send(msg).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
#[path = "attachment_emitter.test.rs"]
mod tests;
