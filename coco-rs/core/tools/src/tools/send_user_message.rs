//! SendUserMessageTool — sends structured messages to the user with optional file attachments.
//!
//! Status distinguishes normal replies from proactive (unsolicited)
//! updates so the UI can render them differently.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::Feature;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Short model-facing label.
const SEND_USER_MESSAGE_DESCRIPTION: &str = "Send a message to the user";

/// Full model-facing tool prompt.
const SEND_USER_MESSAGE_TOOL_PROMPT: &str = "Send a message the user will read. Text outside this tool is visible in the detail view, but most won't open it — the answer lives here.

`message` supports markdown. `attachments` takes file paths (absolute or cwd-relative) for images, diffs, logs.

`status` labels intent: 'normal' when replying to what they just asked; 'proactive' when you're initiating — a scheduled task finished, a blocker surfaced during background work, you need input on something they haven't asked about. Set it honestly; downstream routing uses it.";

/// Message intent — controls how the UI renders the message.
///
/// Wire format stays lowercase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SendUserMessageStatus {
    /// Direct reply to a user request.
    #[default]
    Normal,
    /// Unsolicited update (proactive surfacing).
    Proactive,
}

/// Typed input for [`SendUserMessageTool`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SendUserMessageInput {
    /// The message for the user. Supports markdown formatting.
    pub message: String,
    /// Optional file paths (absolute or relative to cwd) to attach. Use for
    /// photos, screenshots, diffs, logs, or any file the user should see
    /// alongside your message.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Use 'proactive' when you're surfacing something the user hasn't asked
    /// for and needs to see now — task completion while they're away, a
    /// blocker you hit, an unsolicited status update. Use 'normal' when
    /// replying to something the user just said.
    pub status: SendUserMessageStatus,
}

/// Per-attachment metadata returned by [`SendUserMessageTool::execute`].
///
/// The shape stays close to the legacy `serde_json::json!({...})`
/// envelope so transcript replay across the migration boundary
/// round-trips without surprises.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendUserMessageAttachment {
    pub path: String,
    pub exists: bool,
    /// File size in bytes. Cast from `u64` (`std::fs::Metadata::len`)
    /// per the project's `i64`/`u64` convention; realistic file
    /// sizes never approach `i64::MAX` (~9 EiB).
    pub size: i64,
    pub is_image: bool,
}

/// Typed output for [`SendUserMessageTool::execute`]. Mirrors the legacy JSON
/// envelope (`message`/`status`/`attachments`/`timestamp`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendUserMessageOutput {
    pub message: String,
    pub status: SendUserMessageStatus,
    pub attachments: Vec<SendUserMessageAttachment>,
    /// Millisecond Unix timestamp captured at delivery time, encoded
    /// as a decimal string to avoid floating-point precision loss.
    pub timestamp: String,
}

pub struct SendUserMessageTool;

#[async_trait::async_trait]
impl Tool for SendUserMessageTool {
    type Input = SendUserMessageInput;
    coco_tool_runtime::impl_runtime_schema!(SendUserMessageInput);
    type Output = SendUserMessageOutput;

    fn to_auto_classifier_input(&self, input: &SendUserMessageInput) -> Option<String> {
        Some(input.message.clone())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::SendUserMessage)
    }
    fn name(&self) -> &str {
        ToolName::SendUserMessage.as_str()
    }
    fn search_hint(&self) -> Option<&str> {
        Some("send a message to the user — your primary visible output channel")
    }
    fn description(&self, _input: &SendUserMessageInput, _options: &DescriptionOptions) -> String {
        SEND_USER_MESSAGE_DESCRIPTION.into()
    }
    /// The model's tool description is sourced from `prompt()`, not the
    /// short `description()` UI label.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        SEND_USER_MESSAGE_TOOL_PROMPT.to_string()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::KairosBrief)
    }

    fn is_read_only(&self, _input: &SendUserMessageInput) -> bool {
        true
    }
    /// Side-channel UI delivery — semantically read-only regardless of
    /// input shape; Plan mode keeps SendUserMessage visible.
    fn is_always_read_only(&self) -> bool {
        true
    }

    /// These messages are a side-channel to the user — multiple messages in
    /// the same turn are independent and stamped with their own timestamps.
    fn is_concurrency_safe(&self, _input: &SendUserMessageInput) -> bool {
        true
    }

    /// The model only needs the delivery confirmation; the message body
    /// + attachments + timestamp are TUI/state concerns and would waste
    /// tokens if JSON-stringified for the model.
    fn render_for_model(&self, out: &SendUserMessageOutput) -> Vec<ToolResultContentPart> {
        let suffix = match out.attachments.len() {
            0 => String::new(),
            1 => " (1 attachment included)".to_string(),
            n => format!(" ({n} attachments included)"),
        };
        vec![ToolResultContentPart::Text {
            text: format!("Message delivered to user.{suffix}"),
            provider_options: None,
        }]
    }

    /// #48: reject non-existent / not-a-regular-file / inaccessible attachment
    /// paths up-front (errorCode 1) so the model self-corrects instead of
    /// receiving a false success.
    fn validate_input(
        &self,
        input: &SendUserMessageInput,
        ctx: &ToolUseContext,
    ) -> coco_tool_runtime::ValidationResult {
        use coco_tool_runtime::ValidationResult;
        let resolve_root = ctx
            .cwd_override
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        for raw in &input.attachments {
            let path = if std::path::Path::new(raw).is_absolute() {
                std::path::PathBuf::from(raw)
            } else {
                resolve_root.join(raw)
            };
            match std::fs::metadata(&path) {
                Ok(meta) => {
                    if !meta.is_file() {
                        return ValidationResult::invalid_with_code(
                            format!("Attachment \"{raw}\" is not a regular file."),
                            "1",
                        );
                    }
                }
                Err(e) => {
                    let msg = match e.kind() {
                        std::io::ErrorKind::NotFound => format!(
                            "Attachment \"{raw}\" does not exist. \
                             Current working directory: {}.",
                            resolve_root.display()
                        ),
                        std::io::ErrorKind::PermissionDenied => {
                            format!("Attachment \"{raw}\" is not accessible (permission denied).")
                        }
                        _ => format!("Attachment \"{raw}\" is not accessible ({e})."),
                    };
                    return ValidationResult::invalid_with_code(msg, "1");
                }
            }
        }
        ValidationResult::Valid
    }

    async fn execute(
        &self,
        input: SendUserMessageInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<SendUserMessageOutput>, ToolError> {
        // Defensive empty-string guard. Pre-typed-migration this caught
        // omitted `message`; the typed `Input` now rejects missing fields at
        // deserialize time so this only catches the explicitly-empty `""` case.
        if input.message.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "message parameter is required".into(),
                error_code: None,
            });
        }

        // Resolve attachments. Relative paths resolve against the
        // context cwd override (worktree-isolated subagents) before
        // falling back to the process cwd, so a teammate inside a
        // worktree sees its own files rather than the host process's.
        let resolve_root = ctx
            .cwd_override
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        let mut resolved_attachments: Vec<SendUserMessageAttachment> = Vec::new();
        for path_str in &input.attachments {
            let path = if std::path::Path::new(path_str).is_absolute() {
                std::path::PathBuf::from(path_str)
            } else {
                resolve_root.join(path_str)
            };

            let meta = tokio::fs::metadata(&path).await;
            let exists = meta.is_ok();
            let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);
            let is_image = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    matches!(
                        ext.to_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"
                    )
                });

            resolved_attachments.push(SendUserMessageAttachment {
                path: path.display().to_string(),
                exists,
                size,
                is_image,
            });
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string();

        Ok(ToolResult {
            data: SendUserMessageOutput {
                message: input.message,
                status: input.status,
                attachments: resolved_attachments,
                timestamp,
            },
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}
