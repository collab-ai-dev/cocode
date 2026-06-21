//! `SendMessageTool` — deliver a message to a teammate or broadcast.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::FunctionToolSpec;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::SchemaContext;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolInputSchema;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolSpec;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::de;
use serde_json::Value;
use std::sync::OnceLock;

/// Typed input for [`SendMessageTool`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Target agent name, "*" for broadcast, or agent ID
    pub to: String,
    /// Brief summary of the message (5-10 words). Required when
    /// `message` is a plain string (used by the leader UI message
    /// stack); structured messages skip it.
    #[serde(default)]
    pub summary: Option<String>,
    /// Message content: plain text or a structured control response.
    pub message: SendMessagePayload,
}

/// Plain text or structured control payload.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SendMessagePayload {
    Text(String),
    Structured(SendMessageStructuredMessage),
}

/// Structured control messages accepted from the model. Field names match
/// the model-input shape (`request_id`, `approve`), not mailbox wire
/// (`requestId`, `approved`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SendMessageStructuredMessage {
    ShutdownRequest {
        #[serde(default)]
        reason: Option<String>,
    },
    ShutdownResponse {
        request_id: String,
        #[serde(deserialize_with = "deserialize_approve")]
        approve: bool,
        #[serde(default)]
        reason: Option<String>,
    },
    PlanApprovalResponse {
        request_id: String,
        #[serde(deserialize_with = "deserialize_approve")]
        approve: bool,
        #[serde(default)]
        feedback: Option<String>,
    },
}

fn deserialize_approve<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    struct ApproveVisitor;

    impl de::Visitor<'_> for ApproveVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a boolean or the string \"true\"/\"false\"")
        }

        fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
            Ok(value)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            match value {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(E::custom(
                    "approve must be true, false, \"true\", or \"false\"",
                )),
            }
        }
    }

    deserializer.deserialize_any(ApproveVisitor)
}

/// Model-facing prompt (non-UDS variant — `UDS_INBOX` is a dropped feature-gate in coco).
const SEND_MESSAGE_PROMPT: &str = r#"# SendMessage

Send a message to another agent.

```json
{"to": "researcher", "summary": "assign task 1", "message": "start on task #1"}
```

| `to` | |
|---|---|
| `"researcher"` | Teammate by name |
| `"*"` | Broadcast to all teammates — expensive (linear in team size), use only when everyone genuinely needs it |

Your plain text output is NOT visible to other agents — to communicate, you MUST call this tool. Messages from teammates are delivered automatically; you don't check an inbox. Refer to teammates by name, never by UUID. When relaying, don't quote the original — it's already rendered to the user.

## Protocol responses (legacy)

If you receive a JSON message with `type: "shutdown_request"` or `type: "plan_approval_request"`, respond with the matching `_response` type — echo the `request_id`, set `approve` true/false:

```json
{"to": "team-lead", "message": {"type": "shutdown_response", "request_id": "...", "approve": true}}
{"to": "researcher", "message": {"type": "plan_approval_response", "request_id": "...", "approve": false, "feedback": "add error handling"}}
```

Approving shutdown terminates your process. Rejecting plan sends the teammate back to revise. Don't originate `shutdown_request` unless asked. Don't send structured JSON status messages — use TaskUpdate."#;

pub struct SendMessageTool;

#[async_trait::async_trait]
impl Tool for SendMessageTool {
    type Input = SendMessageInput;
    /// Output is `Value` because the wire shape is a tagged union:
    /// bare confirmation string from `agent.send_message` for the
    /// running-agent path, or `{auto_resumed, original_agent_id,
    /// resumed_as, message}` envelope for the terminal-target
    /// auto-resume path.
    type Output = Value;

    fn runtime_validation_schema(&self) -> &ToolInputSchema {
        static SCHEMA: OnceLock<ToolInputSchema> = OnceLock::new();
        SCHEMA.get_or_init(|| ToolInputSchema::from_static_value(send_message_schema(true)))
    }

    async fn tool_spec(
        &self,
        _schema_ctx: &SchemaContext,
        prompt_opts: &PromptOptions,
    ) -> ToolSpec {
        ToolSpec::Function(FunctionToolSpec {
            name: self.name().to_string(),
            description: self.prompt(prompt_opts).await,
            parameters: send_message_schema(false),
            strict: self.strict(),
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::SendMessage)
    }
    fn name(&self) -> &str {
        ToolName::SendMessage.as_str()
    }
    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::AgentTeams)
    }
    fn description(&self, _input: &SendMessageInput, _options: &DescriptionOptions) -> String {
        "Send a message to another agent".into()
    }
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        SEND_MESSAGE_PROMPT.into()
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("send messages to agent teammates (swarm protocol)")
    }

    /// Render either the prebuilt `message` field (auto-resumed path)
    /// or the bare confirmation string returned by `agent.send_message`.
    fn render_for_model(&self, out: &Value) -> Vec<ToolResultContentPart> {
        let text = if let Some(s) = out.as_str() {
            s.to_string()
        } else if let Some(msg) = out.get("message").and_then(Value::as_str) {
            msg.to_string()
        } else {
            serde_json::to_string(out).unwrap_or_default()
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: SendMessageInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        if input.to.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "target agent name or ID ('to') is required".into(),
                error_code: None,
            });
        }

        let content = match &input.message {
            SendMessagePayload::Text(text) => {
                if text.is_empty() {
                    return Err(ToolError::InvalidInput {
                        message: "message content must be non-empty".into(),
                        error_code: None,
                    });
                }
                let summary = input.summary.as_deref().unwrap_or("");
                if summary.is_empty() {
                    return Err(ToolError::InvalidInput {
                        message: "summary is required when sending a plain-text message \
                                  (5-10 word description used by the leader UI)"
                            .into(),
                        error_code: None,
                    });
                }
                text.clone()
            }
            SendMessagePayload::Structured(message) => {
                if input.to == "*" {
                    return Err(ToolError::InvalidInput {
                        message: "structured SendMessage payloads cannot be broadcast".into(),
                        error_code: None,
                    });
                }
                return match message {
                    SendMessageStructuredMessage::ShutdownRequest { reason } => {
                        self.dispatch_shutdown_request(&input.to, reason.as_deref(), ctx)
                            .await
                    }
                    SendMessageStructuredMessage::ShutdownResponse {
                        request_id,
                        approve,
                        reason,
                    } => {
                        self.dispatch_shutdown_response(
                            &input.to,
                            request_id,
                            *approve,
                            reason.as_deref(),
                            ctx,
                        )
                        .await
                    }
                    SendMessageStructuredMessage::PlanApprovalResponse {
                        request_id,
                        approve,
                        feedback,
                    } => {
                        self.dispatch_plan_approval_response(
                            &input.to,
                            request_id,
                            *approve,
                            feedback.as_deref(),
                            ctx,
                        )
                        .await
                    }
                };
            }
        };

        let to = input.to.as_str();

        // Probe the task handle for the target's current state — drives
        // both the auto-resume branch (terminal target) and the
        // pending-message-queue branch (running target).
        let task_status = match ctx.task_handle.as_ref() {
            Some(h) => h.get_task_status(to).await.ok(),
            None => None,
        };

        // When the target is a known background task in a terminal state
        // (Completed / Failed / Killed), auto-resume instead of routing
        // through the team mailbox. The model thinks it's just sending a
        // message; the resume is transparent.
        //
        // Do NOT touch `pendingMessages` on this path — pass the new
        // prompt verbatim to `resumeAgentBackground`. Any prior pending
        // messages stay on the (resumed) task and surface via the
        // `agent_pending_messages` reminder on the next turn. No drain,
        // no prompt-prepend.
        if let Some(info) = task_status.as_ref().filter(|i| i.status.is_terminal()) {
            // Resume needs the parent session id to find the persisted
            // transcript on disk. An empty session id makes the lookup
            // path malformed and surfaces a confusing inner error; reject
            // upfront with a clear message instead.
            let Some(session_id) = ctx
                .session_id_for_history
                .as_deref()
                .filter(|s| !s.is_empty())
            else {
                return Err(ToolError::ExecutionFailed {
                    message: format!(
                        "Agent '{to}' is stopped ({status:?}) and a resume was requested, but \
                         the parent session id is unavailable. Resume is only supported in \
                         persisted sessions — start the session via `coco` (not the \
                         minimal SDK embedding) to enable transcript-backed resume.",
                        status = info.status,
                    ),
                    display_data: None,
                    source: None,
                });
            };
            let resume = ctx
                .agent
                .resume_agent(to, &content, session_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!(
                        "Agent '{to}' is stopped ({status:?}) and could not be resumed: {e}",
                        status = info.status,
                    ),
                    display_data: None,
                    source: None,
                })?;
            let new_id = resume.agent_id.as_deref().unwrap_or(to);
            return Ok(ToolResult {
                data: serde_json::json!({
                    "auto_resumed": true,
                    "original_agent_id": to,
                    "resumed_as": new_id,
                    "message": format!(
                        "Agent '{to}' was stopped ({status:?}); resumed it in the background \
                         with your message. New task id: {new_id}. You'll be notified when it finishes.",
                        status = info.status,
                    ),
                }),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            });
        }

        // Running-agent path: queue the message onto the recipient's
        // per-task `pendingMessages` FIFO so the recipient's next turn
        // sees it as an `agent_pending_messages` system-reminder
        // (`getAgentPendingMessageAttachments` drains and maps to
        // `queued_command` attachments). Routing falls through to the
        // mailbox handle as well so multi-process teammates still see
        // the message via their inbox.
        if let Some(info) = task_status.as_ref()
            && info.status == coco_types::TaskStatus::Running
            && info.task_type() == coco_types::TaskType::BgAgent
        {
            let sender = ctx
                .agent_id
                .as_ref()
                .map(|a| a.as_str().to_string())
                .unwrap_or_else(|| "main".into());
            ctx.pending_messages
                .push(
                    to,
                    coco_tool_runtime::PendingMessage {
                        from: sender,
                        text: content.clone(),
                    },
                )
                .await;
        }

        let result =
            ctx.agent
                .send_message(to, &content)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: e,
                    display_data: None,
                    source: None,
                })?;

        Ok(ToolResult {
            data: serde_json::json!(result),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

impl SendMessageTool {
    /// Leader → teammate: route a structured `shutdown_request` through
    /// the agent handle, which writes a `ShutdownRequest` to the target's
    /// mailbox.
    async fn dispatch_shutdown_request(
        &self,
        to: &str,
        reason: Option<&str>,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let message = ctx.agent.request_shutdown(to, reason).await.map_err(|e| {
            ToolError::ExecutionFailed {
                message: e,
                display_data: None,
                source: None,
            }
        })?;
        Ok(shutdown_result(message))
    }

    /// Teammate → leader: route a structured `shutdown_response` through
    /// the agent handle, which enriches it with the approver's own pane
    /// coordinates and writes `ShutdownApproved` / `ShutdownRejected` to
    /// the team-lead mailbox.
    async fn dispatch_shutdown_response(
        &self,
        to: &str,
        request_id: &str,
        approve: bool,
        reason: Option<&str>,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        // "team-lead" is the canonical leader inbox. Reject any other target.
        if to != "team-lead" {
            return Err(ToolError::InvalidInput {
                message: "shutdown_response must be sent to \"team-lead\"".into(),
                error_code: None,
            });
        }
        if request_id.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "shutdown_response requires a non-empty request_id".into(),
                error_code: None,
            });
        }
        // A rejection MUST carry a reason so the leader (and the worker's own
        // next turn) knows why it declined.
        if !approve && reason.is_none_or(|r| r.trim().is_empty()) {
            return Err(ToolError::InvalidInput {
                message: "reason is required when rejecting a shutdown request".into(),
                error_code: None,
            });
        }
        let message = ctx
            .agent
            .respond_to_shutdown(request_id, approve, reason)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: e,
                display_data: None,
                source: None,
            })?;
        Ok(shutdown_result(message))
    }

    async fn dispatch_plan_approval_response(
        &self,
        to: &str,
        request_id: &str,
        approve: bool,
        feedback: Option<&str>,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        if request_id.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "plan_approval_response requires a non-empty request_id".into(),
                error_code: None,
            });
        }
        let feedback = if approve {
            feedback
        } else {
            Some(feedback.unwrap_or("Plan needs revision"))
        };
        let message = ctx
            .agent
            .respond_to_plan_approval(
                to,
                request_id,
                approve,
                feedback,
                ctx.permission_context.mode,
            )
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: e,
                display_data: None,
                source: None,
            })?;
        Ok(shutdown_result(message))
    }
}

/// Build the `{message}` envelope returned by the shutdown control
/// paths — rendered to the model via [`SendMessageTool::render_for_model`].
fn shutdown_result(message: String) -> ToolResult<Value> {
    ToolResult {
        data: serde_json::json!({ "message": message }),
        new_messages: vec![],
        app_state_patch: None,
        permission_updates: Vec::new(),
        display_data: None,
    }
}

fn send_message_schema(runtime: bool) -> Value {
    fn approve_schema(runtime: bool) -> Value {
        if runtime {
            serde_json::json!({
                "anyOf": [
                    { "type": "boolean" },
                    { "type": "string", "enum": ["true", "false"] }
                ]
            })
        } else {
            serde_json::json!({ "type": "boolean" })
        }
    }

    let shutdown_approve_schema = approve_schema(runtime);
    let plan_approve_schema = approve_schema(runtime);

    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "to": {
                "type": "string",
                "description": "Target agent name, \"*\" for broadcast, or agent ID"
            },
            "summary": {
                "type": "string",
                "description": "Brief summary of the message (5-10 words). Required when message is a plain string."
            },
            "message": {
                "anyOf": [
                    { "type": "string" },
                    {
                        "type": "object",
                        "properties": {
                            "type": { "const": "shutdown_request" },
                            "reason": { "type": "string" }
                        },
                        "required": ["type"]
                    },
                    {
                        "type": "object",
                        "properties": {
                            "type": { "const": "shutdown_response" },
                            "request_id": { "type": "string" },
                            "approve": shutdown_approve_schema,
                            "reason": { "type": "string" }
                        },
                        "required": ["type", "request_id", "approve"]
                    },
                    {
                        "type": "object",
                        "properties": {
                            "type": { "const": "plan_approval_response" },
                            "request_id": { "type": "string" },
                            "approve": plan_approve_schema,
                            "feedback": { "type": "string" }
                        },
                        "required": ["type", "request_id", "approve"]
                    }
                ]
            }
        },
        "required": ["to", "message"]
    })
}
