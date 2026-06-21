//! SDK hook callback output — TS-canonical wire shape.
//!
//! TS reference: `src/entrypoints/sdk/coreSchemas.ts` —
//! `hookJSONOutputSchema`, `syncHookResponseSchema`,
//! `asyncHookResponseSchema`, `hookSpecificOutputSchema`,
//! `permissionRequestDecisionSchema`.
//!
//! The wire shape is a single flat object whose `async` field
//! discriminates between async-mode (further fields ignored except
//! `asyncTimeout`) and sync-mode (the rest). Every field is optional;
//! a `{}` response means "no opinion, continue normally". The
//! per-event `hookSpecificOutput` discriminates on `hookEventName`.
//!
//! **No translation layer.** The Rust runtime, the SDK wire, and the
//! TS reference all use this exact same shape — the hook orchestrator
//! consumes it directly via [`crate::HookSpecificOutput`].
//!
//! Fields use camelCase on the wire to match TS / Python SDK output.
//! `#[serde(alias = "snake_case")]` is allowed where coco-rs's older
//! shell-hook JSON stdout format used snake_case so the parser stays
//! bidirectional, but the canonical emission is always camelCase.

use serde::Deserialize;
use serde::Serialize;

/// SDK hook callback output — a flat object whose `async` field
/// discriminates async-mode.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkHookOutput {
    /// Async mode opt-in. `Some(true)` ⇒ this is an async response;
    /// every sync field below is ignored and the caller awaits the
    /// async result through the side channel. Absent / `Some(false)` ⇒
    /// sync response (the rest of the fields apply).
    #[serde(default, rename = "async", skip_serializing_if = "Option::is_none")]
    pub r#async: Option<bool>,

    /// Optional async timeout in milliseconds. Only meaningful when
    /// `async: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_timeout: Option<i64>,

    // ── Sync-mode fields (TS `syncHookResponseSchema`) ───────────────
    /// Stop the agent loop after this turn when `Some(false)`.
    /// Pair with `stopReason` for the visible message.
    #[serde(default, rename = "continue", skip_serializing_if = "Option::is_none")]
    pub r#continue: Option<bool>,

    /// Suppress default output rendering for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_output: Option<bool>,

    /// Visible reason when `continue: false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,

    /// Permission-style decision applied to the pending tool call.
    /// `block` means deny; `approve` means allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,

    /// Human-readable reason associated with `decision`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Free-form message injected into the conversation as a system
    /// message (visible to model and user). TS parity:
    /// `syncHookResponseSchema.systemMessage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,

    /// Event-specific structured output. Tagged by `hookEventName`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<HookSpecificOutput>,
}

/// Top-level `decision` field. TS:
/// `z.enum(['approve', 'block'])`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookDecision {
    Approve,
    Block,
}

// Permission decision used inside `hookSpecificOutput.PreToolUse`
// reuses the existing `HookPermissionDecision` defined in
// `messages::attachment_body`. TS canonical:
// `z.enum(['allow', 'deny'])`. Both call sites (hook output and silent
// attachment payload) want the same 2-variant decision, so there is
// **one** enum across the whole workspace.
//
// Re-exported by `lib.rs` under the same name — consumers can import
// either path.

/// Elicitation user action. TS:
/// `z.enum(['accept', 'decline', 'cancel'])`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

/// Event-specific hook output. Tagged by `hookEventName`.
///
/// Variants cover every `HOOK_EVENT` value that can carry structured
/// fields back to the agent.
///
/// Each variant carries `rename_all = "camelCase"` so its inner fields
/// match the TS canonical wire shape (`permissionDecision`,
/// `additionalContext`, `updatedInput`, etc.). The enum-level
/// `rename_all` only applies to variant names, not variant fields, so
/// each struct-like variant needs its own attribute.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "hookEventName")]
pub enum HookSpecificOutput {
    #[serde(rename_all = "camelCase")]
    PreToolUse {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_decision: Option<crate::HookPermissionDecision>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_decision_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    PostToolUse {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
        #[serde(
            default,
            rename = "updatedMCPToolOutput",
            skip_serializing_if = "Option::is_none"
        )]
        updated_mcp_tool_output: Option<serde_json::Value>,
    },
    #[serde(rename_all = "camelCase")]
    PostToolUseFailure {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    UserPromptSubmit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SessionStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial_user_message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_paths: Option<Vec<String>>,
    },
    #[serde(rename_all = "camelCase")]
    Setup {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SubagentStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    PermissionDenied {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    Notification {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    PermissionRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<PermissionRequestDecision>,
    },
    Elicitation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<ElicitationAction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<serde_json::Value>,
    },
    ElicitationResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<ElicitationAction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<serde_json::Value>,
    },
    #[serde(rename_all = "camelCase")]
    CwdChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_paths: Option<Vec<String>>,
    },
    #[serde(rename_all = "camelCase")]
    FileChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_paths: Option<Vec<String>>,
    },
    #[serde(rename_all = "camelCase")]
    WorktreeCreate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
    },
}

impl HookSpecificOutput {
    /// The `hookEventName` claimed by this output. Used to enforce TS's
    /// `processHookJSONOutput` cross-check: a hook firing for event X
    /// emitting a `hookSpecificOutput` for event Y is invalid.
    pub fn claimed_event(&self) -> crate::HookEventType {
        use crate::HookEventType;
        match self {
            Self::PreToolUse { .. } => HookEventType::PreToolUse,
            Self::PostToolUse { .. } => HookEventType::PostToolUse,
            Self::PostToolUseFailure { .. } => HookEventType::PostToolUseFailure,
            Self::UserPromptSubmit { .. } => HookEventType::UserPromptSubmit,
            Self::SessionStart { .. } => HookEventType::SessionStart,
            Self::Setup { .. } => HookEventType::Setup,
            Self::SubagentStart { .. } => HookEventType::SubagentStart,
            Self::PermissionDenied { .. } => HookEventType::PermissionDenied,
            Self::Notification { .. } => HookEventType::Notification,
            Self::PermissionRequest { .. } => HookEventType::PermissionRequest,
            Self::Elicitation { .. } => HookEventType::Elicitation,
            Self::ElicitationResult { .. } => HookEventType::ElicitationResult,
            Self::CwdChanged { .. } => HookEventType::CwdChanged,
            Self::FileChanged { .. } => HookEventType::FileChanged,
            Self::WorktreeCreate { .. } => HookEventType::WorktreeCreate,
        }
    }
}

/// Decision returned by a PermissionRequest hook. Tagged by
/// `behavior`. TS: `permissionRequestDecisionSchema`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "lowercase")]
pub enum PermissionRequestDecision {
    #[serde(rename_all = "camelCase")]
    Allow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
    },
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interrupt: Option<bool>,
    },
}

/// Result body for the synchronous JSON-RPC reply to a
/// `hook/callback` server request.
///
/// Correlation is via the outer JSON-RPC `request_id` on the response
/// envelope — there is no inner correlation field. The whole body is
/// the hook output in TS-canonical shape; downstream parsers consume
/// `SdkHookOutput` directly via [`HookCallbackResult::output`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookCallbackResult {
    /// Hook output in TS-canonical shape (TS `hookJSONOutputSchema`).
    pub output: SdkHookOutput,
}

/// Result body for the synchronous JSON-RPC reply to a
/// `mcp/routeMessage` server request.
///
/// Carries the forwarded JSON-RPC response from the SDK-hosted MCP
/// server verbatim. Correlation is via the outer JSON-RPC
/// `request_id` on the response envelope.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpRouteMessageResult {
    /// JSON-RPC message response from the SDK-hosted MCP server.
    pub message: serde_json::Value,
}
