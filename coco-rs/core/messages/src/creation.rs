use crate::ApiError;
use crate::AssistantContent;
use crate::AssistantMessage;
use crate::LlmMessage;
use crate::Message;
use crate::MessageOrigin;
use crate::ProgressMessage;
use crate::SystemApiErrorMessage;
use crate::SystemCompactBoundaryMessage;
use crate::SystemInformationalMessage;
use crate::SystemMessage;
use crate::SystemMessageLevel;
use crate::SystemUserInterruptionMessage;
use crate::ToolContent;
use crate::ToolResultMessage;
use crate::UserMessage;
use coco_llm_types::ToolResultContent;
use coco_llm_types::UserContentPart;
use coco_types::TokenUsage;
use coco_types::ToolId;
use uuid::Uuid;

/// Create a user message from text content.
pub fn create_user_message(text: &str) -> Message {
    create_user_message_with_uuid(Uuid::new_v4(), text)
}

/// Create a user message from text with a caller-supplied UUID.
///
/// Used by the TUI submit path so the UUID minted at user-input time is the
/// same one the engine, file-history snapshots, JSONL transcript, and rewind
/// picker see.
pub fn create_user_message_with_uuid(uuid: Uuid, text: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid,
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(MessageOrigin::UserInput),
        parent_tool_use_id: None,
    })
}

/// Create the clear-context plan-implementation seed message.
///
/// Same shape as [`create_user_message`] (model-visible user text) but tagged
/// `MessageOrigin::PlanImplementation` so the TUI renders it as a compact
/// chip instead of echoing the entire plan as a `❯` input wall. The body
/// still carries the full plan text — the model needs it after the history
/// clear.
pub fn create_plan_implementation_message(text: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(MessageOrigin::PlanImplementation),
        parent_tool_use_id: None,
    })
}

/// Create a user message with mixed content parts (text + images).
///
/// Used when the user input includes @-mentioned images or pasted images
/// alongside text. The provider layer (e.g. Anthropic) already handles
/// `UserContentPart::File` with image/* media types.
pub fn create_user_message_with_parts(parts: Vec<UserContentPart>) -> Message {
    create_user_message_with_parts_and_uuid(Uuid::new_v4(), parts)
}

/// Create a user message with parts and a caller-supplied UUID.
pub fn create_user_message_with_parts_and_uuid(uuid: Uuid, parts: Vec<UserContentPart>) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user(parts),
        uuid,
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(MessageOrigin::UserInput),
        parent_tool_use_id: None,
    })
}

/// Create a system-injected meta message (hidden from UI, visible to model).
///
/// Lands as `Message::Attachment` with [`CriticalSystemReminder`] kind —
/// the generic carrier for system-injected text whose content goes to the
/// model but shouldn't surface in the UI transcript as a "user" message.
pub fn create_meta_message(text: &str) -> Message {
    Message::Attachment(crate::AttachmentMessage::api(
        coco_types::AttachmentKind::CriticalSystemReminder,
        LlmMessage::user_text(text),
    ))
}

/// Create an informational system message.
pub fn create_info_message(title: &str, message: &str) -> Message {
    Message::System(SystemMessage::Informational(SystemInformationalMessage {
        uuid: Uuid::new_v4(),
        level: SystemMessageLevel::Info,
        title: title.to_string(),
        message: message.to_string(),
    }))
}

/// Create a display-only API/stream error row for the transcript.
///
/// Rendered inline by the TUI as a `⚠ <error>` row (mirrors TS Claude Code's
/// `SystemAPIErrorMessage`). `normalize_messages_for_api` drops every
/// `SystemMessage` variant except `LocalCommand`, so this never reaches the
/// model — it is pure transcript provenance for a failed turn.
pub fn create_api_error_message(error: &str, status_code: Option<i32>) -> Message {
    Message::System(SystemMessage::ApiError(SystemApiErrorMessage {
        uuid: Uuid::new_v4(),
        error: error.to_string(),
        status_code,
    }))
}

/// Create an assistant message with content parts, model, and usage.
pub fn create_assistant_message(
    content: Vec<AssistantContent>,
    model: &str,
    usage: TokenUsage,
) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(content),
        uuid: Uuid::new_v4(),
        model: model.to_string(),
        stop_reason: None,
        usage: Some(usage),
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

/// Create a tool result message.
pub fn create_tool_result_message(
    tool_call_id: &str,
    tool_name: &str,
    tool_id: ToolId,
    output: &str,
    is_error: bool,
) -> Message {
    // Surface result size + a one-line preview so operators can tell at a
    // glance whether a tool returned data, an error, or got truncated.
    // Span context (session_id / agent_id / turn_id) comes from the caller.
    tracing::debug!(
        target: "coco_messages::tool_result",
        tool_name = %tool_name,
        tool_call_id = %tool_call_id,
        result_bytes = output.len(),
        is_error,
        preview = %tool_result_preview(output),
        "tool result built"
    );
    let result_content = if is_error {
        ToolResultContent::error_text(output)
    } else {
        ToolResultContent::text(output)
    };
    let tool_result = crate::ToolResultContent {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        output: result_content,
        is_error,
        provider_metadata: None,
    };
    Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        message: LlmMessage::tool(vec![ToolContent::ToolResult(tool_result)]),
        display_data: None,
        tool_use_id: tool_call_id.to_string(),
        tool_id,
        is_error,
    })
}

/// Create an error tool result (shorthand for `create_tool_result_message` with `is_error=true`).
pub fn create_error_tool_result(
    tool_call_id: &str,
    tool_name: &str,
    tool_id: ToolId,
    error: &str,
) -> Message {
    create_tool_result_message(
        tool_call_id,
        tool_name,
        tool_id,
        error,
        /*is_error*/ true,
    )
}

/// Create a tool result message from a sequence of typed content
/// parts (text + images + documents).
///
/// Used by the executor when a tool's [`Tool::render_for_model`]
/// returns more than a single Text part — e.g. `FileReadTool` reading
/// a PNG returns one [`ToolResultContentPart::FileData`] block. The
/// underlying SDK enum [`coco_llm_types::ToolResultContent::Content`]
/// is the canonical multimodal carrier; provider crates already know
/// how to translate it (Anthropic / Gemini 3+ pass through; OpenAI /
/// OpenAI-Compatible degrade non-Text parts to a visible text marker).
///
/// Sibling of [`create_tool_result_message`], which takes a single
/// `&str` and stays the fast path for tools that just return
/// formatted text. The two paths produce semantically identical
/// `Message::ToolResult` envelopes — only the `output` variant
/// differs (`Text` / `ErrorText` vs `Content`).
///
/// `is_error` rides on the outer `ToolResultPart.is_error` flag (the
/// `Content` enum variant has no explicit error form).
pub fn create_tool_result_message_with_parts(
    tool_call_id: &str,
    tool_name: &str,
    tool_id: ToolId,
    parts: Vec<crate::ToolResultContentPart>,
    is_error: bool,
) -> Message {
    let result_content = ToolResultContent::content_parts(parts);
    let tool_result = crate::ToolResultContent {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        output: result_content,
        is_error,
        provider_metadata: None,
    };
    Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        message: LlmMessage::tool(vec![ToolContent::ToolResult(tool_result)]),
        display_data: None,
        tool_use_id: tool_call_id.to_string(),
        tool_id,
        is_error,
    })
}

/// Create a compact boundary system message recording token counts before/after compaction.
pub fn create_compact_boundary_message(tokens_before: i64, tokens_after: i64) -> Message {
    Message::System(SystemMessage::CompactBoundary(
        SystemCompactBoundaryMessage {
            uuid: Uuid::new_v4(),
            tokens_before,
            tokens_after,
            trigger: coco_types::CompactTrigger::Auto,
            user_context: None,
            messages_summarized: None,
            pre_compact_discovered_tools: vec![],
            preserved_segment: None,
        },
    ))
}

/// Create a progress message for streaming tool output.
pub fn create_progress_message(tool_use_id: &str, data: serde_json::Value) -> Message {
    Message::Progress(ProgressMessage {
        tool_use_id: tool_use_id.to_string(),
        data,
        parent_message_uuid: None,
    })
}

/// Literal text content for a Ctrl+C cancellation marker that lives in
/// the message history.
///
/// Rendered specially as the dim "Interrupted · What should Claude do instead?" row.
pub const INTERRUPT_MESSAGE: &str = "[Request interrupted by user]";

/// Variant of [`INTERRUPT_MESSAGE`] used when the cancel happened while a
/// tool was running. Carries slightly different model-facing context so
/// the model knows the prior turn's tool calls were interrupted, not
/// "the user typed a question and then cancelled".
pub const INTERRUPT_MESSAGE_FOR_TOOL_USE: &str = "[Request interrupted by user for tool use]";

/// Tool-result content for a tool whose turn was already aborted *before*
/// execution started. Yielded on the pre-execute abort path (with
/// `is_error: true`), distinct from the mid-execution interrupt messages.
pub const CANCEL_MESSAGE: &str = "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.";

/// Create the typed user-interruption SystemMessage variant. The
/// engine cancel finalizer is the single writer; downstream consumers
/// (TUI render, SDK observers) read `for_tool_use` from this struct
/// rather than recomputing it. See
/// `engine-tui-unified-transcript-plan.md` §7.1.
pub fn create_user_interruption_system_message(for_tool_use: bool) -> Message {
    Message::System(SystemMessage::UserInterruption(
        SystemUserInterruptionMessage {
            uuid: Uuid::new_v4(),
            for_tool_use,
        },
    ))
}

/// Legacy text-based interruption marker (User-role with literal
/// `INTERRUPT_MESSAGE*` text). Retained for backward read-compat with
/// older JSONL transcripts; new engine writes use
/// [`create_user_interruption_system_message`].
pub fn create_user_interruption_message(for_tool_use: bool) -> Message {
    let text = if for_tool_use {
        INTERRUPT_MESSAGE_FOR_TOOL_USE
    } else {
        INTERRUPT_MESSAGE
    };
    Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(MessageOrigin::UserInput),
        parent_tool_use_id: None,
    })
}

/// Create a system message about a tool permission denial.
pub fn create_permission_denied_message(tool_name: &str, reason: &str) -> Message {
    Message::System(SystemMessage::Informational(SystemInformationalMessage {
        uuid: Uuid::new_v4(),
        level: SystemMessageLevel::Warning,
        title: format!("Permission denied: {tool_name}"),
        message: reason.to_string(),
    }))
}

/// Create an assistant error message with an attached API error.
///
/// `error_type` is the short error code (`max_output_tokens`,
/// `prompt_too_long`, `content_filter`, `blocking_limit`, …) used by
/// hook matchers and the C3 death-spiral guard. Pass `None` only when
/// no canonical code applies — the StopFailure hook will then default
/// to `unknown`.
pub fn create_assistant_error_message(
    error: &str,
    request_id: Option<&str>,
    error_type: Option<&str>,
) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(vec![]),
        uuid: Uuid::new_v4(),
        model: String::new(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: request_id.map(str::to_string),
        api_error: Some(ApiError {
            message: error.to_string(),
            status_code: None,
            error_type: error_type.map(str::to_string),
        }),
    })
}

/// One-line preview of a tool result for `debug!` logging. UTF-8 safe,
/// strips interior newlines, capped at 160 chars.
fn tool_result_preview(output: &str) -> String {
    const MAX: usize = 160;
    let mut s: String = output.chars().take(MAX).collect();
    if output.chars().count() > MAX {
        s.push_str("...");
    }
    s.replace('\n', " ")
}

#[cfg(test)]
#[path = "creation.test.rs"]
mod tests;
