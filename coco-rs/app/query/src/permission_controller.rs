use std::sync::Arc;

use coco_hooks::HookRegistry;
use coco_hooks::orchestration::OrchestrationContext;
use coco_hooks::orchestration::PermissionRequestDecision;
use coco_llm_types::FilePart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::UserContentPart;
use coco_messages::MessageHistory;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_types::CoreEvent;
use coco_types::PermissionDecision;
use coco_types::PermissionDecisionReason;
use coco_types::PermissionDenialInfo;
use coco_types::SessionState;
use coco_types::ToolId;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::helpers::ToolCompletionEventMode;
use crate::helpers::complete_tool_call_clarification_messages;
use crate::helpers::complete_tool_call_with_error_messages_mode;
use crate::helpers::complete_tool_call_with_error_mode;
use crate::session_state::SessionStateTracker;
use coco_types::ToolName;

const PLAN_REJECTION_PREFIX: &str = "The agent proposed a plan that was rejected by the user. \
The user chose to stay in plan mode rather than proceed with implementation.\n\nRejected plan:\n";

pub(crate) enum PermissionOutcome {
    Allow(Box<PermissionAllowOutcome>),
    Denied,
    Aborted,
}

pub(crate) struct PermissionAllowOutcome {
    pub(crate) updated_input: Option<serde_json::Value>,
    pub(crate) approval_feedback: Option<String>,
    pub(crate) resolution_detail: Option<coco_types::PermissionResolutionDetail>,
    pub(crate) approval_content_message: Option<coco_messages::Message>,
}

struct PermissionAskPayload {
    message: String,
    suggestions: Vec<coco_types::PermissionUpdate>,
    choices: Option<Vec<coco_types::PermissionAskChoice>>,
    detail: Option<coco_types::PermissionRequestDetail>,
}

fn allow_outcome(
    updated_input: Option<serde_json::Value>,
    approval_feedback: Option<String>,
    resolution_detail: Option<coco_types::PermissionResolutionDetail>,
    approval_content_message: Option<coco_messages::Message>,
) -> PermissionOutcome {
    PermissionOutcome::Allow(Box::new(PermissionAllowOutcome {
        updated_input,
        approval_feedback,
        resolution_detail,
        approval_content_message,
    }))
}

pub(crate) struct PermissionController<'a> {
    event_tx: &'a Option<mpsc::Sender<CoreEvent>>,
    history: &'a mut MessageHistory,
    permission_denials: &'a mut Vec<PermissionDenialInfo>,
    state_tracker: &'a SessionStateTracker,
    permission_bridge: Option<&'a ToolPermissionBridgeRef>,
    session_id: &'a str,
    cancel: &'a CancellationToken,
    /// Hook registry + orchestration context for firing
    /// `PermissionRequest` hooks before the dialog so hooks can
    /// override the user prompt with allow/deny.
    hooks: Option<&'a Arc<HookRegistry>>,
    orchestration_ctx: Option<&'a OrchestrationContext>,
    cwd: Option<String>,
    completion_event_mode: ToolCompletionEventMode,
    deferred_tool_completions: Option<&'a mut crate::helpers::DeferredToolCompletionBuffer>,
    /// True when the session cannot show an interactive permission prompt.
    /// When set, a residual `Ask` with no permission bridge fails closed
    /// (Deny) rather than silently auto-allowing.
    avoid_permission_prompts: bool,
}

impl<'a> PermissionController<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        event_tx: &'a Option<mpsc::Sender<CoreEvent>>,
        history: &'a mut MessageHistory,
        permission_denials: &'a mut Vec<PermissionDenialInfo>,
        state_tracker: &'a SessionStateTracker,
        permission_bridge: Option<&'a ToolPermissionBridgeRef>,
        session_id: &'a str,
        cancel: &'a CancellationToken,
        hooks: Option<&'a Arc<HookRegistry>>,
        orchestration_ctx: Option<&'a OrchestrationContext>,
        cwd: Option<String>,
        completion_event_mode: ToolCompletionEventMode,
        avoid_permission_prompts: bool,
        deferred_tool_completions: Option<&'a mut crate::helpers::DeferredToolCompletionBuffer>,
    ) -> Self {
        Self {
            event_tx,
            history,
            permission_denials,
            state_tracker,
            permission_bridge,
            session_id,
            cancel,
            hooks,
            orchestration_ctx,
            cwd,
            completion_event_mode,
            deferred_tool_completions,
            avoid_permission_prompts,
        }
    }

    pub(crate) async fn resolve(
        &mut self,
        decision: PermissionDecision,
        tool_call: &ToolCallPart,
        tool_input: &serde_json::Value,
        tool_id: &ToolId,
    ) -> PermissionOutcome {
        match decision {
            PermissionDecision::Allow { updated_input, .. } => {
                allow_outcome(updated_input, None, None, None)
            }
            PermissionDecision::Deny { message, reason } => {
                warn!(tool = tool_call.tool_name, %message, "tool permission denied");
                self.record_denial(tool_call, tool_input);
                if is_auto_mode_classifier_denial(&reason) {
                    let display = recent_denial_display(&tool_call.tool_name, tool_input);
                    let _delivered = crate::emit::emit_tui(
                        self.event_tx,
                        TuiOnlyEvent::AutoModeDenied {
                            tool_name: tool_call.tool_name.clone(),
                            display,
                            reason: message.clone(),
                        },
                    )
                    .await;
                }
                let output = format!("Permission denied: {message}");
                complete_tool_call_with_error_mode(
                    self.event_tx,
                    self.history,
                    &tool_call.tool_call_id,
                    &tool_call.tool_name,
                    tool_id,
                    &output,
                    coco_tool_runtime::ToolCallErrorKind::PermissionDenied,
                    self.completion_event_mode,
                    self.deferred_tool_completions.as_deref_mut(),
                )
                .await;
                PermissionOutcome::Denied
            }
            PermissionDecision::Abort { message, .. } => {
                warn!(tool = tool_call.tool_name, %message, "tool permission aborted");
                let output = format!("Permission aborted: {message}");
                complete_tool_call_with_error_mode(
                    self.event_tx,
                    self.history,
                    &tool_call.tool_call_id,
                    &tool_call.tool_name,
                    tool_id,
                    &output,
                    coco_tool_runtime::ToolCallErrorKind::PermissionBridgeFailed,
                    self.completion_event_mode,
                    self.deferred_tool_completions.as_deref_mut(),
                )
                .await;
                PermissionOutcome::Aborted
            }
            PermissionDecision::Ask {
                message,
                suggestions,
                choices,
                detail,
                ..
            } => {
                self.resolve_ask(
                    tool_call,
                    tool_input,
                    tool_id,
                    PermissionAskPayload {
                        message,
                        suggestions,
                        choices,
                        detail,
                    },
                )
                .await
            }
        }
    }

    async fn resolve_ask(
        &mut self,
        tool_call: &ToolCallPart,
        tool_input: &serde_json::Value,
        tool_id: &ToolId,
        ask: PermissionAskPayload,
    ) -> PermissionOutcome {
        // Transition to RequiresAction while waiting for the approval path,
        // then back to Running when it resolves. No bridge preserves legacy
        // headless auto-allow behavior.
        self.state_tracker
            .transition_to(SessionState::RequiresAction, self.event_tx)
            .await;

        // PermissionRequest hook: fires before the dialog. If the hook
        // returns a `decision` (allow/deny), it short-circuits the
        // prompt entirely.
        if let (Some(registry), Some(ctx)) = (self.hooks, self.orchestration_ctx)
            && !ctx.disable_all_hooks
        {
            let permission_suggestions = serde_json::to_value(&ask.suggestions).ok();
            match coco_hooks::orchestration::execute_permission_request(
                registry,
                ctx,
                &tool_call.tool_name,
                tool_input,
                permission_suggestions.as_ref(),
            )
            .await
            {
                Ok(agg) => {
                    if let Some(decision) = agg.permission_request_result {
                        match decision {
                            PermissionRequestDecision::Allow { updated_input } => {
                                self.state_tracker
                                    .transition_to(SessionState::Running, self.event_tx)
                                    .await;
                                return allow_outcome(updated_input, None, None, None);
                            }
                            PermissionRequestDecision::Deny { message, .. } => {
                                let feedback = message
                                    .unwrap_or_else(|| "Permission denied by hook".to_string());
                                warn!(
                                    tool = tool_call.tool_name,
                                    "PermissionRequest hook denied tool execution"
                                );
                                self.record_denial(tool_call, tool_input);
                                let output = format!("Permission denied: {feedback}");
                                complete_tool_call_with_error_mode(
                                    self.event_tx,
                                    self.history,
                                    &tool_call.tool_call_id,
                                    &tool_call.tool_name,
                                    tool_id,
                                    &output,
                                    coco_tool_runtime::ToolCallErrorKind::PermissionDenied,
                                    self.completion_event_mode,
                                    self.deferred_tool_completions.as_deref_mut(),
                                )
                                .await;
                                self.state_tracker
                                    .transition_to(SessionState::Running, self.event_tx)
                                    .await;
                                return PermissionOutcome::Denied;
                            }
                        }
                    }
                    // No decision → fall through to the dialog as TS
                    // does when `hookSpecificOutput.decision` is absent.
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        tool = tool_call.tool_name,
                        "PermissionRequest hook failed; proceeding with dialog"
                    );
                }
            }
        }

        let Some(bridge) = self.permission_bridge else {
            // No interactive bridge. In a non-interactive (headless / SDK
            // print) session there is no one to prompt, so a residual `Ask`
            // must fail closed — DENY — rather than silently auto-allowing.
            // An interactive session with no bridge keeps the legacy
            // embedded-host permissive fallback.
            if self.avoid_permission_prompts {
                warn!(
                    tool = tool_call.tool_name,
                    "denying tool: interactive approval unavailable in non-interactive session"
                );
                self.record_denial(tool_call, tool_input);
                let output = format!(
                    "Permission to use {} requires interactive approval, which is \
                     unavailable in this non-interactive session.",
                    tool_call.tool_name
                );
                complete_tool_call_with_error_mode(
                    self.event_tx,
                    self.history,
                    &tool_call.tool_call_id,
                    &tool_call.tool_name,
                    tool_id,
                    &output,
                    coco_tool_runtime::ToolCallErrorKind::PermissionDenied,
                    self.completion_event_mode,
                    self.deferred_tool_completions.as_deref_mut(),
                )
                .await;
                self.state_tracker
                    .transition_to(SessionState::Running, self.event_tx)
                    .await;
                return PermissionOutcome::Denied;
            }
            self.state_tracker
                .transition_to(SessionState::Running, self.event_tx)
                .await;
            return allow_outcome(None, None, None, None);
        };

        let request = coco_tool_runtime::ToolPermissionRequest {
            id: format!("approval-{}", uuid::Uuid::new_v4()),
            tool_use_id: tool_call.tool_call_id.clone(),
            agent_id: self.session_id.to_string(),
            tool_name: tool_call.tool_name.clone(),
            description: ask.message,
            input: tool_input.clone(),
            cwd: self.cwd.clone(),
            suggestions: ask.suggestions,
            choices: ask.choices,
            detail: ask.detail,
            // The generic controller can't resolve the coordinator's
            // task-local teammate identity, so it leaves the badge empty.
            // For in-process teammates the leader's permission bridge
            // (`leader_permission::enrich_in_process_worker_badge`) fills it
            // in — it runs inline within the teammate's task-local scope.
            // Cross-process teammates are badged in `leader_permission`.
            worker_badge: None,
        };
        let request_detail = request.detail.clone();

        let bridge_result = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                Err("Turn cancelled while waiting for permission approval".to_string())
            }
            r = bridge.request_permission(request) => r,
        };

        match bridge_result {
            Ok(resolution) => match resolution.decision {
                coco_tool_runtime::ToolPermissionDecision::Approved => {
                    self.state_tracker
                        .transition_to(SessionState::Running, self.event_tx)
                        .await;
                    // Forward `updated_input` from the bridge so
                    // `tool_call_preparer::resolve_effective_input_from_permission`
                    // can substitute it for the original tool input. Used
                    // by `AskUserQuestion` to splice user-selected
                    // `answers` into the tool's data envelope.
                    allow_outcome(
                        resolution.updated_input,
                        resolution.feedback,
                        resolution.detail,
                        content_blocks_to_user_message(resolution.content_blocks.as_deref()),
                    )
                }
                coco_tool_runtime::ToolPermissionDecision::Rejected => {
                    let feedback = resolution
                        .feedback
                        .unwrap_or_else(|| "Permission denied by client".into());
                    let content_blocks_message =
                        content_blocks_to_user_message(resolution.content_blocks.as_deref());
                    // AskUserQuestion's "Chat about this" / "Skip interview"
                    // and EnterPlanMode's "No" are deliberate user redirects,
                    // not security denials. ExitPlanMode rejection is also a
                    // deliberate plan-mode continuation: the model must see
                    // the rejected plan and feedback, not a generic permission
                    // error that implies an unavailable tool.
                    let neutral_feedback =
                        if tool_call.tool_name == ToolName::AskUserQuestion.as_str() {
                            Some(feedback.clone())
                        } else if tool_call.tool_name == ToolName::EnterPlanMode.as_str() {
                            Some("User declined to enter plan mode".to_string())
                        } else if tool_call.tool_name == ToolName::ExitPlanMode.as_str() {
                            exit_plan_rejection_feedback(&request_detail, &feedback)
                        } else {
                            None
                        };
                    if let Some(neutral_feedback) = neutral_feedback {
                        warn!(tool = tool_call.tool_name, "approval bridge: redirected");
                        let mut messages = vec![coco_messages::create_tool_result_message(
                            &tool_call.tool_call_id,
                            &tool_call.tool_name,
                            tool_id.clone(),
                            &neutral_feedback,
                            /*is_error*/ false,
                        )];
                        if let Some(message) = content_blocks_message {
                            messages.push(message);
                        }
                        complete_tool_call_clarification_messages(
                            self.event_tx,
                            self.history,
                            &tool_call.tool_call_id,
                            &tool_call.tool_name,
                            tool_id,
                            &neutral_feedback,
                            self.completion_event_mode,
                            self.deferred_tool_completions.as_deref_mut(),
                            messages,
                        )
                        .await;
                        self.state_tracker
                            .transition_to(SessionState::Running, self.event_tx)
                            .await;
                        return PermissionOutcome::Denied;
                    }
                    warn!(tool = tool_call.tool_name, "approval bridge: rejected");
                    self.record_denial(tool_call, tool_input);
                    let output = format!("Permission denied: {feedback}");
                    let mut messages = vec![coco_messages::create_error_tool_result(
                        &tool_call.tool_call_id,
                        &tool_call.tool_name,
                        tool_id.clone(),
                        &output,
                    )];
                    if let Some(message) = content_blocks_message {
                        messages.push(message);
                    }
                    complete_tool_call_with_error_messages_mode(
                        self.event_tx,
                        self.history,
                        &tool_call.tool_call_id,
                        &tool_call.tool_name,
                        tool_id,
                        &output,
                        coco_tool_runtime::ToolCallErrorKind::PermissionDenied,
                        self.completion_event_mode,
                        self.deferred_tool_completions.as_deref_mut(),
                        messages,
                    )
                    .await;
                    self.state_tracker
                        .transition_to(SessionState::Running, self.event_tx)
                        .await;
                    PermissionOutcome::Denied
                }
                coco_tool_runtime::ToolPermissionDecision::Aborted => {
                    let feedback = resolution
                        .feedback
                        .unwrap_or_else(|| "Permission request aborted by client".into());
                    warn!(tool = tool_call.tool_name, "approval bridge: aborted");
                    let output = format!("Permission aborted: {feedback}");
                    complete_tool_call_with_error_mode(
                        self.event_tx,
                        self.history,
                        &tool_call.tool_call_id,
                        &tool_call.tool_name,
                        tool_id,
                        &output,
                        coco_tool_runtime::ToolCallErrorKind::PermissionBridgeFailed,
                        self.completion_event_mode,
                        self.deferred_tool_completions.as_deref_mut(),
                    )
                    .await;
                    self.state_tracker
                        .transition_to(SessionState::Running, self.event_tx)
                        .await;
                    PermissionOutcome::Aborted
                }
            },
            Err(e) => {
                warn!(
                    error = %e,
                    tool = tool_call.tool_name,
                    "approval bridge failed; aborting permission flow"
                );
                let output = format!("Permission aborted: {e}");
                complete_tool_call_with_error_mode(
                    self.event_tx,
                    self.history,
                    &tool_call.tool_call_id,
                    &tool_call.tool_name,
                    tool_id,
                    &output,
                    coco_tool_runtime::ToolCallErrorKind::PermissionBridgeFailed,
                    self.completion_event_mode,
                    self.deferred_tool_completions.as_deref_mut(),
                )
                .await;
                self.state_tracker
                    .transition_to(SessionState::Running, self.event_tx)
                    .await;
                PermissionOutcome::Aborted
            }
        }
    }

    fn record_denial(&mut self, tool_call: &ToolCallPart, tool_input: &serde_json::Value) {
        self.permission_denials.push(PermissionDenialInfo {
            tool_name: tool_call.tool_name.clone(),
            tool_use_id: tool_call.tool_call_id.clone(),
            tool_input: tool_input.clone(),
        });
    }
}

fn content_blocks_to_user_message(
    blocks: Option<&[serde_json::Value]>,
) -> Option<coco_messages::Message> {
    let parts: Vec<UserContentPart> = blocks?
        .iter()
        .filter_map(content_block_to_user_part)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(coco_messages::create_user_message_with_parts(parts))
    }
}

fn exit_plan_rejection_feedback(
    detail: &Option<coco_types::PermissionRequestDetail>,
    feedback: &str,
) -> Option<String> {
    let Some(coco_types::PermissionRequestDetail::ExitPlanMode { outcome, plan, .. }) = detail
    else {
        return None;
    };
    if !outcome.has_implementation_plan() {
        return None;
    }
    let plan = plan
        .as_deref()
        .filter(|plan| !plan.trim().is_empty())
        .unwrap_or("No plan found");
    let trimmed_feedback = feedback.trim();
    let feedback_suffix = if trimmed_feedback.is_empty() {
        String::new()
    } else {
        format!("\n\nUser feedback:\n{trimmed_feedback}")
    };
    Some(format!("{PLAN_REJECTION_PREFIX}{plan}{feedback_suffix}"))
}

fn content_block_to_user_part(block: &serde_json::Value) -> Option<UserContentPart> {
    match block.get("type").and_then(serde_json::Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(UserContentPart::text),
        Some("image") => {
            let source = block.get("source")?;
            match source.get("type").and_then(serde_json::Value::as_str) {
                Some("base64") => {
                    let data = source.get("data")?.as_str()?;
                    let media_type = source
                        .get("media_type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("image/png");
                    Some(UserContentPart::File(FilePart::from_base64(
                        data, media_type,
                    )))
                }
                Some("url") => {
                    let url = source.get("url")?.as_str()?;
                    let media_type = source
                        .get("media_type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("image/*");
                    Some(UserContentPart::File(FilePart::from_url(url, media_type)))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_auto_mode_classifier_denial(reason: &PermissionDecisionReason) -> bool {
    matches!(
        reason,
        PermissionDecisionReason::Classifier { classifier, .. } if classifier == "auto_mode"
    )
}

fn recent_denial_display(tool_name: &str, tool_input: &serde_json::Value) -> String {
    let summary = coco_types::tool_summary::tool_input_summary(tool_name, tool_input);
    if summary.is_empty() {
        tool_name.to_string()
    } else {
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_mode_classifier_denial_matches_rust_classifier_tag() {
        assert!(is_auto_mode_classifier_denial(
            &PermissionDecisionReason::Classifier {
                classifier: "auto_mode".into(),
                reason: "stage=1 model=test".into(),
            }
        ));
        assert!(!is_auto_mode_classifier_denial(
            &PermissionDecisionReason::Classifier {
                classifier: "other".into(),
                reason: "stage=1 model=test".into(),
            }
        ));
        assert!(!is_auto_mode_classifier_denial(
            &PermissionDecisionReason::User
        ));
    }

    #[test]
    fn recent_denial_display_uses_tool_input_summary() {
        assert_eq!(
            recent_denial_display("Bash", &json!({"command": "rm -rf /tmp/build"})),
            "rm -rf /tmp/build"
        );
        assert_eq!(recent_denial_display("Bash", &json!({})), "Bash");
    }
}
