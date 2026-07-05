//! Application event catalog for analytics/telemetry.
//!
//! Each event carries structured attributes emitted via `tracing::info!`
//! and picked up by the OTel pipeline.

use chrono::SecondsFormat;
use chrono::Utc;
use coco_config::EnvKey;
use coco_config::constants::PRODUCT_NAME;
use coco_config::env::is_env_truthy;
use coco_config::env::resolve_log_assistant_responses;
use coco_utils_string::take_bytes_at_char_boundary;
use serde::Serialize;
use std::collections::HashMap;

pub(crate) const TELEMETRY_CONTENT_LIMIT_BYTES: usize = 60 * 1024;
pub(crate) const TELEMETRY_TRUNCATION_MARKER: &str = "\n\n[TRUNCATED - Content exceeds 60KB limit]";
pub(crate) const REDACTED: &str = "<REDACTED>";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AssistantResponsePayload {
    pub(crate) response_length: i64,
    pub(crate) response: String,
}

#[derive(Debug, Clone, Copy)]
pub struct PromptSuggestionFilteredPayload<'a> {
    pub rule: &'a str,
    pub suggestion_text: &'a str,
    pub text_len_bytes: i64,
    pub char_count: i64,
    pub utf16_len: i64,
    pub word_count: i64,
    pub cjk_char_count: i64,
    pub contains_cjk: bool,
    pub request_id: Option<&'a str>,
    pub log_assistant_responses: Option<bool>,
}

/// Application event types (L3 — application-level analytics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppEventType {
    // ── Session lifecycle ──
    SessionStart,
    SessionEnd,
    SessionResume,
    SessionFork,

    // ── Agent turns ──
    TurnStart,
    TurnEnd,
    TurnContinue,

    // ── Tool execution ──
    ToolUse,
    ToolError,
    ToolPermissionDenied,
    ToolPermissionAllowed,

    // ── Model/inference ──
    ApiRequest,
    ApiResponse,
    AssistantResponse,
    PromptSuggestionFiltered,
    ApiError,
    ApiRetry,
    ModelSwitch,
    ThinkingLevelChange,

    // ── Compaction ──
    CompactStart,
    CompactEnd,
    MicroCompact,
    ReactiveCompact,

    // ── Commands ──
    SlashCommand,
    SkillInvocation,

    // ── File operations ──
    FileRead,
    FileWrite,
    FileEdit,
    FileBackupCreated,
    FileRewind,

    // ── Auth ──
    AuthLogin,
    AuthLogout,
    AuthRefresh,
    AuthError,

    // ── Agent/subagent ──
    SubagentSpawn,
    SubagentComplete,
    SubagentError,

    // ── MCP ──
    McpServerConnect,
    McpServerDisconnect,
    McpToolCall,

    // ── User input ──
    UserPrompt,
    UserInterrupt,
}

impl AppEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::SessionEnd => "session_end",
            Self::SessionResume => "session_resume",
            Self::SessionFork => "session_fork",
            Self::TurnStart => "turn_start",
            Self::TurnEnd => "turn_end",
            Self::TurnContinue => "turn_continue",
            Self::ToolUse => "tool_use",
            Self::ToolError => "tool_error",
            Self::ToolPermissionDenied => "tool_permission_denied",
            Self::ToolPermissionAllowed => "tool_permission_allowed",
            Self::ApiRequest => "api_request",
            Self::ApiResponse => "api_response",
            Self::AssistantResponse => "assistant_response",
            Self::PromptSuggestionFiltered => "prompt_suggestion_filtered",
            Self::ApiError => "api_error",
            Self::ApiRetry => "api_retry",
            Self::ModelSwitch => "model_switch",
            Self::ThinkingLevelChange => "thinking_level_change",
            Self::CompactStart => "compact_start",
            Self::CompactEnd => "compact_end",
            Self::MicroCompact => "micro_compact",
            Self::ReactiveCompact => "reactive_compact",
            Self::SlashCommand => "slash_command",
            Self::SkillInvocation => "skill_invocation",
            Self::FileRead => "file_read",
            Self::FileWrite => "file_write",
            Self::FileEdit => "file_edit",
            Self::FileBackupCreated => "file_backup_created",
            Self::FileRewind => "file_rewind",
            Self::AuthLogin => "auth_login",
            Self::AuthLogout => "auth_logout",
            Self::AuthRefresh => "auth_refresh",
            Self::AuthError => "auth_error",
            Self::SubagentSpawn => "subagent_spawn",
            Self::SubagentComplete => "subagent_complete",
            Self::SubagentError => "subagent_error",
            Self::McpServerConnect => "mcp_server_connect",
            Self::McpServerDisconnect => "mcp_server_disconnect",
            Self::McpToolCall => "mcp_tool_call",
            Self::UserPrompt => "user_prompt",
            Self::UserInterrupt => "user_interrupt",
        }
    }
}

pub fn otel_event_name(name: &str) -> String {
    format!("{PRODUCT_NAME}.{name}")
}

/// A structured application event with typed attributes.
#[derive(Debug, Clone, Serialize)]
pub struct AppEvent {
    pub event_type: AppEventType,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub attributes: HashMap<String, serde_json::Value>,
}

impl AppEvent {
    /// Create a new event with the given type and current timestamp.
    pub fn new(event_type: AppEventType) -> Self {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Self {
            event_type,
            timestamp_ms,
            attributes: HashMap::new(),
        }
    }

    /// Add a string attribute.
    pub fn with_str(mut self, key: &str, value: &str) -> Self {
        self.attributes.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
        self
    }

    /// Add an integer attribute.
    pub fn with_int(mut self, key: &str, value: i64) -> Self {
        self.attributes
            .insert(key.to_string(), serde_json::json!(value));
        self
    }

    /// Add a float attribute.
    pub fn with_float(mut self, key: &str, value: f64) -> Self {
        self.attributes
            .insert(key.to_string(), serde_json::json!(value));
        self
    }

    /// Add a boolean attribute.
    pub fn with_bool(mut self, key: &str, value: bool) -> Self {
        self.attributes
            .insert(key.to_string(), serde_json::json!(value));
        self
    }
}

/// Emit an application event via the tracing pipeline.
///
/// Events are emitted as `tracing::info!` with structured fields so they
/// flow through the OTel pipeline to configured exporters.
pub fn emit_event(event: &AppEvent) {
    tracing::info!(
        event_type = event.event_type.as_str(),
        timestamp_ms = event.timestamp_ms,
        attributes = %serde_json::to_string(&event.attributes).unwrap_or_default(),
        "app_event"
    );
}

// ── Convenience emitters for common events ──

/// Emit a session start event.
pub fn emit_session_start(session_id: &str, model: &str) {
    emit_event(
        &AppEvent::new(AppEventType::SessionStart)
            .with_str("session_id", session_id)
            .with_str("model", model),
    );
}

/// Emit a tool use event.
pub fn emit_tool_use(tool_name: &str, duration_ms: i64, success: bool) {
    emit_event(
        &AppEvent::new(AppEventType::ToolUse)
            .with_str("tool_name", tool_name)
            .with_int("duration_ms", duration_ms)
            .with_bool("success", success),
    );
}

/// Emit a file-history backup-created event.
pub fn emit_file_backup_created(file_path: &str, version: i32, file_size: u64) {
    emit_event(
        &AppEvent::new(AppEventType::FileBackupCreated)
            .with_str("file_path", file_path)
            .with_int("version", i64::from(version))
            .with_int("file_size", file_size as i64),
    );
}

/// Emit a file-history snapshot-success event.
pub fn emit_file_snapshot_success(tracked_files: usize, snapshot_count: usize) {
    emit_event(
        &AppEvent::new(AppEventType::FileEdit)
            .with_str("event", "snapshot_success")
            .with_int("tracked_files", tracked_files as i64)
            .with_int("snapshot_count", snapshot_count as i64),
    );
}

/// Emit a file-history track-edit-success event.
pub fn emit_file_track_edit_success(file: &str, version: i32, is_new_file: bool) {
    emit_event(
        &AppEvent::new(AppEventType::FileEdit)
            .with_str("event", "track_edit_success")
            .with_str("file", file)
            .with_int("version", i64::from(version))
            .with_bool("is_new_file", is_new_file),
    );
}

/// Emit a file-history rewind-success event.
pub fn emit_file_rewind_success(tracked_files: usize, files_changed: usize) {
    emit_event(
        &AppEvent::new(AppEventType::FileRewind)
            .with_str("event", "rewind_success")
            .with_int("tracked_files", tracked_files as i64)
            .with_int("files_changed", files_changed as i64),
    );
}

/// Emit a file-history rewind-failed event.
pub fn emit_file_rewind_failed(reason: &str) {
    emit_event(
        &AppEvent::new(AppEventType::FileRewind)
            .with_str("event", "rewind_failed")
            .with_str("reason", reason),
    );
}

/// Emit a conversation-rewind event. Recorded when the user truncates
/// the active history via the rewind picker.
pub fn emit_conversation_rewind(
    pre_count: i64,
    post_count: i64,
    messages_removed: i64,
    rewind_to_index: i64,
) {
    emit_event(
        &AppEvent::new(AppEventType::TurnContinue)
            .with_str("event", "conversation_rewind")
            .with_int("pre_rewind_message_count", pre_count)
            .with_int("post_rewind_message_count", post_count)
            .with_int("messages_removed", messages_removed)
            .with_int("rewind_to_message_index", rewind_to_index),
    );
}

/// Emit an API request event.
pub fn emit_api_request(model: &str, input_tokens: i64, output_tokens: i64, cost_usd: f64) {
    emit_event(
        &AppEvent::new(AppEventType::ApiResponse)
            .with_str("model", model)
            .with_int("input_tokens", input_tokens)
            .with_int("output_tokens", output_tokens)
            .with_float("cost_usd", cost_usd),
    );
}

/// Emit an assistant-response event for a completed model turn.
///
/// Mirrors Claude Code v2.1.193: tool-only/empty text does not emit; response
/// body logging is controlled by
/// `OTEL_LOG_ASSISTANT_RESPONSES ?? OTEL_LOG_USER_PROMPTS`.
pub fn emit_assistant_response(
    response_text: &str,
    model: &str,
    request_id: Option<&str>,
    query_source: &str,
    log_assistant_responses: Option<bool>,
) {
    let log_user_prompts = is_env_truthy(EnvKey::OtelLogUserPrompts);
    let Some(payload) =
        build_assistant_response_payload(response_text, log_user_prompts, log_assistant_responses)
    else {
        return;
    };

    tracing::event!(
        tracing::Level::INFO,
        event.name = %otel_event_name(AppEventType::AssistantResponse.as_str()),
        event.timestamp = %timestamp(),
        model = %model,
        response_length = payload.response_length,
        response = %payload.response,
        request_id = request_id,
        query_source = %query_source,
    );
}

pub fn emit_prompt_suggestion_filtered(payload: PromptSuggestionFilteredPayload<'_>) {
    let log_user_prompts = is_env_truthy(EnvKey::OtelLogUserPrompts);
    let suggestion_text =
        if resolve_log_assistant_responses(payload.log_assistant_responses, log_user_prompts) {
            truncate_for_telemetry(payload.suggestion_text)
        } else {
            REDACTED.to_string()
        };

    tracing::event!(
        tracing::Level::INFO,
        event.name = %otel_event_name(AppEventType::PromptSuggestionFiltered.as_str()),
        event.timestamp = %timestamp(),
        rule = %payload.rule,
        suggestion_text = %suggestion_text,
        text_len_bytes = payload.text_len_bytes,
        char_count = payload.char_count,
        utf16_len = payload.utf16_len,
        word_count = payload.word_count,
        cjk_char_count = payload.cjk_char_count,
        contains_cjk = payload.contains_cjk,
        request_id = payload.request_id,
        query_source = "prompt_suggestion",
    );
}

/// Emit a slash command event.
pub fn emit_slash_command(command_name: &str) {
    emit_event(&AppEvent::new(AppEventType::SlashCommand).with_str("command", command_name));
}

/// Emit a subagent spawn event.
pub fn emit_subagent_spawn(agent_id: &str, agent_type: &str, model: &str) {
    emit_event(
        &AppEvent::new(AppEventType::SubagentSpawn)
            .with_str("agent_id", agent_id)
            .with_str("agent_type", agent_type)
            .with_str("model", model),
    );
}

pub(crate) fn build_assistant_response_payload(
    response_text: &str,
    log_user_prompts: bool,
    log_assistant_responses: Option<bool>,
) -> Option<AssistantResponsePayload> {
    if response_text.is_empty() {
        return None;
    }

    let response = if resolve_log_assistant_responses(log_assistant_responses, log_user_prompts) {
        truncate_for_telemetry(response_text)
    } else {
        REDACTED.to_string()
    };

    Some(AssistantResponsePayload {
        response_length: response_text.encode_utf16().count() as i64,
        response,
    })
}

fn truncate_for_telemetry(content: &str) -> String {
    if content.len() <= TELEMETRY_CONTENT_LIMIT_BYTES {
        return content.to_string();
    }

    format!(
        "{}{}",
        take_bytes_at_char_boundary(content, TELEMETRY_CONTENT_LIMIT_BYTES),
        TELEMETRY_TRUNCATION_MARKER
    )
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
#[path = "events.test.rs"]
mod tests;
