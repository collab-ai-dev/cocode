//! Mid-turn command queue for steering.
//!
//! Enables mid-turn message injection: human-typed prompts, teammate
//! messages, task notifications, and channel pub-sub events are queued
//! and drained at end-of-turn into the conversation history. The queue
//! is hoisted onto `SessionRuntime` and injected into every per-turn
//! `QueryEngine` via `with_command_queue`.
//!
//! The `QueryGuard` 3-state FSM is intentionally not ported: the
//! dispatch loop in `tui_runner` serialises turns via
//! `drain_active_turn`, and the TUI gates `QueueInput` vs `SubmitInput`
//! on the local `is_streaming()` flag — there's no concurrent
//! queue-processor coroutine to guard against.

use coco_system_reminder::QueueOrigin;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use uuid::Uuid;

/// Priority for queued commands.
/// Higher priority (lower numeric value) commands are dequeued first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueuePriority {
    /// Highest: urgent user input during streaming.
    Now = 0,
    /// Normal: standard queued commands.
    Next = 1,
    /// Lowest: background task notifications.
    Later = 2,
}

/// A command queued for mid-turn injection.
///
/// The `origin` field carries the typed [`QueueOrigin`] tag so the
/// `queued_command` system-reminder can render
/// the correct framing per producer (coordinator / task-notification /
/// channel / human). `None` is rendered as human input by
/// [`coco_system_reminder::wrap_command_text`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedCommand {
    /// Stable identifier minted at construction. Threads through
    /// `CommandQueued`/`CommandDequeued` events so TUI / SDK clients
    /// can pair the lifecycle observations of one queue entry.
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    /// The prompt text or slash command.
    pub prompt: String,
    /// Priority level.
    pub priority: QueuePriority,
    /// Agent ID filter (None = main thread).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Whether this is a slash command (starts with /).
    pub is_slash_command: bool,
    /// Origin tag, used by the `queued_command` reminder to pick the
    /// correct framing prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<QueueOrigin>,
    /// Image attachments paired with the queued text (mid-turn screenshot
    /// pastes). Carries image blocks alongside the text prompt.
    /// Empty for text-only
    /// queue items, which is the common case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<QueuedImage>,
}

/// One image attached to a queued command — wire-shape mirrors
/// `coco_context::ImageAttachment` (media_type + base64) without dragging
/// the heavier attachment crate into the queue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedImage {
    /// IANA media type (e.g. `image/png`).
    pub media_type: String,
    /// Base64-encoded image payload.
    pub data_base64: String,
}

impl QueuedCommand {
    pub fn new(prompt: String, priority: QueuePriority) -> Self {
        let is_slash = prompt.trim_start().starts_with('/');
        Self {
            id: Uuid::new_v4(),
            prompt,
            priority,
            agent_id: None,
            is_slash_command: is_slash,
            origin: None,
            images: Vec::new(),
        }
    }

    /// Short preview of the prompt for `CommandQueued.preview`. Caps at
    /// 80 characters and trims to a char boundary so multibyte input
    /// (CJK, emoji) doesn't slice in the middle of a code point.
    pub fn preview(&self) -> String {
        let mut out = String::with_capacity(80);
        for c in self.prompt.chars().take(80) {
            out.push(c);
        }
        out
    }

    pub fn with_agent(mut self, agent_id: String) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    /// Tag the queued item with an origin variant. Determines the
    /// framing prose the model sees in the reminder.
    pub fn with_origin(mut self, origin: QueueOrigin) -> Self {
        self.origin = Some(origin);
        self
    }

    /// Attach images to the queued command.
    pub fn with_images(mut self, images: Vec<QueuedImage>) -> Self {
        self.images = images;
        self
    }

    /// Whether the queued command may be restored into the user's composer.
    pub fn is_editable_by_user(&self) -> bool {
        !self.is_slash_command
            && self.agent_id.is_none()
            && matches!(self.origin.as_ref(), None | Some(QueueOrigin::Human))
    }
}

/// Thread-safe mid-turn command queue with priority ordering.
///
/// Commands enqueued during tool execution are drained between turns.
/// Slash commands are excluded from mid-turn draining (processed post-turn).
#[derive(Debug, Clone)]
pub struct CommandQueue {
    inner: Arc<Mutex<Vec<QueuedCommand>>>,
    changed: Arc<Notify>,
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            changed: Arc::new(Notify::new()),
        }
    }

    /// Enqueue a command, maintaining priority order with FIFO within same priority.
    ///
    /// Uses `partition_point` binary search to find the insertion slot in O(log n)
    /// comparisons + O(n) shift, rather than re-sorting the full Vec on every push.
    pub async fn enqueue(&self, command: QueuedCommand) {
        let mut queue = self.inner.lock().await;
        let pos = queue.partition_point(|c| c.priority <= command.priority);
        queue.insert(pos, command);
        self.changed.notify_waiters();
    }

    /// Dequeue highest-priority non-slash command matching the agent filter.
    pub async fn dequeue(&self, agent_id: Option<&str>) -> Option<QueuedCommand> {
        let mut queue = self.inner.lock().await;
        let idx = queue
            .iter()
            .position(|c| !c.is_slash_command && c.agent_id.as_deref() == agent_id)?;
        Some(queue.remove(idx))
    }

    /// Get all commands at or above (lower numeric value) a max priority.
    /// Used for mid-turn draining.
    pub async fn get_commands_by_max_priority(
        &self,
        max_priority: QueuePriority,
        agent_id: Option<&str>,
    ) -> Vec<QueuedCommand> {
        let queue = self.inner.lock().await;
        queue
            .iter()
            .filter(|c| {
                c.priority <= max_priority
                    && !c.is_slash_command
                    && c.agent_id.as_deref() == agent_id
            })
            .cloned()
            .collect()
    }

    /// Remove specific commands from the queue by their stable
    /// [`QueuedCommand::id`].
    ///
    /// `prompt`-based matching used to live here but couldn't tell two
    /// queue entries with identical prompt text apart — the second
    /// would be wrongly removed when the first was drained. The
    /// [`Uuid`] minted at construction is the canonical identity.
    pub async fn remove_by_ids(&self, ids_to_remove: &[Uuid]) {
        let mut queue = self.inner.lock().await;
        queue.retain(|c| !ids_to_remove.contains(&c.id));
    }

    /// Remove one queued command by id and return it to the caller.
    pub async fn remove_by_id(&self, id: Uuid) -> Option<QueuedCommand> {
        let mut queue = self.inner.lock().await;
        let idx = queue.iter().position(|c| c.id == id)?;
        let removed = queue.remove(idx);
        self.changed.notify_waiters();
        Some(removed)
    }

    /// Remove every user-editable command, leaving task/coordinator/channel
    /// notifications queued for the engine.
    pub async fn dequeue_all_editable(&self) -> Vec<QueuedCommand> {
        self.dequeue_matching(QueuedCommand::is_editable_by_user)
            .await
    }

    /// Check if the queue is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }

    /// Wait for a change to the queue.
    pub async fn wait_for_change(&self) {
        self.changed.notified().await;
    }

    /// Number of queued commands.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Peek at the highest-priority command without removing it.
    pub async fn peek(&self, agent_id: Option<&str>) -> Option<QueuedCommand> {
        let queue = self.inner.lock().await;
        queue
            .iter()
            .find(|c| !c.is_slash_command && c.agent_id.as_deref() == agent_id)
            .cloned()
    }

    /// Snapshot queued commands for the `queued_command` system-reminder.
    ///
    /// TS `getQueuedCommandAttachments` (`attachments.ts:829`) surfaces
    /// drained queue items so the model sees mid-turn injections as
    /// part of its input. The reminder generator wraps each entry via
    /// `wrapCommandText` (`messages.ts:5496`) — coco-rs threads the
    /// typed [`QueueOrigin`] through so the per-origin framing matches
    /// TS exactly.
    ///
    /// Slash commands are excluded — they're processed post-turn and
    /// never become reminders.
    pub async fn snapshot_for_reminder(
        &self,
        agent_id: Option<&str>,
    ) -> Vec<coco_system_reminder::QueuedCommandInfo> {
        let queue = self.inner.lock().await;
        queue
            .iter()
            .filter(|c| !c.is_slash_command && c.agent_id.as_deref() == agent_id)
            .map(|c| coco_system_reminder::QueuedCommandInfo {
                content: c.prompt.clone(),
                origin: c.origin.clone(),
                images: c
                    .images
                    .iter()
                    .map(|img| coco_system_reminder::QueuedCommandImage {
                        media_type: img.media_type.clone(),
                        data_base64: img.data_base64.clone(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Remove all commands matching a predicate.
    pub async fn dequeue_matching<F>(&self, predicate: F) -> Vec<QueuedCommand>
    where
        F: Fn(&QueuedCommand) -> bool,
    {
        let mut queue = self.inner.lock().await;
        let mut matched = Vec::new();
        let mut i = 0;
        while i < queue.len() {
            if predicate(&queue[i]) {
                matched.push(queue.remove(i));
            } else {
                i += 1;
            }
        }
        if !matched.is_empty() {
            self.changed.notify_waiters();
        }
        matched
    }

    /// Remove and return the first command matching a predicate.
    pub async fn dequeue_first_matching<F>(&self, predicate: F) -> Option<QueuedCommand>
    where
        F: Fn(&QueuedCommand) -> bool,
    {
        let mut queue = self.inner.lock().await;
        let idx = queue.iter().position(predicate)?;
        let matched = queue.remove(idx);
        self.changed.notify_waiters();
        Some(matched)
    }

    /// Clear the entire queue.
    pub async fn clear(&self) {
        let mut queue = self.inner.lock().await;
        queue.clear();
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
#[path = "command_queue.test.rs"]
mod tests;
