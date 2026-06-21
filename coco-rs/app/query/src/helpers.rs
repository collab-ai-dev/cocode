//! Small stateless helpers used by the session loop.
//!
//! Extracted from `engine.rs` to keep that module focused on orchestration.
//! All functions here are pure or I/O-free (no awaits except for the queue
//! drain), and easy to unit-test in isolation.

#[cfg(test)]
#[path = "helpers.test.rs"]
mod tests;

use coco_llm_types::AssistantContentPart;
use coco_llm_types::FilePart;
use coco_llm_types::UserContentPart;
use coco_messages::AssistantContent;
use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::UserMessage;
use coco_messages::create_error_tool_result;
use coco_messages::wrapping::wrap_in_system_reminder;
use coco_system_reminder::QueueOrigin;
use coco_system_reminder::wrap_command_text;
use coco_types::AttachmentKind;
use coco_types::MessageOrigin;
use coco_types::ToolId;
use std::sync::Arc;

use crate::BudgetTracker;
use crate::command_queue::CommandQueue;
use crate::command_queue::QueuePriority;
use crate::command_queue::QueuedCommand;
use crate::emit::emit_protocol;
use crate::emit::emit_stream;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ToolCompletionEventMode {
    Emit,
    Defer,
}

pub(crate) struct DeferredToolCompletionBuffer {
    next_model_index: usize,
    outcomes: Vec<coco_tool_runtime::UnstampedToolCallOutcome>,
}

impl DeferredToolCompletionBuffer {
    pub(crate) fn new(next_model_index: usize) -> Self {
        Self {
            next_model_index,
            outcomes: Vec::new(),
        }
    }

    pub(crate) fn next_model_index(&self) -> usize {
        self.next_model_index
    }

    pub(crate) fn into_outcomes(self) -> Vec<coco_tool_runtime::UnstampedToolCallOutcome> {
        self.outcomes
    }

    fn stage(
        &mut self,
        tool_use_id: &str,
        tool_id: ToolId,
        ordered_messages: Vec<Message>,
        error_kind: Option<coco_tool_runtime::ToolCallErrorKind>,
    ) {
        let model_index = self.next_model_index;
        self.next_model_index += 1;
        self.outcomes.push(build_streaming_early_outcome(
            tool_use_id,
            tool_id,
            model_index,
            ordered_messages,
            error_kind,
        ));
    }
}

/// Convert between the two-name alias for `AssistantContent`.
///
/// `coco_messages::AssistantContent` and `coco_llm_types::AssistantContentPart`
/// are the same type re-exported under two aliases (see `coco-types` re-export
/// section). This wrapper exists only to make the conversion intent explicit.
pub(crate) fn convert_to_assistant_content(part: AssistantContentPart) -> AssistantContent {
    part
}

/// Drain queued commands up to `max_priority` from `queue` into `history` as
/// user messages, then emit one `CommandDequeued{id}` per drained item plus
/// a `QueueStateChanged{queued: remaining}` summary.
///
/// Shared by the mid-turn `Now`-only checkpoint and the end-of-turn full drain.
///
/// The TUI's local queued-commands display
/// (`SessionState::queued_commands`) keys off the per-item `CommandDequeued`
/// for ordered removal, with `QueueStateChanged` as a count reconciliation
/// safety net. SDK clients can do the same — every queue entry has a
/// stable [`coco_query::QueuedCommand::id`] minted at construction so the
/// `CommandQueued` / `CommandDequeued` event pair is well-formed.
pub async fn drain_command_queue_into_history(
    queue: &CommandQueue,
    history: &mut MessageHistory,
    event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    max_priority: QueuePriority,
    agent_id: Option<&str>,
) {
    let queued = queue
        .get_commands_by_max_priority(max_priority, agent_id)
        .await;
    if queued.is_empty() {
        return;
    }
    let ids_to_remove: Vec<uuid::Uuid> = queued.iter().map(|c| c.id).collect();
    for cmd in &queued {
        crate::history_sync::history_push_and_emit(
            history,
            queued_command_to_message(cmd),
            event_tx,
        )
        .await;
    }
    queue.remove_by_ids(&ids_to_remove).await;
    for id in &ids_to_remove {
        let _ = emit_protocol(
            event_tx,
            crate::ServerNotification::CommandDequeued { id: id.to_string() },
        )
        .await;
    }
    let remaining = queue.len().await as i32;
    let _ = emit_protocol(
        event_tx,
        crate::ServerNotification::QueueStateChanged { queued: remaining },
    )
    .await;
}

/// Convert a drained [`QueuedCommand`] into the history `Message` that
/// represents it, mirroring TS: a **human** steering message becomes a
/// first-class [`Message::User`] carrying the **raw** text (fully visible in
/// the transcript and counted as a user message); every other origin
/// (coordinator / task-notification / channel) stays a model-only
/// [`AttachmentKind::QueuedCommand`] attachment.
///
/// For the human case the model-facing "The user sent a new message while you
/// were working…" framing is **not** baked here — it is applied at prompt-build
/// time by [`wrap_steering_messages_for_api`], so the stored message stays raw.
pub fn queued_command_to_message(cmd: &QueuedCommand) -> Message {
    match cmd.origin {
        None | Some(QueueOrigin::Human) => queued_command_to_user_message(cmd),
        Some(_) => queued_command_to_attachment(cmd),
    }
}

/// Build the raw `Message::User` for a human steering command (no framing,
/// no `<system-reminder>` wrap). Tagged [`MessageOrigin::QueuedSteering`] so
/// the prompt builder knows to apply the model-facing wrapper.
pub fn queued_command_to_user_message(cmd: &QueuedCommand) -> Message {
    let mut parts: Vec<UserContentPart> = vec![UserContentPart::text(cmd.prompt.clone())];
    for img in &cmd.images {
        parts.push(UserContentPart::File(FilePart::image_base64(
            img.data_base64.clone(),
            img.media_type.clone(),
        )));
    }
    Message::User(UserMessage {
        message: LlmMessage::user(parts),
        uuid: uuid::Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(MessageOrigin::QueuedSteering),
        parent_tool_use_id: None,
    })
}

/// Apply the model-facing steering wrapper to every [`MessageOrigin::QueuedSteering`]
/// user message, returning an API-bound view. Mirrors TS, which wraps queued
/// commands only at API-serialization time (`wrapCommandText` in the
/// `queued_command` attachment case) while the stored message keeps the raw text.
///
/// CoW: only steering messages are rebuilt; every other entry keeps its `Arc`.
/// The wrapped text is `<system-reminder>`-prefixed, so the downstream
/// [`coco_messages::smoosh_system_reminder_into_tool_result`] folds it into the
/// preceding tool result exactly as before.
pub fn wrap_steering_messages_for_api(messages: &[Arc<Message>]) -> Vec<Arc<Message>> {
    messages
        .iter()
        .map(|m| match m.as_ref() {
            Message::User(u) if u.origin == Some(MessageOrigin::QueuedSteering) => {
                let mut wrapped = u.clone();
                wrapped.message = wrap_steering_llm_message(&u.message);
                Arc::new(Message::User(wrapped))
            }
            _ => m.clone(),
        })
        .collect()
}

/// Wrap a raw user `LlmMessage`'s text with the human steering framing +
/// `<system-reminder>` tags, preserving any non-text parts (e.g. pasted
/// images) after the wrapped text.
fn wrap_steering_llm_message(raw: &LlmMessage) -> LlmMessage {
    let LlmMessage::User { content, .. } = raw else {
        return raw.clone();
    };
    let mut text = String::new();
    let mut others: Vec<UserContentPart> = Vec::new();
    for part in content {
        match part {
            UserContentPart::Text(t) => text.push_str(&t.text),
            other => others.push(other.clone()),
        }
    }
    let framed = wrap_command_text(&text, Some(&QueueOrigin::Human));
    let mut parts: Vec<UserContentPart> =
        vec![UserContentPart::text(wrap_in_system_reminder(&framed))];
    parts.extend(others);
    LlmMessage::user(parts)
}

/// Convert a [`QueuedCommand`] drained from the queue into a model-bound
/// `AttachmentMessage` of kind [`AttachmentKind::QueuedCommand`].
///
/// Used for non-human queue origins (coordinator / task-notification /
/// channel) whose framing is model-only and which never surface as a user
/// message. Human steering goes through [`queued_command_to_user_message`].
///
/// Two-step wrapping:
///
/// 1. [`wrap_command_text`] adds origin-specific framing prose
///    ("The user sent a new message while you were working:" /
///    "The coordinator sent a message…" / etc.).
/// 2. [`wrap_in_system_reminder`] wraps the result in
///    `<system-reminder>…</system-reminder>` so the model recognises
///    the entry as a system-injected interruption rather than a fresh
///    user message.
///
/// Image attachments (mid-turn screenshot pastes) ride along as
/// additional `UserContentPart`s after the wrapped text, appending
/// image blocks after the text. Only the text gets the
/// system-reminder wrap; image bytes ride alongside untouched.
///
/// Lands as `Message::Attachment` with `kind = QueuedCommand`. The
/// kind threads through to UI rendering
/// (`AttachmentKind::renders_in_transcript`) and to the API
/// normalization path that surfaces the wrapped text to the model.
pub fn queued_command_to_attachment(cmd: &QueuedCommand) -> Message {
    let framed = wrap_command_text(&cmd.prompt, cmd.origin.as_ref());
    let wrapped = wrap_in_system_reminder(&framed);
    let llm_message = if cmd.images.is_empty() {
        LlmMessage::user_text(wrapped)
    } else {
        let mut parts: Vec<UserContentPart> = vec![UserContentPart::text(wrapped)];
        for img in &cmd.images {
            parts.push(UserContentPart::File(FilePart::image_base64(
                img.data_base64.clone(),
                img.media_type.clone(),
            )));
        }
        LlmMessage::user(parts)
    };
    Message::Attachment(AttachmentMessage::api(
        AttachmentKind::QueuedCommand,
        llm_message,
    ))
}

/// Whether the current budget state warrants forcing another turn.
///
/// Continue when under 90% of budget AND not in diminishing-returns
/// territory (continuation_count < 3).
pub(crate) fn should_continue_for_budget(budget: &BudgetTracker) -> bool {
    let Some(max) = budget.max_tokens else {
        return false;
    };
    if max <= 0 {
        return false;
    }
    let pct = budget.total_tokens() as f64 / max as f64;
    pct < 0.9 && budget.continuation_count() < 3
}

pub(crate) fn budget_pct_used(budget: &BudgetTracker) -> i32 {
    match budget.max_tokens {
        Some(max) if max > 0 => ((budget.total_tokens() as f64 / max as f64) * 100.0) as i32,
        _ => 0,
    }
}

/// Build the user-facing assistant message for an abnormal-stop_reason turn.
///
/// Returned message has empty content (the partial real response was
/// already pushed) and `api_error.message` carrying the human-readable
/// explanation. `ContextWindowExceeded` is a first-class variant
/// (no raw-string sniffing needed). Message text stays provider-agnostic
/// so it covers the multi-LLM unified bucket (Anthropic refusal, OpenAI
/// content_filter, Google SAFETY / RECITATION → `ContentFilter`).
pub(crate) fn build_abnormal_stop_api_error_message(
    parsed: coco_messages::StopReason,
    effective_max_tokens: Option<i64>,
) -> coco_messages::Message {
    use coco_messages::StopReason;
    const PREFIX: &str = "API Error";
    let (text, error_type): (String, &str) = match parsed {
        StopReason::ContextWindowExceeded => (
            format!("{PREFIX}: The model has reached its context window limit."),
            "prompt_too_long",
        ),
        StopReason::MaxTokens => (
            match effective_max_tokens {
                Some(n) if n > 0 => format!(
                    "{PREFIX}: Model response exceeded the {n} output token maximum. \
                     To increase, set `max_output_tokens` in settings.json or via `--max-tokens`."
                ),
                _ => format!(
                    "{PREFIX}: Model response exceeded the configured output token maximum."
                ),
            },
            "max_output_tokens",
        ),
        StopReason::ContentFilter => (
            format!(
                "{PREFIX}: Model declined to respond — the request appears to violate the \
                 provider's content policy or safety filter. Try rephrasing the request or \
                 start a new session."
            ),
            "content_filter",
        ),
        other => (
            format!(
                "{PREFIX}: Turn ended on stop_reason={}.",
                other.as_wire_str()
            ),
            "model_error",
        ),
    };
    coco_messages::create_assistant_error_message(&text, None, Some(error_type))
}

/// Build the synthetic assistant message for the pre-API blocking-limit
/// gate (Finding **C15**). Distinct from
/// [`build_abnormal_stop_api_error_message`] because the blocking-limit
/// is a *client-side* gate decision — no real provider error reached the
/// engine — and labels it `error: 'invalid_request'` rather than
/// `prompt_too_long` so hook matchers can distinguish "we never sent"
/// from "the provider rejected after sending".
pub(crate) fn build_blocking_limit_api_error_message(
    estimated_tokens: i64,
    context_window: i64,
) -> coco_messages::Message {
    let text = format!(
        "API Error: Estimated prompt size ({estimated_tokens} tokens) exceeds the \
         active model's context window ({context_window} tokens). The request was \
         not sent. Reduce history or switch to a model with a larger window."
    );
    coco_messages::create_assistant_error_message(&text, None, Some("invalid_request"))
}

/// Map `HookOutcome` to the protocol-layer `HookOutcomeStatus`.
/// Treats Blocking as Error since blocking is a user-visible failure from the
/// SDK consumer's perspective.
pub(crate) fn hook_outcome_to_status(
    outcome: coco_types::HookOutcome,
) -> coco_types::HookOutcomeStatus {
    match outcome {
        coco_types::HookOutcome::Success => coco_types::HookOutcomeStatus::Success,
        coco_types::HookOutcome::Blocking => coco_types::HookOutcomeStatus::Error,
        coco_types::HookOutcome::NonBlockingError => coco_types::HookOutcomeStatus::Error,
        coco_types::HookOutcome::Cancelled => coco_types::HookOutcomeStatus::Cancelled,
    }
}

/// Extract the last assistant text from message history.
pub(crate) fn extract_last_assistant_text(history: &MessageHistory) -> String {
    history
        .iter()
        .rev()
        .find_map(|m| match m.as_ref() {
            Message::Assistant(a) => match &a.message {
                LlmMessage::Assistant { content, .. } => content.iter().find_map(|c| {
                    if let AssistantContent::Text(t) = c {
                        Some(t.text.clone())
                    } else {
                        None
                    }
                }),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_default()
}

/// Complete a committed tool call with a model-visible error result.
///
/// JSON parse failures never reach this helper because they are dropped before
/// the assistant message is committed. Every committed early-return path should
/// use this so the stream event and history pair stay in sync.
///
/// **Streaming-mode note (I1 ordering).** In the streaming agent loop, the
/// assistant message is committed *after* this helper runs (when the stream
/// hits `Finish`). For multi-tool streams where one call passes preparation
/// and another fails, inline history insertion would produce
/// `user(tool_result) → assistant(tool_use)` ordering that violates Anthropic's
/// adjacency invariant. The streaming path therefore stages a typed
/// [`UnstampedToolCallOutcome`] in [`DeferredToolCompletionBuffer`] so
/// `commit_flush` surfaces it in the correct post-assistant slot.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn complete_tool_call_with_error_mode(
    event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    history: &mut MessageHistory,
    tool_call_id: &str,
    tool_name: &str,
    tool_id: &ToolId,
    output: &str,
    error_kind: coco_tool_runtime::ToolCallErrorKind,
    event_mode: ToolCompletionEventMode,
    deferred: Option<&mut DeferredToolCompletionBuffer>,
) {
    let message = create_error_tool_result(tool_call_id, tool_name, tool_id.clone(), output);
    match event_mode {
        ToolCompletionEventMode::Emit => {
            let _delivered = emit_stream(
                event_tx,
                crate::AgentStreamEvent::ToolUseCompleted {
                    call_id: tool_call_id.to_string(),
                    name: tool_name.to_string(),
                    output: output.to_string(),
                    is_error: true,
                },
            )
            .await;
            crate::history_sync::history_push_and_emit(history, message, event_tx).await;
        }
        ToolCompletionEventMode::Defer => {
            if let Some(deferred) = deferred {
                deferred.stage(
                    tool_call_id,
                    tool_id.clone(),
                    vec![message],
                    Some(error_kind),
                );
            } else {
                history.push(message);
            }
        }
    }
}

/// Complete a tool call with a NON-error result carrying user feedback.
///
/// Used when the user redirects an interactive tool rather than denying it —
/// e.g. AskUserQuestion's "Chat about this" / "Skip interview". The feedback
/// still reaches the model (so it re-engages), but the transcript renders it as
/// a neutral result instead of a red "Permission denied" error, and it is NOT
/// counted as a permission denial.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn complete_tool_call_clarification(
    event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    history: &mut MessageHistory,
    tool_call_id: &str,
    tool_name: &str,
    tool_id: &ToolId,
    output: &str,
    event_mode: ToolCompletionEventMode,
    deferred: Option<&mut DeferredToolCompletionBuffer>,
) {
    let message = coco_messages::create_tool_result_message(
        tool_call_id,
        tool_name,
        tool_id.clone(),
        output,
        /*is_error*/ false,
    );
    match event_mode {
        ToolCompletionEventMode::Emit => {
            let _delivered = emit_stream(
                event_tx,
                crate::AgentStreamEvent::ToolUseCompleted {
                    call_id: tool_call_id.to_string(),
                    name: tool_name.to_string(),
                    output: output.to_string(),
                    is_error: false,
                },
            )
            .await;
            crate::history_sync::history_push_and_emit(history, message, event_tx).await;
        }
        ToolCompletionEventMode::Defer => {
            if let Some(deferred) = deferred {
                deferred.stage(tool_call_id, tool_id.clone(), vec![message], None);
            } else {
                history.push(message);
            }
        }
    }
}

/// Build a typed early-return outcome so the streaming agent loop can surface
/// staged preparation results via
/// [`StreamingHandle::feed_plan(ToolCallPlan::EarlyOutcome(...))`].
///
/// `error_kind` is carried from the exact source branch. Non-error
/// clarification feedback, such as AskUserQuestion redirect text, passes
/// `None` so `ToolUseCompleted.is_error` remains false.
pub(crate) fn build_streaming_early_outcome(
    tool_use_id: &str,
    tool_id: ToolId,
    model_index: usize,
    captured_messages: Vec<Message>,
    error_kind: Option<coco_tool_runtime::ToolCallErrorKind>,
) -> coco_tool_runtime::UnstampedToolCallOutcome {
    coco_tool_runtime::UnstampedToolCallOutcome {
        tool_use_id: tool_use_id.to_string(),
        tool_id,
        model_index,
        ordered_messages: captured_messages,
        message_path: coco_tool_runtime::ToolMessagePath::EarlyReturn,
        error_kind,
        permission_denial: None,
        prevent_continuation: None,
        structured_output: None,
        effects: coco_tool_runtime::ToolSideEffects::none(),
    }
}
