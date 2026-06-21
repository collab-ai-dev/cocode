use crate::AppStatePatch;
use crate::PermissionUpdate;
use crate::ToolDisplayData;
use serde_json::Value;

use super::AttachmentMessage;
use super::Message;
use super::SilentPayload;
use super::StructuredOutputPayload;
use crate::AttachmentKind;

/// Result of a tool execution.
///
/// **Effect channels**:
/// - `app_state_patch` — a queued mutation of shared `ToolAppState`
///   that the executor applies post-execute (serial) or post-batch
///   (concurrent). Tools MUST NOT mutate shared `ToolAppState` inline
///   during `execute()` — `ToolUseContext.app_state` is an
///   [`AppStateReadHandle`](crate::AppStateReadHandle) with no
///   `.write()` method, so the compiler enforces the discipline.
/// - `permission_updates` — declarative permission-rule deltas
///   (typically `PermissionUpdate::AddRules` with destination
///   `Command` for skill `allowed-tools`). Applied via the executor's
///   permission-rule handle so subsequent turns see the new rules.
///   TS parity: `SkillTool` returns a `contextModifier` that wraps
///   `getAppState` to inject `alwaysAllowRules.command`.
///
/// **SDK structured output**: tools surface `structured_output` by
/// pushing an `AttachmentMessage::silent_structured_output(...)`
/// attachment onto [`Self::new_messages`]. The tool-outcome builder
/// then forwards the payload data to the SDK result side-channel.
/// There is no dedicated field — that would force every literal
/// constructor to repeat `structured_output: None,`. Use
/// [`Self::with_structured_output`] for ergonomic construction.
///
/// **UI display data**: tools that have compact, typed display-only
/// context can attach it via [`Self::with_display_data`]. The
/// tool-outcome builder copies it onto `ToolResultMessage.display_data`;
/// it must not replace model-facing output rendered from `data`.
///
/// TS parity: `orchestration.ts:queuedContextModifiers` — per-tool
/// `(ctx) => newCtx` modifiers keyed by `tool_use_id` and applied
/// after the concurrent batch finishes.
///
/// Not `Clone` / `Serialize` / `Deserialize`: the `app_state_patch`
/// closure can't participate in those traits. `ToolResult<T>` is
/// always consumed by the executor (applied + converted to
/// `Message::ToolResult`); no call path clones or serializes the
/// whole struct.
pub struct ToolResult<T> {
    pub data: T,
    pub new_messages: Vec<Message>,
    /// Queued mutation of shared app_state. `None` for tools that
    /// don't need to mutate (the overwhelming majority — only
    /// `EnterPlanMode` / `ExitPlanMode` currently return a patch).
    pub app_state_patch: Option<AppStatePatch>,
    /// Declarative permission-rule deltas. Empty for the overwhelming
    /// majority of tools — only the `SkillTool` populates this today,
    /// to forward a skill's frontmatter `allowed-tools` as Command-source
    /// auto-allow rules.
    pub permission_updates: Vec<PermissionUpdate>,
    /// Typed UI-only data for renderers. The tool's `data` remains the
    /// source for model-facing stdout/stderr or structured output.
    pub display_data: Option<ToolDisplayData>,
}

impl<T> ToolResult<T> {
    /// Shorthand: plain data result, no extra messages, no app_state
    /// mutation. Matches the 90%+ of tool call sites.
    pub fn data(data: T) -> Self {
        Self {
            data,
            new_messages: Vec::new(),
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        }
    }

    /// Construct with data + extra messages and no mutation.
    pub fn with_messages(data: T, new_messages: Vec<Message>) -> Self {
        Self {
            data,
            new_messages,
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        }
    }

    /// Attach a post-execute app_state patch. Consumes and returns
    /// the result fluently.
    pub fn with_patch(mut self, patch: AppStatePatch) -> Self {
        self.app_state_patch = Some(patch);
        self
    }

    /// Attach SDK-facing structured output. Materialized as a silent
    /// `AttachmentMessage` on `new_messages`; the tool-outcome builder
    /// forwards the payload to the SDK result side-channel.
    pub fn with_structured_output(mut self, structured_output: Value) -> Self {
        self.new_messages.push(Message::Attachment(
            AttachmentMessage::silent_structured_output(StructuredOutputPayload {
                data: structured_output,
            }),
        ));
        self
    }

    /// Attach permission-rule deltas the executor should fold into
    /// the running session config after this tool returns.
    pub fn with_permission_updates(mut self, updates: Vec<PermissionUpdate>) -> Self {
        self.permission_updates = updates;
        self
    }

    /// Attach typed UI-only display data to the eventual tool result message.
    pub fn with_display_data(mut self, display_data: ToolDisplayData) -> Self {
        self.display_data = Some(display_data);
        self
    }

    /// Extract the SDK-facing structured_output payload (if any) emitted
    /// via [`Self::with_structured_output`] or pushed manually onto
    /// `new_messages`. Returns the data clone of the most-recent
    /// matching attachment so multiple writes in one result behave
    /// last-writer-wins, matching TS `toolExecution.ts:1272`.
    pub fn structured_output(&self) -> Option<Value> {
        self.new_messages.iter().rev().find_map(|msg| match msg {
            Message::Attachment(att) if att.kind == AttachmentKind::StructuredOutput => {
                match &att.body {
                    super::AttachmentBody::Silent(SilentPayload::StructuredOutput(payload)) => {
                        Some(payload.data.clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        })
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ToolResult<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolResult")
            .field("data", &self.data)
            .field("new_messages", &self.new_messages)
            .field(
                "app_state_patch",
                &self.app_state_patch.as_ref().map(|_| "<fn>"),
            )
            .field("permission_updates", &self.permission_updates)
            .field("display_data", &self.display_data)
            .finish()
    }
}
