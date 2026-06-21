use std::sync::Arc;

use coco_hooks::HookRegistry;
use coco_hooks::orchestration::OrchestrationContext;
use coco_hooks::orchestration::PermissionRequestDecision;
use coco_llm_types::ToolCallPart;
use coco_messages::MessageHistory;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_types::CoreEvent;
use coco_types::PermissionDecision;
use coco_types::PermissionDenialInfo;
use coco_types::SessionState;
use coco_types::ToolId;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::helpers::ToolCompletionEventMode;
use crate::helpers::complete_tool_call_clarification;
use crate::helpers::complete_tool_call_with_error_mode;
use crate::session_state::SessionStateTracker;
use coco_types::ToolName;

pub(crate) enum PermissionOutcome {
    Allow {
        updated_input: Option<serde_json::Value>,
        approval_feedback: Option<String>,
        resolution_detail: Option<coco_types::PermissionResolutionDetail>,
    },
    Denied,
    Aborted,
}

struct PermissionAskPayload {
    message: String,
    suggestions: Vec<coco_types::PermissionUpdate>,
    choices: Option<Vec<coco_types::PermissionAskChoice>>,
    detail: Option<coco_types::PermissionRequestDetail>,
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
            PermissionDecision::Allow { updated_input, .. } => PermissionOutcome::Allow {
                updated_input,
                approval_feedback: None,
                resolution_detail: None,
            },
            PermissionDecision::Deny { message, .. } => {
                warn!(tool = tool_call.tool_name, %message, "tool permission denied");
                self.record_denial(tool_call, tool_input);
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
                                return PermissionOutcome::Allow {
                                    updated_input,
                                    approval_feedback: None,
                                    resolution_detail: None,
                                };
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
            return PermissionOutcome::Allow {
                updated_input: None,
                approval_feedback: None,
                resolution_detail: None,
            };
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
                    PermissionOutcome::Allow {
                        updated_input: resolution.updated_input,
                        approval_feedback: resolution.feedback,
                        resolution_detail: resolution.detail,
                    }
                }
                coco_tool_runtime::ToolPermissionDecision::Rejected => {
                    let feedback = resolution
                        .feedback
                        .unwrap_or_else(|| "Permission denied by client".into());
                    // AskUserQuestion's "Chat about this" / "Skip interview" reach
                    // here as `approved: false` + feedback. That is a deliberate
                    // user REDIRECT, not a permission denial: render the feedback
                    // as a neutral tool result (no red "Permission denied:" prefix)
                    // and do NOT count it as a denial. The model still gets the
                    // feedback and re-engages.
                    if tool_call.tool_name == ToolName::AskUserQuestion.as_str() {
                        warn!(tool = tool_call.tool_name, "approval bridge: clarify");
                        complete_tool_call_clarification(
                            self.event_tx,
                            self.history,
                            &tool_call.tool_call_id,
                            &tool_call.tool_name,
                            tool_id,
                            &feedback,
                            self.completion_event_mode,
                            self.deferred_tool_completions.as_deref_mut(),
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
