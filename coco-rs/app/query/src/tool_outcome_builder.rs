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
    /// Per-session tool-result persistence store. `Some` ⇒ Level 1
    /// persistence is active for this session; the outcome builder
    /// checks `tool.max_result_size_bound()` against the rendered
    /// output and persists when over threshold.
    pub tool_output_store: Option<coco_tool_runtime::ToolOutputStore>,
    /// Optional user message built from permission approval content blocks.
    /// Appended on the success path so the next model turn receives images
    /// or other content supplied alongside the approval.
    pub approval_content_message: Option<Message>,
}

/// Max bytes for any single string value forwarded to hook processes. Matches
/// the Level-1 default bound: hooks never previously saw more than a bounded
/// tool result, and the offload seam's multi-MB retained outputs (Bash) must
/// not silently widen that contract.
const HOOK_STRING_VALUE_CAP: usize = 50_000;

/// Return a copy of `value` with every string longer than `cap` truncated at a
/// char boundary (with a marker), or `None` when nothing exceeds the cap (the
/// common case — zero allocation).
fn cap_value_strings(value: &Value, cap: usize) -> Option<Value> {
    match value {
        Value::String(s) if s.len() > cap => {
            let cut = s.floor_char_boundary(cap);
            Some(Value::String(format!(
                "{}\n[... truncated {} bytes for hook payload ...]",
                &s[..cut],
                s.len() - cut
            )))
        }
        Value::Array(items) => {
            let mut replaced: Option<Vec<Value>> = None;
            for (i, item) in items.iter().enumerate() {
                if let Some(new_item) = cap_value_strings(item, cap) {
                    replaced.get_or_insert_with(|| items.clone())[i] = new_item;
                }
            }
            replaced.map(Value::Array)
        }
        Value::Object(map) => {
            let mut replaced: Option<serde_json::Map<String, Value>> = None;
            for (k, v) in map {
                if let Some(new_v) = cap_value_strings(v, cap) {
                    replaced.get_or_insert_with(|| map.clone())[k.as_str()] = new_v;
                }
            }
            replaced.map(Value::Object)
        }
        _ => None,
    }
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

/// Level-1 per-tool bounding: window + persist an over-threshold text result
/// through the shared offload seam. Infallible — a missing store or a failed
/// write degrades to a pointer-less window (model-visible in the footer),
/// never a tool error. Declared bounds are authoritative (no hidden clamp).
async fn bound_text_for_model(
    store: Option<&coco_tool_runtime::ToolOutputStore>,
    persistence_id: &str,
    content: String,
    is_json: bool,
    declared_bound: coco_tool_runtime::ResultSizeBound,
    inline_window_budget: Option<i64>,
) -> String {
    let Some(threshold) = declared_bound.as_bytes() else {
        return content; // Unbounded — tool opted out.
    };
    if (content.len() as i64) <= threshold
        || coco_tool_runtime::tool_result_storage::is_content_already_persisted(&content)
    {
        return content;
    }

    let key = coco_tool_runtime::ArtifactKey::ToolUse {
        id: persistence_id.to_string(),
        is_json,
    };
    // Tools override `inline_window_budget()` to keep a larger visible window;
    // everything else uses the shared small reference budget. Both are capped
    // by the threshold so the windowed render never re-persists.
    let budget = inline_window_budget
        .map(coco_tool_runtime::InlineBudget::from_request)
        .unwrap_or(coco_tool_runtime::tool_result_offload::REFERENCE_BUDGET)
        .capped_to(threshold);
    coco_tool_runtime::offload_windowed(store, &key, &content, budget)
        .await
        .model_text
}

async fn bound_parts_for_model(
    store: Option<&coco_tool_runtime::ToolOutputStore>,
    tool_use_id: &str,
    output_data: &Value,
    parts: Vec<ToolResultContentPart>,
    declared_bound: coco_tool_runtime::ResultSizeBound,
    inline_window_budget: Option<i64>,
) -> Vec<ToolResultContentPart> {
    let is_json = output_data.is_object() || output_data.is_array();
    let total_text_len = parts
        .iter()
        .filter_map(|part| match part {
            ToolResultContentPart::Text { text, .. } => Some(text.len()),
            _ => None,
        })
        .sum::<usize>();
    // Cheap size pre-check only; the real gate (persisted-check, budget,
    // offload) lives in `bound_text_for_model`.
    let Some(threshold) = declared_bound.as_bytes() else {
        return parts;
    };
    if total_text_len as i64 <= threshold {
        return parts;
    }

    let combined_text = parts
        .iter()
        .filter_map(|part| match part {
            ToolResultContentPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let replacement = bound_text_for_model(
        store,
        &format!("{tool_use_id}-parts-text"),
        combined_text,
        is_json,
        declared_bound,
        inline_window_budget,
    )
    .await;

    let mut inserted_replacement = false;
    parts
        .into_iter()
        .filter_map(|part| match part {
            ToolResultContentPart::Text {
                provider_options, ..
            } if !inserted_replacement => {
                inserted_replacement = true;
                Some(ToolResultContentPart::Text {
                    text: replacement.clone(),
                    provider_options,
                })
            }
            ToolResultContentPart::Text { .. } => None,
            other => Some(other),
        })
        .collect()
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
        tool_output_store,
        approval_content_message,
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
                mut new_messages,
                app_state_patch,
                permission_updates,
                display_data,
            } = tool_result;
            let mut output_data = data;

            // PostToolUse runs on the success branch. Output rewrite is
            // MCP-only, so MCP tools must receive the full envelope; for
            // everything else, bound huge string fields (the offload seam
            // lets Bash retain multi-MB output for artifact recovery) so
            // hook processes keep the pre-offload ≤50K payload contract.
            let hook_output = if is_mcp {
                None
            } else {
                cap_value_strings(&output_data, HOOK_STRING_VALUE_CAP)
            };
            let post = HookController::new(hooks, orchestration_ctx, hook_tx)
                .run_post_tool_use(
                    &tool_name,
                    &tool_use_id,
                    &effective_input,
                    hook_output.as_ref().unwrap_or(&output_data),
                )
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

                    let is_json = output_data.is_object() || output_data.is_array();
                    let rendered_output = bound_text_for_model(
                        tool_output_store.as_ref(),
                        &tool_use_id,
                        rendered_output_raw,
                        is_json,
                        tool.max_result_size_bound(),
                        tool.inline_window_budget(),
                    )
                    .await;

                    create_tool_result_message(
                        &tool_use_id,
                        &tool_name,
                        tool_id.clone(),
                        &rendered_output,
                        tool_result_is_error,
                    )
                }
                None => {
                    let parts = bound_parts_for_model(
                        tool_output_store.as_ref(),
                        &tool_use_id,
                        &output_data,
                        parts,
                        tool.max_result_size_bound(),
                        tool.inline_window_budget(),
                    )
                    .await;
                    create_tool_result_message_with_parts(
                        &tool_use_id,
                        &tool_name,
                        tool_id.clone(),
                        parts,
                        tool_result_is_error,
                    )
                }
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

            if let Some(message) = approval_content_message {
                new_messages.insert(0, message);
            }

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
