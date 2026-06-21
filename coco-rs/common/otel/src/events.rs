//! Application event catalog for analytics/telemetry.
//!
//! Each event carries structured attributes emitted via `tracing::info!`
//! and picked up by the OTel pipeline.

use serde::Serialize;
use std::collections::HashMap;

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

#[cfg(test)]
#[path = "events.test.rs"]
mod tests;
