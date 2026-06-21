//! Build [`UnstampedToolCallOutcome`] from a single tool call's raw
//! result, running post-hooks and flattening `ToolMessageBuckets`.
//!
//! This is the `run_one` success/failure tail that follows
//! `tool.execute()`. The preparer (`tool_call_preparer.rs`) owns the
//! pre-execution lifecycle (pre-hook → re-validate → permission);
//! everything after the tool returns flows through here.

use std::sync::Arc;

use coco_hooks::HookExecutionEvent;
use coco_hooks::HookRegistry;
use coco_hooks::orchestration::OrchestrationContext;
use coco_messages::Message;
use coco_messages::ToolResult;
use coco_messages::ToolResultContentPart;
use coco_messages::create_error_tool_result;
use coco_messages::create_tool_result_message;
use coco_messages::create_tool_result_message_with_parts;
use coco_system_reminder::AttachmentType as ReminderAttachmentType;
use coco_system_reminder::SystemReminder;
use coco_system_reminder::inject_reminders;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolCallErrorKind;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolMessagePath;
use coco_tool_runtime::ToolSideEffects;
use coco_tool_runtime::UnstampedToolCallOutcome;
use coco_types::ToolDisplayData;
use coco_types::ToolId;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;

use crate::hook_controller::HookController;
use crate::tool_message::ToolMessageBuckets;
use crate::tool_message::ToolMessageOrder;
use crate::tool_message::ToolMessagePath as RunnerMessagePath;

/// Inputs `run_one` feeds into the outcome builder AFTER the
/// preparer has resolved the effective input and permission.
pub(crate) struct RunOneTail<'a> {
    pub tool_use_id: String,
    pub tool_id: ToolId,
    pub tool_name: String,
    pub model_index: usize,
    pub tool: Arc<dyn DynTool>,
    pub effective_input: Value,
    pub execute_result: Result<ToolResult<Value>, coco_tool_runtime::ToolError>,
    pub hooks: Option<&'a Arc<HookRegistry>>,
    pub orchestration_ctx: OrchestrationContext,
    pub hook_tx: Option<&'a mpsc::Sender<HookExecutionEvent>>,
    /// Per-session tool-result persistence root. `Some` ⇒ Level 1
    /// persistence is active for this session; the outcome builder
    /// checks `tool.max_result_size_bound()` against the rendered
    /// output and persists to disk when over threshold. `None` ⇒
    /// Level 1 is disabled (legacy behaviour) and tool results stay
    /// inline. Wired by `tool_call_runner` from the engine's resolved
    /// transcript/session artifact root.
    pub tool_result_session_dir: Option<std::path::PathBuf>,
}

fn plain_text_parts(parts: &[ToolResultContentPart]) -> Option<String> {
    let mut rendered = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ToolResultContentPart::Text {
                text,
                provider_options: None,
            } => rendered.push(text.as_str()),
            _ => return None,
        }
    }
    Some(rendered.join("\n\n"))
}

/// Build an `UnstampedToolCallOutcome` from a completed tool call.
///
/// Runs PostToolUse / PostToolUseFailure hooks, assembles the
/// appropriate `ToolMessageBuckets`, flattens via `ToolMessageOrder`,
/// and packages side-effects into [`ToolSideEffects`] so the
/// scheduler can apply the patch at the correct moment.
pub(crate) async fn build_outcome_from_execution(args: RunOneTail<'_>) -> UnstampedToolCallOutcome {
    let RunOneTail {
        tool_use_id,
        tool_id,
        tool_name,
        model_index,
        tool,
        effective_input,
        execute_result,
        hooks,
        orchestration_ctx,
        hook_tx,
        tool_result_session_dir,
    } = args;
    let is_mcp = tool.is_mcp();
    let order = ToolMessageOrder::for_tool(&*tool);

    match execute_result {
        Ok(tool_result) => {
            // Pull the SDK structured_output before destructuring — the
            // accessor scans `new_messages` for the silent attachment we
            // forward via `ToolResult::with_structured_output`.
            let structured_output = tool_result.structured_output();
            let ToolResult {
                data,
                new_messages,
                app_state_patch,
                permission_updates,
                display_data,
            } = tool_result;
            let mut output_data = data;

            // PostToolUse runs on the success branch. Output rewrite
            // is MCP-only.
            let post = HookController::new(hooks, orchestration_ctx, hook_tx)
                .run_post_tool_use(&tool_name, &tool_use_id, &effective_input, &output_data)
                .await;
            if is_mcp && let Some(updated) = post.updated_mcp_tool_output {
                output_data = updated;
            }

            // Project the tool's structured `data` into model-facing
            // content parts. Default impl returns a singleton Text
            // part with `serde_json::to_string(&data)` — byte-identical
            // to the pre-`render_for_model` codepath. Tools opt into
            // custom rendering (token efficiency, multimodal images)
            // by overriding `Tool::render_for_model`.
            let tool_result_is_error = is_mcp
                && output_data
                    .get("error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            let parts = tool.render_for_model(&output_data);

            // Text-only path: stays on the existing string pipeline,
            // including Tool Result Budget Level-1 persistence and the
            // legacy `create_tool_result_message` call. Singleton text
            // is the path 95% of tools take; multiple plain Text blocks
            // are folded so large MCP text chunks still get Level 1.
            //
            // Multi-part path (image / document / mixed): bypass
            // Level-1 persistence (FileData/FileUrl can't be
            // text-persisted as-is) and create the tool_result via
            // the multi-part sibling. Provider crates downstream
            // already handle `ToolResultContent::Content` —
            // Anthropic / Gemini 3+ pass through, OpenAI /
            // OpenAI-Compatible degrade non-Text parts to a visible
            // marker.
            let text_only_output = plain_text_parts(&parts);
            let mut tool_result_msg = match text_only_output {
                Some(rendered_text) => {
                    let rendered_output_raw = if rendered_text.trim().is_empty() {
                        coco_tool_runtime::tool_result_storage::empty_tool_result_message(
                            &tool_name,
                        )
                    } else {
                        rendered_text
                    };

                    // ── Tool Result Budget Level 1 ──
                    //
                    // When the tool opts into persistence (declared
                    // `max_result_size_bound() == Chars(_)`) AND the rendered
                    // output exceeds `resolve_persistence_threshold(declared)`,
                    // write the body to `<session_dir>/tool-results/<id>.{txt,json}`
                    // and replace the inline content with a `<persisted-output>`
                    // reference message. Failures fall back to inline content
                    // (persistence is best-effort, not gating).
                    let rendered_output = if let Some(sess_dir) = tool_result_session_dir.as_ref() {
                        let resolved =
                            coco_tool_runtime::tool_result_storage::resolve_persistence_threshold(
                                tool.max_result_size_bound(),
                            );
                        let over_threshold = match resolved {
                            coco_tool_runtime::ResultSizeBound::Unbounded => None,
                            coco_tool_runtime::ResultSizeBound::Chars(t) => {
                                ((rendered_output_raw.len() as i64) > t).then_some(t)
                            }
                        };
                        if over_threshold.is_some()
                            && !coco_tool_runtime::tool_result_storage::is_content_already_persisted(
                                &rendered_output_raw,
                            )
                        {
                            let is_json = output_data.is_object() || output_data.is_array();
                            let persist_result =
                                coco_tool_runtime::tool_result_storage::persist_to_disk(
                                    sess_dir,
                                    &tool_use_id,
                                    &rendered_output_raw,
                                    is_json,
                                )
                                .await;
                            match persist_result {
                                Ok(persisted) => {
                                    coco_tool_runtime::tool_result_storage::render_persisted_reference(&persisted)
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        tool = %tool_name,
                                        tool_use_id = %tool_use_id,
                                        "Level 1 tool-result persistence failed; falling back to inline"
                                    );
                                    rendered_output_raw
                                }
                            }
                        } else {
                            rendered_output_raw
                        }
                    } else {
                        rendered_output_raw
                    };

                    create_tool_result_message(
                        &tool_use_id,
                        &tool_name,
                        tool_id.clone(),
                        &rendered_output,
                        tool_result_is_error,
                    )
                }
                None => create_tool_result_message_with_parts(
                    &tool_use_id,
                    &tool_name,
                    tool_id.clone(),
                    parts,
                    tool_result_is_error,
                ),
            };
            if let Some(display_data) = display_data
                && let Message::ToolResult(tr) = &mut tool_result_msg
            {
                tr.display_data = Some(display_data);
            }

            // Collect post-hook additional_contexts into message
            // form. Emit them wrapped via system-reminder so the
            // attachment kind + format match the legacy
            // `tool_result_processor` path.
            let post_hook_msgs = render_hook_context_messages(
                &tool_name,
                &post.additional_contexts,
                ReminderAttachmentType::HookAdditionalContext,
            );

            let prevent_attachment = if post.prevent_continuation {
                let reason = post
                    .stop_reason
                    .clone()
                    .unwrap_or_else(|| "PostToolUse hook stopped continuation".into());
                render_hook_stopped_continuation_message(&tool_name, &reason)
            } else {
                None
            };

            // `with_structured_output` already pushed the silent
            // attachment onto `new_messages`; no re-push here.
            let buckets = ToolMessageBuckets {
                pre_hook: Vec::new(),
                tool_result: Some(tool_result_msg),
                new_messages,
                post_hook: post_hook_msgs,
                prevent_continuation_attachment: prevent_attachment,
                path: RunnerMessagePath::Success,
            };
            let ordered_messages = buckets.flatten(order);

            let prevent_reason = post.prevent_continuation.then(|| {
                post.stop_reason
                    .clone()
                    .unwrap_or_else(|| "PostToolUse hook stopped continuation".into())
            });

            UnstampedToolCallOutcome {
                tool_use_id,
                tool_id,
                model_index,
                ordered_messages,
                message_path: ToolMessagePath::Success,
                error_kind: None,
                permission_denial: None,
                prevent_continuation: prevent_reason,
                structured_output,
                effects: ToolSideEffects {
                    app_state_patch,
                    permission_updates,
                },
            }
        }
        Err(error) => {
            let display_data = display_data_from_tool_error(&error).cloned();
            let error_message = error.to_string();

            // PostToolUseFailure carries `is_interrupt: true` when the failure
            // was a user/runtime cancellation rather than a tool-internal error.
            let is_interrupt = matches!(error, coco_tool_runtime::ToolError::Cancelled);

            // A user/runtime cancellation commits the explicit interrupt
            // message, not the generic "Error: cancelled".
            let rendered_error = if is_interrupt {
                format!("Error: {}", coco_messages::INTERRUPT_MESSAGE_FOR_TOOL_USE)
            } else {
                format!("Error: {error_message}")
            };
            warn!(tool = %tool_name, error = %error, "tool execution failed");

            let post = HookController::new(hooks, orchestration_ctx, hook_tx)
                .run_post_tool_use_failure(
                    &tool_name,
                    &tool_use_id,
                    &effective_input,
                    &error_message,
                    is_interrupt,
                )
                .await;

            let mut tool_result_msg = create_error_tool_result(
                &tool_use_id,
                &tool_name,
                tool_id.clone(),
                &rendered_error,
            );
            if let Some(display_data) = display_data
                && let Message::ToolResult(tr) = &mut tool_result_msg
            {
                // Some failed tools can still provide bounded UI context.
                tr.display_data = Some(display_data);
            }
            let post_hook_msgs = render_hook_context_messages(
                &tool_name,
                &post.additional_contexts,
                ReminderAttachmentType::HookAdditionalContext,
            );

            let buckets = ToolMessageBuckets {
                pre_hook: Vec::new(),
                tool_result: Some(tool_result_msg),
                new_messages: Vec::new(),
                post_hook: post_hook_msgs,
                prevent_continuation_attachment: None,
                path: RunnerMessagePath::Failure,
            };
            let ordered_messages = buckets.flatten(order);

            // Classify cancellation vs other execution errors so the
            // error_kind enum is accurate. A pre-execute turn abort is
            // short-circuited in `run_one` into a PreExecutionCancelled
            // EarlyReturn outcome (no failure hooks), so a `Cancelled`
            // seen here is a genuine MID-execution cancel — kept as
            // ExecutionCancelled, which DOES fire PostToolUseFailure.
            let error_kind = match &error {
                coco_tool_runtime::ToolError::Cancelled => ToolCallErrorKind::ExecutionCancelled,
                _ => ToolCallErrorKind::ExecutionFailed,
            };

            UnstampedToolCallOutcome {
                tool_use_id,
                tool_id,
                model_index,
                ordered_messages,
                message_path: ToolMessagePath::Failure,
                error_kind: Some(error_kind),
                permission_denial: None,
                prevent_continuation: None,
                structured_output: None,
                effects: ToolSideEffects::none(),
            }
        }
    }
}

fn display_data_from_tool_error(error: &ToolError) -> Option<&ToolDisplayData> {
    match error {
        ToolError::ExecutionFailed { display_data, .. } => display_data.as_ref(),
        ToolError::NotFound { .. }
        | ToolError::InvalidInput { .. }
        | ToolError::PermissionDenied { .. }
        | ToolError::Timeout { .. }
        | ToolError::Cancelled => None,
    }
}

/// Build an `UnstampedToolCallOutcome` for an EarlyReturn path —
/// unknown tool, schema failure, validation failure, pre-hook block,
/// permission denial, or a pre-execute turn abort (`run_one` emits this
/// when the turn is already cancelled before the tool runs). The
/// EarlyReturn path skips PostToolUseFailure hooks.
pub(crate) fn build_early_outcome(
    tool_use_id: String,
    tool_id: ToolId,
    tool_name: &str,
    model_index: usize,
    error_kind: ToolCallErrorKind,
    synthetic_message: &str,
    permission_denial: Option<coco_types::PermissionDenialInfo>,
) -> UnstampedToolCallOutcome {
    let tool_result_msg =
        create_error_tool_result(&tool_use_id, tool_name, tool_id.clone(), synthetic_message);
    let buckets = ToolMessageBuckets {
        pre_hook: Vec::new(),
        tool_result: Some(tool_result_msg),
        new_messages: Vec::new(),
        post_hook: Vec::new(),
        prevent_continuation_attachment: None,
        path: RunnerMessagePath::EarlyReturn,
    };
    let ordered_messages = buckets.flatten(ToolMessageOrder::NonMcp);
    UnstampedToolCallOutcome {
        tool_use_id,
        tool_id,
        model_index,
        ordered_messages,
        message_path: ToolMessagePath::EarlyReturn,
        error_kind: Some(error_kind),
        permission_denial,
        prevent_continuation: None,
        structured_output: None,
        effects: ToolSideEffects::none(),
    }
}

/// Wrap hook-provided additional_contexts into reminder-injected
/// `Message::Attachment`s. We inject through `inject_reminders` into
/// a throwaway Vec so the resulting attachment kind / wrap format
/// match the legacy path exactly.
fn render_hook_context_messages(
    hook_name: &str,
    additional_contexts: &[String],
    kind: ReminderAttachmentType,
) -> Vec<Message> {
    if additional_contexts.is_empty() {
        return Vec::new();
    }
    let reminders = additional_contexts
        .iter()
        .map(|ctx| SystemReminder::new(kind, format!("{hook_name} hook additional context: {ctx}")))
        .collect();
    inject_reminders(reminders).model_visible
}

fn render_hook_stopped_continuation_message(hook_name: &str, reason: &str) -> Option<Message> {
    let reminders = vec![SystemReminder::new(
        ReminderAttachmentType::HookStoppedContinuation,
        format!("{hook_name} hook stopped continuation: {reason}"),
    )];
    inject_reminders(reminders).model_visible.into_iter().next()
}

#[cfg(test)]
#[path = "tool_outcome_builder.test.rs"]
mod tests;
